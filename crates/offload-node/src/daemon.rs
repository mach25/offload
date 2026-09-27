//! The daemon itself, as a function: everything `offloadd` does once it has a config (ADR-0070).
//!
//! It lived in `main.rs` as one long `main`, which was fine while the only host was a process. An
//! iOS app cannot launch a separate program, so it links this crate and calls [`run`] with its own
//! shutdown signal; the `offloadd` binary calls it with SIGTERM/SIGINT. Moved, not rewritten: the
//! body is what `main` did, and the one line that changed is how it learns to stop.

use crate::{config::Config, identity, server, Supervisor};
use anyhow::{Context, Result};
use std::sync::Arc;

/// Run the daemon on `config` until `shutdown` resolves, then drain and stop.
///
/// The caller owns logging (a binary installs a subscriber, an app routes it elsewhere) and owns
/// the state directory's config; this creates the directory, takes its lock and does the rest.
pub async fn run(config: Config, shutdown: impl std::future::Future<Output = ()>) -> Result<()> {
    std::fs::create_dir_all(&config.state_dir)
        .with_context(|| format!("creating state dir {}", config.state_dir.display()))?;

    // Before anything reads or writes a thing in there. Two daemons on one state directory used
    // to *both start*: the second unlinked the first's socket, so the first kept its runs and
    // its leases and stopped being reachable — one registry, two writers, and no symptom. Held
    // for the life of the process and released by the process ending, however it ends.
    // Named rather than `_`-prefixed because it is no longer only a guard: `leftovers::sweep`
    // takes it, so holding this lock is a precondition the compiler checks rather than an
    // ordering somebody has to preserve.
    let state_dir_lock = crate::statedir::lock(&config.state_dir)?;
    let config = Arc::new(config);

    let identity = identity::load_or_create(&config.state_dir).context("node identity")?;
    let node_id = identity.id();
    report_membership(&config.state_dir, node_id, &config.name);

    // Probed facts plus nominated ones: an agent is discovered on the machine, and a route to
    // a human is declared by its owner (ADR-0010). Both are capabilities; only one is probeable.
    //
    // **Once at startup was the bug** (ADR-0048). The comment that stood here said "phase 3 will
    // re-probe on a schedule to keep gossip honest. For a single node with no one to tell, once
    // is enough" — and phase 3 built exactly that: a re-probe that told the *cluster* and nobody
    // else, in a loop that only runs when there is one. The second sentence stopped being true
    // when ADR-0046 and ADR-0047 gave a single node's own doors something to decide with it.
    // `deliver::Current` is the one copy now, `reprobe` is the one writer, and the fleet's copy
    // is still set from the same value in the same pass.
    // Empty until `read_models` has asked the agent, a couple of seconds from now (ADR-0080):
    // no list is advertised rather than a guessed one.
    let models = Arc::new(crate::deliver::Models::default());
    let capabilities = Arc::new(crate::deliver::Current::new(crate::deliver::capabilities(
        &config, &models,
    )));
    let device_class = capabilities.now().device_class;

    let agent_status = capabilities
        .now()
        .agent(&offload_core::AgentKind::ClaudeCode)
        .map_or_else(
            // Named, because "no agent installed" about a machine that has one is a sentence
            // somebody argues with. What it means is that *this* program did not answer
            // `--version`, and the path is the whole of what they can act on.
            || format!("no agent at {}", config.agent.binary.display()),
            |capability| {
                format!(
                    "claude-code {} ({})",
                    capability
                        .details
                        .agent()
                        .map_or("?", |details| details.version.as_str()),
                    if capability.authenticated {
                        "authenticated".to_string()
                    } else if capability.description.is_empty() {
                        "NOT authenticated".to_string()
                    } else {
                        // …and the clause, where there is one. `agent.account` naming a login
                        // this node is not on produces an agent that is installed, logged in,
                        // and unusable — and "no authenticated agent" about a machine that is
                        // authenticated is the sentence somebody argues with, three lines above
                        // the one that already makes this point about the binary.
                        format!("NOT authenticated — {}", capability.description)
                    }
                )
            },
        );

    let socket_path = config.socket_path();
    let listener = server::bind(&socket_path)
        .with_context(|| format!("binding control socket {}", socket_path.display()))?;

    tracing::info!(
        node_id = %node_id,
        name = %config.name,
        device = ?device_class,
        agent = %agent_status,
        socket = %socket_path.display(),
        "offloadd ready"
    );

    if capabilities
        .now()
        .agent(&offload_core::AgentKind::ClaudeCode)
        .is_none_or(|a| !a.authenticated)
    {
        // Not fatal: the node is still useful for inspection, and refusing to start would
        // be a worse experience than saying so plainly. And with ADR-0019's task tier it is
        // no longer even the whole story — a node with no agent can still host tasks, which
        // is the point of having a tier that costs nothing to keep awake.
        if config.tasks.is_empty() {
            tracing::warn!("no authenticated agent — this node cannot host runs");
        } else {
            tracing::warn!(
                tasks = config.tasks.len(),
                "no authenticated agent — this node can host tasks but not agent runs"
            );
        }
    }
    // Said once at startup rather than on every probe: a warning repeated every thirty
    // seconds is one nobody reads.
    crate::task::check(&config);

    let store = offload_store::Store::open(&config.state_dir).context("opening state store")?;
    // Before the supervisor, which is what builds checkouts there (ADR-0074).
    crate::statedir::claim_checkouts(&config.checkouts_dir(), &config.state_dir, node_id)?;
    tracing::info!(checkouts = %config.checkouts_dir().display(), "run checkouts go here");
    let supervisor = Supervisor::new(config.clone(), node_id, store.clone());
    // What this device is, for the one question a *start* has to ask about it: how many sessions
    // the agent install sustains. The bid round reads it out of `Capabilities` already; nothing
    // on a start path did, so the ceiling shaped what a node bid and stopped nothing.
    supervisor.capabilities_via(capabilities.clone());

    // …and any agent still *running* belonged to it too. It has to go first: a daemon that is
    // killed rather than shut down leaves its agent working, and nothing else in the product can
    // reach it — `offload cancel` goes to whoever holds the run now, which is a different
    // machine, and fencing only refuses writes to a record this process is not making
    // (`crate::leftovers`). Before `recover`, because that walks past a run the fleet has since
    // moved on, which is exactly the case this leaves behind.
    match crate::leftovers::sweep(&config.state_dir, &state_dir_lock) {
        0 => {}
        n => tracing::warn!(count = n, "stopped agents left behind by a previous daemon"),
    }

    // Any run the store still thinks is active belonged to a daemon that is gone. Say so
    // rather than letting `ps` claim work is happening that is not.
    match supervisor.recover() {
        Ok(0) => {}
        Ok(n) => tracing::warn!(count = n, "marked interrupted runs as failed"),
        Err(e) => tracing::error!(error = %e, "could not reconcile previous runs"),
    }
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // The mesh, if this node belongs to a fleet. Everything below tolerates its absence:
    // a fleet of one is how phases 1 and 2 ran, and still how most of this runs.
    let policy = config.work_policy(device_class);
    let mesh = match crate::mesh::start(
        &config,
        &identity,
        &capabilities.now(),
        policy.clone(),
        crate::fleet::load(&config.state_dir).unwrap_or_default(),
        store.clone(),
    ) {
        Ok(mesh) => mesh,
        Err(e) => {
            // Not fatal: the node still hosts local runs, and refusing to start would turn a
            // networking problem into a total outage.
            tracing::error!(error = %e, "could not join the mesh");
            None
        }
    };

    // Whether or not there is a fleet to answer to: a node with no mesh still hands out leases
    // — its own — and still has runs whose agents fall over at 02:00.
    //
    // **After the mesh**, which it did not used to be. Nothing here waits on the fleet's
    // *opinion*, and this pass is still out of the gossip loop for that reason — what it needs
    // is one fact about the fleet's **size**, because a failed run handed to a fleet with no
    // second member is a run nothing will ever offer again. `mesh::start` does not await a dial
    // (that would delay everything below it, which this file has a comment about), so the cost
    // of the move is nothing.
    tokio::spawn(tend_own_runs(
        supervisor.clone(),
        config.clone(),
        capabilities.clone(),
        device_class,
        mesh.as_ref().map(|mesh| mesh.cluster.clone()),
        shutdown_rx.clone(),
    ));

    // Built once and shared by both halves of the delivery plane: the loop that sends *our*
    // notifications, and the answer this node gives when a peer asks it to carry one.
    let sinks = Arc::new(crate::deliver::Sinks::from_config(&config));

    let mesh = mesh.map(Arc::new);
    if let Some(mesh) = &mesh {
        // Checkpoints get copied off this machine from here on. Without this a run survives
        // exactly as long as this daemon does, which is the failure phase 4 exists to fix.
        supervisor.peers_via(mesh.clone() as Arc<dyn crate::supervisor::Peers>);
        // And this node can now be asked to take somebody else's run.
        mesh.cluster.hosts_runs(crate::mesh::NodeHost::new(
            supervisor.clone(),
            &mesh.cluster,
            config.state_dir.clone(),
        ));
        // …and to carry somebody else's notification, which is a separate registration on
        // purpose: the device this plane exists for answers `NoHost` to every run (ADR-0010).
        // …and to let a peer's run reach a resource this device holds, which is the third
        // registration and the one that hands something *out* (ADR-0011).
        mesh.cluster
            .offers_resources(crate::resource::NodeResources::new(
                config.clone(),
                &mesh.cluster,
            ));
        mesh.cluster.delivers(Arc::new(crate::mesh::Delivery::new(
            sinks.clone(),
            &mesh.cluster,
            offload_core::Millis(config.cluster.bid_window_ms),
        )));
        match mesh.address() {
            Ok(address) => tracing::info!(%address, "listening for peers"),
            Err(e) => tracing::warn!(error = %e, "cluster endpoint has no address"),
        }
        tokio::spawn(mesh.cluster.clone().serve());
        tokio::spawn(mesh.cluster.clone().run(shutdown_rx.clone()));

        // What this node is doing, and what it is, both gossiped by polling rather than by a
        // hook: a poll cannot miss an update, and at one second over an in-memory list it
        // costs nothing. Both bump the incarnation only when something actually changed.
        // And notice when the fleet's own front door is used: `offload grant` and
        // `offload revoke` write `fleet.json` with no daemon involved.
        tokio::spawn(watch_membership(
            mesh.clone(),
            config.clone(),
            store.clone(),
            supervisor.clone(),
            shutdown_rx.clone(),
        ));
        // A revocation typed on one machine has to reach the others, which is the one
        // membership fact gossip carries (ADR-0012). Registered separately from hosting and
        // delivery for their reason: a node with no fleet registers none of the three.
        mesh.cluster
            .members_via(mesh.membership().clone() as Arc<dyn offload_cluster::Members>);

        tokio::spawn(report_local_facts(
            mesh.clone(),
            supervisor.clone(),
            store.clone(),
            config.clone(),
            device_class,
            shutdown_rx.clone(),
        ));

        // Addresses in gossip (ADR-0076), for the peers mDNS does not reach — mDNS on or off.
        tokio::spawn(mesh.clone().gossip_addresses(policy.clone()));

        if config.cluster.mdns {
            let mesh = mesh.clone();
            let name = config.name.clone();
            let policy = policy.clone();
            tokio::spawn(async move {
                if let Err(e) = mesh.discover(name, policy).await {
                    tracing::warn!(error = %e, "LAN discovery stopped");
                }
            });
        }

        let seeds = config.cluster.seeds.clone();
        let cluster = mesh.cluster.clone();
        let bootstrap = Bootstrap {
            seeds,
            policy,
            cluster,
        };
        // Spawned, not awaited: this now runs for the life of the process, and startup must not
        // wait on a seed that is not answering.
        tokio::spawn(bootstrap.run(mesh.clone(), shutdown_rx.clone()));
    }

    // Notifications, whether or not there is a fleet: a run that finishes at 02:00 on a machine
    // nobody is logged into is exactly what the delivery plane exists for (ADR-0010), and a fleet
    // of one is where most of this project runs. Spawned after the mesh so that a fleet's routes
    // are reachable from the first pass rather than the second.
    // The disk, for the reason `tend_own_runs` is where it is: this is about *this node's own*
    // state, so the gossip tick would mean it silently does not run on a fleet of one.
    tokio::spawn(tend_the_disk(
        store.clone(),
        supervisor.clone(),
        config.clone(),
        mesh.clone(),
        shutdown_rx.clone(),
    ));

    // With or without a fleet, and after the mesh so it can gossip what it finds: what this
    // device *is* changes under a running daemon — a laptop unplugged, a link that becomes
    // metered — and everything that decides here reads `capabilities` (ADR-0048).
    tokio::spawn(reprobe(
        config.clone(),
        capabilities.clone(),
        models.clone(),
        mesh.as_ref().map(|m| m.cluster.clone()),
        device_class,
        shutdown_rx.clone(),
    ));
    // The agent's model list, read now and again when the agent changes or somebody asks
    // (ADR-0080). After `reprobe`, which it nudges when the list changes.
    tokio::spawn(read_models(
        config.clone(),
        capabilities.clone(),
        models.clone(),
        mesh.as_ref().map(|m| m.cluster.clone()),
        shutdown_rx.clone(),
    ));

    // The delivery pass is spawned from here too, and it is the third unattended pass to move
    // in: a trigger's watchers, the clock, and now the notification plane all need the one
    // `Ctx` — because since ADR-0057 a notice may fire a **rule**, and firing one goes through
    // the same submission checks as everything else. Two `Ctx`s would be two `Triggers` maps
    // and two capability handles, which is the divergence ADR-0048 was written about.
    let server = tokio::spawn(server::serve(
        listener,
        supervisor.clone(),
        store.clone(),
        config.clone(),
        node_id,
        capabilities.clone(),
        models,
        mesh.clone(),
        sinks,
        shutdown_rx,
    ));

    shutdown.await;

    // Hand the work over before saying goodbye, and say goodbye before going: a run moved
    // is a run that keeps its turn, and a node that announces its departure costs nobody a
    // detection timeout (ADR-0005).
    // Through `depart`, which is what records the departure — the same entry point
    // `offload drain` uses, and for a reason measured on this path: while `stop_accepting()`
    // lived in the operator's handler, a `SIGTERM`ed daemon went on bidding and *starting* work
    // for the whole of its shutdown drain, which is up to five minutes. `offload status` said
    // `accepting yes` three seconds after the signal, and the log recorded `spawning claude
    // code` for a run submitted after the daemon had been told to stop.
    //
    // `Report::silent()`: there is no client here. The steps a drain would stream are in the
    // daemon's log already, which is the only reader this path has ever had.
    let drained = crate::mesh::depart(
        &supervisor,
        mesh.as_deref(),
        std::time::Duration::from_secs(config.cluster.drain_deadline_secs),
        offload_core::Millis(config.cluster.bid_window_ms),
        // The drain is the last thing this process does, so a run still mid-turn at the deadline
        // gets no promise about its next boundary: the loop below stops those agents as soon as
        // this returns, and the runs are the orphan path's from there.
        crate::mesh::Lifespan::Exits,
        &crate::mesh::Report::silent(),
    )
    .await;
    if drained.moved > 0 || drained.left > 0 || drained.finished > 0 {
        tracing::info!(
            moved = drained.moved,
            left = drained.left,
            finished = drained.finished,
            "handed over what the fleet would take"
        );
    }
    if let Some(mesh) = &mesh {
        mesh.cluster.announce_departure().await;
    }

    // Whatever the drain above could not hand over. This is the process going away, not a
    // decision that the work was unwanted, so the agents are stopped and their records are left
    // exactly as they stand: a run still mid-turn at the drain deadline keeps its lease and is
    // recovered or reassigned the ordinary way (ADR-0007). The comment here used to say runs
    // "are cancelled rather than migrated… phase 4 replaces this with a drain", which the ten
    // lines above it had been doing for four sessions.
    tracing::info!("shutting down; stopping any agents the drain could not hand over");
    supervisor.cancel_all().await;
    let _ = shutdown_tx.send(true);

    // Give the agent processes their grace period before the runtime tears their pipes out.
    tokio::time::sleep(std::time::Duration::from_millis(
        config.agent.cancel_grace_secs.saturating_mul(100).min(2000),
    ))
    .await;

    server.abort();
    if let Some(mesh) = &mesh {
        mesh.shutdown();
    }
    let _ = std::fs::remove_file(&socket_path);
    tracing::info!("stopped");
    Ok(())
}

