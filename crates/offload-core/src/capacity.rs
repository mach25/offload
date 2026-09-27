//! What hosting a run costs a device, and how much of that a device will have on it at once.
//!
//! ADR-0013. Admission used to be a count: two runs is two runs, whether they are two typo
//! fixes or two full builds. That is the right shape for a fleet where every run costs about
//! the same, and agent runs do not — so a laptop mid-build looks exactly as free as an idle
//! one, wins the bid on its hardware score, and then serves both runs badly.
//!
//! Three things follow, and they are three different mechanisms rather than a bigger number:
//!
//! * **A run declares its [`Demand`], coarsely.** Three levels, because the workload is a
//!   language model with a shell and any finer number would be a fiction with decimal places.
//! * **A device has a budget** of [`Demand::shares`], and the count survives beside it as a
//!   hard ceiling — "never more than two agents on my laptop regardless" is a thing an owner
//!   reasonably wants to say, and it is not the same statement as "at most half this machine".
//! * **Declared demand is a hint; observed load is the truth.** Admission takes the harsher of
//!   the two. Declarations gate the burst — three runs can arrive in the same second, before
//!   any of them registers anywhere — and measurement catches the ones that lied.
//!
//! ## The two counts, again
//!
//! [`Capacity::room_for`] takes the occupancy rather than reading it, because there are two
//! questions here and CLAUDE.md's warning about them is load-bearing: *may I accept more work*
//! counts the runs this node **holds**, since a commitment fills a slot (ADR-0006), while *may
//! I start another agent* counts the ones actually **running**. Answering the second with the
//! first deadlocks a node that took accepting-without-starting at its word. One rule, two
//! callers, each naming its own count — rather than two copies of the rule that will disagree
//! about the budget in six months.
//!
//! ## Where a budget must not be allowed to bite
//!
//! * **A lone run always fits.** A `Heavy` run wanting more shares than a phone's entire
//!   budget must still be placeable on that phone when it is doing nothing else, or a budget —
//!   a number its owner picked for concurrency — has quietly become an eligibility rule and
//!   the run is unrunnable fleet-wide. The budget limits what runs *beside* something, not
//!   what runs at all. Hardware a run genuinely cannot do without is a `Constraint`, which
//!   fails loudly and names itself.
//! * **Pressure gates accepting, never starting.** A node under load refuses *new* work; it
//!   must not refuse to start what it already accepted, because that run has nowhere else to
//!   be and nothing frees the machine on its behalf. The honest way out of a commitment is to
//!   release it, not to sit on it until the load average improves.

use crate::capability::AccountId;
use crate::policy::{Refusal, WorkPolicy};
use crate::time::Millis;
use serde::{Deserialize, Serialize};

/// What hosting this run is expected to cost the device.
///
/// Coarse on purpose. Fine-grained resource declarations are a fiction when the workload is an
/// agent: a run submitted as "fix a typo" can start a full rebuild in its third turn, so any
/// number the submitter writes down is a guess about somebody else's decision. Three levels
/// are wrong often enough to be honest about; twenty would be wrong just as often while
/// looking authoritative.
///
/// When three stops being enough the answer is **dimensions, not more levels** — compiling and
/// encoding video are both "heavy" and are not alike, and two runs that saturate *different*
/// things coexist happily while two that saturate the same one do not. No scalar expresses
/// that at any resolution. That is a `struct Demand { cpu, memory, io, gpu }`, still coarse
/// per axis, and it is worth building the first time genuinely build-shaped work arrives
/// rather than now, when everything Offload orchestrates is bound by model latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Demand {
    /// Seconds of work, negligible CPU, no build. A scheduled check, a review run.
    Light,
    /// The default for an agent run: sustained model latency, some tool execution.
    #[default]
    Normal,
    /// Known up front to be expensive — a full build, a large clone, a long suite.
    Heavy,
}

