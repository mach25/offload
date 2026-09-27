//! Two kinds of policy, deliberately separate from capability.
//!
//! * [`WorkPolicy`] — what a device's *owner* permits. A phone is a perfectly capable
//!   worker; whether it should take a run right now, on battery, on mobile data, is a
//!   different question from whether it could.
//! * [`ReassignPolicy`] — what to do when a holder drops off. Losing contact is not
//!   automatically a reason to move work. See `docs/adr/0007-node-dropoff.md`.
//!
//! Both are pure functions of state plus `now`.

use crate::capability::{AgentKind, Capabilities, DeviceClass};
use crate::capacity::{AccountUse, Capacity, Demand, NodeLoad};
use crate::deadline::Slack;
use crate::id::NodeId;
use crate::run::{Restartability, Run, RunState};
use crate::time::Millis;
use crate::view::{ClusterView, NodeStatus, NodeView};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Owner policy: may this node take work at all?
// ---------------------------------------------------------------------------

/// Which tier of work the owner is being asked about (ADR-0019).
///
/// Three of [`WorkPolicy`]'s clauses have no referent for a task — `allowed_agents`, the
/// agent's own concurrency ceiling, and the account's — because a nominated program has no
/// agent, no model and no bill. The rest of the policy applies to both tiers unchanged: a
/// phone that will not work on battery will not run a shell script on battery either.
///
/// An enum rather than `Option<&AgentKind>` on purpose, and the reason is [`WorkPolicy::permits`]'s
/// own history: `allowed_agents` was an owner's control defeated by an ordering, and a `None`
/// that skips three gates is one an agent-run caller can reach by accident from a
/// `RunSpec::agent()` that happened to be empty. Naming [`Tier::Task`] is a claim about the
/// work, so the compiler asks for it. Construct it from the run itself — `Work::tier` — rather
/// than by matching at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier<'a> {
    /// An agent run, and which agent.
    Agent(&'a AgentKind),
    /// A program the owner nominated (ADR-0019 §1).
    Task,
}

impl Tier<'_> {
    /// The agent this is about, or `None` for a task.
    #[must_use]
    pub fn agent(&self) -> Option<&AgentKind> {
        match self {
            Tier::Agent(agent) => Some(agent),
            Tier::Task => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptWork {
    /// Control plane only: submit and observe runs, never host them.
    Never,
    /// The sensible phone default.
    WhenCharging,
    Always,
}

/// The spelling an owner writes in `node.toml`, which is the one a report has to show.
///
/// `offload policy` printed this with `{:?}` — `WhenCharging` — while the config accepts only
/// `never`, `when_charging` and `always`, and refuses the variant name outright. The refusal is a
/// good one (it lists the three), so the cost was a detour rather than a dead end; it is still
/// the command whose whole job is saying what the policy in force *is*, printing it in a form its
/// own input rejects. Kept in step with `#[serde(rename_all = "snake_case")]` by a test.
impl std::fmt::Display for AcceptWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AcceptWork::Never => "never",
            AcceptWork::WhenCharging => "when_charging",
            AcceptWork::Always => "always",
        })
    }
}

/// The owner's three standing doors, asked again for `Demand::Light` work (ADR-0019 §4).
///
/// Every field is an override: `None` — the default for all three — means light work is asked
/// exactly the question everything else is asked, so **a fleet that says nothing behaves as it
/// always has.** That is the `budget_shares: None` precedent, and it matters more here, because
/// these are the gates people have already configured.
///
/// This is the owner saying *"I trust light work on this device"*, and never the run saying it.
/// `Demand` is declared by the **submitter** and ADR-0013 treats it as a hint whose harsher
/// reading wins at admission, so a gate keyed on it alone would be a gate a submitter could talk
/// their way through: `--demand light` must not start work on a phone whose owner said "only
/// while charging". What makes that safe is that the relaxation has to be written down here, on
/// the device, by the person who owns it.
///
/// What is deliberately *not* here: the concurrency cap and the budget. Those already consult
/// demand (`Capacity::room_for` charges `Demand::shares`, `needs_idle_percent` scales the load
/// threshold), which is why the three below were the whole of what was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LightWork {
    /// The owner's standing yes-or-no for light work. `None` inherits [`WorkPolicy::accept`].
    #[serde(default)]
    pub accept: Option<AcceptWork>,
    /// The battery floor for light work. `None` inherits.
    ///
    /// **`Some(0)` is how an owner says *any* charge will do**, which is the one thing `None`
    /// cannot mean here — `None` is already spoken for by "inherit". A floor of zero refuses
    /// nothing, because the check is `have < need`.
    #[serde(default)]
    pub min_battery_percent: Option<u8>,
    /// Whether light work may run on a link that costs money. `None` inherits.
    #[serde(default)]
    pub allow_metered: Option<bool>,
}

impl LightWork {
    /// Does this say anything at all? `false` is the default and means "ask the ordinary
    /// questions", which is what every existing config means.
    #[must_use]
    pub fn is_stated(&self) -> bool {
        self.accept.is_some() || self.min_battery_percent.is_some() || self.allow_metered.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPolicy {
    pub accept: AcceptWork,
    /// The owner's flat ceiling: never more than this many runs here, whatever they cost.
    pub max_concurrent_runs: u32,
    /// How many [`Demand::shares`] this device will have committed at once.
    ///
    /// `None` — the default, and what every existing config says — means
    /// `Normal × max_concurrent_runs`, so a fleet that never mentions demand behaves exactly
    /// as it did when capacity was a count. Setting it is the owner saying something the count
    /// cannot: *at most half this machine*, in the units the runs themselves are declared in.
    #[serde(default)]
    pub budget_shares: Option<u32>,
    /// Runs this device will have going on one agent *account* at once, across the whole fleet.
    ///
    /// Policy rather than capability, and gossiped for the same reason the rest of this struct
    /// is: enforcing an account's ceiling means every node on that account has to be able to
    /// see it. `None` is "no opinion", which is the honest default — nobody here knows what an
    /// account's rate limit actually is, and inventing one would refuse work on a guess.
    #[serde(default)]
    pub max_concurrent_account: Option<u32>,
    /// Refuse new work below this charge. Ignored on mains.
    pub min_battery_percent: Option<u8>,
    pub allow_metered: bool,
    /// `None` means any agent this node has.
    pub allowed_agents: Option<BTreeSet<AgentKind>>,
    /// The largest workspace **archive** this device will take, in bytes (ADR-0061 §4).
    ///
    /// A *tightening* knob and only that. `None` is "no opinion beyond the fleet's own limit" —
    /// never "no limit", because there has always been one: `MAX_BLOB_BYTES` is the protocol's,
    /// every node enforces it, and it exists because a peer's announced size is an allocation
    /// request. The ADR said the opposite of this until the blob plane was measured, and the
    /// reasoning it gave — a limit invented for a case nobody has hit is a knob nobody
    /// understands — was sound and aimed at the wrong ceiling.
    ///
    /// So this is for the fleet that has a metered link or a small disk, and costs nothing to
    /// the fleet that does not. A value **above** the structural cap is refused where the config
    /// is read, not silently clamped: an owner who wrote a bigger number believes something
    /// about their fleet that is not true, and clamping would leave them believing it.
    ///
    /// Read by the node it belongs to, at bid time, about its own willingness. Gossiped because
    /// everything in this struct is, and acted on by nobody else.
    #[serde(default)]
    pub max_archive_bytes: Option<u64>,
    /// The same three standing doors, asked again for light work (ADR-0019 §4).
    ///
    /// Stated by nobody until an owner writes `[policy.light]`, and then it is the sentence
    /// this whole phase exists for: *take the light watcher always, heavy work only while
    /// charging.*
    #[serde(default)]
    pub light: LightWork,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("node is not accepting work")]
    NotAcceptingWork,
    #[error("node accepts work only while charging")]
    NotCharging,
    /// The same policy, on a device that cannot see its own power at all. Refused all the same —
    /// unknown is not good news — but it must not *say* the device is not charging. Measured on a
    /// Samsung phone under Termux: Android 16 denies an app `/sys/class/power_supply`, so a phone that
    /// was plugged in was told it "accepts work only while charging", which sends its owner to
    /// check a cable that is fine.
    #[error("node accepts work only while charging, and cannot tell whether it is: its power source is unknown")]
    ChargingUnknown,
    #[error("battery at {have}%, policy floor is {need}%")]
    BatteryTooLow { have: u8, need: u8 },
    #[error("network is metered and policy disallows it")]
    MeteredNetwork,
    #[error("at capacity: {running}/{max} runs")]
    AtCapacity { running: u32, max: u32 },
    /// The count has room and the budget does not — one heavy run, or several light ones,
    /// already account for the machine. Kept apart from [`Refusal::AtCapacity`] because
    /// "2/2 runs" and "8/8 shares with a heavy run on it" send somebody to different places.
    #[error("budget full: {committed}/{budget} shares committed, this run wants {wanted}")]
    BudgetFull {
        committed: u32,
        budget: u32,
        wanted: u32,
    },
    /// The budget says there is room and the machine says otherwise. Declared demand is a
    /// hint, observed load is the truth, and admission takes the harsher of the two — a node
    /// that accepts work on the strength of an optimistic declaration and then serves it badly
    /// is worse than one that declines (ADR-0013).
    #[error("cpu at {load_percent}%: {budget_free}% of the budget is free, the machine is not")]
    UnderPressure { load_percent: u8, budget_free: u8 },
    /// The device says it is hot (ADR-0068). The same kind of refusal as `UnderPressure`: what
    /// the device is going through, not a queue it owns, so it defers rather than commits.
    #[error(
        "the device is too hot: thermal status {status}, and this work is refused from {limit}"
    )]
    TooHot {
        status: crate::capacity::Thermal,
        limit: crate::capacity::Thermal,
    },
    #[error("policy does not allow agent {0}")]
    AgentNotAllowed(AgentKind),
    #[error("agent {0} is at its per-node concurrency limit")]
    AgentAtCapacity(AgentKind),
    /// The *account's* limit rather than this machine's. Nodes signed into one login share one
    /// rate limit, so a fleet that counted only per node would win bids and then spend the
    /// afternoon rate-limited. Distinct from [`Refusal::AgentAtCapacity`] because the two send
    /// somebody to different places: one is this device's own ceiling, the other is a number
    /// that no single device can fix.
    #[error("account is at its concurrency limit: {running}/{max} runs on this account")]
    AccountAtCapacity { running: u32, max: u32 },
    /// The account is rate-limited, and the agent said when it lifts (ADR-0029).
    ///
    /// Distinct from [`Refusal::AccountAtCapacity`] for that variant's own reason one step
    /// further out: a ceiling is a number somebody in this fleet chose and a rate limit is one
    /// nobody did, and the two send a person to different places — raise the cap, or wait. It
    /// carries the instant rather than a duration because it is compared against a clock in
    /// several places and a duration would be relative to whenever it was made.
    // No duration in the sentence: `Millis`'s `Display` renders a *duration*, so putting an
    // absolute instant through it prints "496117104h5m" — measured, in `offload status`, before
    // that line was taught to subtract. This crate has no clock and cannot do better, so the
    // readable version is built where there is one: `offload_node::supervisor::describe_refusal`.
    #[error("the account's rate limit is holding new runs until it lifts")]
    AccountRateLimited { until: crate::time::Millis },
}

