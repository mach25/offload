//! Recurring work: a schedule, its ticks, and the occurrences they fire (ADR-0019 §3, ADR-0056).
//!
//! A **schedule** is an interval plus the run to make. It is created on one node, gossiped to the
//! fleet, and fired by exactly one node at a time — its home while that node is available, else
//! the lowest-id available node, which is arbitration's own successor rule and is factored rather
//! than restated ([`crate::view::ClusterView::steward_of`]).
//!
//! Everything here is integer arithmetic on UTC milliseconds, and that is the point rather than a
//! simplification. The whole design rests on **every node deriving the same occurrence id from
//! the same schedule with no message**: bytes 0..6 of a `RunId` are a UUIDv7's 48-bit millisecond
//! clock, so putting the *tick* there makes the id a value anybody can compute, offline, and
//! `merge_run` settles two firings of one tick as one run. A timezone would break exactly that —
//! see ADR-0056 §1 for why cron and local time are refused, and what it costs.
//!
//! There is deliberately **no gossiped cursor**. A schedule has no `last_fired` on the wire,
//! because that would be a mutable shared field describing something the fleet already records
//! with an owner and a merge rule — the occurrence itself. The only mutable field here is the
//! tombstone. What each node *does* keep is a node-local high-water mark
//! (`schedules.last_tick_ms` in the store), because an occurrence's record is reclaimed about an
//! hour after it finishes and a daily schedule's tick lasts a day; ADR-0056 §2 has the whole
//! argument, including why the first draft of it was wrong.

use crate::id::{RunId, ScheduleId};
use crate::run::{Restartability, RunSpec};
use crate::time::Millis;
use serde::{Deserialize, Serialize};

/// The shortest interval a schedule may have.
///
/// Not a performance limit — a task is a process and the fleet can spawn one a second. It is a
/// **bill** limit and a notification limit: an occurrence can be an agent run, every occurrence is
/// a record that gossips, and `Notices` fans one out. A minute is the granularity a person thinks
/// in when they say "watch this", and anything faster is a program that should be watching for
/// itself and firing a trigger (ADR-0020).
pub const MIN_EVERY: Millis = Millis::from_secs(60);

/// One standing instruction on a clock (ADR-0019 §3).
///
/// Every field but [`Self::removed_at`] is immutable, which is what makes the merge rule small
/// enough to be worth having: there is nothing for two copies to disagree about except whether it
/// has been removed, and that is monotonic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: ScheduleId,
    /// The node it was created on, and its steward while that node is available.
    ///
    /// Named `home` for [`crate::run::Run::home`]'s reason and settled by the same rule: this is
    /// who *decides*, not who runs the work. An occurrence is placed by an ordinary bid round and
    /// may land anywhere in the fleet.
    pub home: crate::id::NodeId,
    /// How often it fires. At least [`MIN_EVERY`].
    pub every: Millis,
    /// Where in the period the tick falls, from the epoch-aligned boundary. Always `< every`.
    ///
    /// `every = 24h, offset = 3h` fires at 03:00 **UTC**. This is what stops a daily schedule
    /// from being a fixed appointment at midnight, and it is the only concession to "at a time"
    /// that survives having no timezone database (ADR-0056 §1).
    pub offset: Millis,
    /// What to run, complete. See ADR-0056 §3: a peer that fires this may never have seen the
    /// creating node's config, so the spec cannot be built at the firing the way a rule's is.
    pub spec: RunSpec,
    /// One line for `offload schedules`, written by whoever created it. Never matched on.
    #[serde(default)]
    pub note: String,
    pub created_at: Millis,
    /// When the home node removed it, if it has.
    ///
    /// The tombstone (ADR-0056 §5). A gossiped set with removals needs one, or a peer that has
    /// learned a schedule goes on firing it after the person who wrote it has deleted it. Set by
    /// the home node, monotonic, and any copy carrying it wins [`Self::merge`].
    #[serde(default)]
    pub removed_at: Option<Millis>,
}

/// Why a schedule could not be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleProblem {
    #[error(
        "a schedule must fire at least a minute apart: an occurrence is a record, a \
         notification and possibly a bill, and something that needs to look more often than \
         that is a program that should watch for itself and fire a trigger"
    )]
    TooOften,
    #[error(
        "the offset into the period has to be shorter than the period itself — `every 24h at \
         25h` names no instant"
    )]
    OffsetTooLarge,
    #[error(
        "a scheduled run is `idempotent`: an occurrence that has to move starts again from its \
         spec, because there is no conversation to resume and nobody waiting to be asked"
    )]
    NotIdempotent,
}

