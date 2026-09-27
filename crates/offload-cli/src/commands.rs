//! Subcommands that talk to a running daemon.

use crate::client;
use anyhow::{bail, Context, Result};
use offload_core::PermissionMode;
use offload_node::api::{
    LogKind, Request, Response, RuleRequest, RunSummary, Steward, SubmitRequest,
};
use offload_node::deliver::Reach;
use std::path::Path;

pub async fn status(socket: &Path) -> Result<()> {
    let responses = client::request(socket, &Request::Status).await?;
    let Response::Status(s) = client::expect_one(responses)? else {
        bail!("unexpected response to status");
    };

    println!("node        {}  ({})", s.name, s.device_class);
    println!("id          {}", s.node_id);
    println!("state dir   {}", s.state_dir);
    println!(
        "agent       {}",
        match (&s.agent, s.agent_authenticated) {
            (Some(v), true) => format!("claude-code {v}, authenticated"),
            (Some(v), false) => format!("claude-code {v}, NOT authenticated"),
            // Which program was asked, because that is the only actionable half: the daemon
            // probes `agent.binary` and spawns it, so a machine with an agent somewhere else on
            // it is told where this node was looking rather than left to argue with the word
            // "none".
            (None, _) => format!("none — nothing at {}", s.agent_binary),
        }
    );
    // Who this node is logged in as, and where that came from (ADR-0028). Printed unconditionally
    // rather than only when it is configured, because "which account does this machine spend on"
    // is a question about the machine and not about whether somebody wrote the answer down — and
    // a device with two logins on it is exactly where the default is the wrong guess.
    println!(
        "account     {}{}",
        s.agent_account
            .as_deref()
            .unwrap_or("none this node can identify"),
        if s.agent_state_dir.is_empty() {
            String::new()
        } else {
            format!("  ·  from {}", s.agent_state_dir)
        }
    );
    // The one ceiling on this list that no operator can raise, and the answer to "the machine is
    // idle and my run has not started" (ADR-0029). Said only while it is holding: a line about a
    // rate limit on a node that is not near one is noise on the command somebody runs most.
    if let Some(until) = s.agent_limited_until_unix_ms {
        println!(
            "            └─ rate-limited: new runs start in {}",
            crate::when::until(until)
        );
    }
    println!(
        "runs        {}/{}{}",
        s.running,
        s.max_concurrent,
        // The agent's ceiling, when that is the one that stops a third run rather than the
        // owner's. Said here rather than left to the refusal line, because the refusal only
        // appears once the node is already full.
        match s.agent_max_concurrent {
            Some(sustains) => format!("  ·  claude-code sustains {sustains}, which is what binds"),
            None => String::new(),
        }
    );
    println!(
        "budget      {}/{} shares{}",
        s.committed_shares,
        s.budget,
        match s.cpu_percent {
            // Both numbers, because the interesting case is when they disagree: a budget that
            // says half free on a machine whose load average says otherwise is what refuses
            // work here, and one number alone cannot show it (ADR-0013).
            Some(p) => format!("  ·  cpu {p}%"),
            None => "  ·  cpu not reported".to_string(),
        }
    );
    // Beside the load, and only where a host says (ADR-0068).
    if let Some(sleep) = &s.sleep {
        println!("sleep       {sleep}");
    }
    if let Some(thermal) = &s.thermal {
        println!("thermal     {thermal}");
    }
    if let Some(key) = &s.approval_key {
        println!("approval    {key}");
    }
    match &s.refusal {
        None => println!("accepting   yes"),
        Some(reason) => println!("accepting   no — {reason}"),
    }
    // …and the owner's other answer, where they have given one (ADR-0019 §4). Only when it
    // differs from the line above, which the daemon decides — the two answers come from one
    // `admits`, asked twice, so this cannot disagree with the door. A device configured for
    // light work only used to report `accepting no` one command after running a task.
    match &s.light_refusal {
        None => {}
        Some(offload_node::api::LightAnswer::Accepted) => {
            println!("            …but light work: yes");
        }
        Some(offload_node::api::LightAnswer::Refused { reason }) => {
            println!("            …and light work: no — {reason}");
        }
    }
    // The store's size, and how much of it is other machines' work (ADR-0025). Printed here
    // because there was nowhere else it could appear: `offload rules` prints what a rule is
    // keeping and `offload sinks` prints the queue behind a silence, and the machine these pile
    // up on fastest is the one with neither — a peer that hosts nothing and learns every record
    // by gossip.
    println!(
        "records     {}{}",
        s.records,
        match s.records_learned {
            0 => String::new(),
            n => format!("  ·  {n} for work this node neither ran nor submitted"),
        }
    );
    // ADR-0061 §4's *say the cap before the expensive step*. An agent choosing which 12 MB of a
    // 6.5 GB directory travels is working to a budget, and the other place this number appears is
    // a refusal — which arrives after the packing, and is therefore the expensive way to learn it.
    // Printed unconditionally, because "there is no archive here yet" is exactly when it is worth
    // knowing.
    // Zero means *this daemon did not say* — it is a `#[serde(default)]` field and an older
    // daemon sends nothing — so the line is omitted rather than printed as `archives up to 0
    // bytes`, which is a wrong sentence rather than a missing one. The CLI and the daemon ship
    // together, so this is only ever the mixed-version moment.
    if s.max_archive_bytes > 0 {
        println!(
            "workspace   archives up to {}  ·  a workspace over that has to be a smaller selection",
            human_bytes(s.max_archive_bytes)
        );
    }
    if s.stray_checkouts > 0 {
        // ADR-0023's residual, said out loud. Keeping these is the right answer — the sweep
        // refuses to guess about a run it has never heard of — and the directory may hold the
        // only copy of an agent's uncommitted work, which is the thing this project protects
        // hardest and the thing nothing was pointing at.
        println!(
            "checkouts   {} with no run record here — {}",
            s.stray_checkouts, s.worktrees_dir
        );
        println!("            └─ kept on purpose: nothing here knows what those runs were, and a");
        println!("               checkout can hold uncommitted work. Nothing will reclaim them.");
    }
    if s.records_learned_failed > 0 {
        // Said whenever there is one, because the sentence is about what will *not* happen to
        // it rather than about how many there are: a failed run's record is kept everywhere on
        // purpose (ADR-0021 §2), so a large number here has a cause somebody can act on and no
        // other symptom.
        println!(
            "            └─ {} of those failed. Nothing prunes a failed run's record — it is",
            s.records_learned_failed
        );
        println!("               how any device reads why. A large number is usually a rule");
        println!("               failing every firing: `offload rules` on the node that owns it.");
    }
    for r in &s.resources {
        // The service first, because that is the word a run has to say to be granted it — the
        // id below it is this node's own name and is only worth showing so the owner can find
        // the line in their config.
        println!(
            "resource    {} ({}, {}, {}){}{}",
            r.service,
            r.id,
            r.access,
            r.command,
            // The state and not the cause: this line has a `bool`, and it spent a phase
            // asserting *its program was not found* about a file that was there and not
            // executable. The measured reason arrives inside `description`, which the daemon
            // now takes from the capability (`Capability::unusable`).
            if r.usable { "" } else { " — NOT usable" },
            if r.description.is_empty() {
                String::new()
            } else {
                format!("  ·  {}", r.description)
            }
        );
    }
    for t in &s.tasks {
        // Service first, for the resource line's reason — it is the word `--task` takes, and
        // the block's `id` (which it is *not*) is the commonest confusion about this tier, so
        // both are printed rather than one being guessed at.
        println!(
            "task        {} ({}, {}){}{}",
            t.service,
            t.id,
            t.command,
            // The state, never the cause — see the resource line above.
            if t.usable { "" } else { " — NOT usable" },
            if t.description.is_empty() {
                String::new()
            } else {
                format!("  ·  {}", t.description)
            }
        );
    }
    if s.asks > 0 {
        // A blocked run looks like a working one everywhere else: `ps` says running, the lease
        // is renewed, and the agent is doing nothing whatsoever.
        println!(
            "waiting     {} run(s) stopped for an answer — `offload asks`",
            s.asks
        );
    }
    // The one line that separates *the peer said nothing* from *this machine never spoke*
    // (ADR-0059). Printed only when there is a number, for `asks`'s reason — but the silence
    // it replaces is the expensive kind: a muzzled node reports every peer as unreachable, and
    // three sessions went into a macOS laptop whose only symptom was the fleet looking down.
    if s.sends_refused_total > 0 {
        println!(
            "sends       {} refused by this machine's kernel — that is not the peer's silence.",
            s.sends_refused_total
        );
        for refusal in &s.sends_refused {
            // The kernel's own words, never reworded: `Operation not permitted` on macOS is
            // Local Network access, and the errno is what somebody searches for.
            println!(
                "            └─ {}  ×{}  ·  {}",
                refusal.destination, refusal.refused, refusal.last
            );
        }
        let named: u64 = s.sends_refused.iter().map(|r| r.refused).sum();
        if let Some(rest) = s.sends_refused_total.checked_sub(named).filter(|n| *n > 0) {
            // The breakdown is capped and the total is not, so say which number is short
            // rather than letting the rows read as the whole story.
            println!("            └─ …and {rest} to addresses this list has no room for");
        }
    }
    // The other half of the same silence (ADR-0060): that one is this machine never speaking,
    // this one is this machine speaking and being turned away. Both reach the failure detector
    // as a peer that did not answer — so a node rekeyed out of its fleet called its former
    // fleet `dead` and went on reporting `2 member(s) met` on the line below this one.
    if s.turnaways_total > 0 {
        println!(
            "turned away {} handshake(s) refused by a peer — this node reached it and was not \
             let in.",
            s.turnaways_total
        );
        for turnaway in &s.turnaways {
            // The peer's own words, never reworded: they name both fleets, which is the whole
            // diagnosis, and this node is deliberately not resolving which one is right.
            println!(
                "            └─ {}  ×{}  ·  {}",
                turnaway.peer, turnaway.refused, turnaway.last
            );
        }
        let named: u64 = s.turnaways.iter().map(|t| t.refused).sum();
        if let Some(rest) = s.turnaways_total.checked_sub(named).filter(|n| *n > 0) {
            println!("            └─ …and {rest} from peers this list has no room for");
        }
    }
    if let Some(f) = &s.fleet {
        // The certificate clause is a claim about the future and a revoked one has none — it is
        // unusable now, which `offload fleet` says one command away. Two reports of one device
        // disagreeing is the shape of nearly everything in `docs/pitfalls/reports-and-cli.md`,
        // and here the honest half was already on the `accepting` line four rows up.
        println!(
            "fleet       {}  ·  {} member(s) met, {} approver(s)  ·  {}",
            f.fleet,
            f.members_met,
            f.approvers_met,
            if f.revoked_here {
                "this node has been revoked — its certificate is not usable".to_string()
            } else {
                format!("this node's certificate lasts {} more days", f.cert_days)
            }
        );
        if let Some(days) = f.passphrase_days {
            println!("passphrase  last checked {days} days ago");
        }
        if f.revoked > 0 {
            // "including this one" rather than a separate line: on the device that was revoked,
            // the commonest reading of `revoked 2 device(s)` is *two other* devices, and the one
            // it is really about is the one being read.
            println!(
                "revoked     {} device(s){}",
                f.revoked,
                if f.revoked_here {
                    ", this one among them"
                } else {
                    ""
                }
            );
        }
        for expiring in &f.expiring {
            println!("expiring    {expiring} — it renews itself on contact");
        }
        // The approval's year, which renewal does not move (ADR-0069 §3): the one chore a person
        // has, so it is listed where a person looks, with the command that does it.
        for row in &f.reapproval_due {
            println!(
                "approval    {} runs out in {} day(s) — {}",
                row.name,
                row.days,
                if row.node.to_string() == s.node_id {
                    // No node re-approves itself, so "on an approver" read on the only approver
                    // points at the machine it is being read on.
                    "this node: `offload reapprove` on another approver, or the passphrase"
                } else if row.decided {
                    "re-approval decided here, waiting for it to ask"
                } else {
                    "`offload reapprove` on an approver"
                }
            );
        }
        // ADR-0012 mitigation 6: reported without being asked for, because the states that
        // turn a bad day into a bad week are all silent until the day they are not.
        for note in &f.notes {
            println!();
            println!("note: {note}");
        }
    }
    Ok(())
}

/// Everything `offload run` was given. A struct rather than nine positional arguments, which
/// is what it had grown to and one transposition away from submitting the wrong thing.
pub struct Submission<'a> {
    pub socket: &'a Path,
    pub repo: String,
    pub prompt: String,
    pub model: Option<String>,
    pub permission: Option<PermissionMode>,
    pub git_ref: Option<String>,
    pub allow: Vec<String>,
    pub follow: bool,
    pub queue: bool,
    /// Absolute unix milliseconds, already resolved from whatever the operator typed.
    pub deadline: Option<u64>,
    pub demand: offload_core::Demand,
    pub notify: offload_core::Audience,
    /// Which kinds of news interrupt somebody (ADR-0026). `offload run` defaults to everything;
    /// `offload when` defaults to problems.
    pub notices: offload_core::Notices,
    pub ask: offload_core::AskPolicy,
    pub resources: Vec<offload_core::Service>,
    /// The most turns this run may take. `None` is no limit.
    pub max_turns: Option<std::num::NonZeroU32>,
    pub placing: Placing,
}

/// Where a run should go (ADR-0063): sent unresolved, because the daemon knows the fleet's ids
/// and this process does not.
#[derive(Debug, Clone, Default)]
pub struct Placing {
    pub require: offload_core::Wanted,
    pub prefer: offload_core::Wanted,
    /// Absolute unix milliseconds, resolved here like `deadline`.
    pub hold_until: Option<u64>,
}

/// `--require` and `--prefer`, in `offload match`'s grammar.
pub fn parse_wanted(input: &str) -> Result<offload_core::Wanted, String> {
    crate::constraint_expr::parse(input).map_err(|e| e.to_string())
}

/// What `offload run --task` sends: a service, and the shared half of any submission.
///
/// Deliberately not a `Submission` with the agent fields left blank. The clap definition
/// already refuses `--task` beside `--model`, `--allow`, `--ask` and the rest, and this is the
/// same refusal expressed in the type: there is nowhere here to put a prompt, so no code path
/// downstream has to decide what an empty one would have meant.
pub struct TaskSubmission<'a> {
    pub socket: &'a Path,
    pub service: offload_core::Service,
    pub args: Vec<String>,
    pub follow: bool,
    pub queue: bool,
    pub deadline: Option<u64>,
    pub demand: offload_core::Demand,
    pub notify: offload_core::Audience,
    pub notices: offload_core::Notices,
    pub placing: Placing,
}

/// `offload run --task <service>` — the cheap tier (ADR-0019).
pub async fn run_task(submission: TaskSubmission<'_>) -> Result<()> {
    let TaskSubmission {
        socket,
        service,
        args,
        follow,
        queue,
        deadline,
        demand,
        notify,
        notices,
        placing,
    } = submission;

    let responses = client::request(
        socket,
        &Request::SubmitTask(offload_node::api::SubmitTaskRequest {
            service,
            args,
            queue,
            deadline,
            demand,
            notify,
            notices,
            require: placing.require,
            prefer: placing.prefer,
            hold_until: placing.hold_until,
            // Typed at a keyboard, which is what decides whether anything may later throw the
            // output away (ADR-0024). A rule's is stamped by the daemon at the firing.
            origin: offload_core::Origin::Operator,
        }),
    )
    .await?;

    let Response::Submitted {
        run,
        node,
        waiting,
        queued,
        audience,
        preference,
        ..
    } = client::expect_one(responses)?
    else {
        bail!("unexpected response to submit");
    };

    let where_it_went = match &node {
        Some(name) => format!(" — accepted by {name}"),
        None => String::new(),
    };
    let when = match &waiting {
        Some(when) => format!(", {when}"),
        None => String::new(),
    };
    if let Some(reasons) = &queued {
        // Worded as the agent path words it. This printed `task queued — ` and the canvass, with
        // no run id, so there was nothing to type into `offload cancel` or `explain` without
        // going through `ps` first (session ninety-four).
        println!(
            "task {} — queued; nobody will take it yet:{reasons}",
            short_id(&run)
        );
        println!(
            "  it is offered to the fleet again as things change; `offload explain {}`",
            short_id(&run)
        );
        if let Some(audience) = &audience {
            println!("  {audience}");
        }
        return Ok(());
    }
    println!("run {}{where_it_went}{when}", short_id(&run));
    if let Some(preference) = &preference {
        println!("  {preference}");
    }
    if let Some(due) = deadline {
        println!("  due in {}", crate::when::until(due));
    }
    if let Some(audience) = &audience {
        println!("  {audience}");
    }
    if follow {
        logs(socket, &run, true).await?;
    } else {
        println!("follow with: offload logs -f {}", short_id(&run));
    }
    Ok(())
}