impl Demand {
    /// How much of a device's budget this occupies.
    ///
    /// `Normal` is deliberately not 1: shares are what let four light runs sit in the space of
    /// one agent run, and a scale with no room below the default cannot express that. The
    /// numbers are ratios and nothing else reads them — a device's budget defaults to
    /// `Normal` × its run count, so a fleet that never mentions demand behaves exactly as it
    /// did before there was a budget at all.
    #[must_use]
    pub const fn shares(self) -> u32 {
        match self {
            Demand::Light => 1,
            Demand::Normal => 4,
            Demand::Heavy => 8,
        }
    }

    /// How much of the machine has to be idle before this is worth starting here.
    ///
    /// Deliberately *not* derived from [`Self::shares`] as a fraction of the budget. A share
    /// is a ratio between runs; this is a statement about a machine, and tying them would make
    /// a laptop with a small budget absurdly picky — a `Heavy` run would want the whole
    /// machine motionless, so a fleet of small devices could not place one anywhere. Three
    /// numbers a person can read: a light run needs a tenth of the machine, a normal one a
    /// quarter, a heavy one half.
    #[must_use]
    pub const fn needs_idle_percent(self) -> u8 {
        match self {
            Demand::Light => 10,
            Demand::Normal => 25,
            Demand::Heavy => 50,
        }
    }

    /// What `--demand` accepts, and what a config file may say.
    pub fn parse(s: &str) -> Result<Demand, UnknownDemand> {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" => Ok(Demand::Light),
            "normal" | "" => Ok(Demand::Normal),
            "heavy" => Ok(Demand::Heavy),
            other => Err(UnknownDemand(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown demand {0} — expected light, normal or heavy")]
pub struct UnknownDemand(pub String);

impl std::str::FromStr for Demand {
    type Err = UnknownDemand;

    fn from_str(s: &str) -> Result<Demand, UnknownDemand> {
        Demand::parse(s)
    }
}

impl std::fmt::Display for Demand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Demand::Light => "light",
            Demand::Normal => "normal",
            Demand::Heavy => "heavy",
        })
    }
}

/// How long a node under load asks to be come back to.
///
/// A constant, and a guess — which is the honest description of anything derived from a
/// one-minute load average. It is *that* window rather than a tuned number: the input cannot
/// see a spike shorter than a minute, so promising to be free in fifteen seconds would be a
/// claim about something this node cannot observe. ADR-0013 allows `retry_after` to be wrong
/// and says so out loud; what it must not be is precise-looking.
pub const PRESSURE_RETRY: Millis = Millis::from_secs(60);

/// What a node already has on it, for whichever of the two capacity questions is being asked.
///
/// See the module docs: *may I accept more* counts held runs, *may I start another agent*
/// counts running ones. The struct is the same either way; which runs went into it is the
/// caller's statement about what it is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Occupancy {
    pub runs: u32,
    /// The demands of those runs, added up.
    pub shares: u32,
}

impl Occupancy {
    #[must_use]
    pub const fn new(runs: u32, shares: u32) -> Occupancy {
        Occupancy { runs, shares }
    }

    /// Nothing here at all — which is the case the lone-run rule turns on.
    #[must_use]
    pub const fn is_idle(self) -> bool {
        self.runs == 0 && self.shares == 0
    }
}

/// Everything a node knows about how busy it is, when deciding whether to take on more.
///
/// The counts are of runs this node **holds** — a commitment fills a slot even before its
/// agent starts (ADR-0006) — and the load is local: it is never gossiped, because it is not
/// what a device *is*. `None` means the platform would not say, and the rule that reads it
/// then does not apply rather than assuming an idle machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeLoad {
    pub held: Occupancy,
    /// Held runs on the same agent, for its own per-node cap.
    pub held_for_agent: u32,
    pub cpu_percent: Option<u8>,
    /// How hot the device says it is (ADR-0068), or `None` where nothing says — which is not
    /// a cool device, and the rule that reads it then does not apply.
    pub thermal: Option<Thermal>,
}

