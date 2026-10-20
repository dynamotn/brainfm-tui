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

    let (p, mut events) = player::spawn(tokio::runtime::Handle::current()).unwrap();
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
