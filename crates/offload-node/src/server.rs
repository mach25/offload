//! The control socket.
//!
//! One connection, one request, one or more newline-delimited JSON responses ending in
//! `Done`. Unix domain socket rather than TCP: the daemon holds the user's agent
//! credentials by proxy, and a port is reachable by anything on the machine while socket
//! file permissions are the same access control the rest of the user's home already has.

use crate::api::{NodeStatus, Request, Response};
use crate::config::Config;
use crate::supervisor::Supervisor;
use offload_core::{AgentKind, NodeId};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Bind the control socket, replacing a stale one if the previous daemon died badly.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if path.exists() {
        // A leftover socket file from a crash would make every start fail with
        // "address in use", which is a confusing way to say "the last run crashed".
        //
        // But only a leftover. This used to say that a live daemon already held the lock on the
        // state dir, which was not true of anything — and the consequence was not a confusing
        // error, it was silence: the second daemon unlinked the first's socket and bound its
        // own, so the first kept its runs and its leases and simply stopped being reachable.
        // The state dir is locked now (`crate::statedir`), and this is the case that lock cannot
        // see — a socket path pointed elsewhere by config, shared by two directories.
        if crate::statedir::something_is_listening(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!(
                    "a daemon is already listening on {} — unlinking it would leave that one \
                     running and unreachable",
                    path.display()
                ),
            ));
        }
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    UnixListener::bind(path)
}

/// Accept connections until `shutdown` resolves.
#[allow(clippy::too_many_arguments)]
pub async fn serve(
    listener: UnixListener,
    supervisor: Supervisor,
    store: offload_store::Store,
    config: Arc<Config>,
    node_id: NodeId,
    capabilities: Arc<crate::deliver::Current>,
    models: Arc<crate::deliver::Models>,
    mesh: Option<Arc<crate::mesh::Mesh>>,
    sinks: Arc<crate::deliver::Sinks>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let cluster = mesh.as_ref().map(|m| m.cluster.clone());

    // The watchers start with the socket rather than in `main`, because a trigger's whole job is
    // to submit runs and `submit_run` is here: one copy of the checks, whichever way a run is
    // born (ADR-0020 §5).
    let triggers = crate::trigger::Triggers::new();
    let base = Ctx {
        models,
        supervisor,
        store,
        config,
        node_id,
        capabilities,
        cluster,
        mesh,
        triggers: triggers.clone(),
    };
    crate::trigger::spawn_all(base.clone(), &triggers);
    // …and the clock, for the same reason and in the same place (ADR-0019 §3): a schedule fires
    // a submission, and every submission goes through the checks in this file.
    crate::schedule::spawn(base.clone(), shutdown.clone());
    // …and the delivery plane, which moved here for the same reason once a notice could fire a
    // rule: firing one is a submission, and every submission goes through the checks in this
    // file. It is still its own tick and still not in the gossip loop, which is what its own
    // doc comment is about.
    tokio::spawn(deliver_notifications(base.clone(), sinks, shutdown.clone()));
    // …and keeping the machine awake while this node would take work (ADR-0077): the decision is
    // `hosting_refusal`, which is here.
    tokio::spawn(keep_awake(base.clone(), shutdown.clone()));
    tokio::spawn(hibernate(base.clone(), shutdown.clone()));

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("control socket closing");
                    return;
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let ctx = base.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle(stream, ctx).await {
                                tracing::debug!(error = %e, "client connection ended");
                            }
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "accept failed"),
                }
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Ctx {
    pub(crate) supervisor: Supervisor,
    /// Held directly, rather than reached through the supervisor, for the one question that is
    /// not about runs: what this node's delivery routes have carried (ADR-0010).
    pub(crate) store: offload_store::Store,
    pub(crate) config: Arc<Config>,
    pub(crate) node_id: NodeId,
    /// What this device is *now*. A handle rather than the value, because the daemon re-probes
    /// on a schedule and every door here decides with it — see [`crate::deliver::Current`] for
    /// the three readers that disagreed while it was a startup snapshot.
    pub(crate) capabilities: Arc<crate::deliver::Current>,
    /// `None` when this node is alone — a fleet of one is a supported way to run, not a
    /// failure, so the question has an answer either way.
    pub(crate) cluster: Option<Arc<offload_cluster::Cluster>>,
    /// The mesh itself, for the one operation that needs more than the cluster: a drain has
    /// to talk to the supervisor and the fleet in the same breath.
    pub(crate) mesh: Option<Arc<crate::mesh::Mesh>>,
    /// What this node's nominated watchers are doing (ADR-0020). Held here rather than looked
    /// up, because it is live state of processes this daemon owns and there is nowhere durable
    /// it could be read from: a watcher's restart count is about *this* incarnation.
    pub(crate) triggers: crate::trigger::Triggers,
    /// The agent's model list and the way to ask for it again (ADR-0080).
    pub(crate) models: Arc<crate::deliver::Models>,
}

async fn handle(stream: UnixStream, ctx: Ctx) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();

    let Some(line) = lines.next_line().await? else {
        return Ok(());
    };

    let request: Request = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => {
            return reply(
                &mut write,
                &Response::Error {
                    message: format!("malformed request: {e}"),
                },
            )
            .await;
        }
    };

    dispatch(request, ctx, &mut write, &mut lines).await
}

/// The reader half of a client's connection, kept alive so a follower's *departure* is
/// observable.
///
/// A `logs -f` that nobody is reading any more must stop: the daemon otherwise polls a peer for
/// ever, and — worse — the run keeps looking **attended**, which is the input that decides
/// whether a failed run resumes itself. Nothing else in this protocol notices a hangup, because
/// nothing else waits: a client that goes away mid-answer is discovered by the write failing,
/// and a follow can go minutes between writes.
type Client = tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>;

/// Resolves when the client on the other end has gone.
///
/// `next_line()` at EOF is the hangup. A client that sends a *second* request on the same
/// connection is treated the same way, which is honest for a protocol of one request per
/// connection: there is nothing here that would read it.
async fn hung_up(client: &mut Client) {
    let _ = client.next_line().await;
}

/// Offer a submission to the fleet, and report who took it.
///
/// One bounded round, synchronously, because the person who typed the command is standing
/// there: they get the node that committed, or every reason nobody would (ADR-0014). What
/// they never get is a success that means "filed away, good luck".
/// Where a submission ended up, in the words the operator gets.
pub(crate) struct Placed {
    pub(crate) run: offload_core::RunId,
    /// The node that took it, when that is not this one.
    pub(crate) elsewhere: Option<String>,
    /// When it starts, if that is not immediately.
    pub(crate) waiting: Option<String>,
    /// Whether the fleet would still have this submission if this machine went away now.
    ///
    /// `false` only when this node took the run itself and nobody else answered — the moment
    /// the overnight case is fragile, and the one worth a sentence rather than silence.
    pub(crate) durable: bool,
    /// Nobody would take it and `--queue` said to leave it pending anyway (ADR-0014), with
    /// every node's reason, because "queued" without them is the shrug this project refuses.
    pub(crate) queued: Option<String>,
    /// The run's preference was not met, and which of ADR-0063 §7's three reasons it was.
    /// `None` when it was met or there was none.
    pub(crate) preference: Option<String>,
}

/// Why a run's stated preference did not decide where it went, worded from the round's own
/// opinions (ADR-0063 §7) — the terms each offer was summed from, not a second copy of `score`.
///
/// Three answers, because they send somebody to three places: the preferred node **did not
/// answer** (it is asleep: `--hold` is the lever), it **could not take it** (its reason, which is
/// about that machine), or it **was outscored** (the terms that did it — the fleet had two
/// reasons to go elsewhere, which is the preference yielding as designed).
pub(crate) fn preference_note(
    spec: &offload_core::RunSpec,
    winner: offload_core::NodeId,
    opinions: &[offload_cluster::Opinion],
    name_of: impl Fn(offload_core::NodeId) -> String,
) -> Option<String> {
    use offload_cluster::Verdict;
    if spec.prefer == offload_core::Constraint::Always {
        return None;
    }
    let preferred = |o: &offload_cluster::Opinion| match &o.verdict {
        Verdict::Bids(offer) => offer.terms.is_some_and(|t| t.is_preferred()),
        _ => false,
    };
    let named = spec.prefer.named_nodes();
    // Asked of the ids first, and of the terms only where there are no ids: a preference *for a
    // machine* is met when that machine won, whatever the offer did or did not carry. Asking the
    // terms alone told a submitter their preferred node was outscored — by itself — the first
    // time a bid crossed the wire without them.
    if named.contains(&winner) {
        return None;
    }
    let won = opinions.iter().find(|o| o.node == winner);
    if named.is_empty() && won.is_some_and(preferred) {
        return None;
    }
    let placed = name_of(winner);
    // A preference about capabilities rather than a machine: say whether anybody who answered
    // matched it, since there is no one node to account for.
    if named.is_empty() {
        return Some(match opinions.iter().find(|o| preferred(o)) {
            Some(better) => format!(
                "{}, which matches the preference, was outscored — placed on {placed}{}",
                better.name,
                outscored_by(better, won)
            ),
            None => format!("no node that answered matches the preference — placed on {placed}"),
        });
    }
    let reasons: Vec<String> = named
        .iter()
        .map(|id| {
            let name = name_of(*id);
            match opinions.iter().find(|o| o.node == *id).map(|o| &o.verdict) {
                None | Some(Verdict::Silent) => {
                    format!("preferred {name}, which did not answer")
                }
                Some(Verdict::Unreachable(reason)) => {
                    format!("preferred {name}, which this node could not reach ({reason})")
                }
                Some(Verdict::WillNot(reason)) => {
                    format!("preferred {name}, which could not take it: {reason}")
                }
                Some(Verdict::Bids(offer)) if !offer.available.is_now() => {
                    format!(
                        "preferred {name}, which could not start it now ({})",
                        offer.available
                    )
                }
                Some(Verdict::Bids(_)) => {
                    let mine = opinions.iter().find(|o| o.node == *id);
                    format!(
                        "preferred {name}, which was outscored{}",
                        mine.map_or_else(String::new, |o| outscored_by(o, won))
                    )
                }
            }
        })
        .collect();
    Some(format!("{} — placed on {placed}", reasons.join("; ")))
}

/// ` (80 against 94: hardware +30, load −30 …)` — the terms where the winner's differ from the
/// loser's, largest first.
fn outscored_by(
    loser: &offload_cluster::Opinion,
    winner: Option<&offload_cluster::Opinion>,
) -> String {
    use offload_cluster::Verdict;
    let (Verdict::Bids(lost), Some(Verdict::Bids(won))) =
        (&loser.verdict, winner.map(|w| &w.verdict))
    else {
        return String::new();
    };
    let (Some(lt), Some(wt)) = (lost.terms, won.terms) else {
        return format!(" ({} against {})", lost.score.0, won.score.0);
    };
    let theirs: std::collections::BTreeMap<&str, i64> = wt.named().into_iter().collect();
    let ours: std::collections::BTreeMap<&str, i64> = lt.named().into_iter().collect();
    let mut deltas: Vec<(&str, i64)> = theirs
        .keys()
        .chain(ours.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|k| {
            (
                *k,
                theirs.get(k).copied().unwrap_or(0) - ours.get(k).copied().unwrap_or(0),
            )
        })
        .filter(|(_, d)| *d != 0)
        .collect();
    deltas.sort_by_key(|(_, d)| std::cmp::Reverse(d.abs()));
    let terms: Vec<String> = deltas
        .iter()
        .take(4)
        .map(|(k, d)| format!("{k} {d:+}"))
        .collect();
    format!(
        " ({} against {}: {} to the winner)",
        lost.score.0,
        won.score.0,
        terms.join(", ")
    )
}

/// Place one submission, wherever it belongs. The whole of `offload run`, minus the reply.
///
/// Extracted rather than left inline because there are **two** ways a run is born since ADR-0020:
/// an operator at a keyboard, and a trigger firing a rule they wrote earlier. The checks below
/// are not formalities that a second caller may skip — `Submit` is a grant that can be revoked
/// while a watcher goes on watching, and a service nothing in the fleet offers is a refusal
/// somebody can act on. A second copy of this that forgot one is exactly the shape of bug this
/// project keeps finding, so there is one copy.
/// [`submit_run`] for the cheap tier (ADR-0019).
///
/// The same three gates in the same order — may this node submit at all, is there a fleet to
/// place into, and can somebody host it — because they are questions about a *run*. What is
/// missing is the resource check, and it is missing because a task is granted nothing: it
/// reaches what the owner's nominated program reaches, which is the owner's decision and was
/// made when they nominated it.
pub(crate) async fn submit_task_run(
    ctx: &Ctx,
    req: crate::api::SubmitTaskRequest,
) -> Result<Placed, String> {
    if let Some(reason) = submit_grant_refusal(ctx) {
        return Err(reason);
    }

    let capacity =
        offload_core::Capacity::of(&ctx.config.work_policy(ctx.capabilities.now().device_class));

    match &ctx.cluster {
        Some(cluster) => {
            let run = ctx
                .supervisor
                .build_task(req, &members(ctx))
                .map_err(|e| e.to_string())?;
            place(cluster, ctx, run).await
        }
        // A fleet of one, so the constraint has nobody to be evaluated against and this is
        // where the question gets asked instead. **Refused while the operator is still here**
        // (ADR-0014): the first walk of this path accepted `--task push` on a node nominating
        // no such program, started it, and failed it a millisecond later — which is a run in
        // the log, a notification, and a person finding out at breakfast what they could have
        // been told at the keyboard. With a cluster this is the bid round's job and
        // `Constraint::HasService` already does it.
        None => match hosting_refusal(ctx, offload_core::Tier::Task, req.demand)
            .or_else(|| task_refusal(ctx, &req.service))
        {
            Some(reason) => Err(reason),
            None => ctx
                .supervisor
                .submit_task(req, capacity)
                .await
                .map(|run| Placed {
                    run,
                    elsewhere: None,
                    waiting: None,
                    durable: true,
                    queued: None,
                    preference: None,
                })
                .map_err(|e| e.to_string()),
        },
    }
}

/// Write a schedule down, and say when it first fires.
///
/// The submission becomes a **spec** here, which is the whole of ADR-0056 §3: `Supervisor::build`
/// applies this node's model, permission mode and baseline allowlist, and a peer that fires the
/// occurrence later may never have seen any of them. So the spec is complete at creation, exactly
/// as a submitted run's is.
///
/// `Restartability::Idempotent` is stamped rather than asked for: an occurrence that has to move
/// starts again from its spec, because there is no conversation to resume and nobody waiting to
/// be asked. `Schedule::check` refuses the alternative, and this is what makes sure nobody meets
/// that refusal for a reason they could not have known about.
fn create_schedule(
    ctx: &Ctx,
    spec: crate::api::EverySpec,
) -> Result<(offload_core::ScheduleId, String), String> {
    if let Some(reason) = submit_grant_refusal(ctx) {
        return Err(reason);
    }
    let work = match (spec.agent, spec.task) {
        (Some(request), None) => ctx
            .supervisor
            .build(request, &members(ctx))
            .map_err(|e| e.to_string())?,
        (None, Some(request)) => ctx
            .supervisor
            .build_task(request, &members(ctx))
            .map_err(|e| e.to_string())?,
        // Unreachable from this CLI, which makes the two mutually exclusive at the keyboard.
        // Answered anyway, because the control protocol is a protocol: `nc` can send this.
        _ => return Err("a schedule fires exactly one of an agent run or a task".to_string()),
    };
    let now = crate::supervisor::now();
    let schedule = offload_core::Schedule {
        // Random, the way a `RuleId` is: this is the one identifier here that must not be
        // derived from anything, because two schedules created in one millisecond on one node
        // have to be two schedules. (The *occurrence* ids are the derived ones.)
        id: offload_core::ScheduleId::from_bytes(rand::random()),
        home: ctx.node_id,
        every: offload_core::Millis(spec.every_ms),
        offset: offload_core::Millis(spec.offset_ms),
        spec: offload_core::RunSpec {
            restartability: offload_core::Restartability::Idempotent,
            ..work.spec
        },
        note: spec.note,
        created_at: now,
        removed_at: None,
    };
    schedule.check().map_err(|e| e.to_string())?;
    ctx.store
        .merge_schedule(&schedule)
        .map_err(|e| e.to_string())?;
    // Into the fleet's hands now rather than at the next tick: a schedule the creating node
    // could lose in the next second is the failure this whole mechanism exists to avoid.
    if let (Some(cluster), Ok(schedules)) = (&ctx.cluster, ctx.store.schedules()) {
        cluster.publish_schedules(schedules);
    }
    let next = schedule.next_tick_after(now).saturating_sub(now);
    Ok((schedule.id, format!("in {next}")))
}

/// Every schedule this node knows, worded for a person.
///
/// Composed by the daemon for `NodeSummary`'s reason: it holds the view, so *whether this node
/// is the one that fires a schedule* is a question only it can answer — and a CLI that worked it
/// out would be a second copy of a successor rule (ADR-0056 §4).
fn report_schedules(ctx: &Ctx) -> Result<Vec<crate::api::ScheduleReport>, String> {
    let schedules = ctx.store.schedules().map_err(|e| e.to_string())?;
    let view = ctx.cluster.as_ref().map(|cluster| cluster.view());
    let now = crate::supervisor::now();
    Ok(schedules
        .into_iter()
        .map(|schedule| {
            // Three answers, kept apart. `steward_of` returns `None` when the home is not in
            // this node's view at all — a daemon that restarted while the home was away, or a
            // device retired for good — and nothing fires the schedule then. Collapsing that
            // into "somebody else has it" is the report naming a machine for work no machine
            // is doing.
            let steward = match &view {
                Some(view) => match view.steward_of(schedule.home) {
                    Some(node) if node == ctx.node_id => crate::api::Steward::Here,
                    Some(node) => crate::api::Steward::Elsewhere {
                        node: view
                            .node(&node)
                            .map_or_else(|| node.short(), |n| n.name.clone()),
                    },
                    None => crate::api::Steward::Nobody,
                },
                // No mesh, so the only schedule anything here fires is one homed here — the
                // same rule `schedule::is_steward` takes on this arm, and for its reason.
                None if schedule.home == ctx.node_id => crate::api::Steward::Here,
                None => crate::api::Steward::Nobody,
            };
            crate::api::ScheduleReport {
                id: schedule.id.to_string(),
                home: view
                    .as_ref()
                    .and_then(|view| view.node(&schedule.home))
                    .map_or_else(|| schedule.home.short(), |node| node.name.clone()),
                steward,
                every: match schedule.offset {
                    offload_core::Millis::ZERO => format!("every {}", schedule.every),
                    offset => format!("every {}, {} in (UTC)", schedule.every, offset),
                },
                kind: schedule.spec.work.kind(),
                work: schedule.spec.work.summary(),
                note: schedule.note.clone(),
                next: format!("in {}", schedule.next_tick_after(now).saturating_sub(now)),
                last_fired: ctx
                    .store
                    .schedule_last_tick(schedule.id)
                    .ok()
                    .flatten()
                    .map(|tick| format!("{} ago", now.saturating_sub(tick))),
                removed: schedule.is_removed(),
            }
        })
        .collect())
}

/// Place an occurrence a schedule has just fired (ADR-0019 §3, ADR-0056).
///
/// The **third** way a run is born, beside an operator at a keyboard and a trigger firing a
/// rule — and the first that arrives already built: an occurrence's spec was written when the
/// schedule was created and its id is derived from the tick, so there is nothing here to build
/// and nothing to resolve.
///
/// The gates are the same ones and in the same order, which is why this is beside its siblings
/// rather than in `schedule.rs`. Two are deliberately *not* asked:
///
/// * **`unreachable_resource`**, which is ADR-0014's promise to somebody standing at a keyboard.
///   Nobody is standing here, and a schedule's spec was checked when it was created.
/// * **`task_refusal`** on the no-cluster arm, for the same reason: the refusal it exists to
///   give — *this node nominates no task for `push`* — was given when the schedule was written,
///   and repeating it every five seconds into a log nobody is reading is not an improvement.
///   What the fleet-of-one arm does ask is [`hosting_refusal`], because that answer changes by
///   the hour (a drain, a revocation, a battery) and it is the door that starts an agent.
pub(crate) async fn place_occurrence(ctx: &Ctx, run: offload_core::Run) -> Result<Placed, String> {
    // `Submit` is still the grant: a schedule fires *on behalf of* its author, and a device whose
    // right to ask the fleet for work has been revoked must stop asking (ADR-0012). The one
    // check every path here shares.
    if let Some(reason) = submit_grant_refusal(ctx) {
        return Err(reason);
    }

    let capacity =
        offload_core::Capacity::of(&ctx.config.work_policy(ctx.capabilities.now().device_class));

    match &ctx.cluster {
        Some(cluster) => place(cluster, ctx, run).await,
        None => match hosting_refusal(ctx, run.spec.work.tier(), run.spec.demand) {
            Some(reason) => Err(reason),
            None => {
                let id = run.id;
                ctx.supervisor
                    .submit_built(run, capacity)
                    .await
                    .map(|run| Placed {
                        run,
                        elsewhere: None,
                        waiting: None,
                        durable: true,
                        queued: None,
                        preference: None,
                    })
                    .map_err(|e| format!("{id}: {e}"))
            }
        },
    }
}

pub(crate) async fn submit_run(
    ctx: &Ctx,
    req: crate::api::SubmitRequest,
) -> Result<Placed, String> {
    // `Submit`, not `HostRuns`: a device that may not run agents may still ask the fleet to.
    // ADR-0012 grants `{Submit, Deliver}` at the door precisely so that a phone — or a node
    // still serving its probation — is a useful member from the first minute. Whether *this*
    // node hosts the run is decided when it bids.
    if let Some(reason) = submit_grant_refusal(ctx) {
        return Err(reason);
    }

    // A resource grant is not a placement constraint any more (ADR-0011): the call is
    // forwarded, so a run reaches a mailbox from wherever it lands. What is still answered at
    // the keyboard is whether the fleet has one at all — ADR-0014's rule, moved from the
    // constraint tree to here, because a run refused for asking for a service nobody offers is
    // a refusal somebody can act on, and one discovered at breakfast is not.
    if let Some(reason) = unreachable_resource(ctx, &req.resources) {
        return Err(reason);
    }

    let capacity =
        offload_core::Capacity::of(&ctx.config.work_policy(ctx.capabilities.now().device_class));

    match &ctx.cluster {
        // With peers, the fleet decides (ADR-0006) and the answer comes back while somebody is
        // still at the keyboard (ADR-0014).
        Some(cluster) => {
            let run = ctx
                .supervisor
                .build(req, &members(ctx))
                .map_err(|e| e.to_string())?;
            place(cluster, ctx, run).await
        }
        // Without, "here" is the only answer there has ever been — so the questions a bid would
        // have asked are asked right here instead, by [`hosting_refusal`], which is also what
        // `offload resume` asks (ADR-0047). Its doc comment carries the reasoning and the two
        // clauses that were missing from this arm, one per ADR.
        //
        // Note what is deliberately *not* here: none of this is hoisted above the `match`. A
        // draining node may still **submit** — the grant at the door is `{Submit, Deliver}` and
        // hosting is the separate one — so a laptop being closed can still ask the desktop to do
        // the work. Refusing at the top would take that away, and it is the thing this product is
        // named for.
        None => match hosting_refusal(ctx, AGENT_TIER, req.demand) {
            Some(reason) => Err(reason),
            None => ctx
                .supervisor
                .submit(req, capacity)
                .await
                .map(|run| Placed {
                    run,
                    elsewhere: None,
                    waiting: None,
                    // A fleet of one. Saying so on every submission would be noise, so this is
                    // reported as durable: there is no second machine to be missing, which is a
                    // different thing from having failed to use it.
                    durable: true,
                    queued: None,
                    preference: None,
                })
                .map_err(|e| e.to_string()),
        },
    }
}