/// A device's own thermal status, in Android's terms (`PowerManager.getCurrentThermalStatus`).
/// Observed pressure like the load average: local, never gossiped (ADR-0068).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Thermal {
    None,
    Light,
    Moderate,
    Severe,
    Critical,
    Emergency,
    Shutdown,
}

impl Thermal {
    /// Android's integer status, or `None` for a value this build does not know.
    #[must_use]
    pub fn from_android(status: u8) -> Option<Thermal> {
        Some(match status {
            0 => Thermal::None,
            1 => Thermal::Light,
            2 => Thermal::Moderate,
            3 => Thermal::Severe,
            4 => Thermal::Critical,
            5 => Thermal::Emergency,
            6 => Thermal::Shutdown,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Thermal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            // Android's own name for status 0 is `NONE`, and `thermal none` printed under `cpu
            // not reported` read as a missing reading. An unreported status prints no line at
            // all, so this one has to say what it is.
            Thermal::None => "none (not throttling)",
            Thermal::Light => "light",
            Thermal::Moderate => "moderate",
            Thermal::Severe => "severe",
            Thermal::Critical => "critical",
            Thermal::Emergency => "emergency",
            Thermal::Shutdown => "shutdown",
        })
    }
}

impl Demand {
    /// The thermal status from which a device refuses to accept this demand (ADR-0068 §2):
    /// `Severe` — Android's own "throttling is significant" — and one step earlier for heavy
    /// work, which is the kind that makes a device hotter.
    #[must_use]
    pub const fn too_hot_from(self) -> Thermal {
        match self {
            Demand::Light | Demand::Normal => Thermal::Severe,
            Demand::Heavy => Thermal::Moderate,
        }
    }
}

impl NodeLoad {
    /// How much of the machine is not doing anything, or `None` if it will not say.
    #[must_use]
    pub fn idle_percent(&self) -> Option<u8> {
        self.cpu_percent.map(|busy| 100u8.saturating_sub(busy))
    }
}

/// What one agent *account* has running across the fleet, and the ceiling it must stay under.
///
/// The counts above are facts about one machine, counted exactly from what it holds. This is a
/// fact about an account — nodes signed into one login share one rate limit — so it is summed
/// from the view, which makes it **best-effort by construction**: two nodes each with a free
/// slot can decide to start in the same instant and overshoot it, and gossip is a moment stale
/// besides.
///
/// That is tolerable here, and it is worth being explicit about why, because the same sentence
/// would be indefensible one file over. Overshooting an account cap costs a rate-limit reply
/// the agent already reports and this project already handles (`LogKind::RateLimit`). Getting
/// placement wrong costs two agents committing to one repository. **Nothing about epochs,
/// leases or fencing may ever be justified this way** — the device-local broker exists because
/// the same problem one level down needed a ledger rather than an estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUse {
    /// Which account, for the message. Opaque, and not a credential (`AccountId`).
    pub account: AccountId,
    /// Runs on this account on **other** nodes — never this one.
    ///
    /// Deliberately not the whole total, and the field name is doing a job. A node knows its own
    /// runs exactly, from its own store, and only needs gossip for everybody else's; asking the
    /// view about itself cost two bugs in one demo. It counted a node's own runs a gossip tick
    /// late, so two submissions a second apart both started on a one-run ceiling — and worse, at
    /// *start* time it counted the very run being started, which is `Assigned` and therefore
    /// visible, so a held run was the reason it could not begin and waited for ever.
    pub elsewhere: u32,
    /// The lowest ceiling any node on this account claims. See `ClusterView::account_use`:
    /// the conservative direction is the only safe fold, because a node cannot be allowed to
    /// raise the fleet's limit by claiming more for itself.
    pub limit: u32,
}