impl WorkPolicy {
    /// Sensible starting point per device class. A phone opts in only while charging and
    /// takes one run at a time — but it *does* take work.
    #[must_use]
    pub fn for_class(class: DeviceClass) -> Self {
        match class {
            DeviceClass::Phone | DeviceClass::Tablet => WorkPolicy {
                accept: AcceptWork::WhenCharging,
                max_concurrent_runs: 1,
                budget_shares: None,
                max_concurrent_account: None,
                min_battery_percent: Some(40),
                allow_metered: false,
                allowed_agents: None,
                // **No class default states one**, and that is ADR-0019 §4's own decision: a
                // light form here would change what every existing phone does, and the ADR
                // refuses making `Light` bypass a gate by default. A device class can say what
                // a machine is *like*; only its owner can say what they trust it with.
                // No opinion by default, on every device class: the fleet's own
                // limit is the only one until an owner says otherwise.
                max_archive_bytes: None,
                light: LightWork::default(),
            },
            DeviceClass::Laptop => WorkPolicy {
                accept: AcceptWork::Always,
                max_concurrent_runs: 2,
                budget_shares: None,
                max_concurrent_account: None,
                min_battery_percent: Some(20),
                allow_metered: false,
                allowed_agents: None,
                // No opinion by default, on every device class: the fleet's own
                // limit is the only one until an owner says otherwise.
                max_archive_bytes: None,
                light: LightWork::default(),
            },
            _ => WorkPolicy {
                accept: AcceptWork::Always,
                max_concurrent_runs: 4,
                budget_shares: None,
                max_concurrent_account: None,
                min_battery_percent: None,
                allow_metered: true,
                allowed_agents: None,
                // No opinion by default, on every device class: the fleet's own
                // limit is the only one until an owner says otherwise.
                max_archive_bytes: None,
                light: LightWork::default(),
            },
        }
    }

    /// How many [`Demand::shares`] this device will have committed at once.
    ///
    /// Derived rather than stored when the owner said nothing, so the two numbers cannot
    /// disagree: a device that only ever said "two runs" has a budget of exactly two normal
    /// runs, and demand costs it nothing until something declares itself heavy.
    #[must_use]
    pub fn budget(&self) -> u32 {
        self.budget_shares.unwrap_or_else(|| {
            Demand::Normal
                .shares()
                .saturating_mul(self.max_concurrent_runs)
        })
    }

    /// The owner's standing yes-or-no for work of this demand, light form included.
    ///
    /// These three resolvers are `pub` on purpose: `offload policy` prints the owner's doors,
    /// and a report computed from a second reading of "does the light form apply" would be the
    /// bug this project keeps fixing. There is one place that decides, and both the gate and
    /// the report call it.
    #[must_use]
    pub fn accept_for(&self, demand: Demand) -> AcceptWork {
        match demand {
            Demand::Light => self.light.accept.unwrap_or(self.accept),
            _ => self.accept,
        }
    }

    /// The battery floor for work of this demand, or `None` for no floor at all.
    #[must_use]
    pub fn battery_floor_for(&self, demand: Demand) -> Option<u8> {
        match demand {
            Demand::Light => self
                .light
                .min_battery_percent
                .or(self.min_battery_percent)
                // A stated floor of zero refuses nothing, and saying so here rather than
                // leaving `Some(0)` to the comparison keeps the *report* honest: "any charge"
                // and "0%" are the same door and only one of them reads as a door.
                .filter(|need| *need > 0),
            _ => self.min_battery_percent,
        }
    }

    /// Whether work of this demand may run on a link that costs money.
    #[must_use]
    pub fn allows_metered_for(&self, demand: Demand) -> bool {
        match demand {
            Demand::Light => self.light.allow_metered.unwrap_or(self.allow_metered),
            _ => self.allow_metered,
        }
    }

    /// May this node host `agent` **at all** — the owner's standing answer, before anything
    /// about what the node is currently doing.
    ///
    /// The split matters because the two kinds of refusal are not interchangeable downstream, and
    /// [`crate::bid::evaluate`] says so in its own words: *"Being full is the one refusal that is
    /// not a no… Every other refusal here is a condition of the device, not a queue."* A queue
    /// empties, so a full node **commits** to the run and says when (ADR-0006). A standing "never"
    /// does not empty, and a node that reports one as the other commits to work it must never do.
    ///
    /// That is what `allowed_agents` did. It was checked *after* capacity, so a node whose owner
    /// had told it not to run this agent — but which happened to be full when it was asked —
    /// returned `AtCapacity`, bid, committed, and started the run when a slot freed. Measured:
    /// `admits` on a full desktop with `allowed_agents = ["something-else"]` returned
    /// `Err(AtCapacity { running: 1, max: 1 })`, and nothing between the bid and `start_run` asks
    /// again. ADR-0011's control, defeated by an ordering.
    ///
    /// The other reason this is separate: it is the whole of what a **fleet of one** has to ask.
    /// `server::place`'s no-cluster arm has no bid to refuse, so it asks these questions directly
    /// — and asked only two of them until ADR-0046.
    pub fn permits(
        &self,
        caps: &Capabilities,
        tier: Tier<'_>,
        demand: Demand,
    ) -> Result<(), Refusal> {
        match self.accept_for(demand) {
            AcceptWork::Never => return Err(Refusal::NotAcceptingWork),
            AcceptWork::WhenCharging if caps.power == crate::capability::PowerSource::Unknown => {
                return Err(Refusal::ChargingUnknown)
            }
            AcceptWork::WhenCharging if !caps.power.on_mains() => return Err(Refusal::NotCharging),
            _ => {}
        }

        if !caps.power.on_mains() {
            if let (Some(need), Some(have)) =
                (self.battery_floor_for(demand), caps.power.battery_percent())
            {
                if have < need {
                    return Err(Refusal::BatteryTooLow { have, need });
                }
            }
        }

        // `known_metered`, not "not known unmetered": this refusal claims the bytes cost
        // money, and the claim carries the burden (ADR-0045 §4). Refusing on `Unknown` would
        // stop every machine with no NetworkManager from hosting anything — a certain cost
        // paid to avoid a possible one, which is the same over-claim pointing the other way.
        if caps.metered_network.known_metered() && !self.allows_metered_for(demand) {
            return Err(Refusal::MeteredNetwork);
        }

        // …and the one clause here that is about the *agent* rather than the device, so it is
        // asked only of work that has one. A task is not an unlisted agent — it is not an
        // agent — and refusing it here would make `allowed_agents` a list that quietly bans
        // the cheap tier as well.
        if let (Some(allowed), Some(agent)) = (&self.allowed_agents, tier.agent()) {
            if !allowed.contains(agent) {
                return Err(Refusal::AgentNotAllowed(agent.clone()));
            }
        }

        Ok(())
    }