impl Schedule {
    /// Refuse at creation what cannot be fixed later.
    ///
    /// Asked where a schedule is made — which is one place, in the daemon — rather than at the
    /// CLI, for `RunSpec::check`'s reason: a schedule stored months ago is fired by a daemon that
    /// never saw the parser that wrote it.
    pub fn check(&self) -> Result<(), ScheduleProblem> {
        if self.every < MIN_EVERY {
            return Err(ScheduleProblem::TooOften);
        }
        if self.offset >= self.every {
            return Err(ScheduleProblem::OffsetTooLarge);
        }
        if self.spec.restartability != Restartability::Idempotent {
            return Err(ScheduleProblem::NotIdempotent);
        }
        Ok(())
    }

    /// The tick `now` falls in: the instant this period began.
    ///
    /// Pure integer arithmetic against the unix epoch, so two nodes with clocks a millisecond
    /// apart still agree unless `now` is within a millisecond of a boundary — and when they do
    /// disagree, the derived id makes the two firings one record. That is the net ADR-0056 §4
    /// describes, and it is why this needs no clock agreement to be correct.
    #[must_use]
    pub fn tick_at(&self, now: Millis) -> Millis {
        let period = self.every.0.max(1);
        let offset = self.offset.0 % period;
        // Before the first offset boundary of the epoch there is no earlier tick to name, so the
        // answer is the boundary itself. Reachable only for a `now` in 1970, which is to say in a
        // test — and answering `0` there would make every such schedule share one occurrence id.
        let Some(since) = now.0.checked_sub(offset) else {
            return Millis(offset);
        };
        Millis((since / period) * period + offset)
    }

    /// The instant the tick containing `now` ends, which is the next tick.
    #[must_use]
    pub fn next_tick_after(&self, now: Millis) -> Millis {
        self.tick_at(now).saturating_add(self.every)
    }

    /// Is this tick one this schedule should ever fire?
    ///
    /// **A schedule cannot have been due before it existed**, and without this it is: the tick
    /// containing the moment of creation *started* before it, so the first pass after `offload
    /// every 15m` fires immediately and then again at the next boundary. Measured on a daemon
    /// —`first tick in 36.9s` printed at the keyboard, an occurrence in the log five seconds
    /// later — which is two firings eight minutes apart from something that says `every 15m`,
    /// and a report that was wrong about the one thing somebody had just asked.
    ///
    /// Compared against `created_at` rather than remembered locally, because it has to be the
    /// **same answer on every node**: a peer that learned the schedule a second after it was
    /// made has no local memory of the birth tick, and would otherwise fire the very occurrence
    /// the creating node declined to.
    #[must_use]
    pub fn fires_tick(&self, tick: Millis) -> bool {
        tick >= self.created_at
    }

    /// The id the occurrence for `tick` has, on every node, without asking anybody.
    ///
    /// Bytes 0..6 are the tick's unix milliseconds, which is what a UUIDv7 timestamp already
    /// means — so `RunId::short()` goes on being the clock it has always been and the twelve
    /// displayed characters go on meaning what they mean. The remaining bytes are a digest of the
    /// schedule id, with the version and variant nibbles stamped afterwards so the result is a
    /// well-formed v7 rather than merely v7-shaped.
    ///
    /// Deterministic on purpose, and it is the whole of ADR-0019 §3's answer to *two nodes firing
    /// one tick*: the second firing is the same record, so `merge_run` settles it. Read the ADR
    /// before treating it as permission to place a run twice — a duplicate record merges and a
    /// duplicate agent does not.
    #[must_use]
    pub fn occurrence_id(&self, tick: Millis) -> RunId {
        let mut bytes = [0u8; 16];
        // 48 bits of millisecond clock, big-endian, exactly where a v7 keeps it.
        let ms = tick.0 & 0x0000_ffff_ffff_ffff;
        bytes[0..6].copy_from_slice(&ms.to_be_bytes()[2..8]);

        // The rest from the schedule, so two schedules firing the same tick are two runs. The
        // tick goes into the digest as well as into the prefix: without it, an id whose clock
        // bytes were truncated to 48 bits would collide with the same schedule 8,900 years
        // later, which nobody will meet and which costs one line to make impossible.
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"offload/schedule-occurrence/v1");
        hasher.update(self.id.as_bytes());
        hasher.update(&tick.0.to_be_bytes());
        let digest = hasher.finalize();
        bytes[6..16].copy_from_slice(&digest.as_bytes()[..10]);

