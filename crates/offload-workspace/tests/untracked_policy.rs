//! Properties of the untracked-file policy.
//!
//! This module's own doc comment states what it is for in one sentence — "deliberately
//! conservative in the *include* direction and loud about everything it drops: silently
//! discarding a file the user cared about is the failure this whole module exists to avoid,
//! so anything excluded is named and attributed". Everything below is that sentence, or one
//! of its consequences, aimed at every input rather than at the cases somebody thought of.
//!
//! ADR-0003 is why it is worth the trouble: uncommitted work is the fragile, valuable part,
//! and this is the function that decides which of it travels.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use offload_workspace::untracked::{select, Excluded, UntrackedPolicy};
use proptest::prelude::*;
use std::collections::BTreeSet;

/// What every property below except the last one is about: the repository that tracks
/// nothing in any of these directories, where a list of names is all there is to go on.
fn nothing_tracked() -> BTreeSet<String> {
    BTreeSet::new()
}

/// Paths that look like the ones a real worktree produces: source, build output, nesting,
/// and the names that are a directory in one repo and a file in another.
fn path() -> impl Strategy<Value = String> {
    let component = prop_oneof![
        Just("src"),
        Just("crates"),
        Just("api"),
        Just("target"),
        Just("build"),
        Just("dist"),
        Just("node_modules"),
        Just("__pycache__"),
        Just("a.rs"),
        Just("mod.rs"),
        Just("x.o"),
        Just("notes"),
    ];
    prop::collection::vec(component, 1..5).prop_map(|parts| parts.join("/"))
}

fn entries() -> impl Strategy<Value = Vec<(String, u64)>> {
    prop::collection::vec((path(), 0u64..4_000_000), 0..24).prop_map(|mut entries| {
        // Git lists each path once, and a duplicate would make "every entry is accounted
        // for" mean something weaker than it says.
        let mut seen = BTreeSet::new();
        entries.retain(|(path, _)| seen.insert(path.clone()));
        entries
    })
}

fn policy() -> impl Strategy<Value = UntrackedPolicy> {
    (
        0u64..2_000_000,
        0u64..8_000_000,
        prop::collection::vec(
            prop_oneof![Just("notes".to_string()), Just("api".to_string())],
            0..2,
        ),
    )
        .prop_map(
            |(max_file_bytes, max_total_bytes, also_never_source)| UntrackedPolicy {
                max_file_bytes,
                max_total_bytes,
                also_never_source,
            },
        )
}

