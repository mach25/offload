//! The device-local capacity reservation ledger.
//!
//! ADR-0012 makes multi-fleet membership *one `offloadd` per fleet*, each with its own
//! `OFFLOAD_STATE_DIR` and its own identity. That is the right isolation and it has one
//! arithmetic consequence: neither instance can see what the other accepted, so a laptop
//! configured to host two runs per fleet cheerfully hosts four. The device's budget is a fact
//! about the hardware, and the hardware does not care how many fleets are asking.
//!
//! So ADR-0013 puts the reservations somewhere the *machine* owns: a small ledger under a
//! well-known per-user path, holding `{instance, run, demand, expires_at}`. Every instance
//! consults it before bidding and updates it on accept and release. It is **not** a scheduler,
//! a daemon, or a cross-fleet channel — it decides nothing and it talks to nobody. Reading it
//! answers exactly one question: *how much of this machine has already been promised?*
//!
//! **What crosses fleets is the number, never the work.** A row carries two opaque ids and a
//! coarse label, which is why [`Broker::reserve`] takes no spec: there is no repository here,
//! no prompt, no fleet name, and no way for one instance to learn anything about the other's
//! contents. The other instance learns that half the budget is gone, and `Refusal::AtCapacity`
//! is reported in terms of this device's own budget — a fact about the hardware. Anything
//! richer would make this the bridge ADR-0012 forbids.
//!
//! **Why SQLite rather than a lock file.** Its locking is genuinely multi-process, which is the
//! whole requirement; this crate is `unsafe_code = "forbid"` so `flock` is not available, and
//! the repo has no file-locking primitive of its own. It is deliberately **not** the node's
//! `state.db`: that database is per-state-dir — so there is one per fleet, which is the problem
//! — and its own docs assert "one daemon writes" twice. This one is written by every instance
//! on the machine, has its own independent `user_version`, and holds nothing a node would miss
//! if it were deleted while nothing was running.

use offload_core::{Demand, Millis, Occupancy, RunId, LEASE};
use rusqlite::Connection;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// How long a reservation stands without being renewed.
///
/// The run lease, because a reservation *is* a lease and a second number would be a second
/// answer to the same question. A crashed instance must not hold the machine's capacity for
/// ever, and the instance that is still alive is renewing its lease anyway — so the heartbeat
/// that keeps the run is the heartbeat that keeps the reservation, and there is nothing extra
/// to remember to do.
pub const RESERVATION_TTL: Millis = LEASE;

/// The ledger's file name under its directory.
const LEDGER_FILE: &str = "reservations.db";

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("reservation ledger database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io error on {path}: {reason}")]
    Io { path: PathBuf, reason: String },
}

/// Applied in order. Index + 1 is the resulting `user_version`.
///
/// Append-only, for the usual reason and then some: the instances sharing this file are
/// *different processes*, upgraded at different times, so an edited migration means two
/// `offloadd` builds disagreeing about the schema of a file they both write.
pub const MIGRATIONS: &[&str] = &[
    // v1 — the ledger. ADR-0013's row, plus the two things arithmetic needs.
    r#"
    CREATE TABLE reservations (
        -- Keyed on the run, which is what makes reserve idempotent: an instance that
        -- accepts, crashes, restarts and reserves again has promised the same thing once,
        -- not twice. Run ids are UUIDv7 and globally unique, so two instances can never
        -- legitimately collide here.
        run_id         BLOB PRIMARY KEY NOT NULL,
        -- Whose promise it is. Used for one thing only — letting an instance subtract its
        -- own rows from the total, because it already knows about those. Callers should
        -- pass something opaque and stable (a node id); this file is readable by the
        -- machine's other instances and a name that identified a *fleet* would leak more
        -- than the number this ledger exists to share.
        instance       TEXT    NOT NULL,
        -- The label, for the human with `sqlite3` open at midnight.
        demand         TEXT    NOT NULL,
        -- And the cost, denormalised out of it on purpose. `Demand::shares` is the
        -- authority, but the instance that wrote the row is the one that read it: a newer
        -- build promising a demand an older build has never heard of would otherwise have
        -- its promise mis-counted by the very peer the ledger exists to inform. The
        -- number the promiser computed travels with the promise.
        shares         INTEGER NOT NULL,
        reserved_at_ms INTEGER NOT NULL,
        expires_at_ms  INTEGER NOT NULL
    ) STRICT;

    CREATE INDEX reservations_by_instance ON reservations (instance);
    CREATE INDEX reservations_live        ON reservations (expires_at_ms);
    "#,
];

