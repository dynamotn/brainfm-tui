//! The audio thread: owns the output device and a rodio [`Player`] queue.
//!
//! Tracks stream into memory through stream-download; nothing touches disk.
//! Opening a stream can stall on the network, so it happens on a helper
//! thread and the finished source is handed back tagged with a generation;
//! anything superseded in the meantime is dropped.

use std::collections::VecDeque;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use rodio::source::SineWave;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use stream_download::http::HttpStream;
use stream_download::storage::memory::MemoryStorageProvider;
use stream_download::{Settings, StreamDownload};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::{PlayerCommand, PlayerEvent, near_end};

const FADE_IN: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(100);
const POSITION_EVERY: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);

type TrackSource = Decoder<StreamDownload<MemoryStorageProvider>>;

pub(super) enum Msg {
    Command(PlayerCommand),
    Opened {
        generation: u64,
        replace: bool,
        id: String,
        length: Duration,
        result: anyhow::Result<TrackSource>,
    },
}

struct Entry {
    id: String,
    length: Duration,
    near_end_sent: bool,
}

struct Engine {
    rt: tokio::runtime::Handle,
    http: reqwest::Client,
    sink: MixerDeviceSink,
    player: Player,
    chime: Player,
    volume: f32,
    queue: VecDeque<Entry>,
    /// Bumped by Load and Stop; stale opened sources are discarded.
    generation: u64,
    msgs: mpsc::Sender<Msg>,
    events: UnboundedSender<PlayerEvent>,
    last_position: Instant,
}