proptest! {
    /// The claim in the module's first paragraph: nothing is silently dropped. Every
    /// candidate comes out either carried or named, exactly once, and nothing is invented.
    #[test]
    fn every_candidate_is_either_carried_or_named(entries in entries(), policy in policy()) {
        let selection = select(&entries, &policy, &nothing_tracked());

        let offered: BTreeSet<&String> = entries.iter().map(|(path, _)| path).collect();
        let carried: BTreeSet<&String> = selection.included.iter().collect();
        let named: BTreeSet<&String> = selection.excluded.iter().map(|(path, _)| path).collect();

        prop_assert_eq!(
            selection.included.len() + selection.excluded.len(),
            entries.len(),
            "{:?}", selection
        );
        prop_assert!(carried.is_disjoint(&named));
        prop_assert_eq!(carried.union(&named).copied().collect::<BTreeSet<_>>(), offered);
    }

    /// What the caller is then told. A drop that is not in the summary is a drop nobody sees:
    /// the run's log gets this string and nothing else about what stayed behind, so each
    /// reason has to be counted under its own name — a file left behind because the
    /// checkpoint's budget was full is not a file that was too big, and the two are fixed by
    /// different settings.
    #[test]
    fn the_summary_accounts_for_every_drop_under_its_own_reason(
        entries in entries(),
        policy in policy(),
    ) {
        let selection = select(&entries, &policy, &nothing_tracked());
        let summary = selection.summary();

        let count = |wanted: fn(&Excluded) -> bool| {
            selection.excluded.iter().filter(|(_, why)| wanted(why)).count()
        };
        let build = count(|why| matches!(why, Excluded::BuildOutput { .. }));
        let large = count(|why| matches!(why, Excluded::TooLarge { .. }));
        let budget = count(|why| matches!(why, Excluded::BudgetExhausted));

        prop_assert!(
            summary.contains(&format!("{} untracked file(s)", selection.included.len())),
            "{}", summary
        );
        if build > 0 {
            prop_assert!(
                summary.contains(&format!("{build} skipped as build output")),
                "{}", summary
            );
        }
        if large > 0 {
            prop_assert!(
                summary.contains(&format!("{large} skipped as too large")),
                "{}", summary
            );
        }
        if budget > 0 {
            prop_assert!(summary.contains(&format!("{budget} skipped")), "{}", summary);
            prop_assert!(
                summary.contains("budget"),
                "a full checkpoint budget is a different fix from a file that is too big: {}",
                summary
            );
        }
        prop_assert_eq!(build + large + budget, selection.excluded.len());
    }

    /// The limits are limits. Stated separately from the reasons because these are the two
    /// numbers a node's owner sets, and a selection that overran either one would be a
    /// checkpoint bigger than the machine agreed to make.
    #[test]
    fn nothing_carried_breaks_a_limit(entries in entries(), policy in policy()) {
        let selection = select(&entries, &policy, &nothing_tracked());
        let size = |path: &str| {
            entries.iter().find(|(p, _)| p == path).map(|(_, s)| *s).unwrap()
        };

        let mut total = 0;
        for path in &selection.included {
            prop_assert!(size(path) <= policy.max_file_bytes, "{}", path);
            total += size(path);
        }
        prop_assert_eq!(total, selection.included_bytes);
        prop_assert!(selection.included_bytes <= policy.max_total_bytes);
    }

    /// Git lists a worktree in its own order, and that order is not a policy. Two nodes — or
    /// the same node twice — must decide the same thing about the same worktree.
    #[test]
    fn the_order_git_listed_them_in_does_not_decide_anything(
        entries in entries(),
        policy in policy(),
    ) {
        let mut reversed = entries.clone();
        reversed.reverse();
        prop_assert_eq!(select(&entries, &policy, &nothing_tracked()), select(&reversed, &policy, &nothing_tracked()));

        let mut by_size = entries.clone();
        by_size.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
        prop_assert_eq!(select(&entries, &policy, &nothing_tracked()), select(&by_size, &policy, &nothing_tracked()));
    }

    /// Which files a full budget sacrifices. `select` sorts ascending so that "if something
    /// has to go, it should be the artefact rather than the source file that happened to be
    /// listed last" — small files are overwhelmingly the hand-written ones.
    #[test]
    fn a_full_budget_sacrifices_the_biggest(entries in entries(), policy in policy()) {
        let selection = select(&entries, &policy, &nothing_tracked());
        let size = |path: &str| {
            entries.iter().find(|(p, _)| p == path).map(|(_, s)| *s).unwrap()
        };

        for (dropped, why) in &selection.excluded {
            if !matches!(why, Excluded::BudgetExhausted) {
                continue;
            }
            for kept in &selection.included {
                prop_assert!(
                    (size(dropped), dropped) > (size(kept), kept),
                    "{} was dropped for the budget while the larger {} was carried",
                    dropped, kept
                );
            }
        }
    }

    /// The repository's own answer beats the list of names, whatever the path looks like.
    /// A directory this repo keeps tracked files in is content — so no candidate is ever
    /// dropped *as build output* on the strength of a name that repo has signed for. It may
    /// still be too big, or arrive once the budget is full; those are different sentences and
    /// both name themselves.
    #[test]
    fn a_directory_the_repository_tracks_is_never_called_build_output(
        entries in entries(),
        policy in policy(),
        tracked_names in prop::collection::hash_set(
            prop_oneof![
                Just("target".to_string()),
                Just("build".to_string()),
                Just("dist".to_string()),
                Just("node_modules".to_string()),
                Just("api".to_string()),
            ],
            0..4,
        ),
    ) {
        // What a repo that tracks files in those directories looks like to `select`: every
        // ancestor directory of every candidate whose own name the repo has signed for.
        let mut tracked = BTreeSet::new();
        for (path, _) in &entries {
            let mut components: Vec<&str> = path.split('/').collect();
            components.pop();
            let mut prefix = String::new();
            for component in components {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(component);
                if tracked_names.contains(component) {
                    tracked.insert(prefix.clone());
                }
            }
        }

        let selection = select(&entries, &policy, &tracked);
        for (path, why) in &selection.excluded {
            if let Excluded::BuildOutput { directory } = why {
                prop_assert!(
                    !tracked_names.contains(directory),
                    "{} was called build output because of {}, which this repo tracks",
                    path, directory
                );
            }
        }
    }

    /// Conservative in the include direction, as a comparison rather than an adjective: a
    /// policy with more room never carries less. Worth asserting because the selection is a
    /// greedy prefix over a sorted list, and a bigger per-file limit puts *new* files into
    /// that list — so "more room" is not obviously monotonic in the thing being spent.
    #[test]
    fn a_more_generous_policy_never_drops_more(
        entries in entries(),
        policy in policy(),
        extra_file in 0u64..2_000_000,
        extra_total in 0u64..8_000_000,
    ) {
        let generous = UntrackedPolicy {
            max_file_bytes: policy.max_file_bytes + extra_file,
            max_total_bytes: policy.max_total_bytes + extra_total,
            also_never_source: policy.also_never_source.clone(),
        };

        let tight: BTreeSet<String> = select(&entries, &policy, &nothing_tracked()).included.into_iter().collect();
        let roomy: BTreeSet<String> = select(&entries, &generous, &nothing_tracked()).included.into_iter().collect();
        prop_assert!(
            tight.is_subset(&roomy),
            "carried under the tighter policy and not the looser one: {:?}",
            tight.difference(&roomy).collect::<Vec<_>>()
        );
    }
}