    /// May this node take on `agent` right now, given what it is already doing?
    ///
    /// Ordered cheapest and most-likely first, and every refusal stays distinguishable —
    /// capability match is necessary and not sufficient, and "phone on battery", "two runs
    /// already", "budget spoken for" and "machine is thrashing" send somebody to four
    /// different places.
    ///
    /// The last of those is the one that is not a statement about a *decision* this node made:
    /// the budget counts work this node accepted, and the load average counts everything —
    /// including the build its owner started by hand. That difference is why a pressured node
    /// does not commit to the run the way a full one does; see [`crate::bid::NoBid::Busy`].
    pub fn admits(
        &self,
        caps: &Capabilities,
        tier: Tier<'_>,
        demand: Demand,
        load: &NodeLoad,
        account: Option<&AccountUse>,
    ) -> Result<(), Refusal> {
        // Every standing "never" first, before any question about the queue. See [`permits`]
        // for why the order is load-bearing rather than tidy. The demand goes with it, because
        // since ADR-0019 §4 the standing answer can differ for light work.
        self.permits(caps, tier, demand)?;

        let capacity = Capacity::of(self);
        capacity.room_for(load.held, demand)?;

        // Observed pressure, and the one place it is checked. A second copy of a rule like
        // this is what had a plugged-in laptop refusing every checkpoint replica on a battery
        // floor that did not apply — the same shape of bug, and quieter here, because a node
        // that refuses everything under a load average nobody printed just looks unpopular.
        if let Some(idle) = load.idle_percent() {
            if idle < demand.needs_idle_percent() {
                return Err(Refusal::UnderPressure {
                    load_percent: 100u8.saturating_sub(idle),
                    budget_free: capacity.free_percent(load.held),
                });
            }
        }
        // Heat, beside load and for the same reason (ADR-0068): observed, local, and one place.
        if let Some(status) = load.thermal {
            let limit = demand.too_hot_from();
            if status >= limit {
                return Err(Refusal::TooHot { status, limit });
            }
        }

        // The agent's own ceiling, and the account's below it, are the other two clauses with
        // no referent for a task: `held_for_agent` counts runs on an agent, and there is no
        // agent to count. Skipped rather than answered with a zero — the owner's flat ceiling
        // and the demand budget above are what bound a task, which is ADR-0019 §2's decision
        // that a task's identity for concurrency is its service and not an account.
        if let Some(agent) = tier.agent() {
            match caps.agent_details(agent) {
                Some(a) if load.held_for_agent >= a.max_concurrent => {
                    return Err(Refusal::AgentAtCapacity(agent.clone()));
                }
                _ => {}
            }

            // Last, because it is the only one of these that is not a fact about this machine —
            // so a node that is itself full should say so rather than blaming the account.
            if let Some(account) = account {
                account.room_for_one_more(load.held_for_agent)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Drop-off policy: the holder vanished. Now what?
// ---------------------------------------------------------------------------

/// What we know about a node that used to be holding a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeObservation {
    pub status: NodeStatus,
    /// Rolling average of how long this node's absences have historically lasted. The
    /// single most useful input here: a laptop that always comes back in 90 seconds should
    /// be waited for; one that vanishes for hours should not.
    pub typical_absence: Option<Millis>,
    /// Absences observed so far. A node with no history gets the policy default.
    pub observed_absences: u32,
}

impl NodeObservation {
    #[must_use]
    pub fn unknown(status: NodeStatus) -> Self {
        NodeObservation {
            status,
            typical_absence: None,
            observed_absences: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReassignPolicy {
    /// Never move a run before this much silence, whatever else is true. Absorbs the
    /// ordinary case of a Wi-Fi handover or a laptop suspending for ten seconds.
    pub min_grace: Millis,
    /// Never wait longer than this before moving.
    pub max_grace: Millis,
    /// Used when the holder has no absence history.
    pub default_grace: Millis,
    /// Wait this proportion of the holder's typical absence, e.g. 150%.
    pub return_factor_percent: u64,
    /// A confirmed-dead node is a stronger signal than an unreachable one; shrink the wait.
    pub dead_factor_percent: u64,
    /// Nothing checkpointed yet, so moving costs the whole run — be more patient.
    pub no_checkpoint_factor_percent: u64,
    /// Cheap to restart from scratch, so be less patient.
    pub idempotent_factor_percent: u64,
    /// Added per previous attempt, so a run cannot ping-pong around the fleet.
    pub backoff_per_attempt: Millis,
    /// How long to keep a pinned run alive hoping its node returns.
    pub pinned_timeout: Millis,
}

impl Default for ReassignPolicy {
    fn default() -> Self {
        ReassignPolicy {
            min_grace: Millis::from_secs(15),
            max_grace: Millis::from_mins(10),
            default_grace: Millis::from_secs(45),
            return_factor_percent: 150,
            dead_factor_percent: 40,
            no_checkpoint_factor_percent: 200,
            idempotent_factor_percent: 50,
            backoff_per_attempt: Millis::from_secs(30),
            pinned_timeout: Millis::from_mins(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "hold")]
pub enum HoldReason {
    /// Still inside the grace window; the holder may yet come back and reclaim.
    AwaitingReturn { absent: Millis, grace: Millis },
    /// The holder is reachable again but has not reclaimed yet.
    HolderReturned,
    /// Nowhere else to put it, so waiting costs nothing.
    NoEligibleAlternative,
    /// Pinned runs wait for their node or die with it.
    PinnedToHolder { absent: Millis },
}

/// One wording, used by the log line and by `offload explain` alike.
///
/// A hold is the answer to "why has my run not moved yet", and that question is asked of a
/// running fleet by somebody who cannot read the source. The reason exists as a value so it
/// cannot drift from what was decided; the sentence exists here so two callers cannot word
/// the same decision differently.
impl std::fmt::Display for HoldReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HoldReason::AwaitingReturn { absent, grace } => write!(
                f,
                "its holder has been out of contact {absent}, and this run waits {grace} \
                 before moving"
            ),
            HoldReason::HolderReturned => {
                f.write_str("its holder is answering again and may yet reclaim it")
            }
            HoldReason::NoEligibleAlternative => {
                f.write_str("no other node could take it, so waiting costs nothing")
            }
            HoldReason::PinnedToHolder { absent } => write!(
                f,
                "it is pinned to its holder, which has been gone {absent}"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reassign")]
pub enum ReassignReason {
    /// It told us it was going. No need to wait.
    HolderDraining,
    HolderDead {
        absent: Millis,
    },
    GraceExpired {
        absent: Millis,
        grace: Millis,
    },
    /// The wait ran out early, because it was measured against the run's own deadline rather
    /// than against the holder's history (ADR-0013).
    ///
    /// Distinct from [`ReassignReason::GraceExpired`] on purpose: "it moved after 18 seconds
    /// when the policy says 45" is a bug report unless the answer says which of the two
    /// numbers was doing the deciding.
    DeadlinePressing {
        absent: Millis,
        grace: Millis,
        slack: Slack,
    },
}

impl std::fmt::Display for ReassignReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReassignReason::HolderDraining => {
                f.write_str("its holder said it was leaving, so there is nothing to wait for")
            }
            ReassignReason::HolderDead { absent } => {
                write!(f, "its holder has been gone {absent} and is presumed dead")
            }
            ReassignReason::GraceExpired { absent, grace } => write!(
                f,
                "its holder has been out of contact {absent}, past the {grace} this run waits"
            ),
            ReassignReason::DeadlinePressing {
                absent,
                grace,
                slack,
            } => write!(
                f,
                "its holder has been out of contact {absent}, and with the run {slack} its \
                 deadline allows only {grace} of waiting"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum ReassignDecision {
    /// Run is not orphaned; nothing to decide.
    NoAction,
    Hold {
        until: Millis,
        reason: HoldReason,
    },
    Reassign {
        reason: ReassignReason,
    },
    Abandon {
        reason: String,
    },
}

/// Decide what to do about a run whose holder has dropped off.
///
/// `eligible_alternatives` is how many *other* nodes could host this run right now.
/// Deliberately an input rather than something this function computes: it keeps the
/// decision pure and lets the caller apply whatever eligibility rules are current.
#[must_use]
pub fn decide_reassignment(
    run: &Run,
    holder: &NodeObservation,
    eligible_alternatives: usize,
    now: Millis,
    policy: &ReassignPolicy,
) -> ReassignDecision {
    let RunState::Orphaned { since, .. } = run.state else {
        return ReassignDecision::NoAction;
    };
    let absent = now.saturating_sub(since);

    // A node that announced its departure is not coming back. Don't wait on it.
    if holder.status == NodeStatus::Draining || holder.status == NodeStatus::Departed {
        return ReassignDecision::Reassign {
            reason: ReassignReason::HolderDraining,
        };
    }

    if run.spec.restartability == Restartability::Pinned {
        return if absent >= policy.pinned_timeout {
            ReassignDecision::Abandon {
                reason: format!(
                    "pinned to node {} which has been gone {absent}",
                    run.holder().map(|n| n.short()).unwrap_or_default()
                ),
            }
        } else {
            ReassignDecision::Hold {
                until: since + policy.pinned_timeout,
                reason: HoldReason::PinnedToHolder { absent },
            }
        };
    }

    // The holder is answering again. Give it a moment to reclaim rather than racing it —
    // reclaiming costs nothing and migrating costs a turn.
    if holder.status == NodeStatus::Alive {
        return ReassignDecision::Hold {
            until: now + policy.min_grace,
            reason: HoldReason::HolderReturned,
        };
    }

    // Nothing to move it to, so patience is free. Keep waiting and say why.
    if eligible_alternatives == 0 {
        return ReassignDecision::Hold {
            until: now + policy.min_grace,
            reason: HoldReason::NoEligibleAlternative,
        };
    }

    let grace = grace_for(run, holder, policy, now);

    if absent >= grace.window {
        let reason = if holder.status == NodeStatus::Dead {
            ReassignReason::HolderDead { absent }
        } else if grace.bounded_by_deadline {
            ReassignReason::DeadlinePressing {
                absent,
                grace: grace.window,
                slack: run.slack(now),
            }
        } else {
            ReassignReason::GraceExpired {
                absent,
                grace: grace.window,
            }
        };
        ReassignDecision::Reassign { reason }
    } else {
        ReassignDecision::Hold {
            until: since + grace.window,
            reason: HoldReason::AwaitingReturn {
                absent,
                grace: grace.window,
            },
        }
    }
}

/// How long a run may wait for its holder, and what set that.
///
/// The flag is not decoration: the two numbers that could have decided this — how flaky the
/// holder is, and how much time the run has left — produce the same wait and completely
/// different answers to "why did it move so soon".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grace {
    pub window: Millis,
    /// The run's deadline cut the wait short. Absent that, patience would have been longer.
    pub bounded_by_deadline: bool,
}

/// How long to wait for this particular holder, for this particular run.
///
/// Two inputs that pull in opposite directions, and ADR-0013 is explicit that they compose
/// rather than replace each other: **absence history says how flaky this node is, and the
/// deadline says how much of that flakiness this run can afford.**
///
/// Only a deadline somebody *stated* shortens the wait. A run with none is overdue from its
/// second second — that is what makes urgency rise for free — and letting that shorten the
/// hold-down would move every ordinary run 15 seconds after a Wi-Fi handover, which is the
/// migration thrash ADR-0007 exists to prevent. Pickiness and patience are not the same knob;
/// this one costs a turn.
#[must_use]
pub fn grace_for(
    run: &Run,
    holder: &NodeObservation,
    policy: &ReassignPolicy,
    now: Millis,
) -> Grace {
    let mut grace = match holder.typical_absence {
        Some(typical) if holder.observed_absences > 0 => {
            typical.scaled_percent(policy.return_factor_percent)
        }
        _ => policy.default_grace,
    };

    if holder.status == NodeStatus::Dead {
        grace = grace.scaled_percent(policy.dead_factor_percent);
    }

    match run.spec.restartability {
        // Nothing captured yet: moving now throws away everything the agent has done.
        Restartability::Resumable if run.checkpoint.is_none() => {
            grace = grace.scaled_percent(policy.no_checkpoint_factor_percent);
        }
        Restartability::Idempotent => {
            grace = grace.scaled_percent(policy.idempotent_factor_percent);
        }
        _ => {}
    }

    // Anti-thrash: each previous move makes us slower to move again.
    grace = grace
        + policy
            .backoff_per_attempt
            .saturating_mul(u64::from(run.attempts.saturating_sub(1)));

    grace = grace.clamp_range(policy.min_grace, policy.max_grace);

    let Some(_) = run.spec.deadline else {
        return Grace {
            window: grace,
            bounded_by_deadline: false,
        };
    };

    // What is left is all the waiting this run can pay for. An overdue run gets `min_grace`
    // rather than nothing: below zero the honest move is the earliest available placement, and
    // "I am already late, so move it this instant" would spend a turn to save a second.
    //
    // The floor is where a scheduling hint stops being allowed to decide things. `min_grace`
    // absorbs a suspend and a Wi-Fi handover, and a deadline may change *when we give up*,
    // never *what we may do*: no deadline, however close, buys a migration that races a holder
    // still on its way back.
    let affordable = run
        .slack(now)
        .remaining()
        .unwrap_or(policy.min_grace)
        .clamp_range(policy.min_grace, grace);

    Grace {
        window: affordable,
        bounded_by_deadline: affordable < grace,
    }
}

/// Why this node has nothing to say about a run.
///
/// Every one of these is a *correct* reason to stay out of it, and they are distinguishable
/// because "why did nobody move my run" is the same question as "why is my run still
/// pending", asked about the other end of the run's life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "bystanding")]
pub enum Bystanding {
    /// It finished. Nothing to supervise.
    Terminal,
    /// Nobody is holding it, so there is no holder to have lost — and nothing here will offer
    /// it, which is [`Supervision::Place`]'s job and not this one's.
    ///
    /// This doc used to say "a `Pending` run is waiting for a bid round", which was true when
    /// no round was ever held without a person, and became the description of the *other*
    /// branch the moment ADR-0014's queued runs were placed by this loop. `offload explain`
    /// had taken the sentence from here, so it told an operator a round was coming for a run
    /// this arm exists to leave alone.
    Unheld,
    /// We are holding it ourselves — this loop is about *other* nodes going quiet.
    HeldHere,
    /// Somebody else arbitrates this run. Carries who, because the whole point of the rule
    /// is that every node computes the same answer.
    ArbitratedBy(Option<NodeId>),
    /// The holder is answering and has not lost its lease. The overwhelmingly common case.
    HolderPresent,
}

/// What this node should do about one run, having looked at the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "supervision")]
pub enum Supervision {
    Bystander(Bystanding),
    /// Record that the holder is out of contact. An **observation, not a decision**
    /// (ADR-0007): it grants no authority, and a returning holder reclaims at the same epoch.
    Orphan,
    /// It is already orphaned, and this is what the hold-down makes of that.
    Decided(ReassignDecision),
    /// Nobody holds it and this node arbitrates it: offer it to the fleet again.
    ///
    /// Carries *why*, because there are two reasons and they read differently to the person
    /// asking. A queued run has never been anybody's; a run a node let go was somewhere a
    /// minute ago and its operator was told so. Same effect, two sentences — and a report that
    /// takes the effect and guesses the sentence is the mistake `docs/pitfalls/reports-and-cli.md`
    /// is largely about.
    Place(Offering),
}

/// Why a run is being offered to the fleet again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "offering")]
pub enum Offering {
    /// It was submitted with `--queue` and nobody has ever taken it.
    ///
    /// The half of ADR-0014's opt-in that makes it worth having. "Leave it pending and let
    /// somebody pick it up when things change" is a promise about *later*, and later does not
    /// arrive on its own — a run refused at 23:00 because the desktop was off is a run
    /// somebody has to offer again at 07:00. Bounded by the caller's own backoff, because the
    /// fleet's answer does not change in a second.
    Queued,
    /// A run was assigned to a node, is not any more, and nobody has picked it up (ADR-0042).
    ///
    /// **Not conditioned on `--queue`**, and it must not be: `--queue` is an answer to "may a
    /// submission be filed away rather than accepted to my face", asked of a run that has never
    /// run. This run got as far as being *assigned*, and carrying on with it somewhere else is
    /// the promise the product is named for rather than a decision taken on the operator's
    /// behalf.
    ///
    /// `by` is for naming the node, never for saying what it meant: it is whoever the run was
    /// last assigned to, which on a bid round's give-back is a node that never held it. See
    /// [`crate::Run::release`].
    LetGo { by: NodeId },
}

// ---------------------------------------------------------------------------
// A commitment this node made, and whether it is still worth keeping
// ---------------------------------------------------------------------------

/// What a node should do about a run it accepted and has not been able to start.
///
/// The other side of accepting-without-starting (ADR-0006). A commitment is a promise about a
/// slot, and the promise can go stale: the run is due, this node still has no room, and
/// somewhere else in the fleet is a machine that could start it now. Sitting on it then is the
/// babysitting this project exists to remove — `ps` says `assigned`, the lease renews, and
/// nothing is wrong except that the work is not happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "commitment")]
pub enum Commitment {
    /// Keep waiting for room here.
    Keep(KeepReason),
    /// Hand it back to the pool: this node cannot make the start it promised, and somebody
    /// else can. Never a cancellation — a deadline decides when we give up, not what happens
    /// to the work (ADR-0013).
    GiveBack { overdue_by: Millis },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "keep")]
pub enum KeepReason {
    /// Nobody stated a deadline, so there is nothing to have missed.
    ///
    /// **The trap, and the reason this is the first test.** An unspecified deadline means the
    /// moment of submission, so every held run in the fleet is overdue within a second of
    /// being accepted — a rule that gave those runs back would bounce every commitment
    /// anywhere from one queue to another for ever, at a bid round each. Same distinction as
    /// `grace_for`: ordering uses slack whatever its origin, giving up needs a deadline
    /// somebody actually stated.
    NoStatedDeadline,
    /// Still time to make it.
    TimeLeft(Slack),
    /// Nowhere better for it to go: no other node is both eligible and able to start it, so
    /// giving it back buys a bid round and changes nothing. The same reasoning as
    /// [`HoldReason::NoEligibleAlternative`], asked from the other end.
    NobodyElseCouldStartIt,
    /// A pinned run has exactly one holder for life — `Run::assign` refuses a second attempt —
    /// so releasing this one would not re-place it, it would make it unplaceable.
    PinnedHere,
}

impl std::fmt::Display for KeepReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeepReason::NoStatedDeadline => f.write_str("no deadline was stated for it"),
            KeepReason::TimeLeft(slack) => write!(f, "there is still {slack}"),
            KeepReason::NobodyElseCouldStartIt => {
                f.write_str("no other node could start it either")
            }
            KeepReason::PinnedHere => f.write_str("it is pinned to this node"),
        }
    }
}

/// Parked: nobody holds it, no node let it go, and it has a checkpoint — the run `offload
/// checkpoint` leaves, which nothing moves but `offload resume` (ADR-0042).
///
/// A fact about the **record**, so every node answers it alike. [`supervise`] reaches the same
/// answer only on the arbiter — elsewhere it says who arbitrates — and `offload explain` asking
/// supervision instead told alpha's reader *parked* and bravo's reader *waiting for a node* about
/// one run (session ninety). The test below pins the two together on the arbiter.
#[must_use]
pub fn parked(run: &Run) -> bool {
    !run.state.is_terminal()
        && run.holder().is_none()
        && run.let_go_by().is_none()
        && run.checkpoint.is_some()
}

/// Should this node give back a run it committed to and has not started?
///
/// `ready_elsewhere` is how many *other* nodes look able to start it right now — eligible by
/// the run's constraints, and with room by their gossiped capacity. An estimate, deliberately:
/// it is read from state that is a tick old, and a node that has just taken something else
/// will decline. Being wrong there costs a bid round; being unwilling to estimate at all costs
/// the run its night.
///
/// **It does not anticipate.** Giving a commitment back *before* the deadline would need an
/// estimate of when this node's own slot frees, and `Availability::WhenFree` carries a count
/// rather than an ETA precisely because nothing here has learned to guess. Overdue is the
/// first moment this node can *know* that what it promised is worth nothing.
#[must_use]
pub fn review_commitment(run: &Run, ready_elsewhere: usize, now: Millis) -> Commitment {
    if run.spec.deadline.is_none() {
        return Commitment::Keep(KeepReason::NoStatedDeadline);
    }
    if run.spec.restartability == Restartability::Pinned {
        return Commitment::Keep(KeepReason::PinnedHere);
    }
    let slack = run.slack(now);
    if slack.remaining().is_some() {
        return Commitment::Keep(KeepReason::TimeLeft(slack));
    }
    if ready_elsewhere == 0 {
        return Commitment::Keep(KeepReason::NobodyElseCouldStartIt);
    }
    Commitment::GiveBack {
        overdue_by: Millis(u64::try_from(-slack.0).unwrap_or(0)),
    }
}

/// The whole of the drop-off loop that is a decision, as a pure function of (view, run, now).
///
/// The daemon's supervision pass runs on *every* node once a second and does something on
/// exactly one of them, which is a great deal of trust to place in code that is hard to test.
/// Everything here — who arbitrates, whether the holder counts as gone, how long to wait —
/// is decided from the view, so the interesting cases can be arranged rather than provoked.
/// The caller is left with the effects: orphan and publish, or run a bid round.
///
/// `arbiter_for` is what makes this work with the run's home node dead: arbitration moves to
/// a deterministic successor, so a run outlives the machine somebody submitted it from.
#[must_use]
pub fn supervise(
    view: &ClusterView,
    run: &Run,
    now: Millis,
    policy: &ReassignPolicy,
) -> Supervision {
    if run.state.is_terminal() {
        return Supervision::Bystander(Bystanding::Terminal);
    }
    if !view.is_local_arbiter(run) {
        return Supervision::Bystander(Bystanding::ArbitratedBy(view.arbiter_for(run)));
    }
    let Some(holder) = run.holder() else {
        // Nobody holds it, so there is no absence to reason about — the question is whether
        // anybody should be *offered* it. Two reasons say yes, and until ADR-0042 there was one.
        //
        // The one that used to be missing is asked first because it is the more specific fact:
        // a node let this run go on its way out, so nobody is coming back for it. A run in that
        // state used to fall through to the `else`, on a premise that is right about every
        // other way of reaching it — that a checkpointed `Pending` run was parked by a person
        // with `offload checkpoint`, and picking it up on their behalf is exactly the "we
        // decided for you" this project refuses to do. Right for the parked run, and it left
        // the drained one stopped in front of a fleet that was bidding to continue it.
        return match run.let_go_by() {
            Some(by) => Supervision::Place(Offering::LetGo { by }),
            None if run.spec.queue && run.checkpoint.is_none() => {
                Supervision::Place(Offering::Queued)
            }
            None => Supervision::Bystander(Bystanding::Unheld),
        };
    };
    if holder == view.local {
        return Supervision::Bystander(Bystanding::HeldHere);
    }

    let observation = view.node(&holder).map_or_else(
        || NodeObservation::unknown(NodeStatus::Dead),
        NodeView::observation,
    );

    if !matches!(run.state, RunState::Orphaned { .. }) {
        // Three ways to lose a holder, and the lease is the one that does not depend on the
        // failure detector agreeing with us.
        let gone = observation.status.is_gone()
            || observation.status == NodeStatus::Suspect
            || run.lease_expired(now);
        return if gone {
            Supervision::Orphan
        } else {
            Supervision::Bystander(Bystanding::HolderPresent)
        };
    }

    // How many *other* nodes could take it. Zero means waiting costs nothing, which is a
    // different answer from "the holder might come back" and reads differently in a log.
    let eligible = run.spec.eligibility(now);
    let alternatives = view
        .alive()
        .filter(|n| n.id != holder)
        .filter(|n| eligible.matches_on(n.id, &n.capabilities))
        .count();

    Supervision::Decided(decide_reassignment(
        run,
        &observation,
        alternatives,
        now,
        policy,
    ))
}

#[cfg(test)]
mod tests {
    /// The spelling a report prints has to be the spelling the config accepts.
    ///
    /// `offload policy` printed `{:?}` — `WhenCharging` — and a config saying that is refused
    /// with *"unknown variant `WhenCharging`, expected one of `never`, `when_charging`,
    /// `always`"*. Asserted against **serde's own** rendering rather than against three literals,
    /// so a renamed variant or a changed `rename_all` cannot leave the two disagreeing again.
    #[test]
    fn the_accept_spelling_a_report_prints_is_the_one_a_config_accepts() {
        for accept in [
            super::AcceptWork::Never,
            super::AcceptWork::WhenCharging,
            super::AcceptWork::Always,
        ] {
            let configured = serde_json::to_string(&accept).expect("encode");
            assert_eq!(
                format!("\"{accept}\""),
                configured,
                "the Display and the config spelling have come apart"
            );
        }
    }

    use super::*;
    use crate::capability::{Arch, Os, PowerSource};
    use crate::run::{AgentWork, Work};
    // Used only here since `admits_start` went: the start gate lives on `Room` now, and these
    // tests ask it through `may_start` below.
    use crate::capacity::Occupancy;
    use crate::constraint::Constraint;
    use crate::id::{NodeId, RunId};
    use crate::run::{PermissionMode, RunSpec, WorkspaceSpec};

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    /// A run this node accepted and has not started: `Assigned`, holding a lease, no agent.
    fn held_run() -> Run {
        let mut run = orphaned_run(Restartability::Resumable, Millis(0));
        run.state = RunState::Assigned {
            lease: crate::run::Lease {
                node: node(1),
                epoch: run.epoch,
                expires_at: Millis::from_mins(1),
            },
        };
        run
    }

    fn orphaned_run(restartability: Restartability, since: Millis) -> Run {
        let mut run = Run::new(
            RunId::from_bytes([1; 16]),
            RunSpec {
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
                    permission_mode: PermissionMode::Ask,
                    allow: crate::ToolAllowlist::default(),
                    max_turns: None,
                    ask: crate::AskPolicy::Never,
                }),
                constraint: Constraint::Always,
                restartability,
                priority: 0,
                queue: false,
                deadline: None,
                demand: Demand::Normal,
                notify: Default::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: crate::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            node(9),
            Millis(0),
        );
        let e = run
            .assign(node(1), Millis(0), Millis(10_000))
            .expect("assign");
        run.started(node(1), e, Millis(0)).expect("start");
        run.orphan(since).expect("orphan");
        run
    }

    /// A fleet where `local` is this node, every listed node is alive, and the run is held by
    /// `holder` on behalf of `home`. The four nodes the failover tests need, in one place.
    fn fleet(local: u8, home: u8, holder: u8, others: &[u8]) -> (ClusterView, Run) {
        let mut view = ClusterView::new(node(local));
        let mut ids: Vec<u8> = others.to_vec();
        ids.extend([local, home, holder]);
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            view.upsert_node(NodeView::new(
                node(id),
                Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
                WorkPolicy::for_class(DeviceClass::Desktop),
                Millis(0),
            ));
        }

        let mut run = orphaned_run(Restartability::Resumable, Millis(0));
        run.home = node(home);
        // Rebuild the lease against the holder these tests asked for.
        run.state = RunState::Running {
            lease: crate::Lease {
                node: node(holder),
                epoch: run.epoch,
                expires_at: Millis(60_000),
            },
            started_at: Millis(0),
        };
        (view, run)
    }

    fn set_status(view: &mut ClusterView, id: u8, status: NodeStatus, now: Millis) {
        view.nodes
            .get_mut(&node(id))
            .expect("known node")
            .set_status(status, now);
    }

    #[test]
    fn the_home_node_arbitrates_its_own_run_and_nobody_else_does() {
        let (view, run) = fleet(2, 1, 3, &[]);
        assert_eq!(
            supervise(&view, &run, Millis(1_000), &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::ArbitratedBy(Some(node(1))))
        );
    }

    #[test]
    fn a_run_outlives_the_node_that_submitted_it() {
        // The failover the deterministic successor exists for: the home node is gone, so it
        // cannot notice that the holder is gone too. Somebody has to, or the run sits on a
        // machine that no longer exists until a human looks — which is the babysitting this
        // project is here to remove.
        let now = Millis(600_000);
        let policy = ReassignPolicy::default();
        let (mut view, run) = fleet(2, 1, 3, &[4]);
        set_status(&mut view, 1, NodeStatus::Dead, now);
        set_status(&mut view, 3, NodeStatus::Dead, now);

        // Node 2 is the lowest-id node still alive, so arbitration lands on it.
        assert_eq!(supervise(&view, &run, now, &policy), Supervision::Orphan);

        // And node 4 stays out of it, computing the same successor from the same facts.
        let mut theirs = view.clone();
        theirs.local = node(4);
        assert_eq!(
            supervise(&theirs, &run, now, &policy),
            Supervision::Bystander(Bystanding::ArbitratedBy(Some(node(2))))
        );

        // Once the observation is written down, the hold-down decides — and with the holder
        // confirmed dead and somewhere else to put it, that is a move.
        let mut orphaned = run.clone();
        orphaned.orphan(Millis(1_000)).expect("orphan");
        assert_eq!(
            supervise(&view, &orphaned, now, &policy),
            Supervision::Decided(ReassignDecision::Reassign {
                reason: ReassignReason::HolderDead {
                    absent: Millis(599_000)
                }
            })
        );
    }

    #[test]
    fn a_suspected_home_node_keeps_arbitrating() {
        // `Suspect` is a peer's guess and the node can refute it (ADR-0007). Failing over on
        // it means the home node — which knows perfectly well it is alive — and the successor
        // both arbitrate the same run, both run a bid round, and the run is granted twice.
        // The epochs fence one of the agents afterwards; this avoids starting it at all.
        let now = Millis(600_000);
        let (mut view, run) = fleet(2, 1, 3, &[]);
        set_status(&mut view, 1, NodeStatus::Suspect, now);
        set_status(&mut view, 3, NodeStatus::Dead, now);

        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::ArbitratedBy(Some(node(1))))
        );
    }

    #[test]
    fn the_successor_does_not_supervise_a_run_it_is_holding_itself() {
        // Node 2 arbitrates because the home node is gone, and it is also the holder. There is
        // nothing to notice: a node that has lost contact with itself is not a case.
        let now = Millis(600_000);
        let (mut view, run) = fleet(2, 1, 2, &[3]);
        set_status(&mut view, 1, NodeStatus::Dead, now);

        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::HeldHere)
        );
    }

    #[test]
    fn an_expired_lease_orphans_a_run_the_detector_still_likes() {
        // The failure detector and the lease are two independent ways to notice the same
        // thing, and the lease is the one that does not need peers to agree.
        let (mut view, run) = fleet(1, 1, 3, &[]);
        set_status(&mut view, 3, NodeStatus::Alive, Millis(0));
        let policy = ReassignPolicy::default();

        assert_eq!(
            supervise(&view, &run, Millis(30_000), &policy),
            Supervision::Bystander(Bystanding::HolderPresent)
        );
        assert_eq!(
            supervise(&view, &run, Millis(60_001), &policy),
            Supervision::Orphan
        );
    }

    #[test]
    fn a_finished_run_is_nobodys_business() {
        let now = Millis(600_000);
        let (mut view, mut run) = fleet(2, 1, 3, &[]);
        set_status(&mut view, 1, NodeStatus::Dead, now);
        run.state = RunState::Completed { at: Millis(500) };

        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::Terminal)
        );
    }

    #[test]
    fn a_pending_run_has_no_holder_to_have_lost() {
        // Not the same as "nothing is wrong with it": a run whose home node died while it was
        // still pending is nobody's to place, and this is the state that says so.
        let now = Millis(600_000);
        let (mut view, mut run) = fleet(2, 1, 3, &[]);
        set_status(&mut view, 1, NodeStatus::Dead, now);
        run.state = RunState::Pending {
            since: Millis(0),
            let_go_by: None,
        };

        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::Unheld)
        );
    }

    /// `parked` and the arbiter's own supervision agree, both ways, so a report asking the
    /// first cannot tell a person something the second will not do.
    #[test]
    fn parked_is_what_the_arbiter_leaves_alone() {
        let now = Millis(600_000);
        let (view, mut run) = fleet(1, 1, 3, &[2]);
        run.state = RunState::Pending {
            since: Millis(0),
            let_go_by: None,
        };
        run.checkpoint = Some(crate::Checkpoint {
            session_id: None,
            transcript: crate::BlobHash::from_bytes([1; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f0f0f".into(),
            turns: 1,
            taken_at: Millis(1),
            agent_version: "1".into(),
            replicas: std::collections::BTreeSet::new(),
        });
        assert!(
            view.is_local_arbiter(&run),
            "the harness's local node arbitrates"
        );
        for queue in [false, true] {
            run.spec.queue = queue;
            assert!(parked(&run));
            assert_eq!(
                supervise(&view, &run, now, &ReassignPolicy::default()),
                Supervision::Bystander(Bystanding::Unheld)
            );
        }
        run.checkpoint = None;
        run.spec.queue = true;
        assert!(!parked(&run));
        assert!(matches!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Place(_)
        ));
    }

    #[test]
    fn a_queued_run_is_offered_again_by_whoever_arbitrates_it() {
        // ADR-0014's opt-in, and the half that makes it worth having: "leave it pending and
        // let somebody pick it up when things change" is a promise about later, and later is
        // not a state a run reaches on its own.
        let now = Millis(600_000);
        let (view, mut run) = fleet(1, 1, 3, &[]);
        run.state = RunState::Pending {
            since: Millis(0),
            let_go_by: None,
        };
        run.spec.queue = true;

        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Place(Offering::Queued)
        );

        // Only the arbiter offers it. Every node runs this loop, and two of them running a
        // round for one run is how it gets granted twice.
        let (elsewhere, _) = fleet(2, 1, 3, &[]);
        assert_eq!(
            supervise(&elsewhere, &run, now, &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::ArbitratedBy(Some(node(1))))
        );
    }

    #[test]
    fn a_run_a_human_parked_is_not_picked_up_on_their_behalf() {
        // `offload checkpoint` releases a run to `Pending` and means "I will resume this".
        // A queued run that has been *started* looks identical from the state machine, so the
        // checkpoint is what tells them apart — and deciding for somebody is exactly the
        // babysitting-in-reverse this project refuses.
        let now = Millis(600_000);
        let (view, mut run) = fleet(1, 1, 3, &[]);
        run.state = RunState::Pending {
            since: Millis(0),
            let_go_by: None,
        };
        run.spec.queue = true;
        run.checkpoint = Some(crate::run::Checkpoint {
            session_id: Some("s".into()),
            transcript: crate::BlobHash::from_bytes([1; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f".into(),
            turns: 3,
            taken_at: Millis(1_000),
            agent_version: "2.11.0".into(),
            replicas: BTreeSet::new(),
        });

        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::Unheld)
        );

        // …and the same run, released by the node instead of parked by the person, is the
        // opposite instruction (ADR-0042). Written here rather than in a test of its own
        // because the two cases differ in exactly one field and the pair is the point: this is
        // the assertion that would have gone red for the run a drain left stopped in front of a
        // fleet that was bidding to continue it.
        run.state = RunState::Pending {
            since: Millis(0),
            let_go_by: Some(node(2)),
        };
        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Place(Offering::LetGo { by: node(2) })
        );
    }

    #[test]
    fn a_run_a_node_let_go_is_offered_whether_or_not_it_was_queued() {
        // `--queue` answers "may a *submission* be filed away rather than accepted to my face"
        // (ADR-0014). This run was accepted, ran, and was let go by a node on its way out —
        // carrying on with it is the promise the product is named for, not a decision taken on
        // the operator's behalf, so the opt-in has nothing to say about it.
        let now = Millis(600_000);
        let (view, mut run) = fleet(1, 1, 3, &[]);
        run.spec.queue = false;
        run.state = RunState::Pending {
            since: Millis(0),
            let_go_by: Some(node(3)),
        };

        assert_eq!(
            supervise(&view, &run, now, &ReassignPolicy::default()),
            Supervision::Place(Offering::LetGo { by: node(3) })
        );

        // Still only the arbiter, which is the invariant a second offerer would break: two
        // nodes running a round for one run is how it gets granted twice, and the fact being
        // gossiped rather than node-local is precisely what lets exactly one of them act.
        let (elsewhere, _) = fleet(2, 1, 3, &[]);
        assert_eq!(
            supervise(&elsewhere, &run, now, &ReassignPolicy::default()),
            Supervision::Bystander(Bystanding::ArbitratedBy(Some(node(1))))
        );
    }

    fn phone() -> Capabilities {
        let mut c = Capabilities::empty(Os::Android, Arch::Aarch64, DeviceClass::Phone);
        c.power = PowerSource::Battery {
            percent: 80,
            charging: true,
        };
        c
    }

    fn desktop() -> Capabilities {
        Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop)
    }

    #[test]
    fn a_brief_absence_does_not_move_the_run() {
        // The whole point: Wi-Fi blips and short suspends must not cost a migration.
        let policy = ReassignPolicy::default();
        let run = orphaned_run(Restartability::Resumable, Millis(1_000));
        let obs = NodeObservation::unknown(NodeStatus::Suspect);

        let d = decide_reassignment(&run, &obs, 3, Millis(6_000), &policy);
        assert!(
            matches!(
                d,
                ReassignDecision::Hold {
                    reason: HoldReason::AwaitingReturn { .. },
                    ..
                }
            ),
            "expected hold, got {d:?}"
        );
    }

    #[test]
    fn a_long_absence_moves_the_run() {
        let policy = ReassignPolicy::default();
        let run = orphaned_run(Restartability::Idempotent, Millis(1_000));
        let obs = NodeObservation::unknown(NodeStatus::Dead);

        let d = decide_reassignment(&run, &obs, 3, Millis(600_000), &policy);
        assert!(matches!(d, ReassignDecision::Reassign { .. }), "got {d:?}");
    }

    #[test]
    fn draining_holders_are_not_waited_for() {
        let policy = ReassignPolicy::default();
        let run = orphaned_run(Restartability::Resumable, Millis(1_000));
        let obs = NodeObservation::unknown(NodeStatus::Draining);

        assert_eq!(
            decide_reassignment(&run, &obs, 1, Millis(1_100), &policy),
            ReassignDecision::Reassign {
                reason: ReassignReason::HolderDraining
            }
        );
    }

    #[test]
    fn nowhere_to_go_means_keep_waiting() {
        let policy = ReassignPolicy::default();
        let run = orphaned_run(Restartability::Idempotent, Millis(0));
        let obs = NodeObservation::unknown(NodeStatus::Dead);

        let d = decide_reassignment(&run, &obs, 0, Millis(9_999_999), &policy);
        assert!(
            matches!(
                d,
                ReassignDecision::Hold {
                    reason: HoldReason::NoEligibleAlternative,
                    ..
                }
            ),
            "got {d:?}"
        );
    }

    #[test]
    fn a_reliably_returning_node_earns_a_longer_wait() {
        let policy = ReassignPolicy::default();
        let now = Millis::from_secs(30);
        let run = orphaned_run(Restartability::Idempotent, Millis(0));

        let flaky_but_prompt = NodeObservation {
            status: NodeStatus::Suspect,
            typical_absence: Some(Millis::from_secs(90)),
            observed_absences: 12,
        };
        let unknown = NodeObservation::unknown(NodeStatus::Suspect);

        assert!(
            grace_for(&run, &flaky_but_prompt, &policy, now).window
                > grace_for(&run, &unknown, &policy, now).window,
            "history of prompt returns should buy patience"
        );
    }

    #[test]
    fn an_uncheckpointed_run_is_waited_for_longer_than_an_idempotent_one() {
        let policy = ReassignPolicy::default();
        let now = Millis::from_secs(30);
        let obs = NodeObservation::unknown(NodeStatus::Suspect);

        let precious = orphaned_run(Restartability::Resumable, Millis(0));
        let cheap = orphaned_run(Restartability::Idempotent, Millis(0));

        assert!(
            grace_for(&precious, &obs, &policy, now).window
                > grace_for(&cheap, &obs, &policy, now).window
        );
    }

    #[test]
    fn repeated_moves_slow_down() {
        let policy = ReassignPolicy::default();
        let now = Millis::from_secs(30);
        let obs = NodeObservation::unknown(NodeStatus::Suspect);

        let mut once = orphaned_run(Restartability::Idempotent, Millis(0));
        let mut thrashing = orphaned_run(Restartability::Idempotent, Millis(0));
        thrashing.attempts = 6;

        // Keep both under max_grace so the clamp doesn't hide the effect.
        once.attempts = 1;
        assert!(
            grace_for(&thrashing, &obs, &policy, now).window
                > grace_for(&once, &obs, &policy, now).window
        );
    }

    #[test]
    fn a_run_that_is_nearly_due_waits_less_for_its_holder() {
        // ADR-0013's added input, and the case it is for: the usual answer is "wait 45 seconds
        // for a laptop that is probably suspended", and a run due in twenty has to spend that
        // time landing somewhere rather than hoping.
        let policy = ReassignPolicy::default();
        let now = Millis::from_secs(40);
        let obs = NodeObservation::unknown(NodeStatus::Suspect);

        let mut run = orphaned_run(Restartability::Idempotent, Millis::from_secs(20));
        let patient = grace_for(&run, &obs, &policy, now).window;

        run.spec.deadline = Some(Millis::from_secs(60));
        let pressed = grace_for(&run, &obs, &policy, now);
        assert!(pressed.window < patient, "a deadline may shorten the wait");
        assert!(pressed.bounded_by_deadline);

        // And the reason says which of the two numbers decided, because "it moved after 20
        // seconds when the policy says 45" is otherwise a bug report.
        assert_eq!(
            decide_reassignment(&run, &obs, 3, Millis::from_secs(41), &policy),
            ReassignDecision::Reassign {
                reason: ReassignReason::DeadlinePressing {
                    absent: Millis::from_secs(21),
                    grace: Millis::from_secs(19),
                    slack: Slack(19_000),
                }
            }
        );
    }

    #[test]
    fn a_run_with_no_deadline_keeps_the_whole_hold_down() {
        // The trap in deriving urgency from the submission time: an unspecified deadline makes
        // *every* run overdue within a second, so a rule that let slack shorten the wait would
        // move every run in the fleet 15 seconds after a Wi-Fi handover — the thrash ADR-0007
        // exists to prevent, arrived at through the door marked "scheduling hint".
        let policy = ReassignPolicy::default();
        let obs = NodeObservation::unknown(NodeStatus::Suspect);
        let run = orphaned_run(Restartability::Idempotent, Millis::from_secs(1));

        let hours_later = Millis::from_mins(120);
        assert!(run.slack(hours_later).is_overdue());
        assert_eq!(
            grace_for(&run, &obs, &policy, hours_later),
            grace_for(&run, &obs, &policy, Millis::from_secs(2)),
            "patience must not depend on how long ago it was submitted"
        );
    }

    #[test]
    fn an_overdue_run_still_gets_the_minimum_grace() {
        // "Refuse to wait at all, I am already late" spends a turn to save a second — and it
        // is the reading that would let a deadline race a holder still on its way back, which
        // is where a hint becomes the double execution everything here exists to prevent.
        let policy = ReassignPolicy::default();
        let obs = NodeObservation::unknown(NodeStatus::Suspect);
        let mut run = orphaned_run(Restartability::Resumable, Millis::from_secs(1));
        run.spec.deadline = Some(Millis::from_secs(2));

        let grace = grace_for(&run, &obs, &policy, Millis::from_mins(60));
        assert_eq!(grace.window, policy.min_grace);
        assert!(grace.bounded_by_deadline);
    }

    #[test]
    fn pinned_runs_wait_then_die() {
        let policy = ReassignPolicy::default();
        let run = orphaned_run(Restartability::Pinned, Millis(0));
        let obs = NodeObservation::unknown(NodeStatus::Dead);

        assert!(matches!(
            decide_reassignment(&run, &obs, 5, Millis(60_000), &policy),
            ReassignDecision::Hold {
                reason: HoldReason::PinnedToHolder { .. },
                ..
            }
        ));
        assert!(matches!(
            decide_reassignment(&run, &obs, 5, Millis(60_000_000), &policy),
            ReassignDecision::Abandon { .. }
        ));
    }

    /// An idle node that will not say what its load is — the shape of every call that
    /// predates there being a budget at all.
    fn idle() -> NodeLoad {
        NodeLoad::default()
    }

    fn holding(runs: u32, shares: u32) -> NodeLoad {
        NodeLoad {
            held: Occupancy::new(runs, shares),
            ..NodeLoad::default()
        }
    }

    /// ADR-0046's ordering, and the only thing that holds it.
    ///
    /// A standing "never" must be reported before a queue, because the fleet acts on the
    /// difference: a full node **commits** to the run and says when (ADR-0006), so reporting a
    /// never as a not-yet is what makes a node promise work its owner forbade. `allowed_agents`
    /// was checked after capacity and did exactly that.
    ///
    /// **Nothing live can catch this, which is why the test is the whole of the enforcement.**
    /// `AgentKind::Other` exists at the type level and no config can produce it: `AllowedAgents`
    /// refuses every name but `claude-code` at deserialize time and refuses the empty list, and
    /// `Supervisor::build` writes `ClaudeCode` into every `RunSpec` — so the set a config can
    /// hold always contains the agent every run names, and `Refusal::AgentNotAllowed` is a
    /// sentence no daemon this build can start will ever say. Measured: the three shapes a config
    /// can carry are `["claude-code"]`, which starts, and `[]` and `["codex"]`, which are both
    /// refused at startup. The clause is not dead — it is waiting for the second adapter (phase
    /// 7) — and on the day it arrives, the ordering has to already be right, because that is the
    /// day it becomes reachable *and* unnoticeable.
    #[test]
    fn the_light_form_is_the_owners_and_says_nothing_by_default() {
        // ADR-0019 §4's headline sentence, and the reason the phase needs it: *take the light
        // watcher always, heavy work only while charging.* Unsayable before this.
        let on_battery = {
            let mut caps = desktop();
            caps.power = crate::capability::PowerSource::Battery {
                percent: 55,
                charging: false,
            };
            caps
        };
        let mut phone = WorkPolicy {
            accept: AcceptWork::WhenCharging,
            min_battery_percent: Some(40),
            ..WorkPolicy::for_class(DeviceClass::Phone)
        };

        // The control, and the promise this feature makes to every config already written:
        // saying nothing changes nothing, for every demand.
        assert!(!phone.light.is_stated());
        for demand in [Demand::Light, Demand::Normal, Demand::Heavy] {
            assert_eq!(
                phone.permits(&on_battery, Tier::Task, demand),
                Err(Refusal::NotCharging),
                "an unstated light form must not relax anything"
            );
        }

        phone.light.accept = Some(AcceptWork::Always);
        assert_eq!(
            phone.permits(&on_battery, Tier::Task, Demand::Light),
            Ok(())
        );
        assert_eq!(
            phone.permits(&on_battery, Tier::Task, Demand::Normal),
            Err(Refusal::NotCharging),
            "heavier work still answers to the door the owner left shut"
        );
        assert_eq!(
            phone.permits(&on_battery, Tier::Task, Demand::Heavy),
            Err(Refusal::NotCharging)
        );
    }

    #[test]
    fn a_light_battery_floor_is_inherited_until_it_is_stated_and_zero_means_any_charge() {
        // The one field where `None` is spoken for: it means *inherit*, so "any charge will
        // do" has to be sayable some other way, and a floor of zero refuses nothing.
        let nearly_flat = {
            let mut caps = desktop();
            caps.power = crate::capability::PowerSource::Battery {
                percent: 3,
                charging: false,
            };
            caps
        };
        let mut policy = WorkPolicy {
            accept: AcceptWork::Always,
            min_battery_percent: Some(40),
            ..WorkPolicy::for_class(DeviceClass::Laptop)
        };

        assert_eq!(policy.battery_floor_for(Demand::Light), Some(40));
        assert_eq!(
            policy.permits(&nearly_flat, Tier::Task, Demand::Light),
            Err(Refusal::BatteryTooLow { have: 3, need: 40 }),
            "inherited until stated"
        );

        policy.light.min_battery_percent = Some(0);
        assert_eq!(
            policy.battery_floor_for(Demand::Light),
            None,
            "a stated zero is no floor, and the report has to be able to say so"
        );
        assert_eq!(
            policy.permits(&nearly_flat, Tier::Task, Demand::Light),
            Ok(())
        );
        assert_eq!(
            policy.permits(&nearly_flat, Tier::Task, Demand::Normal),
            Err(Refusal::BatteryTooLow { have: 3, need: 40 })
        );
    }

    #[test]
    fn a_light_form_can_restrict_as_well_as_relax() {
        // Not the case anybody will write, and the reason the wire moved: if the light form
        // could only ever loosen a door, a peer that decoded it away would be wrong in a
        // direction nobody minds. It can tighten, so a dropped field can make a node look
        // willing to do work its owner refused — see the v28 note in `offload-proto`.
        let metered = {
            let mut caps = desktop();
            caps.metered_network = crate::capability::Metered::Yes;
            caps
        };
        let policy = WorkPolicy {
            accept: AcceptWork::Always,
            allow_metered: true,
            light: LightWork {
                allow_metered: Some(false),
                ..LightWork::default()
            },
            ..WorkPolicy::for_class(DeviceClass::Desktop)
        };

        assert_eq!(policy.permits(&metered, Tier::Task, Demand::Normal), Ok(()));
        assert_eq!(
            policy.permits(&metered, Tier::Task, Demand::Light),
            Err(Refusal::MeteredNetwork)
        );
        assert!(policy.light.is_stated());
    }

    #[test]
    fn a_task_is_not_an_unlisted_agent() {
        // `allowed_agents` is the one clause in `permits` that is about the *agent* rather
        // than the device, and `AgentKind::Other` means a config can name one this fleet has
        // never had. A task is not an agent that failed the list — it is not an agent — so
        // refusing it here would make the setting quietly ban the cheap tier as well, on the
        // one path (a fleet of one) that has no bid round to ask properly.
        let mut policy = WorkPolicy::for_class(DeviceClass::Desktop);
        policy.allowed_agents = Some([AgentKind::Other("something-else".into())].into());

        assert_eq!(
            policy.permits(
                &desktop(),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal
            ),
            Err(Refusal::AgentNotAllowed(AgentKind::ClaudeCode))
        );
        assert_eq!(
            policy.permits(&desktop(), Tier::Task, Demand::Normal),
            Ok(())
        );

        // …and the clauses that *are* about the device apply to both. Same policy, a phone
        // that has been told to work only while charging.
        let mut phone = WorkPolicy::for_class(DeviceClass::Phone);
        phone.accept = AcceptWork::WhenCharging;
        let caps = {
            let mut caps = desktop();
            caps.power = crate::capability::PowerSource::Battery {
                percent: 80,
                charging: false,
            };
            caps
        };
        assert_eq!(
            phone.permits(&caps, Tier::Task, Demand::Normal),
            Err(Refusal::NotCharging)
        );
    }

    #[test]
    fn an_agents_ceiling_and_an_accounts_are_asked_only_of_an_agent() {
        // The other two clauses with no referent for a task. `held_for_agent` counts runs on
        // an agent, so a task cannot be at an agent's ceiling — and what does bound a task is
        // the flat run count and the demand budget, which are checked above both of these.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        let at_agent_ceiling = NodeLoad {
            held: Occupancy::default(),
            held_for_agent: 99,
            cpu_percent: Some(0),
            thermal: None,
        };
        let account = AccountUse {
            account: crate::AccountId::of_account("acct"),
            elsewhere: 0,
            limit: 1,
        };
        // With the agent installed the agent's own ceiling is the first of the two to refuse.
        let with_agent = {
            let mut caps = desktop();
            caps.add(crate::capability::Capability::agent(
                AgentKind::ClaudeCode,
                crate::capability::AgentDetails {
                    version: "2.10.0".into(),
                    models: Vec::new(),
                    max_concurrent: 1,
                },
                true,
            ));
            caps
        };

        assert!(matches!(
            policy.admits(
                &with_agent,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &at_agent_ceiling,
                Some(&account)
            ),
            Err(Refusal::AgentAtCapacity(_))
        ));
        // …and without it, the account's ceiling is. Both are agent facts, and both are
        // skipped for a task rather than answered with the same numbers.
        assert!(matches!(
            policy.admits(
                &desktop(),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &at_agent_ceiling,
                Some(&account)
            ),
            Err(Refusal::AccountAtCapacity { .. })
        ));
        assert_eq!(
            policy.admits(
                &with_agent,
                Tier::Task,
                Demand::Normal,
                &at_agent_ceiling,
                Some(&account)
            ),
            Ok(()),
            "an agent's ceiling and an account's are not a task's"
        );
    }

    #[test]
    fn a_forbidden_agent_is_refused_before_a_full_node_is() {
        let mut policy = WorkPolicy::for_class(DeviceClass::Desktop);
        policy.max_concurrent_runs = 1;
        policy.allowed_agents = Some([AgentKind::Other("something-else".into())].into());
        let full = holding(1, Demand::Normal.shares());

        // The refusal that will still be true tomorrow, not the one that empties by itself.
        assert_eq!(
            policy.admits(
                &desktop(),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &full,
                None
            ),
            Err(Refusal::AgentNotAllowed(AgentKind::ClaudeCode)),
            "a node that will never run this agent must not answer with its queue"
        );

        // …and it is the standing half that says so, which is what the fleet-of-one door asks.
        assert_eq!(
            policy.permits(
                &desktop(),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal
            ),
            Err(Refusal::AgentNotAllowed(AgentKind::ClaudeCode))
        );

        // The control: with the agent allowed, being full is still a queue and still reported
        // as one. Hoisting the standing clauses must not have hidden the capacity answer.
        policy.allowed_agents = Some([AgentKind::ClaudeCode].into());
        assert_eq!(
            policy.permits(
                &desktop(),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal
            ),
            Ok(()),
            "the only set a config can express is the one that permits"
        );
        assert!(matches!(
            policy.admits(
                &desktop(),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &full,
                None
            ),
            Err(Refusal::AtCapacity { .. })
        ));
    }

    /// ADR-0068: heat gates accepting the way load does, graduated by demand, and a device whose
    /// host says nothing is not refused for it.
    #[test]
    fn a_hot_device_refuses_from_severe_and_heavy_work_one_step_earlier() {
        use crate::capacity::Thermal;
        let policy = WorkPolicy::for_class(DeviceClass::Laptop);
        let caps = phone();
        let at = |thermal: Option<Thermal>, demand: Demand| {
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                demand,
                &NodeLoad { thermal, ..idle() },
                None,
            )
        };
        assert_eq!(at(None, Demand::Heavy), Ok(()), "unreported is not refused");
        assert_eq!(at(Some(Thermal::Moderate), Demand::Normal), Ok(()));
        assert_eq!(
            at(Some(Thermal::Severe), Demand::Normal),
            Err(Refusal::TooHot {
                status: Thermal::Severe,
                limit: Thermal::Severe
            })
        );
        assert_eq!(
            at(Some(Thermal::Moderate), Demand::Heavy),
            Err(Refusal::TooHot {
                status: Thermal::Moderate,
                limit: Thermal::Moderate
            })
        );
        assert!(at(Some(Thermal::Severe), Demand::Light).is_err());
        assert_eq!(Thermal::from_android(3), Some(Thermal::Severe));
        assert_eq!(
            Thermal::from_android(9),
            None,
            "an unknown status is no answer"
        );
    }

    #[test]
    fn a_phone_takes_work_while_charging_and_declines_on_battery() {
        let policy = WorkPolicy::for_class(DeviceClass::Phone);
        let mut caps = phone();

        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &idle(),
                None
            ),
            Ok(())
        );

        caps.power = PowerSource::Battery {
            percent: 80,
            charging: false,
        };
        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &idle(),
                None
            ),
            Err(Refusal::NotCharging)
        );

        // …and a phone that cannot see its power is refused too, but not told it is unplugged:
        // measured on a Samsung phone under Termux, where Android denies `/sys/class/power_supply`.
        caps.power = PowerSource::Unknown;
        let refused = policy.admits(
            &caps,
            Tier::Agent(&AgentKind::ClaudeCode),
            Demand::Normal,
            &idle(),
            None,
        );
        assert_eq!(refused, Err(Refusal::ChargingUnknown));
        assert!(
            refused
                .as_ref()
                .is_err_and(|r| r.to_string().contains("cannot tell")),
            "{refused:?}"
        );
    }

    #[test]
    fn capacity_and_metering_are_reported_distinctly() {
        let mut policy = WorkPolicy::for_class(DeviceClass::Phone);
        policy.accept = AcceptWork::Always;
        let mut caps = phone();

        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &holding(1, Demand::Normal.shares()),
                None,
            ),
            Err(Refusal::AtCapacity { running: 1, max: 1 })
        );

        // A link somebody could actually say costs money. Reachable at all only since
        // ADR-0045 — the probe reported `false` on every device, so this refusal was a
        // sentence no fleet could produce.
        caps.metered_network = crate::Metered::Yes;
        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &idle(),
                None
            ),
            Err(Refusal::MeteredNetwork)
        );

        // …and `Unknown` does not refuse. The refusal claims the bytes cost money and the claim
        // carries the burden (ADR-0045 §4); refusing here would stop every machine with no
        // NetworkManager from hosting anything, to avoid a cost nobody has established.
        caps.metered_network = crate::Metered::Unknown;
        assert!(policy
            .admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &idle(),
                None
            )
            .is_ok());
    }

    #[test]
    fn a_machine_that_is_busy_refuses_even_though_its_budget_says_it_is_free() {
        // The case counting runs cannot see, and the whole argument for measuring: nothing
        // accepted here, so every declared number says yes — and the machine is compiling
        // something its owner started by hand.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        let caps = desktop();
        let thrashing = NodeLoad {
            cpu_percent: Some(90),
            ..NodeLoad::default()
        };

        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &thrashing,
                None
            ),
            Err(Refusal::UnderPressure {
                load_percent: 90,
                budget_free: 100
            })
        );
        // ...while a light run still gets in, because it wants a tenth of the machine rather
        // than a quarter of it. Demand is what makes pressure a scale rather than a gate.
        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Light,
                &thrashing,
                None
            ),
            Ok(())
        );
    }

    #[test]
    fn a_platform_that_will_not_report_load_is_not_reported_as_idle() {
        // `None` has to skip the rule rather than satisfy it. The alternative — treating an
        // unavailable load average as zero — is the over-claiming the probe exists not to do,
        // and it would arrive here as a node that wins every bid.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        assert_eq!(
            policy.admits(
                &desktop(),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Heavy,
                &idle(),
                None
            ),
            Ok(())
        );
    }

    #[test]
    fn a_commitment_is_given_back_when_it_is_late_and_somebody_else_could_start_it() {
        let now = Millis::from_mins(60);
        let mut run = held_run();
        run.spec.deadline = Some(Millis::from_mins(30));

        assert_eq!(
            review_commitment(&run, 1, now),
            Commitment::GiveBack {
                overdue_by: Millis::from_mins(30)
            }
        );
        // ...and kept when it would only move from one queue to another.
        assert_eq!(
            review_commitment(&run, 0, now),
            Commitment::Keep(KeepReason::NobodyElseCouldStartIt)
        );
        // ...and kept while there is still time, however little.
        run.spec.deadline = Some(Millis::from_mins(61));
        assert!(matches!(
            review_commitment(&run, 1, now),
            Commitment::Keep(KeepReason::TimeLeft(_))
        ));
    }

    #[test]
    fn an_unstated_deadline_never_gives_a_commitment_back() {
        // The trap this rule is shaped around: no deadline means the moment of submission, so
        // this run has been "overdue" for an hour. Bouncing it would move every commitment in
        // the fleet from one queue to another, at a bid round each, for ever.
        let run = held_run();
        assert_eq!(run.spec.deadline, None);
        assert_eq!(
            review_commitment(&run, 4, Millis::from_mins(60)),
            Commitment::Keep(KeepReason::NoStatedDeadline)
        );
    }

    #[test]
    fn a_pinned_commitment_is_kept_however_late_it_is() {
        // `assign` refuses a second attempt on a pinned run, so releasing it does not re-place
        // it — it makes it unplaceable, which is worse than late.
        let mut run = held_run();
        run.spec.deadline = Some(Millis(0));
        run.spec.restartability = Restartability::Pinned;
        assert_eq!(
            review_commitment(&run, 4, Millis::from_mins(60)),
            Commitment::Keep(KeepReason::PinnedHere)
        );
    }

    #[test]
    fn pressure_never_stops_a_run_this_node_already_accepted() {
        // The start gate deliberately asks a smaller question than `admits`. A committed run has
        // nowhere else to be, nothing frees this machine on its behalf, and refusing to start it
        // until the load average improves is a run held hostage — the honest way out of a
        // commitment is to release it.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        assert_eq!(
            may_start(&policy, Occupancy::default(), 0, Demand::Heavy, None),
            Ok(())
        );
    }

    /// The live start gate, asked the way the daemon asks it.
    ///
    /// These four tests used to call `WorkPolicy::admits_start`, which was a *second*
    /// implementation of this question with no production caller — five tests and no callers,
    /// and its own doc comment asserting the mechanism in the present tense. What actually gates
    /// starting is `Room::for_one_more`, so the rules below are asked of that.
    fn may_start(
        policy: &WorkPolicy,
        running: Occupancy,
        on_agent: u32,
        demand: Demand,
        account: Option<AccountUse>,
    ) -> Result<(), Refusal> {
        crate::capacity::Room::new(Capacity::of(policy), account, None)
            .for_one_more(running, on_agent, demand)
    }

    /// `desktop()` and `phone()` carry no agent capability at all, which is why the per-agent
    /// cap below had no test that could fail: `agent_details` returned `None` and the check fell
    /// straight through to `Ok(())` in every one of them.
    fn with_agent(mut caps: Capabilities, max_concurrent: u32) -> Capabilities {
        caps.add(crate::capability::Capability::agent(
            AgentKind::ClaudeCode,
            crate::capability::AgentDetails {
                version: "2.10.0".into(),
                models: Vec::new(),
                max_concurrent,
            },
            true,
        ));
        caps
    }

    /// `elsewhere` runs on other nodes, `limit` the account's ceiling. What this node itself is
    /// running is supplied separately, by the load or by the caller.
    fn account_at(elsewhere: u32, limit: u32) -> AccountUse {
        AccountUse {
            account: crate::capability::AccountId("acct:test".into()),
            elsewhere,
            limit,
        }
    }

    #[test]
    fn an_agent_at_its_own_per_node_limit_is_refused() {
        // The check existed and was enforced against nothing: every test in this module built
        // capabilities with no agent in them, so the branch was unreachable in the suite while
        // looking covered.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        let caps = with_agent(desktop(), 2);

        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &NodeLoad {
                    held: Occupancy::new(2, 8),
                    held_for_agent: 2,
                    cpu_percent: None,
                    thermal: None,
                },
                None,
            ),
            Err(Refusal::AgentAtCapacity(AgentKind::ClaudeCode))
        );

        // One fewer on that agent and the same node takes it.
        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &NodeLoad {
                    held: Occupancy::new(2, 8),
                    held_for_agent: 1,
                    cpu_percent: None,
                    thermal: None,
                },
                None,
            ),
            Ok(())
        );
    }

    #[test]
    fn an_account_at_its_limit_is_refused_however_idle_the_machine_is() {
        // The point of the cap: this desktop has three free slots and a spotless load average,
        // and the thing it is short of is not on this machine at all.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        let caps = with_agent(desktop(), 4);

        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &idle(),
                Some(&account_at(4, 4)),
            ),
            Err(Refusal::AccountAtCapacity { running: 4, max: 4 })
        );
        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &idle(),
                Some(&account_at(3, 4)),
            ),
            Ok(())
        );
    }

    #[test]
    fn an_account_nobody_set_a_limit_for_does_not_refuse_anything() {
        // `None` is "no opinion", not a ceiling of zero. Nobody here knows what an account's
        // real rate limit is, and a cap invented on this node's behalf would refuse work on a
        // guess — the probe's honesty rule, one plane over.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        assert_eq!(policy.max_concurrent_account, None);
        assert_eq!(
            policy.admits(
                &with_agent(desktop(), 4),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &idle(),
                None,
            ),
            Ok(())
        );
    }

    #[test]
    fn this_node_being_full_is_reported_before_the_account_is() {
        // Order matters for the message, not the outcome: a node that is itself at capacity
        // should say so rather than blaming a shared account, because those two refusals send
        // somebody to different places.
        let mut policy = WorkPolicy::for_class(DeviceClass::Phone);
        policy.accept = AcceptWork::Always;

        assert_eq!(
            policy.admits(
                &with_agent(phone(), 4),
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &holding(1, Demand::Normal.shares()),
                Some(&account_at(9, 4)),
            ),
            Err(Refusal::AtCapacity { running: 1, max: 1 })
        );
    }

    #[test]
    fn the_account_cap_gates_starting_where_pressure_deliberately_does_not() {
        // The asymmetry is the whole point. A committed run must not be held hostage to a load
        // average — the owner's build is not something this node can drain — but it must not
        // start past an account ceiling either, or every node would accept up to the cap and
        // then start straight through it, leaving the cap decorative.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);

        assert_eq!(
            may_start(&policy, Occupancy::default(), 0, Demand::Heavy, None),
            Ok(())
        );
        assert_eq!(
            may_start(
                &policy,
                Occupancy::default(),
                0,
                Demand::Normal,
                Some(account_at(4, 4))
            ),
            Err(Refusal::AccountAtCapacity { running: 4, max: 4 })
        );
    }

    #[test]
    fn an_account_cap_counts_requests_and_not_shares() {
        // Demand-blind on purpose: what an account runs out of is requests per hour, so a
        // `Light` run costs it exactly what a `Heavy` one does. The machine's budget is the
        // thing that cares about weight.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        for demand in [Demand::Light, Demand::Normal, Demand::Heavy] {
            assert_eq!(
                policy.admits(
                    &with_agent(desktop(), 4),
                    Tier::Agent(&AgentKind::ClaudeCode),
                    demand,
                    &idle(),
                    Some(&account_at(2, 2)),
                ),
                Err(Refusal::AccountAtCapacity { running: 2, max: 2 })
            );
        }
    }

    #[test]
    fn this_nodes_own_runs_count_against_the_account_too() {
        // `elsewhere` is peers only, so the node's own contribution has to arrive separately —
        // and it must actually arrive, or a fleet of one has no account cap at all.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        let caps = with_agent(desktop(), 4);
        let one_here = NodeLoad {
            held: Occupancy::new(1, 4),
            held_for_agent: 1,
            cpu_percent: None,
            thermal: None,
        };

        assert_eq!(
            policy.admits(
                &caps,
                Tier::Agent(&AgentKind::ClaudeCode),
                Demand::Normal,
                &one_here,
                Some(&account_at(0, 1)),
            ),
            Err(Refusal::AccountAtCapacity { running: 1, max: 1 })
        );
    }

    #[test]
    fn the_run_being_started_is_not_counted_against_its_own_ceiling() {
        // The deadlock a real daemon found. A held run is `Assigned` and therefore visible, so
        // counting it made every commitment the reason it could not begin: run one finished, and
        // run two sat `assigned` for ever on a one-run account. The start side counts started
        // agents excluding this one, which is exactly what the machine's own capacity does.
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        assert_eq!(
            may_start(
                &policy,
                Occupancy::default(),
                0,
                Demand::Normal,
                Some(account_at(0, 1))
            ),
            Ok(())
        );
    }
}