/// Put an archive workspace into the fleet, and answer with the repo string naming it.
///
/// ADR-0061 §1. The daemon reads the file and hashes it — the digest that comes back is
/// computed there rather than claimed here, which is what makes it an authority (ADR-0016).
///
/// The path is resolved to an absolute one first. The daemon reads it, and the daemon's working
/// directory is not the operator's: a relative path that works for the shell would be read
/// against `/` or wherever `offloadd` was started, and the resulting error names a file nobody
/// asked for.
pub async fn store_archive(socket: &Path, path: &Path) -> Result<String> {
    let absolute =
        std::fs::canonicalize(path).with_context(|| format!("no archive at {}", path.display()))?;
    let responses = client::request(
        socket,
        &Request::StoreArchive {
            path: absolute.display().to_string(),
        },
    )
    .await?;
    for response in responses {
        match response {
            Response::ArchiveStored { hash, bytes } => {
                println!(
                    "archive {}  ({})",
                    &hash[..hash.len().min(12)],
                    human_bytes(bytes)
                );
                println!("  the node that takes this run unpacks it and makes it a repository;");
                println!("  results come back as commits on the run's branch, not as files here.");
                return Ok(format!("{}{hash}", offload_core::repo::ARCHIVE_PREFIX));
            }
            Response::Error { message } => anyhow::bail!("{message}"),
            _ => {}
        }
    }
    anyhow::bail!("the daemon did not say what it did with the archive")
}

/// Bytes, for somebody deciding whether their selection is going to fit.
fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

pub async fn run(submission: Submission<'_>) -> Result<()> {
    let Submission {
        socket,
        repo,
        prompt,
        model,
        permission,
        git_ref,
        allow,
        follow,
        queue,
        deadline,
        demand,
        notify,
        notices,
        ask,
        resources,
        max_turns,
        placing,
    } = submission;
    let placing_hold = placing.hold_until;
    // Validate here so a typo is reported before a worktree is built, and so the risky
    // ones are called out while the person who typed them is still looking.
    let parsed = offload_core::ToolAllowlist::parse(&allow).map_err(|e| anyhow::anyhow!("{e}"))?;
    for (pattern, risk) in parsed.risky() {
        eprintln!(
            "warning: --allow {pattern} grants {}, not just that command",
            match risk {
                offload_core::Risk::Unrestricted => "unrestricted shell access",
                offload_core::Risk::GeneralExecution => "general execution",
                offload_core::Risk::Scoped => unreachable!(),
            }
        );
    }

    // Resolve the repo now so the daemon gets a path that means the same thing to it.
    // A relative path is interpreted from wherever the CLI was invoked, which is not
    // where the daemon lives.
    // Not a scratch workspace's spelling, which names no directory (ADR-0072).
    let repo = if repo == offload_core::SCRATCH {
        repo
    } else {
        std::fs::canonicalize(&repo).map_or(repo, |p| p.display().to_string())
    };

    let responses = client::request(
        socket,
        &Request::Submit(SubmitRequest {
            repo,
            prompt,
            model,
            permission,
            git_ref,
            allow,
            queue,
            deadline,
            demand,
            notify,
            notices,
            ask,
            resources,
            max_turns,
            require: placing.require,
            prefer: placing.prefer,
            hold_until: placing.hold_until,
            // A submission typed at a keyboard is an operator's, which is what decides whether
            // anything may later throw its output away (ADR-0024). A rule's runs are stamped by
            // the daemon at the firing, never here.
            origin: offload_core::Origin::Operator,
        }),
    )
    .await?;

    let Response::Submitted {
        run,
        node,
        waiting,
        here_only,
        queued,
        audience,
        ask,
        preference,
        ..
    } = client::expect_one(responses)?
    else {
        bail!("unexpected response to submit");
    };

    // Accepted by a node, or refused to your face — there is no third answer, and the node
    // that took it is the useful half of the first one (ADR-0014). A node may accept work it
    // has no room for yet and hold it (ADR-0006), so acceptance is reported with *when*:
    // "accepted" alone would read as "running" to somebody about to close their laptop.
    let where_it_went = match &node {
        Some(name) => format!(" — accepted by {name}"),
        None => String::new(),
    };
    let when = match &waiting {
        Some(when) => format!(", {when}"),
        None => String::new(),
    };
    if let Some(reasons) = &queued {
        // Not a success and not a refusal: the third outcome, and only because it was asked
        // for. The reasons come with it — "queued" alone is what ADR-0014 exists to prevent,
        // and they are what somebody checks back against in the morning.
        println!(
            "run {} — queued; nobody will take it yet:{reasons}",
            short_id(&run)
        );
        println!(
            "  it is offered to the fleet again as things change; `offload explain {}`",
            short_id(&run)
        );
        if let Some(hold) = placing_hold {
            println!(
                "  held for its preferred node for {}; after that, anywhere",
                crate::when::until(hold)
            );
        }
        // Said here as well: a run that waits until morning is precisely the one whose owner
        // will not be watching when it finally finishes.
        if let Some(audience) = &audience {
            println!("  {audience}");
        }
        if let Some(ask) = &ask {
            println!("  {ask}");
        }
        return Ok(());
    }
    println!("run {}{where_it_went}{when}", short_id(&run));
    if let Some(preference) = &preference {
        // Directly under the line saying where it went, because it is the reason for that line
        // when it was not where the operator asked (ADR-0063 §7).
        println!("  {preference}");
    }
    if let Some(hold) = placing_hold {
        println!(
            "  held for its preferred node for {}; after that, anywhere",
            crate::when::until(hold)
        );
    }
    if let Some(due) = deadline {
        // Said back, because a deadline is the one thing here that changes behaviour with no
        // event to point at later, and because a mistyped duration is silent otherwise.
        println!("  due in {}", crate::when::until(due));
    }
    if let Some(audience) = &audience {
        // Only when it is not the default: where the news goes, or the reason it will go
        // nowhere. Said now because a run whose notification had no route is discovered at
        // breakfast otherwise, which is ADR-0014's argument on the delivery plane.
        println!("  {audience}");
    }
    if let Some(ask) = &ask {
        // The same argument one step further in: this one is about what the run will *do* when
        // it is blocked, so a fleet that can reach nobody has quietly turned the flag off.
        println!("  {ask}");
    }
    if here_only {
        // The overnight case, at the moment it is fragile: this machine took the run and
        // nobody else has a copy of it, so closing the lid now loses work that has not
        // started. Said here rather than left to `ps`, because the person is still reading.
        println!("  no other node has a copy — if this machine goes away, so does the run");
    }
    if follow {
        // Wherever it landed. A run placed on a peer is followed by asking that peer, once a
        // second, through the node in front of you — which is the whole point of the promise on
        // the front of the README: close the laptop, the agent keeps working, and you can still
        // see it working.
        logs(socket, &run, true).await?;
    } else {
        println!("follow with: offload logs -f {}", short_id(&run));
    }
    Ok(())
}

/// `offload drain` — hand this node's work to the fleet before it goes away.
///
/// **Streamed, because this is the one command that can take minutes** (ADR-0035). It waits for
/// every agent to reach a turn boundary, and a run blocked mid-tool-call on `--ask` reaches none
/// until somebody answers or its patience runs out — both five minutes by default. Buffering
/// until the pass returned meant the whole report arrived at the end: measured, forty seconds of
/// a blank terminal, while `offload asks` in the next window named the question and `offload
/// status` said `waiting 1 run(s) stopped for an answer`. The person who could release it
/// instantly is the person standing here.
pub async fn drain(socket: &Path) -> Result<()> {
    let mut outcome = None;
    client::stream(socket, &Request::Drain, |response| {
        match response {
            Response::Draining(step) => say_drain_step(&step),
            Response::Drained {
                moved,
                left,
                finished,
                later,
                pooled,
                handed_back,
                no_fleet,
                no_boundary,
            } => {
                outcome = Some((
                    moved,
                    left,
                    finished,
                    later,
                    pooled,
                    handed_back,
                    no_fleet,
                    no_boundary,
                ));
            }
            Response::Error { message } => bail!("{message}"),
            _ => {}
        }
        Ok(())
    })
    .await?;
    let Some((moved, left, finished, later, pooled, handed_back, no_fleet, no_boundary)) = outcome
    else {
        bail!("the daemon never said how the drain went");
    };

    // What `left` still means, which is narrower than it was. "Refused by every other node" used
    // to end up here and does not any more: a refusal is not an answer about later, so a released
    // run goes to `pooled` and stays the fleet's (ADR-0042). What is left is a run this pass
    // could not get to a turn boundary before the process stops — the shutdown path's mid-turn
    // runs, and any whose record went away underneath it. The lease is what covers those, which
    // is the sentence to print rather than advice to go and look at bids.
    //
    // Pointing somebody at `offload nodes` about a machine whose fleet is itself is the wrong
    // half of the sentence, and it was the half printed — hence the `no_fleet` arm.
    //
    // A slice of lines rather than one literal with a newline in it: the first version of this
    // was written as a continued string and came out with ten spaces baked into the middle of
    // `every other node          —`, which is the failure `offload-node/tests/messages.rs` exists
    // to catch and duly did.
    let why_still_here: &[&str] = if no_fleet {
        &["  there is no fleet to hand it to, so it stays here with its checkpoint."]
    } else {
        &[
            "  a run still here never reached a turn boundary in time — its lease is",
            "  what moves it now; `offload ps` says where each one got to.",
        ]
    };

    match (moved, left) {
        (0, 0)
            if finished == 0
                && later == 0
                && pooled == 0
                && handed_back == 0
                && no_boundary == 0 =>
        {
            println!("  nothing to hand over");
        }
        // `handed_back` earns its place in this guard rather than only in a line below: a node
        // whose only run had failed holds nothing, so every number above is nought and the drain
        // said "nothing to hand over" about a pass that had just given a run to the fleet.
        (0, 0) if finished == 0 && later == 0 && pooled == 0 && no_boundary == 0 => {}
        // Its own line and never `why_still_here`: a run that ended by itself is the *good*
        // outcome, and it used to be counted in `left` and described as one that could not be
        // handed over — which is the sentence that keeps a lid open for nothing.
        (0, 0) if finished > 0 => {
            println!("  nothing to hand over — {finished} run(s) finished while it waited");
        }
        // Everything this pass could not move is still mid-turn, so nothing has gone wrong and
        // there is nothing to look into: the line below says what happens next, and the advice
        // attached to `left` would be pointing at the wrong question.
        (0, 0) => {}
        (moved, 0) => println!("  handed {moved} run(s) to other nodes"),
        // Its own sentence, because "handed 0 run(s) over" is the least useful way to say the
        // most alarming outcome: this node stopped work and the fleet would not take it.
        (0, left) => {
            println!("  nothing could be handed over; {left} run(s) still here");
            for line in why_still_here {
                println!("{line}");
            }
        }
        (moved, left) => {
            println!("  handed {moved} run(s) over; {left} still here");
            // Not a failure: a run mid-turn at the deadline keeps its turn rather than being
            // snapshotted anyway, and one nobody would take is a fleet with nowhere to put it.
            for line in why_still_here {
                println!("{line}");
            }
        }
    }
    if finished > 0 && (moved > 0 || left > 0 || later > 0 || pooled > 0) {
        println!("  {finished} run(s) finished while it waited");
    }
    // Last, because it is the only line about the future: everything above happened, and this is
    // what this node will do without being asked again. Its own sentence rather than a share of
    // `left` — a run mid-turn at the deadline is not one nobody would take, and it was being
    // reported with the words for one, under advice (`offload ps`, `offload nodes`) that cannot
    // answer what it is actually waiting for. Measured: the run went `Pending` twenty seconds
    // after the drain said it was still here, and stayed there.
    if later > 0 {
        // One reason again, and the sentence is right about it: this run's agent is still going
        // here, and the boundary it will be handed over at is one only this process reaches.
        // The other half it used to cover moved to `pooled` below, because the two differ in
        // the thing somebody about to close a laptop is actually asking.
        println!("  {later} run(s) still mid-turn — this node hands each over at its next");
        println!("  turn boundary, without being drained again.");
    }
    // Beside `later` and never inside it, which is where a running task used to be counted: that
    // sentence promises a handover *at the run's next turn boundary*, and a nominated program has
    // none — so the promise could not be kept and the wait before it could not end. The drain
    // waited `drain_deadline_secs` for that boundary, 300 seconds by default, and then said this
    // about a `/bin/sleep`. Nothing is owed for one of these: the node stays up and the program
    // ends by itself, or the process goes and ADR-0043 re-runs it from its spec somewhere else.
    if no_boundary > 0 {
        println!("  {no_boundary} task(s) still running here — a task has no turn boundary, so");
        println!("  nothing waits for one. Leave the daemon up and it finishes; stop it and");
        println!("  the fleet runs it again from its spec.");
    }
    // Last, and the only line that is about somebody else: these runs are in the pool with the
    // record saying a node let them go (ADR-0042), so their arbiter keeps offering them on the
    // backoff. Worth its own sentence because it is the one outcome here that no longer needs
    // this daemon — and because it used to be reported as `left`, under advice to go and look
    // at a run nothing was ever going to move.
    if pooled > 0 {
        println!("  {pooled} run(s) released to the fleet — nobody had room just now, so");
        println!("  whichever node arbitrates each one keeps offering it. That does not");
        println!("  need this daemon: it is on the run's record, not in this process.");
    }
    // A different list from every line above, which is why it is a line of its own: these runs
    // had already *failed* here, so the handover pass never saw them and no number above counts
    // one. Before ADR-0043 they stayed failed on a machine that was leaving, in front of a fleet
    // that would have carried on with them.
    if handed_back > 0 {
        println!("  {handed_back} failed run(s) given back to the fleet to carry on with —");
        println!("  this node would have picked each one up itself, and is leaving instead.");
        println!("  `offload ps` says where they get to; the ones that stayed failed say why.");
    }
    Ok(())
}

/// One line — or four — of a drain in progress.
///
/// The blocked-question step is the reason this streams at all, so it says the three things that
/// make it actionable: what the agent wants to do, that no turn boundary is coming until it is
/// decided, and the command that decides it.
/// What to tell somebody waiting behind a question during a drain — **whichever clock runs out
/// first, named.**
///
/// A function rather than three `println!`s inside a `match` arm so it can be tested: this is the
/// only line in the product that has to pick between two deadlines, and picking wrong is silent.
///
/// The two are not the same event. The question's patience expiring *decides* it (ADR-0017), the
/// agent then takes its turn, reaches a boundary, and the run is handed over normally. The drain's
/// deadline expiring first decides nothing — the run is left mid-turn with the question still open
/// and still answerable. So the smaller is the one an operator is racing, and reporting only the
/// question's told somebody ten seconds from losing the run that they had four minutes
/// (ADR-0035's residual).
///
/// `Millis(0)` for the drain is an older daemon that does not send the field. The fallback is the
/// sentence this had before: half a horizon beats none.
fn blocked_horizon(
    within: offload_core::Millis,
    drain_ends_in: offload_core::Millis,
) -> Vec<String> {
    let only_the_question = vec![format!(
        "it reaches no turn boundary until that is decided, or {within} from now"
    )];
    if drain_ends_in.0 == 0 || drain_ends_in >= within {
        return only_the_question;
    }
    vec![
        format!(
            "it reaches no turn boundary until that is decided — and this drain stops waiting \
             in {drain_ends_in}, which is sooner"
        ),
        format!(
            "unanswered by then, the run is left mid-turn here; the question itself stands \
             for {within}"
        ),
    ]
}

