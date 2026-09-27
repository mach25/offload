//! Database schema and migrations.
//!
//! Migrations are an append-only list applied in order, tracked by SQLite's `user_version`.
//! Never edit a migration that has shipped — nodes upgrade at different times, and phase 3
//! makes that a certainty rather than a possibility, so an edited migration means two nodes
//! disagreeing about what their schema means.

use rusqlite::Connection;

/// Applied in order. Index + 1 is the resulting `user_version`.
pub const MIGRATIONS: &[&str] = &[
    // v1 — the state phase 1 kept in memory, plus what phase 3 will need.
    r#"
    CREATE TABLE runs (
        id            BLOB PRIMARY KEY NOT NULL,
        -- Denormalised out of run_json so the common queries are indexable. The JSON
        -- stays authoritative; these are a projection of it.
        state         TEXT    NOT NULL,
        terminal      INTEGER NOT NULL,
        epoch         INTEGER NOT NULL,
        home_node     BLOB    NOT NULL,
        holder_node   BLOB,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        run_json      TEXT    NOT NULL,
        -- Node-local operational facts. Not part of the domain object: `offload_core::Run`
        -- describes what a run *is*, and these describe what hosting it cost us. Written
        -- by save_stats, deliberately untouched by save_run.
        turns          INTEGER NOT NULL DEFAULT 0,
        denials        INTEGER NOT NULL DEFAULT 0,
        cost_micro_usd INTEGER NOT NULL DEFAULT 0,
        workspace      TEXT    NOT NULL DEFAULT ''
    ) STRICT;

    CREATE INDEX runs_active   ON runs (terminal, created_at_ms DESC);
    CREATE INDEX runs_by_holder ON runs (holder_node) WHERE holder_node IS NOT NULL;

    -- Monotonic `seq` is what lets `offload logs --follow` resume after a reconnect:
    -- "everything after N" is a range scan, not a re-read of the whole log.
    CREATE TABLE run_events (
        seq        INTEGER PRIMARY KEY AUTOINCREMENT,
        run_id     BLOB    NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
        at_unix_ms INTEGER NOT NULL,
        kind       TEXT    NOT NULL,
        event_json TEXT    NOT NULL
    ) STRICT;

    CREATE INDEX run_events_by_run ON run_events (run_id, seq);

    -- The index only. Bytes live on disk under <state>/blobs/, because a git bundle can
    -- be tens of megabytes and SQLite is not where that belongs.
    CREATE TABLE blobs (
        hash          BLOB PRIMARY KEY NOT NULL,
        size_bytes    INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL,
        last_used_ms  INTEGER NOT NULL
    ) STRICT;

    CREATE INDEX blobs_by_last_used ON blobs (last_used_ms);

    -- Survives restarts on purpose: the drop-off policy learns how long a node is
    -- typically away, and in phase 1 that learning reset every time the daemon
    -- restarted, which is exactly when a node has been away.
    CREATE TABLE node_observations (
        node_id           BLOB PRIMARY KEY NOT NULL,
        status            TEXT    NOT NULL,
        absences          INTEGER NOT NULL,
        typical_absence_ms INTEGER,
        absent_since_ms   INTEGER,
        last_seen_ms      INTEGER NOT NULL
    ) STRICT;
    "#,
    // v2 — when a run's numbers were last written by the node that produced them.
    //
    // Progress travels now (`offload_core::progress`), and two records claiming the same
    // turns and the same cost can still differ: the worktree summary changes without any
    // counter moving. This is the tiebreak, and it is the *author's* clock rather than the
    // receiver's — a node relaying somebody else's numbers must not be able to make them look
    // fresher than the newer ones they would overwrite.
    r#"
    ALTER TABLE runs ADD COLUMN progress_at_ms INTEGER NOT NULL DEFAULT 0;
    "#,
    // v3 — the delivery plane's memory (ADR-0010).
    //
    // Two tables because they answer two questions, and collapsing them is how a broken sink
    // starts swallowing the notifications behind it:
    //
    // * `sink_cursors` bounds the *scan*. It is the highest event sequence a sink has been
    //   shown, and it is set to the current end of the log the first time a sink is seen — so
    //   configuring a route today does not replay a month of finished runs at somebody.
    // * `deliveries` is the outbox, one row per `(sink, seq)`, which is ADR-0010's dedup
    //   identity with the run id carried along for tracing. A row is created when the event is
    //   *noticed* and updated when it is sent, so a sink that is down accumulates rows to retry
    //   rather than blocking the cursor behind it. `delivered_at_ms IS NULL` is the queue.
    //
    // The primary key is what makes delivery at-least-once rather than at-most-once: an insert
    // that loses the race is a no-op, and nothing is ever sent twice by *this* node. A
    // duplicate can still reach a person across two nodes, which the ADR accepts by name.
    r#"
    CREATE TABLE sink_cursors (
        sink    TEXT PRIMARY KEY NOT NULL,
        seq     INTEGER NOT NULL,
        seen_at_ms INTEGER NOT NULL
    ) STRICT;

    CREATE TABLE deliveries (
        sink            TEXT    NOT NULL,
        seq             INTEGER NOT NULL,
        run_id          BLOB    NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
        noticed_at_ms   INTEGER NOT NULL,
        delivered_at_ms INTEGER,
        attempts        INTEGER NOT NULL DEFAULT 0,
        last_error      TEXT,
        PRIMARY KEY (sink, seq)
    ) STRICT;

    CREATE INDEX deliveries_pending ON deliveries (delivered_at_ms, seq) WHERE delivered_at_ms IS NULL;
    "#,
    // v4 — how many questions a run has put to a person (ADR-0017).
    //
    // Beside the denial count and for the same reason: a run whose ask budget is spent has its
    // remaining calls decided by the agent's own rules, so the two numbers together are what
    // says whether a run did less than it was asked and why. It is `RunProgress`, so it travels
    // and is arbitrated forward-only — which is also why it cannot live in the holder's own
    // registry, where a migration would hand the new node a fresh budget.
    r#"
    ALTER TABLE runs ADD COLUMN asks INTEGER NOT NULL DEFAULT 0;
    "#,
    // v5 — news that is not about a run (ADR-0012 mitigation 4).
    //
    // A device joining the fleet, and the passphrase coming out of the drawer. The delivery
    // plane could only address runs, and the outbox said so in its schema: `run_id` was
    // `NOT NULL REFERENCES runs(id)`, and the sink cursor was a position in one log.
    //
    // Two logs rather than one table with a nullable run, because they are two logs: separate
    // sequences, separate scans, and a fleet event has none of a run event's shape. What they
    // share is everything downstream — one outbox, one cursor table, one retry policy — which
    // is why `topic` joins the keys instead of a second copy of all of it. Two logs sharing an
    // integer with nothing to distinguish them is how a fleet notification would suppress a
    // run's.
    //
    // The two rebuilds are a rebuild and not an edit: SQLite cannot drop a NOT NULL or widen a
    // primary key in place, and editing a shipped migration is the one thing the digest test
    // exists to prevent.
    r#"
    CREATE TABLE fleet_events (
        seq        INTEGER PRIMARY KEY AUTOINCREMENT,
        at_unix_ms INTEGER NOT NULL,
        kind       TEXT    NOT NULL,
        -- The device an event is about, hex, denormalised out of the JSON so that "have I
        -- already announced this one" is an index lookup rather than a `LIKE` over a blob of
        -- JSON that happens to spell node ids as arrays of integers.
        subject    TEXT    NOT NULL,
        event_json TEXT    NOT NULL
    ) STRICT;

    CREATE INDEX fleet_events_by_kind    ON fleet_events (kind, seq);
    CREATE INDEX fleet_events_by_subject ON fleet_events (subject, kind);

    CREATE TABLE sink_cursors_v5 (
        sink       TEXT    NOT NULL,
        topic      TEXT    NOT NULL,
        seq        INTEGER NOT NULL,
        seen_at_ms INTEGER NOT NULL,
        PRIMARY KEY (sink, topic)
    ) STRICT;

    INSERT INTO sink_cursors_v5 (sink, topic, seq, seen_at_ms)
        SELECT sink, 'run', seq, seen_at_ms FROM sink_cursors;
    DROP TABLE sink_cursors;
    ALTER TABLE sink_cursors_v5 RENAME TO sink_cursors;

    CREATE TABLE deliveries_v5 (
        sink            TEXT    NOT NULL,
        topic           TEXT    NOT NULL,
        seq             INTEGER NOT NULL,
        -- Null for a fleet event, which is about the membership rather than about any run.
        run_id          BLOB    REFERENCES runs(id) ON DELETE CASCADE,
        noticed_at_ms   INTEGER NOT NULL,
        delivered_at_ms INTEGER,
        attempts        INTEGER NOT NULL DEFAULT 0,
        last_error      TEXT,
        PRIMARY KEY (sink, topic, seq)
    ) STRICT;

    INSERT INTO deliveries_v5
        (sink, topic, seq, run_id, noticed_at_ms, delivered_at_ms, attempts, last_error)
        SELECT sink, 'run', seq, run_id, noticed_at_ms, delivered_at_ms, attempts, last_error
        FROM deliveries;
    DROP TABLE deliveries;
    ALTER TABLE deliveries_v5 RENAME TO deliveries;

    CREATE INDEX deliveries_pending ON deliveries (delivered_at_ms, seq) WHERE delivered_at_ms IS NULL;
    "#,
    // v6: the audit log. Every grant this node made or took, and every write it was refused.
    //
    // A third log, and the reason it is not one of the other two is the reason `fleet_events`
    // is not `run_events`. A run's log is the run's *output* — the agent's text, its turns, what
    // a checkpoint captured — read by anybody following it, and served from whichever node holds
    // the run. A leg that has just been *fenced out* is by definition not that node, so a line it
    // wrote there would be a line nobody ever reads. And this is not about the fleet either.
    //
    // What it is: what *this machine* decided and what it was refused. Per-node, never gossiped,
    // for `fleet_events`' reason — two nodes recording the same grant are not disagreeing, they
    // are each describing what they saw, and merging them would mean deciding which observation
    // is authoritative about an event neither owns (ADR-0005's table, read the other way).
    //
    // `run_id` is a plain BLOB with no foreign key on purpose: the most valuable row in here is
    // about a run this node has already stopped holding, and `runs` rows are removed. An audit
    // trail that cascaded away with the thing it audits would be missing exactly the entries
    // somebody goes looking for.
    r#"
    CREATE TABLE audit (
        seq        INTEGER PRIMARY KEY AUTOINCREMENT,
        at_unix_ms INTEGER NOT NULL,
        kind       TEXT    NOT NULL,
        run_id     BLOB    NOT NULL,
        event_json TEXT    NOT NULL
    ) STRICT;

    CREATE INDEX audit_by_run ON audit (run_id, seq);
    "#,
    // v7: which leg a run's numbers came from.
    //
    // The stats columns beside a run are its *position* (what turn it is on, what its worktree
    // holds) and its *spend* (money, denials, questions), and they were merged by one rule —
    // forward only, justified by the numbers having a single author by construction. A second
    // grant is what breaks that construction, and ADR-0006's silence-is-a-decline makes one
    // ordinary: two legs of one run publish two positions, forward-only keeps the larger, and the
    // larger is the *loser's* whenever the loser got further. The surviving leg can then never
    // correct it, because a smaller number is refused for ever.
    //
    // So a position now says whose it is, and is ranked the way the run record beside it is
    // ranked — highest epoch, lowest author id at a tie. Spend stays forward-only, deliberately,
    // because the money left the account whichever leg spent it.
    //
    // Nullable and defaulted rather than backfilled: a row written before this cannot be
    // attributed, and `RunProgress` reads that as an unstamped record which loses to any stamped
    // one. Backfilling with this node's own id would be inventing an author for numbers that may
    // have come from a peer.
    r#"
    ALTER TABLE runs ADD COLUMN progress_by    BLOB;
    ALTER TABLE runs ADD COLUMN progress_epoch INTEGER NOT NULL DEFAULT 0;
    "#,
    // v8 — standing instructions: when a trigger fires here, submit this run (ADR-0020).
    //
    // Node-local and never gossiped, which is the whole reason this half of ADR-0019 is the
    // small one. A trigger is a program *this node's owner* nominated, so it fires on exactly
    // one machine and there is no second node to disagree with about an occurrence — none of
    // the derived-`RunId` machinery a gossiped schedule would have needed.
    //
    // `service` rather than a trigger id, for the reason an `Audience` names services: an id is
    // a node's name for one of its own things, and a rule that named one would be pinned to a
    // particular watcher rather than to what the owner said it watches.
    //
    // `request_json` is opaque here on purpose. It is a whole `SubmitRequest`, which lives in
    // `offload-node` — a layer this crate cannot see and must not — and keeping it as text is
    // the same shape `run_json` already has: the daemon owns the meaning, the store owns the row.
    //
    // `last_run` is a plain BLOB with **no** foreign key. A rule outlives the runs it fires, and
    // an ON DELETE CASCADE here would delete somebody's standing instruction when the run it
    // last fired was cleaned up — the audit table's reasoning, in a place where the consequence
    // is worse than a missing row.
    //
    // `fired` and `dropped` are two counters because they are two facts, and the second is the
    // only symptom its failure has: a rule whose runs are slower than its trigger's cadence is
    // correct, silent, and visible only here.
    r#"
    CREATE TABLE rules (
        id            BLOB PRIMARY KEY NOT NULL,
        service       TEXT    NOT NULL,
        request_json  TEXT    NOT NULL,
        created_at_ms INTEGER NOT NULL,
        fired         INTEGER NOT NULL DEFAULT 0,
        dropped       INTEGER NOT NULL DEFAULT 0,
        last_fired_ms INTEGER,
        last_run      BLOB,
        last_error    TEXT
    ) STRICT;

    CREATE INDEX rules_by_service ON rules (service);
    "#,
    // v9 — which rule fired an occurrence (ADR-0021).
    //
    // Node-local for the reason a rule is: a rule lives on the machine whose trigger fires it
    // and is never gossiped, so "which rule fired this run" is that machine's own bookkeeping.
    // On the row rather than in `run_json` for the same reason — a field in the JSON travels,
    // and a travelling field needs an owner and a merge rule for a fact no peer can state.
    //
    // What it buys is the thing a single `last_run` pointer could not: every bound on pruning a
    // record is a *temporary* no — a phone asleep for one tick, a delivery pass that has not run,
    // a record the fleet is still gossiping — and a prune that gets one chance per record turns
    // each of them into a permanent one. With this, a firing re-asks about every occurrence it
    // has ever made.
    //
    // **`write_run`'s `ON CONFLICT DO UPDATE` list is load-bearing here.** It names the columns a
    // re-write overwrites and this is not one of them, which is what lets a gossip merge write
    // the row back without untagging it. Adding `rule` to that list would silently strand every
    // occurrence the fleet is still talking about.
    r#"
    ALTER TABLE runs ADD COLUMN rule BLOB;

    CREATE INDEX runs_by_rule ON runs (rule) WHERE rule IS NOT NULL;
    "#,
    // v10 — what kind of thing started a run (ADR-0024), denormalised out of `run_json`.
    //
    // The opposite of v9's `rule` in the one way that matters. `rule` is this node's own
    // bookkeeping and must **not** be in `write_run`'s `ON CONFLICT DO UPDATE` list, because a
    // gossip merge writing the row back would erase it. `origin` comes *from the record* and
    // must be in that list, because a peer learning a run for the first time is exactly how this
    // column gets its value on the machine that needs it most.
    //
    // Denormalised rather than read from the JSON for the reason `fleet_events.subject` is: the
    // sweep that reads it wants an indexed scan over a table that, on a node hosting somebody
    // else's occurrences, is the largest one here. And never matched as text — a run's JSON
    // spells its ids as arrays of integers, which is the mistake that made the blob collector
    // delete live checkpoints.
    //
    // `0` is `Operator`, and it is the default in both senses: rows written before this, and
    // records from a build that has never heard of the field. It is the value that keeps things.
    r#"
    ALTER TABLE runs ADD COLUMN machine_started INTEGER NOT NULL DEFAULT 0;

    CREATE INDEX runs_machine_started ON runs (machine_started, terminal)
        WHERE machine_started = 1;
    "#,
    // v11 — what the conversation has consumed, in tokens (ADR-0040).
    //
    // Four columns rather than one total, because the counters are not interchangeable: a cache
    // read is billed at a fraction of an input token and a cache write at more than one, so a
    // single number could never be converted into anything later, even by somebody holding a
    // price list. Deliberately *not* a cost column — ADR-0040 §1 is why this crate stores tokens
    // and never dollars.
    //
    // Beside `turns` and `workspace` rather than beside `cost_micro_usd`, which is the
    // distinction that matters here and is easy to get backwards. These are **position**: they
    // are derived from the transcript the surviving leg holds, so they travel with the leg the
    // record settles on and *replace*. `cost_micro_usd` is per-leg spend and accumulates. Two
    // adjacent numbers, opposite merge rules (`RunProgress::absorb`).
    //
    // `0` for every existing row and for a record from a build that has never heard of them,
    // which reads as "no checkpoint has been taken yet" — the honest answer, since the transcript
    // is only read when one is captured.
    r#"
    ALTER TABLE runs ADD COLUMN tok_input          INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE runs ADD COLUMN tok_output         INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE runs ADD COLUMN tok_cache_creation INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE runs ADD COLUMN tok_cache_read     INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE runs ADD COLUMN tok_messages       INTEGER NOT NULL DEFAULT 0;
    "#,
    // v12 — standing instructions on a clock (ADR-0019 §3, ADR-0056).
    //
    // The first table here whose rows are **gossiped**, and the shape follows from that. A rule
    // (v8) is one machine's own bookkeeping, so its row is written by one writer and read by one
    // reader; a schedule outlives the device that created it, so a peer holds copies of other
    // nodes' schedules and fires them when their home is away.
    //
    // Three consequences visible in these columns:
    //
    // * `home` is stored rather than implied. A node cannot tell its own schedules from the ones
    //   it has learned without it, and *who fires this* is a question about the home node
    //   (`ClusterView::steward_of`).
    // * `removed_at_ms` is the tombstone (ADR-0056 §5), and it is why removal is an UPDATE
    //   rather than a DELETE. A row deleted here comes straight back from the next peer that
    //   gossips it, which is the standing entry in `docs/pitfalls/rules-and-retention.md`
    //   reached from the other end.
    // * `spec_json` is a whole `RunSpec` as text, for `run_json`'s reason one layer along: the
    //   store owns the row and the daemon owns the meaning. Unlike a rule's `request_json` this
    //   one is a **domain** type rather than a control-protocol one, because a peer that never
    //   saw the creating node's config still has to be able to build the run (ADR-0056 §3).
    //
    // `last_tick_ms` is **node-local and never gossiped**, which is `runs.rule`'s arrangement
    // exactly and for a related reason: it is this node's memory of what *it* did, and no peer
    // can state it. It is not in the `ON CONFLICT DO UPDATE` list below, so a gossip merge
    // writing the row back cannot erase it.
    //
    // Why it exists at all, when the occurrence's own record already says a tick fired: because
    // that record is **deleted** in the ordinary course of things. A scheduled occurrence is
    // machine-started (ADR-0024), so `prune_spent_records` reclaims it about an hour after it
    // finishes — which for a daily schedule is twenty-three hours *before* its tick ends. With
    // the record as the only evidence, that schedule fires roughly hourly. The high-water mark
    // is what the firing node remembers; the record is what stops a *second* node from firing a
    // tick this one already served. Two checks, two failures, and neither covers the other.
    r#"
    CREATE TABLE schedules (
        id            BLOB PRIMARY KEY NOT NULL,
        home          BLOB    NOT NULL,
        every_ms      INTEGER NOT NULL,
        offset_ms     INTEGER NOT NULL,
        spec_json     TEXT    NOT NULL,
        note          TEXT    NOT NULL DEFAULT '',
        created_at_ms INTEGER NOT NULL,
        removed_at_ms INTEGER,
        last_tick_ms  INTEGER
    ) STRICT;

    CREATE INDEX schedules_live ON schedules (home) WHERE removed_at_ms IS NULL;
    "#,
    // v13 — what each leg of a run spent (ADR-0067).
    //
    // A list, as JSON, for `run_json`'s reason: the store keeps the row and the core owns the
    // meaning, and the list's length is one per grant the run has ever had. `[]` for every existing
    // row, which is the old behaviour — the position's tokens and nothing beside them.
    r#"
    ALTER TABLE runs ADD COLUMN legs_json TEXT NOT NULL DEFAULT '[]';
    "#,
];

