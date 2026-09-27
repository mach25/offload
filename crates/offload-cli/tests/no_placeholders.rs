//! No `<placeholder>` reaches an operator's screen.
//!
//! Three times now a line meant to be filled in has been printed with the angle brackets still in
//! it, each time in the one line of its output that is an *instruction* — the line somebody is
//! meant to copy:
//!
//! - `offload logs` printed `offload approve <run> <tool_use_id>` (session seventy-four);
//! - `offload logs` printed `run released — resume it with: offload resume <run>` (session
//!   eighty-seven), forty lines from the first in the same `match`, and it survived that fix
//!   because the comment beside it said *"this is the one line in the log that is an
//!   instruction"*;
//! - `offload audit` printed `it is at <state>/worktrees/<run>.superseded*` — where the only copy
//!   of somebody's uncommitted work is, in the log that outlives the run's own.
//!
//! Every one of them had the value in scope. What they have in common is not a subject, so a
//! reviewer looking at the arm in front of them will not find the next one: the thing they share
//! is the *shape*, which is what a test over the source can see and a type cannot. Modelled on
//! `offload-core/tests/no_clock.rs`, for its reason.
//!
//! This is a lint, not a proof. It reads the text of `println!`/`format!` arguments in this crate;
//! a placeholder assembled from pieces, or built in `offload-node` and printed here, goes past it.
//! It catches the way all three of these were actually written, which is the useful thing.

// An integration test is linted as ordinary code, which is right nearly everywhere and not here:
// a source tree this cannot read is a rule it cannot check, and continuing quietly would be the
// worst outcome available. Panicking is the reporting.
#![allow(clippy::expect_used)]

use std::path::Path;

/// Words that read as *fill this in*, in angle brackets.
///
/// Deliberately a list rather than "any `<word>`": `<` is ordinary in prose (`<state dir>`,
/// comparisons, generics in doc comments), and a check that cries wolf gets deleted. These are the
/// names the product's own vocabulary would put in a hole.
const PLACEHOLDERS: &[&str] = &[
    "<run>",
    "<id>",
    "<node>",
    "<state>",
    "<tool_use_id>",
    "<schedule>",
    "<rule>",
    "<path>",
    "<name>",
];

#[test]
fn no_placeholder_reaches_a_screen() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    walk(&src, &mut |path, text| {
        for (n, line) in text.lines().enumerate() {
            // The rule is about what is printed, not about what the code says about itself. Every
            // comment in this tree that explains one of these bugs quotes the bad string, and so
            // does this file — including the `--help` prose, which is a doc comment.
            let code = line.split("//").next().unwrap_or(line);
            if !code.contains("println!") && !code.contains("format!") && !code.contains("write!") {
                continue;
            }
            for needle in PLACEHOLDERS {
                if code.contains(needle) {
                    found.push(format!("{}:{} — {}", path.display(), n + 1, code.trim()));
                }
            }
        }
    });
    assert!(
        found.is_empty(),
        "a placeholder is about to be printed to somebody who cannot fill it in. Every line that \
         has done this had the value in scope — spell it. If the hole genuinely cannot be filled \
         here (a state directory this crate does not know), name the thing in words instead of \
         bracketing it, so the reader has a phrase to search for rather than a token to \
         misread.\n{}",
        found.join("\n")
    );
}

fn walk(dir: &Path, each: &mut impl FnMut(&Path, &str)) {
    let entries = std::fs::read_dir(dir).expect("offload-cli/src is readable");
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
