//! Handing a notification to the node that can actually reach the person (ADR-0010).
//!
//! The delivery plane's one piece of network. It is deliberately tiny, and deliberately *not*
//! attached to [`crate::Host`]: two planes are kept apart in the ADR, and the whole claim that
//! makes the split worth having is that a device can be a full member of the fleet by carrying
//! messages alone. A phone answers `NoHost` to every run and still implements this.
//!
//! What travels is a notification — already projected from the sender's event log — and what
//! comes back is whether it arrived, or a sentence saying why not. Nothing about a sink itself
//! ever crosses: a route's command, its arguments and its credentials stay on the device that
//! holds them (ADR-0002's rule, applied to the other plane). A peer is told *what* to say, never
//! *how* it says it.

use async_trait::async_trait;
use offload_core::{NodeId, Notification};

/// What this node does when a peer asks it to tell somebody something.
///
/// A trait of its own so that implementing it is a separate decision from hosting runs. The
/// default refuses, which is the honest answer for a node with no routes configured: a member
/// that cannot reach anybody must say so rather than accept and drop, because a dropped
/// notification is the one failure this plane cannot detect on its own.
#[async_trait]
pub trait Deliverer: Send + Sync + 'static {
    /// Carry it. `sink` is this node's own name for the route the asker chose, which it learned
    /// from the capability this node advertises.
    ///
    /// `Ok` means a route accepted it — not that a human has read it, which nothing anywhere
    /// can promise. `Err` is a sentence: it goes back to the sender's outbox and ends up in
    /// `offload sinks` on the machine that was trying to tell somebody.
    async fn deliver(&self, from: NodeId, sink: &str, note: &Notification) -> Result<(), String>;
}

/// A member that can reach nobody.
///
/// Not a failure and not unusual — most nodes in a personal fleet have no route configured, and
/// the one that does is the point. It refuses loudly for the reason `offload-probe` never
/// over-claims: accepting and dropping would make the sender believe somebody had been told.
#[derive(Debug)]
pub struct NoDelivery;

#[async_trait]
impl Deliverer for NoDelivery {
    async fn deliver(&self, _from: NodeId, sink: &str, _note: &Notification) -> Result<(), String> {
        Err(format!("this node has no sink called {sink}"))
    }
}