/// The housekeeping a node owes its own runs, fleet or no fleet.
///
/// Two things, and they are here together because they share the reason for not being in the
/// gossip loop: **neither is about the fleet.** The runs on a fleet of one have leases too, and
/// an agent that fell over at 02:00 on a single machine is exactly the case autonomy in failure
/// exists for. The version of the heartbeat that lived in the mesh tick simply did not run on a
/// node with no fleet, which was invisible for as long as the local lease was an hour.
///
/// * **Leases.** A lease is only true while somebody renews it. A holder that stopped saying so
///   is what `Orphaned` is for, so a holder that *is* here has to keep saying so — including one
///   holding a run it has not started, whose whole commitment is the lease (ADR-0006).
/// * **Failed runs.** Unattended, they pick themselves back up; attended, they stay in front of
///   the person who was watching (ADR-0013). Only the node that failed the run can answer
///   either question.
///
/// It takes the cluster for **one** fact, and the exception is worth stating rather than
/// leaving as an argument that has quietly grown: *is there anybody else at all*. Handing a
/// failed run to the fleet leaves it `Pending` for whoever arbitrates to offer, and on a node
/// with no peers that is nobody — measured, and the run then sat `pending` for ever and jammed
/// its rule. It is a question about the fleet's **size** and never about its opinion, which is
/// what keeps this pass out of the gossip loop.
async fn tend_own_runs(
    supervisor: Supervisor,
    config: Arc<Config>,
    capabilities: Arc<crate::deliver::Current>,
    device_class: offload_core::DeviceClass,
    cluster: Option<Arc<offload_cluster::Cluster>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    // Six times per lease period. The cost is one row per held run; what it buys is that a
    // lease means what it says, which nothing else here can rescue — and deriving it from
    // `LEASE` means the two cannot drift apart when somebody tunes one of them.
    let every = std::time::Duration::from_millis((offload_core::LEASE.0 / 6).max(1_000));
    let recovery = offload_core::RecoveryPolicy::default();
    loop {
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            _ = shutdown.changed() => return,
        }
        supervisor.heartbeat();

        let capacity = offload_core::Capacity::of(&config.work_policy(device_class));
        // Asked every pass, off the live handle, because this is the one input here that can
        // change while the daemon runs: a laptop unplugged, a link that becomes metered
        // (ADR-0048). A node that will not host hands the run to the fleet instead of starting
        // an agent on it (ADR-0049).
        let caps = capabilities.now();
        let policy = config.work_policy(caps.device_class);
        // Asked per run, off **one** read of the live handle (ADR-0048): since ADR-0019 §4 the
        // owner's standing answer depends on the run's own demand, and this pass walks a list
        // that may hold a light watcher beside an expensive session. Both facts — the tier and
        // the demand — come off the run, so there is nothing here to stand in for either.
        let hosting = |run: &offload_core::Run| {
            if policy
                .permits(&caps, run.spec.work.tier(), run.spec.demand)
                .is_ok()
            {
                offload_core::Hosting::Allowed
            } else {
                offload_core::Hosting::RefusedByOwner
            }
        };
        for run in supervisor
            .recover_failed_runs(
                capacity,
                &recovery,
                &hosting,
                // Asked every pass, because a fleet gains and loses members while a daemon
                // runs: the answer at startup is not the answer at 02:00.
                cluster
                    .as_ref()
                    .is_none_or(|cluster| cluster.view().alone()),
            )
            .await
            .resumed
        {
            // The verb the run's own tier uses. `Recovery::Resume` is one decision and two
            // mechanisms (ADR-0058), so a line that says "resumed" about a task is the report
            // this project keeps having to fix: a program that started over from its spec did
            // not continue anything.
            tracing::info!(
                run_id = %run.id,
                epoch = run.epoch.0,
                kind = run.spec.work.kind().name(),
                "picked a failed run back up"
            );
        }
    }
}

