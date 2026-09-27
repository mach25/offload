//! The run registry and the event log.
//!
//! Runs are stored as their serialised domain type plus a handful of denormalised columns.
//! The JSON is authoritative; the columns exist so that "active runs, newest first" is an
//! index scan rather than a deserialise-everything-and-filter. Keeping the projection
//! honest is [`Store::save_run`]'s job — nothing else writes those columns.
//!
//! Events are generic over their payload on purpose. The store has no business knowing
//! what a run event *means*, and making it know would mean `offload-store` depending on
//! `offload-node`, which is backwards.

use crate::{now_ms, Store, StoreError};
use offload_core::{Run, RunId};
use rusqlite::OptionalExtension;
use serde::{de::DeserializeOwned, Serialize};

/// What a run has done and cost, as stored here.
///
/// [`offload_core::RunProgress`] itself, rather than a table-shaped copy of it. It was local
/// bookkeeping when a run only ever existed on one machine; now that the numbers are gossiped
/// — with an owner and a merge rule, like every other travelling fact — the type that defines
/// them belongs where the rule lives, and a second definition here would be one edit away from
/// disagreeing with it.
///
/// Still separate from `offload_core::Run` for the original reason: the domain object
/// describes what a run *is* — spec, state, epoch, lease — and none of that should grow a
/// `cost_micro_usd` field.
pub use offload_core::RunProgress as RunStats;

impl Store {
    /// Insert or update a run.
    ///
    /// A **whole-row** write: whatever the row held is replaced by the `Run` handed in. That is
    /// right for a caller that read the row a line earlier and decided something about it, and
    /// wrong for one writing a copy that has been sitting — the fresher fields go with no error
    /// and nothing to notice. Anything holding a run across an `await`, a listing, or a gossip
    /// tick wants [`Store::update_run`] instead.
    pub fn save_run(&self, run: &Run) -> Result<(), StoreError> {
        let conn = self.conn();
        write_run(&conn, run)
    }

    /// Read a run, change it, and write it back, all under one lock.
    ///
    /// The read-modify-write that [`Store::save_run`] cannot express. A caller that loads a run,
    /// changes one field and saves it is not writing that field — it is writing every field, as
    /// they stood when it read them, and anything another task wrote in between is gone. The
    /// window does not need an `await` in it to be real: two tokio tasks on one store are enough,
    /// and the losers here are a checkpoint (its blobs then collected, since nothing references
    /// them any more) and a terminal transition (a run left `Running` with no agent behind it,
    /// its lease renewed by the very loop that erased it).
    ///
    /// `None` means there was no such run, which a caller can tell from a change that did
    /// nothing.
    pub fn update_run<T>(
        &self,
        id: RunId,
        change: impl FnOnce(&mut Run) -> T,
    ) -> Result<Option<T>, StoreError> {
        let conn = self.conn();
        let Some(mut run) = read_run(&conn, id)? else {
            return Ok(None);
        };
        let out = change(&mut run);
        write_run(&conn, &run)?;
        Ok(Some(out))
    }

    /// Read a run that may not be here yet, decide what to write, and write it — one lock.
    ///
    /// [`Store::update_run`] with the one case it cannot express. `update_run` reads first and
    /// gives up when there is no row, which is right for every transition of a run that already
    /// exists and useless for the transition that *creates* one: a submission builds a run and
    /// starts it in the same breath, so the row's first existence is the start's own write.
    ///
    /// `decide` is handed whatever is stored, `None` when nothing is, and returns the row to
    /// write. Returning an error writes nothing — which is the point of handing it the stored
    /// copy rather than taking a closure over the caller's: the caller's copy is as old as
    /// whatever it did on the way here, and a whole-row [`Store::save_run`] of it does not
    /// overrule what landed in between, it erases it.
    pub fn upsert_run<T, E>(
        &self,
        id: RunId,
        decide: impl FnOnce(Option<Run>) -> Result<(Run, T), E>,
    ) -> Result<Result<(Run, T), E>, StoreError> {
        let conn = self.conn();
        let stored = read_run(&conn, id)?;
        match decide(stored) {
            Ok((run, out)) => {
                write_run(&conn, &run)?;
                Ok(Ok((run, out)))
            }
            Err(e) => Ok(Err(e)),
        }
    }

    pub fn load_run(&self, id: RunId) -> Result<Option<Run>, StoreError> {
        let conn = self.conn();
        read_run(&conn, id)
    }

