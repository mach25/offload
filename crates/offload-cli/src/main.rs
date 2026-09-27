//! The `offload` CLI.
//!
//! Phase 0 works entirely against the local machine — there is no daemon to talk to yet.
//! `probe`, `match` and `policy` answer the questions that determine whether the rest of
//! the system will behave: what am I, would I qualify for this run, and would I accept it.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod client;
mod commands;
mod constraint_expr;
mod fleet;
mod when;

use anyhow::Result;
use clap::{Parser, Subcommand};
use offload_core::{Capabilities, Grant, PermissionMode, WorkPolicy};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "offload",
    about = "Orchestrate coding agents across your devices",
    version
)]
struct Cli {
    /// Control socket of the daemon to talk to. Defaults to $OFFLOAD_SOCKET, else
    /// <state dir>/offloadd.sock.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

/// Where a run should go: `offload run`'s placement flags (ADR-0063).
#[derive(Debug, Clone, clap::Args)]
struct PlacingArgs {
    /// What a node must have to take this, in `offload match`'s syntax: `cores>=16, os=linux`.
    ///
    /// A requirement: a node that does not satisfy it does not bid, and if none does the
    /// submission is refused while you are here. `here` and `node=<name>` pin the run to one
    /// machine — which is rarely what you want; `--prefer` is the soft form.
    #[arg(long, value_name = "EXPR", value_parser = commands::parse_wanted)]
    require: Option<offload_core::Wanted>,
    /// Where you would rather it ran: `here`, `node=desktop`, `os=macos`, `class=desktop`.
    ///
    /// Scored, never filtered. A node that matches is worth more to the fleet than one that
    /// does not — enough to beat any single reason to go elsewhere (a bigger machine, a busier
    /// one, a battery) but not two. If the preferred node is off or cannot take it, the run goes
    /// wherever it would have gone anyway, and `offload run` says so. `here` means the daemon
    /// you are typing at.
    #[arg(long, value_name = "EXPR", value_parser = commands::parse_wanted)]
    prefer: Option<offload_core::Wanted>,
    /// Hold out for the preferred node for this long, from now: 8h, 30m.
    ///
    /// Until then `--prefer` is a requirement, and after it a preference again — so "the
    /// desktop, when I get home" is `--prefer node=desktop --hold 8h`. Implies `--queue`, since
    /// the node you are waiting for may be asleep now. A duration, like `--deadline`, and it may
    /// not end after the deadline.
    #[arg(long, value_name = "DURATION", value_parser = when::parse_deadline, requires = "prefer")]
    hold: Option<u64>,
}

impl PlacingArgs {
    fn into_placing(self) -> commands::Placing {
        commands::Placing {
            require: self.require.unwrap_or_default(),
            prefer: self.prefer.unwrap_or_default(),
            hold_until: self.hold,
        }
    }
}

/// `offload when`'s placement flags: `offload run`'s without `--hold`, which ends at an instant
/// and a rule fires for ever.
#[derive(Debug, Clone, clap::Args)]
struct RulePlacingArgs {
    /// What a node must have to take a fired run. See `offload run --require`.
    #[arg(long, value_name = "EXPR", value_parser = commands::parse_wanted)]
    require: Option<offload_core::Wanted>,
    /// Where a fired run would rather go. `here` means **the machine you are typing this at**,
    /// resolved now — not whichever node fires the rule later.
    #[arg(long, value_name = "EXPR", value_parser = commands::parse_wanted)]
    prefer: Option<offload_core::Wanted>,
}

impl RulePlacingArgs {
    fn into_placing(self) -> commands::Placing {
        commands::Placing {
            require: self.require.unwrap_or_default(),
            prefer: self.prefer.unwrap_or_default(),
            hold_until: None,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show what this device is and has.
    Probe {
        /// The daemon's TOML config, so the agent asked about is the one `offloadd` would spawn.
        ///
        /// Without it the answer is `claude` on `PATH`, which is what an ordinary install has and
        /// what the default config names. It matters the moment an owner sets `agent.binary`:
        /// the daemon probes what it will run, and a report that asked a different program is
        /// the confident wrong answer this command exists to prevent.
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Check whether this device satisfies a constraint, and if not, why not.
    Match {
        /// e.g. "agent=claude-code, cores>=8, mem>=16G"
        expr: String,
        /// The daemon's TOML config. `agent=claude-code` is a clause about the binary the daemon
        /// would spawn, so this is the same flag for the same reason as on `probe`.
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Show every clause, not only the failures.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Show the work policy this device would adopt, and whether it would take work now.
    Policy {
        /// The daemon's TOML config, so this answers about the policy `offloadd` actually uses.
        ///
        /// Without it the answer is the *class default*, which is what this command used to
        /// report unconditionally — a confident answer about a policy the daemon does not have
        /// the moment somebody writes a `[policy]` block. The daemon takes the path the same
        /// way (`offloadd --config`) and there is no default location to guess, which is why
        /// this is a flag rather than a lookup.
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Found a fleet on this device and print its passphrase, once.
    ///
    /// The passphrase *is* the fleet (ADR-0012): it is the only thing that can enrol a
    /// device or revoke one, it is stored nowhere, and losing it means re-founding.
    Init {
        /// State directory to found the fleet in. Defaults to $OFFLOAD_STATE_DIR.
        #[arg(long)]
        state_dir: Option<PathBuf>,
        /// Name for this device in `offload fleet`. Defaults to the hostname.
        #[arg(long)]
        name: Option<String>,
        /// Approve with the key the host app holds in secure hardware (ADR-0069 §4), from the
        /// start. Every invitation then needs a person to confirm on this device's screen.
        #[arg(long)]
        hardware_key: bool,
    },

    /// Enrol this device into an existing fleet.
    Join {
        /// Enrol directly with the fleet passphrase, rather than asking an approver.
        ///
        /// The recovery path: founding, and the day every approver is gone. It needs
        /// nothing else to be awake, and grants correspondingly little.
        #[arg(long)]
        passphrase: bool,
        /// Take up an invitation `offload invite` printed on a device that is already a
        /// member. Public material — it names this device's key and nobody else can use it.
        #[arg(long, conflicts_with = "passphrase")]
        token: Option<String>,
        /// State directory to enrol. Defaults to $OFFLOAD_STATE_DIR.
        #[arg(long)]
        state_dir: Option<PathBuf>,
        /// Name for this device in `offload fleet`, with `--passphrase`. Defaults to the
        /// hostname. An invitation already carries the name its issuer gave, so not with `--token`.
        #[arg(long, conflicts_with = "token")]
        name: Option<String>,
    },

    /// Print this device's node id, creating its identity if it has none.
    ///
    /// What `offload invite` needs to name it. Runs before this device belongs to anything.
    Id {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// Issue a certificate for another device, to be carried to it.
    ///
    /// The two-step enrolment for a machine where confirming interactively is awkward: run
    /// `offload id` there, `offload invite <that id>` here, and `offload join --token` back
    /// there. No passphrase unless a grant beyond the door is asked for.
    Invite {
        /// The joining device's node id, from `offload id` on that machine.
        node: String,
        /// What to call it in `offload nodes`. Defaults to a short form of its id.
        #[arg(long)]
        name: Option<String>,
        /// A grant beyond `submit` and `deliver`. Needs the fleet passphrase.
        #[arg(long = "grant", value_parser = fleet::parse_grant)]
        grants: Vec<Grant>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// Check that a passphrase is this fleet's. Grants nothing, changes nothing.
    Verify {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// Add a grant to this node's certificate. Prompts for the fleet passphrase.
    Grant {
        /// submit | deliver | host-runs | approve
        #[arg(value_parser = fleet::parse_grant)]
        grant: Grant,
        /// With `approve`: approve with the key the host app holds in secure hardware
        /// (ADR-0069 §4) rather than this node's own key. Every invitation then needs a person
        /// to confirm on this device's screen.
        #[arg(long)]
        hardware_key: bool,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// Revoke a member. Prompts for the fleet passphrase.
    Revoke {
        /// The node id to revoke, in full.
        node: String,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// Re-found the fleet under a new passphrase, optionally without some device.
    ///
    /// The convergent revocation (ADR-0012). Ordinary revocation has to reach every node;
    /// this does not have to reach anybody — the evicted device is simply not in the new
    /// fleet. Everything this machine owns survives, and every other device needs
    /// re-enrolling with the invitation this prints.
    Rekey {
        /// A device to leave out. Repeatable.
        #[arg(long = "evict")]
        evict: Vec<String>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// The bytes a certificate's signer signs, for the host app's hardware key (ADR-0069 §4).
    #[command(hide = true)]
    SigningBytes,

    /// Start another year for members whose approval is running out (ADR-0069 §3).
    ///
    /// Run on an approver. With no names, lists the memberships this node has met whose approval
    /// runs out within 30 days. Named devices, or `--due` for that whole list, are re-approved the
    /// next time each asks — nothing has to be done on the devices themselves.
    Reapprove {
        /// Devices to re-approve, by node id or a unique prefix of one — the list prints them.
        nodes: Vec<String>,
        /// Every device on the list above, except this node.
        #[arg(long, conflicts_with = "nodes")]
        due: bool,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// Show this node's membership: fleet, grants, certificate, revocations.
    Fleet {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },

    /// Ask the local daemon what it is and what it's doing.
    Status,

    /// Submit an agent run.
    Run {
        /// Repository to work in. Defaults to the current directory. Not used by --task.
        #[arg(long)]
        repo: Option<String>,
        /// A **tar archive** to use as the workspace, instead of a repository (ADR-0061).
        ///
        /// For the ordinary directory that is not a git repository and has no origin any node
        /// could clone. The archive is stored in this fleet as content-addressed bytes, and the
        /// node that takes the run unpacks it and makes it a repository — so from turn one the
        /// run has a base commit and a branch, and migration, checkpointing and resume work
        /// exactly as they do for a cloned repo.
        ///
        /// **Choose what goes in it.** There is a size limit and `offload status` prints it, so
        /// this is a selection rather than a snapshot of a directory: the files the job needs,
        /// not everything beside them. What you leave out is not there, and a run that needs it
        /// fails on the node that took it. (The number lives in one place and is read from the
        /// daemon — written here it would drift from the constant that enforces it.)
        ///
        /// Results come back as commits on the run's branch, never as files in your directory.
        #[arg(long, conflicts_with_all = ["repo", "task", "git_ref"])]
        archive: Option<PathBuf>,
        /// Start in an **empty** workspace instead of a repository (ADR-0072).
        ///
        /// For work that needs no files to begin with. The node that takes the run makes an empty
        /// repository, the same one on every node, so the run still has a branch, checkpoints and
        /// migrates. What it writes comes back as commits on that branch.
        #[arg(long, conflicts_with_all = ["repo", "archive", "task", "git_ref"])]
        scratch: bool,
        /// What the agent should do. Omitted for --task, which runs a program instead.
        #[arg(required_unless_present = "task")]
        prompt: Option<String>,
        /// Run a program this fleet's owner nominated, instead of an agent (ADR-0019).
        ///
        /// The cheap middle tier: no model, no prompt, no workspace and no tokens. You name a
        /// **service** — `offload run --task webhook` — and the node that takes it decides
        /// what that runs, from its own `[[tasks]]` config. You cannot name a command, which
        /// is the point: the outbound access is the owner of that machine's grant, not yours.
        ///
        /// `offload triggers` and `offload status` list what a node offers. Anything after the
        /// service is passed to the program, after the arguments its owner already fixed.
        #[arg(long, value_parser = commands::parse_service, conflicts_with_all = [
            "prompt", "model", "permission", "git_ref", "allow", "ask", "max_turns", "resources",
        ])]
        task: Option<offload_core::Service>,
        /// Arguments for --task, after the ones its owner fixed.
        #[arg(long = "arg", requires = "task")]
        args: Vec<String>,
        /// Model override, e.g. claude-opus-5.
        #[arg(long)]
        model: Option<String>,
        /// ask | accept-edits | full. Defaults to the daemon's setting (accept-edits).
        #[arg(long, value_parser = commands::parse_permission)]
        permission: Option<PermissionMode>,
        /// Branch or commit to start from. Defaults to the repo's default branch.
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// Extra tool grant, repeatable. e.g. --allow "Bash(cargo test:*)"
        ///
        /// Layered onto the node's baseline and the repo's .offload.toml. This is how a
        /// run executes its own tests without the unrestricted access --permission full
        /// grants.
        #[arg(long)]
        allow: Vec<String>,
        /// Stream the run's output instead of returning immediately.
        #[arg(short, long)]
        follow: bool,
        /// Leave the run pending if nobody will take it now, instead of refusing.
        ///
        /// The deliberate case: you know the desktop is off and will be on in the morning.
        /// Without it a submission is accepted by a node while you are still here, or refused
        /// to your face with every node's reason (ADR-0014).
        #[arg(long)]
        queue: bool,
        /// When this needs to be done, from now: 45m, 2h, 1h30m, 9h.
        ///
        /// Not a promise and not a priority (ADR-0013). It says how long a wait may be, so a
        /// run gets more urgent by itself as the time approaches — which decides how long the
        /// fleet waits for a machine that has gone quiet, and how patiently a failure is
        /// handled. Unset means "as soon as you can", which is why a run with no deadline is
        /// not a run with no hurry.
        #[arg(long, value_parser = when::parse_deadline)]
        deadline: Option<u64>,
        /// What this is expected to cost the device that takes it: light, normal or heavy.
        ///
        /// A scheduling hint, never a requirement — hardware a run cannot work without goes in
        /// its constraint, which refuses loudly instead of landing somewhere slow. `heavy` is
        /// for work you already know is expensive (a full build, a long suite): it keeps a
        /// machine to itself rather than sharing it with two other agents. `light` is the
        /// opposite — several fit in the space of one ordinary run.
        #[arg(long, default_value_t = offload_core::Demand::Normal)]
        demand: offload_core::Demand,
        /// How you want to be told about this one: push, email, chat:team, all, none.
        ///
        /// A *service*, never a device: "reach me by push" is a promise about a person, and
        /// which device is holding the credential is the fleet's business (ADR-0010). Defaults
        /// to every route there is, which is the right answer for a fleet of one person's
        /// devices; `none` is for the run you are sitting and watching.
        ///
        /// If nothing in the fleet offers what you ask for, `offload run` says so while you are
        /// still here rather than leaving you to notice at breakfast.
        #[arg(long, default_value_t = offload_core::Audience::Everyone, value_parser = commands::parse_audience)]
        notify: offload_core::Audience,
        /// Which news is worth interrupting you for: everything, or problems.
        ///
        /// The other half of `--notify`, which chooses *routes* and never kinds. `problems` is
        /// everything except the run finishing — a failure, a missed deadline and a question all
        /// still reach you, because each of those is a request for your attention.
        ///
        /// Defaults to `everything` here, because you typed this run and walked away: "is it
        /// done" is the question this plane exists to answer. `offload when` defaults the other
        /// way.
        #[arg(long = "notify-on", default_value_t = offload_core::Notices::Everything,
              value_parser = commands::parse_notices)]
        notices: offload_core::Notices,
        /// Stop and ask instead of being denied, when the agent wants to do something it has
        /// no permission for. `--ask=5` sets how many questions this run may put to you.
        ///
        /// Headless, a tool call the run does not already permit is refused and the run carries
        /// on having done less than it was asked. This says the opposite is preferable: the
        /// question goes wherever your notifications go, and `offload approve` answers it.
        ///
        /// It costs something real, which is why it is opt-in: the run is stopped mid-turn while
        /// it waits, so it cannot be checkpointed or moved, and after a few minutes nobody
        /// answering decides it the way it would have been decided anyway.
        ///
        /// The number bounds the interruption rather than the run. Once it is spent the run
        /// carries on with its calls decided by the agent's own rules — which is what happens
        /// without --ask at all — and says so in its log. It is also what makes
        /// `--permission ask` usable: under that mode every edit needs an answer too.
        ///
        /// The `=` is required for the number, because the prompt is a positional argument and
        /// `--ask "add tests"` would otherwise read the prompt as the count and then run
        /// something else entirely.
        #[arg(long, num_args = 0..=1, require_equals = true,
              default_missing_value = "20", default_value = "0",
              value_parser = commands::parse_ask)]
        ask: offload_core::AskPolicy,
        /// Stop this run after N turns, however unfinished it is.
        ///
        /// A budget on the work rather than on the clock, and the only thing here that stops an
        /// agent that is still making progress. Nothing else bounds how long a model decides a
        /// task deserves: without it a misunderstood prompt is discovered by reading the bill.
        ///
        /// Counted over the run's whole life, so migrating to another machine continues the
        /// count rather than starting a fresh one. When it is reached the run stops at the next
        /// turn boundary, keeps its worktree and its checkpoint, and is reported as having
        /// reached its limit — it is not resumed, by you or by the fleet, because the number
        /// you gave is the number you gave. Unset is what every run has done until now: the
        /// agent stops when it is done.
        #[arg(long, value_name = "N", value_parser = commands::parse_max_turns)]
        max_turns: Option<std::num::NonZeroU32>,
        /// Let this run use one of the fleet's resources: `--use email`. Repeatable.
        ///
        /// A run reaches nothing beyond its own worktree unless it is granted something here.
        /// Holding a resource and hosting a run are two different things — a node with a mailbox
        /// does not hand every run that lands on it the mail — so this is a decision somebody
        /// makes per run, the way `--allow` is.
        ///
        /// Named by *service*, never by the name a node gave one of its own: a service is a
        /// promise about what you can reach and a name is a machine. Until a resource can be
        /// reached across the mesh, granting one places the run where it is — and if nowhere in
        /// the fleet offers it, `offload run` says so while you are still here.
        #[arg(long = "use", value_name = "SERVICE", value_parser = commands::parse_service)]
        resources: Vec<offload_core::Service>,
        #[command(flatten)]
        placing: PlacingArgs,
    },

    /// Answer a question an agent is stopped on: yes.
    ///
    /// A run submitted with --ask stops and asks rather than being denied. It is waiting while
    /// you read this, so `offload asks` is the list and this is the answer. Name the question
    /// when a run is blocked on more than one.
    Approve {
        run: String,
        /// The tool call to answer, from `offload asks`. Needed only when there is more than one.
        tool_use_id: Option<String>,
    },

    /// Answer a question an agent is stopped on: no.
    ///
    /// The agent is told, records the refusal, and carries on without doing that — which is what
    /// would have happened without --ask, except that now it happened because you said so.
    Deny {
        run: String,
        tool_use_id: Option<String>,
    },

    /// What is waiting for a person right now, and how long it has left.
    Asks,

    /// Who else is in the fleet, and how they are doing.
    Nodes {
        /// What this node has witnessed happening to its fleet: devices enrolled, and every
        /// use of the passphrase (ADR-0012).
        ///
        /// A record of observations rather than the fleet's memory — the machine that issued
        /// an invitation saw an enrolment, and every other machine saw a member it had never
        /// met. Both are true where they were written, so ask more than one device.
        #[arg(long)]
        history: bool,
    },

    /// Which models the fleet's agents offer, and which devices offer each (ADR-0080).
    ///
    /// Each device reads the list from its own agent at startup and whenever the agent is
    /// updated. `--model` takes the value in the first column.
    Models {
        /// Ask every device to read its agent's list again, for a model released since.
        #[arg(long)]
        refresh: bool,
    },

    /// What this node decided about runs, and what it was refused.
    ///
    /// The durable answer to the two questions `offload explain` cannot give: which machine a run
    /// was granted to at the time, and whether a write about it was ever fenced out. Local by
    /// design — it is what *this* machine did, so there is nothing to gossip and nothing to
    /// canvass; on a fleet, ask each device.
    Audit {
        /// One run, by id or prefix. Omit for this node's most recent rows across every run.
        ///
        /// The unfiltered listing is the last hundred and says so when there are more. Naming a
        /// run is how you reach further back: the filter is applied in the store, so an old run's
        /// rows are there however much has happened since. This line used to say "everything this
        /// node has recorded", which is what the command has never done.
        run: Option<String>,
    },

    /// This node's routes to a human: whether they work, and what they have carried.
    ///
    /// The answer to "why did nothing tell me". Local by design — a sink's credentials never
    /// leave the device that holds them (ADR-0010), so what another node has delivered is that
    /// node's own question to answer.
    Sinks {
        /// Actually send a test notification through each route.
        ///
        /// The only way to check the half a node cannot probe: it can see that a script is
        /// there, never that it reaches anybody.
        #[arg(long)]
        test: bool,
    },

    /// What this node is watching, and whether the watchers are up.
    ///
    /// A trigger is a program this device's owner nominated that notices something happening
    /// (ADR-0020) — a mailbox, a webhook, a clock. One line of its output is one event. Local by
    /// design, for the reason `offload sinks` is: the command stays on the machine it is on, so
    /// what another node watches is that node's own question to answer.
    Triggers,

    /// Stand up a rule: when a trigger fires here, submit this run.
    ///
    /// `offload when schedule --repo ~/dev/foo -- "check the nightly build"`. The event the
    /// trigger reported is appended to the prompt, under a heading saying it is data from
    /// outside — never substituted into it, and never used *as* the prompt.
    ///
    /// The service is what the owner of this device said their watcher watches, not the name
    /// they gave it: a service is a promise about what happens and a name is one machine's
    /// bookkeeping. `offload triggers` lists what is on offer here.
    ///
    /// A rule fires **one run at a time**. An event arriving while the last one is still going
    /// is dropped and counted rather than queued — a watcher's value is the current state, and a
    /// backlog of stale occurrences is a storm. `offload rules` shows both numbers.
    When {
        /// What fires this: the service of a trigger this node watches — schedule, email,
        /// webhook, chat:team — or, with `--on-notice`, one of `failed`, `finished`, `overdue`,
        /// `asked`, `answered`.
        service: String,
        /// Read the name above as a **notice** rather than as a trigger's service (ADR-0057).
        ///
        /// `offload when failed --on-notice "look at what broke"` is the escalation
        /// phase 8's demo ends with: when a run on this node fails, submit an agent run and let
        /// the fleet place it on a machine that has an agent.
        ///
        /// A flag rather than a name the daemon guesses at, because a trigger's service may be
        /// called anything — `failed` included — and a rule that fired for the wrong reason
        /// would be indistinguishable from one that never fired.
        ///
        /// **A notice about machine-started work fires nothing**, so an escalation cannot
        /// escalate itself (ADR-0057 §3).
        #[arg(long = "on-notice")]
        on_notice: bool,
        /// What the agent should do. The event is appended to it.
        ///
        /// Omit it and pass `--task <service>` instead to fire the cheap tier: a program this
        /// fleet's owner nominated, with no model and no tokens (ADR-0019).
        #[arg(required_unless_present = "task", conflicts_with = "task")]
        prompt: Option<String>,
        /// Fire a nominated program instead of an agent, by service.
        ///
        /// **The event is not passed to it.** A trigger's line is text from outside, and a task
        /// runs a command its owner wrote down — so the event *fires* the task and does not
        /// parametrise it (ADR-0019 §1). An agent rule appends the line to its prompt, where a
        /// model can be told which half is data.
        #[arg(long)]
        task: Option<String>,
        /// Arguments for the task, repeatable. Appended after the owner's own.
        #[arg(long = "arg", requires = "task")]
        args: Vec<String>,
        /// Repository to work in. Defaults to the current directory.
        ///
        /// Resolved here and stored, so it is this machine's path — which is the same thing a
        /// submitted run does, and the same reason a local path pins a run to one device.
        #[arg(long, default_value = ".")]
        repo: String,
        /// Model override, e.g. claude-haiku-4-5-20251001.
        #[arg(long)]
        model: Option<String>,
        /// ask | accept-edits | full. Defaults to the daemon's setting (accept-edits).
        #[arg(long, value_parser = commands::parse_permission)]
        permission: Option<PermissionMode>,
        /// Branch or commit to start from. Defaults to the repo's default branch.
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// Extra tool grant, repeatable. e.g. --allow "Bash(cargo test:*)"
        ///
        /// The grants are yours, decided now. Nothing the trigger reports can add to them, which
        /// is what keeps an event that arrives from outside from being an instruction.
        #[arg(long)]
        allow: Vec<String>,
        /// Leave a fired run pending if nobody will take it now, instead of refusing it.
        #[arg(long)]
        queue: bool,
        /// How long each fired run gets, from the moment it fires: 45m, 2h, 9h.
        ///
        /// A duration rather than a time, unlike `offload run --deadline`, and the difference is
        /// the whole point of a standing instruction: an instant resolved when you typed this
        /// would be in the past by the second firing, and every run after that would be reported
        /// overdue before it started.
        #[arg(long, value_parser = when::parse_duration_secs)]
        deadline: Option<u64>,
        /// What a fired run is expected to cost the device that takes it: light, normal, heavy.
        #[arg(long, default_value_t = offload_core::Demand::Normal)]
        demand: offload_core::Demand,
        /// How you want to be told: push, email, chat:team, all, none.
        ///
        /// Worth more thought here than on a submitted run: nobody is at the keyboard when this
        /// fires, so the notification is how you find out it happened at all.
        #[arg(long, default_value_t = offload_core::Audience::Everyone, value_parser = commands::parse_audience)]
        notify: offload_core::Audience,
        /// Which news is worth interrupting you for: everything, or problems.
        ///
        /// **Defaults to `problems` here**, and that is the difference between a watcher and a
        /// submission. A rule fires on its own, possibly all night: told everything, a watcher
        /// ticking every three seconds put eleven "finished after 2 turn(s)" notifications on a
        /// phone in forty seconds, and the only way to stop it was `--notify none`, which throws
        /// away the failures — the one thing the watcher exists to tell you. `everything` opts
        /// back in, for a rule that fires rarely enough to want a heartbeat.
        #[arg(long = "notify-on", default_value_t = offload_core::Notices::Problems,
              value_parser = commands::parse_notices)]
        notices: offload_core::Notices,
        /// Stop and ask instead of being denied. `--ask=5` bounds the questions per run.
        #[arg(long, num_args = 0..=1, require_equals = true,
              default_missing_value = "20", default_value = "0",
              value_parser = commands::parse_ask)]
        ask: offload_core::AskPolicy,
        /// Stop each fired run after N turns, however unfinished it is.
        ///
        /// Worth more here than on `offload run`, and for the reason a rule exists: this fires
        /// unattended, for months, at an hour nobody is reading the output. A prompt that was
        /// fine in January against a repo that has since changed is the case where a bound on
        /// the work is the only thing that notices.
        #[arg(long, value_name = "N", value_parser = commands::parse_max_turns)]
        max_turns: Option<std::num::NonZeroU32>,
        /// Let a fired run use one of the fleet's resources: `--use email`. Repeatable.
        #[arg(long = "use", value_name = "SERVICE", value_parser = commands::parse_service)]
        resources: Vec<offload_core::Service>,
        #[command(flatten)]
        placing: RulePlacingArgs,
    },

    /// The standing instructions on this node: what they fire, and what they have done.
    ///
    /// Local, like everything else about a trigger. On a fleet, ask each device — the union is
    /// nobody's to own, which is the same reason `offload asks` canvasses rather than reading
    /// gossip.
    Rules,

    /// Forget a standing instruction.
    Unwatch {
        /// The rule, by id or prefix, from `offload rules`.
        rule: String,
    },

    /// Run something on a clock, for as long as the fleet exists (ADR-0019 §3).
    ///
    /// Unlike `offload when`, this does **not** die with the device you type it on: a schedule is
    /// gossiped, so whichever machine is available fires it, and closing the laptop changes
    /// nothing. That is the whole difference between this and `cron`.
    ///
    /// The period is aligned to the clock rather than to when you typed it — `every 15m` fires
    /// at :00, :15, :30 and :45 — and `--at` moves the whole ladder: `every 24h --at 3h` is
    /// 03:00 **UTC**. There is deliberately no local time and no cron syntax; see ADR-0056.
    ///
    /// Missed ticks are not made up. A phone asleep for six hours wakes and fires once.
    Every {
        /// How often: 15m, 6h, 24h. At least a minute.
        #[arg(value_parser = when::parse_duration)]
        every: u64,
        /// How far into the period the tick falls, in **UTC**: `--every 24h --at 3h` is 03:00Z.
        #[arg(long = "at", value_parser = when::parse_duration)]
        at: Option<u64>,
        /// The cheap tier: a program this fleet's owner nominated, by service (ADR-0019 §1).
        ///
        /// The usual thing to schedule, and the reason this command exists — a watcher costs a
        /// process rather than a model.
        #[arg(long, conflicts_with = "prompt")]
        task: Option<String>,
        /// Arguments for the task, repeatable. Appended after the owner's own.
        #[arg(long = "arg", requires = "task")]
        args: Vec<String>,
        /// …or a prompt, for an agent run every tick. Think about the bill before you do.
        #[arg(long)]
        prompt: Option<String>,
        /// Repository for an agent run. Defaults to the current directory.
        #[arg(long, default_value = ".", requires = "prompt")]
        repo: String,
        /// Model override for an agent run.
        #[arg(long, requires = "prompt")]
        model: Option<String>,
        /// Extra tool grant for an agent run, repeatable.
        #[arg(long, requires = "prompt")]
        allow: Vec<String>,
        /// What this run costs a device to host: light, normal, heavy (ADR-0013).
        ///
        /// `light` for a watcher, and it is worth setting: it is what lets a device whose owner
        /// wrote `[policy.light]` take the work on battery (ADR-0019 §4).
        #[arg(long, default_value_t = offload_core::Demand::Normal)]
        demand: offload_core::Demand,
        /// Which routes each occurrence's news is for (ADR-0010).
        ///
        /// `problems` is the sensible pairing for a watcher and is what `--notices` decides;
        /// this chooses the routes. A schedule that reported success every fifteen minutes
        /// would be the notification storm ADR-0010 exists to prevent.
        #[arg(long, default_value_t = offload_core::Audience::Everyone, value_parser = commands::parse_audience)]
        notify: offload_core::Audience,
        /// Leave an occurrence pending if nobody will take it, instead of skipping the tick.
        #[arg(long)]
        queue: bool,
        /// One line to tell it apart in `offload schedules`.
        #[arg(long, default_value = "")]
        note: String,
    },

    /// What runs on a clock, here and anywhere in the fleet.
    Schedules,

    /// Take a schedule away, everywhere.
    Unschedule {
        /// The schedule, by id or prefix, from `offload schedules`.
        schedule: String,
    },

    /// Hand this node's runs to the rest of the fleet.
    ///
    /// Each one is checkpointed at its next turn boundary — never mid-turn — and then offered
    /// to the fleet like a fresh submission. This is what "close the laptop" should mean.
    Drain,

    /// List runs.
    Ps {
        /// Include finished runs.
        #[arg(short, long)]
        all: bool,
    },

    /// Show a run's output.
    Logs {
        /// Run id, or a unique prefix of one.
        run: String,
        /// Keep streaming until the run ends.
        #[arg(short, long)]
        follow: bool,
    },

    /// Why a run is where it is: who decides, what they make of it, and what every node
    /// says about taking it.
    ///
    /// Asks the fleet again rather than replaying the round that placed the run — a bid
    /// describes one second, and the useful answer is what the fleet would say now.
    Explain {
        /// Run id, or a unique prefix of one.
        run: String,
    },

    /// Look inside a run's workspace: list a directory, or print a file (ADR-0075).
    ///
    /// Read-only, from the machine that has it: the checkout, with uncommitted work, while it is
    /// there, and the run's branch after it is cleaned up. Paths are relative to the workspace.
    Files {
        /// Run id, or a unique prefix of one.
        run: String,
        /// A directory or file inside the workspace. The root when left out.
        #[arg(default_value = "")]
        path: String,
    },

    /// Stop a running agent.
    Cancel { run: String },

    /// Capture a run at its next turn boundary and hand it back to the pool.
    ///
    /// The agent finishes the turn it is on, its conversation and workspace are stored,
    /// and the run stops. `offload resume` picks it up again.
    Checkpoint { run: String },

    /// Continue a checkpointed run from where it stopped.
    Resume {
        /// Run id, or a unique prefix of one.
        run: String,
        /// What to tell the agent on the way back in. Defaults to "continue where you
        /// left off".
        prompt: Option<String>,
        /// Stream the run's output instead of returning immediately.
        #[arg(short, long)]
        follow: bool,
    },

    /// Continue a finished run with a new one, starting where its work ended (ADR-0064).
    ///
    /// For a run that is `completed` or `cancelled` — a failed one is resumed instead. The new run
    /// starts from the old one's branch **and** whatever it left uncommitted, in a fresh session
    /// with an empty context, handed three things: what the old run was asked, its own closing
    /// words, and its whole transcript as a file to read if it needs to. `--session` instead
    /// carries on the old conversation itself.
    ///
    /// Everything else is the old run's — repo, model, grants, where it prefers to run — unless a
    /// flag here says otherwise. Type it on any machine: the old run's workspace is read on the
    /// one that finished it, which has to be up. Where the new run *runs* is the ordinary
    /// placement, so `--prefer` works, and `here` means this machine.
    Continue {
        /// The finished run, by id or unique prefix.
        run: String,
        /// What to do next.
        prompt: String,
        /// Carry on the old conversation — the reasoning as live context — rather than start
        /// fresh with a handoff. Fills the context with everything the old run said.
        #[arg(long)]
        session: bool,
        /// Stream the new run's output instead of returning immediately.
        #[arg(short, long)]
        follow: bool,
        /// Leave it pending if nobody will take it now, instead of refusing.
        #[arg(long)]
        queue: bool,
        /// When this needs to be done, from now. The old run's deadline is not inherited.
        #[arg(long, value_parser = when::parse_deadline)]
        deadline: Option<u64>,
        /// A tool grant beyond the old run's, repeatable.
        #[arg(long)]
        allow: Vec<String>,
        /// Stop the new run after N turns. The old run's limit is not inherited.
        #[arg(long, value_name = "N", value_parser = commands::parse_max_turns)]
        max_turns: Option<std::num::NonZeroU32>,
        /// Stop and ask rather than being denied; `--ask=5` sets how many questions.
        #[arg(long, num_args = 0..=1, require_equals = true,
              default_missing_value = "20", default_value = "0",
              value_parser = commands::parse_ask)]
        ask: offload_core::AskPolicy,
        #[command(flatten)]
        placing: PlacingArgs,
    },