/// Reclaim the blobs no run references any more (ADR-0022).
///
/// `blobs::collect_garbage` has existed since phase 2 and was called by nothing, because it is
/// "deliberately conservative and deliberately manual". Conservative is right and earned — the
/// version it replaced matched hashes as hex against JSON that spells them as arrays of integers,
/// found no references at all, and deleted the checkpoints of live runs. **Manual** is the half
/// that stopped being true, for the third time in three ADRs: `cleanup` was never automatic
/// because somebody would read the worktree, `delete_run` was never called because somebody had
/// to decide about news still owed, and both premises name a person who is not there on the
/// machine that hosts the runs.
///
/// What it costs to leave it manual, measured rather than reasoned about: one run of six turn
/// boundaries left six blobs totalling 851 KB, of which the surviving checkpoint referenced
/// **one**, at 243 KB. `Run::record_checkpoint` replaces the checkpoint, and a transcript is the
/// whole conversation so far — so the garbage is quadratic in the conversation's size, and it is
/// every run on every node rather than anything to do with triggers.
///
/// Fifteen minutes, and an hour of grace. A blob is written *before* the row that references it
/// (`capture` puts three and only then records the checkpoint), and the same window exists on the
/// receiving side of a replication, so the grace has to be generously longer than any of them:
/// the cost of waiting is disk already spent, and the cost of being early is a run that cannot
/// resume.
/// The second half is the checkouts (ADR-0023): a worktree a run **left**, and one an occurrence
/// was placed on a **peer**, neither of which anything has ever removed. Same tick because it is
/// the same question about the same disk, and the same reason for being here rather than in the
/// gossip loop — a peer hosting somebody else's occurrences is exactly the machine nobody logs
/// into.
async fn tend_the_disk(
    store: offload_store::Store,
    supervisor: Supervisor,
    config: Arc<Config>,
    mesh: Option<Arc<crate::mesh::Mesh>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let every = std::time::Duration::from_secs(15 * 60);
    const GRACE_MS: u64 = 60 * 60 * 1_000;
    loop {
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            _ = shutdown.changed() => return,
        }
        // Checkouts first: a reclaimed one is megabytes where a blob is kilobytes, and it needs
        // no grace of its own — the guards are about who holds the run and what is in the
        // directory, which are facts rather than windows.
        match supervisor.reclaim_departed_checkouts().await {
            0 => {}
            n => tracing::debug!(count = n, "reclaimed checkouts of runs that are elsewhere"),
        }

        // Then the records of machine-started runs the delivery plane has finished with
        // (ADR-0024). The same pass a firing runs, asked here with no rule to narrow it —
        // because the node this matters most to is a peer, which hosts the occurrences and has
        // never heard of the rule that fired them.
        let view = mesh.as_ref().map(|mesh| mesh.cluster.view());
        let pruned = crate::trigger::prune_spent_records(
            &config,
            &store,
            view.as_ref(),
            None,
            None,
            crate::trigger::Prune::AtAFiring,
            crate::supervisor::now(),
        )
        .await;
        supervisor.forget_legs(&pruned);

        let store = store.clone();
        // Reads every run row and stats every blob, so it is not something to do on the runtime.
        let pass = tokio::task::spawn_blocking(move || store.collect_garbage(GRACE_MS)).await;
        match pass {
            // A pass that fails is loud and is tried again: `collect_garbage` refuses to run at
            // all if one run row will not decode, because a run whose references are *unknown*
            // is not a run with none, and on a timer that has to stay a refusal rather than
            // become a slow leak of somebody's checkpoint.
            Ok(Err(e)) => tracing::error!(error = %e, "could not collect blobs"),
            Err(e) => tracing::error!(error = %e, "blob collection task failed"),
            Ok(Ok(_)) => {}
        }
    }
}