/// Bring a connection up to the current schema version.
pub fn migrate(conn: &Connection) -> rusqlite::Result<u32> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = u32::try_from(index + 1).unwrap_or(u32::MAX);
        tracing::info!(version, "applying schema migration");
        // Each migration plus its version bump is one transaction: a crash halfway
        // through must not leave a database that claims a version it does not have.
        conn.execute_batch(&format!(
            "BEGIN; {sql} PRAGMA user_version = {version}; COMMIT;"
        ))?;
    }

    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Connection settings applied on every open.
pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    // WAL: readers don't block the writer, and the durability semantics under power loss
    // are the well-documented ones.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // NORMAL rather than FULL: with WAL this survives process crashes, and loses at most
    // the last transactions on OS crash. For a run registry that is the right trade —
    // FULL costs an fsync per commit and the agent's own work is the durable artefact.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Wait rather than immediately returning SQLITE_BUSY. One daemon writes, but `offload`
    // may read concurrently and a spurious busy error would surface as a CLI failure.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrating_an_empty_database_reaches_the_current_version() {
        let conn = Connection::open_in_memory().expect("open");
        configure(&conn).expect("configure");
        let version = migrate(&conn).expect("migrate");
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        // Every daemon start runs this. It must be idempotent or the second start fails
        // with "table already exists".
        let conn = Connection::open_in_memory().expect("open");
        configure(&conn).expect("configure");
        let first = migrate(&conn).expect("first");
        let second = migrate(&conn).expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn the_expected_tables_exist() {
        let conn = Connection::open_in_memory().expect("open");
        configure(&conn).expect("configure");
        migrate(&conn).expect("migrate");

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .expect("prepare");
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .expect("query")
            .filter_map(Result::ok)
            .filter(|t: &String| !t.starts_with("sqlite_"))
            .collect();

        assert_eq!(
            tables,
            vec![
                "audit",
                "blobs",
                "deliveries",
                "fleet_events",
                "node_observations",
                "rules",
                "run_events",
                "runs",
                "schedules",
                "sink_cursors"
            ]
        );
    }

    #[test]
    fn a_failed_migration_does_not_bump_the_version() {
        // The property that makes an append-only migration list safe: a crash partway
        // through must leave a database that still knows it is out of date.
        let conn = Connection::open_in_memory().expect("open");
        configure(&conn).expect("configure");
        conn.execute_batch("BEGIN; CREATE TABLE runs (x INTEGER); COMMIT;")
            .expect("conflicting table");

        assert!(
            migrate(&conn).is_err(),
            "should collide with existing table"
        );
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(
            version, 0,
            "version must not advance past a failed migration"
        );
    }
}