/// Where the ledger lives for this user.
///
/// `$XDG_RUNTIME_DIR/offload/` when the platform provides one — it is per-user, already
/// mode 0700, and cleared on logout, which is exactly the lifetime of "what this machine is
/// currently running". Otherwise a per-user directory under the temp dir.
///
/// **Per-user and not per-machine, deliberately.** A ledger shared across users would have to
/// be world-writable, which trades ADR-0012's isolation for a permissions problem and invites
/// a stranger's process to spend this user's budget. The scenario the ADR describes is one
/// person's two fleets on their own laptop, and that is what this covers.
#[must_use]
pub fn default_path() -> PathBuf {
    resolve_path(std::env::var_os("XDG_RUNTIME_DIR"), &current_user())
}

/// The pure half of [`default_path`], so the rule is testable without changing the environment
/// of everything else in the process — the same seam as `offload_agent::transcript::config_dir`.
#[must_use]
pub fn resolve_path(runtime_dir: Option<OsString>, user: &str) -> PathBuf {
    match runtime_dir {
        // An empty variable is somebody unsetting it awkwardly, not a request to put the
        // ledger in the current directory.
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("offload").join(LEDGER_FILE),
        _ => std::env::temp_dir()
            .join(format!("offload-{user}"))
            .join(LEDGER_FILE),
    }
}

/// Something stable that names this user, without `unsafe` and without `libc`.
///
/// `$UID` first because it is what the directory would be called by convention, `$USER` as the
/// name a human recognises. Neither is guaranteed to be exported; the last resort is a shared
/// directory name, and if two users on one machine reach it the second gets a permission error
/// rather than a silently shared ledger — loud in the right direction.
fn current_user() -> String {
    for var in ["UID", "USER", "LOGNAME"] {
        if let Some(value) = std::env::var_os(var) {
            let value = value.to_string_lossy();
            let sanitised: String = value
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !sanitised.is_empty() {
                return sanitised;
            }
        }
    }
    "user".to_string()
}

/// A handle on the machine's reservation ledger.
///
/// Cheap to clone. The mutex is here because `rusqlite::Connection` is `Send` and not `Sync`;
/// it says nothing about the *other processes* on this file, which SQLite serialises itself.
#[derive(Clone)]
pub struct Broker {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl std::fmt::Debug for Broker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Broker").field("path", &self.path).finish()
    }
}

impl Broker {
    /// Open the ledger at [`default_path`], creating it if this is the first instance up.
    pub fn open() -> Result<Broker, BrokerError> {
        Broker::open_at(&default_path())
    }