/// Notice when this node's membership changes underneath the daemon.
///
/// Every membership command works with no daemon and no network, because that is ADR-0012's
/// central claim — a certificate verifies against the fleet key alone, so founding, enrolling,
/// granting and revoking are local acts. The consequence is that `fleet.json` is written by a
/// process that is not this one, and for two phases nothing here noticed:
///
/// * `offload grant host-runs` re-issued this node's certificate, and the daemon kept
///   presenting the old one to every peer. Session eleven's "a grant needs no restart" was true
///   of the *checking* side and false of the granted node's own.
/// * `offload revoke` wrote a revocation the daemon never read, so the ADR's "immediate and
///   local" meant "immediate, once you remember to restart the daemon".
///
/// A poll rather than an inotify watch, for the reason every other loop here is a poll: it
/// cannot miss an update, it re-heals after any error, and it costs one small file read a second
/// on a file that changes a handful of times in a fleet's life.
async fn watch_membership(
    mesh: Arc<crate::mesh::Mesh>,
    config: Arc<Config>,
    store: offload_store::Store,
    supervisor: Supervisor,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let interval = std::time::Duration::from_secs(1);
    // What the startup line `member of fleet … grants=` said, so that the first change after it
    // is reported against the sentence somebody actually read.
    let mut announced = mesh
        .membership()
        .snapshot()
        .grants(offload_cluster::Clock::now(&offload_cluster::SystemClock));
    loop {
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => return,
        }

        // And the other direction: a certificate has a month on it and renewing it is how a
        // fleet stays a fleet (ADR-0012). Self-rate-limited, and a no-op until the last
        // quarter of its life.
        mesh.renew_if_due(offload_core::Millis(config.cluster.bid_window_ms))
            .await;

        // A member this node has never met is either an enrolment somebody performed or one
        // nobody did, and only the second kind is worth waking somebody for — which nothing
        // here can tell apart, so it says both and lets the person decide.
        mesh.note_new_members(&store);
        // Before the change, and deliberately not gated on one: the revocation may have been
        // written down a tick ago by whatever heard it, and there is no second announcement.
        stand_down_if_revoked(&mesh, &supervisor).await;
        let change = match mesh.refresh_membership().await {
            Ok(change) => change,
            Err(e) => {
                // Left alone rather than treated as an empty fleet: dropping out of our own
                // fleet over a transient read error is the wrong direction for every failure
                // this file can have.
                tracing::warn!(error = %e, "could not re-read fleet state");
                continue;
            }
        };
        // Before the `is_empty` below and not part of it: a probation lifting and a certificate
        // this daemon adopted itself both change what the node may do without changing the file.
        let now = offload_cluster::Clock::now(&offload_cluster::SystemClock);
        if let Some(grants) = mesh.membership().grants_changed(&mut announced, now) {
            let list: Vec<String> = grants.iter().map(ToString::to_string).collect();
            tracing::info!(grants = %list.join(","), "this node's grants changed");
        }
        if change.is_empty() {
            continue;
        }
        for peer in &change.revoked {
            tracing::warn!(node = %peer.short(), "revoked; hung up and refusing it from now on");
        }
        if change.refounded {
            tracing::warn!(
                "this fleet has been re-founded under a new key — every other device needs the \
                 invitation `offload rekey` printed, and until then this node is alone"
            );
        }
    }
}