    /// Runs, newest first. `include_terminal` decides whether finished runs come too.
    pub fn list_runs(&self, include_terminal: bool) -> Result<Vec<Run>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, run_json FROM runs
             WHERE (?1 = 1 OR terminal = 0)
             ORDER BY created_at_ms DESC",
        )?;
        let rows = stmt.query_map([i64::from(include_terminal)], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut runs = Vec::new();
        for row in rows {
            let (id, json) = row?;
            // A row we cannot decode is a schema-drift bug, not a reason to fail the whole
            // listing — `ps` showing nine runs beats `ps` showing an error.
            match decode_run(&json, &hex(&id)) {
                Ok(run) => runs.push(run),
                Err(e) => tracing::error!(error = %e, "skipping undecodable run row"),
            }
        }
        Ok(runs)
    }

    /// Resolve a full or abbreviated run id.
    ///
    /// Prefix matching happens on the hex form because that is what the user sees and
    /// copies. Ambiguity is an error rather than a guess: picking one of two runs the
    /// user might have meant is the kind of helpfulness that cancels the wrong job.
    pub fn resolve_run(&self, needle: &str) -> Result<RunId, StoreError> {
        // A prefix, not a pattern. `LIKE` reads `%` and `_` as wildcards, so `ab_d` matched any
        // character in that position and `%` matched every run in the store — and with one match
        // this function *resolves* rather than refusing, which is the "helpfulness that cancels
        // the wrong job" the paragraph above is about. A run id is hex, so anything else is not
        // an abbreviation of one and saying so is more use than a wildcard nobody asked for.
        //
        // The rule is `offload_core::needle` rather than these four lines, because there was a
        // second copy of the prefix match without it — `server::find_run`'s fallback into the
        // cluster view, reached precisely when this function says no. See [`offload_core::Needle`].
        let needle = match offload_core::needle(needle) {
            offload_core::Needle::Prefix(needle) => needle,
            offload_core::Needle::Empty => return Err(StoreError::NoRunGiven),
            offload_core::Needle::NotHex => {
                return Err(StoreError::NoSuchRun(format!(
                    "`{}` (a run id is hex — that is not an abbreviation of one)",
                    needle.trim().to_ascii_lowercase()
                )))
            }
        };

        let conn = self.conn();
        // Three rather than two, because the refusal below *names* what matched and "and
        // others" is worth being able to say truthfully.
        let mut stmt = conn.prepare(
            "SELECT lower(hex(id)) FROM runs WHERE lower(hex(id)) LIKE ?1 || '%'
             ORDER BY created_at_ms LIMIT 3",
        )?;
        let matches: Vec<String> = stmt
            .query_map([&needle], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();

        match matches.len() {
            0 => Err(StoreError::NoSuchRun(format!("`{needle}`"))),
            1 => RunId::parse_hex(&matches[0])
                .map_err(|_| StoreError::NoSuchRun(format!("`{needle}`"))),
            // The candidates, in full, because *"use more characters"* on its own is advice
            // somebody may have no way to take: a scheduled occurrence's displayed id is the
            // tick (`Schedule::occurrence_id`), so two schedules sharing a boundary share all
            // twelve characters anything prints, and there is no thirteenth to be had by looking
            // harder at the screen. Naming them turns a dead end into a line to copy.
            _ => {
                let named = matches
                    .iter()
                    .take(2)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(" or ");
                let more = if matches.len() > 2 { ", or others" } else { "" };
                Err(StoreError::NoSuchRun(format!(
                    "`{needle}` (ambiguous — {named}{more})"
                )))
            }
        }
    }

    /// Update the node-local facts about a run, leaving the domain object alone.
    /// Write a run's numbers **verbatim**, including their timestamp.
    ///
    /// `stats.at` is not restamped here even though every other timestamp in this store is
    /// taken from the local clock: these records travel, and the whole point of that field is
    /// that it belongs to whoever produced the numbers. Restamping on arrival would let a node
    /// relaying a stale copy make it look newer than the record it overwrites — which is the
    /// one failure the tiebreak exists to prevent. Local writers stamp it themselves.
    pub fn save_stats(&self, id: RunId, stats: &RunStats) -> Result<(), StoreError> {
        let conn = self.conn();
        write_stats(&conn, id, stats)
    }

    /// Read a run's numbers, change them, and write them back, all under one lock.
    ///
    /// [`Store::update_run`]'s sibling and for its reason, which arrived here later: these numbers
    /// used to be *replaced* — a gossiped record either won outright or was ignored — so a
    /// load-decide-save was writing a decision rather than a field. They are **folded** now
    /// (`RunProgress::absorb`: the winning leg's position beside the fleet's high-water spend), so
    /// the result depends on what the row holds at the moment of the write, and two tasks reading
    /// the same row before either writes lose one of the two contributions with no error
    /// anywhere. A turn boundary and an arriving gossip tick are exactly those two tasks.
    pub fn update_stats<T>(
        &self,
        id: RunId,
        change: impl FnOnce(&mut RunStats) -> T,
    ) -> Result<T, StoreError> {
        let conn = self.conn();
        let mut stats = read_stats(&conn, id)?;
        let out = change(&mut stats);
        write_stats(&conn, id, &stats)?;
        Ok(out)
    }

    pub fn load_stats(&self, id: RunId) -> Result<RunStats, StoreError> {
        let conn = self.conn();
        read_stats(&conn, id)
    }

    pub fn list_with_stats(
        &self,
        include_terminal: bool,
    ) -> Result<Vec<(Run, RunStats)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, run_json, {STATS_COLUMNS}
             FROM runs
             WHERE (?1 = 1 OR terminal = 0)
             ORDER BY created_at_ms DESC"
        ))?;
        let mut rows = stmt.query([i64::from(include_terminal)])?;

        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let id: Vec<u8> = row.get(0)?;
            let json: String = row.get(1)?;
            // The two leading columns this reader selects, and then the shared list.
            let stats = decode_stats(row, 2)?;
            match decode_run(&json, &hex(&id)) {
                Ok(run) => out.push((run, stats)),
                Err(e) => tracing::error!(error = %e, "skipping undecodable run row"),
            }
        }
        Ok(out)
    }

    /// Forget a run entirely.
    ///
    /// Worth knowing what this takes with it: the foreign keys cascade to the run's **events**
    /// and to its **outbox rows**, so a notification this node had noticed and not yet delivered
    /// disappears with the record. The delivery plane's promise is at-least-once, and an outbox
    /// row is the whole of what that promise is made of.
    ///
    /// The one production caller is ADR-0021's prune of a rule's spent occurrences, and it earns
    /// the right by answering that question rather than inheriting it:
    /// [`Store::spent_occurrences`] is the whole of the decision, and its four clauses are why
    /// there is nothing left to cascade away by the time this is reached. Anything *else* that
    /// starts calling this — a retention policy, an `offload rm --purge` — has to answer it
    /// again for its own path, because none of those clauses is checked here.
    pub fn delete_run(&self, id: RunId) -> Result<(), StoreError> {
        // Events and outbox rows cascade via the foreign key.
        self.conn()
            .execute("DELETE FROM runs WHERE id = ?1", [id.as_bytes().as_slice()])?;
        Ok(())
    }

    // -- events -----------------------------------------------------------------

    /// Append an event and return its sequence number.
    ///
    /// `kind` is denormalised for filtering; `event` is the whole thing.
    pub fn append_event<E: Serialize>(
        &self,
        run: RunId,
        kind: &str,
        at_unix_ms: u64,
        event: &E,
    ) -> Result<i64, StoreError> {
        let json = serde_json::to_string(event).map_err(|e| StoreError::Encode(e.to_string()))?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO run_events (run_id, at_unix_ms, kind, event_json)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                run.as_bytes().as_slice(),
                i64::try_from(at_unix_ms).unwrap_or(i64::MAX),
                kind,
                json
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Events for a run after sequence `after`, oldest first.
    ///
    /// The sequence number is what makes `logs --follow` resumable across a reconnect:
    /// the client remembers the last one it saw and asks for everything since, instead of
    /// re-reading the log and de-duplicating.
    pub fn events_since<E: DeserializeOwned>(
        &self,
        run: RunId,
        after: i64,
    ) -> Result<Vec<(i64, E)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, event_json FROM run_events
             WHERE run_id = ?1 AND seq > ?2
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(rusqlite::params![run.as_bytes().as_slice(), after], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let (seq, json) = row?;
            match serde_json::from_str(&json) {
                Ok(event) => events.push((seq, event)),
                Err(e) => tracing::error!(seq, error = %e, "skipping undecodable event"),
            }
        }
        Ok(events)
    }

    /// Has this run ever logged an event of this kind?
    ///
    /// Answered from the `kind` column rather than by decoding the log, which is what that column
    /// was denormalised for: the question is asked by things that say something *once* per run,
    /// and scanning a night's worth of agent output to find out whether a sentence has already
    /// been said would be a strange way to keep quiet.
    pub fn has_event_kind(&self, run: RunId, kind: &str) -> Result<bool, StoreError> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM run_events WHERE run_id = ?1 AND kind = ?2 LIMIT 1",
                rusqlite::params![run.as_bytes().as_slice(), kind],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn event_count(&self, run: RunId) -> Result<u64, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM run_events WHERE run_id = ?1",
            [run.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// How many run records this node is holding, and how many of them are somebody else's
    /// (ADR-0025).
    ///
    /// The number that had nowhere to be printed. `offload rules` prints what a *rule* is keeping
    /// and `offload sinks` prints the queue that explains a silence — and a node with neither a
    /// rule nor a route accumulates records with nothing on it saying so, which is exactly the
    /// machine this matters on, because it is the one hosting nothing.
    ///
    /// **Finished only.** A live foreign run is the fleet working, and counting it here would put
    /// the fleet's ordinary traffic in a line about residue.
    ///
    /// *Learned* means this node neither submitted the run (`home_node`) nor ran the leg that last
    /// wrote its position (`progress_by`, the durable form of "which leg ran this" — ADR-0023's
    /// rule, and unstamped is *unknown*, which here means nobody ran it rather than somebody
    /// else did). Those are the records nothing on this node will ever remove: the tag a prune
    /// keys on is node-local, and ADR-0024's travelling bit only reaches the *completed* ones.
    pub fn records_kept(&self, local: offload_core::NodeId) -> Result<RecordsKept, StoreError> {
        let me = local.as_bytes().to_vec();
        let conn = self.conn();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))?;
        // One query for both learned numbers, because two would be two scans answering halves of
        // one sentence — and the failed count is the half worth having: it is the only supply of
        // these that is unbounded (ADR-0021 §2 keeps every failure, and nothing here can tell a
        // rule's failures from anybody's).
        let (learned, learned_failed): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(state = 'failed'), 0) FROM runs
              WHERE terminal = 1
                AND home_node != ?1
                AND (progress_by IS NULL OR progress_by != ?1)",
            [&me],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(RecordsKept {
            total: total.unsigned_abs(),
            learned: learned.unsigned_abs(),
            learned_failed: learned_failed.unsigned_abs(),
        })
    }
}

