//! Firing standing instructions on a clock (ADR-0019 §3, ADR-0056).
//!
//! One pass, on a poll, and it is deliberately dull: for every schedule this node is the steward
//! of, is the current tick one that has not been served? If so, build the occurrence from the
//! stored spec — with the id derived from `(schedule, tick)` — and submit it exactly as a person's
//! submission is submitted. Everything after that point is the ordinary machinery: one bid round,
//! one lease, one arbiter, the delivery plane.
//!
//! What is *not* here is the interesting part. There is no catch-up: a device that has been asleep
//! computes the tick it is in and fires at most that one, because a watcher's value is the current
//! state and a backlog of stale occurrences is the notification storm ADR-0010's audience rules
//! exist to prevent. And there is no queue of pending firings — a tick that nobody could serve
//! (the fleet was full, the owner said no) is simply not served, and the next tick asks again.

use crate::server::Ctx;
use offload_core::{Millis, Schedule};

/// How often to look. A schedule's period is at least a minute (`offload_core::MIN_EVERY`), so
/// any cadence below that cannot miss a tick — what it decides is how *late* a firing can be.
///
/// Five seconds rather than the gossip tick's one: the pass is two small queries per schedule and
/// there is no reason to do it a second apart, and rather than a minute because "fires at 03:00"
/// meaning "some time before 03:01" is a worse answer than the arithmetic deserves.
pub const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// How many times one tick is offered to the fleet before it is left alone.
///
/// A dozen, which at [`LOOK_EVERY`] is the first minute of a tick. Both ends of that number are
/// doing work:
///
/// * **Without a retry**, a refusal consumes the tick. Measured on two daemons: the home node
///   was killed, the successor fired correctly, and nobody could host the occurrence because the
///   successor was still on probation — so a daily schedule would have lost the day for a
///   refusal that lasted seconds.
/// * **Without a bound**, a schedule naming a service nobody offers — the likeliest way to write
///   one wrong — runs a bid round every five seconds for ever.
///
/// Bounded by *attempts* rather than by how far into the tick it is, and the difference is a
/// defect this had: a daemon that starts up in the middle of a tick has made no attempts and
/// should serve it, while a clock-based window would have refused to. Measured as arithmetic —
/// the first test written against this fired nothing, because `now` was seven hundred seconds
/// into a fifteen-minute tick.
const TRIES_PER_TICK: u32 = 12;

/// What this node has tried, this tick, in memory only.
///
/// Not in the store on purpose: it is a fact about *this process's* attempts and it is worth
/// nothing after a restart — a daemon that comes back deserves a fresh dozen, since whatever was
/// refusing may have been the thing that restarted. Held by the loop and passed in, so the
/// signature says the pass is not pure rather than hiding it in a lock somewhere.
#[derive(Debug, Default)]
pub(crate) struct Attempts(std::collections::HashMap<offload_core::ScheduleId, (Millis, u32)>);

impl Attempts {
    /// May this node offer `tick` to the fleet again? Counts the attempt if so.
    fn may_try(&mut self, id: offload_core::ScheduleId, tick: Millis) -> bool {
        let entry = self.0.entry(id).or_insert((tick, 0));
        if entry.0 != tick {
            // A new tick, so the count starts again. Also the whole of the cleanup this map
            // needs: an entry per schedule, replaced rather than accumulated.
            *entry = (tick, 0);
        }
        if entry.1 >= TRIES_PER_TICK {
            return false;
        }
        entry.1 += 1;
        true
    }

    /// Forget the attempt that succeeded, so a later tick starts clean.
    fn placed(&mut self, id: offload_core::ScheduleId) {
        self.0.remove(&id);
    }
}

/// Watch the clock for as long as the daemon runs.
///
/// Spawned from `server::serve` rather than from `main`, beside the trigger watchers, and for
/// their reason: a schedule's whole job is to submit runs, the checks a submission passes live
/// beside `submit_run`, and there is one copy of them whichever way a run is born.
pub(crate) fn spawn(ctx: Ctx, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    tokio::spawn(async move {
        let mut tried = Attempts::default();
        loop {
            tokio::select! {
                () = tokio::time::sleep(LOOK_EVERY) => {}
                _ = shutdown.changed() => return,
            }
            pass(&ctx, &mut tried, crate::supervisor::now()).await;
        }
    });
}