/// Stop, once this node has been thrown out of its own fleet (ADR-0044).
///
/// Asked every tick as a standing condition rather than handled as a change, because being
/// revoked is not a transition anybody here is guaranteed to witness: the fact is written down by
/// whichever path hears it first — a peer's gossip, or the refusal on a dial — and an edge is
/// consumed by whoever observes it. [`Supervisor::stand_down`] is the idempotent half, so this
/// costs a lock on every tick of a fleet's whole life but one.
///
/// This used to be loud and nothing more, on the argument that the runs this node holds are still
/// its problem and dropping them would lose the work this project exists to keep. That argument
/// was about a *lapsed certificate* and it does not survive contact with a revocation: the fleet
/// has already decided this device is not to be trusted with the work, it has already taken the
/// runs back under a higher epoch, and the agent still going here is the second one on the
/// repository. Nothing is lost by stopping that this node could have delivered anyway — it can
/// reach nobody — and the worktree, the branch and the transcript all stay where they are for
/// whoever comes back to the machine.
async fn stand_down_if_revoked(mesh: &crate::mesh::Mesh, supervisor: &Supervisor) {
    if !mesh.membership().revoked_here() {
        return;
    }
    let Some(stopped) = supervisor.stand_down().await else {
        return;
    };
    tracing::error!(
        runs = stopped.len(),
        "this node has been revoked from its own fleet: it will take no more work, and the \
         agents it was running have been stopped — the fleet is running those runs elsewhere"
    );
    for run in stopped {
        tracing::warn!(
            run_id = %run,
            "stopped because this node was revoked; its worktree and branch are untouched"
        );
    }
}