fn say_drain_step(step: &offload_node::api::DrainStep) {
    use offload_node::api::DrainStep;
    match step {
        // Said the moment it becomes true, which is the whole point of streaming this one: it is
        // the only thing a drain does on a fleet of one, and it used to be printed at the end.
        DrainStep::StoppedAccepting => {
            println!("this node has stopped accepting work — restart offloadd to take work again");
        }
        // The one line here that is about work *surviving* rather than about work moving, so it
        // says the number that decides whether the lid can be closed — and says nothing at all
        // when nothing was at risk, which is the ordinary drain.
        DrainStep::PushedLastCopies { pushed, stranded } => {
            if *pushed > 0 {
                println!("  copied {pushed} checkpoint(s) nobody else had to a peer");
            }
            if *stranded > 0 {
                println!(
                    "  {stranded} checkpoint(s) are still on this node only — that work does not \
                     survive this machine going away"
                );
                println!("    offload ps --all says which, under SAFE");
            }
        }
        // Nothing at all when there is nothing to wait for. The step is said before the first
        // wait rather than per run, so it used to announce *"waiting for 0 run(s) … up to 20.0s"*
        // on a node holding only tasks — a wait that is not happening, stated with a duration
        // nobody will spend. Reachable the moment work with no turn boundary stopped being
        // waited for.
        DrainStep::Waiting { runs, up_to_ms } if *runs > 0 => {
            let up_to = offload_core::Millis(*up_to_ms);
            println!("  waiting for {runs} run(s) to reach a turn boundary — up to {up_to}");
        }
        DrainStep::Waiting { .. } => {}
        DrainStep::Blocked {
            run,
            tool,
            detail,
            tool_use_id,
            within_ms,
            drain_ends_in_ms,
        } => {
            let within = offload_core::Millis(*within_ms);
            println!("  run {run} is stopped waiting for an answer: {tool} — {detail}");
            for line in blocked_horizon(within, offload_core::Millis(*drain_ends_in_ms)) {
                println!("    {line}");
            }
            println!("    answer it: offload approve {run} {tool_use_id}   (or `offload deny`)");
        }
    }
}

/// `offload nodes` — who else is out there, and how they are doing.
/// `offload sinks [--test]` — the routes from this node to a person, and their state.
///
/// Two things it makes a point of showing. A **broken** route is listed with the reason rather
/// than omitted, because that is exactly what somebody is looking for when they ask why nothing
/// arrived. And a route with nothing delivered *and* nothing waiting is called out as untried:
/// a sink that looks fine and has never carried anything is the state most likely to be a lie,
/// which is what `--test` is for.
/// What a route on **this** node is doing, from its three counters.
///
/// A function rather than a `match` inside the `println!` loop, for session eighty-one's reason:
/// wording that lives inside a print statement is wording no test can fail on. This one had a
/// defect that a test would have caught the day it was written.
///
/// **All three counters, not two.** It matched `(unusable, delivered, waiting)` and left
/// `gave_up` to a guard further down, so a route that had delivered nothing, had nothing queued
/// and had *given up on a message* came out `usable, never used — try offload sinks --test`,
/// with `DROPPED 1` in the column beside it and its own `last failure: gave up: …` line printed
/// directly underneath. The arm that says what that means was written for exactly this case —
/// its comment reads *"the route looks healthy and somebody was not told something"* — and was
/// shadowed by the one above it, in the case where the route looks least healthy of all.
/// Measured on a daemon with a sink whose script exits 7.
fn sink_state(sink: &offload_node::deliver::SinkReport, test: bool) -> String {
    match (&sink.unusable, sink.delivered, sink.waiting, sink.gave_up) {
        (Some(reason), ..) => format!("unusable: {reason}"),
        // A test is deliberately not a delivery — it is not written to the outbox — so the
        // counters below are still zero, and saying "never used" under a line that just said a
        // test went through would read as a contradiction.
        (None, ..) if test => "test delivered".to_string(),
        (None, 0, 0, 0) => "usable, never used — try `offload sinks --test`".to_string(),
        (None, _, waiting, _) if waiting > 0 => format!("{waiting} still to send"),
        // Usable *now* and something was dropped earlier: worth saying out loud, because the
        // route looks healthy and somebody was not told something.
        (None, _, _, gave_up) if gave_up > 0 => format!("ok now — {}", given_up(gave_up)),
        (None, ..) => "ok".to_string(),
    }
}

/// "1 was given up on earlier", "3 were" — the two sink tables say this and both said `were`.
///
/// A helper rather than the phrase twice, because the two tables are twenty lines apart in one
/// function and they had already drifted in a way that mattered: the local one asked *has it
/// delivered anything* before *has it given up on anything*, and the fleet one asked them the
/// other way round. Only one of those orders is right.
fn given_up(count: u64) -> String {
    match count {
        1 => "1 was given up on earlier".to_string(),
        n => format!("{n} were given up on earlier"),
    }
}

pub async fn sinks(socket: &Path, test: bool) -> Result<()> {
    let responses = client::request(socket, &Request::Sinks { test }).await?;
    let Response::Sinks { sinks, fleet } = client::expect_one(responses)? else {
        bail!("unexpected response to sinks");
    };

    if sinks.is_empty() && fleet.is_empty() {
        println!("No delivery routes configured on this node.");
        println!("  Nothing here will tell you when a run finishes, fails, or misses its");
        println!("  deadline — which is fine if you are watching, and the whole problem if");
        println!("  the run is on a machine you are not sitting at.");
        println!();
        println!("  Add one to the node config:");
        println!();
        println!("      [[sinks]]");
        println!("      id = \"phone\"");
        println!("      service = \"push\"");
        println!("      command = \"/home/you/bin/offload-notify\"");
        println!();
        println!("  The command is given the notification as JSON on stdin, as OFFLOAD_*");
        println!("  variables, and as one final argument holding the summary line.");
        return Ok(());
    }

    // The header belongs to the rows, so a node with no routes of its own says so once and does
    // not then print a column heading with nothing under it — which reads as a row that failed to
    // render rather than as a node that has none.
    if sinks.is_empty() {
        println!("No delivery routes on this node — it uses the fleet's, below.");
    } else {
        println!(
            "{:<12} {:<10} {:>9} {:>8} {:>8}  STATE",
            "SINK", "SERVICE", "DELIVERED", "WAITING", "DROPPED"
        );
    }
    for sink in &sinks {
        let state = sink_state(sink, test);
        println!(
            "{:<12} {:<10} {:>9} {:>8} {:>8}  {}",
            truncate(&sink.id, 12),
            truncate(&sink.service, 10),
            sink.delivered,
            sink.waiting,
            sink.gave_up,
            state
        );
        if !sink.description.is_empty() {
            println!("             └─ {}", sink.description);
        }
        // This node's own routes, so the command belongs here — it is the owner reading about
        // their own machine. The fleet listing below shows no such line, deliberately: a peer
        // learns that a device has a route and never how it works.
        if !sink.command.is_empty() {
            println!("             └─ runs {}", sink.command);
        }
        // The last failure, even on a route that is working now: a phone that missed one
        // notification last night is a fact worth seeing, and nothing else records it.
        if let Some(error) = &sink.last_error {
            println!("             └─ last failure: {error}");
        }
    }
    if test {
        println!();
        // **Every** route, not each usable one: the handler loops over all of them and lets the
        // attempt decide, which is the whole point — `usable` before a test is only "the program
        // resolves", and this is what tells a route that resolves and then refuses from one that
        // works. The old sentence claimed a filter that does not happen and used `usable` for a
        // different question from the one the rows above answer with the same word.
        println!("Every route on this node was tried. `test delivered` is one that answered;");
        println!("anything else names what went wrong when the message was actually sent.");
    }

    // The fleet's routes, which this node's own runs can use. Unauthenticated ones included:
    // "there is a phone in this fleet and nothing reaches it" is the question being asked, and
    // hiding the row would answer "there is no phone".
    if !fleet.is_empty() {
        println!();
        println!(
            "{:<20} {:<10} {:>9} {:>8} {:>8}  STATE",
            "FLEET ROUTE", "SERVICE", "DELIVERED", "WAITING", "DROPPED"
        );
        for route in &fleet {
            let state = if !route.reachable && route.waiting > 0 {
                format!(
                    "{} is not answering — {} waiting for it to come back",
                    route.node, route.waiting
                )
            } else if !route.reachable {
                format!("{} is not answering", route.node)
            } else if !route.authenticated {
                // What the bit says, and no more. `Capability::authenticated` is one bit with a
                // different meaning per role, and this asserted a *credential* about a `[[sinks]]`
                // entry whose program was simply not on that device — while the owning node's own
                // table, one command away, said `not found on this device`. The measured reason
                // rides along in `description` and is printed under the row.
                format!("{} says it cannot use it", route.node)
            } else if route.waiting > 0 {
                format!("{} still to send", route.waiting)
            } else if route.gave_up > 0 {
                format!("ok now — {}", given_up(route.gave_up))
            } else if route.delivered == 0 {
                "usable; nothing from this node yet".to_string()
            } else {
                "ok".to_string()
            };
            println!(
                "{:<20} {:<10} {:>9} {:>8} {:>8}  {}",
                truncate(&format!("{}/{}", route.node, route.id), 20),
                truncate(&route.service, 10),
                route.delivered,
                route.waiting,
                route.gave_up,
                state
            );
            // The owner's label and, where that device could not use the route, the reason it
            // gave — both of which gossip, and the second is what turns `alpha says it cannot
            // use it` into something to act on. This line did not exist, so the honest reason
            // was travelling and being thrown away while the STATE column beside it guessed.
            // Still no `runs …` line: a peer learns that a device has a route and never how it
            // works (ADR-0010), which is about the *command* and not about this.
            if !route.description.is_empty() {
                println!("                     └─ {}", route.description);
            }
            if let Some(error) = &route.last_error {
                println!("                     └─ last failure: {error}");
            }
        }
    }
    Ok(())
}

/// `offload nodes --history`: what this node has witnessed happening to its fleet.
///
/// ADR-0012 mitigation 4's durable half. The notification is what wakes somebody; this is what
/// they read afterwards, and it is the half that survives a phone that was flat.
pub async fn fleet_history(socket: &Path) -> Result<()> {
    // `AUDIT_PAGE + 1` for `offload audit`'s reason, and it matters more here: this is the log
    // somebody opens to find out *whether* a device was ever enrolled without their knowledge
    // (ADR-0012 mitigation 4), so a listing that silently stops is answering the question with
    // the wrong half of its evidence.
    let responses = client::request(
        socket,
        &Request::FleetHistory {
            limit: AUDIT_PAGE + 1,
        },
    )
    .await?;
    let Response::FleetHistory { entries } = client::expect_one(responses)? else {
        bail!("unexpected response to fleet history");
    };
    if entries.is_empty() {
        println!("Nothing recorded. This node has not seen a device join or the passphrase used.");
        return Ok(());
    }
    let more = entries.len() > AUDIT_PAGE as usize;
    for entry in entries.iter().take(AUDIT_PAGE as usize) {
        println!(
            "{}  {}  {}",
            when(entry.at_unix_ms),
            &entry.node[..12.min(entry.node.len())],
            entry.summary
        );
    }
    println!();
    if more {
        println!("The {AUDIT_PAGE} most recent events. This node witnessed older ones too.");
    }
    println!("What this device witnessed, not the fleet's memory — ask another one too.");
    Ok(())
}

/// `offload audit [<run>]` — what this node decided about runs, what it was refused, and what
/// was taken away from it.
///
/// Newest first, because the question is nearly always about what just happened. The closing line
/// is not decoration: this log is per-node by design, so a fleet's answer is the union of every
/// device's, and somebody reading one machine's rows should know that is what they have.
pub async fn audit(socket: &Path, run: Option<&str>) -> Result<()> {
    // One more than is shown, so that "there are older rows" is a **fact** rather than a hedge.
    // Asking for exactly the number displayed leaves the two indistinguishable cases — a node
    // that did exactly this much, and a node whose older rows were cut off — looking identical,
    // and this log's whole job is to be read after the fact about a moment somebody has in mind.
    // Measured before this existed: sixty submissions, the output stopped at exactly 100 rows,
    // and the only closing line was about the *other* nodes.
    let responses = client::request(
        socket,
        &Request::Audit {
            run: run.map(ToString::to_string),
            limit: AUDIT_PAGE + 1,
        },
    )
    .await?;
    let Response::Audit { entries, names } = client::expect_one(responses)? else {
        bail!("unexpected response to audit");
    };
    if entries.is_empty() {
        match run {
            Some(run) => println!("Nothing recorded here about {}.", short_id(run)),
            None => println!(
                "Nothing recorded. This node has not granted, accepted, lost or been refused a \
                 run."
            ),
        }
        return Ok(());
    }
    // The width is a fact about *these* rows, exactly as it is in `offload ps` — and this is the
    // listing where getting it wrong is worst. Two scheduled occurrences sharing a tick boundary
    // display the same twelve characters, so their rows interleave into one plausible and
    // alarming story: `granted … accepted … granted … accepted`, all at epoch 1, under one id.
    // That is the signature of an arbiter spending a token twice, which is the single thing this
    // log exists to let somebody detect. Measured on one daemon with `every 1m` beside
    // `every 2m` — four rows, two runs, and `offload audit <that id>` then refused as ambiguous.
    let more = entries.len() > AUDIT_PAGE as usize;
    let entries = &entries[..entries.len().min(AUDIT_PAGE as usize)];
    let ids: Vec<String> = entries.iter().map(|e| e.run().to_string()).collect();
    let width = id_width(ids.iter().map(String::as_str));
    for (entry, id) in entries.iter().zip(&ids) {
        println!(
            "{}  {}  {}",
            when(entry.at_unix_ms),
            at_most(id, width),
            entry.kind.describe_naming(|node| names
                .iter()
                .find(|(id, _)| *id == node)
                .map_or_else(|| node.short(), |(_, name)| name.clone()))
        );
    }
    println!();
    // Two different incompletenesses, and only the second one was ever said. "Ask the others" is
    // about the *fleet*; this one is about rows on this very machine that the listing did not
    // show, which is the one a reader looking for a moment in the past will be caught by.
    if more {
        println!(
            "The {AUDIT_PAGE} most recent rows. This node has older ones — name a run to see \
             its own: `offload audit <run>`."
        );
    }
    println!("What this device did, not the fleet's — ask the others too.");
    Ok(())
}

/// How many rows `offload audit` and `offload nodes --history` show.
///
/// Named because it appears three times in each command — the request, the truncation, and the
/// sentence that tells somebody the listing was truncated — and a number that has to agree with
/// itself in three places is one to write down once.
const AUDIT_PAGE: u32 = 100;

/// A wall-clock timestamp for a person reading a list, rather than a unix millisecond — **in
/// UTC, and it says so**.
///
/// It used to say `local` here and render UTC, which is a doc comment asserting the one thing the
/// code did not do and is why nobody looked. Measured on this machine: a run submitted at 15:20:19
/// CEST appeared in `offload audit` as `2026-09-12 13:20`, bare — two hours out, with nothing on
/// the row to say which frame it was in. Both callers are logs somebody opens *afterwards* to
/// place an event in time: `offload audit` beside the daemon's own `tracing` output, and
/// `offload nodes --history` to see when a device was enrolled. An unmarked wall-clock time is
/// read as the reader's own, so this was silently answering "nothing happened then" to anybody
/// asking about the right two hours.
///
/// **Converting to local was considered and declined**, which is the same call the old comment
/// here made without saying so. There is no timezone in `std`, so it means a dependency (`time`,
/// `chrono`, or `libc` for `localtime_r`) in a crate that has none of the three, or parsing TZif
/// by hand — and both then have a new way to be wrong (no tzdata, `TZ` unset in a container, DST
/// boundaries) where labelling cannot be. The marker also makes the column directly comparable
/// with `offloadd`'s own log lines, which are already `Z`-suffixed UTC and are the other thing
/// open on the screen. If somebody does want local time later, the dependency is the decision and
/// this function is the only place it lands.
fn when(at_unix_ms: u64) -> String {
    let secs = at_unix_ms / 1_000;
    let days = secs / 86_400;
    let time = secs % 86_400;
    // Civil-from-days, the usual algorithm. No chrono in this crate, and a date is not worth
    // a dependency that would then be in the daemon too.
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        time / 3_600,
        (time % 3_600) / 60
    )
}