    /// Change when a run needs to be done.
    ///
    /// One of the two parts of a submitted run that can be changed — because changing your mind
    /// about when you need something is ordinary, and the run that was fine overnight is the one
    /// blocking a demo. On a pending run this re-opens placement; on a running one it changes
    /// nothing you can see until something goes wrong, and it says so.
    Deadline {
        /// Run id, or a unique prefix of one.
        run: String,
        /// From now: 45m, 2h, 1h30m — or `none` for as soon as possible.
        when: String,
    },

    /// Change which runs this one goes ahead of.
    ///
    /// The other editable part, and the weaker one: priority decides who yields when two runs
    /// want the same machine at the same moment, and nothing else. A low-priority run on an idle
    /// fleet starts immediately, a high-priority one preempts nothing, and a run that has been
    /// waiting long enough outranks any priority by itself — urgency leads, and this breaks its
    /// ties.
    #[command(allow_negative_numbers = true)]
    Priority {
        /// Run id, or a unique prefix of one.
        run: String,
        /// The new value, e.g. `10`, `+10` or `-5`. Absolute, not a delta: a delta would have to
        /// be added to a value this machine may have heard a second ago.
        priority: i32,
    },

    /// Discard a finished run's worktree. Its branch and commits are kept.
    Rm { run: String },
}