/// Keep this node's own entry in the view honest: what it is running, and what it is.
///
/// Polling rather than a callback out of the supervisor. A poll cannot miss a transition, it
/// reads the same registry `offload ps` does, and it is self-correcting after any bug that a
/// push-based version would leave permanently wrong. The cost is a second of lag on a
/// kilobyte of gossip.
async fn report_local_facts(
    mesh: Arc<crate::mesh::Mesh>,
    supervisor: Supervisor,
    store: offload_store::Store,
    config: Arc<Config>,
    device_class: offload_core::DeviceClass,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let cluster = mesh.cluster.clone();
    // What this node had learned about its peers before it restarted (ADR-0031). Read once: it is
    // only ever used to *seed* a peer nothing has been learned about yet, so a row that goes stale
    // while this daemon runs is a row that has already been superseded by a live observation.
    // An unreadable table is a warning and the old behaviour — every peer starting from the policy
    // default — never a daemon that refuses to come up.
    let remembered = match store.all_observations() {
        Ok(rows) => rows
            .into_iter()
            .map(|(id, stored)| (id, stored.observation))
            .collect(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not read the absence history; the hold-down starts from the default \
                 for every peer"
            );
            std::collections::BTreeMap::new()
        }
    };
    let runs_interval = std::time::Duration::from_millis(config.cluster.probe_interval_ms.max(200));
    let mut ticks: u32 = 0;

    loop {
        tokio::select! {
            () = tokio::time::sleep(runs_interval) => {}
            _ = shutdown.changed() => return,
        }

        // A run this node accepted but had no room for starts here, as soon as there is room.
        // ADR-0006: a node that is full commits rather than declining, so something has to
        // notice when the queue drains — and it is a poll, because the slot may have been
        // freed by a run finishing, being cancelled, or migrating away.
        let capacity = offload_core::Capacity::of(&config.work_policy(device_class));
        // The account's ceiling gates *starting* as well as accepting (ADR-0013's amendment):
        // otherwise every node commits up to the cap and then starts straight through it.
        let room = offload_core::Room::new(
            capacity,
            cluster
                .view()
                .account_use(&cluster.node(), &offload_core::AgentKind::ClaudeCode),
            // The account's *rate limit* is not passed in: `Supervisor::room` overlays its own,
            // because it is the node's own observation and a caller supplying it was a caller
            // repeating the node (ADR-0029). This is still the loop that starts a held run once
            // the limit lifts, because it is a poll and cannot miss the moment.
            None,
        );
        for run in supervisor.start_held_runs(room).await {
            tracing::info!(run_id = %run.id, "started a run that was waiting for a slot");
        }

        // And the opposite question about the same runs: is a commitment this node made still
        // worth keeping? A run that is due while it waits for a slot here belongs to whoever
        // can start it now — offered first, never abandoned (ADR-0006, ADR-0013).
        mesh.hand_back_late_commitments(
            &supervisor,
            capacity,
            offload_core::Millis(config.cluster.bid_window_ms),
        )
        .await;

        if cluster.set_running(supervisor.active_runs()) {
            tracing::debug!("workload changed; gossiping it");
        }
        // Forget what has stopped being news, by the tail: without this the view kept every
        // finished run it was ever sent, and every probe carried the fleet's whole history (a
        // quarter of a core idle on the phone). **Before** publishing, because what is published
        // next includes finished runs not yet told to anybody, older than the tail, and
        // forgetting after would take them straight back out (session ninety-four).
        let tail_start = offload_cluster::Clock::now(&offload_cluster::SystemClock)
            .saturating_sub(Supervisor::GOSSIP_TAIL);
        let forgotten = cluster.forget_finished_before(tail_start);
        if forgotten > 0 {
            tracing::debug!(
                forgotten,
                "finished runs aged out of what this node gossips"
            );
        }
        // The run records themselves, so the node that submitted one learns what became of
        // it. Published wholesale each tick: the store is the truth, and a poll cannot miss a
        // transition the way a hook can. A finished run stays until it has been out in an
        // exchange, however long this node was alone.
        cluster.publish_runs(supervisor.runs_to_publish(cluster.exchanges()));
        // And what they have done and cost. Beside the records rather than in them: a run's
        // state is arbitrated by epoch and its numbers by being further along, and a node
        // that only ever submitted a run has the record and none of the numbers.
        cluster.publish_progress(supervisor.gossipable_progress());
        // And the schedules, own and learned alike (ADR-0056). Republished from the store each
        // tick for `publish_runs`' reason: the store is the truth, a poll cannot miss a change
        // the way a hook can, and the copy the cluster gossips is a cache of it.
        match store.schedules() {
            Ok(schedules) => cluster.publish_schedules(schedules),
            Err(e) => tracing::warn!(error = %e, "could not read schedules to gossip"),
        }

        // And the other half of the same question: has anybody *else* gone away, and what
        // does that mean for the runs they were holding (ADR-0007)?
        mesh.supervise(
            &supervisor,
            offload_core::Millis(config.cluster.bid_window_ms),
        )
        .await;

        // Seed peers from what this node remembers, and write down what it knows now — one pass,
        // because they are the same fact in opposite directions (ADR-0031). Every tick for the
        // seeding, which is idempotent and is how a peer met a moment ago gets its history; the
        // *writing* is on the reprobe cadence below, because a row per peer per second is a lot of
        // small writes to buy a second of freshness in a value measured in minutes.
        let observed = cluster.exchange_absence_history(&remembered);

        ticks = ticks.wrapping_add(1);
        if ticks % REPROBE_EVERY == 0 {
            for (id, observation) in &observed {
                let stored = offload_store::observations::StoredObservation {
                    observation: observation.clone(),
                    // Not persisted: whether a peer is away *right now* is this incarnation's
                    // observation and a restart has no business believing it. What survives is
                    // how long its absences have historically lasted.
                    absent_since: None,
                    last_seen: crate::supervisor::now(),
                };
                if let Err(e) = store.save_observation(*id, &stored) {
                    tracing::debug!(node = %id.short(), error = %e, "could not save an observation");
                }
            }
        }
    }
}