impl AccountUse {
    /// Room for one more run on this account, given `here` of them on this node already.
    ///
    /// The caller supplies `here` because only the caller knows *which* question it is asking:
    /// held runs when deciding whether to accept more, started agents excluding this one when
    /// deciding what to run next. That is the same held-versus-started split the machine's own
    /// capacity has (see the module docs), and collapsing it is what deadlocked a commitment
    /// against itself.
    ///
    /// Demand-blind on purpose: what an account is short of is requests per hour, not shares of
    /// a machine, so a `Light` run costs it exactly what a `Heavy` one does.
    pub fn room_for_one_more(&self, here: u32) -> Result<(), Refusal> {
        let running = self.elsewhere.saturating_add(here);
        if running >= self.limit {
            return Err(Refusal::AccountAtCapacity {
                running,
                max: self.limit,
            });
        }
        Ok(())
    }
}

/// What a node is prepared to have on it at once: a hard count, and a budget of shares.
///
/// Both, not either. The count is the owner's absolute ceiling and survives from before there
/// was a budget; the budget is what lets a heavy run keep a machine to itself while four light
/// ones share it. Read from the policy the node **gossiped**, so that what it bid with and
/// what it admits with cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub max_runs: u32,
    pub budget: u32,
}

impl Capacity {
    #[must_use]
    pub fn of(policy: &WorkPolicy) -> Capacity {
        Capacity {
            max_runs: policy.max_concurrent_runs,
            budget: policy.budget(),
        }
    }

    /// Room for `runs` ordinary agent runs, which is what a device that never mentioned a
    /// budget has. The shape every caller had before there was one.
    #[must_use]
    pub const fn runs(runs: u32) -> Capacity {
        Capacity {
            max_runs: runs,
            budget: runs.saturating_mul(Demand::Normal.shares()),
        }
    }

    /// Is there room for one more run of this demand beside `have`?
    ///
    /// The count first, because it is the owner's flat "no more than this" and the message it
    /// produces is the one people already know. Then the budget, unless there is nothing here
    /// at all — see the module docs on why a lone run always fits.
    pub fn room_for(&self, have: Occupancy, demand: Demand) -> Result<(), Refusal> {
        if have.runs >= self.max_runs {
            return Err(Refusal::AtCapacity {
                running: have.runs,
                max: self.max_runs,
            });
        }
        let wanted = demand.shares();
        if !have.is_idle() && have.shares.saturating_add(wanted) > self.budget {
            return Err(Refusal::BudgetFull {
                committed: have.shares,
                budget: self.budget,
                wanted,
            });
        }
        Ok(())
    }

    /// How much of the budget is unspoken for, as a percentage. For a refusal's wording: it is
    /// the number that disagrees with the load average, and the disagreement is the point.
    #[must_use]
    pub fn free_percent(&self, have: Occupancy) -> u8 {
        let budget = u64::from(self.budget.max(1));
        let free = budget.saturating_sub(u64::from(have.shares));
        u8::try_from(free * 100 / budget).unwrap_or(100)
    }
}

/// Everything that limits one more run here: this node's own capacity, and the ceiling on the
/// account its agent is signed into.
///
/// Two limits with two owners — the machine's is the device owner's, the account's is shared
/// with every node on that login — and they are carried together because every place that asks
/// "is there room for one more" has to ask both. Answering only the first is what made the
/// account cap decorative in the first draft: each node accepted work up to the cap and then
/// started straight through it, because starting asked a narrower question than accepting did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub capacity: Capacity,
    /// `None` where there is no account to share or nobody claims a ceiling for it — and on a
    /// node with no fleet in view, which cannot count what other machines are running.
    pub account: Option<AccountUse>,
    /// When the account's rate limit lifts, if the agent has said it is blocking (ADR-0029).
    ///
    /// **Already resolved against a clock by the caller**, which is why it is an instant and not
    /// a status: this crate has none, and a reader here must not have to ask whether the value
    /// has expired. `None` means nothing is holding it — never seen one, or the one we saw has
    /// lifted.
    ///
    /// It gates **starting**, and that is the whole reason it is on `Room` rather than checked at
    /// the two call sites that would each have to remember: the four checks that ought to be one
    /// is a mistake this codebase has made in four different words already, and the fifth caller
    /// does not remember. Starting rather than accepting is deliberate and is *not* the
    /// pressure rule — see [`Room::for_one_more`].
    pub rate_limited_until: Option<crate::time::Millis>,
}

