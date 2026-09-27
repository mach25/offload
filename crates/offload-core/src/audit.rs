//! What this machine decided about a run, and what it was refused.
//!
//! The two questions this project's users actually ask are "why is my run *there*" and "why
//! didn't that happen", and neither has a durable answer today. `offload explain` deliberately
//! re-asks the fleet rather than replaying the round that placed the run — a bid describes one
//! second, so a stored one presented as current is a confident wrong answer — which is right, and
//! leaves the historical decision unrecoverable. And a fence firing, which is the single most
//! important event in this design, lives in a `tracing` line on a machine nobody is logged into.
//!
//! **A third log, and not one of the other two.** A run's log is the run's *output*: the agent's
//! text, its turns, what a checkpoint captured. It is read by whoever is following the run and
//! served from whichever node holds it — so a line written by a leg that has just been **fenced
//! out** is a line nobody will ever read, because that node is by definition not the holder. And
//! this is not about the fleet.
//!
//! What it is: what *this machine* did. Per-node and never gossiped, for
//! [`crate::fleet_event`]'s reason — two nodes recording the same grant are not disagreeing, they
//! are each describing what they saw, and merging them would mean deciding which observation is
//! authoritative about an event neither owns (ADR-0005's table, read the other way). Sometimes
//! the answer to "who owns this field" is that it should not be a field.
//!
//! **Deliberately not metrics.** The roadmap bundles this with a Prometheus endpoint, and the two
//! have different justifications: nobody scrapes their phone, and a counter that says three
//! epochs were rejected today answers none of the questions above. What an operator needs is
//! *which* run, *which* machine, and *what was refused* — a log, in order, that can be read after
//! the fact.

use crate::id::{NodeId, RunId};
use crate::run::Epoch;
use serde::{Deserialize, Serialize};

/// One thing this node did about a run, with the moment it did it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at_unix_ms: u64,
    #[serde(flatten)]
    pub kind: AuditEvent,
}