/// Ticks of the gossip loop between re-probes, and now the multiplier [`reprobe`] uses for its
/// own sleep. Capabilities are re-probed far less often than the run set changes: probing shells
/// out to the agent binary, and a battery percentage nobody reads for thirty seconds is not the
/// thing that will go wrong here. The *absence history* is written on this cadence too, which is
/// why the number is still read from the gossip loop.
const REPROBE_EVERY: u32 = 30;

/// Ask the machine what it is, on a schedule, and tell everything that decides with the answer.
///
/// **Its own task, spawned whether or not there is a fleet.** This lived inside the gossip loop,
/// which is the loop a fleet of one never enters — the same mistake `tend_own_runs` exists to
/// avoid, one fact over: *a node's own state needs tending whether or not there is a fleet.* A
/// laptop alone therefore probed once, at startup, and believed it for as long as it ran.
///
/// **One writer, two readers, one pass.** `Current` is what this daemon's own doors read
/// (ADR-0048) and `Cluster::set_capabilities` is what the fleet reads, and they are set from the
/// same probe in the same statement so they cannot answer differently. The gossip decision stays
/// the cluster's: `refresh` reports a change so that a node with nobody to tell still says so
/// once, and the cluster decides for itself whether the change is worth an incarnation.
///
/// Probing shells out — to the agent binary, to `nmcli`, to `/sys` — so the cadence is minutes
/// rather than seconds, and it is derived from the same numbers the gossip loop used before this
/// moved, so that a walk can shorten one knob and get both.
async fn reprobe(
    config: Arc<Config>,
    capabilities: Arc<crate::deliver::Current>,
    models: Arc<crate::deliver::Models>,
    cluster: Option<Arc<offload_cluster::Cluster>>,
    device_class: offload_core::DeviceClass,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let every = reprobe_every(&config);
    loop {
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            // A new model list is a change to what this device is, and waiting up to a probe
            // interval to say so would make somebody's Refresh look like it did nothing.
            () = models.changed() => {}
            _ = shutdown.changed() => return,
        }
        let probed = crate::deliver::capabilities(&config, &models);
        let policy = config.work_policy(device_class);
        let changed = capabilities.refresh(probed.clone());
        if !changed.is_empty() {
            tracing::info!(changed = %changed.join(","), "what this device is has changed");
        }
        if let Some(cluster) = &cluster {
            if cluster.set_capabilities(probed, policy) {
                tracing::info!("capabilities changed; gossiping them");
            }
        }
    }
}

fn reprobe_every(config: &Config) -> std::time::Duration {
    std::time::Duration::from_millis(
        config.cluster.probe_interval_ms.max(200) * u64::from(REPROBE_EVERY),
    )
}

/// Reading the agent's model list (ADR-0080): at startup, whenever the probe sees the agent's
/// version or login change, and whenever somebody asks — here, or anywhere in the fleet.
///
/// Its own loop rather than part of `reprobe`, because a read starts the agent for a couple of
/// seconds and is wanted a few times a day, not every probe. It never writes the capabilities:
/// it stores the list and `Models::store` wakes `reprobe`, the one writer (ADR-0048).
///
/// **The fleet's requests** arrive as `Cluster::models_asked`, a count merged by maximum, which
/// wakes this loop on every rise after the node's first gossip exchange (a count heard in that
/// exchange predates this node, and its startup read answers it). A request made here reads at
/// once and raises the count itself, and consumes that rise, so it is not read a second time.
async fn read_models(
    config: Arc<Config>,
    capabilities: Arc<crate::deliver::Current>,
    models: Arc<crate::deliver::Models>,
    cluster: Option<Arc<offload_cluster::Cluster>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let every = reprobe_every(&config);
    let mut asked = cluster.as_ref().map(|c| c.models_asked());
    let mut why = Some("startup");
    loop {
        if why.is_none() {
            tokio::select! {
                () = tokio::time::sleep(every) => {}
                () = models.asked() => why = Some("asked on this node"),
                () = next_request(&mut asked) => why = Some("asked by the fleet"),
                _ = shutdown.changed() => return,
            }
        }
        // Passed on to the fleet before anything else, and whether or not this device has an
        // agent of its own: the phone somebody presses Refresh on is the device with nothing to
        // read, and the first cut returned before raising the count, so its request went nowhere.
        if why == Some("asked on this node") {
            if let Some(cluster) = &cluster {
                cluster.ask_for_models();
                // Our own raise, already being answered here: consume it rather than read twice.
                if let Some(rx) = asked.as_mut() {
                    rx.borrow_and_update();
                }
            }
        }
        let now = capabilities.now();
        let Some(agent) = now.agent(&offload_core::AgentKind::ClaudeCode) else {
            why = None;
            continue;
        };
        let authenticated = agent.authenticated;
        let version = agent
            .details
            .agent()
            .map(|details| details.version.clone())
            .unwrap_or_default();
        let held = models.held();
        let changed = held
            .as_ref()
            .is_none_or(|read| read.version != version || read.authenticated != authenticated);
        let Some(reason) = why.take().or(changed.then_some("the agent changed")) else {
            continue;
        };
        if !authenticated {
            // A logged-out agent reaches nothing, whatever it would list.
            models.store(crate::deliver::ModelsRead {
                version,
                authenticated,
                models: Vec::new(),
            });
            continue;
        }
        let reading = config.clone();
        let read = tokio::task::spawn_blocking(move || crate::deliver::read_models(&reading)).await;
        match read {
            Ok(Ok(list)) => {
                let count = list.len();
                // Every read, changed or not: a few a day, and a Refresh that found nothing new
                // has to be visible as having happened.
                let changed = models.store(crate::deliver::ModelsRead {
                    version,
                    authenticated,
                    models: list,
                });
                tracing::info!(
                    models = count,
                    changed,
                    reason,
                    "read the agent's model list"
                );
            }
            outcome => {
                let error = match outcome {
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                    Ok(Ok(_)) => String::new(),
                };
                // The same agent's last list still describes it; a new agent's does not.
                if changed {
                    models.store(crate::deliver::ModelsRead {
                        version,
                        authenticated,
                        models: Vec::new(),
                    });
                }
                tracing::warn!(%error, reason, kept = !changed, "could not read the agent's model list");
            }
        }
    }
}