/// End quietly when whatever was reading our output goes away.
///
/// `offload fleet | head` panics: Rust ignores `SIGPIPE` before `main`, so the write fails, the
/// print macro panics, and the operator sees a backtrace that reads as a crash in the fleet code
/// rather than as `head` having closed the pipe after ten lines.
///
/// The obvious fix — restoring the default `SIGPIPE` disposition — needs `unsafe`, which this
/// workspace forbids outright, and that lint is worth more than this papercut. So the panic is
/// caught instead: a print that fails is a reader that stopped caring, and the right answer is to
/// stop, which is exactly what the signal would have done.
///
/// **The coupling is on a std panic message**, and it is deliberate that the failure direction is
/// backwards-safe: if that message ever changes, this stops matching and the behaviour is today's
/// — a panic — rather than something worse. [`a_print_failure`] is the whole of it, and it is
/// tested against the strings std produces.
fn end_quietly_on_a_closed_pipe() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        if a_print_failure(message) {
            // Zero rather than 141: a pipeline whose reader has finished is not a failure, and
            // `set -o pipefail` should not report one.
            std::process::exit(0);
        }
        previous(info);
    }));
}

/// Is this panic the print macros failing to write?
///
/// Matches the prefix and not the error text: an `io::Error`'s message comes from the C library
/// and is translated, so a machine in French would see "Relais brisé" and a check for "Broken
/// pipe" would quietly stop working there.
fn a_print_failure(message: &str) -> bool {
    message.starts_with("failed printing to std")
}