/// The fleet's models, each with the devices that offer it (ADR-0080), or a request for every
/// device to read its list again.
pub async fn models(socket: &Path, refresh: bool) -> Result<()> {
    if refresh {
        let responses = client::request(socket, &Request::RefreshModels).await?;
        let Response::ModelsAsked { fleet } = client::expect_one(responses)? else {
            bail!("unexpected response to models --refresh");
        };
        if fleet {
            println!(
                "Reading this device's model list again, and asking the fleet to do the same."
            );
            println!(
                "Each device answers within a few seconds of hearing it; `offload models` shows"
            );
            println!("what they said. A device that is off reads its list when it next starts.");
        } else {
            println!("Reading this device's model list again. It is not in a mesh, so there is");
            println!("nobody else to ask.");
        }
        return Ok(());
    }

    let responses = client::request(socket, &Request::Nodes).await?;
    let Response::Nodes { nodes, local } = client::expect_one(responses)? else {
        bail!("unexpected response to models");
    };
    if local.is_none() {
        println!("This node is not in a mesh, so there is no fleet to list models for.");
        return Ok(());
    }
    print!("{}", render_models(&nodes));
    Ok(())
}

/// Every model offered anywhere, in the order the first device to offer it lists it, with who
/// offers it; then every device with an agent that offers none, which is not the same as having
/// no agent.
fn render_models(nodes: &[offload_node::mesh::NodeSummary]) -> String {
    use std::fmt::Write as _;
    let mut offered: Vec<(&offload_core::Model, Vec<&str>)> = Vec::new();
    for node in nodes {
        for model in &node.models {
            match offered.iter_mut().find(|(m, _)| m.value == model.value) {
                Some((_, by)) => by.push(&node.name),
                None => offered.push((model, vec![&node.name])),
            }
        }
    }
    let mut out = String::new();
    if offered.is_empty() {
        out.push_str("No device in the fleet lists any models.\n");
    } else {
        let width = offered
            .iter()
            .map(|(m, _)| m.value.len())
            .max()
            .unwrap_or(0)
            .max("MODEL".len());
        let _ = writeln!(out, "{:<width$}  {:<24}  OFFERED BY", "MODEL", "NAME");
        for (model, by) in &offered {
            let _ = writeln!(
                out,
                "{:<width$}  {:<24}  {}",
                model.value,
                truncate(&model.name, 24),
                by.join(", ")
            );
        }
    }
    let silent: Vec<&str> = nodes
        .iter()
        .filter(|n| n.agent.is_some() && n.models.is_empty())
        .map(|n| n.name.as_str())
        .collect();
    if !silent.is_empty() {
        let _ = writeln!(
            out,
            "\n{} {} an agent that lists no models: logged out, or its list could not be read.\n\
             Its own log says which; `offload models --refresh` asks again.",
            silent.join(", "),
            if silent.len() == 1 { "has" } else { "have" }
        );
    }
    out
}

pub async fn nodes(socket: &Path) -> Result<()> {
    let responses = client::request(socket, &Request::Nodes).await?;
    let Response::Nodes { nodes, local } = client::expect_one(responses)? else {
        bail!("unexpected response to nodes");
    };

    let Some(local) = local else {
        // Not an error and not an empty table: a node with no fleet is a supported way to
        // run, and printing nothing would look like a mesh that had failed.
        println!("This node is not in a mesh.");
        println!("  `offload init` founds a fleet here, `offload join --passphrase` enrols into");
        println!("  one, and [cluster] enabled = false in the config turns the mesh off. A device");
        println!("  that joined a fleet after offloadd started joins the mesh when it restarts.");
        return Ok(());
    };

    println!(
        "{:<14} {:<12} {:<12} {:>5} {:>7} {:>5} {:>8}  AGENT",
        "NODE", "NAME", "STATUS", "CPU", "MEM", "RUNS", "SEEN"
    );
    for node in &nodes {
        let mine = if node.id.to_string() == local {
            "*"
        } else {
            " "
        };
        // "seen 4s ago" is the question; a unix timestamp is not. First-hand only, so a node known
        // from other nodes' gossip says `never` rather than borrowing their contact.
        let seen = match node.last_heard_ms {
            None => "never".to_string(),
            Some(ms) if ms < 1_000 => "now".to_string(),
            Some(ms) if ms < 90_000 => format!("{}s", ms / 1_000),
            Some(ms) => format!("{}m", ms / 60_000),
        };
        let status = if node.absences > 0 {
            // A node that keeps going and coming back is not the same as one that has been
            // solid all week, and the hold-down policy treats them differently (ADR-0007).
            format!("{:?} ~{}", node.status, node.absences)
        } else {
            format!("{:?}", node.status)
        };
        println!(
            "{}{:<13} {:<12} {:<12} {:>5} {:>6}G {:>5} {:>8}  {}",
            mine,
            short_id(&node.id.to_string()),
            truncate(&node.name, 12),
            status.to_lowercase(),
            node.cpu_cores,
            node.memory_mb / 1024,
            node.running,
            seen,
            node.agent.as_deref().unwrap_or("-")
        );
    }
    let footnotes =
        nodes.iter().any(|n| n.absences > 0) || nodes.iter().any(|n| n.last_heard_ms.is_none());
    if footnotes {
        println!();
    }
    if nodes.iter().any(|n| n.absences > 0) {
        println!("~N: times that node has dropped off and come back. A node that always");
        println!("    returns quickly earns patience before its runs are moved (ADR-0007).");
    }
    if nodes.iter().any(|n| n.last_heard_ms.is_none()) {
        println!(
            "SEEN is when this node last heard from it. `never`: known only from other nodes'"
        );
        println!("    gossip, so its STATUS is theirs too.");
    }
    Ok(())
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

pub async fn ps(socket: &Path, all: bool) -> Result<()> {
    let responses = client::request(socket, &Request::List).await?;
    let Response::Runs {
        runs,
        resume_refusal,
    } = client::expect_one(responses)?
    else {
        bail!("unexpected response to list");
    };

    let visible: Vec<&RunSummary> = runs
        .iter()
        .filter(|r| all || !is_finished(&r.state))
        .collect();

    if visible.is_empty() {
        let message = if all {
            "no runs"
        } else {
            "no active runs (use --all to include finished ones)"
        };
        println!("{message}");
        return Ok(());
    }

    // One width for the id column, shared by the header and every row below it. They were two
    // numbers, and when `short_id` went from eight characters to twelve only the row was widened
    // — so every line of the most-read command in this CLI sat four columns to the right of the
    // heading it belonged under, and had done since the allowlist work.
    //
    // And it is computed from the rows rather than fixed, because two scheduled occurrences of
    // one tick display the same twelve characters and there is nowhere else on screen to get a
    // thirteenth from. See `id_width`; the usual listing is unaffected and still twelve.
    let id_chars = id_width(visible.iter().map(|r| r.id.as_str()));
    let id = id_chars + 2;
    // The row continuations hang inside the id column, so they follow it.
    let hang = " ".repeat(id_chars - 1);
    // `KIND` before the numeric columns rather than beside the work, and that ordering is the
    // whole point of the column: a task has no turns, no tokens and no cost, so the three
    // dashes to the right of it are read *after* the word that explains them. The last column
    // is `WORK` and not `PROMPT` because a prompt is one of the two things it holds — a table
    // whose busiest column means something different depending on the row is the report shape
    // this project keeps having to fix.
    const KIND: usize = 5;
    println!(
        "{:<id$} {:<13} {:<KIND$} {:>7} {:>7} {:>7} {:<10} {:<18} WORK",
        "RUN", "STATE", "KIND", "TURNS", "TOKENS", "COST", "SAFE", "WORKSPACE"
    );
    for r in &visible {
        // The daemon's own `Work::kind`, compared as a variant rather than as the string
        // `"task"`: a second spelling of the word is a second thing to keep in step.
        let is_task = r.kind == offload_core::WorkKind::Task;
        let cost = if r.cost_micro_usd == 0 {
            "-".to_string()
        } else {
            format!("${:.3}", r.cost_micro_usd as f64 / 1_000_000.0)
        };
        // Denials are shown inline rather than in a column of their own: a hobbled run
        // that looks like a bad model is the failure mode ADR-0008 has to stay visible.
        let state = if r.denials > 0 {
            format!("{} !{}", r.state, r.denials)
        } else {
            r.state.clone()
        };
        // Whether the work would survive this machine. Shown per run because it is a fact
        // about the run, and worth a column because "here only" is the difference between a
        // laptop closing costing nothing and costing an afternoon (ADR-0016).
        let safe = match r.checkpoint_durable {
            None => "-",
            Some(true) => "replicated",
            Some(false) => "here only",
        };
        // `4/10` where a limit was asked for, and the bare count where none was. The column
        // is the only place a turn limit is visible while it is still doing something: by the
        // time it reads `10/10` the run has stopped, and the number somebody wants then is the
        // one they wanted at turn eight.
        //
        // And a dash rather than `0` for work that does not take turns. A task has no turn
        // boundary at all (ADR-0019 §2), so a zero here would not be a count — it would be the
        // column saying "none yet" about a run for which none ever will, which is the
        // zero-that-means-unknown the ADR asked this table not to print. Same dash `TOKENS`
        // and `COST` already use, which those two got right by accident: a task spends
        // neither, so both were already empty.
        let turns = match (is_task, r.max_turns) {
            (true, _) => "-".to_string(),
            (false, Some(limit)) => format!("{}/{limit}", r.turns),
            (false, None) => r.turns.to_string(),
        };
        // Beside the cost and not instead of it (ADR-0040). The cost is the agent's own number
        // and is blank for every run that did not reach a `result` — checkpointed, drained,
        // cancelled, or stopped by its turn limit — which is exactly when somebody most wants to
        // know what it consumed. This column is there for those.
        let tokens = if r.tokens.is_empty() {
            "-".to_string()
        } else {
            compact(r.tokens.total())
        };
        println!(
            "{:<id$} {:<13} {:<KIND$} {:>7} {:>7} {:>7} {:<10} {:<18} {}",
            at_most(&r.id, id_chars),
            state,
            r.kind,
            turns,
            tokens,
            cost,
            safe,
            r.workspace,
            r.work
        );
        if let Some(due) = &r.due {
            println!("{hang}└─ due: {due}");
        }
        if let Some(held) = &r.held {
            println!("{hang}└─ {held}");
        }
        if let Some(parent) = &r.continues {
            println!("{hang}└─ continues {}", short_id(parent));
        }
        // Beside the total, never folded into it (ADR-0067 §5): the column is the conversation
        // the run went on to be, and this is what it cost besides.
        if r.lost_tokens > 0 {
            println!(
                "{hang}└─ +{} tokens spent by a leg that lost: the run was granted twice and this \
                 conversation is the one that went on",
                compact(r.lost_tokens)
            );
        }
        if let Some(err) = &r.error {
            println!("{hang}└─ {err}");
        }
    }
    if visible.iter().any(|r| r.checkpoint_durable == Some(false)) {
        println!();
        println!("here only: that run's checkpoint exists on one machine. If it goes away, so");
        println!("           does the work — a fleet of one has nowhere to put a copy.");
    }
    // The stored reason on a `failed` row names `offload resume` by name, and it is right about
    // the run: the conversation and the uncommitted edits are there, and some machine can carry
    // on with them. It is not a claim about *this* machine, which may since have been drained,
    // revoked, or told by its owner to host nothing (ADR-0047) — and the record must go on
    // saying what it says, because a permanent note about why a run stopped is the wrong place
    // for a fact that changes by the hour. So the live half arrives beside the listing and the
    // two are put together here, where somebody is reading the advice.
    //
    // Against the *visible* rows, which is why `resumable` is per run: without `--all` every
    // failed run is filtered out, and a footnote about work that is not on screen sends
    // somebody looking for it.
    if let Some(reason) = resume_refusal.filter(|_| visible.iter().any(|r| r.resumable)) {
        println!();
        // A colon rather than a dash before the reason: two of these sentences carry an em-dash
        // of their own ("revoked from its fleet — it can host nothing…"), and a second one in
        // front of them reads as a stutter.
        println!("resume:    a run above says it is resumable, which is true of the run. Not of");
        println!("           this node right now: {reason}");
    }
    Ok(())
}

pub async fn logs(socket: &Path, run: &str, follow: bool) -> Result<()> {
    client::stream(
        socket,
        &Request::Logs {
            run: run.to_string(),
            follow,
        },
        |response| {
            match response {
                Response::Event(event) => print_event(&event.kind, run),
                Response::Error { message } => bail!("{message}"),
                _ => {}
            }
            Ok(())
        },
    )
    .await
}

/// The closing sentence of `offload explain` for a run somebody already holds.
///
/// A function rather than a `println!` in the `else` for [`blocked_horizon`]'s reason, one step
/// worse: this one names *another line of its own output*, and nothing checks that the line is
/// there. It said "`held back` above is why it has not begun" unconditionally — but `held_back`
/// is only ever computed for a run in `assigned`, so on a `running` run it pointed at a line
/// that was never printed and called a run that started nine seconds ago one that had not
/// begun. Measured on a run stopped mid-tool-call waiting for a person.
///
/// The first clause is right in every case and is the whole reason the sentence exists: the
/// canvass above asks who would *take* the run, so a holder answers "already holds this run"
/// and every other node is refused for having nothing to do with it — which reads as nobody
/// wanting it. What changes is what to point at next, and the only safe answer is a line this
/// command actually printed.
fn taken_not_unplaced(held_back: bool, waiting: bool) -> &'static str {
    if held_back {
        "It is taken, not unplaced — `held back` above is why it has not begun."
    } else if waiting {
        // It began and then *stopped*, which is the one state where `running` and "making no
        // progress" are both true and neither is a fault.
        "It is taken, not unplaced — `waiting` above is what it is stopped on."
    } else {
        // Nothing more specific to point at, so point at the line that is always there rather
        // than at one that might not be.
        "It is taken, not unplaced — the `state` line above is where it has got to."
    }
}

/// `run` is what the operator typed, not the resolved id, and that is deliberate: it is the
/// one spelling known to resolve on this machine, and the only line here that is meant to be
/// pasted needs a spelling rather than a placeholder.
fn print_event(kind: &LogKind, run: &str) {
    for line in offload_node::render::event_lines(kind, run, crate::when::now_ms()) {
        println!("{line}");
    }
}