/// The fleet's next request, or never when there is no fleet or it has gone.
async fn next_request(asked: &mut Option<tokio::sync::watch::Receiver<u64>>) {
    if let Some(rx) = asked {
        if rx.changed().await.is_ok() {
            rx.borrow_and_update();
            return;
        }
    }
    std::future::pending().await
}

/// Dialling the seed list — at startup, and again whenever there is nobody left to talk to.
///
/// **Not once.** It used to be called from `main` exactly one time, while the LAN path
/// (`Mesh::discover`) says in its own doc comment that it "runs until the process ends" — so the
/// self-healing existed for multicast and not for the internet (ADR-0037). A phone that loses its
/// link to a cell handover, to Doze, or to the laptop's new address had no way back until
/// `offloadd` restarted. Dialling again makes all three of those one event.
///
/// Spawned rather than awaited, which is the other half: a daemon must not sit in startup waiting
/// for a seed that is not answering yet.
struct Bootstrap {
    seeds: Vec<String>,
    policy: offload_core::WorkPolicy,
    cluster: std::sync::Arc<offload_cluster::Cluster>,
}

/// How long to wait before dialling the seed list again, and the ceiling it backs off to.
///
/// The first retry is quick because the common case is a phone that just woke up. The ceiling
/// exists because the other common case is a device with no network at all, and a radio woken
/// every five seconds all night is a battery nobody gets back. Reset the moment a peer answers.
const RESEED_FIRST: std::time::Duration = std::time::Duration::from_secs(5);
const RESEED_MAX: std::time::Duration = std::time::Duration::from_secs(120);

impl Bootstrap {
    async fn run(
        self,
        mesh: std::sync::Arc<crate::mesh::Mesh>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        if self.seeds.is_empty() {
            tracing::info!(
                node = %self.cluster.node().short(),
                "no seeds configured; waiting to be found"
            );
            return;
        }
        let mut wait = RESEED_FIRST;
        loop {
            // Short-circuiting on purpose: a fleet that is talking never dials, so the whole
            // loop costs one predicate over the view in the state a healthy fleet is in.
            let talking = mesh.has_a_live_peer() || mesh.bootstrap(&self.seeds, &self.policy).await;
            if talking {
                wait = RESEED_FIRST;
            } else {
                // Nobody answered. Back off, because the reason is as likely to be "this device
                // has no network" as "that address is stale".
                wait = (wait * 2).min(RESEED_MAX);
                tracing::debug!(
                    seeds = self.seeds.len(),
                    retry_in = ?wait,
                    "no seed answered and no live peer; will dial again"
                );
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => return,
            }
        }
    }
}

/// Say what fleet this node belongs to, if any, and complain if its certificate is no good.
///
/// Worth its own line at startup because the failure it catches is otherwise silent: a
/// certificate that lapsed while the daemon was down looks exactly like one that works, and
/// the symptom is peers quietly declining to talk. `mesh::start` refuses to listen in that
/// case; this is what says why.
fn report_membership(state_dir: &std::path::Path, node_id: offload_core::NodeId, name: &str) {
    let now = offload_core::Millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
    );

    match crate::fleet::load(state_dir) {
        Ok(Some(state)) => match state.check(node_id, now) {
            Ok(()) => {
                // Two names for one machine, and peers only ever see one of them. The
                // certificate's is signed, so it is what the fleet displays (`record_name`); the
                // config's is what every local command shows. They are set at different moments
                // — enrolment usually happens before there is a config — so disagreeing is
                // ordinary rather than a mistake, and being told beats wondering why the desktop
                // shows up as `fedora` on the phone.
                let certified = &state.membership.name;
                if !certified.is_empty() && certified != name {
                    tracing::warn!(
                        configured = %name,
                        certified = %certified,
                        "this node's certificate names it differently from its config; peers \
                         show the certificate's name, because that one is signed"
                    );
                }
                tracing::info!(
                fleet = %state.fleet,
                grants = %state
                    .grants(now)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                    "member of fleet"
                );
            }
            Err(e) => tracing::warn!(fleet = %state.fleet, error = %e, "membership is not usable"),
        },
        Ok(None) => tracing::info!(
            "not a member of any fleet — `offload init` founds one, `offload join` enrols here"
        ),
        Err(e) => tracing::warn!(error = %e, "could not read fleet state"),
    }
}
