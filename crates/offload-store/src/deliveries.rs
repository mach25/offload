//! The delivery plane's memory: what has been noticed, and what has actually been sent.
//!
//! ADR-0010 makes notifications a fan-out *from the event log* rather than a side effect of
//! anything, and this is the state that makes that work. Two questions, two tables, and the
//! reason to keep them apart is the failure they would otherwise share: a sink that is down
//! must not swallow the notifications behind it.
//!
//! * [`Store::sink_cursor`] / [`Store::advance_sink`] bound the **scan**. A cursor is the
//!   highest log sequence a sink has been shown. A sink seen for the first time starts at the
//!   *end* of the log, because configuring a route this evening is not a request to be told
//!   about every run that finished last month.
//! * [`Store::notice_delivery`] and its siblings are the **outbox**, keyed on `(sink, seq)` —
//!   ADR-0010's dedup identity. A row is written when the event is noticed and updated when it
//!   is sent, so retries come from the table and the cursor keeps moving.
//!
//! The primary key is the at-least-once mechanism, and it is worth being precise about what it
//! guarantees: *this* node never sends the same notification twice, because the insert that
//! loses the race is a no-op. Two nodes can still both deliver — that is the duplicate ADR-0010
//! accepts by name, in exchange for never being silent.

use crate::{Store, StoreError};
use offload_core::{FleetLogEvent, Millis, RunId, Topic};

/// What one sink has carried, and what it has dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SinkTotals {
    /// Arrived.
    pub delivered: u64,
    /// Noticed and still to send.
    pub waiting: u64,
    /// Retried to exhaustion and abandoned. Deliberately not folded into `delivered`: a route
    /// that has never worked reporting a healthy count is the one lie a delivery plane must not
    /// tell.
    pub gave_up: u64,
    pub last_error: Option<String>,
}

/// One thing that should be delivered and has not been yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub sink: String,
    /// Which log `seq` counts in. The two number independently, so a sequence without this is
    /// ambiguous — see the v5 migration.
    pub topic: Topic,
    pub seq: u64,
    /// The run this is about, or `None` for news about the fleet itself (ADR-0012).
    pub run: Option<RunId>,
    /// How many times it has been tried. Drives backoff and, past a point, giving up loudly
    /// rather than quietly.
    pub attempts: u32,
    pub last_error: Option<String>,
}

impl Store {
    /// Where a sink's scan has reached, initialising it to the end of the log on first sight.
    ///
    /// The initialisation is the interesting half. A brand-new sink with a cursor of zero would
    /// be handed every notable event this node has ever logged — a month of finished runs
    /// arriving at once, which is how somebody learns to turn notifications off. Starting at the
    /// current end means a sink reports what happens *after* it existed, which is what anybody
    /// configuring one expects.
    pub fn sink_cursor(&self, sink: &str, topic: Topic, now: Millis) -> Result<u64, StoreError> {
        let conn = self.conn();
        let existing: Option<i64> = conn
            .query_row(
                "SELECT seq FROM sink_cursors WHERE sink = ?1 AND topic = ?2",
                rusqlite::params![sink, topic.name()],
                |row| row.get(0),
            )
            .ok();
        if let Some(seq) = existing {
            return Ok(u64::try_from(seq).unwrap_or(0));
        }
        let end: i64 = conn
            .query_row(
                match topic {
                    Topic::Run => "SELECT COALESCE(MAX(seq), 0) FROM run_events",
                    Topic::Fleet => "SELECT COALESCE(MAX(seq), 0) FROM fleet_events",
                },
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        conn.execute(
            "INSERT INTO sink_cursors (sink, topic, seq, seen_at_ms) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                sink,
                topic.name(),
                end,
                i64::try_from(now.0).unwrap_or(i64::MAX)
            ],
        )?;
        tracing::info!(
            sink,
            %topic,
            from_seq = end,
            "new delivery sink; it will report what happens from now on"
        );
        Ok(u64::try_from(end).unwrap_or(0))
    }

    /// A sink's cursor if it has one, without creating it.
    ///
    /// The distinction matters: [`Store::sink_cursor`] *initialises* a missing cursor to the end
    /// of the log, which is right for a delivery pass and wrong for anything that merely looks.
    /// `offload sinks` reading its way into starting a route would be a status command with a
    /// side effect.
    pub fn existing_sink_cursor(
        &self,
        sink: &str,
        topic: Topic,
    ) -> Result<Option<u64>, StoreError> {
        let seq: Option<i64> = self
            .conn()
            .query_row(
                "SELECT seq FROM sink_cursors WHERE sink = ?1 AND topic = ?2",
                rusqlite::params![sink, topic.name()],
                |row| row.get(0),
            )
            .ok();
        Ok(seq.map(|s| u64::try_from(s).unwrap_or(0)))
    }

