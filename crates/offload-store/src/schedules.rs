//! Standing instructions on a clock (ADR-0019 §3, ADR-0056).
//!
//! The first table in this crate whose rows are **gossiped**, and every difference from
//! `rules.rs` next door follows from that one fact:
//!
//! * A row may be about another node's schedule, so `home` is stored rather than implied — and
//!   *who fires it* is a question about that node's liveness, which is the daemon's to ask.
//! * Removal is an UPDATE, not a DELETE. A deleted row comes straight back from the next peer
//!   that gossips it; the tombstone is what makes removal a fact the fleet can hold (ADR-0056 §5).
//! * A write from gossip must not overwrite what this node knows, so [`Store::merge_schedule`]
//!   settles the two copies through `Schedule::merge` — the domain's rule, not a second one here.
//!
//! `spec_json` is a whole `offload_core::RunSpec` as text, which is `run_json`'s arrangement one
//! layer along: the store owns the row and the daemon owns the meaning. Unlike a rule's
//! `request_json` it is a *domain* type rather than a control-protocol one, because the node that
//! fires a schedule may never have seen the config of the node that created it.

use crate::{Store, StoreError};
use offload_core::{Millis, Schedule, ScheduleId};
use rusqlite::OptionalExtension;

/// What [`Store::remove_schedule`] found when it was asked to remove one.
///
/// Three answers rather than a `bool`, because two of them were the same `true` and only one of
/// them was a removal — so `offload unschedule` on a tombstone printed *"removed <id>"* and the
/// whole explanation of what had just travelled, having done nothing and republished nothing
/// new. A removal command may be idempotent; it may not claim an act it did not perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// There was a live schedule, and this call wrote its tombstone.
    Done,
    /// It was already a tombstone. The instant carried is the *first* removal's, which is the
    /// one the fleet agrees on (`Schedule::merge` keeps the earlier).
    Already(Millis),
    /// No such schedule here at all.
    Missing,
}

impl Store {
    /// Write a schedule, or take an incoming copy of one, settling the two.
    ///
    /// One function for both directions on purpose. A node creating a schedule and a node
    /// learning one from gossip are writing the same row, and the only field that can differ is
    /// the tombstone — so the merge rule is asked every time rather than on a path somebody has
    /// to remember to route through. `Schedule::merge` is that rule and it lives in the domain,
    /// where it is a pure function with tests.
    ///
    /// Returns what the row says afterwards, which is what the caller should believe.
    pub fn merge_schedule(&self, incoming: &Schedule) -> Result<Schedule, StoreError> {
        let settled = match self.schedule(incoming.id)? {
            Some(existing) => existing.merge(incoming),
            None => incoming.clone(),
        };
        let spec_json = serde_json::to_string(&settled.spec)
            .map_err(|e| StoreError::Encode(format!("schedule spec: {e}")))?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO schedules
                 (id, home, every_ms, offset_ms, spec_json, note, created_at_ms, removed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 -- Only the tombstone. Every other column is immutable in the domain type, and
                 -- a peer relaying an older copy must not be able to rewrite a definition —
                 -- which is the shape of bug `runs`' own conflict list is commented for.
                 removed_at_ms = excluded.removed_at_ms",
            rusqlite::params![
                settled.id.as_bytes().as_slice(),
                settled.home.as_bytes().as_slice(),
                i64::try_from(settled.every.0).unwrap_or(i64::MAX),
                i64::try_from(settled.offset.0).unwrap_or(i64::MAX),
                spec_json,
                settled.note,
                i64::try_from(settled.created_at.0).unwrap_or(i64::MAX),
                settled
                    .removed_at
                    .map(|at| i64::try_from(at.0).unwrap_or(i64::MAX)),
            ],
        )?;
        Ok(settled)
    }

