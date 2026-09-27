//! Reaching a resource that is on somebody else's machine (ADR-0011).
//!
//! The ADR's rule, and the only correct direction: **a remote resource is a proxied call, never
//! a shipped credential.** If the phone holds the mailbox and the desktop hosts the agent, the
//! desktop exposes the tool and forwards the call to the phone. Shipping the token the other way
//! would put the phone's credential on a machine its owner never gave it to — the same rule
//! ADR-0002 states for agent auth, applied to the other kind of secret.
//!
//! What crosses the wire is the agent's own protocol, one JSON-RPC message per frame, and
//! nothing else. The holder learns which run is asking and which service it was granted; the
//! caller learns nothing about how that service is reached. That asymmetry is the same one the
//! delivery plane has: a peer is told *what* to say and never *how*.
//!
//! A registration of its own beside [`crate::Host`], [`crate::Deliverer`] and [`crate::Members`],
//! for their reason: this crate owns the loop and none of the state. A node that nominates no
//! resources registers nothing and refuses every ask, which is the honest answer.

use async_trait::async_trait;
use offload_core::{NodeId, RunId, Service};

/// One open conversation with a nominated server, from the holder's side.
///
/// A duplex of lines rather than a process, because the process is `offload-node`'s business:
/// this crate must not know what an MCP server is, only that something upstream can be handed a
/// line and will eventually produce some.
#[async_trait]
pub trait ResourceChannel: Send {
    /// Pass a line to the server. An error ends the session, which is right: a JSON-RPC stream
    /// that dropped a message in the middle is not a stream anybody can carry on with.
    async fn send(&mut self, line: String) -> Result<(), String>;

    /// The next line the server produced, or `None` once it has finished.
    async fn next(&mut self) -> Option<String>;

    /// Stop it. Called when the caller hangs up, which is what happens when the agent exits.
    async fn close(&mut self);
}

/// What this node does when a peer's run asks to use one of its resources.
#[async_trait]
pub trait Resources: Send + Sync + 'static {
    /// Open the nominated server for `run`, or say why not.
    ///
    /// `from` is the peer the handshake authenticated, not anything in the message, because it
    /// is the only part of the request that is not the asker's own claim.
    async fn open(
        &self,
        from: NodeId,
        run: RunId,
        service: &Service,
    ) -> Result<Box<dyn ResourceChannel>, String>;
}

/// A node that offers nothing.
///
/// The default, and the ordinary answer: most devices nominate no resources at all. It refuses
/// with a sentence for `NoHost`'s reason — a caller that is told "no" tries something else, and
/// a caller that is accepted and then silently served nothing waits.
#[derive(Debug)]
pub struct NoResources;

#[async_trait]
impl Resources for NoResources {
    async fn open(
        &self,
        _from: NodeId,
        _run: RunId,
        service: &Service,
    ) -> Result<Box<dyn ResourceChannel>, String> {
        Err(format!("this node offers no {service} for a run to use"))
    }
}
