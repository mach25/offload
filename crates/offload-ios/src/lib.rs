//! The iOS host's link to `offloadd` (ADR-0070).
//!
//! An iOS app cannot launch a separate program, so it links this library and runs the daemon on a
//! thread of its own process. Two functions, nothing else: start it on a config file, and stop it
//! so it drains and announces its departure the way SIGTERM makes `offloadd` do. Everything past
//! that — configuration, enrolment, runs — is the ordinary CLI, pointed at the daemon's socket.
//!
//! The one crate in the workspace where `unsafe` is not forbidden, and it is allowed only on the
//! lines a C entry point cannot do without: the two exported symbols, and the read of the one C
//! string the ABI takes, behind a null check (see this crate's `Cargo.toml`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::ffi::{c_char, CStr};
use std::sync::Mutex;

/// The running daemon: its stop signal and its thread. One daemon per process.
static STOP: Mutex<Option<Running>> = Mutex::new(None);

struct Running {
    stop: tokio::sync::oneshot::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

/// Start the daemon on the config at `config_path`, on a thread of its own.
///
/// Returns 0 when it started, 1 when one is already running, 2 for a missing or unreadable path,
/// 3 for a config that does not load. The daemon's own failures after that go to `daemon.log` in
/// its state directory, which is where the app's screen reads from.
///
/// # Safety
///
/// `config_path` must be null or point to a NUL-terminated string that stays valid for the call.
// SAFETY: a unique, crate-prefixed symbol; nothing else in the app exports it.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn offload_start(config_path: *const c_char) -> i32 {
    if config_path.is_null() {
        return 2;
    }
    // SAFETY: non-null (checked above) and NUL-terminated for the call, per this function's
    // documented contract with the Swift caller, which passes a `String`'s C representation.
    #[allow(unsafe_code)]
    let path = match unsafe { CStr::from_ptr(config_path) }.to_str() {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => return 2,
    };
    start(&path)
}

/// Stop the daemon and wait for it: it drains, announces its departure, and only then does this
/// return. A no-op when none is running.
///
/// Waiting is the point. A stop that returned at the signal would free the slot while the old
/// daemon was still draining, and a start straight after it — an app backgrounded and brought
/// back — would run a second daemon on the same state directory and socket.
// SAFETY: as `offload_start`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn offload_stop() {
    let running = STOP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(running) = running {
        let _ = running.stop.send(());
        let _ = running.thread.join();
    }
}

/// [`offload_start`] without the C string, for Rust callers and tests.
pub fn start(config_path: &std::path::Path) -> i32 {
    // The config first: a bad one says so whatever else is running.
    let config = match offload_node::config::Config::load(Some(config_path)) {
        Ok(config) => config,
        Err(_) => return 3,
    };
    let mut slot = STOP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A daemon that ended by itself (an error, logged) leaves its thread finished in the slot.
    if slot.as_ref().is_some_and(|r| !r.thread.is_finished()) {
        return 1;
    }
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    // Held across the spawn, so a concurrent start or stop cannot see a half-filled slot.
    let thread = std::thread::spawn(move || {
        log_to(&config.state_dir);
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(e) => {
                tracing::error!(error = %e, "cannot start a runtime for offloadd");
                return;
            }
        };
        let result = runtime.block_on(offload_node::daemon::run(config, async {
            let _ = stopped.await;
        }));
        if let Err(e) = result {
            tracing::error!(error = %e, "offloadd stopped with an error");
        }
    });
    *slot = Some(Running { stop, thread });
    0
}

/// Logging to `daemon.log` in the state directory, once per process: an app has no terminal, and
/// the app's own screen shows the tail of this file.
fn log_to(state_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(state_dir);
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("daemon.log"))
    else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole ABI, end to end on the host: start answers its socket, a second start is
    /// refused, and stop drains before it returns and lets it be started again at once.
    #[test]
    fn start_serves_the_socket_and_stop_ends_it() {
        let dir = std::env::temp_dir().join(format!("offload-ios-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let config = dir.join("node.toml");
        std::fs::write(
            &config,
            format!(
                "name = \"ios-test\"\nstate_dir = \"{}\"\n[cluster]\nenabled = false\n",
                dir.join("s").display()
            ),
        )
        .expect("config");

        assert_eq!(start(&config), 0);
        assert_eq!(start(&config), 1, "one daemon per process");
        let socket = dir.join("s").join("offloadd.sock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !socket.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(socket.exists(), "the daemon came up and bound its socket");
        assert!(std::os::unix::net::UnixStream::connect(&socket).is_ok());

        offload_stop();
        // Nothing waited for here: stop returns only when the daemon has gone, so its socket
        // answers nobody and a start straight after cannot run a second daemon beside it.
        assert!(
            std::os::unix::net::UnixStream::connect(&socket).is_err(),
            "stop returned before the daemon had gone"
        );
        assert_eq!(start(&config), 0, "stopped, so it starts again");
        offload_stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_config_is_refused_not_panicked_on() {
        assert_eq!(start(std::path::Path::new("/nonexistent/offload.toml")), 3);
    }
}