/// Does this command talk to a daemon, or answer from this device's own state directory?
///
/// Exhaustive on purpose, with no wildcard arm: a new subcommand has to be classified rather than
/// silently inheriting an answer, which is the reason `Constraint::observed` writes its arms out
/// too. Getting it wrong in the *daemon* direction costs a spurious note; getting it wrong the
/// other way is a command that quietly answers about the wrong node.
fn addresses_a_daemon(command: &Command) -> bool {
    match command {
        // Answered from this device: what it is, what its owner permits, and who it belongs to.
        Command::Probe { .. }
        | Command::Match { .. }
        | Command::Policy { .. }
        | Command::Init { .. }
        | Command::Join { .. }
        | Command::Id { .. }
        | Command::Invite { .. }
        | Command::Verify { .. }
        | Command::Grant { .. }
        | Command::Revoke { .. }
        | Command::Rekey { .. }
        | Command::Fleet { .. }
        | Command::SigningBytes => false,
        // Everything about runs, and everything about the fleet as the daemon sees it.
        Command::Run { .. }
        | Command::Ps { .. }
        | Command::Status
        | Command::Logs { .. }
        | Command::Explain { .. }
        | Command::Files { .. }
        | Command::Nodes { .. }
        | Command::Models { .. }
        | Command::Deadline { .. }
        | Command::Priority { .. }
        | Command::Cancel { .. }
        | Command::Checkpoint { .. }
        | Command::Rm { .. }
        | Command::Resume { .. }
        | Command::Continue { .. }
        | Command::Asks
        | Command::Approve { .. }
        | Command::Deny { .. }
        | Command::Audit { .. }
        | Command::Sinks { .. }
        | Command::Triggers
        | Command::When { .. }
        | Command::Rules
        | Command::Unwatch { .. }
        | Command::Every { .. }
        | Command::Schedules
        | Command::Unschedule { .. }
        | Command::Drain
        // Both: the decision is written to the state directory, and the list it decides from is
        // the daemon's, which holds the peers' certificates.
        | Command::Reapprove { .. } => true,
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    end_quietly_on_a_closed_pipe();
    let cli = Cli::parse();
    if cli.socket.is_some() && !addresses_a_daemon(&cli.command) {
        // `--socket` selects which daemon to talk to, and about a third of these commands never
        // talk to one: membership is answered from the state directory, because a certificate
        // verifies against the fleet key alone (ADR-0012). Silently ignoring the flag produced a
        // confident wrong answer on the very setup the docs recommend — two nodes on one machine,
        // where `--socket` is how you pick one — and `offload fleet --socket a.sock` reported
        // "this node has not joined a fleet" about the node that founded it, exit 0.
        //
        // A note rather than an error: the flag is harmless, scripts that pass it globally are not
        // wrong to, and what the reader needs is the name of the thing that *does* select a node.
        eprintln!(
            "note: --socket addresses a daemon, and this command reads the state directory \
             instead. Use --state-dir, or set OFFLOAD_STATE_DIR, to choose which node it \
             answers about."
        );
    }
    let socket = cli.socket.clone().unwrap_or_else(client::default_socket);

    match cli.command {
        // Local-only: these answer questions about this device and need no daemon.
        // The daemon's own config file when none is named (ADR-0074): these answer what
        // `offloadd` would do, so they read what it reads.
        Command::Probe { config, json } => probe(
            offload_node::config::config_path(config.as_deref()).as_deref(),
            json,
        ),
        Command::Match {
            expr,
            config,
            verbose,
        } => match_constraint(
            &expr,
            offload_node::config::config_path(config.as_deref()).as_deref(),
            verbose,
        ),
        Command::Policy { config, json } => policy(
            offload_node::config::config_path(config.as_deref()).as_deref(),
            json,
        ),

        // Fleet membership: local too, and deliberately so. A peer verifies a certificate
        // with the fleet public key alone, so founding, joining and revoking need no mesh.
        Command::Init {
            state_dir,
            name,
            hardware_key,
        } => fleet::init(state_dir, name, hardware_key),
        Command::Join {
            passphrase,
            token,
            state_dir,
            name,
        } => fleet::join(state_dir, name, passphrase, token),
        Command::Id { state_dir } => fleet::id(state_dir),
        Command::Invite {
            node,
            name,
            grants,
            state_dir,
        } => fleet::invite(state_dir, &node, name, grants),
        Command::Reapprove {
            nodes,
            due,
            state_dir,
        } => {
            // The daemon of the node being decided on, not whichever one the default names: with
            // `--state-dir` alone, the default socket belongs to some other node, and the list
            // would describe a fleet this command then writes nothing about.
            let socket = cli.socket.clone().unwrap_or_else(|| {
                state_dir
                    .as_ref()
                    .map_or_else(|| socket.clone(), |dir| dir.join("offloadd.sock"))
            });
            fleet::reapprove(state_dir, &socket, nodes, due).await
        }
        Command::Verify { state_dir } => fleet::verify(state_dir),
        Command::Grant {
            grant,
            hardware_key,
            state_dir,
        } => fleet::grant(state_dir, grant, hardware_key),
        Command::SigningBytes => fleet::signing_bytes(),
        Command::Revoke { node, state_dir } => fleet::revoke(state_dir, &node),
        Command::Rekey { evict, state_dir } => fleet::rekey(state_dir, evict),
        Command::Fleet { state_dir } => fleet::show(state_dir),

        // Everything else talks to offloadd.
        Command::Status => commands::status(&socket).await,
        Command::Run {
            repo,
            archive,
            scratch,
            prompt,
            task,
            args,
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
        } => {
            let placing = placing.into_placing();
            // ADR-0063 §4: a run held for a node that may be asleep has nobody to bid for it now,
            // and being refused to your face for that is not what `--hold` asked for.
            let queue = queue || placing.hold_until.is_some();
            if let Some(service) = task {
                return commands::run_task(commands::TaskSubmission {
                    socket: &socket,
                    service,
                    args,
                    follow,
                    queue,
                    deadline,
                    demand,
                    notify,
                    notices,
                    placing,
                })
                .await;
            }
            // Two steps on purpose (see `Request::StoreArchive`): the bytes go in first and
            // answer with a digest, and only then is a run submitted naming it. A submission the
            // fleet refuses must not leave the operator wondering whether their archive is
            // half-somewhere, and an unreadable file is not a rejected run.
            let repo = match archive {
                Some(path) => commands::store_archive(&socket, &path).await?,
                None if scratch => offload_core::SCRATCH.to_string(),
                None => repo.unwrap_or_else(|| ".".to_string()),
            };
            commands::run(commands::Submission {
                socket: &socket,
                repo,
                // `required_unless_present = "task"` above, and this branch is the `else`.
                prompt: prompt.unwrap_or_default(),
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
            })
            .await
        }
        Command::Approve { run, tool_use_id } => {
            commands::answer(&socket, &run, tool_use_id.as_deref(), true).await
        }
        Command::Deny { run, tool_use_id } => {
            commands::answer(&socket, &run, tool_use_id.as_deref(), false).await
        }
        Command::Asks => commands::asks(&socket).await,
        Command::Audit { run } => commands::audit(&socket, run.as_deref()).await,
        Command::Nodes { history } => {
            if history {
                commands::fleet_history(&socket).await
            } else {
                commands::nodes(&socket).await
            }
        }
        Command::Models { refresh } => commands::models(&socket, refresh).await,
        Command::Sinks { test } => commands::sinks(&socket, test).await,
        Command::Triggers => commands::triggers(&socket).await,
        Command::Rules => commands::rules(&socket).await,
        Command::Unwatch { rule } => commands::unwatch(&socket, &rule).await,
        Command::When {
            service,
            on_notice,
            prompt,
            task,
            args,
            repo,
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
            placing,
        } => {
            let placing = placing.into_placing();
            // Which tier, decided here and refused by clap either way round: `--task` beside
            // a prompt is an unexpected-argument error rather than a rule about which wins.
            let work = match task {
                Some(service) => commands::StandingWork::Task(commands::TaskSubmission {
                    socket: &socket,
                    service: service
                        .parse::<offload_core::Service>()
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                    args,
                    follow: false,
                    queue,
                    // A rule holds the *duration* and the daemon resolves it at each firing.
                    deadline: None,
                    demand,
                    notify,
                    notices,
                    placing: placing.clone(),
                }),
                None => commands::StandingWork::Agent(commands::Submission {
                    socket: &socket,
                    repo,
                    // `required_unless_present = "task"` is what makes this safe, and it is a
                    // clap guarantee rather than a hope: with neither a prompt nor `--task` the
                    // command has already failed at the keyboard.
                    prompt: prompt.unwrap_or_default(),
                    model,
                    permission,
                    git_ref,
                    allow,
                    follow: false,
                    queue,
                    // A standing instruction holds a duration, resolved at each firing. What
                    // goes into the submission itself is nothing.
                    deadline: None,
                    demand,
                    notify,
                    notices,
                    ask,
                    resources,
                    max_turns,
                    placing,
                }),
            };
            commands::stand_up(commands::Standing {
                socket: &socket,
                service,
                fired_by: if on_notice {
                    offload_node::api::FiredBy::Notice
                } else {
                    offload_node::api::FiredBy::Trigger
                },
                work,
                deadline_secs: deadline,
            })
            .await
        }
        Command::Every {
            every,
            at,
            task,
            args,
            prompt,
            repo,
            model,
            allow,
            demand,
            notify,
            queue,
            note,
        } => {
            let task = match task {
                Some(service) => Some((
                    service
                        .parse::<offload_core::Service>()
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                    args,
                )),
                None => None,
            };
            commands::every(commands::Recurring {
                socket: &socket,
                every_ms: every,
                at_ms: at,
                note,
                demand,
                notify,
                queue,
                task,
                agent: prompt.map(|prompt| commands::RecurringAgent {
                    prompt,
                    repo,
                    model,
                    allow,
                }),
            })
            .await
        }
        Command::Schedules => commands::schedules(&socket).await,
        Command::Unschedule { schedule } => commands::unschedule(&socket, &schedule).await,
        Command::Drain => commands::drain(&socket).await,
        Command::Ps { all } => commands::ps(&socket, all).await,
        Command::Logs { run, follow } => commands::logs(&socket, &run, follow).await,
        Command::Explain { run } => commands::explain(&socket, &run).await,
        Command::Files { run, path } => commands::files(&socket, &run, &path).await,
        Command::Cancel { run } => commands::cancel(&socket, &run).await,
        Command::Checkpoint { run } => commands::checkpoint(&socket, &run).await,
        Command::Resume {
            run,
            prompt,
            follow,
        } => commands::resume(&socket, &run, prompt, follow).await,
        Command::Continue {
            run,
            prompt,
            session,
            follow,
            queue,
            deadline,
            allow,
            max_turns,
            ask,
            placing,
        } => {
            let hold = placing.hold;
            commands::continue_run(
                &socket,
                offload_node::api::ContinueRequest {
                    run,
                    prompt,
                    mode: if session {
                        offload_core::ContinueMode::Session
                    } else {
                        offload_core::ContinueMode::Handoff
                    },
                    queue: queue || hold.is_some(),
                    deadline,
                    allow,
                    max_turns,
                    ask,
                    // Absent keeps the parent's, which is what the ADR's inheritance says.
                    require: placing.require,
                    prefer: placing.prefer,
                    hold_until: hold,
                },
                follow,
            )
            .await
        }
        Command::Deadline { run, when } => {
            commands::set_deadline(&socket, &run, when::parse_change(&when)?).await
        }
        Command::Priority { run, priority } => {
            commands::set_priority(&socket, &run, priority).await
        }
        Command::Rm { run } => commands::remove(&socket, &run).await,
    }
}

fn probe(config: Option<&Path>, json: bool) -> Result<()> {
    let (caps, source) = probed(config)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&caps)?);
    } else {
        print!("{}", offload_probe::summarise(&caps));
        // Which program the agent line above was an answer about — the same sentence `offload
        // policy` prints about which policy it read, and for the same reason: this is what
        // somebody runs when a node will not bid, and an agent reported here that the daemon
        // never looks at sends them off to debug the fleet.
        println!("agent from  {source}");
    }
    Ok(())
}

