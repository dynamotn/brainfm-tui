use std::process::Command;

#[test]
fn help_lists_subcommands() {
    let out = Command::new(env!("CARGO_BIN_EXE_brainfm"))
        .arg("--help")
        .output()
        .expect("run brainfm --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for sub in ["login", "logout", "whoami", "list", "play", "stop"] {
        let listed = stdout
            .lines()
            .any(|l| l.split_whitespace().next() == Some(sub));
        assert!(listed, "missing `{sub}` in:\n{stdout}");
    }
    let hidden = stdout
        .lines()
        .any(|l| l.split_whitespace().next() == Some("daemon"));
    assert!(!hidden, "`daemon` must stay hidden in:\n{stdout}");
}

#[test]
fn wrong_password_is_not_reported_as_expired_session() {
    let msg = brainfm::cli::describe_error(
        &anyhow::Error::new(brainfm::api::ApiError::Unauthorized),
        true,
    );
    assert!(msg.contains("Wrong email or password"), "{msg}");
    let msg = brainfm::cli::describe_error(
        &anyhow::Error::new(brainfm::api::ApiError::Unauthorized),
        false,
    );
    assert!(msg.contains("brainfm login"), "{msg}");
}

#[cfg(unix)]
#[test]
fn daemon_without_session_fails() {
    let home = tempfile::tempdir().expect("temp dir");
    let out = Command::new(env!("CARGO_BIN_EXE_brainfm"))
        .arg("daemon")
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .output()
        .expect("run brainfm daemon");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "daemon exited 0; stderr:\n{stderr}");
    assert!(stderr.contains("brainfm login"), "stderr:\n{stderr}");
}

#[cfg(unix)]
#[test]
fn stop_with_nothing_running_is_silent() {
    let home = tempfile::tempdir().expect("temp dir");
    let out = Command::new(env!("CARGO_BIN_EXE_brainfm"))
        .arg("stop")
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .output()
        .expect("run brainfm stop");
    assert!(out.status.success(), "stop failed: {out:?}");
    assert!(out.stdout.is_empty(), "stdout: {out:?}");
    assert!(out.stderr.is_empty(), "stderr: {out:?}");
}