    /// Move a sink's scan forward. Unconditional: the outbox rows are what remember the work.
    pub fn advance_sink(&self, sink: &str, topic: Topic, seq: u64) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE sink_cursors SET seq = ?3 WHERE sink = ?1 AND topic = ?2 AND seq < ?3",
            rusqlite::params![sink, topic.name(), i64::try_from(seq).unwrap_or(i64::MAX)],
        )?;
        Ok(())
    }

    /// Log entries a sink has not been shown yet, oldest first, filtered to the kinds that can
    /// become a notification.
    ///
    /// The `kind` column exists for exactly this (`append_event` denormalises it), which is why
    /// this can be one indexed range scan rather than a decode of every event in the database.
    /// The caller supplies the kind list from `offload_core::NOTABLE_KINDS`, so the filter
    /// cannot drift from the projection that consumes it.
    pub fn events_to_notice(
        &self,
        after: u64,
        kinds: &[&str],
        limit: u32,
    ) -> Result<Vec<(u64, RunId, String, String)>, StoreError> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (0..kinds.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        // The `kind` comes back too, because the scan now decides *which* news a run wanted as
        // well as which routes (ADR-0026) — and it is the denormalised column rather than
        // anything decoded from the payload, which the scan deliberately never parses.
        let sql = format!(
            "SELECT seq, run_id, event_json, kind FROM run_events
             WHERE seq > ?1 AND kind IN ({placeholders})
             ORDER BY seq LIMIT {limit}"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(kinds.len() + 1);
        let after = i64::try_from(after).unwrap_or(i64::MAX);
        params.push(&after);
        for kind in kinds {
            params.push(kind);
        }
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (seq, id, json, kind) = row?;
            match run_id(&id) {
                Some(run) => out.push((u64::try_from(seq).unwrap_or(0), run, json, kind)),
                None => tracing::error!(seq, "skipping an event with an unreadable run id"),
            }
        }
        Ok(out)
    }

    /// Write down something that happened to the fleet (ADR-0012 mitigation 4).
    ///
    /// Its own log rather than a run's, because it is not about a run — and durable rather than
    /// a `tracing` line, because "an enrolment nobody performed" has to survive a daemon restart
    /// and be readable in the morning by somebody who was asleep when the push arrived.
    ///
    /// Written by the CLI as well as the daemon: `offload invite` and `offload join` happen with
    /// no daemon running, and an alarm that only fired when a daemon happened to be up would be
    /// missing on exactly the machine somebody has just walked up to.
    pub fn append_fleet_event(&self, event: &FleetLogEvent) -> Result<u64, StoreError> {
        let json = serde_json::to_string(event).map_err(|e| StoreError::Encode(e.to_string()))?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO fleet_events (at_unix_ms, kind, subject, event_json)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                i64::try_from(event.at_unix_ms).unwrap_or(i64::MAX),
                event.kind_name(),
                event.subject(),
                json
            ],
        )?;
        Ok(u64::try_from(conn.last_insert_rowid()).unwrap_or(0))
    }

    /// Write down a decision this node made about a run, or one it was refused
    /// (`offload_core::audit`).
    ///
    /// Its own log rather than the run's, because the entry most worth having is written by a node
    /// that has just stopped being the run's holder — and a run's log is served from the holder,
    /// so a line written there would be one nobody reads. Durable rather than a `tracing` line for
    /// `append_fleet_event`'s reason: "two agents nearly ran on one repository" has to survive a
    /// restart and be readable by somebody who was asleep.
    ///
    /// Never fails the caller's work. Every call site is a decision that has already been taken or
    /// refused, and a full disk must not turn a fence that fired into a fence that panicked — so
    /// the error goes to `tracing` here and the return is `()`.
    pub fn append_audit(&self, entry: &offload_core::AuditEntry) {
        let json = match serde_json::to_string(entry) {
            Ok(json) => json,
            Err(e) => {
                tracing::error!(error = %e, "could not encode an audit entry");
                return;
            }
        };
        let written = self.conn().execute(
            "INSERT INTO audit (at_unix_ms, kind, run_id, event_json) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                i64::try_from(entry.at_unix_ms).unwrap_or(i64::MAX),
                entry.kind.kind_name(),
                entry.run().as_bytes().as_slice(),
                json
            ],
        );
        if let Err(e) = written {
            tracing::error!(error = %e, "could not record an audit entry");
        }
    }

    /// The audit trail, newest first. `run` narrows it to one run.
    ///
    /// Newest first because the question is almost always about what just happened, and bounded
    /// because this table is never pruned: it outlives the runs it describes on purpose, since the
    /// rows worth reading are the ones about a run this node has stopped holding.
    pub fn audit(
        &self,
        run: Option<offload_core::RunId>,
        limit: u32,
    ) -> Result<Vec<offload_core::AuditEntry>, StoreError> {
        let conn = self.conn();
        let rows: Vec<String> = match run {
            Some(run) => conn
                .prepare(
                    "SELECT event_json FROM audit WHERE run_id = ?1 ORDER BY seq DESC LIMIT ?2",
                )?
                .query_map(rusqlite::params![run.as_bytes().as_slice(), limit], |row| {
                    row.get(0)
                })?
                .filter_map(Result::ok)
                .collect(),
            None => conn
                .prepare("SELECT event_json FROM audit ORDER BY seq DESC LIMIT ?1")?
                .query_map([limit], |row| row.get(0))?
                .filter_map(Result::ok)
                .collect(),
        };
        Ok(rows
            .iter()
            .filter_map(|json| serde_json::from_str(json).ok())
            .collect())
    }

    /// Has this node already recorded that `node` is a member?
    ///
    /// What stops the same device being announced twice: the machine that issued an invitation
    /// records an enrolment, and then meets the device and would record it again as a stranger.
    /// One event per device per node — which is the most this log can promise, because it is a
    /// record of what *this* node witnessed and not the fleet's memory.
    pub fn knows_member(&self, node: &offload_core::NodeId) -> Result<bool, StoreError> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM fleet_events WHERE subject = ?1 AND kind = 'enrolled' LIMIT 1",
                [node.to_string()],
                |row| row.get(0),
            )
            .ok();
        Ok(found.is_some())
    }

    /// Every device this node has recorded as a member, with the grants it was recorded with.
    ///
    /// What `offload rekey` re-issues to. This is the only list of members available to a
    /// command that runs with no daemon and no network, which is what these commands do by
    /// design — and it is honest about its own limits: it holds the devices this node
    /// *witnessed*, so a member enrolled elsewhere and never met is one somebody has to
    /// re-invite by hand. Said in the output rather than papered over.
    pub fn known_members(
        &self,
    ) -> Result<Vec<(offload_core::NodeId, String, Vec<String>)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT event_json FROM fleet_events WHERE kind = 'enrolled' ORDER BY seq")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out: std::collections::BTreeMap<offload_core::NodeId, (String, Vec<String>)> =
            std::collections::BTreeMap::new();
        for row in rows {
            let json = row?;
            let Ok(event) = serde_json::from_str::<FleetLogEvent>(&json) else {
                continue;
            };
            if let offload_core::FleetEvent::Enrolled {
                node, name, grants, ..
            } = event.kind
            {
                // Last writer wins: a device that was met again under a wider certificate is
                // described by the most recent thing this node saw about it.
                out.insert(
                    node,
                    (name, grants.iter().map(ToString::to_string).collect()),
                );
            }
        }
        Ok(out
            .into_iter()
            .map(|(node, (name, grants))| (node, name, grants))
            .collect())
    }

    /// The fleet log, newest first, for `offload nodes --history`.
    pub fn fleet_events(&self, limit: u32) -> Result<Vec<(u64, FleetLogEvent)>, StoreError> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT seq, event_json FROM fleet_events ORDER BY seq DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, json) = row?;
            match serde_json::from_str(&json) {
                Ok(event) => out.push((u64::try_from(seq).unwrap_or(0), event)),
                Err(e) => tracing::error!(seq, error = %e, "unreadable fleet event"),
            }
        }
        Ok(out)
    }

    /// Fleet log entries a sink has not been shown yet, oldest first.
    ///
    /// The mirror of [`Store::events_to_notice`], and separate for the reason the logs are
    /// separate: the sequences are independent, so one scan cannot serve both.
    pub fn fleet_events_to_notice(
        &self,
        after: u64,
        kinds: &[&str],
        limit: u32,
    ) -> Result<Vec<(u64, String)>, StoreError> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (0..kinds.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT seq, event_json FROM fleet_events
             WHERE seq > ?1 AND kind IN ({placeholders})
             ORDER BY seq LIMIT {limit}"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(kinds.len() + 1);
        let after = i64::try_from(after).unwrap_or(i64::MAX);
        params.push(&after);
        for kind in kinds {
            params.push(kind);
        }
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, json) = row?;
            out.push((u64::try_from(seq).unwrap_or(0), json));
        }
        Ok(out)
    }

    /// One fleet log entry by sequence, for turning an outbox row back into a notification.
    pub fn fleet_event_at(&self, seq: u64) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT event_json FROM fleet_events WHERE seq = ?1",
                [i64::try_from(seq).unwrap_or(i64::MAX)],
                |row| row.get(0),
            )
            .ok())
    }

    /// Put a notification in a sink's outbox, if it is not already there.
    ///
    /// Returns whether this call was the one that created the row. Idempotent by primary key,
    /// which is what makes the scan safe to run twice — a daemon that restarts mid-pass
    /// re-notices the same events and adds nothing.
    pub fn notice_delivery(
        &self,
        sink: &str,
        topic: Topic,
        seq: u64,
        run: Option<RunId>,
        now: Millis,
    ) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "INSERT OR IGNORE INTO deliveries (sink, topic, seq, run_id, noticed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                sink,
                topic.name(),
                i64::try_from(seq).unwrap_or(i64::MAX),
                run.map(|run| run.as_bytes().to_vec()),
                i64::try_from(now.0).unwrap_or(i64::MAX)
            ],
        )?;
        Ok(changed > 0)
    }

    /// The queue: noticed, not delivered, oldest first — and **`per_sink`, not per pass**.
    ///
    /// Oldest first because a person reading two notifications wants them in the order the
    /// things happened, and because a sink that has been down for an hour should drain in that
    /// order rather than newest-first.
    ///
    /// The budget is per sink because a shared one starves. A route that is merely *away* keeps
    /// its rows pending indefinitely and spends none of its retry budget — that is the promise,
    /// and it is right — but those rows are also the *oldest* ones in the table. One page of the
    /// lowest sequences was therefore all phone: a device asleep for a weekend accumulated more
    /// undeliverable rows than a page holds, and from then on every other route on the node
    /// stopped being delivered to. Not deferred, not failed — never selected, so `offload sinks`
    /// showed a working desktop with a growing "waiting" count and no error to explain it. A
    /// plane that cannot explain a silence has failed at its only job; this one was causing the
    /// silence it could not explain. Per sink, one device's backlog is one device's problem.
    ///
    /// Ordered by when this node *noticed*, with the sequence as a tiebreak. The sequence alone
    /// was the ordering and is not a clock: there are two logs and they number independently, so
    /// a fleet event at seq 3 sorted ahead of a run event at seq 900 that happened a week
    /// earlier. `noticed_at_ms` is this node's own observation of both, which makes the
    /// documented "in the order the things happened" true rather than nearly true.
    pub fn pending_deliveries(&self, per_sink: u32) -> Result<Vec<Pending>, StoreError> {
        let conn = self.conn();
        let mut sinks = conn.prepare(
            "SELECT DISTINCT sink FROM deliveries WHERE delivered_at_ms IS NULL ORDER BY sink",
        )?;
        let names: Vec<String> = sinks
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        drop(sinks);

        let mut stmt = conn.prepare(
            "SELECT sink, topic, seq, run_id, attempts, last_error FROM deliveries
             WHERE delivered_at_ms IS NULL AND sink = ?1
             ORDER BY noticed_at_ms, seq LIMIT ?2",
        )?;
        let mut raw = Vec::new();
        for name in &names {
            let rows = stmt.query_map(rusqlite::params![name, per_sink], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?;
            for row in rows {
                raw.push(row?);
            }
        }

        let mut out = Vec::new();
        for (sink, topic, seq, id, attempts, last_error) in raw {
            let Some(topic) = Topic::from_name(&topic) else {
                tracing::error!(seq, topic, "skipping a delivery for an unknown topic");
                continue;
            };
            // A run row with an unreadable id is skipped loudly; a fleet row has none by
            // design, and confusing the two would drop every membership alarm.
            let run = match (topic, id) {
                (Topic::Run, Some(bytes)) => match run_id(&bytes) {
                    Some(run) => Some(run),
                    None => {
                        tracing::error!(seq, "skipping a delivery with an unreadable run id");
                        continue;
                    }
                },
                (Topic::Run, None) => {
                    tracing::error!(seq, "skipping a run delivery with no run id");
                    continue;
                }
                (Topic::Fleet, _) => None,
            };
            out.push(Pending {
                sink,
                topic,
                seq: u64::try_from(seq).unwrap_or(0),
                run,
                attempts: u32::try_from(attempts).unwrap_or(u32::MAX),
                last_error,
            });
        }
        Ok(out)
    }

    /// One log entry by sequence, for turning an outbox row back into a notification.
    ///
    /// The outbox stores the *identity* of a notification and not its content, so that the
    /// content is always re-derived from the log — ADR-0010's "the event log stays the source
    /// of truth" taken literally. A copy in the outbox would be a second version of the fact,
    /// and the two would eventually disagree.
    pub fn event_at(&self, seq: u64) -> Result<Option<(RunId, String)>, StoreError> {
        let conn = self.conn();
        let row: Option<(Vec<u8>, String)> = conn
            .query_row(
                "SELECT run_id, event_json FROM run_events WHERE seq = ?1",
                [i64::try_from(seq).unwrap_or(i64::MAX)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        let Some((id, json)) = row else {
            return Ok(None);
        };
        Ok(run_id(&id).map(|run| (run, json)))
    }

    /// It arrived. Recorded before anything else, because the alternative to writing this down
    /// is sending it again.
    pub fn delivery_sent(
        &self,
        sink: &str,
        topic: Topic,
        seq: u64,
        now: Millis,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE deliveries
             SET delivered_at_ms = ?3, attempts = attempts + 1, last_error = NULL
             WHERE sink = ?1 AND topic = ?4 AND seq = ?2",
            rusqlite::params![
                sink,
                i64::try_from(seq).unwrap_or(i64::MAX),
                i64::try_from(now.0).unwrap_or(i64::MAX),
                topic.name()
            ],
        )?;
        Ok(())
    }

    /// It did not. The row stays pending, and the reason stays with it: "why did my phone not
    /// buzz" is the question this plane exists to be able to answer.
    pub fn delivery_failed(
        &self,
        sink: &str,
        topic: Topic,
        seq: u64,
        error: &str,
    ) -> Result<u32, StoreError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE deliveries SET attempts = attempts + 1, last_error = ?3
             WHERE sink = ?1 AND topic = ?4 AND seq = ?2",
            rusqlite::params![
                sink,
                i64::try_from(seq).unwrap_or(i64::MAX),
                error,
                topic.name()
            ],
        )?;
        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM deliveries WHERE sink = ?1 AND topic = ?3 AND seq = ?2",
                rusqlite::params![sink, i64::try_from(seq).unwrap_or(i64::MAX), topic.name()],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(u32::try_from(attempts).unwrap_or(u32::MAX))
    }

    /// It could not be tried, because the device holding the route is not answering.
    ///
    /// The reason is recorded and the attempt count is **not** touched, which is the whole point:
    /// a notification that never reached a device is not one the device refused. Without this
    /// split, a phone asleep for a minute exhausts its retries and the news is thrown away —
    /// which is the exact opposite of what somebody wants from a plane whose promise is that they
    /// will be told when they wake up. So a queued notification waits indefinitely for a route
    /// that is merely away, and gives up only on one that answered and would not carry it.
    ///
    /// The cost, stated plainly: rows accumulate for a device that never comes back. They are
    /// tens of bytes, and the answer for a device that is gone for good is to revoke it.
    pub fn delivery_deferred(
        &self,
        sink: &str,
        topic: Topic,
        seq: u64,
        reason: &str,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE deliveries SET last_error = ?3 WHERE sink = ?1 AND topic = ?4 AND seq = ?2",
            rusqlite::params![
                sink,
                i64::try_from(seq).unwrap_or(i64::MAX),
                reason,
                topic.name()
            ],
        )?;
        Ok(())
    }

    /// Stop retrying, and keep the reason. Not a delete: a notification that was given up on
    /// is a thing that happened, and a table that only remembers successes cannot explain a
    /// silence.
    pub fn delivery_abandoned(
        &self,
        sink: &str,
        topic: Topic,
        seq: u64,
        now: Millis,
        error: &str,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE deliveries SET delivered_at_ms = ?3, last_error = ?4
             WHERE sink = ?1 AND topic = ?5 AND seq = ?2",
            rusqlite::params![
                sink,
                i64::try_from(seq).unwrap_or(i64::MAX),
                i64::try_from(now.0).unwrap_or(i64::MAX),
                format!("gave up: {error}"),
                topic.name()
            ],
        )?;
        Ok(())
    }

    /// What a sink has done lately, for `offload sinks`.
    ///
    /// **Delivered and given-up are counted apart**, and they are both resolved rows, which is
    /// why this is easy to get wrong — the first version of `offload sinks` reported a route
    /// that had never worked as having delivered four notifications, because "not waiting any
    /// more" is not the same as "arrived". A row that was abandoned keeps its reason, so the
    /// reason is what tells them apart.
    pub fn sink_totals(&self, sink: &str) -> Result<SinkTotals, StoreError> {
        let conn = self.conn();
        let sent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM deliveries
                 WHERE sink = ?1 AND delivered_at_ms IS NOT NULL AND last_error IS NULL",
                [sink],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let gave_up: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM deliveries
                 WHERE sink = ?1 AND delivered_at_ms IS NOT NULL AND last_error IS NOT NULL",
                [sink],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let waiting: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM deliveries WHERE sink = ?1 AND delivered_at_ms IS NULL",
                [sink],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let last_error: Option<String> = conn
            .query_row(
                "SELECT last_error FROM deliveries
                 WHERE sink = ?1 AND last_error IS NOT NULL
                 ORDER BY seq DESC LIMIT 1",
                [sink],
                |row| row.get(0),
            )
            .ok()
            .flatten();
        Ok(SinkTotals {
            delivered: u64::try_from(sent).unwrap_or(0),
            waiting: u64::try_from(waiting).unwrap_or(0),
            gave_up: u64::try_from(gave_up).unwrap_or(0),
            last_error,
        })
    }
}