    /// The last tick **this node** fired for a schedule, if it has fired one.
    ///
    /// Node-local and deliberately not part of [`offload_core::Schedule`]: it is this machine's
    /// memory of what it did, no peer can state it, and putting it in the domain type would put
    /// it on the wire where it would need an owner and a merge rule for a fact nobody else
    /// holds (ADR-0005). The column is outside `merge_schedule`'s conflict list for the same
    /// reason `runs.rule` is outside `write_run`'s.
    pub fn schedule_last_tick(&self, id: ScheduleId) -> Result<Option<Millis>, StoreError> {
        let conn = self.conn();
        let tick: Option<Option<i64>> = conn
            .query_row(
                "SELECT last_tick_ms FROM schedules WHERE id = ?1",
                [id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(tick.flatten().map(|ms| Millis(ms.unsigned_abs())))
    }

    /// Remember that this node fired `tick`.
    ///
    /// A high-water mark rather than an assignment: the passes that read it are polls, and a
    /// clock that steps backwards — an NTP correction, a suspended laptop — must not let a tick
    /// already served be served again.
    pub fn note_schedule_fired(&self, id: ScheduleId, tick: Millis) -> Result<(), StoreError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE schedules SET last_tick_ms = max(coalesce(last_tick_ms, 0), ?2)
             WHERE id = ?1",
            rusqlite::params![
                id.as_bytes().as_slice(),
                i64::try_from(tick.0).unwrap_or(i64::MAX)
            ],
        )?;
        Ok(())
    }

    /// One schedule, tombstoned or not.
    pub fn schedule(&self, id: ScheduleId) -> Result<Option<Schedule>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, home, every_ms, offset_ms, spec_json, note, created_at_ms, removed_at_ms
             FROM schedules WHERE id = ?1",
        )?;
        stmt.query_row([id.as_bytes().as_slice()], decode_schedule)
            .optional()?
            .transpose()
    }

    /// Every schedule this node knows, its own and the fleet's, oldest first.
    ///
    /// **Tombstones included**, because they are what has to be gossiped: a caller that wants
    /// only the live ones filters, and the one that gossips must not. Two callers, opposite
    /// needs, and the shorter list is the one that is easy to reconstruct.
    pub fn schedules(&self) -> Result<Vec<Schedule>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, home, every_ms, offset_ms, spec_json, note, created_at_ms, removed_at_ms
             FROM schedules ORDER BY created_at_ms, id",
        )?;
        let rows = stmt.query_map([], decode_schedule)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    /// Mark a schedule removed, here and — once it is gossiped — everywhere.
    ///
    /// Idempotent, and [`Removal`] is which of the three ways: a second removal keeps the first
    /// instant, because `Schedule::merge` prefers the earlier tombstone and the fleet has to
    /// agree on which one it is.
    pub fn remove_schedule(&self, id: ScheduleId, now_ms: i64) -> Result<Removal, StoreError> {
        let Some(existing) = self.schedule(id)? else {
            return Ok(Removal::Missing);
        };
        // Told apart from a removal this call made, rather than folded into it. Removing a
        // tombstone is a no-op here and the right outcome for whoever typed it — but it is not
        // the thing `offload unschedule` said it was while both answered `true`. A removal
        // command may be idempotent; it may not claim an act it did not perform.
        if let Some(at) = existing.removed_at {
            return Ok(Removal::Already(at));
        }
        self.merge_schedule(&Schedule {
            removed_at: Some(Millis(now_ms.unsigned_abs())),
            ..existing
        })?;
        Ok(Removal::Done)
    }

    /// Resolve a full or abbreviated schedule id.
    ///
    /// A prefix rather than a pattern, for `resolve_rule`'s reason stated there at length: `LIKE`
    /// reads `%` and `_` as wildcards, and a single match here *resolves* — which is how a
    /// wildcard becomes somebody's standing instruction being removed.
    pub fn resolve_schedule(&self, needle: &str) -> Result<ScheduleId, StoreError> {
        let needle = needle.trim().to_ascii_lowercase();
        if needle.is_empty() || !needle.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StoreError::NoSuchSchedule(format!(
                "`{needle}` (a schedule id is hex — that is not an abbreviation of one)"
            )));
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM schedules WHERE lower(hex(id)) LIKE ?1 || '%'
             ORDER BY created_at_ms LIMIT 2",
        )?;
        let matches: Vec<Vec<u8>> = stmt
            .query_map([&needle], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();
        match matches.len() {
            0 => Err(StoreError::NoSuchSchedule(format!("`{needle}`"))),
            1 => {
                let bytes: [u8; 8] = matches[0]
                    .clone()
                    .try_into()
                    .map_err(|_| StoreError::NoSuchSchedule(format!("`{needle}`")))?;
                Ok(ScheduleId::from_bytes(bytes))
            }
            _ => Err(StoreError::NoSuchSchedule(format!(
                "`{needle}` (ambiguous — use more characters)"
            ))),
        }
    }
}