/// Fire whatever is due, once.
///
/// Returns the occurrences started, for the caller's log — and for tests, which is the only way
/// to observe a pass that is supposed to be silent.
pub(crate) async fn pass(ctx: &Ctx, tried: &mut Attempts, now: Millis) -> Vec<offload_core::RunId> {
    let schedules = match ctx.store.schedules() {
        Ok(schedules) => schedules,
        Err(e) => {
            // Loud, and it changes nothing: not being able to read the schedules is not
            // permission to fire none of them for ever, and the next pass tries again.
            tracing::error!(error = %e, "could not read this node's schedules");
            return Vec::new();
        }
    };

    let mut fired = Vec::new();
    for schedule in schedules {
        if schedule.is_removed() {
            continue;
        }
        if !is_steward(ctx, &schedule) {
            continue;
        }
        let tick = schedule.tick_at(now);
        // A schedule is not due for the tick it was born in: that tick started before it
        // existed, and firing it makes `every 15m` produce two occurrences eight minutes
        // apart — the second of which is the one the keyboard was told about.
        if !schedule.fires_tick(tick) {
            continue;
        }
        match served(ctx, &schedule, tick) {
            Served::Yes => continue,
            // A store that will not answer is not permission to start a second agent on
            // somebody's repository. Same rule as `rule_run_in_flight`'s, and the same
            // direction: not knowing whether an occurrence is running means not starting one.
            Served::Unknown(why) => {
                tracing::warn!(
                    schedule = %schedule.id,
                    "{why}; not firing this tick, because not knowing is not permission"
                );
                continue;
            }
            Served::No => {}
        }

        // A refusal is transient more often than not — a full fleet, a laptop that has not
        // finished starting, a phone on battery — so an occurrence nobody would take is offered
        // again, a bounded number of times. See [`TRIES_PER_TICK`] for what each end of that
        // bound is protecting against.
        if !tried.may_try(schedule.id, tick) {
            continue;
        }

        let run = occurrence(&schedule, tick, ctx.node_id, now);
        let id = run.id;
        match crate::server::place_occurrence(ctx, run).await {
            Ok(placed) => {
                // Written down **after** it is placed, which is the opposite of what this first
                // did. Marking the tick first meant a refused occurrence consumed it, so a
                // daily schedule whose fleet was busy for one second lost the day — measured on
                // two daemons, where the successor fired into a fleet that had nobody able to
                // host and the tick was spent. The window above is what makes recording it late
                // safe: an unmarked tick is retried, and only for a minute.
                if let Err(e) = ctx.store.note_schedule_fired(schedule.id, tick) {
                    // Loud, and the run is already placed: what this costs is one repeated
                    // occurrence, which the derived id makes one *record* and the `Idempotent`
                    // spec makes safe to run twice.
                    tracing::error!(
                        schedule = %schedule.id,
                        run = %id,
                        error = %e,
                        "fired a schedule but could not record which tick"
                    );
                }
                tracing::info!(
                    schedule = %schedule.id,
                    run = %id,
                    node = placed.elsewhere.as_deref().unwrap_or("here"),
                    tick = tick.0,
                    "a schedule fired"
                );
                tried.placed(schedule.id);
                fired.push(id);
            }
            // Not an error: an occurrence nobody will take is the fleet being busy or asleep,
            // and a watcher that misses a look is doing its job. Said at `info` with the reason
            // the round gave, because "why is my schedule not running" is answered here or
            // nowhere.
            Err(why) => tracing::info!(
                schedule = %schedule.id,
                run = %id,
                tick = tick.0,
                "nobody took this occurrence: {why}"
            ),
        }
    }
    fired
}