/// What [`Store::records_kept`] found: the store's size, and how much of it is other machines'.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordsKept {
    pub total: u64,
    /// Finished runs this node neither submitted nor ran.
    pub learned: u64,
    /// …and how many of those *failed*, which is the only unbounded supply of them.
    pub learned_failed: u64,
}

fn write_stats(conn: &rusqlite::Connection, id: RunId, stats: &RunStats) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE runs SET turns = ?2, denials = ?3, cost_micro_usd = ?4, workspace = ?5,
                         progress_at_ms = ?6, asks = ?7, progress_by = ?8,
                         progress_epoch = ?9, updated_at_ms = ?10,
                         tok_input = ?11, tok_output = ?12, tok_cache_creation = ?13,
                         tok_cache_read = ?14, tok_messages = ?15, legs_json = ?16
         WHERE id = ?1",
        rusqlite::params![
            id.as_bytes().as_slice(),
            i64::from(stats.turns),
            i64::from(stats.denials),
            i64::try_from(stats.cost_micro_usd).unwrap_or(i64::MAX),
            stats.workspace,
            i64::try_from(stats.at.0).unwrap_or(i64::MAX),
            i64::from(stats.asks),
            stats.by.map(|node| node.as_bytes().to_vec()),
            i64::try_from(stats.epoch.0).unwrap_or(i64::MAX),
            now_ms(),
            i64::try_from(stats.tokens.input).unwrap_or(i64::MAX),
            i64::try_from(stats.tokens.output).unwrap_or(i64::MAX),
            i64::try_from(stats.tokens.cache_creation).unwrap_or(i64::MAX),
            i64::try_from(stats.tokens.cache_read).unwrap_or(i64::MAX),
            i64::try_from(stats.tokens.messages).unwrap_or(i64::MAX),
            serde_json::to_string(&stats.legs).unwrap_or_else(|_| "[]".to_string()),
        ],
    )?;
    Ok(())
}

