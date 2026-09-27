//! Keeping the machine awake while a node may take work (ADR-0077).
//!
//! A Mac mini with the default `sleep 1` went to sleep a minute after the last keystroke, and its
//! daemon with it: the fleet marked it dead, and it answered only in the seconds after a packet
//! woke it (session ninety-two). Programs that must keep running tell macOS so, by holding a
//! "prevent system sleep" assertion. This is Offload doing the same, for exactly as long as it
//! holds a [`KeepAwake`]. On mains power only: macOS ignores the assertion on battery.
//!
//! **Idle sleep only.** A person closing a lid or choosing Sleep is obeyed; what is prevented is the
//! machine deciding on its own that nobody needs it.
//!
//! The one crate in the workspace allowed `unsafe`, and only in `macos.rs`, because the request
//! is a C call. Everything here is safe to use.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

#[cfg(target_os = "macos")]
mod macos;

/// While this exists, the machine does not sleep on its own. Dropping it lets it again.
#[derive(Debug)]
pub struct KeepAwake {
    /// Held for its `Drop`, which releases it: never read, and that is the point.
    #[cfg(target_os = "macos")]
    _assertion: macos::Assertion,
}

impl KeepAwake {
    /// Ask the system not to idle-sleep, naming why: `reason` is what a person sees in
    /// `pmset -g assertions` (macOS), so it should say what is being kept running.
    ///
    /// # Errors
    ///
    /// In words, when the system refuses or this platform has no way to ask.
    pub fn hold(reason: &str) -> Result<KeepAwake, String> {
        #[cfg(target_os = "macos")]
        {
            macos::Assertion::prevent_idle_sleep(reason).map(|assertion| KeepAwake {
                _assertion: assertion,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            Err("keeping the machine awake is not built for this platform".to_string())
        }
    }
}

/// Whether this build can keep the machine awake at all.
#[must_use]
pub const fn supported() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(test)]
mod tests {
    /// The crate's exception to the workspace's `forbid(unsafe_code)` is one file. A second file
    /// that allowed it would be an exception nobody decided on.
    #[test]
    fn unsafe_is_allowed_in_macos_rs_alone() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut allowing = Vec::new();
        for entry in std::fs::read_dir(&src).expect("src") {
            let path = entry.expect("entry").path();
            let text = std::fs::read_to_string(&path).expect("read");
            let code: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            // Assembled at run time, so this test's own source does not match itself.
            let needles = [
                ["allow(", "unsafe_code)"].concat(),
                ["unsafe", " {"].concat(),
                ["unsafe", " extern"].concat(),
            ];
            if needles.iter().any(|n| code.contains(n.as_str())) {
                allowing.push(
                    path.file_name()
                        .expect("name")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        allowing.sort();
        assert_eq!(allowing, vec!["macos.rs".to_string()]);
    }

    /// The assertion is really held, and really let go, as macOS itself reports it. Run on a Mac;
    /// ignored by default because it changes the machine's power state while it runs.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "holds a real power assertion; run on a Mac with --ignored"]
    fn macos_lists_the_assertion_while_it_is_held() {
        let listed = || {
            let out = std::process::Command::new("pmset")
                .args(["-g", "assertions"])
                .output()
                .expect("pmset");
            String::from_utf8_lossy(&out.stdout).contains("Offload: power test")
        };
        let held = super::KeepAwake::hold("Offload: power test").expect("held");
        assert!(listed(), "pmset lists it while held");
        drop(held);
        assert!(!listed(), "and not after it is dropped");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn elsewhere_it_says_so_rather_than_pretending() {
        assert!(!super::supported());
        let refused = super::KeepAwake::hold("test").expect_err("not built here");
        assert!(refused.contains("not built for this platform"), "{refused}");
    }
}