/// `offload explain <run>` — the answer to "why is this not happening".
///
/// Everything printed here was already a structured value somewhere; what this adds is
/// putting them side by side. The two halves answer different questions and are worth
/// reading in order: the top says who is *deciding* about this run and what they have
/// decided, and the bottom says what each node would do if asked to take it right now.
pub async fn explain(socket: &Path, run: &str) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Explain {
            run: run.to_string(),
        },
    )
    .await?;
    let Response::Explanation(e) = client::expect_one(responses)? else {
        bail!("unexpected response to explain");
    };

    // In full, and this is the one report where that is the right answer: there is no column to
    // fit and no listing to take a width from, and the id names the single run the whole screen
    // is about. Twelve characters here re-collapsed a pair of scheduled occurrences that
    // `offload ps` had just been widened to tell apart — so the id somebody typed came back
    // shorter than they typed it, naming both runs again. See `id_width`.
    println!("run         {}", e.run);
    // Which tier, on its own line, and then what the work is. It was one line reading
    // `prompt` over whatever `Work::summary` returned, which for a task is a shell script's
    // service — the label being the part that was wrong. Two lines rather than a qualifier
    // inside one, because `agent` here would collide with the agent's own *name*.
    println!("kind        {}", e.kind);
    println!("work        {}", truncate(&e.work, 60));
    println!("state       {} — {}", e.state, e.state_detail);
    println!("due         {}", e.due);
    if !e.demand.is_empty() {
        println!("demand      {}", e.demand);
    }
    // Only when somebody set it. `due` above is the slack that leads and this is what breaks
    // its ties, so the pair is what decides which of the runs waiting here goes next — and it
    // was readable nowhere: `offload priority` echoed the number once and no command would say
    // it again. Zero is the default and a line saying so on every run is noise.
    if e.priority != 0 {
        println!(
            "priority    {} — breaks ties in `due`, and nothing else (ADR-0013)",
            e.priority
        );
    }
    if !e.attendance.is_empty() {
        println!("attendance  {}", e.attendance);
    }
    println!("epoch       {}", e.epoch);
    println!("holder      {}", e.holder.as_deref().unwrap_or("nobody"));
    match (&e.arbiter, e.arbiter_is_local) {
        (Some(name), true) => println!("arbiter     {name} (this node)"),
        (Some(name), false) => println!("arbiter     {name}"),
        // Not a fault: a node that has never met the run's home node refuses to guess a
        // successor, because two arbiters grant one run twice (ADR-0006).
        (None, _) => println!("arbiter     undecided here"),
    }
    println!("            {}", e.verdict);
    // Why a run this node holds has not begun, which is the question for a run in `assigned` and
    // was the one thing this command did not answer about it.
    if let Some(reason) = &e.held_back {
        println!("held back   {reason}");
    }
    // …and the other way a run this node holds is making no progress, which is the one the
    // report could not see at all: it is *stopped*, mid-tool-call, waiting for a person
    // (ADR-0017). Beside `held back` because they answer one question in two states — that one
    // for a run that has not begun, this one for a run that began and then stopped.
    for ask in &e.waiting {
        println!(
            "waiting     stopped for an answer: {} — {}  ·  {} left",
            ask.tool, ask.detail, ask.left
        );
        // Both commands, and the run id **whole**: this is the line somebody acts on, and an
        // instruction has no column to fit, so there is nothing an abbreviation buys. It was
        // twelve characters, four lines under a header that prints the id in full — one screen
        // with two widths, and only the top one is guaranteed to resolve. Session seventy-four
        // made that header whole for this reason and the instruction below it was missed.
        println!(
            "            offload approve {} {}   (or `offload deny`)",
            e.run, ask.tool_use_id
        );
    }
    // …and for a failed one, whether it is coming back. `failed` looks the same whether a retry is
    // thirty seconds away or the run was deliberately left for the person who was watching.
    if let Some(recovery) = &e.recovery {
        println!("recovery    {recovery}");
    }
    // …and whether the way out this run's own `state` line names is open here. The two are not
    // the same answer: `recovery` is what this node does unasked, and a run it has left for a
    // person is one a person is being invited to pick up — an invitation a drained, revoked or
    // policy-refusing node declines (ADR-0047). Said as `offload resume` because that is the
    // command in the sentence three lines above it.
    if let Some(reason) = &e.resume_refusal {
        println!("resume      refused here right now: {reason}");
    }
    if let Some(checkpoint) = &e.checkpoint {
        println!("checkpoint  {checkpoint}");
    }

    println!();
    if let Some(reason) = &e.not_canvassed {
        println!("nobody was asked to take it: {reason}");
        return Ok(());
    }

    println!("what each node says about taking it, asked just now:");
    for opinion in &e.opinions {
        println!(
            "  {}{:<12} {}",
            if opinion.would_win { "→ " } else { "  " },
            truncate(&opinion.node, 12),
            opinion.verdict
        );
    }
    if e.parked && e.opinions.iter().any(|o| o.would_win) {
        // No round is coming for a parked run — the `arbiter` line above says so — so "would win
        // a round held right now" offered one nothing will hold (HANDOFF item 17). What the
        // canvass is still good for is where a resume could go, and `offload resume` runs on the
        // node it is typed at.
        println!();
        println!(
            "→ bids highest, but no round will be held: it is parked. `offload resume` starts"
        );
        println!("  it on the node you type it at; these bids say which would take it now.");
    } else if e.opinions.iter().any(|o| o.would_win) {
        println!();
        println!("→ would win a round held right now. Bids describe this second, so these are");
        println!("  today's answers rather than the ones that placed the run.");
    } else if e.opinions.iter().all(|o| !o.bidding) && e.holder.is_none() {
        // The case the whole command exists for: every node has a reason and none of them
        // is "no eligible nodes" (ADR-0014).
        println!();
        println!("No node would take it right now — each line above is why.");
    } else if e.opinions.iter().all(|o| !o.bidding) {
        // …and *not* for a run somebody already holds, which is the sentence this printed about
        // a run alpha had accepted thirty seconds earlier: the canvass asks who would *take* it,
        // so a holder answers "already holds this run" and every other node is refused for
        // having nothing to do with it. "No node would take it" is then exactly backwards, and it
        // is the line somebody reads when they are already worried.
        println!();
        println!(
            "{}",
            taken_not_unplaced(e.held_back.is_some(), !e.waiting.is_empty())
        );
    }
    Ok(())
}

/// `offload deadline <run> <when>` — I need this by a different time now.
pub async fn set_deadline(socket: &Path, run: &str, deadline: Option<u64>) -> Result<()> {
    edit(
        socket,
        Request::SetDeadline {
            run: run.to_string(),
            deadline,
        },
    )
    .await
}

/// `offload priority <run> <n>` — let this one go ahead of the others.
pub async fn set_priority(socket: &Path, run: &str, priority: i32) -> Result<()> {
    edit(
        socket,
        Request::SetPriority {
            run: run.to_string(),
            priority,
        },
    )
    .await
}

/// The two editable fields, printed the same way, because the thing worth saying is the same:
/// what the field says now, and what changing it actually does to *this* run.
async fn edit(socket: &Path, request: Request) -> Result<()> {
    let responses = client::request(socket, &request).await?;
    let Response::SpecEdited { run, summary, note } = client::expect_one(responses)? else {
        bail!("unexpected response to a spec edit");
    };

    println!("run {} — {summary}", short_id(&run));
    // What the change actually does, which depends on what the run is doing. Printed always,
    // because the gap between "I moved the deadline" and "nothing got faster" is where these
    // commands would otherwise be reported as broken.
    println!("  {note}");
    Ok(())
}

/// `offload cancel <run>` — stop a run, wherever in the fleet it is.
///
/// The node is named when it is not this one, and the note says what was actually stopped: an
/// agent mid-turn and a commitment that never started are both "cancelled", and the difference is
/// whether the last few minutes cost anything.
pub async fn cancel(socket: &Path, run: &str) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Cancel {
            run: run.to_string(),
        },
    )
    .await?;
    for response in responses {
        match response {
            Response::Error { message } => bail!("{message}"),
            Response::Cancelled { run, node, note } => match node {
                Some(node) => println!("cancelled {} on {node}: {note}", short_id(&run)),
                None => println!("cancelled {}: {note}", short_id(&run)),
            },
            _ => {}
        }
    }
    Ok(())
}

/// `offload approve|deny <run> [tool_use_id]` — answer a question an agent is blocked on.
///
/// The run is stopped while this is outstanding, so the answer is reported with *what* it was
/// about: an agent can be blocked on more than one call at a time, and "approved" without saying
/// what would be the wrong kind of reassuring.
pub async fn answer(socket: &Path, run: &str, which: Option<&str>, allow: bool) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Answer {
            run: run.to_string(),
            tool_use_id: which.map(str::to_string),
            allow,
        },
    )
    .await?;
    match client::expect_one(responses)? {
        Response::Answered {
            tool,
            detail,
            allowed,
        } => {
            println!(
                "{} {tool}: {detail}",
                if allowed { "allowed" } else { "denied" }
            );
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        _ => bail!("unexpected response to an answer"),
    }
}

/// `offload asks` — what is waiting for a person on this node.
///
/// Local by design, like `offload sinks`: the process that is blocked is on the machine running
/// the agent, so a question somewhere else is answered where it is being asked.
pub async fn asks(socket: &Path) -> Result<()> {
    let responses = client::request(socket, &Request::Asks).await?;
    let Response::Asks { asks } = client::expect_one(responses)? else {
        bail!("unexpected response to asks");
    };
    if asks.is_empty() {
        println!("nothing is waiting for an answer");
        return Ok(());
    }
    println!(
        "{:<14} {:<10} {:<8} {:<8} {:<10} WANTS TO",
        "RUN", "TOOL", "WAITING", "LEFT", "WHERE"
    );
    for ask in &asks {
        let run = ask.run.to_string();
        println!(
            "{:<14} {:<10} {:<8} {:<8} {:<10} {}",
            short_id(&run),
            ask.tool,
            ask.waiting.to_string(),
            ask.left.to_string(),
            // Where the agent is blocked, which is usually not where you are typing — and that
            // is the point: the answer is forwarded to it.
            ask.where_it_waits(),
            ask.detail
        );
        // The id is what disambiguates when one run is blocked on several calls, so it is shown
        // rather than left to be discovered from the log. Both answers, like the other two
        // places that offer one: this is the report whose whole job is the question, and it was
        // the only one of the three naming just the *yes* — in front of a line that is as often
        // `rm -rf` as it is `cargo test`.
        //
        // The run id **whole**, unlike the RUN column above it, which is a table and is padded.
        // A line meant to be pasted is not a column and has nothing to line up with, so there is
        // no width to trade against — and a pasted command that the command it names then
        // refuses is the worst of the three outcomes available here.
        println!(
            "               offload approve {run} {}   (or `offload deny`)",
            ask.tool_use_id
        );
    }
    Ok(())
}

pub async fn checkpoint(socket: &Path, run: &str) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Checkpoint {
            run: run.to_string(),
        },
    )
    .await?;
    for response in responses {
        match response {
            Response::Error { message } => bail!("{message}"),
            // Deliberately future tense: the agent may be mid-turn, and mid-turn is not a safe
            // capture point. Saying "checkpointed" here would be a lie for however long the
            // current turn has left to run. The machine is named when it is not this one — the
            // boundary being waited for is on that machine's clock, not ours.
            Response::CheckpointRequested {
                node: Some(node), ..
            } => println!(
                "checkpoint requested on {node} — it will be taken at that run's next turn \
                 boundary"
            ),
            Response::CheckpointRequested { node: None, .. } => {
                println!("checkpoint requested — it will be taken at the next turn boundary");
            }
            _ => {}
        }
    }
    println!("watch for it with: offload logs -f {}", short_id(run));
    Ok(())
}

pub async fn resume(socket: &Path, run: &str, prompt: Option<String>, follow: bool) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Resume {
            run: run.to_string(),
            prompt,
        },
    )
    .await?;

    let Response::Submitted { run, .. } = client::expect_one(responses)? else {
        bail!("unexpected response to resume");
    };

    println!("resumed {}", short_id(&run));
    if follow {
        logs(socket, &run, true).await?;
    } else {
        println!("follow with: offload logs -f {}", short_id(&run));
    }
    Ok(())
}

/// `offload continue` (ADR-0064): send the request, and say where the new run went.
pub async fn continue_run(
    socket: &Path,
    request: offload_node::api::ContinueRequest,
    follow: bool,
) -> Result<()> {
    let parent = request.run.clone();
    let mode = request.mode;
    let responses = client::request(socket, &Request::Continue(Box::new(request))).await?;
    let Response::Submitted {
        run,
        node,
        waiting,
        queued,
        here_only,
        preference,
        siblings,
        ..
    } = client::expect_one(responses)?
    else {
        bail!("unexpected response to continue");
    };
    let where_it_went = node.map_or_else(String::new, |name| format!(" — accepted by {name}"));
    let when = waiting.map_or_else(String::new, |when| format!(", {when}"));
    if let Some(reasons) = &queued {
        println!(
            "run {} continues {parent} ({mode}) — queued; nobody will take it yet:{reasons}",
            short_id(&run)
        );
        return Ok(());
    }
    println!(
        "run {} continues {parent} ({mode}){where_it_went}{when}",
        short_id(&run)
    );
    if let Some(preference) = &preference {
        println!("  {preference}");
    }
    if !siblings.is_empty() {
        // Whole ids: the next thing typed is `offload continue <one of these>`.
        println!(
            "  {parent} was already continued by: {} — this one starts from {parent} too, not from those",
            siblings.join(", ")
        );
    }
    if here_only {
        println!("  no other node has a copy — if this machine goes away, so does the run");
    }
    if follow {
        logs(socket, &run, true).await?;
    } else {
        println!("follow with: offload logs -f {}", short_id(&run));
    }
    Ok(())
}

pub async fn remove(socket: &Path, run: &str) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Remove {
            run: run.to_string(),
        },
    )
    .await?;
    let mut discarded = None;
    for response in responses {
        match response {
            Response::Error { message } => bail!("{message}"),
            Response::Removed {
                discarded: held, ..
            } => discarded = held,
            _ => {}
        }
    }
    println!("{}", removed_line(discarded.as_deref()));
    Ok(())
}

/// What `offload rm` says afterwards.
///
/// The old line was *"worktree removed (the run's branch and commits are kept)"*, unconditionally
/// — the reassuring half of a true sentence, and the only half. A `failed` run is terminal *and*
/// resumable, so its worktree is the one most likely to hold edits past the last turn boundary,
/// and those have no copy anywhere: not on the branch, not in the checkpoint, not in the bundle.
/// Measured — a modified file and an untracked one, removed, then resumed to completion, with
/// `offload ps`, `offload logs` and `offload audit` between them saying nothing about either.
///
/// So the parenthesis stays for the ordinary case and gives way to what actually went when there
/// was something. Not a refusal: whoever typed this is entitled to the disk back (the command is
/// manual for exactly that reason). What they are owed is being told, once, while they can still
/// act on it — `git fsck` finds nothing here, because none of it was ever an object.
fn removed_line(discarded: Option<&str>) -> String {
    match discarded {
        None => "worktree removed (the run's branch and commits are kept)".to_string(),
        Some(held) => format!(
            "worktree removed — its branch and commits are kept, but it held {held} that nothing else has a copy of, and that is gone"
        ),
    }
}

/// How much of a run id to show.
///
/// The same twelve characters `RunId::short` produces, and the reasoning lives there. This exists
/// because the control protocol carries ids as strings: the CLI never holds a `RunId`, so it
/// cannot ask one. The two lengths agree, which they did not while the daemon's own refusals
/// printed eight — the CLI said `01a026064ec4` and the error beside it said `01a02606`.
fn short_id(id: &str) -> &str {
    at_most(id, SHORT_ID)
}

/// The twelve characters of [`short_id`], named because [`id_width`] counts up from it.
const SHORT_ID: usize = 12;

/// The first `chars` characters, or the whole thing if it is shorter.
///
/// `get` rather than a slice: an id is ASCII hex and `&id[..n]` was fine for as long as that held,
/// but it is a panic on a char boundary if anything ever hands this a string that is not one, and
/// a listing is the wrong place to find that out.
fn at_most(id: &str, chars: usize) -> &str {
    id.get(..chars).unwrap_or(id)
}

/// How much of a run id a *listing* has to show, which is not a constant.
///
/// Twelve is [`short_id`]'s answer and it is right for a run whose id is a clock plus
/// randomness: two of those share their displayed form only when they were submitted in the
/// same millisecond. It is wrong for a **scheduled occurrence**, whose id
/// `Schedule::occurrence_id` derives from the tick and the schedule — the tick's milliseconds
/// land in exactly the six bytes `RunId::short` displays, and what follows them is a digest of
/// the schedule rather than randomness. So `every 1m` beside `every 2m` produces two different
/// runs with one displayed id on every boundary they share, for ever; both then refuse every
/// command that takes an id, and the screen holds no more characters to obey *"ambiguous — use
/// more characters"* with.
///
/// Session nineteen weighed this and left it, correctly for what existed then: with a random
/// tail, fourteen characters would have reached one random byte and turned a same-millisecond
/// collision into a 1-in-256 one, "a worse kind of rare". ADR-0056's derived id has no random
/// tail, so the answer is not a longer fixed prefix — it is a width that is a fact about what is
/// on screen: the shortest whole number of bytes at which these rows are distinct.
fn id_width<'a>(ids: impl Iterator<Item = &'a str>) -> usize {
    let mut full: Vec<&str> = ids.collect();
    full.sort_unstable();
    full.dedup();
    let mut need = SHORT_ID;
    for pair in full.windows(2) {
        // One character past where they stop agreeing. Adjacent pairs of a sorted list are the
        // only ones that can share a prefix, so this is the whole comparison.
        let common = pair[0]
            .bytes()
            .zip(pair[1].bytes())
            .take_while(|(a, b)| a == b)
            .count();
        need = need.max(common + 1);
    }
    // Whole bytes, because an id is bytes and half of one names nothing.
    (need + need % 2).min(32)
}

