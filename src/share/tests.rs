use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::watch;

use super::*;
use crate::app::state::{AppState, Overlay};
use crate::config::Config;

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn socket_lives_in_the_runtime_dir() {
    let p = socket_path_in(
        Some("/run/user/1000".into()),
        std::path::Path::new("/fallback"),
    );
    assert_eq!(
        p,
        std::path::PathBuf::from("/run/user/1000/brainfm/tui.sock")
    );
    let p = socket_path_in(None, std::path::Path::new("/home/u/.local/state/brainfm"));
    assert_eq!(
        p,
        std::path::PathBuf::from("/home/u/.local/state/brainfm/tui.sock")
    );
}

#[test]
fn quit_keys_stay_local_but_typing_q_in_login_does_not() {
    let s = AppState::new(Config::default(), true, Some("u".into()));
    assert!(is_local_quit(&s, &key('q')));
    assert!(is_local_quit(
        &s,
        &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));
    assert!(!is_local_quit(&s, &key('n')));
    let mut login = s.clone();
    login.overlay = Some(Overlay::login());
    assert!(
        !is_local_quit(&login, &key('q')),
        "q is a letter in the login form"
    );
    assert!(is_local_quit(
        &login,
        &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));
}

#[tokio::test]
async fn a_client_mirrors_host_state_and_forwards_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brainfm/tui.sock");
    let (state_tx, state_rx) = watch::channel(String::new());
    let (actions_tx, mut actions) = unbounded_channel();
    let host = Host::bind(&path).unwrap();
    tokio::spawn(host.serve(state_rx, actions_tx));

    let mut s = AppState::new(Config::default(), true, Some("u".into()));
    s.config.volume = 42;
    state_tx.send(serde_json::to_string(&s).unwrap()).unwrap();

    let Attach::Session(mut client) = attach(&path).await.unwrap() else {
        panic!("expected a session");
    };
    let got = tokio::time::timeout(Duration::from_secs(5), client.next_state())
        .await
        .unwrap()
        .unwrap()
        .expect("a state");
    assert_eq!(got.config.volume, 42);

    client.send_key(key('n')).await.unwrap();
    let action = tokio::time::timeout(Duration::from_secs(5), actions.recv())
        .await
        .unwrap();
    assert!(
        matches!(action, Some(crate::app::update::Action::RemoteKey(k)) if k.code == KeyCode::Char('n'))
    );

    s.config.volume = 7;
    state_tx.send(serde_json::to_string(&s).unwrap()).unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), client.next_state())
        .await
        .unwrap()
        .unwrap()
        .expect("an update");
    assert_eq!(got.config.volume, 7);
}

#[tokio::test]
async fn no_host_means_no_session() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        attach(&dir.path().join("tui.sock")).await.unwrap(),
        Attach::NoHost
    ));
}

#[tokio::test]
async fn a_stale_socket_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    assert!(matches!(attach(&path).await.unwrap(), Attach::NoHost));
    Host::bind(&path).expect("stale socket is cleaned up");
}

#[tokio::test]
async fn a_busy_host_turns_clients_away_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let host = Host::bind(&path).unwrap();
    tokio::spawn(host.refuse("brainfm play is running"));
    match attach(&path).await.unwrap() {
        Attach::Busy(reason) => assert!(reason.contains("brainfm play")),
        _ => panic!("expected Busy"),
    }
}