/// Build the occurrence for one tick.
///
/// The id is the schedule's own derivation, so two nodes that both fire this tick produce the
/// *same* run and `merge_run` settles them as one record (ADR-0019 §3). `home` is the firing node
/// rather than the schedule's home, because home on a `Run` means *who arbitrates this run* — and
/// the node that submitted it is the one holding the round.
///
/// `Origin::Rule` is what marks it machine-started: no person typed this and none is waiting, so
/// its record may be reclaimed once the delivery plane is finished with it (ADR-0024). The
/// variant's name is narrower than its meaning; see ADR-0056's consequences.
#[must_use]
pub fn occurrence(
    schedule: &Schedule,
    tick: Millis,
    firing_node: offload_core::NodeId,
    now: Millis,
) -> offload_core::Run {
    offload_core::Run::new(
        schedule.occurrence_id(tick),
        schedule.spec.clone(),
        firing_node,
        now,
    )
    .started_by(offload_core::Origin::Rule)
}

/// Is this node the one node that fires this schedule right now?
///
/// `ClusterView::steward_of` is the answer and there is no second copy of it — the same successor
/// rule arbitration uses, for the reason ADR-0056 §4 gives. On a fleet of one there is no view to
/// ask, and the answer is yes for the only schedules that can exist there: its own.
fn is_steward(ctx: &Ctx, schedule: &Schedule) -> bool {
    match &ctx.cluster {
        Some(cluster) => cluster.view().steward_of(schedule.home) == Some(ctx.node_id),
        // No mesh, so nobody else could be firing it. A schedule whose home is *another* node
        // can only have arrived by gossip, which needs a mesh — but a database that has been
        // moved between machines is a real thing, and firing another node's schedule from a
        // machine that cannot see it is the one case here that could double up.
        None => schedule.home == ctx.node_id,
    }
}

/// Whether this tick has already been served, and by what evidence.
///
/// Three-valued because the third answer is the one that matters: a store that will not answer
/// has not said no (`docs/pitfalls`' standing rule — unknown is not none, and unknown is not
/// good news).
enum Served {
    Yes,
    No,
    Unknown(String),
}