/// A token count in the width a table column has: `84.2k`, `1.4M`.
///
/// Rounded on purpose. The exact number is a fact about somebody else's billing that nobody
/// reads digit by digit in a list — what the column answers is "was this run expensive", and a
/// figure narrow enough to sit beside the others answers it better than one that pushes the
/// prompt off the line.
fn compact(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

fn is_finished(state: &str) -> bool {
    offload_node::render::is_finished(state)
}

/// Parse `--permission`, accepting the spellings people actually type.
/// `--ask`, `--ask 5`, or the absence of the flag.
///
/// Zero is *never*, which is what the flag's absence resolves to, so the three cases are one
/// value rather than an `Option` the daemon has to interpret. Somebody typing `--ask 0`
/// explicitly gets the same thing, and it means what it says.
pub fn parse_ask(value: &str) -> Result<offload_core::AskPolicy, String> {
    let questions: u32 = value
        .trim()
        .parse()
        .map_err(|_| format!("--ask takes a number of questions, not `{value}`"))?;
    Ok(if questions == 0 {
        offload_core::AskPolicy::Never
    } else {
        offload_core::AskPolicy::UpTo { questions }
    })
}

/// Parse `--max-turns`: how many turns a run may take before it is stopped.
///
/// Refuses zero rather than accepting it, which is the opposite of `--ask 0` next door and the
/// difference is worth the two parsers. There, zero is a *setting* — "never ask me" is the
/// flag's own absence said out loud. Here it would be "take no turns", which is not a run
/// anybody means to submit: the sentence somebody is reaching for when they type it is either
/// "don't run this" (don't submit it) or "no limit" (leave the flag off). Refusing at the
/// keyboard is the only place the difference can still be asked about — this is
/// `WorkPolicy::allowed_agents`' empty list, and the daemon refuses it a second time in the
/// type, because a rule stored tonight is fired by a build that never saw this parser.
pub fn parse_max_turns(value: &str) -> Result<std::num::NonZeroU32, String> {
    let turns: u32 = value
        .trim()
        .parse()
        .map_err(|_| format!("--max-turns takes a number of turns, not `{value}`"))?;
    std::num::NonZeroU32::new(turns).ok_or_else(|| {
        "--max-turns 0 would be a run that takes no turns; leave the flag off for no limit"
            .to_string()
    })
}

pub fn parse_permission(value: &str) -> Result<PermissionMode, String> {
    match value.to_ascii_lowercase().replace(['-', '_'], "") {
        v if v == "ask" || v == "manual" => Ok(PermissionMode::Ask),
        v if v == "acceptedits" || v == "edits" => Ok(PermissionMode::AcceptEdits),
        v if v == "full" || v == "bypass" || v == "bypasspermissions" => Ok(PermissionMode::Full),
        _ => Err(format!(
            "unknown permission mode `{value}` (expected: ask, accept-edits, full)"
        )),
    }
}

/// Parse `--notify`: a service to be reached by, or `all`, or `none`.
///
/// Thin over [`offload_core::Audience`]'s own `FromStr`, which is where the spellings live —
/// two parsers for one word is how a fleet ends up with two names for the same route. What is
/// added here is the one thing a CLI can add: an error that says what was expected, and a
/// refusal to route news to an agent, which is a service that is not a route to anybody.
/// A service a run asks to be able to use (ADR-0011).
///
/// Unknown names become `Service::Other`, which is the escape hatch working as designed — a
/// fleet may offer something this build has never heard of. The one name refused is an agent:
/// `Service` is one enum for every plane, so `agent:claude-code` parses and would then match no
/// resource anywhere, silently. `parse_audience` refuses it for the same reason on the other
/// plane.
pub fn parse_service(value: &str) -> Result<offload_core::Service, String> {
    let service: offload_core::Service = value
        .parse()
        .map_err(|e: offload_core::UnknownService| e.to_string())?;
    if let offload_core::Service::Agent(kind) = &service {
        return Err(format!(
            "{kind} is the agent running your work, not something it can be granted the use of"
        ));
    }
    Ok(service)
}

/// `--notify-on everything|problems`, the kinds half of ADR-0026.
pub fn parse_notices(value: &str) -> Result<offload_core::Notices, String> {
    value.parse()
}

pub fn parse_audience(value: &str) -> Result<offload_core::Audience, String> {
    let audience: offload_core::Audience = value.parse().map_err(|_| {
        format!("`{value}` is not a way to reach anybody (try: push, email, all, none)")
    })?;
    if let offload_core::Audience::Service {
        service: offload_core::Service::Agent(kind),
    } = &audience
    {
        // `Service` is one enum for both planes, so `agent:claude-code` parses — and would then
        // match no route in any fleet, silently. Refused here rather than explained later.
        return Err(format!("{kind} is an agent, not a way of reaching anybody"));
    }
    Ok(audience)
}

/// Everything `offload when` needs: the submission, plus what only a standing instruction has.
pub struct Standing<'a> {
    pub socket: &'a Path,
    /// What the rule is bound to: a trigger's service, or a notice's name (ADR-0057).
    pub service: String,
    /// Which of those two the name above is.
    pub fired_by: offload_node::api::FiredBy,
    /// What a firing starts: an agent run, or the cheap tier (ADR-0019).
    pub work: StandingWork<'a>,
    /// How long each fired run gets, from the firing. A duration, not an instant — see
    /// `when::parse_duration_secs`.
    pub deadline_secs: Option<u64>,
}

/// One of the two, never both — the shape `offload run` and `offload every` already have.
pub enum StandingWork<'a> {
    Agent(Submission<'a>),
    Task(TaskSubmission<'a>),
}

/// Write a standing instruction: when a trigger fires here, submit this run (ADR-0020).
/// `offload when <service> --task <task>` — a trigger that starts the cheap tier.
///
/// Its own function rather than a branch inside [`stand_up`], and the split is where the two
/// commands genuinely differ: an agent rule validates an allowlist and warns about risky grants,
/// and a task rule has neither to validate — the owner who wrote `[[tasks]]` decided what the
/// program may do (ADR-0019 §1). What they share is everything after the request is built, which
/// is [`report_watching`].
async fn stand_up_task(
    socket: &Path,
    service: &str,
    fired_by: offload_node::api::FiredBy,
    task: TaskSubmission<'_>,
    deadline_secs: Option<u64>,
) -> Result<()> {
    let notices = task.notices;
    let responses = client::request(
        socket,
        &Request::Watch {
            service: service.to_string(),
            rule: RuleRequest {
                fired_by,
                work: offload_node::api::RuleWork::Task(offload_node::api::SubmitTaskRequest {
                    service: task.service,
                    args: task.args,
                    queue: task.queue,
                    // A rule holds a *duration* and the daemon resolves it at each firing, for
                    // the reason `RuleSpec` gives at length: an instant resolved here would be
                    // in the past by the second firing.
                    deadline: None,
                    demand: task.demand,
                    notify: task.notify,
                    notices,
                    // Stamped by the daemon at the firing (ADR-0024), never trusted from here.
                    origin: offload_core::Origin::Operator,
                    // Resolved by the daemon when it writes the rule, so `here` is this machine.
                    require: task.placing.require,
                    prefer: task.placing.prefer,
                    hold_until: None,
                }),
                deadline_secs,
            },
        },
    )
    .await?;
    report_watching(service, notices, responses)
}

pub async fn stand_up(standing: Standing<'_>) -> Result<()> {
    let Standing {
        socket,
        service,
        fired_by,
        work,
        deadline_secs,
    } = standing;

    let submission = match work {
        StandingWork::Agent(submission) => submission,
        // The cheap tier has no allowlist to check and no prompt to append to: what a task may
        // do was decided by the owner who nominated the program, which is ADR-0019 §1's whole
        // point and the reason there is nothing for this command to warn about here.
        StandingWork::Task(task) => {
            return stand_up_task(socket, &service, fired_by, task, deadline_secs).await;
        }
    };

    // The same validation `run` does, in the same place and for a sharper version of the same
    // reason: a rule is written once and fires unattended for months, so a grant nobody looked
    // at twice is one nobody will look at at all.
    let parsed = offload_core::ToolAllowlist::parse(&submission.allow)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for (pattern, risk) in parsed.risky() {
        eprintln!(
            "warning: --allow {pattern} grants {}, not just that command",
            match risk {
                offload_core::Risk::Unrestricted => "unrestricted shell access",
                offload_core::Risk::GeneralExecution => "general execution",
                offload_core::Risk::Scoped => unreachable!(),
            }
        );
    }

    let repo = std::fs::canonicalize(&submission.repo)
        .map_or(submission.repo.clone(), |p| p.display().to_string());

    let notices = submission.notices;
    let request = SubmitRequest {
        repo,
        prompt: submission.prompt,
        model: submission.model,
        permission: submission.permission,
        git_ref: submission.git_ref,
        allow: submission.allow,
        queue: submission.queue,
        deadline: None,
        demand: submission.demand,
        notify: submission.notify,
        notices: submission.notices,
        ask: submission.ask,
        resources: submission.resources,
        max_turns: submission.max_turns,
        // Resolved by the daemon when it writes the rule, so `here` is this machine.
        require: submission.placing.require,
        prefer: submission.placing.prefer,
        hold_until: None,
        // Stored on the rule and overwritten at every firing by `RuleSpec::into_request`, which
        // is where the daemon says what it is. Written here so the value is never absent.
        origin: offload_core::Origin::Operator,
    };

    let responses = client::request(
        socket,
        &Request::Watch {
            service: service.clone(),
            rule: RuleRequest {
                fired_by,
                work: offload_node::api::RuleWork::Agent(request),
                deadline_secs,
            },
        },
    )
    .await?;
    report_watching(&service, notices, responses)
}

/// What `offload when` prints once the rule is written, for either tier.
///
/// One copy, shared by the two commands above, because every line of it is about the *rule* and
/// the fleet rather than about what the rule fires: where its news goes, which kinds interrupt
/// somebody, and whether anything here watches the service at all. A second copy would be two
/// places for a precondition to stop being reported (ADR-0032's own lesson, and this file
/// already carries the version of it about `--use`).
fn report_watching(
    service: &str,
    notices: offload_core::Notices,
    responses: Vec<Response>,
) -> Result<()> {
    let Response::Watching {
        rule,
        trigger_present,
        fired_by,
        audience,
        ask,
        reach,
        resources,
    } = client::expect_one(responses)?
    else {
        bail!("unexpected response to when");
    };

    match fired_by {
        offload_node::api::FiredBy::Trigger => println!("rule {rule} — on `{service}`"),
        // Said differently because it *is* different, and the difference is the thing somebody
        // needs to know about an escalation: it fires on this node's own news, and a run a
        // machine started produces none of it (ADR-0057 §3), so an escalation cannot escalate
        // itself.
        offload_node::api::FiredBy::Notice => {
            println!("rule {rule} — when a run here is `{service}`");
            println!("  It fires on this node's own news, so nothing has to be watching.");
            println!("  A run that a *machine* started fires nothing: an escalation does not");
            println!("  escalate itself, and its own failure is a notification for a person.");
        }
    }
    // Where its news goes, before what it will say about itself: whether anything can carry a
    // notification is what decides if the next line is a promise or a setting.
    if let Some(note) = &audience {
        println!("  {note}");
    }
    // A default that differs from `offload run`'s has to be said, once, where it is chosen.
    // A rule is written and then fires unattended for months, so "quiet unless something is
    // wrong" is either the thing somebody wanted or the thing they need to know they got.
    //
    // Said in the future tense only where the fleet can keep it. This printed "you will hear
    // about a failure, a missed deadline or a question" on a node with no route to a person —
    // three notifications, none of them possible — and the plainest invocation of the command
    // was one of the cases, because the default audience is the one `audience_note` above is
    // deliberately silent about.
    match (reach, notices) {
        (Reach::Somebody, offload_core::Notices::Problems) => println!(
            "  Quiet while it works: you will hear about a failure, a missed deadline or a \
             question, and not about a firing that went fine. `--notify-on everything` for a \
             heartbeat."
        ),
        (Reach::Somebody, offload_core::Notices::Everything) => println!(
            "  Every firing will be reported, including the ones that go fine — which is a \
             notification per event on whatever this fleet can reach."
        ),
        // The setting still travels with every occurrence and this fleet may gain a route
        // tomorrow, so which way the switch went is worth saying — as a setting, not a promise.
        (Reach::NobodyWanted, kinds) => println!(
            "  Set to report {kinds} — kept on the rule, and carried to nobody while the \
             audience is none."
        ),
        (Reach::NoRoute, kinds) => {
            println!(
                "  Set to report {kinds}, and there is nothing in this fleet to report it to,"
            );
            println!("  so the rule will fire silently — including when it fails. `offload sinks`");
            println!("  says how to add a route.");
        }
    }
    // What `--ask` amounts to here. Last of the three because it is about what the run *does*
    // rather than who hears about it, and it is the only one that can be absent for a good
    // reason — the rule did not ask.
    if let Some(note) = &ask {
        println!("  {note}");
    }
    // And what `--use` amounts to here, which is the one of the three that is a *refusal* on the
    // neighbouring command: a fired run asking for a service nobody offers is not submitted at
    // all. Beside the other preconditions rather than at the end, because it is a fact about the
    // fleet and not about this node's watchers.
    if let Some(note) = &resources {
        println!("  {note}");
    }
    if trigger_present {
        // Only when there is nothing standing in the way. "Nothing else to do" was printed over
        // a rule whose every firing would be refused, which is the reassurance in place of the
        // warning that ADR-0032 was written about.
        if resources.is_none() {
            println!("  Nothing else to do: the next event fires it.");
        } else {
            println!("  The next event will fire it, and be refused, until then.");
        }
    } else {
        // Written rather than refused, and said rather than left to be discovered. A rule for a
        // service nothing here watches is inert, and inert-and-silent is how somebody spends a
        // week wondering why their trigger never fired.
        println!("  Nothing on this node watches `{service}` yet, so it will not fire.");
        println!("  Add a watcher to the node config and restart offloadd:");
        println!();
        println!("      [[triggers]]");
        println!("      id = \"{service}-watch\"");
        println!("      service = \"{service}\"");
        println!("      command = \"/home/you/bin/watch-{service}\"");
        println!();
        println!("  One line of its stdout is one event. `offload triggers` lists them.");
    }
    Ok(())
}

/// This node's standing instructions.
pub async fn rules(socket: &Path) -> Result<()> {
    let responses = client::request(socket, &Request::Rules).await?;
    let Response::Rules { rules } = client::expect_one(responses)? else {
        bail!("unexpected response to rules");
    };

    if rules.is_empty() {
        println!("No standing instructions on this node.");
        println!("  `offload when <service> -- \"<what to do>\"` writes one, and");
        println!("  `offload triggers` says what this device is watching.");
        return Ok(());
    }

    // One `const` for the header and the rows, because two numbers that agree only because
    // somebody remembered are two things to remember — `offload ps` printed every row four
    // columns right of its own heading for two phases on exactly this.
    println!(
        "{:<18} {:<12} {:<5} {:>6} {:>8} {:>5}  WHAT",
        "RULE", "ON", "KIND", "FIRED", "DROPPED", "KEPT"
    );
    for rule in &rules {
        println!(
            "{:<18} {:<12} {:<5} {:>6} {:>8} {:>5}  {}",
            truncate(&rule.id, 18),
            truncate(&rule.service, 12),
            // `offload ps`'s column, on the other table that describes work: `WHAT` holds a
            // prompt on one row and a task's service on the next, and a column that means two
            // things depending on the row is what this project keeps having to fix.
            rule.kind,
            rule.fired,
            rule.dropped,
            rule.kept,
            truncate(rule.work.lines().next().unwrap_or(""), 40)
        );
        // Only about a rule a *trigger* fires: the delivery plane fires a notice-bound one and
        // is always there, so this line printed "nothing on this node watches `failed` — it
        // cannot fire" under an escalation rule that had just fired (ADR-0057).
        if !rule.trigger_present && rule.fired_by == offload_node::api::FiredBy::Trigger {
            println!(
                "                   └─ nothing on this node watches `{}` — it cannot fire",
                rule.service
            );
        }
        if rule.fired_by == offload_node::api::FiredBy::Notice {
            println!(
                "                   └─ fires when a run here is `{}`; a run a machine started \
                 fires nothing",
                rule.service
            );
        }
        // Said only when it is the quiet setting, because that is the one where a silence has
        // two readings — nothing went wrong, or nothing is being reported.
        if rule.notices == "problems" {
            println!("                   └─ reports problems only, not a firing that went fine");
        }
        match (&rule.in_flight, &rule.last_run) {
            // What is true now beats what was true once. A rule with an occurrence going is the
            // reason the next event will be dropped, and it is the one thing here that changes
            // between two readings a second apart.
            (Some(run), _) => println!("                   └─ running now {run}"),
            (None, Some(run)) => println!("                   └─ last fired {run}"),
            (None, None) => {}
        }
        // A dropped count without its reason is the number that prompts the question rather
        // than answering it, and the reason is why the last event produced nothing.
        if let Some(why) = &rule.last_error {
            println!("                   └─ {why}");
        }
    }
    Ok(())
}

/// Forget one standing instruction.
pub async fn unwatch(socket: &Path, rule: &str) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Unwatch {
            rule: rule.to_string(),
        },
    )
    .await?;
    let Response::Forgotten { rule, kept } = client::expect_one(responses)? else {
        bail!("unexpected response to unwatch");
    };
    println!("rule {rule} forgotten");
    if kept > 0 {
        // What survives a prune is a failed occurrence or one whose news has not gone out
        // (ADR-0021), and with the rule gone nothing else will ever mention them.
        println!("  {kept} occurrence record(s) kept — `offload ps --all` lists them");
    }
    Ok(())
}