fn match_constraint(expr: &str, config: Option<&Path>, verbose: bool) -> Result<()> {
    let wanted = constraint_expr::parse(expr)?;
    let (caps, source) = probed(config)?;
    let explain = wanted.clauses.explain(&caps);
    // `here` and `node=` are about identity, which the daemon resolves to an id at submission
    // (ADR-0063 §3). Asked locally, `here` is this device by definition and a name is this
    // device's configured one — the parser is the same, so what parses here parses there.
    let name = offload_node::Config::load(config).map(|c| c.name).ok();
    let node_failures: Vec<String> = wanted
        .nodes
        .iter()
        .filter_map(|node| match node {
            offload_core::NodeRef::Here => None,
            offload_core::NodeRef::Named(wanted) if Some(wanted) == name.as_ref() => None,
            offload_core::NodeRef::Named(wanted) => Some(match &name {
                Some(name) => format!("is node {wanted} (this device is {name})"),
                None => format!("is node {wanted} (this device's name could not be read)"),
            }),
        })
        .collect();

    if verbose {
        print!("{}", explain.render());
        println!();
    }
    // Before the verdict, because `agent=claude-code` is a clause about a particular program and
    // this command's whole job is to answer for the machine as the daemon sees it.
    println!("agent from  {source}");

    if explain.satisfied && node_failures.is_empty() {
        println!("match: this device satisfies the constraint");
        Ok(())
    } else {
        println!("no match:");
        let clause_failures = if explain.satisfied {
            Vec::new()
        } else {
            explain.failures()
        };
        for failure in clause_failures.iter().chain(&node_failures) {
            println!("  - {failure}");
        }
        // Exit non-zero so this composes in scripts.
        std::process::exit(1);
    }
}