/// A run id out of the blob column, or nothing.
///
/// `None` rather than a panic on a wrong-length row: this is a delivery path, and the right
/// answer to one unreadable row is to skip it loudly and keep sending the rest.
fn run_id(bytes: &[u8]) -> Option<RunId> {
    <[u8; 16]>::try_from(bytes).ok().map(RunId::from_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{
        AgentKind, AgentWork, Constraint, LogEvent, LogKind, NodeId, PermissionMode,
        Restartability, Run, RunSpec, ToolAllowlist, Work, WorkspaceSpec,
    };

    fn store_with_a_run() -> (Store, RunId) {
        let store = Store::open_memory().expect("open");
        let id = RunId::from_bytes([3; 16]);
        let spec = RunSpec {
            work: Work::Agent(AgentWork {
                agent: AgentKind::ClaudeCode,
                model: None,
                prompt: "p".into(),
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
            demand: offload_core::Demand::Normal,
            notify: offload_core::Audience::Everyone,
            notices: Default::default(),
            resources: Vec::new(),
            prefer: offload_core::Constraint::Always,
            hold_until: None,
            parent: None,
        };
        let run = Run::new(id, spec, NodeId::from_bytes([1; 32]), Millis(1));
        store.save_run(&run).expect("save");
        (store, id)
    }

    fn append(store: &Store, run: RunId, kind: LogKind) -> u64 {
        let event = LogEvent::at(Millis(100), kind);
        let seq = store
            .append_event(run, event.kind_name(), event.at_unix_ms, &event)
            .expect("append");
        u64::try_from(seq).unwrap_or(0)
    }

    /// A device that is away must not take the whole fleet's news down with it.
    ///
    /// The second-order failure of "away is not gone" — which is right, and which makes a
    /// sleeping phone's rows both permanent and the *oldest* in the table. With one shared page
    /// budget ordered by sequence, a phone asleep for a weekend filled every page, and from then
    /// on every other route on the node stopped being delivered to. Never attempted, so no error
    /// was recorded: `offload sinks` showed a working desktop with a growing "waiting" count and
    /// nothing at all to explain it.
    #[test]
    fn one_sleeping_device_does_not_starve_every_other_route() {
        let (store, run) = store_with_a_run();
        for seq in 1..=40u64 {
            store
                .notice_delivery("phone", Topic::Run, seq, Some(run), Millis(seq))
                .expect("notice");
            store
                .delivery_deferred("phone", Topic::Run, seq, "phone is not answering")
                .expect("defer");
        }
        // …and then something finishes now, with the desktop's own route owed it.
        store
            .notice_delivery("desktop", Topic::Run, 41, Some(run), Millis(41))
            .expect("notice");

        let page = store.pending_deliveries(32).expect("pending");
        assert!(
            page.iter().any(|p| p.sink == "desktop"),
            "the working route was crowded out by a sleeping one: {:?}",
            page.iter().map(|p| p.sink.as_str()).collect::<Vec<_>>()
        );
        // And the phone still gets its budget, in the order the news happened.
        let phone: Vec<u64> = page
            .iter()
            .filter(|p| p.sink == "phone")
            .map(|p| p.seq)
            .collect();
        assert_eq!(phone.len(), 32, "the away route keeps its own budget");
        assert!(
            phone.windows(2).all(|w| w[0] < w[1]),
            "oldest first: {phone:?}"
        );
    }

    /// Two logs number independently, so a sequence is not a clock.
    ///
    /// `ORDER BY seq` put a fleet event at seq 3 ahead of a run event at seq 900 that happened a
    /// week earlier. What both logs share is when *this node* noticed them.
    #[test]
    fn news_is_queued_in_the_order_it_was_noticed_across_both_logs() {
        let (store, run) = store_with_a_run();
        store
            .notice_delivery("phone", Topic::Run, 900, Some(run), Millis(1_000))
            .expect("notice");
        store
            .notice_delivery("phone", Topic::Fleet, 3, None, Millis(2_000))
            .expect("notice");

        let page = store.pending_deliveries(8).expect("pending");
        let order: Vec<Topic> = page.iter().map(|p| p.topic).collect();
        assert_eq!(
            order,
            vec![Topic::Run, Topic::Fleet],
            "the run event happened first and has the larger sequence"
        );
    }

    #[test]
    fn a_new_sink_starts_at_the_end_of_the_log() {
        // The rule that stops somebody's first notification being a month of history. Three
        // finished runs already logged, and a sink configured now hears about none of them.
        let (store, run) = store_with_a_run();
        for _ in 0..3 {
            append(
                &store,
                run,
                LogKind::Failed {
                    reason: "old".into(),
                },
            );
        }
        let cursor = store
            .sink_cursor("phone", Topic::Run, Millis(1))
            .expect("cursor");
        assert_eq!(
            store
                .events_to_notice(cursor, offload_core::NOTABLE_KINDS, 10)
                .expect("scan")
                .len(),
            0
        );

        // And from here on it hears everything.
        let seq = append(
            &store,
            run,
            LogKind::Failed {
                reason: "new".into(),
            },
        );
        let found = store
            .events_to_notice(cursor, offload_core::NOTABLE_KINDS, 10)
            .expect("scan");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, seq);
    }

    #[test]
    fn the_scan_ignores_everything_that_is_not_news() {
        let (store, run) = store_with_a_run();
        let cursor = store
            .sink_cursor("phone", Topic::Run, Millis(1))
            .expect("cursor");
        append(
            &store,
            run,
            LogKind::Text {
                text: "chatter".into(),
            },
        );
        append(&store, run, LogKind::TurnBoundary { turn: 1 });
        append(
            &store,
            run,
            LogKind::ToolUse {
                name: "Bash".into(),
            },
        );
        let news = append(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );

        let found = store
            .events_to_notice(cursor, offload_core::NOTABLE_KINDS, 10)
            .expect("scan");
        assert_eq!(
            found.len(),
            1,
            "one of four events was worth telling anybody"
        );
        assert_eq!(found[0].0, news);
    }

    #[test]
    fn noticing_the_same_event_twice_queues_it_once() {
        // The at-least-once mechanism, from the side that matters: a daemon that restarts
        // mid-pass re-scans and must not double-send.
        let (store, run) = store_with_a_run();
        let seq = append(&store, run, LogKind::Failed { reason: "x".into() });
        assert!(store
            .notice_delivery("phone", Topic::Run, seq, Some(run), Millis(1))
            .expect("notice"));
        assert!(!store
            .notice_delivery("phone", Topic::Run, seq, Some(run), Millis(1))
            .expect("notice again"));
        assert_eq!(store.pending_deliveries(10).expect("pending").len(), 1);

        // A second sink is a second delivery of the same fact, which is the other half of the
        // `(sink, seq)` key.
        assert!(store
            .notice_delivery("laptop", Topic::Run, seq, Some(run), Millis(1))
            .expect("notice"));
        assert_eq!(store.pending_deliveries(10).expect("pending").len(), 2);
    }

    #[test]
    fn a_broken_sink_does_not_block_the_one_behind_it() {
        // The reason the cursor and the outbox are two tables. The phone is down; the laptop
        // is fine; the cursor moves for both and the phone accumulates work to retry.
        let (store, run) = store_with_a_run();
        let cursor = store
            .sink_cursor("phone", Topic::Run, Millis(1))
            .expect("cursor");
        let first = append(
            &store,
            run,
            LogKind::Failed {
                reason: "one".into(),
            },
        );
        let second = append(
            &store,
            run,
            LogKind::Failed {
                reason: "two".into(),
            },
        );

        for seq in [first, second] {
            store
                .notice_delivery("phone", Topic::Run, seq, Some(run), Millis(1))
                .expect("notice");
        }
        store
            .advance_sink("phone", Topic::Run, second)
            .expect("advance");
        assert_eq!(
            store
                .sink_cursor("phone", Topic::Run, Millis(1))
                .expect("cursor"),
            second,
            "the scan moved on regardless"
        );
        assert!(cursor < first);

        let attempts = store
            .delivery_failed("phone", Topic::Run, first, "no such command")
            .expect("failed");
        assert_eq!(attempts, 1);
        store
            .delivery_sent("phone", Topic::Run, second, Millis(9))
            .expect("sent");

        let pending = store.pending_deliveries(10).expect("pending");
        assert_eq!(pending.len(), 1, "only the failure is still queued");
        assert_eq!(pending[0].seq, first);
        assert_eq!(pending[0].last_error.as_deref(), Some("no such command"));
    }

    #[test]
    fn giving_up_keeps_the_reason() {
        // A table that only remembers successes cannot explain a silence, which is the one
        // question a delivery plane has to be able to answer.
        let (store, run) = store_with_a_run();
        let seq = append(&store, run, LogKind::Failed { reason: "x".into() });
        store
            .notice_delivery("phone", Topic::Run, seq, Some(run), Millis(1))
            .expect("notice");
        store
            .delivery_abandoned("phone", Topic::Run, seq, Millis(9), "no such command")
            .expect("abandon");

        assert!(store.pending_deliveries(10).expect("pending").is_empty());
        let totals = store.sink_totals("phone").expect("totals");
        assert_eq!(
            (totals.delivered, totals.waiting, totals.gave_up),
            (0, 0, 1),
            "abandoned is not delivered"
        );
        assert_eq!(
            totals.last_error.as_deref(),
            Some("gave up: no such command")
        );
    }

    #[test]
    fn a_delivery_is_re_derived_from_the_log_rather_than_copied() {
        let (store, run) = store_with_a_run();
        let seq = append(
            &store,
            run,
            LogKind::Failed {
                reason: "why".into(),
            },
        );
        let (found, json) = store.event_at(seq).expect("event").expect("some");
        assert_eq!(found, run);
        let event: LogEvent = serde_json::from_str(&json).expect("decode");
        assert!(matches!(event.kind, LogKind::Failed { .. }));
        assert!(store.event_at(seq + 999).expect("event").is_none());
    }

    fn enrolled(seed: u8, name: &str) -> offload_core::FleetLogEvent {
        offload_core::FleetLogEvent::new(
            1_700_000_000_000,
            offload_core::FleetEvent::Enrolled {
                node: NodeId::from_bytes([seed; 32]),
                name: name.into(),
                grants: offload_core::default_grants(),
                how: offload_core::Enrolment::Invited,
            },
        )
    }

    #[test]
    fn the_two_logs_number_separately_and_do_not_shadow_each_other() {
        // The reason `topic` is half of every key here. Both logs start at 1, so a fleet event
        // and a run event share a sequence — and an outbox keyed on the sequence alone would
        // have the first of them mark the second sent.
        let (store, run) = store_with_a_run();
        let run_seq = append(
            &store,
            run,
            LogKind::Finished {
                success: true,
                turns: 1,
                denials: 0,
                cost_micro_usd: 1,
                work: offload_core::WorkKind::Agent,
                result: None,
            },
        );
        let fleet_seq = store
            .append_fleet_event(&enrolled(9, "laptop"))
            .expect("append");
        assert_eq!(run_seq, fleet_seq, "the premise: two logs, one number");

        assert!(store
            .notice_delivery("phone", Topic::Run, run_seq, Some(run), Millis(1))
            .expect("notice"));
        assert!(
            store
                .notice_delivery("phone", Topic::Fleet, fleet_seq, None, Millis(1))
                .expect("notice"),
            "the fleet event is owed a delivery of its own"
        );

        let pending = store.pending_deliveries(10).expect("pending");
        assert_eq!(pending.len(), 2);

        // And marking one sent leaves the other waiting.
        store
            .delivery_sent("phone", Topic::Run, run_seq, Millis(2))
            .expect("sent");
        let pending = store.pending_deliveries(10).expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].topic, Topic::Fleet);
        assert_eq!(pending[0].run, None);
    }

    #[test]
    fn a_new_sink_starts_at_the_end_of_the_fleet_log_too() {
        // The same rule as the run log, and it matters more here: a route configured today must
        // not announce every device that ever joined, because the whole value of this alarm is
        // that it is rare enough to read.
        let (store, _run) = store_with_a_run();
        store
            .append_fleet_event(&enrolled(9, "laptop"))
            .expect("append");

        let cursor = store
            .sink_cursor("phone", Topic::Fleet, Millis(1))
            .expect("cursor");
        assert_eq!(cursor, 1);
        assert!(store
            .fleet_events_to_notice(cursor, offload_core::NOTABLE_FLEET_KINDS, 10)
            .expect("scan")
            .is_empty());

        store
            .append_fleet_event(&enrolled(8, "phone"))
            .expect("append");
        assert_eq!(
            store
                .fleet_events_to_notice(cursor, offload_core::NOTABLE_FLEET_KINDS, 10)
                .expect("scan")
                .len(),
            1
        );
    }

    #[test]
    fn a_member_already_recorded_is_not_announced_twice() {
        // The machine that issued an invitation records an enrolment and then *meets* the
        // device. Without this it would announce the same laptop twice, and the second one
        // reads as a second device.
        let (store, _run) = store_with_a_run();
        let node = NodeId::from_bytes([9; 32]);
        assert!(!store.knows_member(&node).expect("knows"));
        store
            .append_fleet_event(&enrolled(9, "laptop"))
            .expect("append");
        assert!(store.knows_member(&node).expect("knows"));
        assert!(
            !store
                .knows_member(&NodeId::from_bytes([8; 32]))
                .expect("knows"),
            "and it is about this device rather than about there being any"
        );
    }
}
