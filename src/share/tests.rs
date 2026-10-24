use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::watch;

use super::*;
use crate::app::state::{AppState, Overlay};
use crate::app::update::Action;
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
    let action = loop {
        let a = tokio::time::timeout(Duration::from_secs(5), actions.recv())
            .await
            .unwrap();
        if !matches!(a, Some(Action::Views(_))) {
            break a;
        }
    };
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
    let (stop_tx, _stop_rx) = unbounded_channel();
    tokio::spawn(host.refuse("brainfm play is running", stop_tx));
    match attach(&path).await.unwrap() {
        Attach::Busy(reason) => assert!(reason.contains("brainfm play")),
        _ => panic!("expected Busy"),
    }
}

type Actions = tokio::sync::mpsc::UnboundedReceiver<Action>;

fn serve_at(path: &Path) -> (watch::Sender<String>, Actions) {
    let (state_tx, state_rx) = watch::channel(String::new());
    let (actions_tx, actions) = unbounded_channel();
    tokio::spawn(Host::bind(path).unwrap().serve(state_rx, actions_tx));
    (state_tx, actions)
}

async fn next(actions: &mut Actions) -> Action {
    tokio::time::timeout(Duration::from_secs(5), actions.recv())
        .await
        .expect("an action in time")
        .expect("the channel is open")
}

#[tokio::test]
async fn stop_is_accepted_without_hello() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (_state_tx, mut actions) = serve_at(&path);
    let mut raw = UnixStream::connect(&path).await.unwrap();
    // The frozen wire shape, written literally.
    raw.write_all(b"{\"t\":\"Stop\"}\n").await.unwrap();
    assert!(matches!(next(&mut actions).await, Action::Shutdown));
}

#[tokio::test]
async fn stop_is_accepted_from_another_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (_state_tx, mut actions) = serve_at(&path);
    let mut raw = UnixStream::connect(&path).await.unwrap();
    raw.write_all(
        line(&ToHost::Hello {
            version: "0.0.0".into(),
        })
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut answer = String::new();
    BufReader::new(&mut raw)
        .read_line(&mut answer)
        .await
        .unwrap();
    assert!(answer.contains("Busy"), "{answer}");
    assert!(answer.contains("brainfm stop"), "{answer}");
    let mut second = UnixStream::connect(&path).await.unwrap();
    second.write_all(b"{\"t\":\"Stop\"}\n").await.unwrap();
    assert!(matches!(next(&mut actions).await, Action::Shutdown));
}

#[tokio::test]
async fn reload_after_hello_reaches_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (state_tx, mut actions) = serve_at(&path);
    state_tx
        .send(serde_json::to_string(&AppState::new(Config::default(), true, None)).unwrap())
        .unwrap();
    let Attach::Session(client) = attach(&path).await.unwrap() else {
        panic!("expected a session");
    };
    let (_first, _rx, mut keys) = client.split();
    keys.reload().await.unwrap();
    loop {
        match next(&mut actions).await {
            Action::ReloadSession => break,
            Action::Views(_) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn views_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (state_tx, mut actions) = serve_at(&path);
    state_tx
        .send(serde_json::to_string(&AppState::new(Config::default(), true, None)).unwrap())
        .unwrap();
    let Attach::Session(client) = attach(&path).await.unwrap() else {
        panic!("expected a session");
    };
    assert!(matches!(next(&mut actions).await, Action::Views(1)));
    drop(client);
    assert!(matches!(next(&mut actions).await, Action::Views(0)));
}

#[tokio::test]
async fn refuse_obeys_stop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (stop_tx, mut stop_rx) = unbounded_channel();
    tokio::spawn(
        Host::bind(&path)
            .unwrap()
            .refuse("brainfm play is running", stop_tx),
    );
    // The host stays up, so only the stop signal is checked here.
    let waiting = tokio::spawn({
        let path = path.clone();
        async move { request_stop(&path).await }
    });
    tokio::time::timeout(Duration::from_secs(5), stop_rx.recv())
        .await
        .unwrap()
        .expect("a stop signal");
    waiting.abort();
    match attach(&path).await.unwrap() {
        Attach::Busy(reason) => assert_eq!(reason, "brainfm play is running"),
        _ => panic!("expected Busy"),
    }
}

#[tokio::test]
async fn request_stop_with_nothing_running() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    assert_eq!(
        request_stop(&path).await.unwrap(),
        StopOutcome::NothingRunning
    );
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    assert_eq!(
        request_stop(&path).await.unwrap(),
        StopOutcome::NothingRunning
    );
    assert!(!path.exists(), "the stale file is removed");
}

#[tokio::test]
async fn request_stop_waits_for_the_host_to_go() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (state_tx, state_rx) = watch::channel(String::new());
    let (actions_tx, mut actions) = unbounded_channel();
    let host = tokio::spawn(Host::bind(&path).unwrap().serve(state_rx, actions_tx));
    let file = path.clone();
    tokio::spawn(async move {
        let _keep = state_tx;
        while let Some(a) = actions.recv().await {
            if matches!(a, Action::Shutdown) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                host.abort();
                let _ = host.await;
                let _ = std::fs::remove_file(&file);
                break;
            }
        }
    });
    assert_eq!(request_stop(&path).await.unwrap(), StopOutcome::Stopped);
}

#[tokio::test]
async fn request_reload_with_nothing_running_is_false() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!request_reload(&dir.path().join("tui.sock")).await.unwrap());
}

#[tokio::test]
async fn second_bind_while_live_is_addr_in_use() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let _first = Host::bind(&path).unwrap();
    let err = Host::bind(&path).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_binds_over_a_stale_file_yield_one_host() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            tokio::spawn(async move { Host::bind(&path) })
        })
        .collect();
    let mut hosts = Vec::new();
    let mut in_use = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(h) => hosts.push(h),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => in_use += 1,
            Err(e) => panic!("unexpected {e}"),
        }
    }
    assert_eq!((hosts.len(), in_use), (1, 7));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn view_counts_settle_at_zero_after_concurrent_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (state_tx, mut actions) = serve_at(&path);
    state_tx
        .send(serde_json::to_string(&AppState::new(Config::default(), true, None)).unwrap())
        .unwrap();
    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let path = path.clone();
            tokio::spawn(async move {
                for _ in 0..5 {
                    if let Attach::Session(c) = attach(&path).await.unwrap() {
                        drop(c);
                    }
                }
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    let mut last = None;
    while let Ok(Some(Action::Views(n))) =
        tokio::time::timeout(Duration::from_millis(500), actions.recv()).await
    {
        last = Some(n);
    }
    assert_eq!(last, Some(0));
}

#[tokio::test]
async fn stop_after_hello_reaches_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    let (state_tx, mut actions) = serve_at(&path);
    state_tx
        .send(serde_json::to_string(&AppState::new(Config::default(), true, None)).unwrap())
        .unwrap();
    let Attach::Session(client) = attach(&path).await.unwrap() else {
        panic!("expected a session");
    };
    let (_first, _rx, mut keys) = client.split();
    keys.stop().await.unwrap();
    loop {
        match next(&mut actions).await {
            Action::Shutdown => break,
            Action::Views(_) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_silent_host_is_not_responding_rather_than_busy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.sock");
    // Bound and listening, but nobody ever answers the hello.
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    assert!(matches!(
        attach(&path).await.unwrap(),
        Attach::NotResponding
    ));
}