/// What this node is watching (ADR-0020).
pub async fn triggers(socket: &Path) -> Result<()> {
    let responses = client::request(socket, &Request::Triggers).await?;
    let Response::Triggers { triggers } = client::expect_one(responses)? else {
        bail!("unexpected response to triggers");
    };

    if triggers.is_empty() {
        println!("This node is watching nothing.");
        println!("  A trigger is a program you nominate that notices something happening —");
        println!("  a mailbox, a webhook, a clock. One line of its stdout is one event.");
        println!();
        println!("      [[triggers]]");
        println!("      id = \"nightly\"");
        println!("      service = \"schedule\"");
        println!("      command = \"/bin/sh\"");
        println!("      args = [\"-c\", \"while sleep 3600; do echo tick; done\"]");
        println!();
        println!("  There is deliberately no interval setting: the cadence is the program's,");
        println!("  which is what keeps a scheduler out of the daemon. `offload when` binds");
        println!("  one to work.");
        return Ok(());
    }

    println!(
        "{:<14} {:<12} {:>7} {:>6} {:>9}  STATE",
        "TRIGGER", "SERVICE", "EVENTS", "RULES", "RESTARTS"
    );
    for trigger in &triggers {
        let state = match (&trigger.last_error, trigger.watching, trigger.rules) {
            // Up and bound to nothing: the commonest reason "my trigger does not work" turns
            // out to be true and uninteresting, so it is said before anything subtler.
            (_, true, 0) => "watching — but no rule is bound to it".to_string(),
            (None, true, _) => "watching".to_string(),
            // Up *and* carrying an error is the restart loop having recovered. Worth saying,
            // because the count beside it is otherwise unexplained.
            (Some(why), true, _) => format!("watching — last stopped: {why}"),
            (Some(why), false, _) => format!("down, retrying: {why}"),
            (None, false, _) => "starting".to_string(),
        };
        println!(
            "{:<14} {:<12} {:>7} {:>6} {:>9}  {}",
            truncate(&trigger.id, 14),
            truncate(&trigger.service, 12),
            trigger.events,
            trigger.rules,
            trigger.restarts,
            state
        );
        if !trigger.description.is_empty() {
            println!("               └─ {}", trigger.description);
        }
        // This node's own watcher, so the command belongs here: it is the owner reading about
        // their own machine, and it is never gossiped.
        println!("               └─ {}", trigger.command);
    }
    Ok(())
}

/// What `offload every` sends. One of the two work halves, never both.
pub struct Recurring<'a> {
    pub socket: &'a Path,
    pub every_ms: u64,
    pub at_ms: Option<u64>,
    pub note: String,
    pub demand: offload_core::Demand,
    pub notify: offload_core::Audience,
    pub queue: bool,
    /// The cheap tier: a service and its arguments.
    pub task: Option<(offload_core::Service, Vec<String>)>,
    /// …or an agent run: a prompt, a repo, and the usual two overrides.
    pub agent: Option<RecurringAgent>,
}

/// The agent half of a schedule, for the case somebody really does want a model on a clock.
pub struct RecurringAgent {
    pub prompt: String,
    pub repo: String,
    pub model: Option<String>,
    pub allow: Vec<String>,
}

/// `offload every <interval>` — write down something that runs on a clock (ADR-0019 §3).
pub async fn every(spec: Recurring<'_>) -> Result<()> {
    // `Problems` rather than `Everything`, and it is the same default `offload when` chose for
    // the same reason: a standing instruction's value is that it is quiet until something
    // happens, and a watcher that reported success every fifteen minutes would train its owner
    // to ignore it (ADR-0026 §3).
    let notices = offload_core::Notices::Problems;
    let request = match (spec.task, spec.agent) {
        (Some((service, args)), None) => offload_node::api::EverySpec {
            every_ms: spec.every_ms,
            offset_ms: spec.at_ms.unwrap_or_default(),
            note: spec.note,
            agent: None,
            task: Some(offload_node::api::SubmitTaskRequest {
                service,
                args,
                queue: spec.queue,
                // A schedule's occurrence gets no stated deadline. `offload when` stores a
                // *duration* because a rule's firing is an event somebody may be waiting on;
                // a tick is a moment that will come again, and a deadline would make every
                // skipped occurrence an overdue one (ADR-0013).
                deadline: None,
                demand: spec.demand,
                notify: spec.notify,
                notices,
                // Overwritten by the daemon when it builds each occurrence, which is where
                // ADR-0024's fact is stated (`schedule::occurrence`). Written so it is never
                // absent.
                origin: offload_core::Origin::Operator,
                require: offload_core::Wanted::default(),
                prefer: offload_core::Wanted::default(),
                hold_until: None,
            }),
        },
        (None, Some(agent)) => {
            let repo = std::fs::canonicalize(&agent.repo)
                .map_or(agent.repo.clone(), |p| p.display().to_string());
            offload_node::api::EverySpec {
                every_ms: spec.every_ms,
                offset_ms: spec.at_ms.unwrap_or_default(),
                note: spec.note,
                task: None,
                agent: Some(SubmitRequest {
                    repo,
                    prompt: agent.prompt,
                    model: agent.model,
                    permission: None,
                    git_ref: None,
                    allow: agent.allow,
                    queue: spec.queue,
                    deadline: None,
                    demand: spec.demand,
                    notify: spec.notify,
                    notices,
                    ask: offload_core::AskPolicy::Never,
                    resources: Vec::new(),
                    max_turns: None,
                    // Overwritten by the daemon when it builds the occurrence, which is where
                    // ADR-0024's fact is stated. Written here so the field is never absent.
                    origin: offload_core::Origin::Operator,
                    require: offload_core::Wanted::default(),
                    prefer: offload_core::Wanted::default(),
                    hold_until: None,
                }),
            }
        }
        _ => bail!("say either --task <service> or --prompt, and not both"),
    };

    let responses = client::request(spec.socket, &Request::Every(Box::new(request))).await?;
    match client::expect_one(responses)? {
        Response::Scheduled {
            schedule,
            next,
            audience,
            reach,
            resources,
        } => {
            println!("schedule {schedule}");
            println!("first tick {next}");
            // Said because it is the difference between this and `cron`, and because somebody
            // who has just typed a period on a laptop is entitled to know that closing it
            // changes nothing.
            println!("            it is the fleet's now: whichever device is available fires it");
            // The three notes `offload when` prints, worded for a tick. They were missing here
            // entirely: `offload every 1m --task nosuch` was accepted in silence and then
            // refused at every tick for ever, with `offload schedules` saying `nothing fired
            // from here yet` — the sentence it also uses for a schedule that has not reached
            // its first tick. Every argument for warning a rule is stronger here, because this
            // command's own help promises the schedule outlives the device.
            if let Some(note) = &audience {
                println!("  {note}");
            }
            match (reach, notices) {
                (Reach::Somebody, offload_core::Notices::Problems) => println!(
                    "  Quiet while it works: you will hear about a failure or a question, and \
                     not about a tick that went fine."
                ),
                (Reach::Somebody, offload_core::Notices::Everything) => println!(
                    "  Every tick will be reported, including the ones that go fine — which on \
                     this period is a notification every {}.",
                    offload_core::Millis(spec.every_ms)
                ),
                (Reach::NobodyWanted, kinds) => println!(
                    "  Set to report {kinds} — kept on the schedule, and carried to nobody \
                     while the audience is none."
                ),
                (Reach::NoRoute, kinds) => {
                    println!(
                        "  Set to report {kinds}, and there is nothing in this fleet to report \
                         it to,"
                    );
                    println!(
                        "  so it will tick silently — including when it fails. `offload sinks`"
                    );
                    println!("  says how to add a route.");
                }
            }
            // The precondition that is a *refusal* on `offload run --task`. Last, and it is the
            // one that makes the difference between a schedule and a clock that does nothing.
            if let Some(note) = &resources {
                println!("  {note}");
                // The half a rule does not need saying, because a rule's refused firing is
                // counted under DROPPED in `offload rules` and this is counted by nothing:
                // `offload schedules` prints `nothing fired from here yet`, which is also what
                // it says about a schedule that has not reached its first tick.
                println!("  Nothing counts a refused tick, so `offload schedules` will go on");
                println!("  saying nothing has fired from here.");
            }
            Ok(())
        }
        Response::Error { message } => bail!(message),
        _ => bail!("unexpected response to every"),
    }
}

/// `offload schedules` — what runs on a clock.
pub async fn schedules(socket: &Path) -> Result<()> {
    let responses = client::request(socket, &Request::Schedules).await?;
    let Response::Schedules { schedules } = client::expect_one(responses)? else {
        bail!("unexpected response to schedules");
    };
    if schedules.is_empty() {
        println!("nothing runs on a clock here (`offload every 15m --task <service>`)");
        return Ok(());
    }
    for s in &schedules {
        // The id first and in full: it is what `offload unschedule` takes, and eight bytes is
        // short enough to type (`ScheduleId`, and the reason it is not abbreviated).
        println!("{}  {}", s.id, s.every);
        println!("    {} {}", s.kind, s.work);
        if !s.note.is_empty() {
            println!("    note      {}", s.note);
        }
        // Whose it is, and whether *this* node is the one that fires it. Two facts and not one:
        // a schedule created on the desktop is fired here while the desktop is away, which is
        // the whole point and is otherwise invisible.
        println!(
            "    home      {}{}",
            s.home,
            steward_note(s.removed, &s.steward)
        );
        if s.removed {
            // Kept deliberately: the tombstone is what travels (ADR-0056 §5). Said plainly, so
            // nobody reads a removed row as a schedule that is still going to fire.
            println!("    removed   it fires nothing; the record is the removal, and it gossips");
        } else if s.steward == Steward::Nobody {
            // **Not a countdown.** A tick nothing will serve is not a tick, and printing the
            // arithmetic under a row nobody fires is the report promising work that is not
            // coming. Say the state and the way out of it instead.
            println!("    next      nothing fires it: no node here can see its home, so the");
            println!("              successor rule picks nobody. It starts again on its own when");
            println!("              that device comes back — and if it is gone for good,");
            println!("              `offload unschedule` from any node, then make a new one here.");
        } else {
            println!(
                "    next      {}{}",
                s.next,
                match &s.last_fired {
                    Some(ago) => format!("  ·  last fired here {ago}"),
                    // Not a fault, and the commonest case on a node that is not the steward.
                    None => "  ·  nothing fired from here yet".to_string(),
                }
            );
        }
    }
    Ok(())
}

/// What to say about a schedule's `home`, beside whose it is.
///
/// Extracted from the row it prints so the one rule in it is checked rather than hoped: a
/// **tombstone has no steward worth naming**. The field is still computed for a removed
/// schedule and still answers `Here`, so this line said `fired from here right now` directly
/// above the `removed` line saying it fires nothing — one screen holding both answers, which is
/// the defect session sixty-nine fixed one field over on this very report, arriving the second
/// time through `removed` instead of through the steward.
fn steward_note(removed: bool, steward: &Steward) -> String {
    if removed {
        // Whose it was, and nothing about who fires it, because nobody does.
        return String::new();
    }
    match steward {
        Steward::Here => "  ·  fired from here right now".to_string(),
        // Named, because the steward is not always the home: a home that has gone hands the
        // tick to the lowest-id node that is alive, and "elsewhere" then sends somebody to the
        // wrong machine's logs.
        Steward::Elsewhere { node } => format!("  ·  fired by {node}"),
        Steward::Nobody => "  ·  nothing here can see its home".to_string(),
    }
}

/// `offload unschedule <id>` — take one away, everywhere.
pub async fn unschedule(socket: &Path, schedule: &str) -> Result<()> {
    let responses = client::request(
        socket,
        &Request::Unschedule {
            schedule: schedule.to_string(),
        },
    )
    .await?;
    match client::expect_one(responses)? {
        // Already gone, and said so rather than claimed again. Not an error: the schedule is
        // removed, which is what was typed, and exiting non-zero on "it is already the way you
        // asked for" makes a retry look like a failure. What it is not is a removal — this
        // printed *"removed <id>"* and the whole paragraph about what had just travelled, twice
        // running, on a call that wrote nothing and published nothing.
        Response::Unscheduled {
            schedule,
            already: Some(ago),
        } => {
            println!("{schedule} was already removed {ago}");
            println!("        nothing more to do, and nothing new to tell the fleet — the");
            println!("        tombstone it has is the one every node has to agree on");
            Ok(())
        }
        Response::Unscheduled { schedule, .. } => {
            println!("removed {schedule}");
            // The mechanism, in one line, because it is the surprising part: the row stays so
            // that the removal can travel, and a peer that has not heard yet may fire one more
            // occurrence.
            println!("        the removal is what the fleet is told, so it reaches nodes that");
            println!("        are away — the record stays behind to carry it");
            Ok(())
        }
        Response::Error { message } => bail!(message),
        _ => bail!("unexpected response to unschedule"),
    }
}

