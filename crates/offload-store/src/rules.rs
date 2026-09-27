//! Standing instructions: when a trigger fires here, submit this run (ADR-0020).
//!
//! A **rule** binds one of this node's triggers — named by *service*, never by the trigger's own
//! id — to a whole submission. It is node-local and never gossiped, and that is the entire reason
//! this is the small half of ADR-0019: a trigger is a program *this node's owner* nominated, so
//! it fires on exactly one machine, and the two-nodes-fire-one-tick problem that forced a
//! gossiped schedule into a derived `RunId` simply does not arise.
//!
//! What this crate deliberately does not know is what a rule *fires*. `request_json` is a whole
//! `SubmitRequest`, which lives in `offload-node` — a layer this one cannot see and must not —
//! so it is kept as text, exactly as `run_json` is. The daemon owns the meaning; the store owns
//! the row.
//!
//! Two counters, because they are two facts. `fired` is what happened; `dropped` is what arrived
//! while the last occurrence was still running and was therefore discarded (ADR-0020 §3, which is
//! ADR-0019's no-catch-up rule reached from the other direction). The second is the *only*
//! symptom its own failure has — a rule whose runs are slower than its trigger's cadence is
//! correct, silent in the fleet, and visible nowhere else.

use crate::{Store, StoreError};
use offload_core::{RuleId, RunId};
use rusqlite::OptionalExtension;

/// One standing instruction, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: RuleId,
    /// The service whose trigger fires it, spelled as `offload_core::Service` displays it.
    /// Canonicalised by the writer, so comparison here is a string compare.
    pub service: String,
    /// The submission to make, opaque to this crate.
    pub request_json: String,
    pub created_at_ms: i64,
    pub fired: u64,
    /// Events discarded because this rule's last run had not finished. Never folded into
    /// `fired`: a rule reporting "fired 3, dropped 412" is telling somebody the one thing they
    /// need to know, and a single total would hide it.
    pub dropped: u64,
    pub last_fired_ms: Option<i64>,
    pub last_run: Option<RunId>,
    /// Why the last firing did not produce a run, if it did not. Cleared by a firing that works.
    pub last_error: Option<String>,
}