fn served(ctx: &Ctx, schedule: &Schedule, tick: Millis) -> Served {
    // This node's own memory first, because it is the check that survives the occurrence's
    // record being reclaimed — which happens to every machine-started run about an hour after it
    // finishes, and is most of a day before a daily schedule's tick ends (ADR-0056 §2).
    match ctx.store.schedule_last_tick(schedule.id) {
        Ok(Some(last)) if last >= tick => return Served::Yes,
        Ok(_) => {}
        Err(e) => return Served::Unknown(format!("cannot read the last tick fired here: {e}")),
    }
    // Then the fleet's record of it, which is what stops a *second* node from serving a tick
    // this one has. Derived id, so no message is needed to look.
    match ctx.store.load_run(schedule.occurrence_id(tick)) {
        Ok(Some(_)) => Served::Yes,
        Ok(None) => Served::No,
        Err(e) => Served::Unknown(format!("cannot read this tick's occurrence: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{Constraint, Demand, NodeId, Restartability, RunSpec, ScheduleId, Work};

    fn a_schedule(home: NodeId) -> Schedule {
        Schedule {
            id: ScheduleId::from_bytes([3; 8]),
            home,
            every: Millis::from_mins(15),
            offset: Millis::ZERO,
            spec: RunSpec {
                work: Work::Task(offload_core::TaskWork {
                    service: offload_core::Service::Other("watch-api".into()),
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
                prefer: offload_core::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            note: String::new(),
            created_at: Millis(0),
            removed_at: None,
        }
    }

    fn ctx_for(dir: &std::path::Path, me: NodeId) -> crate::server::Ctx {
        crate::server::test_ctx(dir, me)
    }

    /// A daemon with no mesh, one nominated task, and nothing else.
    fn a_node(name: &str, me: NodeId) -> (std::path::PathBuf, crate::server::Ctx) {
        let dir = std::env::temp_dir().join(format!("offload-sched-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let ctx = ctx_for(&dir, me);
        (dir, ctx)
    }

    #[tokio::test]
    async fn a_pass_fires_the_current_tick_once_and_then_leaves_it_alone() {
        // The whole mechanism in one test: the tick is served, and looking again inside it does
        // nothing. Two different things say so — this node's high-water mark and the
        // occurrence's own record — and this is the first of them, since the record here is
        // written by the same pass.
        let me = NodeId::from_bytes([1; 32]);
        let (_dir, ctx) = a_node("once", me);
        let schedule = Schedule {
            created_at: Millis(0),
            ..a_schedule(me)
        };
        ctx.store.merge_schedule(&schedule).expect("write");

        // Deliberately not the real clock: the tick is arithmetic, so the test drives it.
        let now = Millis(1_789_000_000_000);
        let tick = schedule.tick_at(now);
        let mut tried = Attempts::default();
        let first = pass(&ctx, &mut tried, now).await;
        assert_eq!(first.len(), 1, "the current tick is served");
        assert_eq!(first[0], schedule.occurrence_id(tick));
        assert_eq!(
            ctx.store.schedule_last_tick(schedule.id).expect("read"),
            Some(tick),
            "…and recorded, after it was placed rather than before"
        );

        let again = pass(&ctx, &mut tried, now.saturating_add(Millis(5_000))).await;
        assert!(again.is_empty(), "the same tick is not served twice");

        // The next tick is a different occurrence, and this is what "no catch-up" means: a pass
        // that wakes up hours later fires the tick it is in and nothing before it.
        let much_later = now.saturating_add(Millis::from_mins(6 * 60));
        let late = pass(&ctx, &mut tried, much_later).await;
        assert_eq!(late.len(), 1, "one tick, not the twenty-four that passed");
        assert_eq!(
            late[0],
            schedule.occurrence_id(schedule.tick_at(much_later))
        );
    }

    #[tokio::test]
    async fn a_removed_schedule_and_another_nodes_schedule_are_both_left_alone() {
        let me = NodeId::from_bytes([1; 32]);
        let (_dir, ctx) = a_node("quiet", me);
        let now = Millis(1_789_000_000_000);

        let mine = Schedule {
            created_at: Millis(0),
            ..a_schedule(me)
        };
        ctx.store.merge_schedule(&mine).expect("write");
        ctx.store
            .remove_schedule(mine.id, 1_000)
            .expect("tombstone");

        // A schedule whose home is somebody else, on a node with no mesh. It is here because
        // gossip put it here, and firing it would be this node deciding it is the steward with
        // no view to ask (`is_steward`'s no-cluster arm).
        let theirs = Schedule {
            id: offload_core::ScheduleId::from_bytes([9; 8]),
            home: NodeId::from_bytes([2; 32]),
            created_at: Millis(0),
            ..a_schedule(me)
        };
        ctx.store.merge_schedule(&theirs).expect("write");

        assert!(
            pass(&ctx, &mut Attempts::default(), now).await.is_empty(),
            "a tombstone fires nothing, and neither does another node's schedule"
        );
    }

    #[test]
    fn an_occurrence_is_machine_started_and_carries_the_schedules_own_spec() {
        let me = NodeId::from_bytes([1; 32]);
        let schedule = a_schedule(me);
        let tick = Millis(1_757_000_000_000);
        let run = occurrence(&schedule, tick, me, tick.saturating_add(Millis(40)));

        assert_eq!(run.id, schedule.occurrence_id(tick));
        assert_eq!(run.spec, schedule.spec, "the spec is the schedule's, whole");
        assert_eq!(
            run.origin,
            offload_core::Origin::Rule,
            "nobody typed this and nobody is waiting for the output"
        );
        assert_eq!(
            run.home, me,
            "home on a run is who arbitrates it, which is whoever fired it"
        );
        // Two nodes firing one tick make the same run, which is the whole of ADR-0019 §3's
        // answer to a transient second steward.
        let them = NodeId::from_bytes([2; 32]);
        assert_eq!(
            occurrence(&schedule, tick, them, tick).id,
            run.id,
            "one tick, one record"
        );
    }
}