/// `offload files <run> [path]` (ADR-0075): a directory's entries, or a file's text as it is.
pub async fn files(socket: &Path, run: &str, path: &str) -> Result<()> {
    use offload_node::api::{FilesContent, FilesSource};
    let responses = client::request(
        socket,
        &Request::Files {
            run: run.to_string(),
            path: path.to_string(),
        },
    )
    .await?;
    let Response::Files { view } = client::expect_one(responses)? else {
        bail!("unexpected response to files");
    };
    let from = match view.source {
        FilesSource::Checkout => "its checkout",
        FilesSource::Branch => "its branch (the checkout is gone, so committed files only)",
    };
    match view.content {
        FilesContent::Listing { entries, truncated } => {
            println!("/{}  on {}, from {from}", view.path, view.node);
            for entry in &entries {
                if entry.dir {
                    println!("  {}/", entry.name);
                } else {
                    println!("  {:<40} {}", entry.name, human_bytes(entry.bytes));
                }
            }
            if entries.is_empty() {
                println!("  (empty)");
            }
            if truncated {
                println!("  … and more, not listed");
            }
        }
        FilesContent::Text {
            text,
            bytes,
            truncated,
        } => {
            // The file as it is, on stdout, so it can be piped; where it came from on stderr.
            eprintln!(
                "{}  on {}, from {from}, {}",
                view.path,
                view.node,
                human_bytes(bytes)
            );
            print!("{text}");
            if truncated {
                eprintln!("\n… cut: the whole file is {}", human_bytes(bytes));
            }
        }
        FilesContent::Binary { bytes } => {
            println!(
                "{} is not text ({}), so it is not shown",
                view.path,
                human_bytes(bytes)
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// ADR-0080: one row per model, with every device offering it, in the order the first device
    /// lists it; and a device with an agent that lists nothing is named, since it is not the same
    /// as a device with no agent.
    #[test]
    fn the_model_listing_names_who_offers_each_and_who_offers_none() {
        let node = |name: &str, agent: bool, models: &[&str]| offload_node::mesh::NodeSummary {
            id: offload_core::NodeId::from_bytes([name.bytes().next().unwrap_or(0); 32]),
            name: name.into(),
            status: offload_core::NodeStatus::Alive,
            device_class: "Desktop".into(),
            cpu_cores: 8,
            memory_mb: 8192,
            agent: agent.then(|| "claude-code 2.1.283".into()),
            running: 0,
            last_heard_ms: Some(0),
            absences: 0,
            programs: Vec::new(),
            may_host: Some(true),
            models: models
                .iter()
                .map(|m| offload_core::Model::named(m))
                .collect(),
        };
        let out = super::render_models(&[
            node("laptop", true, &["default", "opus"]),
            node("macmini", true, &["default", "haiku"]),
            node("s22", false, &[]),
            node("tablet", true, &[]),
        ]);
        assert_eq!(
            out,
            "MODEL    NAME                      OFFERED BY\n\
             default  default                   laptop, macmini\n\
             opus     opus                      laptop\n\
             haiku    haiku                     macmini\n\
             \n\
             tablet has an agent that lists no models: logged out, or its list could not be read.\n\
             Its own log says which; `offload models --refresh` asks again.\n"
        );
    }

    /// The timestamp says which clock it is on, and it is not the reader's.
    ///
    /// `when` computes civil-from-days straight off the unix millisecond, which is UTC, and the
    /// column said nothing about that — under a doc comment claiming it was local. Measured on a
    /// CEST machine: a run submitted at 15:20:19 local read `2026-09-12 13:20` in `offload audit`.
    /// Both callers are logs somebody opens *afterwards* to place an event in time, so an unmarked
    /// wall-clock time silently answers "nothing happened then" to anybody asking about the right
    /// two hours.
    ///
    /// A known answer rather than a round trip: 1_700_000_000_000 ms is 2023-11-14 22:13:20 UTC,
    /// which is checkable by hand and stays checkable if somebody changes the arithmetic.
    #[test]
    fn a_log_timestamp_names_the_clock_it_is_on() {
        assert_eq!(super::when(1_700_000_000_000), "2023-11-14 22:13 UTC");
        // The epoch itself, because the civil-from-days constants are the easy thing to get
        // wrong and this is the one date everybody can check.
        assert_eq!(super::when(0), "1970-01-01 00:00 UTC");
    }

    fn route(delivered: u64, waiting: u64, gave_up: u64) -> offload_node::deliver::SinkReport {
        offload_node::deliver::SinkReport {
            id: "phone".into(),
            service: "push".into(),
            description: String::new(),
            command: "/bin/true".into(),
            unusable: None,
            delivered,
            waiting,
            gave_up,
            last_error: None,
        }
    }

    /// A route that gave up on a message has been used, whatever the delivered count says.
    ///
    /// `sink_state` matched on two of the three counters and let a guard below ask about the
    /// third, so `(0, 0, 1)` — nothing delivered, nothing queued, one message abandoned — took
    /// the `never used` arm, with `DROPPED 1` in the column beside it and `last failure: gave
    /// up: …` printed underneath. Measured on a daemon with a sink whose script exits 7.
    #[test]
    fn a_route_that_gave_up_on_something_has_not_never_been_used() {
        assert_eq!(
            super::sink_state(&route(0, 0, 1), false),
            "ok now — 1 was given up on earlier"
        );
        // …and the number agrees with itself. Both tables said `1 were`.
        assert_eq!(
            super::sink_state(&route(0, 0, 3), false),
            "ok now — 3 were given up on earlier"
        );

        // The arm it was shadowing is still right for the route it was written for.
        assert_eq!(
            super::sink_state(&route(0, 0, 0), false),
            "usable, never used — try `offload sinks --test`"
        );
        // And the ones that were already right.
        assert_eq!(super::sink_state(&route(4, 0, 0), false), "ok");
        assert_eq!(super::sink_state(&route(0, 2, 0), false), "2 still to send");
        // A test is not a delivery, so it must not be described by counters it did not move.
        assert_eq!(super::sink_state(&route(0, 0, 1), true), "test delivered");
    }

    /// A teardown line must not report only the half that survived.
    #[test]
    fn a_teardown_that_lost_something_says_so() {
        // The old line was this string unconditionally, and it is still right for the ordinary
        // case — a completed run whose agent committed everything.
        assert_eq!(
            super::removed_line(None),
            "worktree removed (the run's branch and commits are kept)"
        );

        // And wrong for the run this command is most dangerous on. Nothing else anywhere holds
        // these: not the branch, not the checkpoint, not the bundle, and `git fsck` finds
        // nothing because none of it was ever an object.
        let said = super::removed_line(Some("1 modified, 1 new"));
        assert!(
            said.contains("1 modified, 1 new") && said.contains("that is gone"),
            "the sentence kept its reassurance and dropped the loss: {said}"
        );
        assert!(
            said.contains("branch and commits are kept"),
            "the reassuring half is still true and still has to be said: {said}"
        );
    }

    /// An agent line must name what is known and say what is not, never leave a gap.
    #[test]
    fn an_agent_that_reported_no_model_does_not_render_as_a_gap() {
        // Rendered as blanks these produced `agent claude-code , model` — two empty slots where
        // the facts should be, which reads as a broken report rather than as a missing fact. The
        // same sentence `TaskStarted` was split out to avoid, from the agent side.
        assert_eq!(
            offload_node::render::agent_line("claude-opus-5", "2.1.268"),
            "claude-code 2.1.268, model claude-opus-5"
        );
        assert_eq!(
            offload_node::render::agent_line("", ""),
            "claude-code (version not reported), model not reported"
        );
        assert_eq!(
            offload_node::render::agent_line("claude-opus-5", ""),
            "claude-code (version not reported), model claude-opus-5"
        );
        assert_eq!(
            offload_node::render::agent_line("", "2.1.268"),
            "claude-code 2.1.268, model not reported"
        );
    }

    /// The number an agent budgets a 6.5 GB directory against, so it has to read as a size.
    #[test]
    fn the_archive_budget_reads_as_a_size_somebody_can_pack_to() {
        // The small end stays exact on purpose: a walk runs with the cap lowered, and
        // "0.0 MiB" would be useless in exactly the staging that needs it most.
        assert_eq!(super::human_bytes(512 * 1024 * 1024), "512.0 MiB");
        assert_eq!(super::human_bytes(131_072), "131072 bytes");
        assert_eq!(super::human_bytes(0), "0 bytes");
    }

    /// The one line in the product that picks between two deadlines.
    #[test]
    fn a_blocked_run_is_told_whichever_clock_runs_out_first() {
        use offload_core::Millis;
        let m = |ms| Millis(ms);

        // The drain has the longer run: the question decides it, the agent then takes its turn,
        // and the run is handed over normally. One line, unchanged since ADR-0035.
        let far = blocked_horizon(m(60_000), m(600_000));
        assert_eq!(far.len(), 1);
        assert!(far[0].contains("or 1m0s from now"), "{far:?}");
        assert!(!far[0].contains("drain"), "nothing to warn about: {far:?}");

        // The drain gives up first, which decides nothing and leaves the run mid-turn. This is
        // the case the old sentence was confidently wrong about — it said `4m50s` to somebody
        // with ten seconds.
        let near = blocked_horizon(m(290_000), m(10_000));
        assert_eq!(
            near.len(),
            2,
            "the second line is the consequence: {near:?}"
        );
        assert!(near[0].contains("stops waiting in 10.0s"), "{near:?}");
        assert!(near[0].contains("sooner"), "{near:?}");
        assert!(
            near[1].contains("left mid-turn") && near[1].contains("4m50s"),
            "and the question's own clock is still said, because it is still answerable: {near:?}"
        );

        // Equal is not "sooner": at the same instant the question decides it, which is the
        // better of the two outcomes, so it is not the one to warn about.
        assert_eq!(blocked_horizon(m(5_000), m(5_000)).len(), 1);

        // An older daemon does not send the field. Half a horizon beats none.
        assert_eq!(
            blocked_horizon(m(5_000), m(0)),
            blocked_horizon(m(5_000), m(600_000))
        );
    }

    /// `offload explain`'s last line must not name a line it did not print.
    ///
    /// The pre-fix behaviour is this function returning its first arm unconditionally, and each
    /// of the two assertions below fails on it. Measured on a live daemon: a run stopped
    /// mid-tool-call read `state running — started 9.0s ago` and then `\`held back\` above is
    /// why it has not begun`, with no `held back` line anywhere above it.
    #[test]
    fn the_last_line_of_an_explanation_points_at_a_line_it_printed() {
        // A run in `assigned` that a gate is holding back — the only state `held_back` is
        // computed for, and the case the sentence was written against. Unchanged.
        assert!(taken_not_unplaced(true, false).contains("`held back`"));

        // Running and stopped on a question. Two ways to be wrong and the fix has to avoid
        // both: pointing at `held back`, and saying a run that began has not begun.
        let stopped = taken_not_unplaced(false, true);
        assert!(stopped.contains("`waiting`"), "{stopped}");
        assert!(!stopped.contains("held back"), "{stopped}");
        assert!(!stopped.contains("has not begun"), "{stopped}");

        // Running, nothing to point at but the state. Must still not claim either.
        let plain = taken_not_unplaced(false, false);
        assert!(plain.contains("`state`"), "{plain}");
        assert!(!plain.contains("held back"), "{plain}");
        assert!(!plain.contains("has not begun"), "{plain}");

        // The clause that is true in every case, and the reason the sentence exists at all.
        for line in [taken_not_unplaced(true, false), stopped, plain] {
            assert!(line.starts_with("It is taken, not unplaced — "), "{line}");
        }
    }
    use super::*;

    #[test]
    fn permission_accepts_the_spellings_people_type() {
        for (input, expected) in [
            ("full", PermissionMode::Full),
            ("FULL", PermissionMode::Full),
            ("bypass", PermissionMode::Full),
            ("accept-edits", PermissionMode::AcceptEdits),
            ("acceptEdits", PermissionMode::AcceptEdits),
            ("accept_edits", PermissionMode::AcceptEdits),
            ("edits", PermissionMode::AcceptEdits),
            ("ask", PermissionMode::Ask),
        ] {
            assert_eq!(parse_permission(input), Ok(expected), "input {input}");
        }
    }

    #[test]
    fn an_unknown_permission_mode_lists_the_valid_ones() {
        let err = parse_permission("yolo").expect_err("should reject");
        assert!(err.contains("accept-edits"), "{err}");
        assert!(err.contains("full"), "{err}");
    }

    #[test]
    fn displayed_ids_are_long_enough_to_distinguish_uuidv7_runs() {
        // Two runs a few seconds apart share their first eight hex characters, which is why
        // twelve is the length and why this is checked at all.
        let a = "019f9a2b7e0b73b2bdf724c7c18e065d";
        let b = "019f9a2b344d72f381d06c0c846023bb";
        assert_eq!(a[..8], b[..8], "eight characters really do collide");
        assert_ne!(short_id(a), short_id(b));
    }

    /// Every listing that prints a run id has to widen, not only the one somebody measured.
    #[test]
    fn the_audit_log_tells_two_runs_apart_because_that_is_what_it_is_for() {
        // `offload audit` used a fixed twelve while `offload ps` computed its width, so the same
        // pair of occurrences that made `ps` widen made the audit log *merge*: four rows under
        // one id reading `granted … accepted … granted … accepted`, every one at epoch 1.
        // Measured on one daemon with `every 1m` beside `every 2m`.
        //
        // That is worse than a duplicate-looking listing, which is what `ps` showed. It is the
        // signature of an arbiter spending a token twice — the single thing this log exists to
        // let somebody detect — and it is a *rendering* of two ordinary runs.
        let a = "01a090304e407095b074047456d6a7bc";
        let b = "01a090304e40752fbb9fd4afa00ae5cd";
        assert_eq!(a[..12], b[..12], "twelve characters name the tick");
        let width = id_width([a, b].into_iter());
        assert_eq!(width, 14);
        assert_ne!(at_most(a, width), at_most(b, width));
        // And the ordinary audit log, whose rows are one run repeated, stays at twelve: a
        // listing may hold several entries about one run and there is nothing to tell apart.
        assert_eq!(id_width([a, a, a].into_iter()), SHORT_ID);
    }

    /// The width a listing needs, which twelve is not when the ids were *derived*.
    #[test]
    fn a_listing_widens_only_as_far_as_its_own_rows_force_it() {
        // Two occurrences of one tick, from two schedules that share a boundary. Real ids,
        // measured on a daemon running `every 1m` beside `every 2m`: the first six bytes are
        // the tick, which is exactly what `RunId::short` displays, and what follows is a digest
        // of the schedule rather than randomness — so this is every shared boundary, for ever,
        // and not a same-millisecond coincidence.
        let a = "01a08f52bf80790aa2b9ddbb2a50abbd";
        let b = "01a08f52bf80744b8e8aff3d897b37d6";
        assert_eq!(
            a[..12],
            b[..12],
            "twelve characters name the tick, not the run"
        );
        assert_eq!(id_width([a, b].into_iter()), 14);
        assert_ne!(
            at_most(a, 14),
            at_most(b, 14),
            "and fourteen tells them apart"
        );

        // The ordinary listing is untouched: two UUIDv7 runs seconds apart differ well inside
        // twelve, so the column does not grow and `ps` looks exactly as it did.
        let c = "019f9a2b7e0b73b2bdf724c7c18e065d";
        let d = "019f9a2b344d72f381d06c0c846023bb";
        assert_eq!(id_width([c, d].into_iter()), SHORT_ID);
        // One row, and no rows, are both twelve.
        assert_eq!(id_width([c].into_iter()), SHORT_ID);
        assert_eq!(id_width(std::iter::empty()), SHORT_ID);
        // The same run twice — a listing may hold one row per node — must not drive the width
        // to the full id: there is nothing to tell apart.
        assert_eq!(id_width([c, c].into_iter()), SHORT_ID);

        // Whole bytes, because half a byte names nothing. These two agree on thirteen
        // characters and part on the fourteenth, so thirteen would separate them and fourteen
        // is what is shown.
        let e = "01a08f52bf807900000000000000000a";
        let f = "01a08f52bf80780000000000000000b0";
        assert_eq!(e[..13], f[..13]);
        assert_eq!(id_width([e, f].into_iter()), 14);

        // …and one that needs a byte further in gets it: agreeing through the fourteenth
        // character forces sixteen rather than fifteen.
        let g = "01a08f52bf8079000000000000000000";
        let h = "01a08f52bf80791000000000000000b0";
        assert_eq!(g[..14], h[..14]);
        assert_eq!(id_width([g, h].into_iter()), 16);
    }

    #[test]
    fn a_removed_schedule_is_not_told_who_fires_it() {
        use super::Steward;
        // The three live answers are unchanged and each names a machine or says it cannot.
        assert!(steward_note(false, &Steward::Here).contains("fired from here right now"));
        assert!(steward_note(
            false,
            &Steward::Elsewhere {
                node: "bravo".into()
            }
        )
        .contains("fired by bravo"));
        assert!(steward_note(false, &Steward::Nobody).contains("nothing here can see its home"));

        // And a tombstone gets none of them, whichever the steward came out as. `Here` is the
        // one measured on a daemon: `offload unschedule` then `offload schedules` printed
        // `fired from here right now` one line above `it fires nothing`.
        for steward in [
            Steward::Here,
            Steward::Elsewhere {
                node: "bravo".into(),
            },
            Steward::Nobody,
        ] {
            assert_eq!(
                steward_note(true, &steward),
                "",
                "a removed schedule fires nowhere, so no line may say where"
            );
        }
    }

    #[test]
    fn finished_states_match_the_run_state_machine() {
        // These strings come from RunState::name(); drift means `ps` silently shows
        // finished runs as active forever.
        assert!(is_finished("completed"));
        assert!(is_finished("failed"));
        assert!(is_finished("cancelled"));
        assert!(!is_finished("running"));
        assert!(!is_finished("orphaned"));
        assert!(!is_finished("pending"));
    }
    #[test]
    fn ask_parses_the_three_things_somebody_can_mean() {
        use offload_core::AskPolicy;
        // The flag's absence, which clap resolves through `default_value`.
        assert_eq!(parse_ask("0"), Ok(AskPolicy::Never));
        // The flag on its own, through `default_missing_value`.
        assert_eq!(
            parse_ask(&offload_core::DEFAULT_ASK_BUDGET.to_string()),
            Ok(AskPolicy::UpTo {
                questions: offload_core::DEFAULT_ASK_BUDGET
            })
        );
        assert_eq!(parse_ask("5"), Ok(AskPolicy::UpTo { questions: 5 }));
        // And a prompt typed where a number was expected says so, rather than being counted.
        assert!(parse_ask("add tests for the parser").is_err());
    }

    #[test]
    fn max_turns_refuses_the_zero_that_ask_accepts() {
        assert_eq!(parse_max_turns("3").map(std::num::NonZeroU32::get), Ok(3));

        // The difference from `--ask 0` one test up, which is a real setting: there, zero is
        // "never ask me", the flag's own absence said out loud. Here it would be a run that
        // takes no turns — which is not something anybody means to submit, and reads as a limit
        // being set. Two spellings of "don't", neither of them this one.
        let err = parse_max_turns("0").expect_err("zero is not a limit anybody means");
        assert!(
            err.contains("no limit"),
            "and the error has to name the thing they probably wanted: {err}"
        );

        // A prompt typed where a number was expected says so rather than being counted.
        assert!(parse_max_turns("add tests for the parser").is_err());
    }
}