impl AuditEntry {
    /// The run this entry is about. Every variant has one — an audit trail with no subject is a
    /// list of adjectives.
    #[must_use]
    pub fn run(&self) -> RunId {
        match self.kind {
            AuditEvent::Granted { run, .. }
            | AuditEvent::Accepted { run, .. }
            | AuditEvent::Refused { run, .. }
            | AuditEvent::Superseded { run, .. }
            | AuditEvent::Reclaimed { run, .. }
            | AuditEvent::Rescued { run, .. } => run,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "audit", rename_all = "snake_case")]
pub enum AuditEvent {
    /// This node, arbitrating, gave the run to somebody — possibly itself.
    ///
    /// The epoch is the point. It is the fencing token that grant was made under, so a pair of
    /// these at one number is one arbiter having spent a token twice, which is the failure the
    /// whole epoch scheme exists to prevent and which nothing durable recorded until now.
    Granted {
        run: RunId,
        to: NodeId,
        epoch: Epoch,
    },
    /// This node took a run, under the epoch it was granted at.
    ///
    /// The other half of an assignment, from the machine that has to honour it. Recorded
    /// separately rather than inferred from `Granted` because silence is a decline (ADR-0006):
    /// the ordinary failure is a node that took the grant and whose answer never came back, and
    /// the two rows are what tell that apart from a node that never got it.
    Accepted { run: RunId, epoch: Epoch },
    /// This node was refused a write about the run, because the run is no longer its own.
    ///
    /// The rows worth having. Every one of them is a moment when two agents on one repository was
    /// prevented rather than merely unlikely — and the reason it is *this* node writing it is
    /// that the fenced-out leg is the only one that knows.
    Refused {
        run: RunId,
        /// What it was trying to do. A closed set for a reason — see [`Attempt`].
        attempt: Attempt,
        /// The epoch this node believed it held.
        held: Epoch,
    },
    /// The fleet gave this run to somebody else, and the agent here was stopped.
    ///
    /// Not a [`AuditEvent::Refused`], and the difference is the whole reason this is its own
    /// variant: nothing was attempted. This node was *told* — a gossiped record naming another
    /// holder at or above the epoch it was running under — which is the one moment a leg learns
    /// it lost, and the only place the fact exists at all, because the surviving leg never hears
    /// that there was another one.
    ///
    /// `turns` is why the row is worth writing rather than logging. A run's *position* follows
    /// the leg its record settled on (ADR-0005's second amendment), so the moment a leg is
    /// superseded the run's reported turn count drops back to the survivor's — correct, and
    /// indistinguishable from a fault unless something can say that the higher number belonged to
    /// work which is no longer the run's. This is that something, on the one machine that can say
    /// it.
    Superseded {
        run: RunId,
        /// The epoch this node was running under.
        held: Epoch,
        /// Who the fleet says holds it now, and under what.
        to: NodeId,
        epoch: Epoch,
        /// The turn this leg had reached — the number that is about to stop being the run's.
        turns: u32,
        /// The leg had already **finished** when it learned — its agent done, the run recorded
        /// here as completed at the lower epoch. The case a daemon frozen across its agent's last
        /// turn reaches: on thawing it read the result before it read the gossip. Defaulted, so
        /// rows written before this read as the running case they were.
        #[serde(default)]
        finished: bool,
    },
    /// This node removed a run's checkout from its own disk (ADR-0023).
    ///
    /// The sweep's removal had no durable record of any kind, and the reason is worth keeping
    /// because it is not an oversight: `offload rm` notes `workspace: "removed"` beside the run,
    /// and `RunProgress::workspace` is a single fleet-agreed value written by one leg — so the
    /// node reclaiming a *departed* checkout is by construction the leg that may not write there
    /// (session seventeen's bug). Every field on the record has that shape, which is what left
    /// this as a `tracing::info!` on a machine nobody is logged into.
    ///
    /// So it is not a field. What happened to a directory is a fact about **one disk**, and this
    /// log is the per-node one — the same answer `fleet_event` gives, and the same answer
    /// ADR-0013 gives attendance: sometimes the resolution to "who owns this field" is that it
    /// should not be one. The row is written by whichever node's disk changed, and no other node
    /// has an opinion to merge with it.
    ///
    /// `why` is the point rather than decoration. The sweep has two doors and they answer
    /// different questions — one says the run is *elsewhere now*, the other that nobody was ever
    /// going to read this output — and "your worktree is gone" without which of them is the
    /// decision having discarded its reasoning.
    Reclaimed { run: RunId, why: Reclamation },
    /// This node set something of its own aside rather than let it go, and nothing will ever
    /// remove it.
    ///
    /// The counterpart to [`AuditEvent::Reclaimed`], and the row exists for the same reason: a
    /// rescue is a fact about **one disk**, with no second opinion anywhere to merge with. Two of
    /// them happen on the path that rebuilds a checkout from a checkpoint — a worktree from an
    /// earlier leg moved aside because it held uncommitted work, and commits this branch had that
    /// the checkpoint did not, given a name before the reset moved off them. Both were a
    /// `tracing::warn!` on a machine nobody is logged into plus a sentence in the *run's* log,
    /// which is served by whichever node **holds** the run — so the leg that did the rescuing is
    /// often not the node anybody can read it from, and once the run is deleted nobody can.
    ///
    /// **The run id is the whole address**, which is why the row needs no path and no hash. Both
    /// names are derived: the copy is `<state>/worktrees/<run>.superseded*` and the commits are
    /// under `refs/offload/left-behind/<run>/*`. So this row plus the two conventions is enough
    /// to find what was kept, which is exactly what a rescue nobody was told about lacked. The
    /// specifics stay in the run's own log, where there is somebody following it.
    Rescued { run: RunId, what: Rescue },
}

/// Which of the two things a rebuild sets aside.
///
/// A closed set for [`Attempt`]'s reason — "has this machine ever kept a checkout it can no
/// longer ask git about" is a question worth asking of the log, and a free-text noun makes it a
/// substring search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rescue {
    /// A checkout from an earlier leg of this run, moved aside because it held uncommitted work.
    /// One that held none is a [`Reclamation::Redundant`] row instead, since it was removed.
    Checkout,
    /// Commits this checkout had and the checkpoint did not, named before a `reset --hard` moved
    /// the branch off them.
    ///
    /// `named` is `false` for the case worth stating rather than implying: the ref could not be
    /// written, so the reflog really is all that holds them — and an unreachable commit's reflog
    /// entry expires.
    Commits { named: bool },
}