/// This device, probed the way the daemon would probe it when a config says which agent it runs,
/// and the sentence saying which program that was.
///
/// One function so the three local commands cannot answer from three different programs, and the
/// two halves together because a report of what this device has is only as good as knowing what
/// it asked. Without a config it is the plain probe, which asks `claude` on `PATH` — right for a
/// machine with an ordinary install, and stated as such wherever it is printed.
fn probed(config: Option<&Path>) -> Result<(offload_core::Capabilities, String)> {
    let Some(path) = config else {
        return Ok((
            offload_probe::probe(),
            format!(
                "{} on PATH — pass --config to ask about the daemon's",
                offload_probe::DEFAULT_CLAUDE_BINARY
            ),
        ));
    };
    let loaded = offload_node::Config::load(Some(path))?;
    let source = format!(
        "{} — agent.binary in {}{}",
        loaded.agent.binary.display(),
        path.display(),
        // …and which state directory it was asked about, because that is what selects the
        // *account* (ADR-0028) and therefore whether "authenticated" above is about the login
        // the owner meant. Silent when unset, which is the ordinary one-login machine.
        match &loaded.agent.config_dir {
            Some(dir) => format!(", agent.config_dir {}", dir.display()),
            None => String::new(),
        }
    );
    Ok((
        offload_node::deliver::capabilities_reading_models(&loaded),
        source,
    ))
}