impl Store {
    /// Write a new rule. The caller mints the id and canonicalises the service.
    pub fn add_rule(
        &self,
        id: RuleId,
        service: &str,
        request_json: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO rules (id, service, request_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id.as_bytes().as_slice(), service, request_json, now_ms],
        )?;
        Ok(())
    }

    /// Every rule on this node, oldest first.
    pub fn rules(&self) -> Result<Vec<Rule>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, service, request_json, created_at_ms, fired, dropped,
                    last_fired_ms, last_run, last_error
             FROM rules ORDER BY created_at_ms",
        )?;
        let rows = stmt.query_map([], decode_rule)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// The rules a given service's trigger fires, oldest first.
    pub fn rules_for_service(&self, service: &str) -> Result<Vec<Rule>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, service, request_json, created_at_ms, fired, dropped,
                    last_fired_ms, last_run, last_error
             FROM rules WHERE service = ?1 ORDER BY created_at_ms",
        )?;
        let rows = stmt.query_map([service], decode_rule)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Resolve a full or abbreviated rule id.
    ///
    /// A prefix, not a pattern — `LIKE` reads `%` and `_` as wildcards, and a single match here
    /// *resolves* rather than refusing, which is how a wildcard becomes somebody's standing
    /// instruction being deleted. A rule id is hex; anything else is not an abbreviation of one.
    pub fn resolve_rule(&self, needle: &str) -> Result<RuleId, StoreError> {
        let needle = needle.trim().to_ascii_lowercase();
        if needle.is_empty() || !needle.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StoreError::NoSuchRule(format!(
                "`{needle}` (a rule id is hex — that is not an abbreviation of one)"
            )));
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM rules WHERE lower(hex(id)) LIKE ?1 || '%'
             ORDER BY created_at_ms LIMIT 2",
        )?;
        let matches: Vec<Vec<u8>> = stmt
            .query_map([&needle], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();
        match matches.len() {
            0 => Err(StoreError::NoSuchRule(format!("`{needle}`"))),
            1 => {
                let bytes: [u8; 8] = matches[0]
                    .clone()
                    .try_into()
                    .map_err(|_| StoreError::NoSuchRule(format!("`{needle}`")))?;
                Ok(RuleId::from_bytes(bytes))
            }
            _ => Err(StoreError::NoSuchRule(format!(
                "`{needle}` (ambiguous — use more characters)"
            ))),
        }
    }

    /// Forget a rule. `false` if there was nothing there.
    pub fn remove_rule(&self, id: RuleId) -> Result<bool, StoreError> {
        let conn = self.conn();
        let n = conn.execute(
            "DELETE FROM rules WHERE id = ?1",
            [id.as_bytes().as_slice()],
        )?;
        Ok(n > 0)
    }

    /// Record that a rule produced a run.
    pub fn note_rule_fired(&self, id: RuleId, run: RunId, now_ms: i64) -> Result<(), StoreError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE rules
             SET fired = fired + 1, last_fired_ms = ?2, last_run = ?3, last_error = NULL
             WHERE id = ?1",
            rusqlite::params![id.as_bytes().as_slice(), now_ms, run.as_bytes().as_slice()],
        )?;
        Ok(())
    }

    /// Record that an event arrived and was discarded.
    ///
    /// The reason is kept as `last_error` for the same purpose the count is: "why did nothing
    /// happen" is the only question anybody asks a trigger, and a bare number does not answer it.
    pub fn note_rule_dropped(&self, id: RuleId, why: &str) -> Result<(), StoreError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE rules SET dropped = dropped + 1, last_error = ?2 WHERE id = ?1",
            rusqlite::params![id.as_bytes().as_slice(), why],
        )?;
        Ok(())
    }

    /// Record that a run is this rule's occurrence (ADR-0021 §1).
    ///
    /// Separate from [`Store::note_rule_fired`] and from the run's own row-write on purpose: the
    /// tag is node-local bookkeeping about a run whose record may have been written by the
    /// placement path a moment earlier, or by a gossip merge, and neither of those knows anything
    /// about rules. `false` means there was no such row to tag — which cannot happen on the path
    /// that calls this (a placed run has a local record either way) and is reported rather than
    /// swallowed, because an untagged occurrence is one nothing will ever prune.
    pub fn tag_occurrence(&self, run: RunId, rule: RuleId) -> Result<bool, StoreError> {
        let n = self.conn().execute(
            "UPDATE runs SET rule = ?2 WHERE id = ?1",
            rusqlite::params![run.as_bytes().as_slice(), rule.as_bytes().as_slice()],
        )?;
        Ok(n > 0)
    }

    /// Is this run tagged as an occurrence of a rule *on this node*?
    ///
    /// The discriminator ADR-0030 rests on, and it needs nothing new because `runs.rule` already
    /// has exactly the right shape: it is written by the firing (`tag_occurrence`) and
    /// deliberately **left alone by a gossip merge**, so a peer that learns the record learns it
    /// untagged. Which makes "tagged here" mean "a rule on this machine fired it" — and a run
    /// whose `origin` says a machine started it and whose tag is absent is somebody else's rule's.
    ///
    /// `false` for every operator run too, which is why the caller checks `origin` first: this
    /// answers "is it one of mine", not "is it an occurrence".
    pub fn is_local_occurrence(&self, run: RunId) -> Result<bool, StoreError> {
        let tagged: Option<Option<Vec<u8>>> = self
            .conn()
            .query_row(
                "SELECT rule FROM runs WHERE id = ?1",
                [run.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(matches!(tagged, Some(Some(_))))
    }

    /// How many of a rule's occurrences still have a record here.
    ///
    /// What `offload rules` prints. A number that only grows is the symptom of every residual
    /// ADR-0021 accepts — a failing rule, a phone that never came back, a route nobody removed —
    /// and none of them has any other visible sign.
    pub fn occurrences_kept(&self, rule: RuleId) -> Result<u64, StoreError> {
        let n: i64 = self.conn().query_row(
            "SELECT count(*) FROM runs WHERE rule = ?1",
            [rule.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        Ok(n.unsigned_abs())
    }

    /// The occurrences of a rule whose records nothing needs any more (ADR-0021 §§2-5).
    ///
    /// Every clause is a bound from somewhere else, and each one is a *temporary* no — which is
    /// why this is asked about every occurrence at every firing rather than once per record:
    ///
    /// * **completed**, never failed or cancelled. A failure is the record somebody comes back
    ///   for: the notification said it failed and only the log says why (§2).
    /// * **quiet since `written_before_ms`**, which is `now - GOSSIP_TAIL`. A finished run is
    ///   still gossiped for a while after it ends, and a peer would teach a deleted record
    ///   straight back — untagged, because the merge path knows nothing about rules, and
    ///   therefore unprunable for ever. `updated_at_ms` asks *has anything written this row*,
    ///   which is the question, rather than inferring it from when the run ended (§5).
    /// * **nothing owed**: no outbox row about it is still pending. Delivered and abandoned both
    ///   count as resolved — giving up was a decision already taken (§3).
    /// * **nothing left to notice**: no event of this run sits above `scanned_to`, the lowest
    ///   run-log cursor among the routes that currently exist. The outbox is filled by a scan,
    ///   so deleting events a route has not reached destroys the news before the promise to
    ///   deliver it is ever made — and that promise has no row to point at yet, which is what
    ///   makes this the half that is easy to miss (§3).
    ///
    /// `except` is the occurrence the caller has just recorded, which is never a candidate.
    ///
    /// **Asked of `machine_started`, not of the rule** (ADR-0024). The clauses are unchanged; what
    /// changed is who may ask. A rule id is one machine's name for one of its own things, so
    /// keying on it meant only the rule's own node could ever prune — and a peer that hosted the
    /// occurrences kept a record for each of them for ever (measured: 81 → 181 in five minutes).
    /// The clauses degrade correctly rather than being skipped there: a peer holds no events for
    /// a run it never ran, so "nothing owed" and "nothing left to notice" are satisfied by having
    /// nothing, and the quiet period and the completed-not-failed rule do the work. `rule`
    /// narrows it when the caller has one, which is the firing.
    pub fn spent_occurrences(
        &self,
        rule: Option<RuleId>,
        except: Option<RunId>,
        written_before_ms: i64,
        scanned_to: u64,
    ) -> Result<Vec<RunId>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM runs
              WHERE machine_started = 1
                AND (?1 IS NULL OR rule = ?1)
                AND state = 'completed'
                AND updated_at_ms < ?2
                AND (?3 IS NULL OR id != ?3)
                AND NOT EXISTS (
                      SELECT 1 FROM deliveries d
                       WHERE d.run_id = runs.id AND d.delivered_at_ms IS NULL)
                AND NOT EXISTS (
                      SELECT 1 FROM run_events e
                       WHERE e.run_id = runs.id AND e.seq > ?4)
              ORDER BY created_at_ms",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                rule.map(|id| id.as_bytes().to_vec()),
                written_before_ms,
                except.map(|id| id.as_bytes().to_vec()),
                i64::try_from(scanned_to).unwrap_or(i64::MAX),
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )?;
        let mut out = Vec::new();
        for row in rows {
            let bytes: [u8; 16] = row?.try_into().map_err(|_| StoreError::Decode {
                what: "runs.id".into(),
                reason: "not 16 bytes".into(),
            })?;
            out.push(RunId::from_bytes(bytes));
        }
        Ok(out)
    }

    /// Whether this rule's last run has finished — or was never made, or has been cleaned up.
    ///
    /// The in-flight test ADR-0020 §3 rests on. A rule whose last run's row is *gone* is not in
    /// flight: `offload rm` removes finished runs, and reading a missing row as "still running"
    /// would wedge the rule for ever with nothing able to unwedge it.
    pub fn rule_run_in_flight(&self, id: RuleId) -> Result<Option<RunId>, StoreError> {
        let conn = self.conn();
        let last: Option<Vec<u8>> = conn.query_row(
            "SELECT last_run FROM rules WHERE id = ?1",
            [id.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        let Some(bytes) = last else {
            return Ok(None);
        };
        let terminal: Option<i64> = conn
            .query_row("SELECT terminal FROM runs WHERE id = ?1", [&bytes], |row| {
                row.get(0)
            })
            .ok();
        match terminal {
            Some(0) => {
                let arr: [u8; 16] = bytes.try_into().map_err(|_| StoreError::Decode {
                    what: "rules.last_run".into(),
                    reason: "not 16 bytes".into(),
                })?;
                Ok(Some(RunId::from_bytes(arr)))
            }
            _ => Ok(None),
        }
    }
}

fn decode_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<Rule> {
    let id: Vec<u8> = row.get(0)?;
    let last_run: Option<Vec<u8>> = row.get(7)?;
    Ok(Rule {
        id: RuleId::from_bytes(id.try_into().unwrap_or([0u8; 8])),
        service: row.get(1)?,
        request_json: row.get(2)?,
        created_at_ms: row.get(3)?,
        fired: row.get::<_, i64>(4)?.unsigned_abs(),
        dropped: row.get::<_, i64>(5)?.unsigned_abs(),
        last_fired_ms: row.get(6)?,
        last_run: last_run
            .and_then(|b| b.try_into().ok())
            .map(RunId::from_bytes),
        last_error: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{
        AgentKind, AgentWork, Constraint, Millis, NodeId, PermissionMode, Restartability, Run,
        RunSpec, ToolAllowlist, Work, WorkspaceSpec,
    };

    fn a_run(store: &Store, id: RunId) -> Run {
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
        run
    }

    #[test]
    fn a_rule_round_trips_and_is_found_by_its_service() {
        let store = Store::open_memory().expect("open");
        let id = RuleId::from_bytes([9; 8]);
        store.add_rule(id, "email", "{}", 100).expect("add");

        let all = store.rules().expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].fired, 0);
        assert_eq!(all[0].dropped, 0);

        assert_eq!(
            store.rules_for_service("email").expect("by service").len(),
            1
        );
        assert!(
            store
                .rules_for_service("push")
                .expect("by service")
                .is_empty(),
            "a rule belongs to one service; a trigger for another must not fire it"
        );
    }

    /// The in-flight test ADR-0020 §3 rests on, in all three of its states.
    ///
    /// The third one is the interesting one: a rule whose last run's *row* has gone — cleaned up
    /// by `offload rm`, or collected — is not in flight. Reading a missing row as "still running"
    /// would wedge the rule for ever, with nothing able to unwedge it and nothing saying why.
    #[test]
    fn a_rule_is_in_flight_only_while_its_last_run_is() {
        let store = Store::open_memory().expect("open");
        let rule = RuleId::from_bytes([1; 8]);
        store.add_rule(rule, "schedule", "{}", 100).expect("add");
        assert_eq!(
            store.rule_run_in_flight(rule).expect("check"),
            None,
            "a rule that has never fired is not in flight"
        );

        let run_id = RunId::from_bytes([7; 16]);
        let mut run = a_run(&store, run_id);
        store.note_rule_fired(rule, run_id, 200).expect("fired");
        assert_eq!(store.rule_run_in_flight(rule).expect("check"), Some(run_id));

        run.cancel(Millis(300)).expect("cancel");
        store.save_run(&run).expect("save");
        assert!(
            run.state.is_terminal(),
            "the fixture has to actually finish it"
        );
        assert_eq!(
            store.rule_run_in_flight(rule).expect("check"),
            None,
            "a finished run frees the rule to fire again"
        );

        store
            .conn()
            .execute(
                "DELETE FROM runs WHERE id = ?1",
                [run_id.as_bytes().as_slice()],
            )
            .expect("remove the run row");
        assert_eq!(
            store.rule_run_in_flight(rule).expect("check"),
            None,
            "and so does a run whose record has been cleaned up — the alternative is a rule \
             wedged for ever by a row nobody can bring back"
        );
        assert!(
            store.rules().expect("list").len() == 1,
            "the rule outlives the runs it fires: no foreign key, no cascade"
        );
    }

    #[test]
    fn firing_and_dropping_are_counted_separately() {
        let store = Store::open_memory().expect("open");
        let rule = RuleId::from_bytes([2; 8]);
        store.add_rule(rule, "webhook", "{}", 100).expect("add");
        let run_id = RunId::from_bytes([8; 16]);
        a_run(&store, run_id);

        store.note_rule_fired(rule, run_id, 200).expect("fired");
        store
            .note_rule_dropped(rule, "still running")
            .expect("drop");
        store
            .note_rule_dropped(rule, "still running")
            .expect("drop");

        let rule = store.rules().expect("list").remove(0);
        assert_eq!(rule.fired, 1);
        assert_eq!(
            rule.dropped, 2,
            "a drop is not a firing that happened quietly"
        );
        assert_eq!(rule.last_error.as_deref(), Some("still running"));
        assert_eq!(rule.last_run, Some(run_id));
    }

    /// A firing that works clears the reason the last one did not.
    #[test]
    fn a_successful_firing_clears_the_last_reason() {
        let store = Store::open_memory().expect("open");
        let rule = RuleId::from_bytes([3; 8]);
        store.add_rule(rule, "schedule", "{}", 100).expect("add");
        store
            .note_rule_dropped(rule, "nobody would take it")
            .expect("drop");
        let run_id = RunId::from_bytes([9; 16]);
        a_run(&store, run_id);
        store.note_rule_fired(rule, run_id, 300).expect("fired");
        assert_eq!(store.rules().expect("list")[0].last_error, None);
    }

    /// The same lesson `resolve_run` learned, in the place it would delete a standing
    /// instruction rather than cancel a job: `LIKE` reads `%` and `_` as wildcards, and a single
    /// match here *resolves* rather than refusing.
    #[test]
    fn a_rule_prefix_is_a_prefix_and_not_a_pattern() {
        let store = Store::open_memory().expect("open");
        let id = RuleId::from_bytes([0xab, 0xcd, 1, 2, 3, 4, 5, 6]);
        store.add_rule(id, "email", "{}", 100).expect("add");

        assert_eq!(store.resolve_rule("abcd").expect("prefix"), id);
        assert_eq!(store.resolve_rule(&id.to_string()).expect("whole"), id);
        assert!(
            store.resolve_rule("%").is_err(),
            "a wildcard must not resolve to the only rule in the store"
        );
        assert!(
            store.resolve_rule("ab_d").is_err(),
            "nor must an underscore stand in for a character the id does not have"
        );
        assert!(store.resolve_rule("").is_err());
    }

    #[test]
    fn an_ambiguous_prefix_is_refused_rather_than_guessed() {
        let store = Store::open_memory().expect("open");
        store
            .add_rule(
                RuleId::from_bytes([0xaa, 1, 0, 0, 0, 0, 0, 0]),
                "email",
                "{}",
                100,
            )
            .expect("add");
        store
            .add_rule(
                RuleId::from_bytes([0xaa, 2, 0, 0, 0, 0, 0, 0]),
                "email",
                "{}",
                101,
            )
            .expect("add");
        assert!(store.resolve_rule("aa").is_err());
        assert!(store.resolve_rule("aa01").is_ok());
    }

    /// Complete a run and tag it as `rule`'s occurrence, returning its last event's sequence.
    fn an_occurrence(store: &Store, rule: RuleId, id: RunId) -> u64 {
        let mut run = a_run(store, id);
        run.state = offload_core::RunState::Completed { at: Millis(500) };
        // A rule fired it, which is the fact the query keys on now (ADR-0024) — and the reason
        // a peer can ask the same question about a rule it has never heard of.
        run.origin = offload_core::Origin::Rule;
        store.save_run(&run).expect("save");
        assert!(store.tag_occurrence(id, rule).expect("tag"));
        store
            .append_event(id, "finished", 500, &"done")
            .expect("event")
            .unsigned_abs()
    }

    /// The load-bearing claim in ADR-0021 §1, and the one nothing in the type system protects.
    ///
    /// `write_run`'s `ON CONFLICT DO UPDATE` names the columns a re-write overwrites, and `rule`
    /// is not one of them — which is the only reason a gossip merge writing a peer's copy of the
    /// record back does not untag it. Adding the column to that list would strand every
    /// occurrence the fleet is still talking about, silently, with the symptom appearing weeks
    /// later as a disk that does not stop growing.
    #[test]
    fn a_gossip_rewrite_does_not_untag_an_occurrence() {
        let store = Store::open_memory().expect("open");
        let rule = RuleId::from_bytes([5; 8]);
        let id = RunId::from_bytes([21; 16]);
        an_occurrence(&store, rule, id);
        assert_eq!(store.occurrences_kept(rule).expect("count"), 1);

        let run = store.load_run(id).expect("load").expect("there");
        store
            .save_run(&run)
            .expect("write it back, as `absorb` does");

        assert_eq!(
            store.occurrences_kept(rule).expect("count"),
            1,
            "a whole-row write must leave the tag alone"
        );
    }

    /// Every clause of ADR-0021's "spent", one at a time, against one rule.
    ///
    /// Written as one test because the point is the *conjunction*: each clause on its own is a
    /// reason to keep a record, and a fixture that satisfied three of them would pass a test for
    /// the fourth by accident.
    #[test]
    fn an_occurrence_is_spent_only_when_every_clause_holds() {
        let store = Store::open_memory().expect("open");
        let rule = RuleId::from_bytes([6; 8]);
        // `written_before_ms` is the cutoff a row's `updated_at_ms` has to be *under*. These
        // rows are written now, so the whole of time is "quiet" and the epoch is "still noisy".
        let quiet = i64::MAX;
        let noisy = 0i64;

        let plain = RunId::from_bytes([31; 16]);
        let seq = an_occurrence(&store, rule, plain);

        assert_eq!(
            store
                .spent_occurrences(Some(rule), None, quiet, u64::MAX)
                .expect("look"),
            vec![plain],
            "completed, quiet, nothing owed, nothing left to scan"
        );
        assert!(
            store
                .spent_occurrences(Some(rule), Some(plain), quiet, u64::MAX)
                .expect("look")
                .is_empty(),
            "the occurrence the caller has just recorded is never a candidate"
        );
        assert!(
            store
                .spent_occurrences(Some(rule), None, noisy, u64::MAX)
                .expect("look")
                .is_empty(),
            "a record the fleet may still be gossiping is one a peer would teach straight back"
        );
        assert!(
            store
                .spent_occurrences(Some(rule), None, quiet, seq - 1)
                .expect("look")
                .is_empty(),
            "an event no route has scanned yet is news with nothing to point at"
        );

        store
            .notice_delivery(
                "phone",
                offload_core::notify::Topic::Run,
                seq,
                Some(plain),
                Millis(600),
            )
            .expect("notice");
        assert!(
            store
                .spent_occurrences(Some(rule), None, quiet, u64::MAX)
                .expect("look")
                .is_empty(),
            "an outbox row that has not been sent is news still owed"
        );
        store
            .delivery_abandoned(
                "phone",
                offload_core::notify::Topic::Run,
                seq,
                Millis(700),
                "the route is gone",
            )
            .expect("abandon");
        assert_eq!(
            store
                .spent_occurrences(Some(rule), None, quiet, u64::MAX)
                .expect("look"),
            vec![plain],
            "giving up on a route is a decision already taken, not news in flight"
        );

        // A failure is the record somebody comes back for, whatever the plane has finished with.
        let failed = RunId::from_bytes([32; 16]);
        let mut run = a_run(&store, failed);
        run.state = offload_core::RunState::Failed {
            at: Millis(500),
            reason: "the build".into(),
        };
        run.origin = offload_core::Origin::Rule;
        store.save_run(&run).expect("save");
        assert!(store.tag_occurrence(failed, rule).expect("tag"));
        assert_eq!(
            store
                .spent_occurrences(Some(rule), None, quiet, u64::MAX)
                .expect("look"),
            vec![plain],
            "the notification said it failed; only the log says why"
        );
        assert_eq!(store.occurrences_kept(rule).expect("count"), 2);
    }

    /// Pruning takes the run's events and its resolved outbox rows with it — and leaves the
    /// rule, which outlives what it fires.
    #[test]
    fn pruning_an_occurrence_takes_its_log_and_leaves_the_rule() {
        let store = Store::open_memory().expect("open");
        let rule = RuleId::from_bytes([7; 8]);
        store.add_rule(rule, "schedule", "{}", 100).expect("add");
        let id = RunId::from_bytes([41; 16]);
        let seq = an_occurrence(&store, rule, id);
        store
            .notice_delivery(
                "phone",
                offload_core::notify::Topic::Run,
                seq,
                Some(id),
                Millis(600),
            )
            .expect("notice");
        store
            .delivery_sent("phone", offload_core::notify::Topic::Run, seq, Millis(700))
            .expect("sent");

        store.delete_run(id).expect("prune");

        assert!(store.load_run(id).expect("load").is_none());
        assert_eq!(store.occurrences_kept(rule).expect("count"), 0);
        assert!(
            store.event_at(seq).expect("event").is_none(),
            "the events go with the row: a record with no log is a run reporting it said nothing"
        );
        assert_eq!(
            store.rules().expect("list").len(),
            1,
            "the rule is not one of its own occurrences"
        );
    }

    #[test]
    fn forgetting_a_rule_says_whether_there_was_one() {
        let store = Store::open_memory().expect("open");
        let id = RuleId::from_bytes([4; 8]);
        store.add_rule(id, "email", "{}", 100).expect("add");
        assert!(store.remove_rule(id).expect("remove"));
        assert!(
            !store.remove_rule(id).expect("remove"),
            "removing what is not there is not an error, and it is not a removal either"
        );
    }
}