/// Which of the sweep's two doors a reclaimed checkout went through.
///
/// A closed set for [`Attempt`]'s reason: "has this node ever thrown away a checkout for a run
/// that merely moved" is a question worth asking of the log, and a free-text reason makes it a
/// substring search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reclamation {
    /// Another leg produced the run's latest position, so the run has moved on and the work
    /// went with it — the checkpoint travelled and committed work is on the branch.
    MovedOn,
    /// A finished machine-started run: nobody submitted it, so nobody is coming to read it
    /// (ADR-0020 §6, made legible to a node that never heard of the rule by ADR-0024).
    NobodyWaiting,
    /// A checkout from an earlier leg, superseded by a checkpoint the fleet has moved past, that
    /// held nothing uncommitted — so the run branch in the mirror already had every byte of it.
    ///
    /// The third door, and the one that is a *measurement* rather than a judgement about who is
    /// watching: `git status` was asked while the worktree could still answer. Its sibling is
    /// [`Rescue::Checkout`], the same door with the opposite answer.
    Redundant,
}

impl Reclamation {
    /// Why the checkout went, as a clause that follows a colon.
    ///
    /// Lifted out of [`AuditEvent::describe`] so `offload rm` can say what became of a worktree
    /// in the same words `offload audit` does. Two renderings of one fact drift, and this one is
    /// read by somebody deciding whether their work survived.
    #[must_use]
    pub fn why(&self) -> &'static str {
        match self {
            Reclamation::MovedOn => "another node has run it since, so the work went with it",
            Reclamation::NobodyWaiting => {
                "a rule's own occurrence, finished, with nobody to read it"
            }
            Reclamation::Redundant => {
                "an earlier leg's, holding nothing uncommitted, so the run branch already had \
                 all of it"
            }
        }
    }
}

/// What a refused write was trying to do.
///
/// A closed set rather than a string, because these rows are read by machine as well as by a
/// person — "has this node ever been fenced out of a start" is the question worth asking, and a
/// free-text verb makes it a substring search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attempt {
    /// About to spawn an agent, and the run had moved. The one that matters most: past this point
    /// there would have been two.
    Start,
    /// About to record the run as failed, and it was not this node's to fail.
    Fail,
    /// About to record the run as cancelled, and it had already ended.
    Cancel,
    /// About to say something about the run's worktree or its numbers, and the run had moved.
    Describe,
}

