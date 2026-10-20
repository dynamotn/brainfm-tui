//! The audio thread: owns the output device and a rodio [`Player`] queue.
//!
//! Tracks stream into memory through stream-download; nothing touches disk.

use std::collections::VecDeque;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use rodio::source::SineWave;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use stream_download::storage::memory::MemoryStorageProvider;
use stream_download::{Settings, StreamDownload};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::{PlayerCommand, PlayerEvent, near_end};

const FADE_IN: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(100);
const POSITION_EVERY: Duration = Duration::from_millis(500);

struct Entry {
    id: String,
    length: Duration,
    near_end_sent: bool,
}

struct Engine {
    rt: tokio::runtime::Handle,
    _sink: MixerDeviceSink,
    player: Player,
    chime: Player,
    volume: f32,
    queue: VecDeque<Entry>,
    events: UnboundedSender<PlayerEvent>,
    last_position: Instant,
}

pub(super) fn start(
    rt: tokio::runtime::Handle,
    commands: mpsc::Receiver<PlayerCommand>,
) -> anyhow::Result<UnboundedReceiver<PlayerEvent>> {
    let (ev_tx, ev_rx) = unbounded_channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("brainfm-audio".into())
        .spawn(move || match Engine::open(rt, ev_tx) {
            Ok(mut engine) => {
                let _ = ready_tx.send(Ok(()));
                engine.run(commands);
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

impl Engine {
    fn open(
        rt: tokio::runtime::Handle,
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
        Ok(Self {
            rt,
            _sink: sink,
            player,
            chime,
            volume: 0.7,
            queue: VecDeque::new(),
            events,
            last_position: Instant::now(),
        })
    }

    fn emit(&self, e: PlayerEvent) {
        let _ = self.events.send(e);
    }

    fn run(&mut self, commands: mpsc::Receiver<PlayerCommand>) {
        loop {
            match commands.recv_timeout(POLL) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.poll();
        }
    }

    fn open_source(
        &self,
        url: &str,
    ) -> anyhow::Result<Decoder<StreamDownload<MemoryStorageProvider>>> {
        let url = url.parse().context("invalid track url")?;
        let reader = self
            .rt
            .block_on(StreamDownload::new_http(
                url,
                MemoryStorageProvider,
                Settings::default(),
            ))
            .map_err(|e| anyhow!("could not start stream: {e}"))?;
        Decoder::new(reader).context("could not decode track")
    }

    fn handle(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Load { id, url, length } => {
                self.player.clear();
                self.queue.clear();
                match self.open_source(&url) {
                    Ok(src) => {
                        self.player.set_volume(self.volume);
                        self.player.append(src.fade_in(FADE_IN));
                        self.player.play();
                        self.queue.push_back(Entry {
                            id: id.clone(),
                            length,
                            near_end_sent: false,
                        });
                        self.emit(PlayerEvent::Started { id });
                    }
                    Err(e) => self.emit(PlayerEvent::Error {
                        id,
                        message: format!("{e:#}"),
                    }),
                }
            }
            PlayerCommand::Queue { id, url, length } => match self.open_source(&url) {
                Ok(src) => {
                    self.player.append(src);
                    self.queue.push_back(Entry {
                        id,
                        length,
                        near_end_sent: false,
                    });
                }
                Err(e) => self.emit(PlayerEvent::Error {
                    id,
                    message: format!("{e:#}"),
                }),
            },
            PlayerCommand::Play => self.player.play(),
            PlayerCommand::Pause => self.player.pause(),
            PlayerCommand::Stop { fade } => {
                self.fade_out(fade);
                self.player.stop();
                self.player.set_volume(self.volume);
                self.queue.clear();
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
