//! Plays two short generated mp3s through the real audio device.
//! Needs ffmpeg and a sound card: `cargo test --test player_smoke -- --ignored`.

use std::process::Command;
use std::time::Duration;

use brainfm::player::{self, PlayerCommand, PlayerEvent};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sine_mp3(hz: u32, secs: u32) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency={hz}:duration={secs}"))
        .args(["-f", "mp3", "-"])
        .output()
        .expect("ffmpeg");
    assert!(out.status.success());
    out.stdout
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs ffmpeg and an audio device"]
async fn streams_and_continues_gaplessly() {
    let server = MockServer::start().await;
    for (name, hz) in [("a.mp3", 440), ("b.mp3", 660)] {
        Mock::given(path(format!("/{name}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/mpeg")
                    .set_body_bytes(sine_mp3(hz, 3)),
            )
            .mount(&server)
            .await;
    }

    let (p, mut events) = player::spawn_insecure(tokio::runtime::Handle::current()).unwrap();
    p.send(PlayerCommand::SetVolume(0.05));
    let len = Duration::from_secs(3);
    p.send(PlayerCommand::Load {
        id: "a".into(),
        url: format!("{}/a.mp3", server.uri()),
        length: len,
    });

    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, events.recv()).await {
        match ev {
            PlayerEvent::Position { .. } => continue,
            PlayerEvent::NearEnd { ref id } if id == "a" => {
                p.send(PlayerCommand::Queue {
                    id: "b".into(),
                    url: format!("{}/b.mp3", server.uri()),
                    length: len,
                });
            }
            PlayerEvent::Ended { ref id } if id == "b" => {
                seen.push(ev);
                break;
            }
            _ => {}
        }
        seen.push(ev);
    }
    let ids: Vec<String> = seen
        .iter()
        .map(|e| match e {
            PlayerEvent::Started { id } => format!("start:{id}"),
            PlayerEvent::NearEnd { id } => format!("near:{id}"),
            PlayerEvent::Ended { id } => format!("end:{id}"),
            PlayerEvent::Error { message, .. } => format!("error:{message}"),
            PlayerEvent::Position { .. } => unreachable!(),
        })
        .collect();
    assert_eq!(
        ids,
        ["start:a", "near:a", "end:a", "start:b", "near:b", "end:b"]
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs ffmpeg and an audio device"]
async fn stalled_stream_does_not_block_next_load() {
    let server = MockServer::start().await;
    Mock::given(path("/stall.mp3"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_mins(2)))
        .mount(&server)
        .await;
    Mock::given(path("/ok.mp3"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(sine_mp3(440, 2)),
        )
        .mount(&server)
        .await;

    let (p, mut events) = player::spawn_insecure(tokio::runtime::Handle::current()).unwrap();
    p.send(PlayerCommand::SetVolume(0.05));
    let len = Duration::from_secs(2);
    p.send(PlayerCommand::Load {
        id: "stall".into(),
        url: format!("{}/stall.mp3", server.uri()),
        length: len,
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    p.send(PlayerCommand::Load {
        id: "ok".into(),
        url: format!("{}/ok.mp3", server.uri()),
        length: len,
    });

    let started = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Some(PlayerEvent::Started { id }) => break id,
                Some(_) => {}
                None => panic!("engine gone"),
            }
        }
    })
    .await
    .expect("second Load must not wait behind the stalled one");
    assert_eq!(started, "ok");
    p.send(PlayerCommand::Stop {
        fade: Duration::ZERO,
    });
}

fn audio_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("linux /proc")
        .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
        .filter(|c| c.trim() == "brainfm-audio")
        .count()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an audio device"]
async fn dropping_the_handle_stops_the_engine() {
    let before = audio_threads();
    let (p, events) = player::spawn_insecure(tokio::runtime::Handle::current()).unwrap();
    assert_eq!(audio_threads(), before + 1);
    drop((p, events));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(audio_threads(), before);
}
