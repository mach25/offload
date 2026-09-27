//! The one property of this crate that the compiler cannot check.
//!
//! `offload-core` is synchronous, deterministic, and has **no clock** (ADR-0001). Every decision
//! in it — placement, bidding, reassignment, the hold-down — is a pure function of
//! `(view, run, now)`, and `now` arrives as an argument. That is what lets the simulation tests
//! arrange a case rather than provoke it, and it is why probing lives in `offload-probe`.
//!
//! The absence of tokio is enforced by the absence of a dependency, and the absence of I/O
//! mostly is too. `SystemTime::now()` is not: it is in `std`, so nothing stops a function here
//! reaching for it, and the damage would be quiet — a decision that reads the wall clock is
//! untestable *here*, so the test that would have caught it is the one that stops being possible.
//! By the time anybody notices, the arrangement this crate exists for is gone.
//!
//! Hence a test that reads the source. It is a strange thing to do and it is the only thing that
//! works: the rule is about what the code may *call*, and there is no type that says so.

// An integration test is linted as ordinary code, which is right nearly everywhere and not here:
// a source tree this cannot read is a rule it cannot check, and continuing quietly would be the
// worst outcome available. Panicking is the reporting.
#![allow(clippy::expect_used)]

use std::path::Path;

/// Calling any of these in this crate means a decision that cannot be arranged in a test.
const FORBIDDEN: &[&str] = &[
    "SystemTime::now",
    "Instant::now",
    "Utc::now",
    "std::env::var",
    "std::fs::",
];

#[test]
fn nothing_in_this_crate_reads_the_world() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    walk(&src, &mut |path, text| {
        for (n, line) in text.lines().enumerate() {
            // The rule is about what the code does, not about what it says about itself: this
            // file's own prose names every one of these, and so does the doc comment on `Millis`.
            let code = line.split("//").next().unwrap_or(line);
            for needle in FORBIDDEN {
                if code.contains(needle) {
                    found.push(format!("{}:{} — {}", path.display(), n + 1, code.trim()));
                }
            }
        }
    });
    assert!(
        found.is_empty(),
        "offload-core has no clock and no I/O (ADR-0001): time is injected as a `Millis`, and \
         anything that reads the world belongs in offload-probe or the daemon. A decision that \
         reads the wall clock is untestable here, which means the test that would have caught \
         it is the one that stops being possible.\n{}",
        found.join("\n")
    );
}

fn walk(dir: &Path, each: &mut impl FnMut(&Path, &str)) {
    let entries = std::fs::read_dir(dir).expect("offload-core/src is readable");
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            walk(&path, each);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).expect("source file is readable");
            each(&path, &text);
        }
    }
}