pub(super) fn start(
    rt: tokio::runtime::Handle,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
) -> anyhow::Result<UnboundedReceiver<PlayerEvent>> {
    let (ev_tx, ev_rx) = unbounded_channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("brainfm-audio".into())
        .spawn(move || match Engine::open(rt, tx, ev_tx) {
            Ok(mut engine) => {
                let _ = ready_tx.send(Ok(()));
                engine.run(rx);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .context("spawning audio thread")?;
    ready_rx
        .recv()
        .map_err(|_| anyhow!("audio thread exited during startup"))??;
    Ok(ev_rx)
}

fn open_source(
    rt: &tokio::runtime::Handle,
    http: reqwest::Client,
    url: &str,
) -> anyhow::Result<TrackSource> {
    let url: reqwest::Url = url.parse().context("invalid track url")?;
    let reader = rt
        .block_on(async move {
            // The timer must be created inside the runtime context.
            tokio::time::timeout(OPEN_TIMEOUT, async move {
                let stream = HttpStream::new(http, url)
                    .await
                    .map_err(|e| anyhow!("could not start stream: {e}"))?;
                StreamDownload::from_stream(stream, MemoryStorageProvider, Settings::default())
                    .await
                    .map_err(|e| anyhow!("could not start stream: {e}"))
            })
            .await
        })
        .map_err(|_| anyhow!("timed out connecting to the audio server"))??;
    Decoder::new(reader).context("could not decode track")
}

impl Engine {
    fn open(
        rt: tokio::runtime::Handle,
        msgs: mpsc::Sender<Msg>,
        events: UnboundedSender<PlayerEvent>,
    ) -> anyhow::Result<Self> {
        let mut sink = DeviceSinkBuilder::from_default_device()
            .context("no audio output device")?
            .with_error_callback(|e| tracing::warn!("audio stream error: {e}"))
            .open_sink_or_fallback()
            .context("could not open the audio output device")?;
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        let chime = Player::connect_new(sink.mixer());
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .context("building audio http client")?;
        Ok(Self {
            rt,
            http,
            sink,
            player,
            chime,
            volume: 0.7,
            queue: VecDeque::new(),
            generation: 0,
            msgs,
            events,
            last_position: Instant::now(),
        })
    }

    fn emit(&self, e: PlayerEvent) {
        let _ = self.events.send(e);
    }

    fn run(&mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            match rx.recv_timeout(POLL) {
                Ok(Msg::Command(cmd)) => self.handle(cmd),
                Ok(Msg::Opened {
                    generation,
                    replace,
                    id,
                    length,
                    result,
                }) => {
                    if generation == self.generation {
                        self.opened(replace, id, length, result);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.poll();
        }
    }

    /// Opens a stream on a helper thread; the result comes back as [`Msg::Opened`].
    fn open_async(&self, replace: bool, id: String, url: String, length: Duration) {
        let (rt, http, tx, generation) = (
            self.rt.clone(),
            self.http.clone(),
            self.msgs.clone(),
            self.generation,
        );
        let spawned = std::thread::Builder::new()
            .name("brainfm-open".into())
            .spawn(move || {
                let result = open_source(&rt, http, &url);
                let _ = tx.send(Msg::Opened {
                    generation,
                    replace,
                    id,
                    length,
                    result,
                });
            });
        if let Err(e) = spawned {
            tracing::warn!("could not spawn stream opener: {e}");
        }
    }

    /// Drops whatever is playing without blocking on the audio callback.
    fn reset_player(&mut self) {
        self.player = Player::connect_new(self.sink.mixer());
        self.player.set_volume(self.volume);
        self.queue.clear();
    }

    fn opened(
        &mut self,
        replace: bool,
        id: String,
        length: Duration,
        result: anyhow::Result<TrackSource>,
    ) {
        if !replace && self.queue.is_empty() {
            // Nothing is playing for it to follow (stopped or ended meanwhile).
            return;
        }
        let src = match result {
            Ok(src) => src,
            Err(e) => {
                self.emit(PlayerEvent::Error {
                    id,
                    message: format!("{e:#}"),
                });
                return;
            }
        };
        let entry = Entry {
            id: id.clone(),
            length,
            near_end_sent: false,
        };
        if replace {
            self.reset_player();
            self.player.append(src.fade_in(FADE_IN));
            self.queue.push_back(entry);
            self.emit(PlayerEvent::Started { id });
        } else {
            self.player.append(src);
            self.queue.push_back(entry);
        }
    }

    fn handle(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Load { id, url, length } => {
                self.generation += 1;
                self.open_async(true, id, url, length);
            }
            PlayerCommand::Queue { id, url, length } => self.open_async(false, id, url, length),
            PlayerCommand::CancelPending => self.generation += 1,
            PlayerCommand::Play => self.player.play(),
            PlayerCommand::Pause => self.player.pause(),
            PlayerCommand::Stop { fade } => {
                self.generation += 1;
                self.fade_out(fade);
                self.reset_player();
            }
            PlayerCommand::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.player.set_volume(self.volume);
            }
            PlayerCommand::Chime => {
                let tone = |hz: f32| {
                    SineWave::new(hz)
                        .take_duration(Duration::from_millis(220))
                        .fade_out(Duration::from_millis(200))
                        .amplify(0.25)
                };
                self.chime.append(tone(880.0));
                self.chime.append(tone(660.0));
                self.chime.play();
            }
        }
    }

    fn fade_out(&self, fade: Duration) {
        if self.player.is_paused() || self.queue.is_empty() || fade.is_zero() {
            return;
        }
        let steps = 20u32;
        for i in (0..steps).rev() {
            self.player
                .set_volume(self.volume * i as f32 / steps as f32);
            std::thread::sleep(fade / steps);
        }
    }

    fn poll(&mut self) {
        // The player drops finished sources; anything we track beyond its
        // length has ended.
        while self.queue.len() > self.player.len() {
            let done = self.queue.pop_front().expect("non-empty");
            self.emit(PlayerEvent::Ended { id: done.id });
            if let Some(next) = self.queue.front() {
                self.emit(PlayerEvent::Started {
                    id: next.id.clone(),
                });
            }
        }
        if self.player.is_paused() {
            return;
        }
        let pos = self.player.get_pos();
        let Some(cur) = self.queue.front_mut() else {
            return;
        };
        if near_end(cur.length, pos, cur.near_end_sent) {
            cur.near_end_sent = true;
            let id = cur.id.clone();
            self.emit(PlayerEvent::NearEnd { id });
        }
        if self.last_position.elapsed() >= POSITION_EVERY {
            self.last_position = Instant::now();
            let id = self.queue[0].id.clone();
            self.emit(PlayerEvent::Position { id, pos });
        }
    }
}
