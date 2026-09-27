//! Pure domain model for Offload.
//!
//! This crate is deliberately free of I/O, async, and clock access: every function that
//! needs the time takes a [`Millis`] argument. That is what makes placement, negotiation
//! and drop-off decisions property-testable and deterministically simulatable.
//! See `docs/adr/0001-crate-layout.md`.
//!
//! The pieces, roughly in the order they matter:
//!
//! * [`capability`] — what a device is and has.
//! * [`policy`] — what its owner permits, and what to do when a node drops off.
//! * [`capacity`] — what a run costs a device, and how much of that it will have on it.
//! * [`constraint`] — what a run needs, and why a node didn't qualify.
//! * [`run`] — the run state machine and epoch fencing.
//! * [`allowlist`] — per-tool grants, so a run can execute its tests without a shell.
//! * [`view`] — every node's replicated picture of the cluster.
//! * [`bid`] — nodes negotiating over who takes which run.
//! * [`progress`] — what a run has done and cost, and who is allowed to say so.
//! * [`deadline`] — when a run is due, and the urgency derived from that rather than stored.
//! * [`event`] — a run's event log, which travels because a follower may be elsewhere.
//! * [`recovery`] — whether anybody is watching, and what that means when a run breaks.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod allowlist;
pub mod ask;
pub mod audit;
pub mod bid;
pub mod capability;
pub mod capacity;
pub mod constraint;
pub mod deadline;
pub mod event;
pub mod fleet;
pub mod fleet_event;
pub mod id;
pub mod notify;
pub mod policy;
pub mod progress;
pub mod recovery;
pub mod repo;
pub mod run;
pub mod schedule;
pub mod time;
pub mod version;
pub mod view;

pub use allowlist::{PatternError, Risk, ToolAllowlist, ToolPattern};
pub use ask::{AskPolicy, PendingAsk, DEFAULT_ASK_BUDGET};
pub use audit::{Attempt, AuditEntry, AuditEvent, Reclamation, Rescue};
pub use bid::{Availability, Bid, BidWeights, LocalFacts, NoBid, Offer, Score, ScoreTerms};
pub use capability::{
    Access, AccountId, AgentDetails, AgentKind, Arch, Capabilities, Capability, CapabilityId,
    DeviceClass, Metered, Model, Os, PowerSource, Role, Service, ServiceDetails, Stability,
    UnknownService,
};
pub use capacity::{
    AccountUse, Capacity, Demand, NodeLoad, Occupancy, Room, Thermal, UnknownDemand, PRESSURE_RETRY,
};
pub use constraint::{Constraint, Explain, NodeRef, Unresolved, Wanted};
pub use deadline::{Prospect, Slack};
pub use event::{rate_limit_blocks, Answer, LogEvent, LogKind, Waiting};
pub use fleet::{
    default_grants, Delegation, FleetId, Grant, Issuer, MembershipCert, MembershipError,
    RenewalProof, Revocation, Succession, Terms,
};
pub use fleet_event::{Enrolment, FleetEvent, FleetLogEvent};
pub use id::{needle, BlobHash, Needle, NodeId, RuleId, RunId, ScheduleId};
pub use notify::{
    deliverability, notable, notable_fleet, Audience, Notice, Notices, Notification, Subject,
    Topic, Undeliverable, NOTABLE_FLEET_KINDS, NOTABLE_KINDS,
};
pub use policy::{
    decide_reassignment, grace_for, parked, review_commitment, supervise, AcceptWork, Bystanding,
    Commitment, Grace, HoldReason, KeepReason, LightWork, NodeObservation, Offering,
    ReassignDecision, ReassignPolicy, ReassignReason, Refusal, Supervision, Tier, WorkPolicy,
};
pub use progress::{LegSpend, RunProgress, TokenUse};
pub use recovery::{
    decide_recovery, Attendance, Circumstances, Escalation, Hosting, Recovery, RecoveryPolicy,
    Standing,
};
pub use repo::{Portability, RepoReach, RepoSource, SCRATCH};
pub use run::{
    AgentWork, Checkpoint, Continuation, ContinuationBase, ContinueMode, Epoch, GivenUp, Lease,
    Origin, PermissionMode, Restartability, Run, RunSpec, RunState, SpecEdit, SpecProblem,
    TaskWork, TransitionError, Work, WorkKind, WorkspaceSpec, LEASE, PARENT_TRANSCRIPT,
};
pub use schedule::{Schedule, ScheduleProblem, MIN_EVERY};
pub use time::Millis;
pub use view::{settle_spec, ClusterView, NodeStatus, NodeView};
