//! What happened to the *fleet*, as opposed to what happened to a run.
//!
//! ADR-0012 mitigation 4: every enrolment is announced and recorded. With no per-join approval
//! step, that announcement is the compensating control the whole posture rests on — an
//! enrolment the owner did not perform has to be visible within seconds, while revoking is
//! still ahead of the attacker rather than behind. Probation (mitigation 2) buys the fifteen
//! minutes; this is what is supposed to fill them.
//!
//! Two kinds and no more, because the test is the same one [`crate::notify::Notice`] applies:
//! would somebody want their evening interrupted by it. A device joining the fleet, and the
//! fleet's root secret coming out of the drawer. Everything else about membership is visible in
//! `offload fleet` when somebody goes looking.
//!
//! **This is a log of what a node witnessed, not a fleet-wide record**, and the difference is
//! deliberate. ADR-0012 says an enrolment is "written durably to every node's store", which
//! would make it a gossiped fact — and a gossiped fact needs an owner and an arbitration rule
//! (ADR-0005), which an append-only log of independent observations does not have. So each node
//! records what it saw: the machine that issued the invitation saw an enrolment, and every
//! other machine sees a member it had never met. Two different sentences about one event, each
//! true where it was written, and neither pretending to be the fleet's memory.

use crate::fleet::Grant;
use crate::id::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One thing that happened to this node's fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetLogEvent {
    pub at_unix_ms: u64,
    #[serde(flatten)]
    pub kind: FleetEvent,
}

/// Why a device is suddenly a member.
///
/// On the event rather than inferred from the grants, because the two enrolments that matter
/// look identical afterwards and are worth telling apart at the moment they happen: one was
/// somebody at a keyboard with an existing member, and the other was the fleet's root secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enrolment {
    /// The fleet was founded here.
    Founded,
    /// An approver issued a certificate for it (`offload invite`).
    Invited,
    /// It enrolled itself with the passphrase (`offload join --passphrase`).
    Passphrase,
    /// A member this node had never met turned up and presented valid papers.
    ///
    /// Not an enrolment this node witnessed — it is the same fact seen from every *other*
    /// machine, which is what makes the alarm reach somebody when the enrolling device is not
    /// the one holding a push credential. Said differently in the notification, because "a
    /// device joined" and "a device you have not seen before is in your fleet" have different
    /// things to do about them.
    Met,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fleet_event", rename_all = "snake_case")]
pub enum FleetEvent {
    /// A device is a member of this fleet.
    Enrolled {
        node: NodeId,
        name: String,
        grants: BTreeSet<Grant>,
        how: Enrolment,
    },
    /// The fleet passphrase was used on this machine.
    ///
    /// ADR-0012: once an approver exists, the passphrase is a break-glass credential, and the
    /// whole value of a break-glass credential is that every use is signal rather than noise.
    /// Which is also why this records the *fact* and never anything derived from the phrase.
    PassphraseUsed {
        /// What it was used for, in the CLI's own words: `init`, `join`, `grant`, `revoke`,
        /// `verify`. A string rather than an enum because the producer is the command layer and
        /// this type has no business enumerating commands.
        what: String,
        /// Which device it was typed on.
        node: NodeId,
        /// And what that device is called, because a node id is not something anybody reads at
        /// three in the morning.
        name: String,
    },
}

impl FleetEvent {
    /// A short, stable name for the kind. An interface, like [`crate::LogEvent::kind_name`]:
    /// it is the `kind` column a scan filters on and it reaches a notification script.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            FleetEvent::Enrolled { .. } => "enrolled",
            FleetEvent::PassphraseUsed { .. } => "passphrase_used",
        }
    }
}

impl FleetEvent {
    /// The device this event is about, as the store's `subject` column.
    ///
    /// Denormalised out of the JSON so "have I already announced this one" is an index lookup.
    /// Both variants have one — a passphrase use is about the machine it was typed on, which is
    /// exactly what somebody reading the alarm wants to know.
    #[must_use]
    pub fn subject(&self) -> String {
        match self {
            FleetEvent::Enrolled { node, .. } | FleetEvent::PassphraseUsed { node, .. } => {
                node.to_string()
            }
        }
    }
}

impl FleetLogEvent {
    #[must_use]
    pub fn new(at_unix_ms: u64, kind: FleetEvent) -> FleetLogEvent {
        FleetLogEvent { at_unix_ms, kind }
    }

    #[must_use]
    pub fn subject(&self) -> String {
        self.kind.subject()
    }

    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        self.kind.kind_name()
    }
}
