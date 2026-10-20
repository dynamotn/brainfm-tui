use std::process::Command;

#[test]
fn help_lists_subcommands() {
    let out = Command::new(env!("CARGO_BIN_EXE_brainfm"))
        .arg("--help")
        .output()
        .expect("run brainfm --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for sub in ["login", "logout", "whoami", "list", "play"] {
        assert!(stdout.contains(sub), "missing `{sub}` in:\n{stdout}");
    }
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