type Decoded = Result<Schedule, StoreError>;

/// One row, or why it could not be read.
///
/// The nested `Result` is the same arrangement `decode_run` uses and for the same reason: a
/// `rusqlite` row mapper may only fail with a `rusqlite::Error`, and "the JSON in this column is
/// from a newer build" is not one. Surfacing it as the row's value keeps the difference between
/// *the database is broken* and *this row is from the future*.
fn decode_schedule(row: &rusqlite::Row<'_>) -> rusqlite::Result<Decoded> {
    let id: Vec<u8> = row.get(0)?;
    let home: Vec<u8> = row.get(1)?;
    let every: i64 = row.get(2)?;
    let offset: i64 = row.get(3)?;
    let spec_json: String = row.get(4)?;
    let note: String = row.get(5)?;
    let created: i64 = row.get(6)?;
    let removed: Option<i64> = row.get(7)?;

    Ok(decode(
        id, home, every, offset, &spec_json, note, created, removed,
    ))
}

#[allow(clippy::too_many_arguments)]
fn decode(
    id: Vec<u8>,
    home: Vec<u8>,
    every: i64,
    offset: i64,
    spec_json: &str,
    note: String,
    created: i64,
    removed: Option<i64>,
) -> Decoded {
    let id: [u8; 8] = id.try_into().map_err(|_| StoreError::Decode {
        what: "schedule id".into(),
        reason: "not 8 bytes".into(),
    })?;
    let home: [u8; 32] = home.try_into().map_err(|_| StoreError::Decode {
        what: "schedule home".into(),
        reason: "not 32 bytes".into(),
    })?;
    let spec = serde_json::from_str(spec_json).map_err(|e| StoreError::Decode {
        what: "schedule spec".into(),
        reason: e.to_string(),
    })?;
    Ok(Schedule {
        id: ScheduleId::from_bytes(id),
        home: offload_core::NodeId::from_bytes(home),
        every: Millis(every.unsigned_abs()),
        offset: Millis(offset.unsigned_abs()),
        spec,
        note,
        created_at: Millis(created.unsigned_abs()),
        removed_at: removed.map(|at| Millis(at.unsigned_abs())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{Constraint, Demand, NodeId, Restartability, RunSpec, TaskWork, Work};

    fn a_schedule(id: u8, home: u8) -> Schedule {
        Schedule {
            id: ScheduleId::from_bytes([id; 8]),
            home: NodeId::from_bytes([home; 32]),
            every: Millis::from_mins(15),
            offset: Millis::ZERO,
            spec: RunSpec {
                work: Work::Task(TaskWork {
                    service: offload_core::Service::Other("watch-api".into()),
                    args: vec!["--once".into()],
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
                prefer: offload_core::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            note: "watch the API".into(),
            created_at: Millis(1_000),
            removed_at: None,
        }
    }

    #[test]
    fn a_schedule_survives_the_round_trip_with_its_spec() {
        let store = Store::open_memory().expect("store");
        let sched = a_schedule(1, 9);
        store.merge_schedule(&sched).expect("write");

        let back = store.schedule(sched.id).expect("read").expect("present");
        assert_eq!(
            back, sched,
            "including the whole spec, which is the payload"
        );
        assert_eq!(store.schedules().expect("list"), vec![sched]);
    }

    #[test]
    fn a_peer_relaying_an_older_copy_cannot_rewrite_a_definition_or_undo_a_removal() {
        // The two halves of ADR-0056 §5. A schedule's fields are immutable, so the only thing a
        // second copy can legitimately add is the tombstone — and it must not be able to take
        // one away, or a removed schedule fires for ever on whichever node was last to hear.
        let store = Store::open_memory().expect("store");
        let sched = a_schedule(2, 9);
        store.merge_schedule(&sched).expect("write");
        store.remove_schedule(sched.id, 5_000).expect("remove");

        let settled = store.merge_schedule(&sched).expect("relay the live copy");
        assert!(
            settled.is_removed(),
            "a peer that has not heard about the removal must not undo it"
        );
        assert_eq!(
            store
                .schedule(sched.id)
                .expect("read")
                .expect("present")
                .removed_at,
            Some(Millis(5_000)),
            "and the row keeps the instant the fleet agreed on"
        );

        // A definition arriving with different content does not overwrite the stored one.
        let edited = Schedule {
            every: Millis::from_mins(1),
            note: "something else".into(),
            ..a_schedule(2, 9)
        };
        store.merge_schedule(&edited).expect("relay");
        let row = store.schedule(sched.id).expect("read").expect("present");
        assert_eq!(row.every, Millis::from_mins(15));
        assert_eq!(row.note, "watch the API");
    }

    #[test]
    fn removing_twice_keeps_the_first_instant_and_removing_nothing_says_so() {
        let store = Store::open_memory().expect("store");
        let sched = a_schedule(3, 9);
        store.merge_schedule(&sched).expect("write");

        assert_eq!(
            store.remove_schedule(sched.id, 100).expect("remove"),
            Removal::Done
        );
        // A second removal is not an error — the schedule is gone, which is what was asked —
        // and it is not a removal either. Both were `true`, so `offload unschedule` printed
        // "removed <id>" and the whole paragraph about the tombstone travelling, on a call that
        // wrote nothing. The instant it names is the *first* one, which is the one that has to
        // be told, because that is the one the fleet agrees on.
        assert_eq!(
            store.remove_schedule(sched.id, 900).expect("again"),
            Removal::Already(Millis(100)),
            "idempotent, and it may not claim an act it did not perform"
        );
        assert_eq!(
            store
                .schedule(sched.id)
                .expect("read")
                .expect("present")
                .removed_at,
            Some(Millis(100)),
            "the fleet has to agree on which instant, so the first one stands"
        );
        assert_eq!(
            store
                .remove_schedule(ScheduleId::from_bytes([9; 8]), 100)
                .expect("absent"),
            Removal::Missing
        );
    }

    #[test]
    fn a_tombstoned_schedule_is_still_listed_because_that_is_what_travels() {
        let store = Store::open_memory().expect("store");
        let live = a_schedule(4, 9);
        let doomed = a_schedule(5, 9);
        store.merge_schedule(&live).expect("write");
        store.merge_schedule(&doomed).expect("write");
        store.remove_schedule(doomed.id, 7_000).expect("remove");

        let all = store.schedules().expect("list");
        assert_eq!(all.len(), 2, "the tombstone is a row and it gossips");
        assert_eq!(all.iter().filter(|s| !s.is_removed()).count(), 1);
    }

    #[test]
    fn the_tick_mark_is_this_nodes_own_and_a_merge_cannot_erase_it() {
        // Why it exists: a scheduled occurrence is machine-started, so the retention pass
        // reclaims its record about an hour after it finishes — which for a daily schedule is
        // most of a day before its tick ends. With the record as the only evidence that
        // schedule fires hourly. Measured as arithmetic rather than on a daemon, because the
        // pass that does it is on a fifteen-minute timer.
        let store = Store::open_memory().expect("store");
        let sched = a_schedule(6, 9);
        store.merge_schedule(&sched).expect("write");
        assert_eq!(store.schedule_last_tick(sched.id).expect("read"), None);

        store
            .note_schedule_fired(sched.id, Millis(900_000))
            .expect("note");
        assert_eq!(
            store.schedule_last_tick(sched.id).expect("read"),
            Some(Millis(900_000))
        );

        // A high-water mark: a clock that steps backwards must not re-open a served tick.
        store
            .note_schedule_fired(sched.id, Millis(60_000))
            .expect("note");
        assert_eq!(
            store.schedule_last_tick(sched.id).expect("read"),
            Some(Millis(900_000))
        );

        // …and a peer relaying the schedule writes the row without touching it, which is the
        // whole reason the column is outside the conflict list.
        store.merge_schedule(&sched).expect("relay");
        assert_eq!(
            store.schedule_last_tick(sched.id).expect("read"),
            Some(Millis(900_000)),
            "a gossip merge must not erase what this node remembers doing"
        );
    }

    #[test]
    fn an_id_resolves_by_prefix_and_refuses_what_is_not_one() {
        let store = Store::open_memory().expect("store");
        let sched = a_schedule(0xab, 9);
        store.merge_schedule(&sched).expect("write");

        assert_eq!(store.resolve_schedule("abab").expect("prefix"), sched.id);
        assert_eq!(
            store
                .resolve_schedule(&sched.id.to_string())
                .expect("whole"),
            sched.id
        );
        assert!(store.resolve_schedule("%").is_err(), "not a wildcard");
        assert!(store.resolve_schedule("").is_err());
        assert!(store.resolve_schedule("ffff").is_err());
    }
}