/// Continue a finished run with a new one (ADR-0064). A submission in every respect but where
/// its spec comes from, so it passes the same three gates in the same order as [`submit_run`].
pub(crate) async fn continue_run(
    ctx: &Ctx,
    req: crate::api::ContinueRequest,
) -> Result<Placed, String> {
    if let Some(reason) = submit_grant_refusal(ctx) {
        return Err(reason);
    }
    let run = build_continuation(ctx, req).await?;
    // The parent's grants came with the spec, and the fleet may have lost the service since.
    if let Some(reason) = unreachable_resource(ctx, &run.spec.resources) {
        return Err(reason);
    }
    let capacity =
        offload_core::Capacity::of(&ctx.config.work_policy(ctx.capabilities.now().device_class));
    match &ctx.cluster {
        Some(cluster) => place(cluster, ctx, run).await,
        None => match hosting_refusal(ctx, AGENT_TIER, run.spec.demand) {
            Some(reason) => Err(reason),
            None => ctx
                .supervisor
                .submit_built(run, capacity)
                .await
                .map(|run| Placed {
                    run,
                    elsewhere: None,
                    waiting: None,
                    durable: true,
                    queued: None,
                    preference: None,
                })
                .map_err(|e| e.to_string()),
        },
    }
}

/// A continuation, built here or with its base from the node that has the parent's checkout.
///
/// Routed the way `offload logs` is: the node that wrote the parent's last position ran its last
/// leg, and a finished run's worktree is on that leg's machine ([`log_source`]). When that is a
/// peer, only the **base** is asked of it, and the run is made here — so its home is this node
/// and `--prefer here` means the machine the person is typing at, not the one that happened to
/// hold the parent's checkout. Before this, a continuation typed anywhere else was refused with a
/// sentence sending the person to that machine, for a request this node could forward.
async fn build_continuation(
    ctx: &Ctx,
    req: crate::api::ContinueRequest,
) -> Result<offload_core::Run, String> {
    let members = members(ctx);
    let parent = find_run(ctx, &req.run).ok();
    let leg = parent
        .as_ref()
        .and_then(|parent| ctx.supervisor.progress_leg(parent.id));
    let (Some(cluster), Some(parent), Some(leg)) = (&ctx.cluster, parent, leg) else {
        return local_continuation(ctx, req, &members).await;
    };
    if leg == ctx.node_id {
        return local_continuation(ctx, req, &members).await;
    }
    // Refused without asking when the view already says the machine is gone: a dial to a dead
    // peer is not refused, it times out, and a person at the keyboard waited the whole window
    // to be told what `offload nodes` already knew — measured, 30s. Suspect is still asked,
    // because a suspect node usually answers (the reason `fetch_blob` asks one). Draining is
    // too: a node on its way out is still up, and building a base writes nothing there.
    if let Some(gone) = cluster.view().node(&leg).and_then(|n| match n.status {
        offload_core::NodeStatus::Dead => Some("dead"),
        offload_core::NodeStatus::Departed => Some("gone from the fleet"),
        _ => None,
    }) {
        return Err(format!(
            "run {} cannot be continued yet: {}, which ran its last leg, is {gone}, and its \
             workspace is there and nowhere else — continue it once that machine is back",
            parent.id.short(),
            node_name(cluster, leg)
        ));
    }
    let base = cluster
        .continuation_base_at(
            leg,
            parent.id,
            req.mode == offload_core::ContinueMode::Session,
            CONTINUE_BASE_WINDOW.max(offload_core::Millis(ctx.config.cluster.bid_window_ms)),
        )
        .await
        .map_err(|reason| format!("run {} cannot be continued: {reason}", parent.id.short()))?;
    ctx.supervisor
        .continuation_from(req, &parent, base, &members)
        .map_err(|e| e.to_string())
}

async fn local_continuation(
    ctx: &Ctx,
    req: crate::api::ContinueRequest,
    members: &[(offload_core::NodeId, String)],
) -> Result<offload_core::Run, String> {
    ctx.supervisor
        .build_continuation(req, members)
        .await
        .map_err(|e| e.to_string())
}

/// How long a peer is given to build a continuation's base. A capture rather than a lookup — a
/// bundle of the parent's branch and a patch of its checkout — so longer than a bid round, and
/// short enough that a person at the keyboard hears about a machine that has gone quiet.
const CONTINUE_BASE_WINDOW: offload_core::Millis = offload_core::Millis(30_000);

/// Continue a run that stopped, here, now — the operator's half of ADR-0004.
///
/// **A resume starts an agent, so it answers the same standing questions a submission does**
/// (ADR-0047). It asked none of them: this handler built a `Room` and went straight to
/// [`Supervisor::resume`], so the only gate was the queue. Measured on one daemon — a node
/// saying `accepting no — node is not accepting work`, a node saying `accepting no — drained;
/// restart offloadd to take work again`, and a **revoked** node whose run ADR-0044 had just
/// failed with *this node was revoked from its fleet, so it stopped running it* — each resumed
/// the run at its own socket and went on taking turns. The last of those is the one that matters
/// most: the sentence a revoked node prints on that failed run names `offload resume` as the way
/// forward, so the report was pointing at the hole.
///
/// **After `resolve`, and before everything else.** The run id is the request; the refusal is
/// about the node — so a typo still earns `no such run` rather than a lecture about the drain,
/// and everything `Supervisor::resume` knows about *this run* (no checkpoint, turn limit spent,
/// agent still shutting down) is asked only once the node has agreed it may host anything at all.
///
/// The refusal is not one of the ones `Supervisor::resume` returns, deliberately: it is reported
/// before the recovery bookkeeping is touched, and `stop_recovering` still runs only on the way
/// out. A refused resume must not hand the run a fresh retry budget — that rule is the one this
/// function's own history is about, one caller further in.
async fn resume_run(
    ctx: &Ctx,
    run: &str,
    prompt: Option<String>,
) -> Result<offload_core::RunId, String> {
    let id = ctx.supervisor.resolve(run).map_err(|e| e.to_string())?;
    // About *this* run, since ADR-0019 §4: a policy can permit a light run and refuse an
    // ordinary one, so a door that asked the node-level question would refuse work its owner
    // allows — or, in the other direction, admit work they do not. A row that has gone missing
    // between the resolve and here falls back to the ordinary question, which is the stricter
    // of the two and is answered properly a few lines down by the resume itself.
    let asked_about = ctx.supervisor.run(id);
    let (tier, demand) = asked_about
        .as_ref()
        .map_or((AGENT_TIER, ORDINARY_RUN), |run| {
            (run.spec.work.tier(), run.spec.demand)
        });
    if let Some(reason) = hosting_refusal(ctx, tier, demand) {
        return Err(reason);
    }

    let capacity =
        offload_core::Capacity::of(&ctx.config.work_policy(ctx.capabilities.now().device_class));
    // A resume is another agent starting, so it answers to the account's ceiling like any
    // other. Nothing to count on a node with no fleet in view.
    let room = offload_core::Room::new(
        capacity,
        ctx.cluster.as_ref().and_then(|cluster| {
            cluster
                .view()
                .account_use(&ctx.node_id, &AgentKind::ClaudeCode)
        }),
        // A resume spawns an agent, so it waits on the account's rate limit like any other
        // start — supplied by `Supervisor::room`, not here (ADR-0029).
        None,
    );

    ctx.supervisor
        .resume(id, prompt, room)
        .await
        .map_err(|e| e.to_string())?;

    // A person has looked at it, so whatever this node had decided about retrying it is no
    // longer the answer — and the run gets a fresh retry budget, which is the honest reading of
    // somebody having intervened. Before `Recovering::decided` existed this happened by the
    // escalate branch deleting the entry, which meant it applied only to a run already given up
    // on.
    //
    // **On the way out, not on the way in.** It ran before the resume for as long as it
    // existed, so a resume that was *refused* cleared the entry too — and `stop_recovering`
    // removes it outright, so the run then fell out of the recovery bookkeeping for good: no
    // `recovery` line in `offload explain`, and the tick never looked at it again because the
    // tick iterates that map. Measured on a run stopped by its turn limit, which cannot be
    // resumed at all, so *every* attempt hit the refusal and erased the one place the reason was
    // written down. The same erasure was reachable before this by the four refusals above it —
    // including "its agent is still shutting down here; try again in a moment", which asks the
    // operator to do the thing that does the damage. `docs/pitfalls/lifecycle-and-recovery.md`
    // already says a fresh retry budget must not be a side effect; this is that rule one caller
    // further out, where the side effect was a *failed* request's. The refusals ADR-0047 added
    // are above this for the same reason.
    ctx.supervisor.stop_recovering(id);
    Ok(id)
}

