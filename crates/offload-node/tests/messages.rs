//! What this daemon *says* is part of what it does, so the text is checked like anything else.
//!
//! One rule, and it exists because the defect it catches is invisible in review. A Rust string
//! literal continued with a trailing backslash drops the newline **and** the next line's
//! indentation — but a tool that rewrites source through a language whose own strings treat
//! backslash-newline as a line continuation (Python's do) collapses it *without* dropping the
//! indentation, leaving a run of spaces baked into the literal. The diff then shows one long line
//! and the mangling reads as formatting.
//!
//! It had reached three of this daemon's operator-facing refusals, including the one the handoff
//! singles out as the model of a good one — "a cancel cannot reach its agent yet" printed with
//! twenty-two spaces in the middle of the sentence. Found by running `offload rm` on the wrong
//! machine and reading the answer, which is the only way anybody was going to find it.
//!
//! **The heuristic, and why it is this one.** A run of four or more spaces is deliberate when it
//! is *column padding* — `"agent       claude-code {version}"` — and that padding always sits
//! near the start of the literal, behind a short label. Mangled continuations land mid-sentence.
//! So: a run of four or more spaces beginning at least twenty characters into the literal. On the
//! tree where this was written that is six real hits and zero false positives, and the escape
//! hatch for a genuine wide gap is to put the padding at the front, where a table's label goes.
//!
//! **It walks the whole workspace, and it used not to.** The first version was scoped to this
//! crate on `no_clock.rs`'s reasoning — `CARGO_MANIFEST_DIR` is the honest unit — plus a claim
//! about where it matters: "the daemon's refusals are the longest sentences in the workspace".
//! That claim had a date on it. `Escalation`'s reasons live in `offload-core`, are rendered
//! straight into the daemon's log, and one of them shipped with **thirty spaces** in the middle of
//! it — written in this session, by the exact tool-and-Python route the paragraph above describes,
//! and found by reading a walk's output rather than by the check.
//!
//! The two rules differ in kind, which is why they are scoped differently: *no clock in
//! `offload-core`* is a statement about one crate, and *no mangled operator text* is a statement
//! about the workspace. One test that walks `crates/*/src` beats three copies that drift.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

/// Column padding is allowed this far into a literal, where a table's label lives.
const PADDING_ALLOWED_BEFORE: usize = 20;
/// A run this long or longer is a gap somebody would notice in the output.
const RUN: usize = 4;

#[test]
fn no_message_carries_a_mangled_line_continuation() {
    // Every crate's `src`, not just this one — see the module docs. Reached from this crate's
    // manifest rather than from the workspace root, because that is the one path a test can be
    // sure of.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is this crate's parent");
    let mut sources: Vec<std::path::PathBuf> = std::fs::read_dir(crates)
        .expect("read crates/")
        .filter_map(|entry| entry.ok().map(|e| e.path().join("src")))
        .filter(|src| src.is_dir())
        .collect();
    sources.sort();
    assert!(
        sources.len() >= 8,
        "found {} crates to check, which is fewer than this workspace has — the walk is looking          in the wrong place and would pass by finding nothing",
        sources.len()
    );

    let mut found = Vec::new();
    for src in &sources {
        walk(src, &mut |path, text| {
            for (n, line) in text.lines().enumerate() {
                // Comments are prose about the code and may say anything, including examples of this.
                let code = line.split("//").next().unwrap_or(line);
                for literal in literals(code) {
                    if let Some(at) = mangled_run(literal) {
                        found.push(format!(
                            "{}:{} — a run of spaces at offset {at} of a literal",
                            path.display(),
                            n + 1,
                        ));
                    }
                }
            }
        });
    }
    assert!(
        found.is_empty(),
        "a string literal carries a run of spaces mid-sentence, which is a line continuation \
         collapsed by a tool that kept the indentation. The text prints with a gap in it. Rewrite \
         the literal as one line, or — if the gap is deliberate column padding — put it within the \
         first twenty characters, where a table's label lives.\n{}",
        found.join("\n")
    );
}

/// Offset of the first mangled-looking run of spaces, if any.
fn mangled_run(literal: &str) -> Option<usize> {
    let bytes = literal.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            let start = i;
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            if i - start >= RUN && start >= PADDING_ALLOWED_BEFORE {
                return Some(start);
            }
        } else {
            i += 1;
        }
    }
    None
}

/// The string literals on one line of code, escapes skipped rather than interpreted.
fn literals(code: &str) -> Vec<&str> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'"' => break,
                _ => j += 1,
            }
        }
        if let Some(text) = code.get(start..j.min(bytes.len())) {
            out.push(text);
        }
        i = j + 1;
    }
    out
}

fn walk(dir: &Path, each: &mut impl FnMut(&Path, &str)) {
    let entries = std::fs::read_dir(dir).expect("offload-node/src is readable");
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