impl AuditEvent {
    /// A short, stable name for the kind — the `kind` column a scan filters on.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            AuditEvent::Granted { .. } => "granted",
            AuditEvent::Accepted { .. } => "accepted",
            AuditEvent::Refused { .. } => "refused",
            AuditEvent::Superseded { .. } => "superseded",
            AuditEvent::Reclaimed { .. } => "reclaimed",
            AuditEvent::Rescued { .. } => "rescued",
        }
    }

    /// One line, for `offload audit`, with nodes by their short id.
    #[must_use]
    pub fn describe(&self) -> String {
        self.describe_naming(|node| node.short())
    }

    /// One line, with nodes named by `name` — the reader's names for them, which this crate does
    /// not have. `offload audit` printed `the run is 2a3b5d5d's at epoch 2` about a node
    /// `offload nodes` calls `bravo`, on the row that says which machine now has the run.
    #[must_use]
    pub fn describe_naming(&self, name: impl Fn(NodeId) -> String) -> String {
        match self {
            AuditEvent::Granted { to, epoch, .. } => {
                format!("granted to {} at epoch {}", name(*to), epoch.0)
            }
            AuditEvent::Accepted { epoch, .. } => format!("accepted here at epoch {}", epoch.0),
            AuditEvent::Refused { attempt, held, .. } => format!(
                "refused: this node held epoch {} and tried to {}",
                held.0,
                match attempt {
                    Attempt::Start => "start it",
                    Attempt::Fail => "fail it",
                    Attempt::Cancel => "cancel it",
                    Attempt::Describe => "describe it",
                }
            ),
            AuditEvent::Superseded {
                held,
                to,
                epoch,
                turns,
                finished: true,
                ..
            } => format!(
                // Measured, session ninety: alpha frozen across its agent's last turn, bravo
                // granted the run at epoch 2, alpha thawed and completed it at epoch 1 — and the
                // merge put bravo's record over alpha's with nothing on alpha saying so.
                "superseded after finishing: this node completed it at epoch {} after {} turn(s), \
                 and the run is {}'s at epoch {} — what it did here is not the run's",
                held.0,
                turns,
                name(*to),
                epoch.0
            ),
            AuditEvent::Superseded {
                held,
                to,
                epoch,
                turns,
                ..
            } if *turns == 0 => format!(
                // The first one read on a screen was this case — a leg frozen before its first
                // turn ended — and said "had reached turn 0 … the turns after this one are not
                // the run's", about a leg that had no turns to lose.
                "superseded: this node held epoch {} and had not finished a turn, and the run \
                 is {}'s at epoch {} — nothing this leg did is in the run's count",
                held.0,
                name(*to),
                epoch.0
            ),
            AuditEvent::Superseded {
                held,
                to,
                epoch,
                turns,
                ..
            } => format!(
                "superseded: this node held epoch {} and had reached turn {}, and the run is \
                 {}'s at epoch {} — the turns after this one are not the run's",
                held.0,
                turns,
                name(*to),
                epoch.0
            ),
            AuditEvent::Reclaimed { why, .. } => {
                format!("reclaimed this run's checkout here: {}", why.why())
            }
            // **The names are written out, because a placeholder in this line is worse here than
            // anywhere else.** These two rows say where the only copy of somebody's uncommitted
            // work is, and they read `<state>/worktrees/<run>.superseded*` — measured on one
            // daemon, on screen, verbatim. The run id was the part that stung: it is on the row
            // already, abbreviated to twelve by `id_width`, and the directory is named by the
            // **full thirty-two**, so a reader who did substitute what was in front of them got a
            // path that does not exist. `run` is in the variant; this is one interpolation.
            //
            // The state directory is not, and cannot be — this crate has no filesystem and no
            // config. It is named as a thing to look under rather than spelled as a token,
            // because a reader who knows their own state directory can then finish the path, and
            // one who does not has a phrase to search for instead of an angle bracket.
            AuditEvent::Rescued { run, what } => match what {
                Rescue::Checkout => format!(
                    "kept an earlier leg's checkout here: it holds uncommitted work, so it is \
                     at worktrees/{run}.superseded* under this node's state directory, and \
                     nothing will remove it"
                ),
                Rescue::Commits { named: true } => format!(
                    "kept commits this branch had and the checkpoint did not, under \
                     refs/offload/left-behind/{run}/ in this repo's mirror, and nothing will \
                     remove them"
                ),
                Rescue::Commits { named: false } => {
                    "could not name the commits this branch had and the checkpoint did not: \
                     only the reflog holds them, and it expires"
                        .to_string()
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> RunId {
        RunId::from_bytes([7; 16])
    }

    /// The row that says which machine has the run now names it the way the reader does, and a
    /// leg that lost before finishing a turn is not told its later turns are not the run's.
    #[test]
    fn a_superseded_row_names_the_new_holder_and_minds_turn_zero() {
        let bravo = NodeId::from_bytes([2; 32]);
        let named = |node: NodeId| {
            if node == bravo {
                "bravo".to_string()
            } else {
                node.short()
            }
        };
        let lost = |turns| AuditEvent::Superseded {
            run: run(),
            held: Epoch(1),
            to: bravo,
            epoch: Epoch(2),
            turns,
            finished: false,
        };
        let early = lost(0).describe_naming(named);
        assert!(early.contains("bravo's at epoch 2"), "{early}");
        assert!(early.contains("had not finished a turn"), "{early}");
        assert!(!early.contains("turn 0"), "{early}");
        let late = lost(7).describe_naming(named);
        assert!(late.contains("had reached turn 7"), "{late}");
        assert!(lost(7).describe().contains(&bravo.short()));
        let done = AuditEvent::Superseded {
            run: run(),
            held: Epoch(1),
            to: bravo,
            epoch: Epoch(2),
            turns: 1,
            finished: true,
        }
        .describe_naming(named);
        assert!(done.contains("after finishing"), "{done}");
        assert!(
            done.contains("completed it at epoch 1 after 1 turn(s)"),
            "{done}"
        );
        assert!(done.contains("bravo's at epoch 2"), "{done}");
        let granted = AuditEvent::Granted {
            run: run(),
            to: bravo,
            epoch: Epoch(1),
        };
        assert_eq!(
            granted.describe_naming(named),
            "granted to bravo at epoch 1"
        );
    }

    /// Both directions, over the encoding the store uses. Internally-tagged enums have failed at
    /// runtime in this workspace before, and a log that cannot be read back is not a log.
    ///
    /// The list is hand-written, which is the one weakness worth naming: a variant added without
    /// a line here is untested rather than caught, and that is exactly what happened to
    /// `Rescued`. [`Self::the_round_trip_covers_every_kind_there_is`] is the guard against it —
    /// it counts what this list reaches against `kind_name`'s arms.
    #[test]
    fn every_kind_round_trips() {
        let entries = [
            AuditEvent::Granted {
                run: run(),
                to: NodeId::from_bytes([2; 32]),
                epoch: Epoch(4),
            },
            AuditEvent::Accepted {
                run: run(),
                epoch: Epoch(4),
            },
            AuditEvent::Refused {
                run: run(),
                attempt: Attempt::Start,
                held: Epoch(3),
            },
            AuditEvent::Superseded {
                run: run(),
                held: Epoch(4),
                to: NodeId::from_bytes([9; 32]),
                epoch: Epoch(5),
                turns: 19,
                finished: false,
            },
            AuditEvent::Reclaimed {
                run: run(),
                why: Reclamation::MovedOn,
            },
            AuditEvent::Reclaimed {
                run: run(),
                why: Reclamation::NobodyWaiting,
            },
            AuditEvent::Reclaimed {
                run: run(),
                why: Reclamation::Redundant,
            },
            AuditEvent::Rescued {
                run: run(),
                what: Rescue::Checkout,
            },
            AuditEvent::Rescued {
                run: run(),
                what: Rescue::Commits { named: true },
            },
            AuditEvent::Rescued {
                run: run(),
                what: Rescue::Commits { named: false },
            },
        ];
        for kind in entries {
            let entry = AuditEntry {
                at_unix_ms: 1_700_000_000_000,
                kind,
            };
            let json = serde_json::to_string(&entry).expect("encode");
            let back: AuditEntry = serde_json::from_str(&json).expect("decode");
            assert_eq!(back, entry, "{json}");
            assert_eq!(back.run(), run());
            let said = back.kind.describe();
            assert!(!said.is_empty());
            // **No angle brackets, anywhere, in a line an operator reads.** Two arms of `Rescued`
            // rendered `<state>/worktrees/<run>.superseded*` and
            // `refs/offload/left-behind/<run>/` — measured on a daemon, on screen, verbatim, in
            // the one line that says where the only copy of somebody's uncommitted work is. This
            // sweep is here rather than an assertion on those two arms because the mistake is not
            // about rescues: it is about writing a path in a sentence, and the next variant that
            // does it will be a different one. Same shape as `offload logs` printing a literal
            // `offload approve <run> …`, one log over.
            assert!(
                !said.contains('<') && !said.contains('>'),
                "a placeholder reached a person's screen: {said}"
            );
        }
    }

    /// The run id in full, because the row's own id column is not enough to finish the path.
    ///
    /// The sweep above catches the placeholder; this catches the fix that merely *removes* it.
    /// `id_width` abbreviates the listing's id column to twelve characters and the directory is
    /// named by the whole thirty-two, so a reader substituting what is in front of them got
    /// `worktrees/01a095c903c7.superseded*`, which does not exist — measured, with `ls`. The run
    /// is already in the variant, so there is nothing to look up.
    #[test]
    fn a_rescue_names_the_place_it_put_the_work() {
        let full = run().to_string();
        assert_eq!(full.len(), 32, "the directory is named by the whole id");

        let checkout = AuditEvent::Rescued {
            run: run(),
            what: Rescue::Checkout,
        }
        .describe();
        assert!(
            checkout.contains(&format!("worktrees/{full}.superseded")),
            "the path somebody has to type, in full: {checkout}"
        );

        let commits = AuditEvent::Rescued {
            run: run(),
            what: Rescue::Commits { named: true },
        }
        .describe();
        assert!(
            commits.contains(&format!("refs/offload/left-behind/{full}/")),
            "and the ref namespace the commits are under: {commits}"
        );

        // The third arm names no place on purpose — there is none, which is what it says — so it
        // is checked for the opposite: it must not imply one.
        let lost = AuditEvent::Rescued {
            run: run(),
            what: Rescue::Commits { named: false },
        }
        .describe();
        assert!(
            !lost.contains("refs/offload"),
            "nothing was named, so no namespace is offered: {lost}"
        );
    }

    /// The guard on the list above, which is hand-written and therefore goes stale in silence.
    ///
    /// `Rescued` is how this was found: a variant added with no line in `every_kind_round_trips`
    /// is not a failing test, it is a variant nothing has ever encoded — and the encoding is the
    /// only thing between this log and being unreadable. So the arms of [`AuditEvent::kind_name`]
    /// are the enumeration, and every one of them has to be reachable from the list.
    ///
    /// What it does **not** reach, said rather than implied: a new arm of [`Reclamation`],
    /// [`Rescue`] or [`Attempt`] is a new *payload* under a kind this already covers, so it
    /// passes here untested. Those are enumerated by hand above too, and nothing checks them.
    #[test]
    fn the_round_trip_covers_every_kind_there_is() {
        // Constructed once per kind, so this list is what `kind_name`'s arms are checked against
        // — the same values the round trip walks, with the payloads that vary collapsed out.
        let covered = [
            AuditEvent::Granted {
                run: run(),
                to: NodeId::from_bytes([2; 32]),
                epoch: Epoch(1),
            },
            AuditEvent::Accepted {
                run: run(),
                epoch: Epoch(1),
            },
            AuditEvent::Refused {
                run: run(),
                attempt: Attempt::Start,
                held: Epoch(1),
            },
            AuditEvent::Superseded {
                run: run(),
                held: Epoch(1),
                to: NodeId::from_bytes([9; 32]),
                epoch: Epoch(2),
                turns: 1,
                finished: false,
            },
            AuditEvent::Reclaimed {
                run: run(),
                why: Reclamation::MovedOn,
            },
            AuditEvent::Rescued {
                run: run(),
                what: Rescue::Checkout,
            },
        ];
        let names: Vec<&str> = covered.iter().map(AuditEvent::kind_name).collect();

        // The enum's own source is the authority on how many there are, because there is no type
        // that says so — `offload-core/tests/no_clock.rs`'s reason, in a smaller place. A
        // variant is a line at one level of indentation ending in `{` or `,` inside the `enum
        // AuditEvent` block; the doc comments above each are what the filter is for.
        let src = include_str!("audit.rs");
        let body = src
            .split_once("pub enum AuditEvent {")
            .expect("this file declares AuditEvent")
            .1;
        let body = body.split_once("\n}").expect("and closes it").0;
        let declared = body
            .lines()
            .filter(|line| {
                let t = line.trim();
                !t.starts_with("///") && !t.starts_with("//") && !t.is_empty()
            })
            .filter(|line| {
                // `Accepted { run: RunId, epoch: Epoch },` and `Granted {` both start a variant;
                // the continuation lines of a multi-line one are indented further.
                line.starts_with("    ") && !line.starts_with("        ")
            })
            .filter(|line| line.trim().chars().next().is_some_and(char::is_uppercase))
            .count();

        assert_eq!(
            names.len(),
            declared,
            "the round trip covers {names:?}, and this file declares {declared} kinds — a kind \
             with no line in `every_kind_round_trips` has never been encoded"
        );
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "one kind, one name: {names:?}");
    }

    /// The two doors are two different pieces of news, and one sentence for both is the
    /// decision having discarded its reasoning.
    ///
    /// Written out rather than computed from the enum: a test that asks `describe` what
    /// `describe` said passes on any answer at all, which is the tautology session fifty found
    /// in the resumable sweep.
    #[test]
    fn a_reclaimed_checkout_names_which_door_it_went_through() {
        let moved = AuditEvent::Reclaimed {
            run: run(),
            why: Reclamation::MovedOn,
        };
        let unwatched = AuditEvent::Reclaimed {
            run: run(),
            why: Reclamation::NobodyWaiting,
        };
        assert!(
            moved.describe().contains("another node has run it since"),
            "{}",
            moved.describe()
        );
        assert!(
            unwatched.describe().contains("nobody to read it"),
            "{}",
            unwatched.describe()
        );
        assert_ne!(
            moved.describe(),
            unwatched.describe(),
            "one sentence for both doors is the reason the tracing line was not enough"
        );
        assert_eq!(moved.kind_name(), "reclaimed");
        assert_eq!(unwatched.kind_name(), "reclaimed");
    }

    #[test]
    fn a_refusal_names_what_it_was_refused() {
        // The line an operator reads at 07:00, so it has to say which of the four it was rather
        // than "fenced".
        let refused = AuditEvent::Refused {
            run: run(),
            attempt: Attempt::Start,
            held: Epoch(3),
        };
        assert!(
            refused.describe().contains("start it"),
            "{}",
            refused.describe()
        );
        assert_eq!(refused.kind_name(), "refused");
    }

    #[test]
    fn a_supersession_names_both_numbers_somebody_is_asking_about() {
        // The question this row exists to answer is "why does `ps` say turn 3 when I watched it
        // reach 19", so the line has to carry the number that was on screen *and* the one that
        // replaced it. Naming only the epochs would be a true sentence about a fencing token and
        // no answer at all.
        let lost = AuditEvent::Superseded {
            run: run(),
            held: Epoch(4),
            to: NodeId::from_bytes([9; 32]),
            epoch: Epoch(5),
            turns: 19,
            finished: false,
        };
        let line = lost.describe();
        assert!(line.contains("turn 19"), "{line}");
        assert!(
            line.contains("epoch 4") && line.contains("epoch 5"),
            "{line}"
        );
        assert_eq!(lost.kind_name(), "superseded");
    }
}