    /// Open the ledger at an explicit path. `OFFLOAD_STATE_DIR` deliberately does not reach
    /// here — two instances on one machine relocate their state and still share this file —
    /// but tests and a single-machine two-fleet rehearsal both need to say where it is.
    pub fn open_at(path: &Path) -> Result<Broker, BrokerError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| BrokerError::Io {
                path: parent.to_path_buf(),
                reason: e.to_string(),
            })?;
        }
        let conn = Connection::open(path)?;
        configure(&conn)?;
        let version = migrate(&conn)?;
        tracing::debug!(path = %path.display(), version, "opened reservation ledger");
        Ok(Broker {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
        })
    }

    /// In-memory ledger, for tests. Shares nothing with anybody, which is the one thing the
    /// real one is for — so the multi-process property has to be tested on a file.
    pub fn open_memory() -> Result<Broker, BrokerError> {
        let conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&conn)?;
        Ok(Broker {
            conn: Arc::new(Mutex::new(conn)),
            path: PathBuf::from(":memory:"),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Promise `demand` for `run` until `now + ttl`.
    ///
    /// Idempotent on the run: reserving twice is one promise, so a retried accept or a
    /// restarted instance cannot make the machine look busier than it is. A demand that
    /// changed replaces the old one — the last thing the holder said about a run it holds is
    /// the truth about that run.
    pub fn reserve(
        &self,
        instance: &str,
        run: RunId,
        demand: Demand,
        now: Millis,
        ttl: Millis,
    ) -> Result<(), BrokerError> {
        let shares = i64::from(demand.shares());
        self.lock().execute(
            "INSERT INTO reservations
                 (run_id, instance, demand, shares, reserved_at_ms, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(run_id) DO UPDATE SET
                 instance      = excluded.instance,
                 demand        = excluded.demand,
                 shares        = excluded.shares,
                 expires_at_ms = excluded.expires_at_ms",
            rusqlite::params![
                run.as_bytes().as_slice(),
                instance,
                demand.to_string(),
                shares,
                as_i64(now),
                as_i64(now.saturating_add(ttl)),
            ],
        )?;
        tracing::debug!(instance, run_id = %run, %demand, shares, "reserved device capacity");
        Ok(())
    }

    /// Push the expiry out. Returns whether there was still a row to push.
    ///
    /// `false` means this instance's reservation has already been swept — it was away longer
    /// than a lease — and the caller should [`Broker::reserve`] again rather than assume the
    /// machine is still holding space for a run it is still running.
    ///
    /// Scoped to the caller's own rows: an instance may renew what it promised and nothing
    /// else. One fleet's daemon must not be able to extend another's promise.
    pub fn renew(
        &self,
        instance: &str,
        run: RunId,
        now: Millis,
        ttl: Millis,
    ) -> Result<bool, BrokerError> {
        let changed = self.lock().execute(
            "UPDATE reservations SET expires_at_ms = ?3
             WHERE run_id = ?1 AND instance = ?2",
            rusqlite::params![
                run.as_bytes().as_slice(),
                instance,
                as_i64(now.saturating_add(ttl)),
            ],
        )?;
        if changed == 0 {
            tracing::debug!(
                instance,
                run_id = %run,
                "renewed a reservation that was no longer there"
            );
        }
        Ok(changed > 0)
    }

    /// Give the capacity back. Returns whether this instance was holding it.
    ///
    /// Same scoping as [`Broker::renew`], for the stronger version of the same reason: an
    /// instance that could release another's reservation could make the machine look idle
    /// while somebody else's agent was mid-turn on it.
    pub fn release(&self, instance: &str, run: RunId) -> Result<bool, BrokerError> {
        let removed = self.lock().execute(
            "DELETE FROM reservations WHERE run_id = ?1 AND instance = ?2",
            rusqlite::params![run.as_bytes().as_slice(), instance],
        )?;
        if removed > 0 {
            tracing::debug!(instance, run_id = %run, "released device capacity");
        }
        Ok(removed > 0)
    }

    /// What the whole machine has promised, across every instance on it.
    pub fn committed(&self, now: Millis) -> Result<Occupancy, BrokerError> {
        self.total(now, None)
    }

    /// The same total with this instance's own rows left out.
    ///
    /// This is the number a node actually wants: it already knows what it holds, from its own
    /// store and with more detail than a ledger row carries. Adding the two is how a node
    /// counts its own runs twice and refuses work it has room for.
    pub fn committed_elsewhere(
        &self,
        instance: &str,
        now: Millis,
    ) -> Result<Occupancy, BrokerError> {
        self.total(now, Some(instance))
    }

    /// The runs this instance currently has promised the machine.
    ///
    /// For reconciliation: the ledger is authoritative about what was promised and the store is
    /// authoritative about what is actually held, and the two drift for ordinary reasons — a run
    /// finishes, a run migrates away, a daemon is killed and restarts to find its own rows from
    /// a previous life still sitting there. Live rows only, so a row this instance abandoned
    /// long enough ago to expire is the sweeper's business rather than something to release
    /// twice.
    pub fn mine(&self, instance: &str, now: Millis) -> Result<Vec<RunId>, BrokerError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT run_id FROM reservations WHERE instance = ?2 AND {LIVE}"
        ))?;
        let rows = stmt.query_map(rusqlite::params![as_i64(now), instance], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            let bytes = row?;
            // A row whose id is not 16 bytes is not one of ours to reason about. Skipped rather
            // than guessed at: this file is shared, and a future build may write things this one
            // does not understand.
            if let Ok(id) = <[u8; 16]>::try_from(bytes.as_slice()) {
                out.push(RunId::from_bytes(id));
            }
        }
        Ok(out)
    }

    /// Delete reservations that have expired. Returns how many went.
    ///
    /// The exact complement of the liveness test `committed` applies — see [`LIVE`]. A row for
    /// a dead instance is precisely what this is for; a row for a live instance is being
    /// renewed by its heartbeat and can never be reached by it.
    pub fn sweep(&self, now: Millis) -> Result<usize, BrokerError> {
        let removed = self.lock().execute(
            &format!("DELETE FROM reservations WHERE {EXPIRED}"),
            [as_i64(now)],
        )?;
        if removed > 0 {
            tracing::info!(removed, "swept expired capacity reservations");
        }
        Ok(removed)
    }

    fn total(&self, now: Millis, exclude: Option<&str>) -> Result<Occupancy, BrokerError> {
        let (runs, shares): (i64, i64) = self.lock().query_row(
            &format!(
                "SELECT COUNT(*), COALESCE(SUM(shares), 0) FROM reservations
                 WHERE {LIVE} AND (?2 IS NULL OR instance <> ?2)"
            ),
            rusqlite::params![as_i64(now), exclude],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(Occupancy::new(as_u32(runs), as_u32(shares)))
    }

    /// Recover from a poisoned lock rather than propagating the panic: one failed query should
    /// not leave the daemon unable to ask about capacity for the rest of its life.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The expiry boundary, written down once so nothing can hold half of it. `?1` is `now`.
///
/// A reservation is live while `expires_at > now`, which makes it expired at `now >=
/// expires_at` — the same comparison as `offload_core::Lease::is_expired`, because a
/// reservation *is* a lease and two rules for one word is how they drift apart.
///
/// The direction matters more than it looks, for `blobs::collect_garbage`'s reason from the
/// other side: everything written in a millisecond shares a timestamp, so `committed` and
/// `sweep` are routinely asked about the same row at the same `now`. The two predicates are
/// therefore exact complements and both are spliced from here — otherwise a row could be
/// counted *and* swept in one tick (an instance reserves capacity, then sweeps away its own
/// promise) or neither (a dead instance's row is uncollectable and holds the machine for
/// ever). One boundary, two queries derived from it.
const LIVE: &str = "expires_at_ms > ?1";

/// The complement of [`LIVE`], and nothing else. `?1` is `now`.
const EXPIRED: &str = "expires_at_ms <= ?1";

/// Bring a connection up to the current schema version. Its own counter — this is a separate
/// database from the node's state, and the two version numbers mean different things.
fn migrate(conn: &Connection) -> rusqlite::Result<u32> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = u32::try_from(index + 1).unwrap_or(u32::MAX);
        tracing::info!(version, "applying reservation ledger migration");
        // Migration plus version bump in one transaction: a crash halfway through must not
        // leave a file claiming a version it does not have — and here the next process to
        // open it may be a different build.
        conn.execute_batch(&format!(
            "BEGIN; {sql} PRAGMA user_version = {version}; COMMIT;"
        ))?;
    }

    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Connection settings applied on every open.