fn policy(config: Option<&Path>, json: bool) -> Result<()> {
    let (caps, _) = probed(config)?;
    // Through the daemon's own function, not a second copy of the same layering: the whole point
    // of this command is that its answer matches what `offloadd` would do, and two places that
    // both start from the class default and apply overrides is two places that can drift.
    let loaded = offload_node::Config::load(config)?;
    let policy = loaded.work_policy(caps.device_class);

    if json {
        println!("{}", serde_json::to_string_pretty(&policy)?);
        return Ok(());
    }

    println!("device class      {}", caps.device_class);
    // Which policy this is an answer about. It used to be the class default always, silently,
    // so an owner who had set `accept = "always"` on their phone was told it accepts work only
    // while charging — and the natural conclusion is that the fleet is ignoring them.
    println!(
        "policy from       {}",
        match config {
            Some(path) => format!("{}", path.display()),
            None => "the class default — pass --config to read the daemon's".to_string(),
        }
    );
    // The spelling the **config** uses, not the variant name. `{:?}` printed `WhenCharging`,
    // and a config saying `accept = "WhenCharging"` is refused — *"unknown variant, expected one
    // of `never`, `when_charging`, `always`"*. The refusal is good and self-correcting, which is
    // why this cost a detour rather than a dead end; it is still the one command whose whole job
    // is saying what the policy in force **is**, printing it in a form its own input rejects.
    println!("accept work       {}", policy.accept);
    println!("max concurrent    {}", policy.max_concurrent_runs);
    println!(
        "budget            {} shares ({} normal runs){}",
        policy.budget(),
        policy.budget() / offload_core::Demand::Normal.shares().max(1),
        if policy.budget_shares.is_some() {
            ""
        } else {
            ", from the run count"
        }
    );
    println!(
        "battery floor     {}",
        policy
            .min_battery_percent
            .map_or_else(|| "none".to_string(), |p| format!("{p}%"))
    );
    // What the owner permits, *and* whether this device is in a state where that permission
    // ever comes up. The line said "refused" on every device in the world, guarding against a
    // state nothing could enter — the capability was a `bool` the probe set to `false` and
    // nothing could set otherwise (ADR-0045). Same disease as `agents allowed` below, which is
    // why that one's comment is worth reading here: a rule that cannot be reached reads as a
    // control that is being applied.
    println!(
        "metered network   {}{}",
        if policy.allow_metered {
            "allowed"
        } else {
            "refused"
        },
        match caps.metered_network {
            offload_core::Metered::Yes => " — and this link is metered",
            offload_core::Metered::No => " — this link is not metered, so it does not arise",
            offload_core::Metered::Unknown =>
                " — but nobody knows whether this link is metered, so it does not arise; \
                 say `metered = \"yes\"` in the config if it is",
        }
    );
    // Shown because it is now settable. It was on `WorkPolicy` and enforced by `admits` from the
    // start with nothing able to set it, so this line would have said "any" on every device in
    // the world.
    println!(
        "agents allowed    {}",
        policy.allowed_agents.as_ref().map_or_else(
            || "any this device has".to_string(),
            |set| set
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    );
    // The light form, and only when there is one: three more lines on every device in the
    // world, saying "same as above", is how a report teaches people to skim it (ADR-0019 §4).
    //
    // Printed through the same `*_for` resolvers the gate calls, never by re-reading
    // `policy.light` — the whole subject of this feature is a door that answers differently
    // depending on what is asked, and a report that worked that out for itself is the shape
    // this project keeps having to fix.
    if policy.light.is_stated() {
        println!();
        let light = offload_core::Demand::Light;
        // **The heading does not claim the owner said all three, because they need only have
        // said one.** Every field of `LightWork` is an override and `None` inherits (ADR-0019
        // §4), so a `[policy.light]` block naming `accept` alone still printed three lines under
        // *"the owner has said what this device does with it"* — with the **ordinary** battery
        // floor among them. Measured: `accept = "always"` and nothing else, and the block read
        // `battery floor 40%`, which is the main policy's number and changes when that one does,
        // without the light block being touched. Somebody reading that has been told they set a
        // light-work floor they did not set.
        //
        // The values still come from the `*_for` resolvers the gate itself calls — the note
        // below reads `policy.light` only to ask *whether the owner stated it*, which is a
        // different fact from what the value is and is the one the heading was wrong about.
        // The marker sits in a column, so three lines whose values are `always`, `40%` and
        // `refused` do not put it in three places. `when_charging` is the widest value there is.
        let note = |value: String, stated: bool| {
            if stated {
                value
            } else {
                format!("{value:<14} (inherited)")
            }
        };
        println!("light work        what this device does with it:");
        println!(
            "  accept work     {}",
            note(
                policy.accept_for(light).to_string(),
                policy.light.accept.is_some()
            )
        );
        println!(
            "  battery floor   {}",
            note(
                policy
                    .battery_floor_for(light)
                    .map_or_else(|| "none".to_string(), |p| format!("{p}%")),
                policy.light.min_battery_percent.is_some()
            )
        );
        println!(
            "  metered network {}",
            note(
                if policy.allows_metered_for(light) {
                    "allowed".to_string()
                } else {
                    "refused".to_string()
                },
                policy.light.allow_metered.is_some()
            )
        );
    }
    println!();

    report_admission(
        &caps,
        &policy,
        offload_node::deliver::host_thermal(&loaded.state_dir),
    );
    Ok(())
}

/// The question a device owner actually cares about: would this thing take work right now?
fn report_admission(
    caps: &Capabilities,
    policy: &WorkPolicy,
    thermal: Option<offload_core::Thermal>,
) {
    let agents: Vec<offload_core::AgentKind> = caps
        .playing(offload_core::Role::Execute)
        .filter_map(|capability| match &capability.service {
            offload_core::Service::Agent(kind) => Some(kind.clone()),
            _ => None,
        })
        .collect();

    // What this device offers of the cheap tier, by service (ADR-0019 §1). Off the same
    // `Role::Execute` list as the agents, because that is what a task advertises itself as.
    let tasks: Vec<String> = caps
        .playing(offload_core::Role::Execute)
        .filter(|capability| !matches!(capability.service, offload_core::Service::Agent(_)))
        .map(|capability| capability.service.to_string())
        .collect();

    // **Not a return any more**, and this line was the whole of ADR-0019's scenario answered
    // wrongly: a phone with no agent that watches an API is the device the cheap tier exists
    // for, and this command told its owner it would accept no work at all. It is still the
    // right sentence when the device offers neither tier.
    if agents.is_empty() && tasks.is_empty() {
        println!("would accept work: no — no agents installed and no tasks nominated");
        return;
    }

    // Nothing held, because this is a machine being asked about itself rather than a daemon
    // reporting its load — but the *load average* is real, and it is the one thing here that
    // can refuse work on a device that is otherwise entirely willing (ADR-0013).
    let load = offload_core::NodeLoad {
        cpu_percent: offload_probe::cpu_load_percent(&caps.os, caps.cpu_cores),
        // Read the way the daemon reads it (ADR-0068), from the host's facts in the state dir.
        thermal,
        ..offload_core::NodeLoad::default()
    };
    println!(
        "cpu load          {}",
        load.cpu_percent
            .map_or_else(|| "not reported".to_string(), |p| format!("{p}%"))
    );
    // Only where a host says: every other platform has no status to print, and "none" would
    // read as a cool device rather than an unasked question.
    if let Some(status) = load.thermal {
        println!("thermal           {status}");
    }

    for agent in &agents {
        for demand in [
            offload_core::Demand::Light,
            offload_core::Demand::Normal,
            offload_core::Demand::Heavy,
        ] {
            // No account ceiling here: this command runs with no daemon and no view, and an
            // account's limit is a statement about runs on other machines. `offload status`
            // asks the same question of a node that can see its fleet.
            match policy.admits(caps, offload_core::Tier::Agent(agent), demand, &load, None) {
                Ok(()) => println!("would accept work now: yes ({agent}, {demand})"),
                Err(refusal) => {
                    println!("would accept work now: no ({agent}, {demand}) — {refusal}");
                }
            }
        }
    }

    // The cheap tier's own answer, once rather than per task: `Tier::Task` is what the gates
    // are asked, and they are asked nothing about *which* program — a task's identity for
    // concurrency is its service and the owner's ceilings bound them all together
    // (ADR-0019 §2). The services are named so the reader knows what the line is about.
    if !tasks.is_empty() {
        // Named first, so the three lines under it are read as being about these. A sentence
        // rather than a label-and-value like the block above: it sits among sentences.
        println!("tasks nominated: {}", tasks.join(", "));
        for demand in [
            offload_core::Demand::Light,
            offload_core::Demand::Normal,
            offload_core::Demand::Heavy,
        ] {
            match policy.admits(caps, offload_core::Tier::Task, demand, &load, None) {
                Ok(()) => println!("would accept a task now: yes ({demand})"),
                Err(refusal) => println!("would accept a task now: no ({demand}) — {refusal}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_pipe_is_recognised_by_the_message_std_produces() {
        // The exact strings, because this is the whole of the coupling: `print!` panics with
        // "failed printing to {stdout|stderr}: {io error}", and the error text is the C
        // library's and therefore translated. Matching the prefix is what keeps a French
        // machine working.
        assert!(a_print_failure(
            "failed printing to stdout: Broken pipe (os error 32)"
        ));
        assert!(a_print_failure(
            "failed printing to stderr: Relais brisé (pipe)"
        ));
        // Everything else is a real panic and must reach the operator with its backtrace.
        assert!(!a_print_failure("attempt to divide by zero"));
        assert!(!a_print_failure(""));
    }
}