        // Version 7 in the high nibble of byte 6, and the RFC 4122 variant in the top two bits
        // of byte 8. Costs two lines and means anything that formats this as a UUID gets a
        // truthful one.
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        RunId::from_bytes(bytes)
    }

    /// Has the home node taken this schedule away?
    #[must_use]
    pub fn is_removed(&self) -> bool {
        self.removed_at.is_some()
    }

    /// Settle two copies of one schedule.
    ///
    /// Every field but the tombstone is immutable, so there is nothing else to arbitrate: the
    /// answer is "whichever copy knows it was removed", and it is monotonic, so the fleet
    /// converges however the gossip is ordered. The earlier `removed_at` wins between two copies
    /// that both carry one, for the same reason every tiebreak here has to be total: two nodes
    /// that ordered it differently would disagree for ever.
    #[must_use]
    pub fn merge(&self, other: &Schedule) -> Schedule {
        debug_assert_eq!(self.id, other.id, "merging two different schedules");
        match (self.removed_at, other.removed_at) {
            (None, Some(_)) => other.clone(),
            (Some(mine), Some(theirs)) if theirs < mine => other.clone(),
            _ => self.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capacity::Demand;
    use crate::constraint::Constraint;
    use crate::id::NodeId;
    use crate::run::{TaskWork, Work};

    fn schedule(every: Millis, offset: Millis) -> Schedule {
        Schedule {
            id: ScheduleId::from_bytes([7; 8]),
            home: NodeId::from_bytes([1; 32]),
            every,
            offset,
            spec: RunSpec {
                work: Work::Task(TaskWork {
                    service: crate::Service::Other("watch-api".into()),
                    args: Vec::new(),
                }),
                constraint: Constraint::Always,
                restartability: Restartability::Idempotent,
                priority: 0,
                queue: false,
                deadline: None,
                demand: Demand::Light,
                notify: Default::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: crate::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            note: String::new(),
            created_at: Millis(0),
            removed_at: None,
        }
    }

    #[test]
    fn a_tick_is_the_same_instant_on_every_node() {
        // The property everything else rests on: the tick is a function of the schedule and the
        // clock, so two nodes compute it without exchanging anything.
        let every_15m = schedule(Millis::from_mins(15), Millis::ZERO);
        let boundary = Millis::from_mins(15 * 4_000_000);

        assert_eq!(every_15m.tick_at(boundary), boundary, "on the boundary");
        assert_eq!(
            every_15m.tick_at(boundary.saturating_add(Millis(1))),
            boundary,
            "a millisecond in is the same tick"
        );
        assert_eq!(
            every_15m.tick_at(boundary.saturating_add(Millis::from_mins(14))),
            boundary,
            "fourteen minutes in is still the same tick"
        );
        assert_eq!(
            every_15m.tick_at(boundary.saturating_add(Millis::from_mins(15))),
            boundary.saturating_add(Millis::from_mins(15)),
            "and the next boundary is the next tick"
        );
        assert_eq!(
            every_15m.next_tick_after(boundary.saturating_add(Millis(1))),
            boundary.saturating_add(Millis::from_mins(15))
        );
    }

    #[test]
    fn an_offset_moves_the_whole_ladder_and_never_off_it() {
        // What makes `every 24h` an appointment rather than midnight UTC.
        let daily_at_three = schedule(Millis::from_mins(24 * 60), Millis::from_mins(3 * 60));
        // 1970-01-02T03:00:00Z, and the tick before it.
        let day = Millis::from_mins(24 * 60);
        let three_am_day_two = day.saturating_add(Millis::from_mins(3 * 60));

        assert_eq!(daily_at_three.tick_at(three_am_day_two), three_am_day_two);
        assert_eq!(
            daily_at_three.tick_at(three_am_day_two.saturating_sub(Millis(1))),
            Millis::from_mins(3 * 60),
            "a millisecond before 03:00 belongs to yesterday's tick"
        );
        // Before the first offset boundary there is no earlier tick to name.
        assert_eq!(daily_at_three.tick_at(Millis(0)), Millis::from_mins(3 * 60));
    }

    #[test]
    fn a_schedule_is_not_due_for_the_tick_it_was_born_in() {
        // The tick containing the moment of creation began before it, so firing it means
        // `every 15m` produces two occurrences eight minutes apart — and the "first tick in
        // 36.9s" a person was just told is wrong. Every node answers this the same way,
        // because `created_at` is immutable and gossiped: a peer that learns the schedule
        // immediately must decline the same occurrence the creator did.
        let boundary = Millis::from_mins(15 * 4_000_000);
        let born = boundary.saturating_add(Millis::from_mins(7));
        let schedule = Schedule {
            created_at: born,
            ..schedule(Millis::from_mins(15), Millis::ZERO)
        };

        assert_eq!(schedule.tick_at(born), boundary);
        assert!(
            !schedule.fires_tick(boundary),
            "the tick it was born in had already started"
        );
        assert!(
            schedule.fires_tick(schedule.next_tick_after(born)),
            "and the next one is the first it fires — which is what the CLI printed"
        );

        // A schedule created exactly on a boundary fires that tick, because it *is* due: there
        // is nothing about it that had already happened.
        let punctual = Schedule {
            created_at: boundary,
            ..schedule.clone()
        };
        assert!(punctual.fires_tick(boundary));
    }

    #[test]
    fn an_occurrence_id_is_a_uuidv7_whose_clock_is_the_tick() {
        // Two properties in one: the displayed prefix is still the clock (`RunId::short` is
        // twelve characters *because* those bytes are a millisecond timestamp), and the id is a
        // well-formed v7 rather than merely v7-shaped.
        let every_15m = schedule(Millis::from_mins(15), Millis::ZERO);
        let tick = Millis(1_757_000_000_000);
        let id = every_15m.occurrence_id(tick);
        let bytes = id.as_bytes();

        let mut clock = [0u8; 8];
        clock[2..8].copy_from_slice(&bytes[0..6]);
        assert_eq!(
            u64::from_be_bytes(clock),
            tick.0,
            "the tick is readable straight out of the id"
        );
        assert_eq!(bytes[6] >> 4, 0x7, "version 7");
        assert_eq!(bytes[8] >> 6, 0b10, "RFC 4122 variant");
    }

    #[test]
    fn two_nodes_firing_one_tick_name_one_run_and_two_ticks_never_collide() {
        let mine = schedule(Millis::from_mins(15), Millis::ZERO);
        // A second node's copy of the *same* schedule — the point of gossiping the definition
        // rather than nominating it twice (ADR-0056's rejected alternative).
        let theirs = Schedule {
            home: NodeId::from_bytes([9; 32]),
            ..mine.clone()
        };
        let tick = Millis(1_757_000_000_000);

        assert_eq!(
            mine.occurrence_id(tick),
            theirs.occurrence_id(tick),
            "one tick, one record — whoever fires it"
        );
        assert_ne!(
            mine.occurrence_id(tick),
            mine.occurrence_id(tick.saturating_add(mine.every)),
            "the next tick is a different run"
        );

        // …and two schedules that happen to share a tick are two runs.
        let other_schedule = Schedule {
            id: ScheduleId::from_bytes([8; 8]),
            ..mine.clone()
        };
        assert_ne!(mine.occurrence_id(tick), other_schedule.occurrence_id(tick));
    }

    #[test]
    fn a_tombstone_wins_whichever_way_the_gossip_arrives() {
        let live = schedule(Millis::from_mins(15), Millis::ZERO);
        let removed = Schedule {
            removed_at: Some(Millis(500)),
            ..live.clone()
        };

        assert!(live.merge(&removed).is_removed(), "learned from a peer");
        assert!(removed.merge(&live).is_removed(), "and not un-learned");
        // Total, not merely monotonic: two nodes that saw two removal instants have to pick the
        // same one or they disagree for ever.
        let later = Schedule {
            removed_at: Some(Millis(900)),
            ..live.clone()
        };
        assert_eq!(removed.merge(&later).removed_at, Some(Millis(500)));
        assert_eq!(later.merge(&removed).removed_at, Some(Millis(500)));
    }

    #[test]
    fn what_is_refused_at_creation() {
        let fine = schedule(Millis::from_mins(15), Millis::ZERO);
        assert_eq!(fine.check(), Ok(()));

        assert_eq!(
            schedule(Millis::from_secs(30), Millis::ZERO).check(),
            Err(ScheduleProblem::TooOften)
        );
        assert_eq!(
            schedule(Millis::from_mins(15), Millis::from_mins(15)).check(),
            Err(ScheduleProblem::OffsetTooLarge),
            "an offset of a whole period names the next boundary, not a time within this one"
        );

        let resumable = Schedule {
            spec: RunSpec {
                restartability: Restartability::Resumable,
                ..fine.spec.clone()
            },
            ..fine.clone()
        };
        assert_eq!(resumable.check(), Err(ScheduleProblem::NotIdempotent));
    }
}
