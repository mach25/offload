//! Talking to the local `offloadd` over its control socket.
//!
//! One implementation for every client: the `offload` CLI and the Android app's `offload-mobile`
//! (ADR-0071), so the framing, the end-of-answer rule and the sentences for "no daemon" cannot
//! differ between a terminal and a phone.

use crate::api::{Request, Response};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Where the daemon's control socket lives, unless overridden.
#[must_use]
pub fn default_socket() -> PathBuf {
    socket_from(
        std::env::var_os("OFFLOAD_SOCKET").map(PathBuf::from),
        std::env::var_os("OFFLOAD_STATE_DIR").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// The resolution order, as a pure function.
///
/// Split out so it can be tested without mutating process environment — which the
/// workspace's `unsafe_code = "forbid"` rules out anyway, and which is flaky under a
/// parallel test runner regardless.
#[must_use]
fn socket_from(
    socket_env: Option<PathBuf>,
    state_env: Option<PathBuf>,
    home: Option<PathBuf>,
) -> PathBuf {
    if let Some(explicit) = socket_env {
        return explicit;
    }
    state_env
        .or_else(|| home.map(|h| h.join(".offload")))
        .unwrap_or_else(|| PathBuf::from("/tmp/offload"))
        .join("offloadd.sock")
}

/// Send one request and collect responses until `Done`.
pub async fn request(socket: &Path, req: &Request) -> Result<Vec<Response>> {
    let mut stream = connect(socket).await?;
    write_request(&mut stream, req).await?;

    let (read, _write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut responses = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let response: Response =
            serde_json::from_str(&line).with_context(|| format!("bad response: {line}"))?;
        if matches!(response, Response::Done) {
            break;
        }
        responses.push(response);
    }
    Ok(responses)
}

/// Send one request and hand each response to `on_event` as it arrives.
///
/// Separate from [`request`] because `logs --follow` must print turns while the agent is
/// still working; buffering until `Done` would defeat the point.
pub async fn stream<F>(socket: &Path, req: &Request, mut on_event: F) -> Result<()>
where
    F: FnMut(Response) -> Result<()>,
{
    let mut stream = connect(socket).await?;
    write_request(&mut stream, req).await?;

    let (read, _write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let response: Response =
            serde_json::from_str(&line).with_context(|| format!("bad response: {line}"))?;
        if matches!(response, Response::Done) {
            break;
        }
        on_event(response)?;
    }
    Ok(())
}

async fn connect(socket: &Path) -> Result<UnixStream> {
    match UnixStream::connect(socket).await {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The overwhelmingly common cause, and the message should say so rather than
            // making the user work out what a missing socket file means.
            bail!(
                "no daemon at {} — is offloadd running?\n  start it with: offloadd",
                socket.display()
            )
        }
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            bail!(
                "socket {} exists but nothing is listening — a previous offloadd probably \
                 crashed. Starting a new one will clear it.",
                socket.display()
            )
        }
        Err(e) => Err(e).with_context(|| format!("connecting to {}", socket.display())),
    }
}

async fn write_request(stream: &mut UnixStream, req: &Request) -> Result<()> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    Ok(())
}

/// Pull the single expected response out, turning a daemon-side error into ours.
pub fn expect_one(responses: Vec<Response>) -> Result<Response> {
    match responses.into_iter().next() {
        Some(Response::Error { message }) => bail!("{message}"),
        Some(other) => Ok(other),
        None => bail!("daemon returned no response"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_socket_wins_over_everything() {
        // Multi-node dev on one machine depends on this; without it every `offload`
        // command talks to whichever daemon happens to own the default path.
        assert_eq!(
            socket_from(
                Some("/tmp/custom.sock".into()),
                Some("/tmp/state".into()),
                Some("/home/j".into())
            ),
            PathBuf::from("/tmp/custom.sock")
        );
    }

    #[test]
    fn falls_back_through_state_dir_then_home() {
        assert_eq!(
            socket_from(None, Some("/tmp/state".into()), Some("/home/j".into())),
            PathBuf::from("/tmp/state/offloadd.sock")
        );
        assert_eq!(
            socket_from(None, None, Some("/home/j".into())),
            PathBuf::from("/home/j/.offload/offloadd.sock")
        );
        assert_eq!(
            socket_from(None, None, None),
            PathBuf::from("/tmp/offload/offloadd.sock")
        );
    }

    #[test]
    fn the_cli_and_daemon_agree_on_the_default_path() {
        // Two crates deriving the same path independently is a bug waiting to happen:
        // they drift, and `offload` silently cannot find a running `offloadd`.
        let config = crate::Config {
            state_dir: PathBuf::from("/tmp/state"),
            ..crate::Config::default()
        };
        assert_eq!(
            config.socket_path(),
            socket_from(None, Some("/tmp/state".into()), None)
        );
    }

    #[test]
    fn a_daemon_error_becomes_our_error() {
        let err = expect_one(vec![Response::Error {
            message: "nope".into(),
        }])
        .expect_err("should surface");
        assert_eq!(err.to_string(), "nope");
    }
}