#[cfg(test)]
mod pinned {
    use super::MIGRATIONS;

    /// Every migration that has shipped, by content.
    ///
    /// The one house rule in this file that nothing else could enforce. Nodes upgrade at
    /// different times, so editing a migration that has shipped means two builds disagreeing
    /// about the schema of a database they both write — and the disagreement is silent, because
    /// `user_version` reads the same number on both. There is no error to notice.
    ///
    /// So this is a known-answer test, the same shape as the ones pinning the fleet key
    /// derivation and the account fingerprint, and for the same reason: the value is a contract
    /// with other processes rather than an implementation detail. **Adding** a migration adds a
    /// digest at the end. **Changing** one fails here, which is the entire point, and the fix is
    /// never to update the number.
    ///
    /// FNV-1a rather than a real hash, because the adversary is a text editor. What this has to
    /// catch is somebody tidying shipped SQL, and any digest catches that.
    const SHIPPED: &[u64] = &[
        0xdbbfb989e70b75ec,
        0x0e4872170457cf98,
        0xb19c23c97e011dfa,
        0xed3fee90986effbc,
        0x389abf37f931df61,
        0xaba48a7dbf0afc18,
        0x8863147effc850b5,
        0x0c17d46b15a6b07d,
        0x09f07eca389f7043,
        0xdf7676b880a9311b,
        0xb36af5bac965070b,
        0x506288897adc699d,
        0xfe5d34945656a1ff,
    ];

    fn digest(sql: &str) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in sql.as_bytes() {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    #[test]
    fn a_shipped_migration_is_never_edited() {
        assert!(
            MIGRATIONS.len() >= SHIPPED.len(),
            "a migration was removed. Nodes that already applied it sit at a user_version this \
             build no longer has, and nothing will tell them."
        );
        for (i, (sql, pinned)) in MIGRATIONS.iter().zip(SHIPPED).enumerate() {
            assert_eq!(
                digest(sql),
                *pinned,
                "migration v{} has been edited. Nodes upgrade at different times, so this is two \
                 builds disagreeing about a schema while `user_version` says they agree. Add a \
                 new migration instead; do not update the digest.",
                i + 1
            );
        }
    }

    #[test]
    fn a_new_migration_is_pinned_before_it_ships() {
        assert_eq!(
            MIGRATIONS.len(),
            SHIPPED.len(),
            "migration v{} has no pinned digest. Add one — an unpinned migration is one somebody \
             may quietly edit later, which is the failure above exists to prevent.",
            SHIPPED.len() + 1
        );
    }
}