/// A run's numbers, or the default for a run this store has never seen.
///
/// Deliberately not an `Option`: a row with no numbers in it and no row at all are the same
/// answer to "what has this run done", and every caller wants zeros rather than a branch.
fn read_stats(conn: &rusqlite::Connection, id: RunId) -> Result<RunStats, StoreError> {
    let mut stmt = conn.prepare(&format!("SELECT {STATS_COLUMNS} FROM runs WHERE id = ?1"))?;
    let mut rows = stmt.query([id.as_bytes().as_slice()])?;
    let Some(row) = rows.next()? else {
        return Ok(RunStats::default());
    };
    Ok(decode_stats(row, 0)?)
}

/// The stats columns, in the order [`decode_stats`] expects them.
///
/// A constant because there are two readers with different leading columns, and they used to
/// carry two copies of this list with two sets of hand-written indices. Adding v11's token
/// columns to one and not the other would have compiled and then reported zeros from whichever
/// reader was missed — `offload ps` and `offload explain` disagreeing about the same run, with
/// nothing to notice.
pub(crate) const STATS_COLUMNS: &str = concat!(
    "turns, denials, cost_micro_usd, workspace, progress_at_ms, asks, progress_by, ",
    "progress_epoch, tok_input, tok_output, tok_cache_creation, tok_cache_read, tok_messages, ",
    "legs_json"
);

/// Decode the [`STATS_COLUMNS`] beginning at `base`, which is however many columns the caller
/// selected in front of them.
fn decode_stats(row: &rusqlite::Row<'_>, base: usize) -> rusqlite::Result<RunStats> {
    Ok(RunStats {
        turns: row.get::<_, i64>(base)?.try_into().unwrap_or(u32::MAX),
        denials: row.get::<_, i64>(base + 1)?.try_into().unwrap_or(u32::MAX),
        cost_micro_usd: row.get::<_, i64>(base + 2)?.try_into().unwrap_or(u64::MAX),
        workspace: row.get(base + 3)?,
        at: offload_core::Millis(row.get::<_, i64>(base + 4)?.try_into().unwrap_or(u64::MAX)),
        asks: row.get::<_, i64>(base + 5)?.try_into().unwrap_or(u32::MAX),
        by: decode_progress_by(row.get::<_, Option<Vec<u8>>>(base + 6)?),
        epoch: offload_core::run::Epoch(
            row.get::<_, i64>(base + 7)?.try_into().unwrap_or(u64::MAX),
        ),
        tokens: offload_core::TokenUse {
            input: row.get::<_, i64>(base + 8)?.try_into().unwrap_or(u64::MAX),
            output: row.get::<_, i64>(base + 9)?.try_into().unwrap_or(u64::MAX),
            cache_creation: row.get::<_, i64>(base + 10)?.try_into().unwrap_or(u64::MAX),
            cache_read: row.get::<_, i64>(base + 11)?.try_into().unwrap_or(u64::MAX),
            messages: row.get::<_, i64>(base + 12)?.try_into().unwrap_or(u64::MAX),
        },
        // A list this build cannot read is no legs rather than an error, for `decode_progress_by`'s
        // reason: refusing the run's turn count over it would be worse than not counting a fork.
        legs: serde_json::from_str(&row.get::<_, String>(base + 13)?).unwrap_or_default(),
    })
}