impl Room {
    #[must_use]
    pub fn new(
        capacity: Capacity,
        account: Option<AccountUse>,
        rate_limited_until: Option<crate::time::Millis>,
    ) -> Room {
        Room {
            capacity,
            account,
            rate_limited_until,
        }
    }

    /// Is there room for one more run of this demand beside `have`?
    ///
    /// `on_agent` is how many of `have` are on the account's own agent — the number the account
    /// ceiling is measured against, where `have` is what the machine's capacity is measured
    /// against. Two counts because they answer to two different limits.
    /// **The live start gate**, and deliberately a smaller question than [`WorkPolicy::admits`].
    ///
    /// It counts agents *running* rather than runs *held*, and it asks nothing about battery,
    /// network or observed load: a run that is already accepted has nowhere else to be, nothing
    /// frees this machine on its behalf, and refusing to start it because the machine is busy
    /// would hold it hostage to a load average. The way out of a commitment is to release it, not
    /// to sit on it (`review_commitment`).
    ///
    /// That paragraph used to live on a `WorkPolicy::admits_start` with **no production caller**
    /// — five tests, no callers, and a doc comment asserting the mechanism in the present tense.
    /// It was also *divergent*: it had no rate limit in it, so the next person to wire a start
    /// gate would have reached for the function whose name says so and silently reopened the hole
    /// ADR-0029 closed. Deleted, with the reasoning moved here, where the code is.
    ///
    /// Two ceilings and one clock, and the clock is the odd one out (ADR-0029).
    ///
    /// A rate limit gates starting, which "load gates accepting, never starting" appears to
    /// forbid and does not: that rule is about a number **nobody controls** — the owner's build
    /// on the same machine, undrainable from here and never promised — so holding a committed run
    /// behind it would be holding it hostage. A rate limit is the other kind. It lifts at an
    /// instant the agent stated, nothing outside this fleet has to happen first, and the run it
    /// would hold is one whose agent would otherwise be spawned only to stall on the same limit
    /// and spend a session slot doing it. That is being *full*, with a clock instead of a count.
    pub fn for_one_more(
        &self,
        have: Occupancy,
        on_agent: u32,
        demand: Demand,
    ) -> Result<(), Refusal> {
        self.capacity.room_for(have, demand)?;
        if let Some(account) = &self.account {
            account.room_for_one_more(on_agent)?;
        }
        if let Some(until) = self.rate_limited_until {
            return Err(Refusal::AccountRateLimited { until });
        }
        Ok(())
    }
}