/// Every node `node=<name>` may name (ADR-0063 §3): the view's members by the names they gossip,
/// which since session eighty-two are their certificates' — the names `offload nodes` prints, so
/// the name somebody reads there is the name they can type. This node is always among them, under
/// its configured name when it has no view or is somehow not in its own.
///
/// **Including nodes that are down**, deliberately: `--prefer node=desktop --hold 17:00` is typed
/// precisely while the desktop is asleep, and refusing it as unknown would make the one scenario
/// the hold exists for impossible to express.
pub(crate) fn members(ctx: &Ctx) -> Vec<(offload_core::NodeId, String)> {
    let mut out: Vec<(offload_core::NodeId, String)> = ctx
        .cluster
        .as_ref()
        .map(|cluster| {
            cluster
                .view()
                .nodes
                .values()
                .map(|n| (n.id, n.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    if !out.iter().any(|(id, _)| *id == ctx.node_id) {
        out.push((ctx.node_id, ctx.config.name.clone()));
    }
    out
}

/// Resolve a rule's `--require` and `--prefer` to node ids in place, before it is stored.
///
/// A hold is refused rather than carried: it is an absolute instant, and a rule fires for ever.
fn pin_rule_placement(ctx: &Ctx, work: &mut crate::api::RuleWork) -> Result<(), String> {
    let members = members(ctx);
    let (require, prefer, hold_until) = match work {
        crate::api::RuleWork::Agent(r) => (&mut r.require, &mut r.prefer, r.hold_until),
        crate::api::RuleWork::Task(r) => (&mut r.require, &mut r.prefer, r.hold_until),
    };
    if hold_until.is_some() {
        return Err(
            "a rule cannot hold out for a node: a hold ends at a stated time, and a rule \
             fires for ever — use --prefer, or --require for a pin"
                .to_string(),
        );
    }
    for wanted in [require, prefer] {
        *wanted = offload_core::Wanted::of(
            wanted
                .resolve(ctx.node_id, &members)
                .map_err(|e| e.to_string())?,
        );
    }
    Ok(())
}

/// Run the bid round for a run that has already been built.
///
/// Takes the `Run` rather than the request because both tiers arrive here — an agent run and a
/// task are placed by the same round, against the same view, and the only thing that differs is
/// what each one's constraint asks for. Building outside means neither caller has to know how
/// the other's submission is shaped.
async fn place(
    cluster: &Arc<offload_cluster::Cluster>,
    ctx: &Ctx,
    run: offload_core::Run,
) -> Result<Placed, String> {
    let id = run.id;

    // This node bids like any other — usually winning, because the workspace is warm here and
    // that is worth more than a machine twice the size.
    let mine = cluster.host().evaluate(&run).await;
    let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);

    match cluster.place(&run, mine, window).await {
        offload_cluster::Placement::Accepted {
            node,
            name,
            starting,
            opinions,
        } => {
            let preference = preference_note(&run.spec, node, &opinions, |id| {
                cluster
                    .view()
                    .node(&id)
                    .map_or_else(|| id.short(), |n| n.name.clone())
            });
            // A run that went to a peer is already on a second machine; one this node kept is
            // on exactly one, and the next thing that happens to a laptop at 23:00 is that it
            // closes. `offload run` waits for the copy rather than reporting a success the
            // fleet cannot honour (ADR-0014).
            let durable = if node == ctx.node_id {
                // **The record as it stands, not the copy we had before the round.** `run` is
                // what `build` produced — `Pending`, epoch 0, nobody holding it — and accepting
                // moved every one of those. `confirm_record` publishes what it is given into
                // this node's own view, so handing it the stale copy erased the acceptance
                // `NodeHost::accept` had just published, for as long as it took the next tick to
                // republish the store: measured on one daemon capped at one run, a second
                // submission a moment later was told it could start **now** on a node that was
                // already full, and a third was told two runs were ahead of it when three were.
                // That is exactly the fact `accept`'s own comment says it publishes early
                // because the view is what the next bid is answered from.
                //
                // From the store because the store is the truth for a run we hold, and it is
                // where `take_run` has just written it. The fallback is only reachable if the
                // row vanished between accepting and reading it back, and it is the old
                // behaviour rather than a failure.
                let accepted = ctx.supervisor.run(id).unwrap_or_else(|| run.clone());
                cluster.confirm_record(&accepted, window).await.is_some()
            } else {
                true
            };
            Ok(Placed {
                run: id,
                elsewhere: (node != ctx.node_id).then_some(name),
                // Only when it is not "now": the common case needs no words, and a sentence
                // that appears on every submission is one nobody reads on the submission that
                // matters.
                waiting: (!starting.is_now()).then(|| starting.to_string()),
                durable,
                queued: None,
                preference,
            })
        }
        offload_cluster::Placement::Refused { refusals, spent } => {
            let mut reasons = String::new();
            for refusal in refusals {
                reasons.push_str(&format!("\n  {:<12} {}", refusal.name, refusal.reason));
            }
            if !run.spec.queue {
                return Err(format!(
                    "no node will take this run{reasons}\n  (use --queue to leave it pending \
                     anyway)"
                ));
            }

            // The deliberate case (ADR-0014): leave it pending and let somebody pick it up
            // when things change. It has to be *written down* and gossiped to be picked up at
            // all — a run only this process remembers is one nobody will ever bid for — and
            // the same copy-off-this-machine rule applies, more strongly: nobody is holding it.
            //
            // **The run as the round left it**, which is `place`'s own `spent` and is `run` only
            // when the round handed nothing out. A round that granted and was not confirmed has
            // moved the epoch past every token it issued, deliberately, and writing this copy
            // instead would un-spend them — in the store *and* back into the view, over the
            // record `place` had just published for exactly this reason.
            let queued = spent.map_or_else(|| run.clone(), |spent| *spent);
            ctx.supervisor
                .record_run(&queued)
                .map_err(|e| format!("could not queue the run: {e}"))?;
            cluster.confirm_record(&queued, window).await;
            Ok(Placed {
                run: id,
                elsewhere: None,
                waiting: None,
                durable: true,
                queued: Some(reasons),
                preference: None,
            })
        }
    }
}

/// Put an agent's question to a person, and wait for the answer.
///
/// The one blocking handler in this protocol, and the three ways out are the reason it is written
/// as a `select!` rather than a timeout:
///
/// * **Answered.** The verdict goes back to the hook in the words the person used.
/// * **Nobody answered in time.** Not a denial ([`offload_core::Answer::Unanswered`]): nothing
///   was granted, so the agent's own rules decide — which for a gated command is a refusal and
///   for a harmless one is not. That is exactly what happens on every run in the fleet today,
///   which is what makes this safe to fail at.
/// * **The hook went away.** The agent was cancelled, or the run migrated out from under it.
///   Noticed rather than waited for, the same way an abandoned `logs -f` is: without it the
///   question stays in front of an operator long after anything could act on their answer.
async fn ask(
    ctx: &Ctx,
    run: &str,
    epoch: u64,
    tool_use_id: &str,
    tool: &str,
    detail: &str,
    client: &mut Client,
) -> Response {
    let Ok(id) = ctx.supervisor.resolve(run) else {
        return decision(
            offload_core::Answer::Unanswered,
            format!("offload does not know a run called {run}"),
        );
    };

    // Is there anybody this could reach? A question that stalls a run for minutes and is then
    // answered by the clock is worse than the denial it replaced. The same predicate the
    // submission was answered with, so the two cannot say different things about one fleet.
    let reachable = crate::deliver::can_reach_a_person(
        &ctx.config,
        ctx.cluster.as_ref().map(|cluster| cluster.view()).as_ref(),
        &ctx.store,
    );

    let waiting = match ctx.supervisor.ask(
        id,
        offload_core::Epoch(epoch),
        tool_use_id,
        tool,
        detail,
        reachable,
    ) {
        Ok(waiting) => waiting,
        Err(reason) => {
            // Logged at debug rather than warn: "an existing grant covers this" is the ordinary
            // case on a run with an allowlist, and it happens once per tool call.
            tracing::debug!(run_id = %id, %tool, %reason, "not putting a question to anybody");
            return decision(offload_core::Answer::Unanswered, reason.to_string());
        }
    };

    let patience = std::time::Duration::from_millis(waiting.patience.0);
    let mut answer = waiting.answer;
    tokio::select! {
        verdict = &mut answer => match verdict {
            Ok(verdict) => {
                let outcome = if verdict.allow {
                    offload_core::Answer::Allowed
                } else {
                    offload_core::Answer::Denied
                };
                ctx.supervisor.asked_and_done(id, tool_use_id, outcome, &verdict.by, tool);
                decision(outcome, format!("{outcome} by {}", verdict.by))
            }
            // The sender was dropped without an answer, which means the registry replaced this
            // question — a hook that asked twice about the same call.
            Err(_) => decision(
                offload_core::Answer::Unanswered,
                "this question was replaced by a newer one".to_string(),
            ),
        },
        () = tokio::time::sleep(patience) => {
            let by = format!("nobody answered within {}", waiting.patience);
            ctx.supervisor.asked_and_done(id, tool_use_id, offload_core::Answer::Unanswered, &by, tool);
            decision(offload_core::Answer::Unanswered, by)
        }
        () = hung_up(client) => {
            let by = "the agent stopped waiting".to_string();
            ctx.supervisor.asked_and_done(id, tool_use_id, offload_core::Answer::Unanswered, &by, tool);
            decision(offload_core::Answer::Unanswered, by)
        }
    }
}

/// Answer a question, wherever the agent that is blocked on it happens to be (ADR-0017).
///
/// Local first, then forwarded to the holder — the mirror of a `SpecEdit` going to the field's
/// owner, and for a stricter reason: an edit is a record that could in principle be reconciled
/// anywhere, while a blocked *process* exists on exactly one machine. Answering on the laptop for
/// a run on the desktop is the whole point; the alternative is walking to the desktop.
///
/// The holder comes from the run record rather than from a search. A run nobody holds has no
/// agent and therefore no blocked call, so "nothing is waiting" is the honest answer.
async fn answer(ctx: &Ctx, run: &str, tool_use_id: Option<String>, allow: bool) -> Response {
    let record = match find_run(ctx, run) {
        Ok(record) => record,
        Err(message) => return Response::Error { message },
    };

    // Held here: the registry is in this process, and no round trip is needed to find that out.
    if record.holder() == Some(ctx.node_id) {
        return match ctx
            .supervisor
            .answer(record.id, tool_use_id.as_deref(), allow, "an operator")
        {
            Ok(answered) => Response::Answered {
                tool: answered.tool,
                detail: answered.detail,
                allowed: answered.allowed,
            },
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        };
    }

    let (Some(holder), Some(cluster)) = (record.holder(), &ctx.cluster) else {
        return Response::Error {
            message: format!(
                "nobody is running {} right now, so nothing is waiting for an answer",
                record.id.short()
            ),
        };
    };
    if let Some(why) = holder_out_of_reach(cluster, &record, holder) {
        return Response::Error {
            message: format!(
                "{} holds run {} and is {why}, so nothing can reach the agent waiting on that \
                 question — it will be reclaimed or reassigned",
                node_name(cluster, holder),
                record.id.short()
            ),
        };
    }
    let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
    match cluster
        .answer_at(holder, record.id, tool_use_id, allow, window)
        .await
    {
        Ok((tool, detail, allowed)) => Response::Answered {
            tool,
            detail,
            allowed,
        },
        Err(message) => Response::Error { message },
    }
}

/// Why a forward to `holder` would reach nobody, if this node already knows it would.
///
/// A dial to a dead peer is not refused, it times out: measured, `offload checkpoint` and `offload
/// approve` each waited the whole bid window and then said the holder *"is running this run, did
/// not answer"* — about a run `offload ps` listed as `orphaned`, on a node `offload nodes` listed as
/// `dead`. `offload cancel` already refused an orphaned run at once; this is that check, widened to
/// the view's own verdict on the machine (`build_continuation`'s), and shared so the three doors
/// cannot drift. Suspect and draining nodes are still asked: both are usually up.
fn holder_out_of_reach(
    cluster: &offload_cluster::Cluster,
    record: &offload_core::Run,
    holder: offload_core::NodeId,
) -> Option<&'static str> {
    match cluster.view().node(&holder).map(|n| n.status) {
        Some(offload_core::NodeStatus::Dead) => return Some("dead"),
        Some(offload_core::NodeStatus::Departed) => return Some("gone from the fleet"),
        _ => {}
    }
    matches!(record.state, offload_core::RunState::Orphaned { .. }).then_some("out of contact")
}

/// What to call a node in a sentence somebody reads: its own name, or its short id when this
/// node has never met it.
fn node_name(cluster: &offload_cluster::Cluster, node: offload_core::NodeId) -> String {
    cluster
        .view()
        .node(&node)
        .map_or_else(|| node.short(), offload_core::NodeView::display_name)
}

/// Ask a run to checkpoint and give itself up, wherever in the fleet it is.
///
/// [`cancel`]'s sibling with the record half removed. A cancel has two kinds of target because a
/// run nobody holds is still a record somebody owns; a checkpoint has one, because what is being
/// asked for is a pause in a *process* at a moment only that process reaches (ADR-0004). A run
/// nobody holds has no turn boundary coming and is already in the pool, which is where a
/// checkpoint would have put it — so that is the answer rather than a forward into nowhere.
///
/// It was the last command that acted only where it was typed. Its refusal was not false — "it is
/// not running here, so there is no turn boundary coming" — but it named this machine and stayed
/// silent about the one the run is on, which leaves the operator with nowhere to go.
async fn checkpoint(ctx: &Ctx, run: &str) -> Response {
    let record = match find_run(ctx, run) {
        Ok(record) => record,
        Err(message) => return Response::Error { message },
    };
    let id = record.id;

    // **Before the holder, because it is not a question about a machine.** A task has no agent,
    // no conversation and no turn boundary (ADR-0019 §2), so there is nothing to capture wherever
    // it is running — and asking a peer to work that out is a round trip for a fact this node is
    // holding. `Run::request_checkpoint` refuses it as well, which is the mechanism's own door;
    // this is the one that words a sentence. Same shape as `no_checkout_here` asking about a task
    // before it asks about a leg.
    if record.spec.work.kind() != offload_core::WorkKind::Agent {
        return Response::Error {
            message: format!(
                "run {} is a task — it has no agent, no conversation and no turn boundary, so there is nothing for `offload checkpoint` to capture. A failed task is restarted from its spec rather than resumed (ADR-0058); `offload cancel` is what stops one",
                id.short()
            ),
        };
    }

    let Some(holder) = record.holder() else {
        return Response::Error {
            message: format!(
                "nobody is running {} — it is {}, so there is no turn boundary coming and \
                 nothing to hand back",
                id.short(),
                record.state.name()
            ),
        };
    };

    if holder == ctx.node_id {
        return match ctx
            .supervisor
            .request_checkpoint(id, offload_core::GivenUp::Parked)
        {
            Ok(()) => Response::CheckpointRequested {
                run: id.to_string(),
                node: None,
            },
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        };
    }

    let Some(cluster) = &ctx.cluster else {
        return Response::Error {
            message: format!(
                "run {} is on {}, and this node is not in a mesh, so there is no way to ask",
                id.short(),
                holder.short()
            ),
        };
    };
    if let Some(why) = holder_out_of_reach(cluster, &record, holder) {
        return Response::Error {
            message: format!(
                "{} holds run {} and is {why}, so nothing can reach its agent to stop it at a turn \
                 boundary — it will be reclaimed or reassigned",
                node_name(cluster, holder),
                id.short()
            ),
        };
    }
    let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
    match cluster.checkpoint_at(holder, id, window).await {
        Ok(()) => Response::CheckpointRequested {
            run: id.to_string(),
            node: Some(node_name(cluster, holder)),
        },
        Err(message) => Response::Error { message },
    }
}

/// Stop a run, wherever in the fleet it is.
///
/// The last operator command that acted on the machine it was typed at rather than the machine
/// the run is on. It read this node's *process table*, which cannot tell a run that is elsewhere
/// from a run that is here and has not started yet, and answered both with "run ab12… is not
/// running" — a false sentence about a run spending money on the desktop, and a false sentence
/// about a commitment this node was holding.
///
/// So the target comes from the record, and there are two kinds of it — which is the one thing
/// this routing has that `answer` does not:
///
/// * a run somebody is **holding** is on that node, agent or no agent, and only the holder may
///   write its terminal state;
/// * a run nobody holds is a **record**, and a record belongs to the node that arbitrates it —
///   the same `arbiter_for` a `SpecEdit` uses, for the same reason (two nodes deciding locally
///   is two nodes undoing each other).
///
/// A run whose holder has gone quiet is refused rather than forwarded into a timeout: `Orphaned`
/// is the arbiter noticing, not a decision (ADR-0007), and an agent may well still be running
/// there. Cancelling the record from here would leave it running with the fleet convinced
/// otherwise — the same trade `arbiter_for` makes when it prefers a stalled run to a duplicated
/// one. The hold-down either reclaims it or reassigns it, and the cancel works on whoever
/// answers next.
async fn cancel(ctx: &Ctx, run: &str) -> Response {
    let record = match find_run(ctx, run) {
        Ok(record) => record,
        Err(message) => return Response::Error { message },
    };
    let id = record.id;

    let apply_here = || async {
        // This node's own name rather than "here": the log is read on other machines and after
        // the fact, where "here" names nothing. And for that same reason it is the name on the
        // **certificate** rather than the one in this node's config, which defaults to the
        // machine's hostname — `cancelled from fedora`, in a log another device reads, on a
        // fleet where two devices can carry that hostname. See `fleet_name`.
        match ctx.supervisor.cancel_run(id, &fleet_name(ctx)).await {
            Ok(note) => {
                // Published rather than left to the next tick, for `edit_spec`'s reason: the
                // next decision about this run reads the view, and a pending run cancelled a
                // moment ago is one a bid round would otherwise still offer.
                if let Some(cluster) = &ctx.cluster {
                    if let Some(cancelled) = ctx.supervisor.run(id) {
                        cluster.publish_run(cancelled);
                    }
                }
                Response::Cancelled {
                    run: id.to_string(),
                    node: None,
                    note,
                }
            }
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        }
    };

    if let Some(holder) = record.holder() {
        if holder == ctx.node_id {
            return apply_here().await;
        }
        let out_of_reach = match &ctx.cluster {
            Some(cluster) => holder_out_of_reach(cluster, &record, holder),
            None => matches!(record.state, offload_core::RunState::Orphaned { .. })
                .then_some("out of contact"),
        };
        if let Some(why) = out_of_reach {
            // Named, not numbered. This printed `21177c09`, while `offload explain` two lines
            // later and `offload nodes` one command away both called the same machine `bravo` —
            // one screen, two spellings, and the unreadable one was in the sentence the operator
            // has to act on. `node_name` falls back to the id for a node nobody has met, which is
            // the only case where a number is the honest answer.
            let holder_name = ctx
                .cluster
                .as_ref()
                .map_or_else(|| holder.short(), |cluster| node_name(cluster, holder));
            return Response::Error {
                message: format!(
                    "{holder_name} holds run {} and is {why}, so a cancel cannot reach its agent yet — it will be reclaimed or reassigned, and cancelling then reaches whoever answers",
                    id.short()
                ),
            };
        }
        let Some(cluster) = &ctx.cluster else {
            return Response::Error {
                message: format!(
                    "run {} is on {}, and this node is not in a mesh, so there is no way to reach it",
                    id.short(),
                    holder.short()
                ),
            };
        };
        let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
        return match cluster.cancel_at(holder, id, window).await {
            Ok(note) => Response::Cancelled {
                run: id.to_string(),
                node: Some(node_name(cluster, holder)),
                note,
            },
            Err(message) => Response::Error { message },
        };
    }

    // Nobody holds it, so what is being stopped is the record.
    let Some(cluster) = &ctx.cluster else {
        return apply_here().await;
    };
    match cluster.view().arbiter_for(&record) {
        Some(owner) if owner == ctx.node_id => apply_here().await,
        Some(owner) => {
            let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
            match cluster.cancel_at(owner, id, window).await {
                Ok(note) => Response::Cancelled {
                    run: id.to_string(),
                    node: Some(node_name(cluster, owner)),
                    note,
                },
                Err(message) => Response::Error { message },
            }
        }
        // Guessing an owner for a record is how two nodes each decide something different
        // about it — the same refusal `edit_spec` makes, for the same reason.
        None => Response::Error {
            message: format!(
                "nobody is running {} and this node has never met {}, which owns its record",
                id.short(),
                record.home.short()
            ),
        },
    }
}

fn decision(answer: offload_core::Answer, reason: String) -> Response {
    Response::Decision { answer, reason }
}

/// Find a run by id or prefix, in the store first and then in what the fleet has gossiped.
///
/// Both halves are needed and neither is enough. The store is this node's own record — the
/// runs it holds, plus the ones it submitted and remembers placing elsewhere — while gossip
/// carries runs it has merely heard about, which is the state a third node is in when
/// somebody asks it why a run is stuck.
/// Which node serves this run's log.
///
/// A run nobody holds has **two** ways of getting there, and one fallback used to cover both.
///
/// *Finished*: the lease went with the terminal transition, so the record names nobody, and the
/// log is on whichever machine actually ran it. That machine is the one that last wrote the run's
/// *position* — `RunProgress::by`, added for the merge's ranking (ADR-0005's second amendment) and
/// answering this too, because the leg that produced the numbers is by construction the leg that
/// wrote the log. Falling through to the arbiter resolved to a node that never had the log, and
/// when the arbiter was the asking node it printed **nothing at all**: exit 0, no output, which
/// reads as "this run produced none". An overnight run that finished on the desktop was invisible
/// from the laptop it was submitted on. Measured on two daemons — 56 lines there, 0 here.
///
/// *Pending*: nothing has run it, so there is no leg, and its **arbiter** is right — that is the
/// node offering it and the node that writes down that it is not going to make its deadline
/// (ADR-0013). Reporting nothing for it would hide the one message a queued run ever produces,
/// which is why the fallback exists and why it stays.
///
/// A pure function because the resolution is otherwise reachable only through a live mesh, which
/// is the same reason `power::settle` and `Selection::summary` are ones.
fn log_source(
    run: &offload_core::Run,
    progress_leg: Option<NodeId>,
    arbiter: Option<NodeId>,
) -> Option<NodeId> {
    run.holder()
        .or_else(|| run.state.is_terminal().then_some(progress_leg).flatten())
        .or(arbiter)
}

/// Which machine this run's checkout was made on, as far as this node's record can tell.
///
/// The three answers `offload rm` has to tell apart, because a checkout is on exactly one
/// machine and the command acts only where it is typed — deliberately, unlike every other
/// run-targeted command, since a directory cannot be torn down over the network.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Leg<'a> {
    /// Nobody ever wrote a position for this run, so no node ever started it and no checkout was
    /// ever made anywhere. Not the same as "not here": there is nothing to go looking for.
    Never,
    /// This node ran it.
    Here,
    /// A peer ran it, under the name the operator would see it by elsewhere.
    Peer(&'a str),
}

/// The one word the worktree summary uses for a checkout its own node has torn down.
///
/// Matched on rather than guessed at: it is written by `Supervisor::note_workspace` and gossips,
/// so it is the same string on every node in the fleet.
const REMOVED: &str = "removed";

/// Why this node has no checkout to discard, in words somebody can act on.
///
/// **Four causes reached one two-branch sentence**, and only one of the branches was ever right.
/// Walked on two daemons: a *task* — which has no workspace on any machine by construction
/// (ADR-0019 §2) — a run *cancelled before it ever started*, a checkout *already removed here*,
/// and a run that finished *on a peer* were each told "it was removed already, or the run
/// finished on another machine, where `offload rm` is what discards it". Three of those four are
/// false in both branches, and the fourth named no machine while the answer sat in this node's
/// own `runs` row: `RunProgress::by`, which [`log_source`] one function up already reads to
/// route `offload logs` to the machine that ran it. A refusal that sends somebody hunting for a
/// machine the daemon could have named is the shape `checkpoint`'s own doc comment describes —
/// "true and useless" — met again on the one command that is right to stay local.
///
/// A pure function for the reason the `agent claude-code , model` line became one: wording that
/// lives inside the arm that prints it is wording no test can fail on.
fn no_checkout_here(
    run: &offload_core::Run,
    leg: &Leg<'_>,
    note: Option<&str>,
    reclaimed: Option<offload_core::Reclamation>,
) -> String {
    let id = run.id.short();
    // Asked first, because it is the only one of these that is not about a machine at all.
    // `RunSpec::workspace` is an `Option` precisely so this absence is a consequence of the kind
    // of work rather than something a submission left out — and `cleanup` flattens it to `""` one
    // line before anything has to word a sentence about it.
    if run.spec.workspace().is_none() {
        return format!(
            "run {id} is a task — it has no workspace on any machine, so there is no checkout for `offload rm` to discard"
        );
    }
    // The worktree summary is the fleet's one agreed statement about that directory, written by
    // the leg it is on and gossiped from there — so `removed` is the same answer wherever it is
    // read, and it settles the question before the machine is worth naming. Without it, a node
    // whose peer had already torn a checkout down would send somebody to that machine to type a
    // command that has nothing left to do: the second arm below, right until the first rm.
    let gone = note == Some(REMOVED);
    match (leg, gone) {
        (Leg::Never, _) => format!(
            "run {id} never started on any machine, so no checkout was ever made for `offload rm` to discard"
        ),
        (Leg::Here, true) => format!(
            "no checkout for {id} on this node — it ran here and its worktree has already been removed"
        ),
        // Ran here, and this node never said it removed anything. The first draft pointed at
        // `offload audit {id}` and said it *"says what happened to it"* — a promise the log
        // keeps only when the sweep was what took it, and the sweep runs on the leg that has
        // **lost** the run ([`Supervisor::note_reclaimed`]'s own doc comment), which renders
        // `Leg::Peer` rather than this arm. Measured: a checkout gone with the run's last leg
        // still here printed `granted` and `accepted` and not one word about a directory. So the
        // row is *read* rather than named, in the audit log's own wording, and its absence is
        // said out loud — "nobody here recorded a teardown" is what distinguishes a leg that
        // died before a checkout was made from one that went through a door.
        (Leg::Here, false) => match reclaimed {
            Some(why) => format!(
                "no checkout for {id} on this node — it ran here and this node reclaimed its worktree: {}",
                why.why()
            ),
            None => format!(
                "no checkout for {id} on this node — it ran here, its worktree is gone and nothing here recorded a teardown: the leg died before one was made, or it went from outside offload"
            ),
        },
        (Leg::Peer(name), true) => format!(
            "no checkout for {id} on this node — it ran on {name}, where its worktree has already been removed"
        ),
        (Leg::Peer(name), false) => format!(
            "no checkout for {id} on this node — it ran on {name}, and `offload rm` there is what discards it"
        ),
    }
}

/// Resolve what an operator typed to a run: this node's store first, then the fleet's view.
///
/// The second half is not a duplicate of the first. The store holds the runs this node has
/// written down and the view holds every run it has *heard of*, so a run placed on a peer a
/// second ago is findable here and nowhere else — which is what makes `offload explain <a run on
/// the desktop>` work from the laptop.
///
/// **What it must not be is a second copy of the *rule*.** `Store::resolve_run` decides what may
/// be an abbreviation of a run id — non-empty, hex — with the reasoning beside it, and this
/// fallback re-implemented the prefix match without either check. It is reached exactly when the
/// store said no, so the guard was missing precisely where it was the only one: `"".starts_with
/// ("")` is true of every run, and on a node with one run in view `offload cancel ""` stopped it.
/// Measured on two daemons; `offload explain`, `offload checkpoint` and `offload audit` come
/// through here too, and `offload logs`, `offload rm` and `offload resume` — which do not — were
/// refusing the same argument correctly one command away. `offload_core::needle` is the one rule
/// now, asked here before the scan rather than inside it.
fn find_run(ctx: &Ctx, needle: &str) -> Result<offload_core::Run, String> {
    let local = ctx
        .supervisor
        .resolve(needle)
        .map_err(|e| e.to_string())
        .and_then(|id| {
            ctx.supervisor
                .run(id)
                .ok_or_else(|| format!("run {} is not in this node's registry", id.short()))
        });
    let Err(reason) = local else {
        return local;
    };

    let Some(cluster) = &ctx.cluster else {
        return Err(reason);
    };
    // The store's own refusal, kept: it is the one that says *why* this is not an id, and
    // re-wording it here would be the second copy arriving as a sentence instead of as a match.
    let offload_core::Needle::Prefix(needle) = offload_core::needle(needle) else {
        return Err(reason);
    };
    let view = cluster.view();
    let mut matches = view
        .runs
        .values()
        .filter(|run| run.id.to_string().starts_with(&needle));

    match (matches.next(), matches.next()) {
        (Some(run), None) => Ok(run.clone()),
        // Ambiguity is reported rather than resolved by picking one: run ids are UUIDv7, so
        // every run submitted in the same minute shares its leading characters.
        //
        // …and the candidates are named, in full, for `Store::resolve_run`'s reason: a
        // scheduled occurrence's displayed id is the tick, so "more than one run matches" can
        // be said about an id there is no longer version of anywhere on the operator's screen.
        (Some(a), Some(b)) => Err(format!(
            "more than one run matches {needle} — {} or {}",
            a.id, b.id
        )),
        _ => Err(reason),
    }
}

/// Ask every node what it makes of a run, and assemble the answer.
///
/// The canvass costs one bounded round — the same one a submission runs, granting nothing —
/// because bids describe *now*: showing somebody the answers from the round that placed the
/// run would be showing them what the fleet thought at 09:00 to explain what it is doing at
/// midnight. A finished run is not canvassed at all; there is nothing to place.
async fn explain_run(ctx: &Ctx, run: offload_core::Run) -> crate::api::Explanation {
    let now = now_millis();
    let policy = offload_core::ReassignPolicy::default();

    let (view, opinions, not_canvassed) = match &ctx.cluster {
        // "it is cancelled", not "it cancelled": `name()` is an adjective and two of the three
        // terminal states read as verbs without the copula — *it cancelled* puts the run in the
        // subject position of something it did not do. One word, and the line it lands in is
        // `nobody was asked to take it: …`.
        Some(cluster) if run.state.is_terminal() => (
            cluster.view(),
            Vec::new(),
            Some(format!("it is {}", run.state.name())),
        ),
        Some(cluster) => {
            let mine = cluster.host().evaluate(&run).await;
            let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
            let canvassed = cluster.canvass(&run, mine, window).await;
            (
                cluster.view(),
                crate::explain::opinions(&run, canvassed),
                None,
            )
        }
        // A fleet of one is a supported way to run, so the question still has an answer:
        // everything about the run is true, and there was simply nobody to ask.
        None => (
            local_view(ctx, now),
            Vec::new(),
            Some("this node is not in a mesh, so there was nobody to ask".to_string()),
        ),
    };

    let watchers = ctx.supervisor.watchers(run.id);
    // Why a run *we* hold has not started, asked of the same gate that would start it. Only for a
    // run this node holds and has not begun: the arithmetic is this machine's occupancy and its own
    // account's rate limit, and neither is something another node's copy of the record can answer.
    let held_back = matches!(run.state, offload_core::RunState::Assigned { .. })
        .then(|| run.holder() == Some(ctx.node_id))
        .unwrap_or(false)
        .then(|| {
            let room = offload_core::Room::new(
                offload_core::Capacity::of(
                    &ctx.config.work_policy(ctx.capabilities.now().device_class),
                ),
                ctx.cluster.as_ref().and_then(|cluster| {
                    run.spec
                        .agent_kind()
                        .and_then(|kind| cluster.view().account_use(&ctx.node_id, kind))
                }),
                None,
            );
            ctx.supervisor.start_refusal(&run, room)
        })
        .flatten()
        .map(|refusal| crate::supervisor::describe_refusal(&refusal));
    // What the run is stopped waiting for a person to answer (ADR-0017), from the registry
    // `offload asks` reads rather than from the log: the log says a question was *asked*, and
    // whether it is still waiting is exactly what the registry knows and the log does not.
    //
    // **Asked of the fleet when this node is not the holder**, which this used to refuse to do.
    // The reason given was `held_back`'s, carried one step further — a blocked process exists on
    // exactly one machine — and it was borrowed from the wrong neighbour. `held_back` is
    // *arithmetic*: this machine's occupancy and its own account's rate limit, which no other
    // node's copy of the record could compute. A pending question is not arithmetic; it is a fact
    // the holder can be **asked** for, and `offload asks` has asked the whole fleet for exactly
    // this since ADR-0017 landed. So the two commands disagreed on one node: measured on two
    // daemons, `offload explain` on the peer said *"only alpha can tell; it is running there"*
    // with no mention of a question at all, while `offload asks` on that same socket printed the
    // tool, the clock and `offload approve` — and the approve travels, so the person reading the
    // silent screen was one command away from being able to answer it.
    //
    // One canvass, bounded by the same window as the opinions round above, and skipped for a
    // terminal run for that round's reason: there is no live question on a run that has stopped.
    let waiting: Vec<offload_core::PendingAsk> = if run.holder() == Some(ctx.node_id) {
        ctx.supervisor
            .asks()
            .into_iter()
            .filter(|ask| ask.run == run.id)
            .collect()
    } else if let Some(cluster) = ctx.cluster.as_ref().filter(|_| !run.state.is_terminal()) {
        cluster
            .canvass_asks(offload_core::Millis(ctx.config.cluster.bid_window_ms))
            .await
            .into_iter()
            .filter(|ask| ask.run == run.id)
            .collect()
    } else {
        Vec::new()
    };
    crate::explain::explain(
        &view,
        &run,
        now,
        &policy,
        crate::explain::Observed {
            watchers,
            opinions,
            not_canvassed,
            held_back,
            waiting,
            // What this node remembers about retrying it. Only this node can answer — the
            // observation was taken here at the moment the run failed and is gossiped nowhere.
            recovery: ctx.supervisor.recovery_state(run.id),
            turns: ctx.supervisor.turns_taken(run.id),
            // What a person typing `offload resume` at this node would be told, from the door
            // that would tell them. `recovery` above is the other half and not this one: it
            // says what the node does *unasked*, and a run left for a person because nobody
            // could be seen watching is one a person is then invited to pick up — an invitation
            // a drained, revoked or policy-refusing node declines (ADR-0047).
            resume_refusal: hosting_refusal(ctx, run.spec.work.tier(), run.spec.demand),
            // Read from the same supervisor the recovery tick asks, so the explanation and the
            // decision cannot disagree about whether this node will pick the run back up.
            node: crate::explain::NodeStanding {
                departing: ctx.supervisor.is_draining(),
                revoked: ctx.supervisor.is_revoked(),
                // The same question, off the same live handle, as the `accepting` line and the
                // two doors that start runs. A second way of computing it here is how a report
                // comes to disagree with the decision it describes (ADR-0049).
                hosting: hosting_allowed(ctx, &run),
            },
        },
    )
}

/// Change a run's deadline or priority, at whichever node owns that.
///
/// The owner is the run's arbiter — its home node until that node is *gone*, then the
/// deterministic successor — so this is `arbiter_for` rather than a second rule that would
/// disagree with it on the day the home node dies. An operator may type the command anywhere;
/// what they must not do is have two nodes each write revision 1 (ADR-0013).
///
/// A node with no mesh owns everything it knows about, which is not a special case so much as
/// the same rule with one node in the fleet.
async fn edit_spec(ctx: &Ctx, run: &offload_core::Run, edit: offload_core::SpecEdit) -> Response {
    let apply_here = || match ctx.supervisor.edit_spec(run.id, edit) {
        Ok((amended, note)) => {
            if let Some(cluster) = &ctx.cluster {
                cluster.publish_run(amended.clone());
            }
            Response::SpecEdited {
                run: amended.id.to_string(),
                // The value *before* the edit, which only the node that applied it knows: the
                // amended record would report every priority change as a no-op.
                summary: summarize_edit(edit, Some(run.spec.priority), now_millis()),
                note,
            }
        }
        Err(e) => Response::Error {
            message: e.to_string(),
        },
    };

    let Some(cluster) = &ctx.cluster else {
        return apply_here();
    };

    match cluster.view().arbiter_for(run) {
        Some(owner) if owner == ctx.node_id => apply_here(),
        Some(owner) => {
            let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
            match cluster.edit_spec_at(owner, run.id, edit, window).await {
                Ok(note) => Response::SpecEdited {
                    run: run.id.to_string(),
                    // Said from what we asked for rather than from our own copy of the record:
                    // the owner has written it down, and our copy catches up at the next
                    // gossip tick a second from now. `None` for the same reason — this node's
                    // idea of the old priority is a tick old, and "unchanged" would be a claim
                    // about a value it did not read.
                    summary: summarize_edit(edit, None, now_millis()),
                    note,
                },
                Err(message) => Response::Error { message },
            }
        }
        // Nobody arbitrates it here, which is what a node says about a run whose home it has
        // never met — and guessing an owner is exactly how two nodes write revision 1.
        None => Response::Error {
            message: format!(
                "this node has never met {}, which owns this run's deadline and priority",
                run.home.short()
            ),
        },
    }
}

/// What the edit did, in the terms the operator asked in.
/// `was` is the value being replaced, and `None` means this node does not know it — the case
/// whenever the edit was applied somewhere else, because the only copy here is a gossip tick
/// old. "Unchanged" from that would be a claim about a value nobody read; the amended record
/// would be worse still, since it reports every priority change as a no-op.
fn summarize_edit(
    edit: offload_core::SpecEdit,
    was: Option<i32>,
    now: offload_core::Millis,
) -> String {
    match edit {
        offload_core::SpecEdit::Deadline { at: deadline } => due_at(deadline, now),
        offload_core::SpecEdit::Priority { to } if was == Some(to) => {
            format!("priority {to}, unchanged")
        }
        offload_core::SpecEdit::Priority { to } => format!("priority {to}"),
    }
}

fn due_at(deadline: Option<offload_core::Millis>, now: offload_core::Millis) -> String {
    match deadline {
        Some(at) => offload_core::Slack::between(at, now).to_string(),
        None => "as soon as a node can take it".to_string(),
    }
}

/// The view a node with no mesh has: itself, alive, arbitrating its own runs.
fn local_view(ctx: &Ctx, now: offload_core::Millis) -> offload_core::ClusterView {
    let mut view = offload_core::ClusterView::new(ctx.node_id);
    // One read, for `hosting_refusal`'s reason: the capabilities in the view and the policy
    // chosen for them must be answers about the same probe.
    let caps = ctx.capabilities.now();
    view.upsert_node(
        offload_core::NodeView::new(
            ctx.node_id,
            (*caps).clone(),
            ctx.config.work_policy(caps.device_class),
            now,
        )
        // The fleet-facing name, like the cluster's own entry: this synthesises the local node's
        // view where there is no mesh, and a run's record made here is read elsewhere later.
        .named(fleet_name(ctx)),
    );
    view
}

fn now_millis() -> offload_core::Millis {
    offload_core::Millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
    )
}

/// What an evicted device is told, wherever the eviction is noticed.
///
/// A `const` because two callers state it and they must not drift: [`grant_refusal`] reads it out
/// of `fleet.json`, where the revocation is recorded, and [`revoked_refusal`] falls back to it
/// from the in-memory latch when that file cannot be read. Same fact, two sources, one sentence.
const REVOKED: &str = "this node has been revoked from its fleet — it can host nothing and submit nothing until it is enrolled again";

/// Why this node may not do `grant`, if it may not.
///
/// Read from `fleet.json` at each request rather than cached at startup: `offload grant`
/// writes that file, and a grant that needs a daemon restart to take effect is a grant
/// somebody will think is broken.
///
/// A node with no fleet is unrestricted. That is not a loophole — it is phases 1 and 2, where
/// the only thing a run can be placed on is the machine you typed the command into.
fn grant_refusal(ctx: &Ctx, grant: offload_core::Grant) -> Option<String> {
    let state = crate::fleet::load(&ctx.config.state_dir).ok().flatten()?;
    let now = now_millis();

    if state.grants(now).contains(&grant) {
        return None;
    }
    // Asked first, because `grants` now returns nothing for a revoked node and every other
    // sentence below would tell an evicted device to run `offload grant`, which cannot work and
    // is not what happened to it (ADR-0044).
    if state.is_revoked(state.membership.member) {
        return Some(REVOKED.to_string());
    }
    Some(match state.membership.probation_until(now) {
        // The same words the mesh's refusal uses, from the same function: this copy divided by
        // sixty itself and printed "for another 0 minutes" in the last minute (measured, the phone
        // app's probation, session ninety-two).
        Some(until) if grant == offload_core::Grant::HostRuns => format!(
            "this node may not host runs yet: host-runs is granted but on probation for {}",
            offload_core::fleet::dormant_for(until.saturating_sub(now))
        ),
        _ => format!("this node has not been granted {grant} — run `offload grant {grant}`"),
    })
}

/// Whether this node may ask the fleet to run something at all.
///
/// Named rather than reached through `grant_refusal` directly, because there are two callers now
/// and they are asking for different reasons: a person at a keyboard, and a rule this node wrote
/// down earlier and is about to act on with nobody watching (ADR-0020 §5).
pub(crate) fn submit_grant_refusal(ctx: &Ctx) -> Option<String> {
    grant_refusal(ctx, offload_core::Grant::Submit)
}

fn host_runs_refusal(ctx: &Ctx) -> Option<String> {
    grant_refusal(ctx, offload_core::Grant::HostRuns)
}

/// The eviction, said from whichever source can still say it — and `None` only when there is
/// none.
///
/// Asked **above the drain** by both callers, because `stand_down` sets *both* latches
/// (ADR-0044) and the drain's sentence is advice that cannot work on an evicted device:
/// restarting `offloadd` on a machine the fleet has thrown out changes nothing. `status` had
/// this ordering and its reasoning from the beginning; [`hosting_refusal`] did not, so every
/// door it guards told a revoked device's owner to restart it. Measured on a revoked
/// fleet-of-one: `offload resume` answered *this node is draining; restart offloadd to take
/// work again* one line below a run that had failed with *this node was revoked from its
/// fleet*, while `offload run` — the submit door, which reaches `host_runs_refusal` through the
/// mesh rather than through here — named the revocation. Both orderings refuse, which is why it
/// survived; only one of them says what happened. A wrong cause confidently stated is what
/// ADR-0043 deleted `NodeIsDeparting` for.
///
/// Two sources and no gap between them. The **latch** decides that there is an answer, and
/// `fleet.json` — where the revocation is recorded, and what every other door reads — supplies
/// the sentence; where that file cannot be read the constant does, because a revoked node
/// falling through to the clauses below would be told it would host. Unknown is not good news
/// anywhere here, and least of all at the doors whose good news is a second agent in somebody's
/// repo.
/// What this device is called to the fleet. See [`crate::fleet::display_name`], which is the one
/// answer — this is the `Ctx`-shaped way to ask it.
fn fleet_name(ctx: &Ctx) -> String {
    crate::fleet::display_name(&ctx.config.state_dir, &ctx.config.name)
}

fn revoked_refusal(ctx: &Ctx) -> Option<String> {
    ctx.supervisor
        .is_revoked()
        .then(|| host_runs_refusal(ctx).unwrap_or_else(|| REVOKED.to_string()))
}

/// The standing questions this node has to answer before it starts an agent, in
/// `Host::evaluate`'s order — and the one place both doors that can start one ask them.
///
/// This node leaving comes **first of all**, because it is neither a capability question nor a
/// policy one; then the grant that says whether it may host, which is also where a revoked
/// device is turned away (ADR-0044); and then the owner's own standing answer.
///
/// **It exists because each clause was found missing separately, on a different door.** The
/// history is the argument for keeping one copy:
///
/// * The drain half was missing from the no-cluster *submission* arm, and a fleet of one is the
///   case where it is the whole feature: there is no bid to refuse and no grant to decline, so
///   `offload drain` set a flag that nothing on that path read. Measured — drained, `offload
///   status` saying `accepting no`, and the next submission accepted and started anyway.
/// * **The owner's policy was missing too** (ADR-0046), with the same symptom word for word: a
///   node reporting `accepting no — network is metered and policy disallows it` accepted a
///   submission at its own socket and ran it to completion.
/// * **And `offload resume` asked none of the three** (ADR-0047). A resume starts an agent
///   exactly the way a submission does, and its handler asked only `Room::for_one_more` — the
///   queue. Measured on one daemon, three times: `accepting no — node is not accepting work`,
///   `accepting no — drained; restart offloadd to take work again`, and a **revoked** node whose
///   run ADR-0044 had just failed with *this node was revoked from its fleet, so it stopped
///   running it* — each of which then resumed the run at its own socket and went on taking turns.
///
/// `permits` rather than `admits`: the queue half of the owner's policy is answered by the
/// supervisor, whose `Room::for_one_more` sees the device ledger and the account's rate limit as
/// well — and on a fleet of one, being full is a refusal rather than ADR-0006's commitment,
/// because there is no round to commit to. What was missing on both doors is the half that does
/// not empty by itself.
///
/// `ClaudeCode` because that is the only agent either door can name: `SubmitRequest` carries no
/// agent field and `Supervisor::build` writes `AgentKind::ClaudeCode` into every `RunSpec`, so a
/// resumed run's own agent is that value too. This is one of the places that has to change on the
/// day `build` learns a second one — `allowed_agents` is asked about *an* agent, and asking about
/// the wrong one would refuse the work its owner allowed.
/// Why this node cannot be the one to run a task for `service`, if it cannot.
///
/// Separate from [`hosting_refusal`] because it is a different sentence about a different
/// thing: that one answers "may this node host runs at all", and a node perfectly able to host
/// runs can still have no *working* program for what was asked for. Naming what it *does* offer,
/// so the answer is actionable rather than merely correct — the commonest cause is a service
/// spelled differently in `node.toml`, and the next is a `command` that is not there.
///
/// Three states, because a `bool` here answered `true` to a `[[tasks]]` entry naming a program
/// this device does not have: such a run was accepted, started and failed a millisecond later,
/// which is the outcome ADR-0014 exists to prevent and which this function was written to
/// prevent for the *other* cause (see ADR-0019's amendment).
fn task_refusal(ctx: &Ctx, service: &offload_core::Service) -> Option<String> {
    // The capability the bid round would place on, so the fleet-of-one door and the round
    // cannot answer one question two ways. `Nomination` is three-valued because this used to
    // ask `task_for(..).is_some()` — true of a `[[tasks]]` entry naming a program that is not
    // there, which was then accepted, started and failed a millisecond later.
    match crate::task::nomination(&ctx.capabilities.now(), &ctx.config, service) {
        crate::task::Nomination::Ready => return None,
        crate::task::Nomination::NoProgram(cfg) => {
            // The **measured** cause, not a guess at it: a program that is there without its
            // execute bit is not a program that is missing, and the first cut of this said it
            // was — about a file sitting right there. `Program::why` is the one function that
            // answers, the same one the probe set the capability's bit from; asked again here
            // rather than second-guessed, and falling back to the bare fact on the one race it
            // has (the program arriving in the seconds since the last probe, where the gate
            // still refuses and there is nothing left to name).
            let why = crate::resource::lookup(&cfg.command)
                .why()
                .unwrap_or("this node cannot run it");
            return Some(format!(
                "this node nominates `{service}`, and {why}: {}. Nothing here can run it until \
                 that changes — `offload status` lists what this node can.",
                cfg.command
            ));
        }
        crate::task::Nomination::Nothing => {}
    }
    // What it offers is what it can actually *run*: this list is an invitation, and naming a
    // second task whose program is also missing sends somebody from one broken entry to
    // another. Measured from the same capabilities as the arm above, for that arm's reason.
    let caps = ctx.capabilities.now();
    let offered: Vec<String> = ctx
        .config
        .tasks
        .iter()
        .filter_map(|cfg| cfg.service().ok())
        .filter(|s| {
            matches!(
                crate::task::nomination(&caps, &ctx.config, s),
                crate::task::Nomination::Ready
            )
        })
        .map(|s| s.to_string())
        .collect();
    Some(if ctx.config.tasks.is_empty() {
        format!(
            "this node nominates no tasks at all, so it cannot run `{service}`. \
             Add a [[tasks]] entry to node.toml."
        )
    } else if offered.is_empty() {
        format!(
            "this node nominates no task for `{service}`, and it cannot run any of the tasks \
             it does nominate — `offload status` says why, one line each."
        )
    } else {
        format!(
            "this node nominates no task for `{service}`. It offers: {}",
            offered.join(", ")
        )
    })
}

/// A `Ctx` for a test in another module of this crate.
///
/// Here rather than duplicated in `schedule.rs`, because a second one would be a second set of
/// defaults to keep in step with this file's — and what a test of the firing pass needs is
/// exactly what the socket gets.
#[cfg(test)]
pub(crate) fn test_ctx(dir: &std::path::Path, node_id: NodeId) -> Ctx {
    let config = Arc::new(Config {
        state_dir: dir.to_path_buf(),
        ..Config::default()
    });
    let store = offload_store::Store::open(&dir.join("state.db")).expect("store");
    Ctx {
        models: std::sync::Arc::default(),
        supervisor: Supervisor::new(config.clone(), node_id, store.clone()).with_private_ledger(),
        store,
        config,
        node_id,
        capabilities: Arc::new(crate::deliver::Current::new(
            offload_core::Capabilities::empty(
                offload_core::Os::Linux,
                offload_core::Arch::X86_64,
                offload_core::DeviceClass::Desktop,
            ),
        )),
        cluster: None,
        mesh: None,
        triggers: crate::trigger::Triggers::new(),
    }
}

/// The agent tier, for the callers that are asking about an agent run and nothing else.
///
/// A `static` because [`offload_core::Tier`] borrows the agent it names — the borrow is what
/// makes `Tier::Agent` unconstructible without one, which is the whole reason it is not an
/// `Option`.
static CLAUDE_CODE: offload_core::AgentKind = offload_core::AgentKind::ClaudeCode;
const AGENT_TIER: offload_core::Tier<'static> = offload_core::Tier::Agent(&CLAUDE_CODE);

/// What a *node-level* answer is about, beside [`AGENT_TIER`]: an ordinary run.
///
/// For the lines that describe the machine rather than a run — `offload status`'s `accepting`,
/// and the standing resume refusal that travels beside a listing. Since ADR-0019 §4 a policy
/// can answer differently for light work, so "would this node take work" is no longer one
/// question, and these lines answer the commonest one. Named so that is a decision rather than
/// an accident; where a run is in hand, its own demand is what to ask about.
const ORDINARY_RUN: offload_core::Demand = offload_core::Demand::Normal;

/// Would this node host work of `tier` and `demand` right now, and if not, why?
///
/// `tier` because the owner's standing answer has one clause that is about an *agent* rather
/// than about the device — `allowed_agents` — and a task is not an unlisted agent. A node whose
/// owner listed some other agent would otherwise refuse a shell script for not being that
/// agent, on the one path that has no bid round to ask properly (ADR-0019 §1).
///
/// `demand` because since ADR-0019 §4 three of those clauses have a *light* form, so "would
/// this node take work" is no longer one question. Where a run is in hand its own demand is
/// what to ask; where there is not — a node-level line in a listing — the honest question is
/// about an ordinary run, and [`NODE_LEVEL`] is that, named so the choice is visible.
///
/// Every caller says which it is asking about, which is the point of naming them: there is
/// still **one** function, so the door and the sentence a report prints cannot disagree.
fn hosting_refusal(
    ctx: &Ctx,
    tier: offload_core::Tier<'_>,
    demand: offload_core::Demand,
) -> Option<String> {
    // Eviction above the drain, and the reasoning is at `revoked_refusal` because `status` asks
    // the same question in the same place. This function had the two the other way round, so
    // every door it guards told a revoked device's owner to restart `offloadd`.
    revoked_refusal(ctx)
        .or_else(|| {
            ctx.supervisor
                .is_draining()
                .then(|| "this node is draining; restart offloadd to take work again".to_string())
        })
        .or_else(|| host_runs_refusal(ctx))
        .or_else(|| owner_policy_refusal(ctx, tier, demand).map(|refusal| refusal.to_string()))
}

/// Why the keep-awake tick is holding the machine awake, as `offload status` reports it (ADR-0077).
/// `None` while it is not.
static AWAKE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Why this machine should not idle-sleep now, or why it may (ADR-0077): it holds a run, or it
/// would take one — `hosting_refusal`, the rule behind `offload status`'s `accepting` line, for an
/// agent run and then for a program. A node the owner said may not take work, or the fleet has not
/// let host, is left to sleep, as a machine that is not a host should be.
fn stay_awake(ctx: &Ctx) -> Result<String, String> {
    let held = ctx.supervisor.held_count();
    if held > 0 {
        return Ok(format!("it is holding {held} run(s)"));
    }
    match hosting_refusal(ctx, AGENT_TIER, offload_core::Demand::default()) {
        None => Ok("it may take agent runs".to_string()),
        Some(agent) => {
            match hosting_refusal(ctx, offload_core::Tier::Task, offload_core::Demand::Light) {
                None => Ok("it may take programs".to_string()),
                Some(_) => Err(agent),
            }
        }
    }
}

/// Hold the machine awake while [`stay_awake`] says so, re-asked every 30 s: the same cadence as
/// the probe, since what it reads (the battery, the policy, a drain) changes no faster.
async fn keep_awake(ctx: Ctx, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    if !offload_power::supported() {
        return;
    }
    let mut held: Option<offload_power::KeepAwake> = None;
    let mut refused_once = false;
    loop {
        match (held.is_some(), stay_awake(&ctx)) {
            (false, Ok(why)) => match offload_power::KeepAwake::hold(&format!("Offload: {why}")) {
                Ok(assertion) => {
                    tracing::info!(reason = %why, "keeping this machine awake");
                    held = Some(assertion);
                    *AWAKE
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why);
                }
                Err(e) if !refused_once => {
                    refused_once = true;
                    tracing::warn!(error = %e, "could not keep this machine awake; it may sleep and drop out");
                }
                Err(_) => {}
            },
            (true, Err(why)) => {
                held = None;
                *AWAKE
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                tracing::info!(reason = %why, "letting this machine sleep again");
            }
            (true, Ok(why)) => {
                *AWAKE
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why);
            }
            (false, Err(_)) => {}
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// Whether this node should ask the fleet to leave it mostly alone (ADR-0078): the device is set
/// down — on battery with its screen off, each as its host said — **and** holds no run. A run held
/// here keeps the ordinary rate, since the arbiter's patience with it is what fences it; unknown
/// facts are not quiet. Reasons, like [`stay_awake`], so the log says why it changed.
fn quiet(ctx: &Ctx) -> Result<&'static str, &'static str> {
    let held = ctx.supervisor.held_count();
    if held > 0 {
        return Err("it is holding a run");
    }
    // Whatever its age: see `last_host_facts` for why stale is the expected state here.
    match crate::deliver::last_host_facts(&ctx.config.state_dir) {
        Some(facts) if facts.quiet() => Ok("on battery with the screen off, holding nothing"),
        Some(_) => Err("charging, or its screen is on"),
        None => Err("its host says nothing about power"),
    }
}

/// Re-ask [`quiet`] every 15 s and tell the cluster when the answer changes. Each tick is one small
/// file read, and a suspended phone runs no timers at all; the host rewrites its facts the moment
/// the screen comes on, so waking up costs at most one tick plus the quiet probe interval.
async fn hibernate(ctx: Ctx, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let Some(cluster) = ctx.cluster.clone() else {
        return;
    };
    loop {
        let answer = quiet(&ctx);
        if cluster.set_quiet(answer.is_ok()) {
            match answer {
                Ok(why) => tracing::info!(
                    reason = why,
                    "going quiet: probing and probed about once a minute"
                ),
                Err(why) => tracing::info!(
                    reason = why,
                    "no longer quiet: probing at the ordinary rate"
                ),
            }
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(15)) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// The owner's standing answer on its own, without the drain or the grant.
///
/// Split out because three callers need three different amounts of it: [`hosting_refusal`] wants
/// all three questions, `offload explain` wants this one *separately* — `departing` and `revoked`
/// are their own fields on `Circumstances` and folding them in here would answer one question
/// three times — and the recovery tick wants the same fact as a [`offload_core::Hosting`].
///
/// One read of the handle, not two: the device class and the battery it is asked about have to
/// come from the same probe, or a re-probe landing between them would answer with half of each
/// (ADR-0048).
fn owner_policy_refusal(
    ctx: &Ctx,
    tier: offload_core::Tier<'_>,
    demand: offload_core::Demand,
) -> Option<offload_core::Refusal> {
    let caps = ctx.capabilities.now();
    ctx.config
        .work_policy(caps.device_class)
        .permits(&caps, tier, demand)
        .err()
}

/// …as the recovery tick and `offload explain` ask it (ADR-0049), about one run.
///
/// The run rather than the node, since ADR-0019 §4: one policy can permit a light watcher and
/// refuse an expensive agent session, so "will this node host" has to be asked about something.
pub(crate) fn hosting_allowed(ctx: &Ctx, run: &offload_core::Run) -> offload_core::Hosting {
    if owner_policy_refusal(ctx, run.spec.work.tier(), run.spec.demand).is_none() {
        offload_core::Hosting::Allowed
    } else {
        offload_core::Hosting::RefusedByOwner
    }
}

async fn dispatch(
    request: Request,
    ctx: Ctx,
    write: &mut tokio::net::unix::OwnedWriteHalf,
    client: &mut Client,
) -> std::io::Result<()> {
    match request {
        Request::Status => {
            reply(write, &Response::Status(Box::new(status(&ctx)))).await?;
        }

        Request::Drain => {
            // Streamed rather than answered once (ADR-0035). Everything this prints used to
            // arrive after the pass, which meant `offload drain` printed **nothing at all** for
            // up to five minutes when a run was blocked mid-turn on an unanswered question —
            // measured: forty seconds of silence, an `offload asks` in the next terminal naming
            // the question, and `offload status` reporting `waiting 1 run(s) stopped for an
            // answer` about the wait its own drain was in.
            let (steps, mut incoming) = tokio::sync::mpsc::channel(16);
            let report = crate::mesh::Report::to(steps);
            // Both are `Some` or both are `None` — the cluster is read out of the mesh — but the
            // tuple is what `no_fleet` has always been decided by, and a drain reports which case
            // it was in.
            let fleet = match (&ctx.cluster, &ctx.mesh) {
                (Some(_), Some(mesh)) => Some(mesh.as_ref()),
                _ => None,
            };
            let no_fleet = fleet.is_none();
            let pass = crate::mesh::depart(
                &ctx.supervisor,
                fleet,
                std::time::Duration::from_secs(ctx.config.cluster.drain_deadline_secs),
                offload_core::Millis(ctx.config.cluster.bid_window_ms),
                // This node stops accepting and stays up, so a run still mid-turn when the
                // deadline expires is one it can still hand over when the turn ends.
                crate::mesh::Lifespan::StaysUp,
                &report,
            );
            tokio::pin!(pass);
            // Both numbers come from the pass itself. `left` used to be `held_count()`, asked
            // after the drain had returned — a capacity question, blind to the run this drain
            // had just *released*, which is the one outcome worth reporting.
            let drained = loop {
                tokio::select! {
                    Some(step) = incoming.recv() => {
                        reply(write, &Response::Draining(step)).await?;
                    }
                    outcome = &mut pass => break outcome,
                }
            };
            // Whatever the last poll queued while the pass was finishing. Dropped steps would be
            // the *first* lines of the report, since a fast drain says everything at once.
            while let Ok(step) = incoming.try_recv() {
                reply(write, &Response::Draining(step)).await?;
            }
            reply(
                write,
                &Response::Drained {
                    moved: u32::try_from(drained.moved).unwrap_or(u32::MAX),
                    left: u32::try_from(drained.left).unwrap_or(u32::MAX),
                    finished: u32::try_from(drained.finished).unwrap_or(u32::MAX),
                    later: u32::try_from(drained.later).unwrap_or(u32::MAX),
                    pooled: u32::try_from(drained.pooled).unwrap_or(u32::MAX),
                    handed_back: u32::try_from(drained.handed_back).unwrap_or(u32::MAX),
                    no_fleet,
                    no_boundary: u32::try_from(drained.no_boundary).unwrap_or(u32::MAX),
                },
            )
            .await?;
            reply(write, &Response::Done).await?;
        }

        Request::Nodes => {
            let now = now_millis();
            let (nodes, local) = match &ctx.cluster {
                Some(cluster) => {
                    let view = cluster.view();
                    // Hosting is decided on the certificate a node presented (the bid round
                    // checks `host-runs` on it), so that is where this reads it from. This node's
                    // own is its own grant (`host_runs_refusal`).
                    let at = now;
                    let hosts: std::collections::HashMap<offload_core::NodeId, bool> = cluster
                        .certificates()
                        .into_iter()
                        .map(|cert| (cert.member, cert.granted(offload_core::Grant::HostRuns, at)))
                        .collect();
                    let me = cluster.node();
                    let nodes = view
                        .nodes
                        .values()
                        .map(|n| {
                            let mut summary = crate::mesh::summarize(n, now);
                            summary.may_host = if n.id == me {
                                Some(host_runs_refusal(&ctx).is_none())
                            } else {
                                hosts.get(&n.id).copied()
                            };
                            summary
                        })
                        .collect();
                    (nodes, Some(cluster.node().to_string()))
                }
                None => (Vec::new(), None),
            };
            reply(write, &Response::Nodes { nodes, local }).await?;
        }

        Request::RefreshModels => {
            // The reader raises the fleet's count itself once it starts, so a request made here
            // is read once, here, and not again when the count comes back (ADR-0080).
            ctx.models.ask();
            reply(
                write,
                &Response::ModelsAsked {
                    fleet: ctx.cluster.is_some(),
                },
            )
            .await?;
        }

        Request::ResourceLine { .. } => {
            // Only meaningful after `UseResource`, which never returns to this dispatcher.
            reply(
                write,
                &Response::Error {
                    message: "no resource is open on this connection".into(),
                },
            )
            .await?;
        }

        Request::UseResource { run, service } => {
            let Some(mesh) = ctx.mesh.clone() else {
                reply(
                    write,
                    &Response::Error {
                        message: "this node is not in a fleet, so it can reach no resource that \
                                  is not already configured here"
                            .into(),
                    },
                )
                .await?;
                return Ok(());
            };
            let (run, service) = match (
                ctx.supervisor.resolve(&run),
                service.parse::<offload_core::Service>(),
            ) {
                (Ok(run), Ok(service)) => (run, service),
                (Err(e), _) => {
                    reply(
                        write,
                        &Response::Error {
                            message: e.to_string(),
                        },
                    )
                    .await?;
                    return Ok(());
                }
                (_, Err(e)) => {
                    reply(
                        write,
                        &Response::Error {
                            message: e.to_string(),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            };
            proxy_resource(&mesh, run, &service, write, client).await?;
        }

        Request::Audit { run, limit } => {
            // The run id is resolved here rather than in the store, so a prefix works and an
            // ambiguous one is refused by name — `resolve_run`'s rule, which the audit table's
            // own index cannot apply.
            let resolved = match run.as_deref() {
                None => Ok(None),
                Some(needle) => find_run(&ctx, needle).map(|run| Some(run.id)),
            };
            let response = match resolved.and_then(|run| {
                ctx.store
                    .audit(run, limit.clamp(1, 500))
                    .map_err(|e| e.to_string())
            }) {
                Ok(entries) => Response::Audit {
                    entries,
                    names: members(&ctx),
                },
                Err(message) => Response::Error { message },
            };
            reply(write, &response).await?;
        }

        Request::FleetHistory { limit } => {
            // Worded here rather than in the CLI, for `NodeSummary`'s reason: the daemon owns
            // what an event means, and a second renderer is a second thing to keep in step.
            let events = match ctx.store.fleet_events(limit.clamp(1, 500)) {
                Ok(events) => events,
                Err(e) => {
                    reply(
                        write,
                        &Response::Error {
                            message: e.to_string(),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            };
            let entries = events
                .into_iter()
                .map(|(_seq, event)| {
                    let (kind, node, summary) = match &event.kind {
                        offload_core::FleetEvent::Enrolled {
                            node,
                            name,
                            grants,
                            how,
                        } => {
                            let grants = grants
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ");
                            (
                                "enrolled",
                                node.to_string(),
                                match how {
                                    offload_core::Enrolment::Founded => {
                                        format!("{name} founded the fleet — granted {grants}")
                                    }
                                    // Deliberately not "by this device": the same event is
                                    // recorded on the machine that issued the invitation and
                                    // on the one that took it up, and only one of them did the
                                    // inviting.
                                    offload_core::Enrolment::Invited => format!(
                                        "{name} was invited into the fleet — granted {grants}"
                                    ),
                                    offload_core::Enrolment::Passphrase => format!(
                                        "{name} enrolled itself with the passphrase — granted \
                                         {grants}"
                                    ),
                                    offload_core::Enrolment::Met => format!(
                                        "{name} was met for the first time — granted {grants}"
                                    ),
                                },
                            )
                        }
                        offload_core::FleetEvent::PassphraseUsed { what, node, name } => (
                            "passphrase_used",
                            node.to_string(),
                            format!("the fleet passphrase was used on {name} to {what}"),
                        ),
                    };
                    crate::api::FleetHistoryEntry {
                        at_unix_ms: event.at_unix_ms,
                        kind: kind.to_string(),
                        node,
                        summary,
                    }
                })
                .collect();
            reply(write, &Response::FleetHistory { entries }).await?;
        }

        Request::StoreArchive { path } => {
            // Read, hashed and stored by the blob plane exactly as a checkpoint's bytes are —
            // the hash is computed here rather than taken on trust, which is what makes the
            // digest the caller gets back an authority rather than a claim (ADR-0016).
            let path = std::path::PathBuf::from(&path);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    reply(
                        write,
                        &Response::Error {
                            message: format!("reading the archive {}: {e}", path.display()),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            };
            let size = bytes.len() as u64;
            // Refused here rather than at the far end of a bid round, which is §4's "say the cap
            // before the expensive step" met at the first place that can: every node enforces the
            // same limit, so this is a fact about the file and needs no peer to establish.
            let limit = offload_proto::cluster::MAX_BLOB_BYTES;
            if size > limit {
                reply(
                    write,
                    &Response::Error {
                        message: format!(
                            "that archive is {size} bytes and the fleet moves at most {limit} in \
                             one exchange, so no node could take a copy — it has to be a smaller \
                             selection, not the whole directory (ADR-0061 §4)"
                        ),
                    },
                )
                .await?;
                return Ok(());
            }
            let store = ctx.store.clone();
            let stored = tokio::task::spawn_blocking(move || store.put_blob(&bytes)).await;
            let response = match stored {
                Ok(Ok(hash)) => Response::ArchiveStored {
                    hash: hash.to_string(),
                    bytes: size,
                },
                Ok(Err(e)) => Response::Error {
                    message: format!("storing the archive: {e}"),
                },
                Err(e) => Response::Error {
                    message: format!("storing the archive: {e}"),
                },
            };
            reply(write, &response).await?;
        }
        Request::Sinks { test } => {
            let mut sinks = crate::deliver::report(&ctx.config, &ctx.store);
            if test {
                // Through the real adapter and deliberately *not* through the outbox: a test is
                // not a run event, so it must not be able to mark a real notification sent, and
                // a failure here is reported in place rather than retried for ever.
                let routes = crate::deliver::Sinks::from_config(&ctx.config);
                for report in &mut sinks {
                    let Some(route) = routes.get(&report.id) else {
                        continue;
                    };
                    if let Err(reason) = route.deliver(&crate::deliver::probe_notice()).await {
                        report.unusable = Some(reason);
                    }
                }
            }
            let fleet = ctx.cluster.as_ref().map_or_else(Vec::new, |cluster| {
                crate::deliver::fleet_routes(&cluster.view(), &ctx.store)
            });
            reply(write, &Response::Sinks { sinks, fleet }).await?;
        }

        Request::Watch { service, mut rule } => {
            // `here` and `node=` are resolved **now**, when the rule is written, and stored as
            // ids (ADR-0063 §3): a rule firing at 03:00 on the node that holds the mailbox has
            // to prefer the machine it was typed at, not whichever daemon happens to fire it.
            if let Err(message) = pin_rule_placement(&ctx, &mut rule.work) {
                reply(write, &Response::Error { message }).await?;
                return Ok(());
            }
            // The delivery plane's preconditions, answered here for the same reason
            // `Request::Submit` answers them — and more sharply, because a rule is written once
            // and then fires with nobody present to notice that its news went nowhere. The same
            // three functions, so a rule and a run cannot say different things about one fleet.
            let view = ctx.cluster.as_ref().map(|cluster| cluster.view());
            // The two halves of the delivery plane every rule has, whichever tier it fires: an
            // audience and the routes to honour it. Read off whichever request is in the rule,
            // because they are shared fields (ADR-0019 §2) and a note computed for one tier and
            // not the other is the shape of gap this whole block exists to close.
            let (notify, ask_policy) = match &rule.work {
                crate::api::RuleWork::Agent(request) => (request.notify.clone(), request.ask),
                // A task cannot stop and ask a person: there is no tool call to interrupt and
                // nothing to grant (ADR-0017 is about an agent). `Never` is the fact rather than
                // a default standing in for one.
                crate::api::RuleWork::Task(request) => {
                    (request.notify.clone(), offload_core::AskPolicy::Never)
                }
            };
            let audience = crate::deliver::audience_note(&notify, &ctx.config, view.as_ref());
            let ask = crate::deliver::ask_note(
                ask_policy,
                crate::deliver::can_reach_a_person(&ctx.config, view.as_ref(), &ctx.store),
            );
            let reach = crate::deliver::reach(&notify, &ctx.config, view.as_ref());
            // …and the precondition that is a *refusal* on the neighbouring command, which is why
            // it was the one nobody thought to answer here: `--use` on a fleet with no such
            // resource stops the run being submitted at all, so a rule with one fires, is refused,
            // and produces nothing — while `offload when` printed `Nothing else to do: the next
            // event fires it`.
            //
            // The cheap tier's version of the same sentence sits beside it: a task nobody
            // nominates is a firing that will be refused, and `--task` is the field it is about.
            // Two tiers, one arrangement, because the mistake was never about resources.
            let resources = match &rule.work {
                crate::api::RuleWork::Agent(request) => resource_note(&ctx, &request.resources),
                crate::api::RuleWork::Task(request) => task_note(&ctx, &request.service),
            };
            let fired_by = rule.fired_by;
            match crate::trigger::write_rule(&ctx, &service, rule) {
                Ok((id, trigger_present)) => {
                    reply(
                        write,
                        &Response::Watching {
                            rule: id.to_string(),
                            // A notice-bound rule needs no trigger and never will, so the
                            // question does not arise: `false` here printed *"nothing on this
                            // node watches `failed` — add a [[triggers]] entry"*, which is
                            // advice to nominate a program that would do nothing (ADR-0057).
                            // The plane is what fires it, and the plane is always there.
                            trigger_present: trigger_present
                                || fired_by == crate::api::FiredBy::Notice,
                            fired_by,
                            audience,
                            ask,
                            reach,
                            resources,
                        },
                    )
                    .await?;
                }
                Err(message) => reply(write, &Response::Error { message }).await?,
            }
        }

        Request::Every(spec) => {
            let spec = *spec;
            // The same three notes `Request::Watch` prints, from the same functions — and the
            // comment above that one explains why they were missing here: they were added when
            // somebody walked a *rule*, and nobody had walked a schedule. Every argument for
            // warning applies more strongly here, because `offload every`'s own help promises
            // this does **not** die with the device you type it on. Measured before the fix:
            // `offload every 1m --task nosuch` was accepted in silence, then refused at every
            // tick for ever (`ineligible: has nosuch as execute`) while `offload schedules`
            // said `nothing fired from here yet` — the same sentence as a schedule that has not
            // reached its first tick.
            //
            // Computed before `create_schedule` takes the spec, and read off the spec rather
            // than off what was stored, which is `Request::Watch`'s arrangement too.
            let view = ctx.cluster.as_ref().map(|cluster| cluster.view());
            let notify = spec
                .agent
                .as_ref()
                .map(|request| request.notify.clone())
                .or_else(|| spec.task.as_ref().map(|request| request.notify.clone()))
                .unwrap_or_default();
            let audience = crate::deliver::audience_note(&notify, &ctx.config, view.as_ref());
            let reach = crate::deliver::reach(&notify, &ctx.config, view.as_ref());
            let resources = schedule_precondition(&ctx, &spec);
            match create_schedule(&ctx, spec) {
                Ok((id, next)) => {
                    reply(
                        write,
                        &Response::Scheduled {
                            schedule: id.to_string(),
                            next,
                            audience,
                            reach,
                            resources,
                        },
                    )
                    .await?;
                }
                Err(message) => reply(write, &Response::Error { message }).await?,
            }
        }

        Request::Schedules => match report_schedules(&ctx) {
            Ok(schedules) => reply(write, &Response::Schedules { schedules }).await?,
            Err(message) => reply(write, &Response::Error { message }).await?,
        },

        Request::Unschedule { schedule } => {
            let removed = ctx
                .store
                .resolve_schedule(&schedule)
                .and_then(|id| {
                    ctx.store
                        .remove_schedule(id, offload_store::now_ms())
                        .map(|gone| (id, gone))
                })
                .map_err(|e| e.to_string());
            match removed {
                // **Republished immediately** rather than at the next gossip tick, and this is
                // the one place that matters: a removal is a tombstone, and the second between
                // typing the command and the tick is a second in which a peer could fire it.
                Ok((id, offload_store::schedules::Removal::Done)) => {
                    if let (Some(cluster), Ok(schedules)) = (&ctx.cluster, ctx.store.schedules()) {
                        cluster.publish_schedules(schedules);
                    }
                    reply(
                        write,
                        &Response::Unscheduled {
                            schedule: id.to_string(),
                            already: None,
                        },
                    )
                    .await?;
                }
                // Not an error — the schedule is gone, which is what was asked for — and not a
                // removal either. Nothing is republished, because nothing changed: the tombstone
                // the fleet already has is the one it has to agree on.
                Ok((id, offload_store::schedules::Removal::Already(at))) => {
                    reply(
                        write,
                        &Response::Unscheduled {
                            schedule: id.to_string(),
                            // `"{} ago"` from a `Millis` difference, which is how the schedules
                            // report words `last fired` two hundred lines up.
                            already: Some(format!(
                                "{} ago",
                                crate::supervisor::now().saturating_sub(at)
                            )),
                        },
                    )
                    .await?;
                }
                Ok((id, offload_store::schedules::Removal::Missing)) => {
                    reply(
                        write,
                        &Response::Error {
                            message: format!("no schedule {id} here"),
                        },
                    )
                    .await?;
                }
                Err(message) => reply(write, &Response::Error { message }).await?,
            }
        }

        Request::Rules => match crate::trigger::report_rules(&ctx) {
            Ok(rules) => reply(write, &Response::Rules { rules }).await?,
            Err(message) => reply(write, &Response::Error { message }).await?,
        },

        Request::Unwatch { rule } => {
            let resolved = ctx
                .store
                .resolve_rule(&rule)
                .map_err(|e| e.to_string())
                .and_then(|id| {
                    ctx.store
                        .remove_rule(id)
                        .map_err(|e| e.to_string())
                        .map(|gone| (id, gone))
                });
            match resolved {
                Ok((id, true)) => {
                    // On the way out, because the tag on an occurrence outlives the rule that
                    // wrote it and nothing else will ever ask about it again (ADR-0021 §4). What
                    // survives is what would have survived any firing — a failure, or news still
                    // owed — and it is counted rather than left silent.
                    crate::trigger::prune_occurrences(
                        &ctx,
                        Some(id),
                        None,
                        crate::trigger::Prune::Finally,
                    )
                    .await;
                    let kept = ctx.store.occurrences_kept(id).unwrap_or_default();
                    reply(
                        write,
                        &Response::Forgotten {
                            rule: id.to_string(),
                            kept,
                        },
                    )
                    .await?;
                }
                Ok((id, false)) => {
                    reply(
                        write,
                        &Response::Error {
                            message: format!("rule {id} was already gone"),
                        },
                    )
                    .await?;
                }
                Err(message) => reply(write, &Response::Error { message }).await?,
            }
        }

        Request::Triggers => {
            let triggers = crate::trigger::report_triggers(&ctx);
            reply(write, &Response::Triggers { triggers }).await?;
        }

        Request::List => {
            let view = ctx.cluster.as_ref().map(|cluster| cluster.view());
            let mut runs = ctx.supervisor.list(&|id| {
                view.as_ref()
                    .and_then(|view| view.node(&id))
                    .map_or_else(|| id.short(), |n| n.name.clone())
            });
            runs.sort_by_key(|r| std::cmp::Reverse(r.started_at_unix));
            // The node's standing answer, travelling with the runs because one of the sentences
            // in the listing names `offload resume` and this node may refuse it. Composed here
            // for the reason every other sentence in this file is: the CLI deciding either half
            // would be a second copy of a rule that changes.
            //
            // The refusal is `hosting_refusal`, the function the resume door itself calls. The
            // other half of the pairing — which runs it is about — is `RunSummary::resumable`,
            // per row rather than folded in here, because `offload ps` hides finished runs
            // unless asked and a node-level gate would print advice about a run nobody can see.
            let resume_refusal = hosting_refusal(&ctx, AGENT_TIER, ORDINARY_RUN);
            reply(
                write,
                &Response::Runs {
                    runs,
                    resume_refusal,
                },
            )
            .await?;
        }

        Request::Submit(req) => {
            // Where its news will go, answered before the run is placed and reported whatever
            // the placement turns out to be: a refused submission has no news to route, and a
            // run accepted by a peer is exactly the one whose owner will not be watching.
            let audience = crate::deliver::audience_note(
                &req.notify,
                &ctx.config,
                ctx.cluster.as_ref().map(|cluster| cluster.view()).as_ref(),
            );
            // And the other half of "can this fleet reach you", which is about what the *run*
            // will do rather than who hears about it: `--ask` on a fleet with no route falls
            // through to the agent's own rules, which is the behaviour of not having passed it.
            // Silent until now, at both ends — nothing at submission, and a `debug` line on the
            // holder hours later.
            let ask = crate::deliver::ask_note(
                req.ask,
                crate::deliver::can_reach_a_person(
                    &ctx.config,
                    ctx.cluster.as_ref().map(|cluster| cluster.view()).as_ref(),
                    &ctx.store,
                ),
            );

            let result = submit_run(&ctx, req).await;

            match result {
                Ok(placed) => {
                    reply(
                        write,
                        &Response::Submitted {
                            run: placed.run.to_string(),
                            node: placed.elsewhere,
                            waiting: placed.waiting,
                            here_only: !placed.durable,
                            queued: placed.queued,
                            audience,
                            ask,
                            preference: placed.preference,
                            siblings: Vec::new(),
                        },
                    )
                    .await?;
                }
                Err(message) => {
                    reply(write, &Response::Error { message }).await?;
                }
            }
        }

        Request::SubmitTask(req) => {
            let audience = crate::deliver::audience_note(
                &req.notify,
                &ctx.config,
                ctx.cluster.as_ref().map(|cluster| cluster.view()).as_ref(),
            );
            match submit_task_run(&ctx, req).await {
                Ok(placed) => {
                    reply(
                        write,
                        &Response::Submitted {
                            run: placed.run.to_string(),
                            node: placed.elsewhere,
                            waiting: placed.waiting,
                            // A task has no checkpoint and nothing to replicate, so there is
                            // no copy of it that could exist on only one machine. Saying
                            // "here only" would be a warning about a risk it does not carry.
                            here_only: false,
                            queued: placed.queued,
                            audience,
                            // `--ask` is an agent's channel to a person mid-tool-call; a task
                            // has no tool calls and never asks.
                            ask: None,
                            preference: placed.preference,
                            siblings: Vec::new(),
                        },
                    )
                    .await?;
                }
                Err(message) => {
                    reply(write, &Response::Error { message }).await?;
                }
            }
        }

        // An agent, blocked mid-tool-call, asking whether it may proceed (ADR-0017). The only
        // request in this protocol that waits on a human.
        Request::Ask {
            run,
            epoch,
            tool_use_id,
            tool,
            detail,
        } => {
            let decided = ask(&ctx, &run, epoch, &tool_use_id, &tool, &detail, client).await;
            reply(write, &decided).await?;
        }

        Request::Answer {
            run,
            tool_use_id,
            allow,
        } => {
            let response = answer(&ctx, &run, tool_use_id, allow).await;
            reply(write, &response).await?;
        }

        Request::Asks => {
            // This node's own, plus whatever the fleet is waiting on — asked now rather than
            // remembered, because a question stops existing the moment somebody answers it.
            // Without the fleet half, `offload asks` on the laptop says "nothing is waiting"
            // while the desktop is stopped, which is the confident wrong answer this project
            // spends most of its design avoiding.
            let mut asks = ctx.supervisor.asks();
            if let Some(cluster) = &ctx.cluster {
                let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
                asks.extend(cluster.canvass_asks(window).await);
            }
            reply(write, &Response::Asks { asks }).await?;
        }

        Request::Explain { run } => {
            let response = match find_run(&ctx, &run) {
                Ok(run) => Response::Explanation(Box::new(explain_run(&ctx, run).await)),
                Err(message) => Response::Error { message },
            };
            reply(write, &response).await?;
        }

        Request::SetDeadline { run, deadline } => {
            let edit = offload_core::SpecEdit::Deadline {
                at: deadline.map(offload_core::Millis),
            };
            let response = match find_run(&ctx, &run) {
                Ok(run) => edit_spec(&ctx, &run, edit).await,
                Err(message) => Response::Error { message },
            };
            reply(write, &response).await?;
        }

        Request::SetPriority { run, priority } => {
            let response = match find_run(&ctx, &run) {
                Ok(run) => {
                    edit_spec(
                        &ctx,
                        &run,
                        offload_core::SpecEdit::Priority { to: priority },
                    )
                    .await
                }
                Err(message) => Response::Error { message },
            };
            reply(write, &response).await?;
        }

        Request::Cancel { run } => {
            let response = cancel(&ctx, &run).await;
            let failed = matches!(response, Response::Error { .. });
            reply(write, &response).await?;
            if !failed {
                // `Cancelled` is the whole answer, and the run's own stream is about to end.
                return Ok(());
            }
        }

        Request::Remove { run } => match ctx.supervisor.resolve(&run) {
            Ok(id) => {
                // A checkout is on one machine, and a *finished* run names none — the lease goes
                // with the terminal transition — so this is answered by asking this node's own
                // disk rather than the record. Saying `Done` when nothing was removed is the
                // kind of success that sends somebody looking for reclaimed space that is still
                // in use, on a machine they were not told about.
                match ctx.supervisor.cleanup(id).await {
                    Ok(offload_workspace::Removal::Removed { discarded }) => {
                        reply(
                            write,
                            &Response::Removed {
                                run: id.to_string(),
                                discarded,
                            },
                        )
                        .await?;
                    }
                    Ok(offload_workspace::Removal::NothingHere) => {
                        // The *record* answers why, and it is not the same question the disk was
                        // asked. See [`no_checkout_here`]: three of the four ways of getting
                        // here have nothing to do with another machine, and the fourth can name
                        // the one it is on.
                        let message = match ctx.supervisor.run(id) {
                            Some(record) => {
                                let leg = ctx.supervisor.progress_leg(id);
                                // Named outside the `match` because `Leg::Peer` borrows it: the
                                // name is a `String` the cluster view produces, and it has to
                                // outlive the arm that puts it in the sentence.
                                let peer =
                                    leg.filter(|node| *node != ctx.node_id)
                                        .map(|node| match &ctx.cluster {
                                            Some(cluster) => node_name(cluster, node),
                                            None => node.short(),
                                        });
                                let leg = match (leg, &peer) {
                                    (None, _) => Leg::Never,
                                    (Some(_), None) => Leg::Here,
                                    (Some(_), Some(peer)) => Leg::Peer(peer),
                                };
                                let note = ctx.supervisor.workspace_note(id);
                                // Read, not pointed at: the sentence below says what became of
                                // the checkout, so it has to have the row rather than a command
                                // that may not hold one.
                                let reclaimed = ctx.supervisor.reclaimed_here(id);
                                no_checkout_here(&record, &leg, note.as_deref(), reclaimed)
                            }
                            // Resolved and then gone, which is a store this node no longer has a
                            // row in rather than anything about a checkout.
                            None => format!("run {} is not in this node's registry", id.short()),
                        };
                        reply(write, &Response::Error { message }).await?;
                        return Ok(());
                    }
                    Err(e) => {
                        reply(
                            write,
                            &Response::Error {
                                message: e.to_string(),
                            },
                        )
                        .await?;
                    }
                }
            }
            Err(e) => {
                reply(
                    write,
                    &Response::Error {
                        message: e.to_string(),
                    },
                )
                .await?;
            }
        },

        Request::Checkpoint { run } => {
            let response = checkpoint(&ctx, &run).await;
            let failed = matches!(response, Response::Error { .. });
            reply(write, &response).await?;
            if !failed {
                return Ok(());
            }
        }

        Request::Files { run, path } => {
            let response = match files(&ctx, &run, &path).await {
                Ok(view) => Response::Files { view },
                Err(message) => Response::Error { message },
            };
            reply(write, &response).await?;
        }

        Request::Continue(req) => match continue_run(&ctx, *req).await {
            Ok(placed) => {
                let siblings = ctx.supervisor.continuations_of_parent_of(placed.run);
                reply(
                    write,
                    &Response::Submitted {
                        run: placed.run.to_string(),
                        node: placed.elsewhere,
                        waiting: placed.waiting,
                        here_only: !placed.durable,
                        queued: placed.queued,
                        // Inherited from the parent and said when it was submitted.
                        audience: None,
                        ask: None,
                        preference: placed.preference,
                        siblings,
                    },
                )
                .await?;
            }
            Err(message) => reply(write, &Response::Error { message }).await?,
        },
        Request::Resume { run, prompt } => match resume_run(&ctx, &run, prompt).await {
            // The same reply as a submission, so `offload resume --follow` streams the run
            // exactly the way `offload run --follow` does.
            Ok(id) => {
                reply(
                    write,
                    &Response::Submitted {
                        run: id.to_string(),
                        node: None,
                        waiting: None,
                        // A resume is not a submission: the run already exists in the fleet's
                        // record of it, wherever it has been.
                        here_only: false,
                        queued: None,
                        // Nor is its audience being chosen again: it was decided when the run
                        // was submitted and has travelled with the spec since.
                        audience: None,
                        ask: None,
                        preference: None,
                        siblings: Vec::new(),
                    },
                )
                .await?;
            }
            Err(message) => {
                reply(write, &Response::Error { message }).await?;
            }
        },

        Request::Logs { run, follow } => {
            let id = match ctx.supervisor.resolve(&run) {
                Ok(id) => id,
                Err(e) => {
                    return reply(
                        write,
                        &Response::Error {
                            message: e.to_string(),
                        },
                    )
                    .await;
                }
            };
            // Returns rather than falls through: `stream_logs` ends the stream itself, and a
            // second `Done` after it is a second end to a conversation that had one.
            return stream_logs(&ctx, write, client, id, follow).await;
        }
    }

    reply(write, &Response::Done).await
}

/// Everything this node can say about a run's output, from wherever it is.
///
/// Two sources, in this order, and the order is the transcript's:
///
/// * **This node's own log**, if it ran the run or a leg of it. Live while the agent is here.
/// * **The holder's log**, fetched over the fleet, when the run is somewhere else. That is what
///   makes `--follow` mean anything for the runs this project exists to move: a laptop that
///   submitted a run and watched it migrate to the desktop should keep seeing output, not stop
///   at the handover.
///
/// Sequence numbers are per node, so this is a concatenation by leg rather than a merge by
/// clock. A run that bounced A → B → A has two of A's legs around one of B's, and nothing here
/// interleaves them — a real limitation, and a much smaller lie than sorting log lines from two
/// machines by their own timestamps.
async fn stream_logs(
    ctx: &Ctx,
    write: &mut tokio::net::unix::OwnedWriteHalf,
    client: &mut Client,
    id: offload_core::RunId,
    follow: bool,
) -> std::io::Result<()> {
    // What this node has itself, which for a run that never came here is nothing at all.
    let local = ctx.supervisor.subscribe(id).ok();
    let mut ended = false;
    let mut live = None;

    if let Some((backlog, channel)) = local {
        ended = backlog_ended(&backlog);
        for event in backlog {
            reply(write, &Response::Event(event)).await?;
        }
        live = channel;
    }

    // The agent is here: follow it directly, which is the common case and needs no network.
    if let Some(mut live) = live {
        if follow && !ended {
            loop {
                let next = tokio::select! {
                    // A client that has closed its end stops being followed, rather than being
                    // counted as watching until the next event happens to fail to reach it.
                    () = hung_up(client) => break,
                    event = live.recv() => event,
                };
                match next {
                    Ok(event) => {
                        let terminal = event.is_terminal();
                        reply(write, &Response::Event(event)).await?;
                        if terminal {
                            break;
                        }
                    }
                    // Lagged: the follower fell behind the ring buffer. Say so rather than
                    // silently skipping events — a missing turn boundary in the output would
                    // look like the agent did less than it did.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        reply(
                            write,
                            &Response::Error {
                                message: format!("log follower fell behind; {n} events skipped"),
                            },
                        )
                        .await?;
                    }
                    Err(_) => break,
                }
            }
        }
        return reply(write, &Response::Done).await;
    }

    // Not here, so somebody else has it — see [`log_source`] for which node that is.
    let source = find_run(ctx, &id.to_string()).ok().and_then(|run| {
        let leg = ctx.supervisor.progress_leg(run.id);
        let arbiter = ctx
            .cluster
            .as_ref()
            .and_then(|cluster| cluster.view().arbiter_for(&run));
        log_source(&run, leg, arbiter)
    });
    let (Some(cluster), Some(source)) = (&ctx.cluster, source) else {
        // No mesh, or a run so new that nothing has an opinion about it: the local log is all
        // there is, and for a run that never came here that is nothing — which `subscribe` has
        // already reported as such.
        return reply(write, &Response::Done).await;
    };
    if source == ctx.node_id {
        return reply(write, &Response::Done).await;
    }

    proxy_logs(ctx, write, client, cluster, source, id, follow).await
}

/// What is at `path` in a run's workspace (ADR-0075): read here when the checkout or the branch
/// is here, else asked of the node that ran it — the one `offload logs` asks, [`log_source`].
async fn files(
    ctx: &Ctx,
    run: &str,
    path: &str,
) -> Result<offload_proto::cluster::FilesView, String> {
    let record = find_run(ctx, run)?;
    let here = ctx.supervisor.peek(record.id, path).await;
    let Err(why_not_here) = here else {
        return here;
    };
    let leg = ctx.supervisor.progress_leg(record.id);
    let arbiter = ctx
        .cluster
        .as_ref()
        .and_then(|cluster| cluster.view().arbiter_for(&record));
    match (&ctx.cluster, log_source(&record, leg, arbiter)) {
        (Some(cluster), Some(node)) if node != ctx.node_id => {
            let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
            cluster.fetch_files(node, record.id, path, window).await
        }
        _ => Err(why_not_here),
    }
}

/// Follow a run on another node by asking it, repeatedly.
///
/// A poll rather than a stream held open, for the reason every other loop in this daemon is a
/// poll: it cannot miss an event, it needs nothing kept alive across a migration or a restart,
/// and a second of lag on output measured in agent turns is free. What it costs is one small
/// request per second per follower, which is why nothing polls unless somebody is watching.
async fn proxy_logs(
    ctx: &Ctx,
    write: &mut tokio::net::unix::OwnedWriteHalf,
    client: &mut Client,
    cluster: &std::sync::Arc<offload_cluster::Cluster>,
    holder: offload_core::NodeId,
    id: offload_core::RunId,
    follow: bool,
) -> std::io::Result<()> {
    /// Enough that a whole run's log usually arrives in one or two pages, and small enough that
    /// a page cannot outgrow a frame.
    const PAGE: u32 = 256;
    let window = offload_core::Millis(ctx.config.cluster.bid_window_ms);
    let mut after = 0u64;

    loop {
        match cluster.fetch_events(holder, id, after, PAGE, window).await {
            Ok((events, done)) => {
                let page = events.len();
                let mut terminal = false;
                for offload_proto::cluster::SeqEvent { seq, event } in events {
                    after = seq;
                    terminal = event.is_terminal();
                    reply(write, &Response::Event(event)).await?;
                }
                // A full page means there is more *right now*; `done` means there will never be
                // more from this node. Only the second one ends a follow, because a run between
                // turns is quiet for minutes and is not over.
                if terminal || done {
                    break;
                }
                if !follow && page < PAGE as usize {
                    break;
                }
                if page < PAGE as usize {
                    // Waiting for the next page is also where a departed client is noticed. It
                    // has to be: a quiet run writes nothing to fail on, so without this the
                    // polling outlives the person and the holder goes on believing somebody is
                    // watching — which is the input that decides whether a failure resumes
                    // itself (ADR-0013).
                    tokio::select! {
                        () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                        () = hung_up(client) => break,
                    }
                }
            }
            Err(reason) => {
                reply(write, &Response::Error { message: reason }).await?;
                break;
            }
        }
    }
    reply(write, &Response::Done).await
}

/// Has this run already said everything it is going to?
///
/// Only the *last* event counts, not whether a terminal one appears anywhere. A resumed
/// run's history contains the checkpoint that released it, and that is emphatically not a
/// reason to refuse to follow the turns it has produced since.
fn backlog_ended(backlog: &[crate::api::LogEvent]) -> bool {
    backlog
        .last()
        .is_some_and(crate::api::LogEvent::is_terminal)
}

fn status(ctx: &Ctx) -> NodeStatus {
    let caps = &ctx.capabilities.now();
    let policy = ctx.config.work_policy(caps.device_class);
    let agent = caps.agent(&AgentKind::ClaudeCode);
    let held = ctx.supervisor.held();
    let kept = ctx.store.records_kept(ctx.node_id).unwrap_or_default();
    let cpu_percent = offload_probe::cpu_load_percent(&caps.os, caps.cpu_cores);
    let thermal = crate::deliver::host_thermal(&ctx.config.state_dir);

    // The fleet's answer first, then the owner's: being refused by the fleet is not a
    // policy question and would be confusing filed under one.
    //
    // Asked about a *normal* run, because that is what `offload status` is answering: would
    // this node take an ordinary agent run right now. A heavy one is a different question and
    // `offload explain` asks it about a run that actually exists.
    // This node's own decision first of all: a drained node is leaving, and nothing about the
    // fleet's opinion or the owner's policy is the reason it takes no work.
    //
    // **Revocation is asked above the drain** — `revoked_refusal` carries the reasoning, and is
    // shared with `hosting_refusal` so that the line a person reads and the door they then type
    // at cannot order these differently. They did, and the door had it wrong.
    let drained = ctx
        .supervisor
        .is_draining()
        .then(|| "drained; restart offloadd to take work again".to_string());
    let refusal = revoked_refusal(ctx)
        .or(drained)
        .or_else(|| host_runs_refusal(ctx))
        .or_else(|| {
            let load = offload_core::NodeLoad {
                held,
                // The real per-agent count. This was `held.runs` — every agent conflated — so the
                // number here disagreed with the one `bid::evaluate` computes from the view, and
                // the rule they both feed is the per-agent cap.
                held_for_agent: ctx.supervisor.held_for_agent(&AgentKind::ClaudeCode),
                cpu_percent,
                thermal,
            };
            // Only answerable with a fleet in view: an account's ceiling is about runs on other
            // machines, and a node on its own has none to count.
            let account = ctx.cluster.as_ref().and_then(|cluster| {
                cluster
                    .view()
                    .account_use(&ctx.node_id, &AgentKind::ClaudeCode)
            });
            policy
                .admits(
                    caps,
                    // `offload status`'s own line: whether this node would take an ordinary
                    // run. A task's answer differs only where the owner listed agents, and a
                    // status line that hedged across both tiers would be answering two
                    // questions in one sentence.
                    AGENT_TIER,
                    offload_core::Demand::Normal,
                    &load,
                    account.as_ref(),
                )
                .err()
                .map(|r| r.to_string())
        });

    // …and the second answer, when the owner has given one. Same three doors, same order,
    // asked about light work — because a machine whose `accept = "never"` sits beside
    // `[policy.light] accept = "always"` was reporting `accepting no` one command after
    // running a task, and a line that describes the machine has to describe what it does.
    //
    // Silent unless the owner stated a light form, and silent when the two answers agree:
    // a second line saying the same thing as the first is a line nobody reads. The
    // early refusals above are deliberately not re-asked — a revoked, drained or
    // ungranted node hosts nothing of any weight, and there is no light form of that.
    let light_refusal = policy.light.is_stated().then(|| {
        let load = offload_core::NodeLoad {
            held,
            held_for_agent: ctx.supervisor.held_for_agent(&AgentKind::ClaudeCode),
            cpu_percent,
            thermal,
        };
        let account = ctx.cluster.as_ref().and_then(|cluster| {
            cluster
                .view()
                .account_use(&ctx.node_id, &AgentKind::ClaudeCode)
        });
        match policy.admits(
            caps,
            AGENT_TIER,
            offload_core::Demand::Light,
            &load,
            account.as_ref(),
        ) {
            Ok(()) => crate::api::LightAnswer::Accepted,
            Err(refusal) => crate::api::LightAnswer::Refused {
                reason: refusal.to_string(),
            },
        }
    });
    // Nothing to say where the answer is the one already printed.
    let light_refusal = light_refusal.filter(|light| match (&refusal, light) {
        (None, crate::api::LightAnswer::Accepted) => false,
        (Some(said), crate::api::LightAnswer::Refused { reason }) => said != reason,
        _ => true,
    });

    NodeStatus {
        node_id: ctx.node_id.to_string(),
        name: ctx.config.name.clone(),
        device_class: caps.device_class.to_string(),
        agent: caps
            .agent_details(&AgentKind::ClaudeCode)
            .map(|details| details.version.clone()),
        agent_authenticated: agent.is_some_and(|a| a.authenticated),
        agent_binary: ctx.config.agent.binary.display().to_string(),
        // Only when it is the number that decides. Both ceilings are real and the lower one is
        // what an operator meets, so reporting only the owner's meant a node refusing work at 2
        // while its own status line said 4.
        agent_max_concurrent: caps
            .agent_details(&AgentKind::ClaudeCode)
            .map(|details| details.max_concurrent)
            .filter(|sustains| *sustains < policy.max_concurrent_runs),
        // The **effective** limit: the fleet's own, tightened by whatever the owner said. One
        // number, because one number is what an agent packs to — telling somebody there are two
        // and letting them work out which binds is the sort of report this tree keeps removing.
        // `max_archive_bytes` may only tighten (the config refuses a larger value outright), so
        // the smaller is always the owner's when they set one.
        max_archive_bytes: policy
            .max_archive_bytes
            .unwrap_or(offload_proto::cluster::MAX_BLOB_BYTES)
            .min(offload_proto::cluster::MAX_BLOB_BYTES),
        // From the advertised capability rather than from the config, because the config holds at
        // most the account the owner *expected*: what this says is who the node is actually
        // logged in as, which is the half that can be a surprise.
        agent_account: agent
            .and_then(|a| a.identity.as_ref())
            .map(|id| id.0.clone()),
        agent_state_dir: ctx.supervisor.agent_state_dir().display().to_string(),
        agent_limited_until_unix_ms: ctx.supervisor.account_limited_until().map(|at| at.0),
        state_dir: ctx.config.state_dir.display().to_string(),
        running: held.runs,
        holding: ctx.supervisor.held_work(),
        asks: u32::try_from(ctx.supervisor.asks().len()).unwrap_or(u32::MAX),
        // Three counts from one scan. A store that cannot be read is reported as empty rather
        // than failing the whole status: this is a number beside the ones somebody came for.
        records: kept.total,
        records_learned: kept.learned,
        records_learned_failed: kept.learned_failed,
        stray_checkouts: ctx.supervisor.stray_checkouts(),
        worktrees_dir: ctx.config.checkouts_dir().display().to_string(),
        // Read from the config rather than from the advertised capabilities, because the two
        // answer different questions: a capability says "this device offers email", and the
        // owner wants to know *which of the things they wrote down* is working.
        resources: ctx
            .config
            .resources
            .iter()
            .filter(|cfg| !cfg.id.trim().is_empty())
            .map(|cfg| crate::api::ResourceStatus {
                service: cfg
                    .service()
                    .map_or_else(|e| format!("unreadable ({e})"), |s| s.to_string()),
                id: cfg.id.clone(),
                access: cfg.access().map_or_else(
                    |_| "unreadable".to_string(),
                    |a| format!("{a:?}").to_lowercase(),
                ),
                command: cfg.command.clone(),
                usable: caps
                    .get(&offload_core::CapabilityId(cfg.capability_id()))
                    .is_some_and(|c| c.authenticated),
                // From the **capability**, not from the config, and that is the whole of what
                // makes the line honest: `Capability::unusable` puts the owner's label and the
                // measured reason in it, in that order, so a program that is there and not
                // executable says so. The CLI had been inventing the cause from a `bool` —
                // *"its program was not found"* about a file sitting right there with mode 644.
                // Falls back to the config's label for a capability that was dropped entirely
                // (an unreadable service), where there is no measurement to report.
                description: caps
                    .get(&offload_core::CapabilityId(cfg.capability_id()))
                    .map_or_else(|| cfg.description.clone(), |c| c.description.clone()),
            })
            .collect(),
        // The same shape one tier along, and for the same reason — the owner wants to know
        // which of the things they wrote down is working. `usable` comes off the capability,
        // which is the fact the bid round places on, so this line and the door cannot disagree.
        tasks: ctx
            .config
            .tasks
            .iter()
            .filter(|cfg| !cfg.id.trim().is_empty())
            .map(|cfg| crate::api::TaskStatus {
                service: cfg
                    .service()
                    .map_or_else(|e| format!("unreadable ({e})"), |s| s.to_string()),
                id: cfg.id.clone(),
                command: cfg.command.clone(),
                usable: caps
                    .get(&offload_core::CapabilityId(cfg.capability_id()))
                    .is_some_and(|c| c.authenticated),
                // From the capability, for the reason the resource line above it is.
                description: caps
                    .get(&offload_core::CapabilityId(cfg.capability_id()))
                    .map_or_else(|| cfg.description.clone(), |c| c.description.clone()),
            })
            .collect(),
        max_concurrent: policy.max_concurrent_runs,
        committed_shares: held.shares,
        budget: policy.budget(),
        cpu_percent,
        thermal: thermal.map(|t| t.to_string()),
        // What the keep-awake tick holds, not a second reckoning of whether it should.
        sleep: offload_power::supported().then(|| {
            AWAKE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .map_or_else(
                    || "allowed — this node would take no work".to_string(),
                    |why| format!("kept awake — {why}"),
                )
        }),
        approval_key: crate::approval_key::read(&ctx.config.state_dir).map(|info| {
            let named = info.issuer_key().ok().is_some_and(|key| {
                crate::fleet::load(&ctx.config.state_dir)
                    .ok()
                    .flatten()
                    .and_then(|state| state.approver)
                    .is_some_and(|d| d.issuer_key == key)
            });
            format!(
                "P-256 {} — {}",
                info.level(),
                if named {
                    "this node approves with it"
                } else {
                    "not named by this node's delegation (`offload grant approve --hardware-key`)"
                }
            )
        }),
        refusal,
        light_refusal,
        // From the mesh, because the socket is the mesh's. A node with none has sent nothing
        // and refused nothing, and an empty list is the truth there rather than a shrug.
        sends_refused: ctx
            .mesh
            .as_ref()
            .map(|mesh| {
                mesh.send_refusals()
                    .into_iter()
                    .map(|refusal| crate::api::SendRefusal {
                        destination: refusal.destination.to_string(),
                        refused: refusal.refused,
                        last: refusal.last,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        sends_refused_total: ctx.mesh.as_ref().map_or(0, |mesh| mesh.sends_refused()),
        turnaways: ctx
            .mesh
            .as_ref()
            .map(|mesh| {
                mesh.turnaways()
                    .into_iter()
                    .map(|t| crate::api::Turnaway {
                        peer: t.peer.short(),
                        refused: t.refused,
                        last: t.last,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        turnaways_total: ctx.mesh.as_ref().map_or(0, |mesh| mesh.turnaways_total()),
        // Read from disk rather than from the mesh's copy, so it is right on a node with no
        // mesh at all — which is where a fleet of one lives, and where "you have no approver"
        // is most worth saying.
        fleet: crate::fleet::load(&ctx.config.state_dir)
            .ok()
            .flatten()
            .map(|state| {
                let certificates = ctx
                    .cluster
                    .as_ref()
                    .map(|cluster| cluster.certificates())
                    .unwrap_or_default();
                crate::fleet::health(
                    &state,
                    crate::fleet::Met::Handshakes {
                        certificates: &certificates,
                        turned_away: ctx.mesh.as_ref().map_or(0, |mesh| mesh.turnaways_total()),
                    },
                    offload_cluster::Clock::now(&offload_cluster::SystemClock),
                )
            }),
    }
}

/// Is there anywhere in this fleet a granted service could be reached from?
///
/// The half of `Constraint::CanUse` that survived the proxy. The constraint asked "does *this*
/// node hold it", which was right while a grant could only be honoured by its holder and is
/// wrong now — the phone that holds the mailbox is exactly the device that hosts nothing. What
/// is left is the question the operator can still act on: does anybody have one.
///
/// Checked against this node's own config as well as the view, because a fleet of one is a fleet
/// and its resources are real.
fn unreachable_resource(ctx: &Ctx, wanted: &[offload_core::Service]) -> Option<String> {
    let missing = missing_resources(ctx, wanted);
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "no node in this fleet offers {} for a run to use — `offload status` on each device \
         lists what it offers, and `[[resources]]` in its config is where one is nominated",
        missing.join(" or ")
    ))
}

/// The same question, asked while a *rule* is being written (ADR-0036).
///
/// A warning rather than a refusal, and the difference is the one ADR-0032 rests on: a run is
/// submitted now, so refusing it is actionable now, while a rule fires for months and the fleet it
/// fires into changes. The mailbox is on the phone, and the phone may not have enrolled yet — so
/// refusing here would make the command unusable in exactly the arrangement ADR-0011 was built
/// for. It is worded in the future tense for that reason, and it says what the silence costs,
/// because a rule whose every occurrence is refused looks *busy*: measured, 13 firings and 0 runs
/// in thirty seconds, with nothing at the keyboard, nothing on the phone, and one `WARN` per
/// firing on the machine nobody is logged into.
///
/// Through the same `missing_resources` the refusal uses, so a rule and a run cannot say different
/// things about one fleet.
fn resource_note(ctx: &Ctx, wanted: &[offload_core::Service]) -> Option<String> {
    let missing = missing_resources(ctx, wanted);
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "Nothing in this fleet offers {}, so a firing will be refused rather than run — \
         `offload rules` counts those under DROPPED, with the reason. `[[resources]]` in a \
         node's config nominates one, and the next firing after that will work.",
        missing.join(" or ")
    ))
}

/// The same question for the cheap tier: can anybody in this fleet run this task?
///
/// A **note** rather than a refusal, which is `resource_note`'s decision one tier along and for
/// exactly its reason (ADR-0032, ADR-0036): a rule fires for months and the fleet it fires into
/// changes, so the machine that will offer the task may not have enrolled when the rule is
/// written. Refusing here would make the order of two commands matter.
///
/// Which is *not* what `offload run --task` does — that refuses, because somebody is standing
/// there and can act on it. Two commands, two answers, one fact underneath, and the fact is
/// asked here so a rule and a run cannot disagree about one fleet.
fn task_note(ctx: &Ctx, service: &offload_core::Service) -> Option<String> {
    if nominates_task(ctx, service) {
        return None;
    }
    // "can run", not "nominates": the predicate reads `authenticated`, so this is also the
    // sentence for a `[[tasks]]` entry whose program is not on the device it was nominated on.
    // Both halves are named, because they are fixed by different actions and the note has no
    // way to know which node it is talking about.
    Some(format!(
        "Nothing in this fleet can run a task for `{service}`, so a firing will be refused \
         rather than run — `offload rules` counts those under DROPPED, with the reason. Either \
         no node has a `[[tasks]]` entry for it, or the node that does cannot run the program \
         it names — `offload status` there says which."
    ))
}

/// The same preconditions for a **schedule**, worded for a tick rather than a firing.
///
/// A separate wording over the *same* predicates deliberately, and the distinction is the one
/// ADR-0032 already draws between `offload run --task` (a refusal) and `offload when --task` (a
/// note): what must not diverge is the fact — `nominates_task` and `missing_resources` — while
/// what each command *says about the consequence* is properly its own, because the consequence
/// differs. A rule's refused firing is counted: `offload rules` shows it under DROPPED, and the
/// rule note promises that. A schedule's is counted by nothing, so pointing somebody at a report
/// that will not mention it would be worse than the silence this replaces.
fn schedule_precondition(ctx: &Ctx, spec: &crate::api::EverySpec) -> Option<String> {
    let unnominated = spec
        .task
        .as_ref()
        .filter(|request| !nominates_task(ctx, &request.service))
        .map(|request| request.service.to_string());
    let unoffered = spec
        .agent
        .as_ref()
        .map(|request| missing_resources(ctx, &request.resources))
        .unwrap_or_default();
    schedule_precondition_note(unnominated.as_deref(), &unoffered)
}

/// …and its wording, separated so it can be tested without a fleet.
///
/// The assertion worth having is a **negative** one: this must not point at `offload rules`,
/// which the rule-tier sentence does. That is not a copy-paste slip waiting to happen, it is
/// the decision — a rule's refused firing is counted under DROPPED and a schedule's is counted
/// by nothing, so borrowing the sentence would send somebody to a report that will never
/// mention their tick.
fn schedule_precondition_note(unnominated: Option<&str>, unoffered: &[String]) -> Option<String> {
    if let Some(service) = unnominated {
        return Some(format!(
            "Nothing in this fleet can run a task for `{service}`, so every tick will be \
             refused rather than run. Either no node has a `[[tasks]]` entry for it, or the \
             node that does cannot run the program it names — `offload status` there says \
             which."
        ));
    }
    if !unoffered.is_empty() {
        return Some(format!(
            "Nothing in this fleet offers {}, so every tick will be refused rather than run. \
             `[[resources]]` in a node's config nominates one, and the next tick after that \
             will work.",
            unoffered.join(" or ")
        ));
    }
    None
}

/// Does any node in this fleet have a **working** program for this service?
///
/// This node's own capabilities, plus what the fleet has advertised. A task is advertised as a
/// `Role::Execute` capability under its *service* (ADR-0019 §1), so the view answers for every
/// peer without anybody being asked — and the command never travels, which is the whole point.
///
/// `authenticated` on both arms, which is the same bit `Constraint::ServiceAuthenticated` places
/// on: for `Role::Execute` it means the nominated program is on that device. Without it this
/// answered `true` for a `[[tasks]]` entry pointing at a path that does not exist, so
/// `offload every --task` and `offload when --task` accepted such a schedule and such a rule in
/// **silence** — every tick refused for ever, reported nowhere — while the control, a service
/// nobody nominates at all, got the whole paragraph. That silence is the one session
/// seventy-three removed from this command, arriving again one cause over.
fn nominates_task(ctx: &Ctx, service: &offload_core::Service) -> bool {
    if matches!(
        crate::task::nomination(&ctx.capabilities.now(), &ctx.config, service),
        crate::task::Nomination::Ready
    ) {
        return true;
    }
    let Some(cluster) = &ctx.cluster else {
        return false;
    };
    let me = cluster.node();
    cluster.view().alive().any(|node| {
        node.id != me
            && node
                .capabilities
                .playing(offload_core::Role::Execute)
                .any(|capability| &capability.service == service && capability.authenticated)
    })
}

/// Which of the services a run asked for nobody in this fleet offers.
///
/// The fact both callers above are wording. `offload run` refuses on it and `offload when` warns
/// on it, and the two must not be able to disagree — the same reason `Reach` is computed through
/// `Audience::admits` rather than described a second time.
fn missing_resources(ctx: &Ctx, wanted: &[offload_core::Service]) -> Vec<String> {
    let mine: Vec<offload_core::Service> = ctx
        .config
        .resources
        .iter()
        .filter_map(|cfg| cfg.service().ok())
        .collect();
    let elsewhere = ctx
        .cluster
        .as_ref()
        .map_or_else(crate::resource::Reachable::default, |cluster| {
            crate::resource::Reachable::in_view(&cluster.view(), cluster.node())
        });
    wanted
        .iter()
        .filter(|service| !mine.contains(service) && !elsewhere.elsewhere.contains_key(service))
        .map(ToString::to_string)
        .collect()
}

/// Carry one agent's conversation with a resource on another machine (ADR-0011).
///
/// Three hops and no state: the agent's stdio, this connection, and a stream to the holder. What
/// travels is the agent's own protocol, opaque here for the reason `offload-agent` never parses
/// model output — a proxy that understood MCP would be a second implementation of somebody
/// else's protocol, kept in step by hand.
///
/// The grant is checked *here* as well as on the holder, and the two checks answer different
/// questions: this one is "did this run ask for that", which the holder cannot know reliably from
/// gossip, and the holder's is "may this node have it", which this one has no business deciding.
async fn proxy_resource(
    mesh: &Arc<crate::mesh::Mesh>,
    run: offload_core::RunId,
    service: &offload_core::Service,
    write: &mut tokio::net::unix::OwnedWriteHalf,
    lines: &mut Client,
) -> std::io::Result<()> {
    let elsewhere = crate::resource::Reachable::in_view(&mesh.cluster.view(), mesh.cluster.node());
    let Some(node) = elsewhere.elsewhere.get(service).copied() else {
        return reply(
            write,
            &Response::Error {
                message: format!("no node in this fleet offers {service} for a run to use"),
            },
        )
        .await;
    };

    let mut stream = match mesh.cluster.open_resource(node, run, service).await {
        Ok(stream) => stream,
        Err(message) => return reply(write, &Response::Error { message }).await,
    };
    reply(write, &Response::ResourceOpen { node: node.short() }).await?;

    loop {
        tokio::select! {
            from_agent = lines.next_line() => match from_agent {
                Ok(Some(line)) => {
                    // Anything that is not a resource line ends the session rather than being
                    // answered: this connection stopped being the control protocol the moment
                    // the resource opened, and guessing what a stray request meant is how a
                    // proxy grows a second personality.
                    let Ok(Request::ResourceLine { line }) = serde_json::from_str::<Request>(&line)
                    else {
                        break;
                    };
                    if stream
                        .send(&offload_proto::ClusterMessage::ResourceData { line })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                // The agent exited. Its side of the pipe closing is the whole teardown signal:
                // the holder sees this stream end and kills the program it started.
                Ok(None) | Err(_) => break,
            },
            from_holder = stream.recv::<offload_proto::ClusterMessage>() => match from_holder {
                Ok(offload_proto::ClusterMessage::ResourceData { line }) => {
                    reply(write, &Response::ResourceLine { line }).await?;
                }
                Ok(offload_proto::ClusterMessage::ResourceClosed { reason }) => {
                    tracing::info!(run_id = %run, %service, %reason, "a resource closed");
                    break;
                }
                Ok(_) => break,
                Err(e) => {
                    tracing::warn!(run_id = %run, %service, error = %e, "a resource went away");
                    break;
                }
            },
        }
    }
    let _ = stream
        .send(&offload_proto::ClusterMessage::ResourceClosed {
            reason: "the agent finished with it".into(),
        })
        .await;
    let _ = stream.finish().await;
    Ok(())
}

async fn reply(
    write: &mut tokio::net::unix::OwnedWriteHalf,
    response: &Response,
) -> std::io::Result<()> {
    let mut line = serde_json::to_string(response)
        .unwrap_or_else(|_| r#"{"reply":"error","message":"failed to encode response"}"#.into());
    line.push('\n');
    write.write_all(line.as_bytes()).await
}

/// Fan notifications out of the event log, at whatever pace the sinks manage (ADR-0010).
///
/// A tick of its own, and *not* part of the run loop, which is the ADR's last consequence taken
/// literally: a sink can be slow or unavailable in ways a run cannot, so delivery must never be
/// something a turn boundary or a checkpoint waits on. The worst a wedged script can do here is
/// cost this loop one interval.
///
/// It lives beside `tend_own_runs` rather than in the mesh tick for the reason that has now cost
/// this project two bugs: anything about *this node's own* runs that goes into the gossip loop
/// silently does not run on a fleet of one — and a fleet of one is precisely the setup where
/// there is nobody else to notice that an agent fell over in the night.
pub(crate) async fn deliver_notifications(
    ctx: Ctx,
    sinks: Arc<crate::deliver::Sinks>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let (store, config, mesh) = (ctx.store.clone(), ctx.config.clone(), ctx.mesh.clone());
    // A node with no routes of its own still runs this loop, because the fleet may have one: the
    // desktop that hosts every run is exactly the machine with nothing to notify *through*.
    let fleet: Option<Arc<crate::mesh::Delivery>> = mesh.as_ref().map(|mesh| {
        Arc::new(crate::mesh::Delivery::new(
            sinks.clone(),
            &mesh.cluster,
            offload_core::Millis(config.cluster.bid_window_ms),
        ))
    });
    // **Not returned from when there are no sinks and no fleet**, which it used to do: since
    // ADR-0057 a notice may fire a *rule* on this node, and a node with no route to a person is
    // exactly the machine that wants one — the phone running a watcher with nothing to buzz.
    // The pass costs two queries when there is nothing to do.
    if sinks.is_empty() && fleet.is_none() {
        tracing::info!(
            "no delivery sinks and no fleet; only notice-bound rules will be fired from here"
        );
    }
    tracing::info!(sinks = config.sinks.len(), "delivering notifications");

    // What a notice-bound rule is fired *through*: the same submission path a person's command
    // takes, so a rule cannot start work by a route that skipped a check (ADR-0057 §2).
    let escalate = crate::trigger::Escalations::new(ctx);

    // Slower than the lease heartbeat, because nothing here is racing anything: a notification
    // is worth a person's attention and is not worth a second of latency budget.
    let every = std::time::Duration::from_secs(5);
    loop {
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            _ = shutdown.changed() => return,
        }
        let routes = fleet.as_deref().map(|f| f as &dyn crate::deliver::Fleet);
        match crate::deliver::tend_deliveries(
            &store,
            &sinks,
            routes,
            Some(&escalate),
            crate::supervisor::now(),
        )
        .await
        {
            Ok(0) => {}
            Ok(sent) => tracing::debug!(sent, "notifications delivered"),
            Err(e) => tracing::error!(error = %e, "could not run a delivery pass"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binding_replaces_a_socket_left_behind_by_a_crash() {
        // Otherwise every restart after a hard kill fails with "address in use", which is
        // a confusing way to report "the last run crashed".
        let dir = std::env::temp_dir().join(format!("offload-sock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("offloadd.sock");
        std::fs::write(&path, b"stale").expect("write stale socket file");

        let listener = bind(&path).expect("bind over stale socket");
        drop(listener);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `offload every` was the third caller of a precondition nobody had added it to.
    ///
    /// Measured before the fix: `offload every 1m --task nosuch` was accepted in silence, then
    /// refused at every tick for ever (`ineligible: has nosuch as execute`), while `offload
    /// schedules` said `nothing fired from here yet` — the sentence it also uses for a
    /// schedule that has not reached its first tick. `offload when --task nosuch` warned, and
    /// had done since ADR-0032.
    ///
    /// The wording gained a second cause when the predicate did: `nominates_task` reads the
    /// capability's `authenticated` bit, so this is also the note for a `[[tasks]]` entry whose
    /// program is not on the device it was nominated on — measured in the same silence, one
    /// session later. Both halves are named because they are fixed by different actions and
    /// this note cannot know which node it is about.
    #[test]
    fn a_schedule_for_a_task_nobody_nominates_says_so_at_the_keyboard() {
        let note = schedule_precondition_note(Some("nosuch"), &[]).expect("warned");
        assert!(note.contains("`nosuch`"), "{note}");
        assert!(note.contains("[[tasks]]"), "{note}");
        assert!(note.contains("every tick"), "{note}");
        // The second cause, and where to look for which of the two it is.
        assert!(note.contains("cannot run the program"), "{note}");
        assert!(note.contains("offload status"), "{note}");
        // The decision, asserted: the rule tier's sentence points at `offload rules`, which
        // counts a refused firing under DROPPED. Nothing counts a refused *tick*, so pointing
        // there would send somebody to a report that will never mention it.
        assert!(!note.contains("offload rules"), "{note}");
        assert!(!note.contains("DROPPED"), "{note}");

        // The agent tier's half of the same precondition.
        let resources = schedule_precondition_note(None, &["`email`".to_string()]).expect("warned");
        assert!(resources.contains("[[resources]]"), "{resources}");
        assert!(!resources.contains("offload rules"), "{resources}");

        // The control, and the reason the note is an `Option`: a schedule whose preconditions
        // are met says nothing extra at all.
        assert_eq!(schedule_precondition_note(None, &[]), None);
    }

    #[test]
    fn following_a_resumed_run_is_not_cut_short_by_its_own_history() {
        // Found live: `offload resume --follow` printed the backlog and exited, because a
        // released checkpoint sits in the middle of a resumed run's log and the old check
        // asked whether *any* event was terminal.
        use crate::api::LogKind;

        let released = crate::api::logged_now(LogKind::Checkpointed {
            turn: 6,
            summary: "1 KiB patch".into(),
            released: true,
        });
        let resumed = crate::api::logged_now(LogKind::Resumed {
            from_turn: 6,
            session: "s".into(),
            workspace: "adopted in place".into(),
        });

        assert!(
            backlog_ended(std::slice::from_ref(&released)),
            "stop on a finished run"
        );
        assert!(
            !backlog_ended(&[released, resumed]),
            "but not when the run has started talking again"
        );
        assert!(!backlog_ended(&[]), "an empty backlog is not an ending");
    }

    /// A drained node with no fleet must refuse to host, and that is the whole of what a drain
    /// can do there.
    ///
    /// `stop_accepting` lived at the top of `Mesh::drain`, which `Request::Drain` skips entirely
    /// when there is no mesh — so `offload drain` on a fleet of one set no flag at all. The flag
    /// is set by the handler now, and this is the other half: the refusal it turns on has to be on
    /// the path a fleet of one actually takes. Both halves were live at once, so fixing only the
    /// first gave a node that reported `accepting no` and started the next run anyway.
    ///
    /// Shaped like the mesh test that guards `Host::evaluate` — the point is that the *reason*
    /// changes, not that the submission fails, because on this fixture it fails either way.
    #[tokio::test]
    async fn a_drained_node_with_no_fleet_refuses_to_host() {
        let dir = std::env::temp_dir().join(format!("offload-drain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let me = NodeId::from_bytes([7; 32]);
        let ctx = ctx_with(&dir, me);

        let request = || crate::api::SubmitRequest {
            max_turns: None,
            repo: "https://example.com/me/api.git".into(),
            prompt: "add tests for the parser".into(),
            model: None,
            permission: None,
            git_ref: None,
            allow: Vec::new(),
            queue: false,
            deadline: None,
            demand: offload_core::Demand::Normal,
            notify: offload_core::Audience::Everyone,
            notices: offload_core::Notices::Everything,
            ask: offload_core::AskPolicy::Never,
            resources: Vec::new(),
            origin: offload_core::Origin::Operator,
            require: offload_core::Wanted::default(),
            prefer: offload_core::Wanted::default(),
            hold_until: None,
        };

        // Whatever this node says before the drain, it is not "draining".
        let before = submit_run(&ctx, request()).await;
        assert!(
            !before.is_err_and(|why| why.contains("draining")),
            "nothing has asked it to leave yet"
        );

        ctx.supervisor.stop_accepting();

        let after = submit_run(&ctx, request()).await.err();
        assert!(
            after.as_deref().is_some_and(|why| why.contains("draining")),
            "a drained node hosts nothing: {after:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The other door, which asked none of the three (ADR-0047).
    ///
    /// `offload resume` starts an agent exactly the way a submission does, and its handler asked
    /// only `Room::for_one_more` — the queue. Measured on one daemon: a node reporting `accepting
    /// no — node is not accepting work` resumed a run at its own socket and went on taking turns,
    /// and so did a drained one, and so did a **revoked** one whose run ADR-0044 had just failed
    /// with *this node was revoked from its fleet, so it stopped running it*.
    ///
    /// Shaped like the submission test above, and paired with it on purpose: what has to hold is
    /// that the two doors give the *same* answer to the same standing question, which is why
    /// there is one [`hosting_refusal`] rather than two lists that agree today.
    #[tokio::test]
    async fn a_drained_node_refuses_to_resume_as_well_as_to_submit() {
        let dir = std::env::temp_dir().join(format!("offload-resume-drain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let me = NodeId::from_bytes([9; 32]);
        let ctx = ctx_with(&dir, me);
        let run = a_run(me);
        ctx.store.save_run(&run).expect("save");
        let id = run.id.to_string();

        // Whatever this node says before the drain, it is not "draining" — and it is not the
        // hosting gate at all: the run has no checkpoint, so `Supervisor::resume` refuses it on
        // its own terms, which is the control that keeps this test honest about *which* refusal
        // moved.
        let before = resume_run(&ctx, &id, None).await.err();
        assert!(
            before
                .as_deref()
                .is_some_and(|why| why.contains("no conversation to continue")),
            "the run itself is what stops it here: {before:?}"
        );

        ctx.supervisor.stop_accepting();

        let after = resume_run(&ctx, &id, None).await.err();
        assert!(
            after.as_deref().is_some_and(|why| why.contains("draining")),
            "a drained node resumes nothing either: {after:?}"
        );

        // And a run that does not exist still hears about itself rather than about the node:
        // the refusal is asked after `resolve` on purpose.
        let typo = resume_run(&ctx, "0000deadbeef", None).await.err();
        assert!(
            typo.as_deref().is_some_and(|why| !why.contains("draining")),
            "a typo is not a policy question: {typo:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `accept = "never"` means it on both doors, and says the same sentence at each.
    ///
    /// The owner's standing answer reached neither the no-cluster submission arm (ADR-0046) nor
    /// the resume handler (ADR-0047), and each was found separately. What this pins is the
    /// agreement: one policy, two doors, one wording — the thing a second copy of the list would
    /// break silently, because both copies are right on the day they are written.
    #[tokio::test]
    async fn a_node_that_hosts_nothing_says_so_at_both_doors() {
        let dir = std::env::temp_dir().join(format!("offload-never-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let me = NodeId::from_bytes([11; 32]);
        let mut ctx = ctx_with(&dir, me);
        let run = a_run(me);
        ctx.store.save_run(&run).expect("save");
        let id = run.id.to_string();

        let mut config = (*ctx.config).clone();
        config.policy = Some(crate::config::PolicyConfig {
            accept: Some(offload_core::AcceptWork::Never),
            ..crate::config::PolicyConfig::default()
        });
        ctx.config = Arc::new(config);

        let request = crate::api::SubmitRequest {
            max_turns: None,
            repo: "https://example.com/me/api.git".into(),
            prompt: "add tests for the parser".into(),
            model: None,
            permission: None,
            git_ref: None,
            allow: Vec::new(),
            queue: false,
            deadline: None,
            demand: offload_core::Demand::Normal,
            notify: offload_core::Audience::Everyone,
            notices: offload_core::Notices::Everything,
            ask: offload_core::AskPolicy::Never,
            resources: Vec::new(),
            origin: offload_core::Origin::Operator,
            require: offload_core::Wanted::default(),
            prefer: offload_core::Wanted::default(),
            hold_until: None,
        };

        let submitted = submit_run(&ctx, request).await.err();
        let resumed = resume_run(&ctx, &id, None).await.err();
        assert_eq!(
            submitted, resumed,
            "one standing answer, said the same way at whichever door it is asked"
        );
        assert!(
            submitted
                .as_deref()
                .is_some_and(|why| why.contains("not accepting work")),
            "and it is the owner's policy that is speaking: {submitted:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Standing down sets both latches, so the door has to ask which one it means.
    ///
    /// `stand_down` marks the node revoked **and** draining (ADR-0044), and `hosting_refusal`
    /// asked the drain first — so every door it guards told a revoked device's owner to *restart
    /// `offloadd` to take work again*, which is advice that cannot work: the fleet has thrown the
    /// device out, and a restart changes nothing. `status` has always ordered these the other way
    /// round, with the reason written beside it; the door and the report were two orderings of
    /// three questions, and only one of them had the argument.
    ///
    /// Found by pairing the two on a live daemon. A revoked fleet-of-one whose run had just
    /// failed with `this node was revoked from its fleet, so it stopped running it` printed, one
    /// line below that in `offload ps`, `not of this node right now: this node is draining`. Both
    /// sentences came from this daemon, a second apart, and disagreed. At the door: `offload
    /// resume` answered `this node is draining; restart offloadd to take work again` while
    /// `offload run` — the *submit* door, which reaches `host_runs_refusal` through the mesh
    /// rather than through here — named the revocation.
    ///
    /// The assertion is on the **cause**, not on refusal: both orderings refuse, which is why
    /// this survived. A wrong cause confidently stated is what ADR-0043 deleted
    /// `NodeIsDeparting` for and what ADR-0049 gave the handover a third sentence for, and it is
    /// the same mistake here at the one door a person types at.
    #[tokio::test]
    async fn a_revoked_node_is_not_told_to_restart_itself() {
        let dir = std::env::temp_dir().join(format!("offload-revdoor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let ctx = ctx_with(&dir, NodeId::from_bytes([21; 32]));

        assert!(
            hosting_refusal(&ctx, AGENT_TIER, ORDINARY_RUN).is_none(),
            "the control: it would host"
        );

        ctx.supervisor.stand_down().await;
        assert!(ctx.supervisor.is_revoked());
        assert!(
            ctx.supervisor.is_draining(),
            "both latches, which is the whole hazard"
        );

        let door =
            hosting_refusal(&ctx, AGENT_TIER, ORDINARY_RUN).expect("a revoked node hosts nothing");
        assert!(
            door.contains("revoked"),
            "the door names the cause that is true: {door}"
        );
        assert!(
            !door.contains("restart"),
            "and not the one whose advice cannot work: {door}"
        );
        // The two that must not drift, for the reason this whole family of tests exists: the
        // `accepting` line and the door a person types `offload resume` at answer one question.
        assert_eq!(
            status(&ctx).refusal.as_deref(),
            Some(door.as_str()),
            "the report and the door, one sentence"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// What the device *is* changes while the daemon runs, and both readers have to see it.
    ///
    /// The capabilities were an `Arc<Capabilities>` built once in `main`; the re-probe handed its
    /// result to `Cluster::set_capabilities` and to nothing else, in a loop a fleet of one never
    /// enters. So `offload status` and the doors that start runs answered from a snapshot while
    /// the fleet answered from the probe. Measured on one daemon whose link was turned metered
    /// underneath it — `accepting yes`, a submission refused by the bid round with `network is
    /// metered and policy disallows it`, and `offload resume` starting an agent, all within a few
    /// seconds (ADR-0048).
    ///
    /// This asserts the property that broke rather than the plumbing: after a refresh, the door
    /// and the report say the **same** thing, and it is the new fact.
    #[tokio::test]
    async fn the_report_and_the_door_both_see_a_device_that_changed() {
        let dir = std::env::temp_dir().join(format!("offload-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let ctx = ctx_with(&dir, NodeId::from_bytes([13; 32]));

        assert!(
            hosting_refusal(&ctx, AGENT_TIER, ORDINARY_RUN).is_none(),
            "the control: it would host"
        );

        // The one clause a running daemon can change under itself: NetworkManager starts saying
        // the link costs money (ADR-0045). No config edit, no restart.
        let mut metered = (*ctx.capabilities.now()).clone();
        metered.metered_network = offload_core::Metered::Yes;
        assert_eq!(
            ctx.capabilities.refresh(metered),
            vec!["metered_network"],
            "a probe that says something new, and names it"
        );

        let door = hosting_refusal(&ctx, AGENT_TIER, ORDINARY_RUN);
        let report = status(&ctx).refusal;
        assert!(
            door.as_deref().is_some_and(|why| why.contains("metered")),
            "the door decides with the new answer: {door:?}"
        );
        // Equal, not merely both refusing, and it holds whatever else is true of the machine
        // this test runs on: `admits` asks `permits` first (ADR-0046), so a standing clause is
        // what the report names even when the fixture is also at capacity or under load.
        assert_eq!(
            door, report,
            "the report has to come from where the decision reads"
        );

        // …and back again, because a laptop leaves the tether as often as it joins it. Not
        // asserted as `None` on the report: `status` answers with `admits`, which also sees a
        // queue and a load average this test does not control.
        let mut free = (*ctx.capabilities.now()).clone();
        free.metered_network = offload_core::Metered::No;
        assert_eq!(ctx.capabilities.refresh(free), vec!["metered_network"]);
        assert!(hosting_refusal(&ctx, AGENT_TIER, ORDINARY_RUN).is_none());
        assert!(
            !status(&ctx)
                .refusal
                .is_some_and(|why| why.contains("metered")),
            "a fact that went away has to stop being reported"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A context with a store and no fleet, for the routing decisions that are taken before any
    /// peer is contacted.
    fn ctx_with(dir: &std::path::Path, node_id: NodeId) -> Ctx {
        let cfg = Arc::new(Config {
            state_dir: dir.to_path_buf(),
            ..Config::default()
        });
        let store = offload_store::Store::open_memory().expect("store");
        Ctx {
            models: std::sync::Arc::default(),
            supervisor: Supervisor::new(cfg.clone(), node_id, store.clone()).with_private_ledger(),
            store,
            config: cfg,
            node_id,
            capabilities: Arc::new(crate::deliver::Current::new(
                offload_core::Capabilities::empty(
                    offload_core::Os::Linux,
                    offload_core::Arch::X86_64,
                    offload_core::DeviceClass::Laptop,
                ),
            )),
            cluster: None,
            mesh: None,
            triggers: crate::trigger::Triggers::new(),
        }
    }

    #[test]
    fn a_device_configured_for_light_work_only_says_so_in_its_own_status() {
        // Measured on a daemon before this: `accepting no — node is not accepting work`, one
        // command after that node had run `--task webhook --demand light` to completion. Both
        // sentences were true and the pair was the answer; the line describes a machine, so it
        // has to describe what the machine does (ADR-0019 §4).
        let dir = std::env::temp_dir().join(format!("offload-light-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let mut ctx = ctx_with(&dir, NodeId::from_bytes([31; 32]));
        let mut cfg = (*ctx.config).clone();
        cfg.policy = Some(crate::config::PolicyConfig {
            accept: Some(offload_core::AcceptWork::Never),
            light: Some(crate::config::LightPolicyConfig {
                accept: Some(offload_core::AcceptWork::Always),
                ..crate::config::LightPolicyConfig::default()
            }),
            ..Default::default()
        });
        ctx.config = Arc::new(cfg);

        let reported = status(&ctx);
        assert!(
            reported
                .refusal
                .as_deref()
                .is_some_and(|why| why.contains("not accepting work")),
            "the ordinary answer is still the headline: {:?}",
            reported.refusal
        );
        // **Not asserted as `Accepted`**, and the reason is worth the words: `status` measures
        // this machine's own load average, and `admits` refuses work under pressure — so a test
        // that demanded a yes here passes on an idle laptop and fails inside `cargo test
        // --workspace`, which pegs the CPU. Measured exactly that way. What this test is
        // actually about is that the owner's *second* answer is reported and is its own
        // sentence, and that holds at any load.
        match reported.light_refusal {
            Some(crate::api::LightAnswer::Accepted) => {}
            Some(crate::api::LightAnswer::Refused { reason }) => {
                assert!(
                    !reason.contains("not accepting work"),
                    "the light door is open, so a refusal must come from a different clause: \
                     {reason}"
                );
                assert_ne!(
                    Some(reason),
                    reported.refusal,
                    "a second line saying the first line's sentence is a line nobody reads"
                );
            }
            None => panic!("the owner stated a light form and the two answers differ"),
        }

        // …and it stays quiet on every device whose owner said nothing about light work, which
        // is all of them until somebody writes `[policy.light]`. Load-independent: with no
        // light form stated there is no second question to ask.
        let quiet = ctx_with(&dir, NodeId::from_bytes([32; 32]));
        assert_eq!(status(&quiet).light_refusal, None);
    }

    /// The precondition a rule shared with a run and was never asked about.
    ///
    /// `offload run --use email` is *refused* where no node offers a mailbox; `offload when` with
    /// the same flag was written in silence and printed `Nothing else to do: the next event fires
    /// it`. Measured on a rule ticking every three seconds: **13 firings, 0 runs**, nothing at the
    /// keyboard, nothing on the phone, one `WARN` per firing in the daemon's log.
    ///
    /// What this pins is that the two answers cannot drift: one fact, two wordings, and neither
    /// may be silent where the other speaks.
    #[test]
    fn a_rule_and_a_run_agree_about_a_resource_the_fleet_does_not_have() {
        let dir = std::env::temp_dir().join(format!("offload-res-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let mut ctx = ctx_with(&dir, NodeId::from_bytes([4; 32]));
        let email = "email".parse::<offload_core::Service>().expect("service");

        let wanted = [email.clone()];
        let refusal = unreachable_resource(&ctx, &wanted).expect("a run is refused");
        let note = resource_note(&ctx, &wanted).expect("and a rule is warned");
        assert!(refusal.contains("email"), "{refusal}");
        // The rule's wording has to carry what the silence costs: a refused firing is counted
        // rather than reported, and `offload rules` is the only place it shows.
        assert!(note.contains("DROPPED"), "{note}");
        assert!(
            note.contains("refused"),
            "a rule is told what a firing will do: {note}"
        );

        // Nominated here, so both go quiet — a fleet of one is a fleet and its resources are real.
        let mut config = (*ctx.config).clone();
        config.resources.push(crate::config::ResourceConfig {
            id: "mailbox".into(),
            service: "email".into(),
            command: "/usr/bin/true".into(),
            args: Vec::new(),
            env: Vec::new(),
            access: None,
            identity: None,
            description: "a mailbox".into(),
        });
        ctx.config = Arc::new(config);
        assert!(unreachable_resource(&ctx, &wanted).is_none());
        assert!(
            resource_note(&ctx, &wanted).is_none(),
            "neither may speak where the other is silent"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ADR-0063 §7: the three reasons a preferred node was not the one placed, each worded from
    /// the round's opinions — and no sentence at all when the preference was met or absent.
    #[test]
    fn a_preference_that_was_not_met_says_which_of_three_reasons_it_was() {
        use offload_cluster::{Opinion, Verdict};
        let laptop = NodeId::from_bytes([1; 32]);
        let desktop = NodeId::from_bytes([2; 32]);
        let names = |id: NodeId| if id == laptop { "laptop" } else { "desktop" }.to_string();
        let offer = |score: i64, preferred: bool| {
            let terms = offload_core::ScoreTerms {
                hardware: score - if preferred { 60 } else { 0 },
                preferred: if preferred { 60 } else { 0 },
                ..offload_core::ScoreTerms::default()
            };
            Verdict::Bids(offload_core::Offer {
                score: terms.total(),
                available: offload_core::Availability::Now,
                terms: Some(terms),
            })
        };
        let opinion = |node, verdict| Opinion {
            node,
            name: names(node),
            verdict,
        };
        let mut spec = a_run(laptop).spec;

        // No preference: nothing to say.
        let round = vec![opinion(laptop, offer(40, false))];
        assert_eq!(preference_note(&spec, laptop, &round, names), None);

        spec.prefer = offload_core::Constraint::Node(desktop);
        // Met: nothing to say.
        let round = vec![
            opinion(desktop, offer(120, true)),
            opinion(laptop, offer(40, false)),
        ];
        assert_eq!(preference_note(&spec, desktop, &round, names), None);

        // Did not answer.
        let round = vec![
            opinion(laptop, offer(40, false)),
            opinion(desktop, Verdict::Silent),
        ];
        let note = preference_note(&spec, laptop, &round, names).expect("a note");
        assert!(
            note.contains("preferred desktop, which did not answer"),
            "{note}"
        );
        assert!(note.ends_with("placed on laptop"), "{note}");

        // Could not take it, with its own reason.
        let round = vec![
            opinion(laptop, offer(40, false)),
            opinion(
                desktop,
                Verdict::WillNot("node is not accepting work".into()),
            ),
        ];
        let note = preference_note(&spec, laptop, &round, names).expect("a note");
        assert!(
            note.contains("could not take it: node is not accepting work"),
            "{note}"
        );

        // Outscored, with the terms that did it.
        let round = vec![
            opinion(laptop, offer(130, false)),
            opinion(desktop, offer(100, true)),
        ];
        let note = preference_note(&spec, laptop, &round, names).expect("a note");
        assert!(note.contains("was outscored (100 against 130"), "{note}");
        assert!(note.contains("hardware +90"), "{note}");
        assert!(note.contains("preferred -60"), "{note}");
    }

    fn a_run(home: NodeId) -> offload_core::Run {
        use offload_core::{
            AgentKind, AgentWork, Constraint, PermissionMode, Restartability, RunSpec,
            ToolAllowlist, Work, WorkspaceSpec,
        };
        offload_core::Run::new(
            offload_core::RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            RunSpec {
                work: Work::Agent(AgentWork {
                    agent: AgentKind::ClaudeCode,
                    model: None,
                    prompt: "add tests for the parser".into(),
                    workspace: WorkspaceSpec {
                        repo: "https://example.com/me/api.git".into(),
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
                notify: offload_core::Audience::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: offload_core::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            home,
            now_millis(),
        )
    }

    /// `a_run`'s counterpart for the cheap tier: no prompt, no model, and **no workspace**.
    fn a_task(home: NodeId) -> offload_core::Run {
        let mut run = a_run(home);
        run.spec.work = offload_core::Work::Task(offload_core::TaskWork {
            service: "webhook".parse().expect("service"),
            args: Vec::new(),
        });
        run
    }

    #[test]
    fn nothing_to_discard_says_which_of_the_four_reasons_it_is() {
        // One sentence covered four causes and was false for three of them. Walked on two
        // daemons: `offload rm` on a task, on a run cancelled before it ever started, on a
        // checkout already removed here, and on a run that finished on a peer all answered "it
        // was removed already, or the run finished on another machine, where `offload rm` is
        // what discards it". The fourth case is the only one that sentence describes, and even
        // there it named no machine while `progress_leg` — which `log_source` above already
        // reads to route `offload logs` — sat in this node's own row holding the answer.
        let me = NodeId::from_bytes([2; 32]);

        // A task has no workspace on any machine (ADR-0019 §2), so no machine is the answer and
        // the leg it happens to have run on is not what the operator needs to hear. Asked first
        // for that reason: it is true whatever the leg says.
        let task = a_task(me);
        for leg in [Leg::Never, Leg::Here, Leg::Peer("desktop")] {
            for note in [None, Some(REMOVED), Some("3 commit(s)")] {
                let said = no_checkout_here(&task, &leg, note, None);
                assert!(
                    said.contains("is a task") && said.contains("no workspace on any machine"),
                    "a task was told about a machine: {said}"
                );
            }
        }

        let run = a_run(me);

        // Never placed anywhere — cancelled out of the queue. There is nothing to go looking
        // for, which is the opposite of what "on another machine" sends somebody to do.
        let said = no_checkout_here(&run, &Leg::Never, None, None);
        assert!(
            said.contains("never started on any machine"),
            "a run that never ran was sent hunting: {said}"
        );

        // Ran here, and this node's own note says it tore the worktree down. `offload ps` says
        // `removed` on every node in the fleet at this point, so the screen beside this sentence
        // already knew.
        let said = no_checkout_here(&run, &Leg::Here, Some(REMOVED), None);
        assert!(
            said.contains("ran here") && said.contains("already been removed"),
            "an already-removed checkout was described as possibly elsewhere: {said}"
        );

        // Ran here and nothing here claims to have removed it, and no audit row either. The
        // first fix pointed at `offload audit {id}` and said it *"says what happened to it"* —
        // measured on a real daemon against a run whose last leg was still this node, and the
        // log printed `granted`, `accepted`, and not one word about a directory. The sweep is
        // what writes that row and it runs on the leg that has **lost** the run, which is
        // `Leg::Peer` and not this arm. So the absence is stated rather than papered over.
        let said = no_checkout_here(&run, &Leg::Here, Some("3 commit(s)"), None);
        assert!(
            !said.contains("offload audit"),
            "a command with nothing in it was named as the place to look: {said}"
        );
        assert!(
            said.contains("nothing here recorded a teardown"),
            "not knowing what became of a checkout is the answer, and has to be said: {said}"
        );

        // …and where this node *did* record it, it says what became of it rather than naming a
        // command at all. One copy of the wording, in `Reclamation::why`, so this sentence and
        // `offload audit`'s cannot drift apart.
        let said = no_checkout_here(
            &run,
            &Leg::Here,
            Some("3 commit(s)"),
            Some(offload_core::Reclamation::NobodyWaiting),
        );
        assert!(
            said.contains(offload_core::Reclamation::NobodyWaiting.why()),
            "the node held the reason and made somebody go and look it up: {said}"
        );

        // Ran on a peer: the one case the old sentence described, and the machine is named.
        let said = no_checkout_here(&run, &Leg::Peer("desktop"), Some("3 commit(s)"), None);
        assert!(
            said.contains("ran on desktop"),
            "the machine holding the checkout was not named: {said}"
        );
        assert!(
            said.contains("`offload rm` there"),
            "naming the machine without saying what to do there is half an answer: {said}"
        );

        // …and the peer has already torn it down, which gossips. Sending somebody to another
        // machine to type a command with nothing left to do is the same defect one node over,
        // and it is the one this arm was introduced with before the note was consulted.
        let said = no_checkout_here(&run, &Leg::Peer("desktop"), Some(REMOVED), None);
        assert!(
            said.contains("desktop") && said.contains("already been removed"),
            "a checkout the fleet agrees is gone still sent somebody to go and remove it: {said}"
        );
        assert!(
            !said.contains("`offload rm` there"),
            "there is nothing left for that command to do: {said}"
        );
    }

    #[test]
    fn a_finished_runs_log_is_on_the_machine_that_ran_it() {
        // `logs` resolved holder-then-arbiter, and a run with no holder is *two* states. For a
        // finished one the arbiter is precisely the node that does not have the log — and when the
        // arbiter is the asking node the answer was silence: exit 0, no output, which reads as "the
        // run produced none". Measured on two daemons before the fix: 56 lines on the machine that
        // ran it, 0 on the machine it was submitted from.
        let me = NodeId::from_bytes([2; 32]);
        let ran_it = NodeId::from_bytes([9; 32]);
        let mut run = a_run(me);

        // Pending, never run: no leg, and the arbiter is right — it is the node offering it and
        // the one that writes down a missed deadline. This is the case the fallback was written
        // for and it must not change.
        assert_eq!(log_source(&run, None, Some(me)), Some(me));

        // Running elsewhere: the holder, whatever else is known.
        let epoch = run
            .assign(ran_it, now_millis(), offload_core::Millis(60_000))
            .expect("assign");
        run.started(ran_it, epoch, now_millis()).expect("start");
        assert_eq!(log_source(&run, Some(ran_it), Some(me)), Some(ran_it));

        // Finished: no holder, and the leg that wrote the numbers is the leg that wrote the log.
        run.complete(ran_it, epoch, now_millis()).expect("complete");
        assert_eq!(run.holder(), None, "a finished run names no node");
        assert_eq!(
            log_source(&run, Some(ran_it), Some(me)),
            Some(ran_it),
            "the arbiter never had this run's log"
        );

        // And a finished run whose numbers nobody stamped — a row written by a build older than
        // schema v7 — still resolves somewhere rather than nowhere.
        assert_eq!(log_source(&run, None, Some(me)), Some(me));
    }

    #[tokio::test]
    async fn a_command_about_a_run_elsewhere_names_the_machine_it_is_on() {
        // The rule `offload approve` and `offload deadline` have always followed and the other
        // two did not: a command names a run, a run is on a machine, so it acts there or it says
        // it cannot. What must never happen is the answer these two used to give — cancel read
        // this node's process table and said "not running", and checkpoint said "not running
        // *here*", which is true and useless. Without a mesh there is nowhere to forward to, so
        // what is left is the sentence, and the sentence has to carry the machine.
        let dir = std::env::temp_dir().join(format!("offload-route-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let me = NodeId::from_bytes([2; 32]);
        let desktop = NodeId::from_bytes([9; 32]);
        let ctx = ctx_with(&dir, me);

        let mut elsewhere = a_run(me);
        let epoch = elsewhere
            .assign(desktop, now_millis(), offload_core::Millis(60_000))
            .expect("assign");
        elsewhere
            .started(desktop, epoch, now_millis())
            .expect("start");
        ctx.store.save_run(&elsewhere).expect("save");

        // The **whole** id, not `short()`, whose own doc comment says never to use it for a
        // lookup: twelve characters is exactly the 48-bit millisecond clock at the front of a
        // UUIDv7, so two runs minted in one millisecond print the same twelve and `resolve_run`
        // correctly refuses both as ambiguous. This test mints two, and it failed that way about
        // as often as the two `now_v7()` calls landed in one tick — a flake whose message
        // ("ambiguous — use more characters") points at the id and not at what is under test.
        let id = elsewhere.id.to_string();
        for response in [cancel(&ctx, &id).await, checkpoint(&ctx, &id).await] {
            let Response::Error { message } = response else {
                panic!("a run on another machine cannot be acted on here: {response:?}");
            };
            assert!(
                message.contains(&desktop.short()),
                "the refusal has to say where the run is: {message}"
            );
        }

        // And a run nobody holds: a checkpoint is a pause in a *process*, so there is nothing to
        // ask and nowhere to ask it. Saying "not running here" would blame this machine for a
        // run that is not running anywhere.
        let mut pending = a_run(me);
        pending.spec.queue = true;
        ctx.store.save_run(&pending).expect("save");
        let Response::Error { message } = checkpoint(&ctx, &pending.id.to_string()).await else {
            panic!("a pending run has no turn boundary coming");
        };
        assert!(
            message.contains("pending") && message.contains("nobody is running"),
            "the run's own state is the reason, not this machine: {message}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_malformed_request_gets_an_error_not_a_dropped_connection() {
        let dir = std::env::temp_dir().join(format!("offload-sock2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("s.sock");
        let listener = bind(&path).expect("bind");

        let cfg = Arc::new(Config {
            state_dir: dir.clone(),
            ..Config::default()
        });
        let node_id = NodeId::from_bytes([2; 32]);
        let store = offload_store::Store::open_memory().expect("store");
        let ctx = Ctx {
            models: std::sync::Arc::default(),
            supervisor: Supervisor::new(cfg.clone(), node_id, store.clone()).with_private_ledger(),
            store,
            config: cfg,
            node_id,
            capabilities: Arc::new(crate::deliver::Current::new(
                offload_core::Capabilities::empty(
                    offload_core::Os::Linux,
                    offload_core::Arch::X86_64,
                    offload_core::DeviceClass::Laptop,
                ),
            )),
            cluster: None,
            mesh: None,
            triggers: crate::trigger::Triggers::new(),
        };

        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle(stream, ctx).await;
            }
        });

        let stream = UnixStream::connect(&path).await.expect("connect");
        let (read, mut write) = stream.into_split();
        write.write_all(b"{ not json\n").await.expect("write");
        let mut lines = BufReader::new(read).lines();
        let line = lines.next_line().await.expect("read").expect("a response");

        let response: Response = serde_json::from_str(&line).expect("decode");
        assert!(
            matches!(response, Response::Error { message } if message.contains("malformed")),
            "expected a legible error, got {line}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