/// The leg that wrote a run's numbers, or `None` for a row from before v7 recorded one.
///
/// A stored id of the wrong length is `None` rather than an error: it can only mean a row this
/// build cannot attribute, which is what an unstamped row already means, and refusing to read the
/// run's turn count over it would be the collector's mistake in the other direction.
fn decode_progress_by(bytes: Option<Vec<u8>>) -> Option<offload_core::NodeId> {
    let bytes = bytes?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .ok()
        .map(offload_core::NodeId::from_bytes)
}

/// Every blob any run in this store still needs, asked of the runs rather than of their text.
///
/// Exists for [`crate::blobs::collect_garbage`], and it is a `Result` rather than a best-effort
/// set on purpose: a run whose JSON will not decode is a run whose references are *unknown*, and
/// the difference between unknown and none is a resumable run turned into a lost one. So an
/// undecodable row fails the whole question, and the caller declines to collect anything.
///
/// The set comes from `Checkpoint::blobs`, which is the same accessor a receiving node uses to
/// work out what it has to fetch before it can materialise a checkpoint. One definition, so a
/// field that adds a blob reference cannot be added to the checkpoint and forgotten here.
pub(crate) fn referenced_blobs(
    conn: &rusqlite::Connection,
) -> Result<std::collections::BTreeSet<offload_core::BlobHash>, StoreError> {
    let mut stmt = conn.prepare("SELECT id, run_json FROM runs")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut referenced = std::collections::BTreeSet::new();
    for row in rows {
        let (id, json) = row?;
        let run = decode_run(&json, &hex(&id))?;
        // `resumable_checkpoint`, not the field. A completed or cancelled run's checkpoint is a
        // note about the last capture rather than something anything can start from — `resume`
        // refuses both states by name — so its blobs are referenced by a record and reachable by
        // nothing. Measured before this line existed: one blob, 472 KB, kept for ever per
        // completed run, on the home node, on the leg that ran it, and on every peer that took a
        // replica. ADR-0022 collected the *superseded* checkpoints and left the last one, which
        // is the biggest of them.
        if let Some(checkpoint) = run.resumable_checkpoint() {
            referenced.extend(checkpoint.blobs());
        }
        // A continuation's parent transcript, for as long as the continuation exists and in any
        // state (ADR-0064 §3): it is a file in the run's worktree, and a finished continuation is
        // exactly what somebody continues next. Naming it is the whole of its retention.
        if let Some(transcript) = run.spec.parent.as_ref().and_then(|p| p.transcript) {
            referenced.insert(transcript);
        }
    }
    Ok(referenced)
}

/// The row write behind [`Store::save_run`] and [`Store::update_run`], for callers that
/// already hold the connection.
fn write_run(conn: &rusqlite::Connection, run: &Run) -> Result<(), StoreError> {
    let json = serde_json::to_string(run).map_err(|e| StoreError::Encode(e.to_string()))?;
    conn.execute(
        // `machine_started` is in the update list and `rule` is deliberately not: this one is a
        // projection of the record (ADR-0024) and has to arrive with a peer's copy, while that
        // one is this node's own bookkeeping and a merge writing the row back must leave it
        // alone. Two columns, opposite rules, one statement — see both migrations.
        "INSERT INTO runs (id, state, terminal, epoch, home_node, holder_node,
                           created_at_ms, updated_at_ms, run_json, machine_started)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
             state           = excluded.state,
             terminal        = excluded.terminal,
             epoch           = excluded.epoch,
             holder_node     = excluded.holder_node,
             updated_at_ms   = excluded.updated_at_ms,
             run_json        = excluded.run_json,
             machine_started = excluded.machine_started",
        rusqlite::params![
            run.id.as_bytes().as_slice(),
            run.state.name(),
            i64::from(run.state.is_terminal()),
            i64::try_from(run.epoch.0).unwrap_or(i64::MAX),
            run.home.as_bytes().as_slice(),
            run.holder().map(|n| n.as_bytes().to_vec()),
            i64::try_from(run.created_at.0).unwrap_or(i64::MAX),
            now_ms(),
            json,
            i64::from(run.origin == offload_core::Origin::Rule),
        ],
    )?;
    Ok(())
}

