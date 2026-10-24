//! `brainfm daemon`: the session host without a terminal. It owns the
//! player and the API client and serves every TUI over the Unix socket.

use anyhow::bail;

use super::{Frontend, host_loop, init_logging, setup, start};
use crate::share::{Host, SocketGuard, socket_path};

/// Hosts the session until a `Stop`, an idle timeout or a quit from a view.
/// Returns at once, successfully, if another host already holds the socket.
pub async fn run() -> anyhow::Result<()> {
    let _log = init_logging()?;
    let result = host().await;
    // Started from a TUI with no stderr, so the log is the only place the
    // reason can be read.
    if let Err(e) = &result {
        tracing::error!("{e:#}");
    }
    result
}

async fn host() -> anyhow::Result<()> {
    let mut setup = setup()?;
    // Checked before the audio device or the socket are touched.
    if setup.state.user_id.is_none() {
        bail!("not logged in — run `brainfm login`");
    }
    let path = socket_path()?;
    let host = match Host::bind(&path) {
        Ok(h) => h,
        // Another daemon won the race; it serves the views.
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let _socket = SocketGuard(path);
    // Logging in happens in a view, which then asks for a reload.
    setup.state.login_here = false;

    let (rt, inputs, state, state_rx) = start(setup).await?;
    tokio::spawn(host.serve(state_rx, rt.tx.clone()));
    host_loop(rt, inputs, state, Frontend::Headless).await?;
    Ok(())
}