fn configure(conn: &Connection) -> rusqlite::Result<()> {
    // WAL, so an instance reading the total is not blocked by another instance accepting a
    // run. This is the setting the whole multi-process story rests on.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // NORMAL is generous here: a reservation describes what is running *now*, so anything
    // lost to a power cut describes a machine that is no longer running it.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Unlike the node's store, several processes genuinely write this file. A busy error
    // surfacing as a refused bid would be a device declining work because two of its own
    // daemons wrote at once, so wait instead.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

fn as_i64(value: Millis) -> i64 {
    i64::try_from(value.0).unwrap_or(i64::MAX)
}

fn as_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(byte: u8) -> RunId {
        RunId::from_bytes([byte; 16])
    }

    /// The ordinary call: a run accepted now, held for a lease. Tests that are *about* the
    /// expiry say so with an explicit ttl instead.
    fn hold(broker: &Broker, instance: &str, id: u8, demand: Demand) {
        broker
            .reserve(instance, run(id), demand, Millis(0), RESERVATION_TTL)
            .expect("reserve");
    }

    /// Scratch file, named for the process so a parallel test binary cannot collide.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("offload-broker-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir.join(LEDGER_FILE)
    }

    #[test]
    fn two_instances_reservations_sum() {
        // The whole reason this file exists: neither fleet's daemon can see the other's
        // state directory, and the machine has one set of cores.
        let broker = Broker::open_memory().expect("open");
        hold(&broker, "fleet-a", 1, Demand::Normal);
        hold(&broker, "fleet-b", 2, Demand::Heavy);

        let total = broker.committed(Millis(1_000)).expect("committed");
        assert_eq!(total.runs, 2);
        assert_eq!(
            total.shares,
            Demand::Normal.shares() + Demand::Heavy.shares()
        );
    }

    #[test]
    fn an_expired_reservation_stops_counting() {
        // A crashed instance must not hold the machine's capacity for ever, and it must
        // stop counting the moment it lapses — not when somebody remembers to sweep.
        let broker = Broker::open_memory().expect("open");
        broker
            .reserve(
                "gone",
                run(1),
                Demand::Heavy,
                Millis(0),
                Millis::from_secs(30),
            )
            .expect("reserve");

        assert_eq!(
            broker
                .committed(Millis::from_secs(29))
                .expect("before")
                .runs,
            1
        );
        assert!(broker
            .committed(Millis::from_secs(31))
            .expect("after")
            .is_idle());
    }

    #[test]
    fn the_expiry_boundary_is_a_leases_boundary_and_sweep_is_its_complement() {
        // Live while `expires_at > now`, expired at `now >= expires_at` — the comparison
        // `Lease::is_expired` uses. And the two queries must partition the row exactly: at
        // the boundary instant it must be gone from the total *and* reachable by sweep, so
        // that no row is ever both counted and collected, or neither.
        let broker = Broker::open_memory().expect("open");
        let ttl = Millis::from_secs(60);
        broker
            .reserve("a", run(7), Demand::Normal, Millis(0), ttl)
            .expect("reserve");

        let boundary = ttl;
        let just_before = Millis(boundary.0 - 1);

        assert_eq!(
            broker.committed(just_before).expect("before").runs,
            1,
            "a reservation is live up to but not including its expiry"
        );
        assert_eq!(
            broker.sweep(just_before).expect("sweep before"),
            0,
            "and is not collectable while it is live"
        );

        assert!(
            broker.committed(boundary).expect("at").is_idle(),
            "at the expiry instant it stops counting"
        );
        assert_eq!(
            broker.sweep(boundary).expect("sweep at"),
            1,
            "and becomes collectable in the same millisecond it stopped counting"
        );
    }

    #[test]
    fn committed_elsewhere_excludes_the_callers_own_rows() {
        // A node knows its own runs in more detail than a ledger row carries. Adding the
        // two totals is how it counts them twice and refuses work it has room for.
        let broker = Broker::open_memory().expect("open");
        hold(&broker, "mine", 1, Demand::Normal);
        hold(&broker, "theirs", 2, Demand::Light);

        let elsewhere = broker
            .committed_elsewhere("mine", Millis(0))
            .expect("elsewhere");
        assert_eq!(elsewhere.runs, 1);
        assert_eq!(elsewhere.shares, Demand::Light.shares());

        assert_eq!(
            broker.committed(Millis(0)).expect("all").runs,
            2,
            "the total is unchanged by who asked"
        );
    }

    #[test]
    fn reserving_the_same_run_twice_does_not_double_count() {
        // A retried accept, or an instance that crashed between reserving and recording it.
        let broker = Broker::open_memory().expect("open");
        for _ in 0..3 {
            hold(&broker, "a", 4, Demand::Normal);
        }
        let total = broker.committed(Millis(0)).expect("committed");
        assert_eq!(total.runs, 1);
        assert_eq!(total.shares, Demand::Normal.shares());
    }

    #[test]
    fn re_reserving_with_a_new_demand_replaces_the_old_cost() {
        let broker = Broker::open_memory().expect("open");
        hold(&broker, "a", 4, Demand::Light);
        hold(&broker, "a", 4, Demand::Heavy);
        assert_eq!(
            broker.committed(Millis(0)).expect("committed").shares,
            Demand::Heavy.shares()
        );
    }

    #[test]
    fn release_frees_the_budget() {
        let broker = Broker::open_memory().expect("open");
        hold(&broker, "a", 1, Demand::Heavy);
        assert!(broker.release("a", run(1)).expect("release"));
        assert!(broker.committed(Millis(0)).expect("committed").is_idle());
        assert!(
            !broker.release("a", run(1)).expect("again"),
            "releasing twice is not an error, and says it found nothing"
        );
    }

    #[test]
    fn an_instance_may_only_touch_its_own_promises() {
        // Cross-fleet interference on a shared file: one instance releasing another's
        // reservation would make the machine look idle while an agent was mid-turn on it.
        let broker = Broker::open_memory().expect("open");
        hold(&broker, "owner", 1, Demand::Normal);

        assert!(!broker.release("stranger", run(1)).expect("release"));
        assert!(!broker
            .renew("stranger", run(1), Millis(0), Millis::from_secs(600))
            .expect("renew"));
        assert_eq!(broker.committed(Millis(0)).expect("committed").runs, 1);
    }

    #[test]
    fn sweep_drops_a_dead_instance_and_keeps_a_renewing_one() {
        // The two halves of what expiry is for. `dead` stopped heartbeating a lease ago;
        // `live` has renewed, and no amount of sweeping may touch it.
        let broker = Broker::open_memory().expect("open");
        hold(&broker, "dead", 1, Demand::Normal);
        hold(&broker, "live", 2, Demand::Normal);

        // Five renewals across five lease periods, sweeping at each step.
        let mut now = Millis(0);
        for _ in 0..5 {
            now = now.saturating_add(RESERVATION_TTL.scaled_percent(50));
            assert!(
                broker
                    .renew("live", run(2), now, RESERVATION_TTL)
                    .expect("renew"),
                "a renewing instance keeps its reservation"
            );
            broker.sweep(now).expect("sweep");
        }

        assert_eq!(
            broker.committed(now).expect("committed").runs,
            1,
            "only the renewing instance is left"
        );
        assert!(
            !broker.release("dead", run(1)).expect("dead row"),
            "the dead instance's row was swept"
        );
        assert!(
            broker.release("live", run(2)).expect("live row"),
            "the live one was never swept"
        );
    }

    #[test]
    fn sweep_reports_only_what_it_removed() {
        let broker = Broker::open_memory().expect("open");
        broker
            .reserve(
                "a",
                run(1),
                Demand::Normal,
                Millis(0),
                Millis::from_secs(10),
            )
            .expect("short");
        broker
            .reserve(
                "a",
                run(2),
                Demand::Normal,
                Millis(0),
                Millis::from_secs(600),
            )
            .expect("long");

        assert_eq!(broker.sweep(Millis::from_secs(60)).expect("sweep"), 1);
        assert_eq!(broker.sweep(Millis::from_secs(60)).expect("again"), 0);
        assert_eq!(
            broker.committed(Millis::from_secs(60)).expect("left").runs,
            1
        );
    }

    #[test]
    fn the_ledger_path_prefers_the_runtime_directory() {
        let path = resolve_path(Some(OsString::from("/run/user/1000")), "1000");
        assert_eq!(
            path,
            PathBuf::from("/run/user/1000/offload/reservations.db")
        );
    }

    #[test]
    fn an_empty_runtime_directory_is_not_the_current_directory() {
        // Somebody unsetting the variable awkwardly. A relative path here would give every
        // instance a private ledger in whatever directory it was started from.
        let path = resolve_path(Some(OsString::new()), "1000");
        assert!(path.is_absolute(), "got {}", path.display());
        assert!(path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn without_a_runtime_directory_the_ledger_is_per_user_under_temp() {
        let path = resolve_path(None, "1000");
        assert_eq!(
            path,
            std::env::temp_dir()
                .join("offload-1000")
                .join("reservations.db")
        );
    }

    #[test]
    fn the_user_identifier_is_never_empty_or_a_path() {
        // It becomes a directory name, so a `/` in it would silently move the ledger.
        let user = current_user();
        assert!(!user.is_empty());
        assert!(!user.contains('/'), "got {user}");
    }

    #[test]
    fn two_handles_on_one_file_both_write() {
        // The property the entire choice of SQLite rests on: two `offloadd` instances, two
        // connections, one file, both accepting work. Two handles is the honest simulation
        // available in a unit test — the locking they contend on is the same.
        let path = scratch("shared");
        let a = Broker::open_at(&path).expect("open a");
        let b = Broker::open_at(&path).expect("open b");

        for i in 0..20u8 {
            a.reserve("fleet-a", run(i), Demand::Light, Millis(0), RESERVATION_TTL)
                .expect("a writes");
            b.reserve(
                "fleet-b",
                run(100 + i),
                Demand::Light,
                Millis(0),
                RESERVATION_TTL,
            )
            .expect("b writes");
        }

        // Each sees the other's promises, which is the only reason to have a ledger.
        assert_eq!(a.committed(Millis(0)).expect("a total").runs, 40);
        assert_eq!(b.committed(Millis(0)).expect("b total").runs, 40);
        assert_eq!(
            a.committed_elsewhere("fleet-a", Millis(0))
                .expect("elsewhere")
                .runs,
            20
        );

        // And a third instance starting later reads what is already promised.
        drop(a);
        let c = Broker::open_at(&path).expect("open c");
        assert_eq!(c.committed(Millis(0)).expect("c total").runs, 40);

        if let Some(dir) = path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn opening_twice_is_a_no_op_for_the_schema() {
        // Every instance start runs the migrations, and there may be several starts a day.
        let path = scratch("migrate");
        let first = Broker::open_at(&path).expect("first");
        drop(first);
        let second = Broker::open_at(&path).expect("second");
        assert!(second.committed(Millis(0)).expect("committed").is_idle());

        let version: u32 = second
            .lock()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version as usize, MIGRATIONS.len());

        if let Some(dir) = path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
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
    const SHIPPED: &[u64] = &[0x019fad3423c8bae7];

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