/// The read behind [`Store::load_run`], for callers that already hold the connection.
fn read_run(conn: &rusqlite::Connection, id: RunId) -> Result<Option<Run>, StoreError> {
    let mut stmt = conn.prepare("SELECT run_json FROM runs WHERE id = ?1")?;
    let mut rows = stmt.query([id.as_bytes().as_slice()])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let json: String = row.get(0)?;
    decode_run(&json, &id.to_string()).map(Some)
}

fn decode_run(json: &str, what: &str) -> Result<Run, StoreError> {
    serde_json::from_str(json).map_err(|e| StoreError::Decode {
        what: what.to_string(),
        reason: e.to_string(),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{
        AgentKind, AgentWork, Constraint, Millis, NodeId, PermissionMode, Restartability, RunSpec,
        ToolAllowlist, Work, WorkspaceSpec,
    };
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestEvent {
        note: String,
    }

    fn run_with(id: u8) -> Run {
        let mut bytes = [0u8; 16];
        bytes[0] = id;
        Run::new(
            RunId::from_bytes(bytes),
            RunSpec {
                work: Work::Agent(AgentWork {
                    agent: AgentKind::ClaudeCode,
                    model: None,
                    prompt: "do the thing".into(),
                    workspace: WorkspaceSpec {
                        repo: "/repo".into(),
                        archive_bytes: None,
                        git_ref: None,
                        branch: None,
                    },
                    permission_mode: PermissionMode::AcceptEdits,
                    allow: ToolAllowlist::default(),
                    max_turns: None,
                    ask: offload_core::AskPolicy::Never,
                }),
                constraint: Constraint::Always,
                restartability: Restartability::Resumable,
                priority: 0,
                queue: false,
                deadline: None,
                demand: Default::default(),
                notify: Default::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: offload_core::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            NodeId::from_bytes([9; 32]),
            Millis(1_000),
        )
    }

    #[test]
    fn a_run_survives_a_restart_with_its_state_machine_intact() {
        // The gap phase 1 shipped with: restarting forgot every run.
        let store = Store::open_memory().expect("open");
        let mut run = run_with(1);
        let epoch = run
            .assign(NodeId::from_bytes([1; 32]), Millis(2_000), Millis(60_000))
            .expect("assign");
        run.started(NodeId::from_bytes([1; 32]), epoch, Millis(2_100))
            .expect("start");

        store.save_run(&run).expect("save");
        let loaded = store.load_run(run.id).expect("load").expect("present");

        assert_eq!(loaded, run, "the whole domain object round-trips");
        assert_eq!(
            loaded.epoch, epoch,
            "epoch survives — fencing depends on it"
        );
        assert_eq!(loaded.state.name(), "running");
    }

    #[test]
    fn saving_the_same_run_twice_updates_rather_than_duplicates() {
        let store = Store::open_memory().expect("open");
        let mut run = run_with(2);
        store.save_run(&run).expect("first");

        let epoch = run
            .assign(NodeId::from_bytes([1; 32]), Millis(2), Millis(60_000))
            .expect("assign");
        run.complete(NodeId::from_bytes([1; 32]), epoch, Millis(3))
            .expect("complete");
        store.save_run(&run).expect("second");

        assert_eq!(store.list_runs(true).expect("list").len(), 1);
        assert_eq!(
            store
                .load_run(run.id)
                .expect("load")
                .expect("present")
                .state
                .name(),
            "completed"
        );
    }

    #[test]
    fn the_denormalised_columns_track_the_json() {
        // They are a projection, and a stale projection means `ps` disagreeing with the
        // run's actual state.
        let store = Store::open_memory().expect("open");
        let mut run = run_with(3);
        store.save_run(&run).expect("save pending");

        let listed = store.list_runs(false).expect("list active");
        assert_eq!(listed.len(), 1, "pending run is active");

        let epoch = run
            .assign(NodeId::from_bytes([1; 32]), Millis(2), Millis(60_000))
            .expect("assign");
        run.complete(NodeId::from_bytes([1; 32]), epoch, Millis(3))
            .expect("complete");
        store.save_run(&run).expect("save terminal");

        assert!(
            store.list_runs(false).expect("list").is_empty(),
            "terminal run is filtered out by the projection, not by decoding every row"
        );
        assert_eq!(store.list_runs(true).expect("list all").len(), 1);
    }

    #[test]
    fn run_ids_resolve_by_prefix_and_ambiguity_is_refused() {
        // Picking one of two runs the user might have meant is how you cancel the wrong job.
        let store = Store::open_memory().expect("open");
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        a[..4].copy_from_slice(&[0x01, 0x9f, 0x9a, 0x2b]);
        b[..4].copy_from_slice(&[0x01, 0x9f, 0x9a, 0x2b]);
        a[15] = 0xaa;
        b[15] = 0xbb;

        for bytes in [a, b] {
            let mut run = run_with(0);
            run.id = RunId::from_bytes(bytes);
            store.save_run(&run).expect("save");
        }

        assert!(
            store.resolve_run("019f9a2b").is_err(),
            "the shared UUIDv7 timestamp prefix is ambiguous"
        );
        assert_eq!(
            store
                .resolve_run(&RunId::from_bytes(a).to_string())
                .expect("full id"),
            RunId::from_bytes(a)
        );
        assert!(store.resolve_run("ffff").is_err());
    }

    #[test]
    fn a_run_id_is_a_prefix_and_not_a_pattern() {
        // `LIKE` reads `%` and `_` as wildcards, and this function *resolves* on a single match
        // rather than refusing — so on a store with one run, `offload cancel %` cancelled it,
        // and `ab_d` reached a run whose id has no underscore in it. Found by sweeping every
        // place a hash or an id is compared as text, after the blob collector turned out to be
        // matching hex against JSON that spells its hashes as arrays of integers.
        let store = Store::open_memory().expect("open");
        let mut only = [0u8; 16];
        only[..4].copy_from_slice(&[0x01, 0x9f, 0x9a, 0x2b]);
        let mut run = run_with(0);
        run.id = RunId::from_bytes(only);
        store.save_run(&run).expect("save");

        // The prefix still works, which is the point of having it.
        assert_eq!(
            store.resolve_run("019f9a2b").expect("prefix"),
            RunId::from_bytes(only)
        );

        for pattern in ["%", "_", "019f9a2_", "019f9a2%", "0%b"] {
            let err = store
                .resolve_run(pattern)
                .expect_err(&format!("`{pattern}` resolved to a run nobody named"));
            assert!(
                err.to_string().contains("hex"),
                "the refusal should say what a run id is: {err}"
            );
        }
    }

    #[test]
    fn events_replay_in_order_and_resume_from_a_sequence_number() {
        // What makes `logs --follow` survive a reconnect without re-reading everything.
        let store = Store::open_memory().expect("open");
        let run = run_with(4);
        store.save_run(&run).expect("save");

        let mut seqs = Vec::new();
        for note in ["first", "second", "third"] {
            seqs.push(
                store
                    .append_event(
                        run.id,
                        "test",
                        1_000,
                        &TestEvent {
                            note: note.to_string(),
                        },
                    )
                    .expect("append"),
            );
        }
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "sequence increases");

        let all: Vec<(i64, TestEvent)> = store.events_since(run.id, 0).expect("all");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].1.note, "first");

        let tail: Vec<(i64, TestEvent)> = store.events_since(run.id, seqs[0]).expect("tail");
        assert_eq!(tail.len(), 2, "resumes after the given sequence");
        assert_eq!(tail[0].1.note, "second");
    }

    #[test]
    fn stats_are_preserved_when_the_domain_object_is_re_saved() {
        // save_run and save_stats write disjoint columns. If save_run clobbered the
        // stats, every state transition would reset the run's turn count and cost to
        // zero — visible in `ps` as a run that keeps starting over.
        let store = Store::open_memory().expect("open");
        let mut run = run_with(6);
        store.save_run(&run).expect("save");
        store
            .save_stats(
                run.id,
                &RunStats {
                    at: offload_core::Millis(7),
                    turns: 4,
                    denials: 1,
                    cost_micro_usd: 30_500,
                    workspace: "2 modified".into(),
                    // The leg that wrote them (schema v7). In this test for the reason the rest
                    // of the row is: these are two columns written by hand in one UPDATE and read
                    // by hand in two SELECTs, and a mistyped index is silent.
                    by: Some(NodeId::from_bytes([4; 32])),
                    epoch: offload_core::run::Epoch(9),
                    ..RunStats::default()
                },
            )
            .expect("stats");

        let epoch = run
            .assign(NodeId::from_bytes([1; 32]), Millis(2), Millis(60_000))
            .expect("assign");
        run.complete(NodeId::from_bytes([1; 32]), epoch, Millis(3))
            .expect("complete");
        store.save_run(&run).expect("re-save");

        let stats = store.load_stats(run.id).expect("load stats");
        assert_eq!(stats.turns, 4);
        assert_eq!(stats.cost_micro_usd, 30_500);
        assert_eq!(stats.workspace, "2 modified");
        assert_eq!(stats.by, Some(NodeId::from_bytes([4; 32])));
        assert_eq!(stats.epoch, offload_core::run::Epoch(9));
        assert_eq!(
            store
                .list_with_stats(true)
                .expect("list")
                .iter()
                .find(|(r, _)| r.id == run.id)
                .map(|(_, s)| (s.by, s.epoch)),
            Some((
                Some(NodeId::from_bytes([4; 32])),
                offload_core::run::Epoch(9)
            )),
            "and the listing reads the same columns as the single-row load"
        );
        assert_eq!(
            stats.at,
            offload_core::Millis(7),
            "the author's timestamp is stored, not the receiver's clock"
        );
    }

    #[test]
    fn a_learned_record_keeps_the_timestamp_it_arrived_with() {
        // Every other timestamp in this store comes from the local clock. This one must not:
        // it is how two records claiming the same turns and the same cost are ordered, and a
        // node that restamped somebody else's numbers on arrival could make a stale copy it is
        // relaying look newer than the record it overwrites.
        let store = Store::open_memory().expect("open");
        let run = run_with(9);
        store.save_run(&run).expect("save");

        let long_ago = offload_core::Millis(1_000);
        store
            .save_stats(
                run.id,
                &RunStats {
                    at: long_ago,
                    turns: 3,
                    ..RunStats::default()
                },
            )
            .expect("stats");

        assert_eq!(store.load_stats(run.id).expect("load").at, long_ago);
    }

    #[test]
    fn list_with_stats_pairs_each_run_with_its_own_numbers() {
        let store = Store::open_memory().expect("open");
        for (id, turns) in [(7u8, 2u32), (8, 5)] {
            let run = run_with(id);
            store.save_run(&run).expect("save");
            store
                .save_stats(
                    run.id,
                    &RunStats {
                        turns,
                        ..RunStats::default()
                    },
                )
                .expect("stats");
        }

        let listed = store.list_with_stats(true).expect("list");
        assert_eq!(listed.len(), 2);
        for (run, stats) in listed {
            let expected = if run.id.as_bytes()[0] == 7 { 2 } else { 5 };
            assert_eq!(stats.turns, expected, "stats followed the right run");
        }
    }

    /// The three relationships a node can have to a record, and only one of them is residue.
    ///
    /// What `offload status` prints, and it has to be right about the middle case: a run
    /// *submitted* elsewhere and **run here** is this node's own work, and counting it as
    /// somebody else's would make every busy host look like it was hoarding a peer's records.
    /// `progress_by` is the durable form of "which leg ran this" (ADR-0023), and unstamped is
    /// *unknown* — which here means nobody ran it, not that somebody else did.
    #[test]
    fn a_node_counts_only_the_records_it_neither_submitted_nor_ran() {
        let store = Store::open_memory().expect("open");
        let me = NodeId::from_bytes([1; 32]);
        let peer = NodeId::from_bytes([9; 32]);

        // Driven through the transitions, not written into a fixture: `complete` is fenced, so
        // a run that was never assigned silently stays `Pending` and every count below reads 0.
        let finish = |id: u8, holder: NodeId| {
            let mut run = run_with(id);
            let epoch = run
                .assign(holder, Millis(1_000), Millis(60_000))
                .expect("assign");
            run.started(holder, epoch, Millis(1_100)).expect("start");
            run
        };

        // Ours by submission: `run_with`'s home is the peer, so say so explicitly.
        let mut mine = finish(1, me);
        mine.home = me;
        mine.complete(me, mine.epoch, Millis(2_000)).expect("done");
        store.save_run(&mine).expect("save");

        // Submitted on the peer and run here — our work, learned or not.
        let mut ran_here = finish(2, me);
        ran_here
            .complete(me, ran_here.epoch, Millis(2_000))
            .expect("done");
        store.save_run(&ran_here).expect("save");
        store
            .save_stats(
                ran_here.id,
                &RunStats {
                    turns: 4,
                    by: Some(me),
                    ..RunStats::default()
                },
            )
            .expect("stats");

        // Somebody else's, end to end. The residue.
        let mut theirs = finish(3, peer);
        theirs
            .complete(peer, theirs.epoch, Millis(2_000))
            .expect("done");
        store.save_run(&theirs).expect("save");
        store
            .save_stats(
                theirs.id,
                &RunStats {
                    turns: 6,
                    by: Some(peer),
                    ..RunStats::default()
                },
            )
            .expect("stats");

        // …and one of those that failed, which is the only unbounded supply of them.
        let mut failed = finish(4, peer);
        failed
            .fail(peer, failed.epoch, "the agent died", Millis(2_000))
            .expect("failed");
        store.save_run(&failed).expect("save");
        store
            .save_stats(
                failed.id,
                &RunStats {
                    turns: 1,
                    by: Some(peer),
                    ..RunStats::default()
                },
            )
            .expect("stats");

        // A live run of the peer's is the fleet working, not residue.
        let running = run_with(5);
        store.save_run(&running).expect("save");

        let kept = store.records_kept(me).expect("count");
        assert_eq!(kept.total, 5);
        assert_eq!(kept.learned, 2, "the two finished runs we had no part in");
        assert_eq!(kept.learned_failed, 1);

        // Asked from the other machine, the same store says the mirror image — which is the
        // cheapest check that this reads the local node's own id rather than a fixed column.
        let from_peer = store.records_kept(peer).expect("count");
        assert_eq!(from_peer.learned, 1, "only the run alpha submitted and ran");
    }

    #[test]
    fn deleting_a_run_takes_its_events_with_it() {
        let store = Store::open_memory().expect("open");
        let run = run_with(5);
        store.save_run(&run).expect("save");
        store
            .append_event(run.id, "test", 0, &TestEvent { note: "x".into() })
            .expect("append");

        store.delete_run(run.id).expect("delete");
        assert!(store.load_run(run.id).expect("load").is_none());
        assert_eq!(
            store.event_count(run.id).expect("count"),
            0,
            "cascade left no orphaned events"
        );
    }
}