/// So a caller with no fleet in view — and every test written before there was an account cap
/// — can still say what it means with a bare [`Capacity`].
impl From<Capacity> for Room {
    fn from(capacity: Capacity) -> Room {
        Room {
            capacity,
            account: None,
            rate_limited_until: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::DeviceClass;

    fn laptop() -> WorkPolicy {
        WorkPolicy::for_class(DeviceClass::Laptop)
    }

    #[test]
    fn a_budget_nobody_set_behaves_exactly_like_the_run_count_it_replaced() {
        // The compatibility property that lets this ship: two normal runs on a two-run laptop,
        // and the third refused by the ceiling rather than by a number nobody chose.
        let cap = Capacity::of(&laptop());
        assert_eq!(cap.budget, 2 * Demand::Normal.shares());
        assert!(cap.room_for(Occupancy::new(1, 4), Demand::Normal).is_ok());
        assert!(matches!(
            cap.room_for(Occupancy::new(2, 8), Demand::Normal),
            Err(Refusal::AtCapacity { .. })
        ));
    }

    #[test]
    fn a_rate_limited_account_does_not_start_a_run_however_idle_the_machine() {
        // ADR-0029, and the reason the check is on `Room` rather than at the two call sites that
        // would each have to remember it: this is what gates *starting*, and the submit path and
        // the every-tick "there is room now" path both go through here.
        let capacity = Capacity::of(&laptop());
        let idle = Occupancy::default();

        let free = Room::new(capacity, None, None);
        assert!(free.for_one_more(idle, 0, Demand::Normal).is_ok());

        let held = Room::new(capacity, None, Some(crate::time::Millis(9_000)));
        assert!(matches!(
            held.for_one_more(idle, 0, Demand::Normal),
            Err(Refusal::AccountRateLimited {
                until: crate::time::Millis(9_000)
            })
        ));

        // The machine's own ceilings are reported first when they also bind, because those are
        // the ones an operator can do something about — and because the count is the harsher
        // answer: it does not lift on a clock.
        let full = Occupancy {
            runs: capacity.max_runs,
            shares: 0,
        };
        assert!(matches!(
            held.for_one_more(full, 0, Demand::Normal),
            Err(Refusal::AtCapacity { .. })
        ));

        // And `From<Capacity>` — the shape every caller with no fleet in view uses — holds
        // nothing back, which is the behaviour there was before this existed.
        assert!(Room::from(capacity)
            .for_one_more(idle, 0, Demand::Normal)
            .is_ok());
    }

    #[test]
    fn a_heavy_run_keeps_the_machine_to_itself() {
        // The case counting runs cannot express: room by the count, no room by the budget.
        let cap = Capacity::of(&laptop());
        assert!(matches!(
            cap.room_for(Occupancy::new(1, Demand::Heavy.shares()), Demand::Normal),
            Err(Refusal::BudgetFull {
                committed: 8,
                budget: 8,
                wanted: 4
            })
        ));
        // ...and four light ones sit in the space of one agent run. On a desktop, because
        // the laptop's *count* would stop the third of them — which is the ceiling doing its
        // job and not the budget doing this one.
        let desktop = Capacity::of(&WorkPolicy::for_class(DeviceClass::Desktop));
        let mut light = Occupancy::default();
        for _ in 0..4 {
            assert!(desktop.room_for(light, Demand::Light).is_ok());
            light = Occupancy::new(light.runs + 1, light.shares + Demand::Light.shares());
        }
        assert_eq!(light.shares, Demand::Normal.shares());
    }

    #[test]
    fn a_lone_run_fits_even_when_it_is_bigger_than_the_whole_budget() {
        // A phone allowed one run has a budget of one normal run, and a heavy run wants eight
        // shares. Refusing it here would make the budget an eligibility rule, and a heavy run
        // would be unplaceable on a fleet of phones rather than merely slow on one.
        let phone = WorkPolicy::for_class(DeviceClass::Phone);
        let cap = Capacity::of(&phone);
        assert!(Demand::Heavy.shares() > cap.budget);
        assert!(cap.room_for(Occupancy::default(), Demand::Heavy).is_ok());
    }

    #[test]
    fn the_count_stays_a_hard_ceiling_however_light_the_work_is() {
        // "Never more than two agents on my laptop regardless" is a different statement from
        // "at most half this machine", and the budget must not talk the count out of it.
        let cap = Capacity::of(&laptop());
        assert!(matches!(
            cap.room_for(Occupancy::new(2, 2), Demand::Light),
            Err(Refusal::AtCapacity { running: 2, max: 2 })
        ));
    }

    #[test]
    fn demand_round_trips_through_what_a_person_types() {
        for demand in [Demand::Light, Demand::Normal, Demand::Heavy] {
            assert_eq!(Demand::parse(&demand.to_string()), Ok(demand));
        }
        assert_eq!(Demand::parse(" HEAVY "), Ok(Demand::Heavy));
        assert!(Demand::parse("huge").is_err());
    }
}
