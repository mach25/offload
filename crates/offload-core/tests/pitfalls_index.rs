//! The pitfalls index in `CLAUDE.md` is a promise, not a type: a table mapping a path you are
//! about to touch to the pitfall file that covers it. CLAUDE.md's own instruction is "a new file
//! only for a subject none of them covers, and then a row here", and CONTRIBUTING.md requires a
//! rule and its full `detail/` entry to land together. Nothing enforces either — the table is
//! prose, and an unindexed pitfall file is a rule nobody will find until they trip over it.
//!
//! This lives in `offload-core` for the same reason `no_clock.rs` does: it is the crate
//! everything else in the workspace depends on, and there is no crate that owns the repository's
//! own documentation — the check has to live somewhere, and downstream of nothing is as close to
//! "the repository itself" as a `cargo test` gets.
//!
//! Like `no_clock.rs`, this reads files the compiler has no reason to look at: `CLAUDE.md`'s
//! table and the `docs/pitfalls/` directory. There is no type for "every file is indexed" either.

// An integration test is linted as ordinary code, which is right nearly everywhere and not here:
// a source tree this cannot read is a rule it cannot check, and continuing quietly would be the
// worst outcome available. Panicking is the reporting — same rationale as `no_clock.rs`.
#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `CARGO_MANIFEST_DIR` is `<repo>/crates/offload-core`; the repository root is two levels up.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("offload-core lives two directories below the repository root")
        .to_path_buf()
}

/// The `Read` column of every data row in CLAUDE.md's pitfalls index, in file order. Table cells
/// name files in backticks — sometimes several, comma-separated, in one cell — so this returns
/// the raw cell text and leaves picking the names apart to `backtick_names`.
///
/// Panics loudly on a row shape it does not recognise, rather than silently skipping it: a parser
/// that tolerates an unexpected shape is a parser that stops checking without saying so.
fn read_column_cells() -> Vec<String> {
    let claude_md = std::fs::read_to_string(repo_root().join("CLAUDE.md"))
        .expect("CLAUDE.md is readable from the repository root");

    let mut in_table = false;
    let mut cells = Vec::new();
    for line in claude_md.lines() {
        let trimmed = line.trim();
        if trimmed == "| Before you touch | Read |" {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if !trimmed.starts_with('|') {
            // The table ends at the first line that isn't a table row.
            break;
        }
        // The `| --- | --- |` separator row: no names to collect, skip it.
        if trimmed.chars().all(|c| matches!(c, '|' | '-' | ' ')) {
            continue;
        }
        let body = trimmed.trim_start_matches('|').trim_end_matches('|');
        let row_cells: Vec<&str> = body.split('|').collect();
        assert_eq!(
            row_cells.len(),
            2,
            "pitfalls index row does not have exactly two columns, so the table's shape has \
             changed and this parser needs updating: {trimmed}"
        );
        cells.push(row_cells[1].trim().to_string());
    }

    assert!(
        !cells.is_empty(),
        "found no rows in CLAUDE.md's pitfalls index table — this parser is looking in the \
         wrong place, not confirming an empty table"
    );
    cells
}

/// The backtick-quoted names in one `Read` cell, liberally normalised: a name may be given as a
/// bare stem (`fencing-and-epochs`), or with a `docs/pitfalls/` prefix and/or a `.md` suffix, and
/// this accepts all three so a harmless change in the table's own style doesn't fail the test for
/// the wrong reason.
fn backtick_names(cell: &str) -> Vec<String> {
    cell.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, raw)| {
            let name = raw.trim();
            let name = name.strip_prefix("docs/pitfalls/").unwrap_or(name);
            let name = name.strip_suffix(".md").unwrap_or(name);
            name.to_string()
        })
        .collect()
}

/// Every pitfall name the index table references, deduplicated, across every row.
fn indexed_names() -> BTreeSet<String> {
    read_column_cells()
        .iter()
        .flat_map(|cell| backtick_names(cell))
        .collect()
}

/// The top-level `docs/pitfalls/*.md` files — the rules — excluding `docs/pitfalls/detail/`,
/// which holds the same entries in full and is not itself indexed.
fn pitfall_files() -> Vec<PathBuf> {
    let dir = repo_root().join("docs/pitfalls");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("docs/pitfalls is readable from the repository root")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_pitfall_file_has_a_row_in_the_index() {
    let files = pitfall_files();
    // A walk that finds nothing passes for the wrong reason: guard against looking in the
    // wrong directory, same as the crate-count sanity check in `messages.rs`.
    assert!(
        files.len() >= 10,
        "found only {} files under docs/pitfalls — this is probably looking in the wrong place",
        files.len()
    );

    let indexed = indexed_names();
    let unindexed: Vec<String> = files
        .iter()
        .filter_map(|path| path.file_stem().and_then(|s| s.to_str()))
        .filter(|stem| !indexed.contains(*stem))
        .map(str::to_string)
        .collect();

    assert!(
        unindexed.is_empty(),
        "these docs/pitfalls files have no row in CLAUDE.md's index table — CLAUDE.md says a \
         new pitfall file needs \"a row here\", and an unindexed file is a rule nobody will \
         find:\n{}",
        unindexed.join("\n")
    );
}

#[test]
fn every_indexed_name_names_a_file_that_exists() {
    let indexed = indexed_names();
    assert!(
        indexed.len() >= 10,
        "found only {} names in CLAUDE.md's pitfalls index — the table parser is probably broken",
        indexed.len()
    );

    let missing: Vec<String> = indexed
        .iter()
        .filter(|name| {
            !repo_root()
                .join("docs/pitfalls")
                .join(format!("{name}.md"))
                .is_file()
        })
        .cloned()
        .collect();

    assert!(
        missing.is_empty(),
        "CLAUDE.md's pitfalls index names files that do not exist under docs/pitfalls:\n{}",
        missing.join("\n")
    );
}

#[test]
fn every_pitfall_file_has_a_detail_counterpart() {
    let files = pitfall_files();
    assert!(
        !files.is_empty(),
        "found no files under docs/pitfalls — this is probably looking in the wrong place"
    );

    let missing: Vec<String> = files
        .iter()
        .filter_map(|path| path.file_name().and_then(|s| s.to_str()))
        .filter(|name| {
            !repo_root()
                .join("docs/pitfalls/detail")
                .join(name)
                .is_file()
        })
        .map(str::to_string)
        .collect();

    assert!(
        missing.is_empty(),
        "CONTRIBUTING.md requires a rule and its full docs/pitfalls/detail/ entry to be added \
         at the same time; these have no counterpart there:\n{}",
        missing.join("\n")
    );
}
