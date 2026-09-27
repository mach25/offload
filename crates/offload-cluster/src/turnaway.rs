//! Handshakes a peer refused, counted where the claim inside them cannot be believed
//! (ADR-0060).
//!
//! `offload rekey` is the convergent revocation (ADR-0012): the evicted device is simply not in
//! the new fleet, and nothing has to reach it. So the only message it ever gets is the refusal
//! on its own next dial — `that certificate is for fleet bf36a64a, this is fleet 433de62b` —
//! which is precise, correct, and **not something this node may act on**. A refusal is a peer's
//! claim about this node, and a device that stood down when told to is a device any peer could
//! switch off. [`Refusal::Revoked`](offload_proto::handshake::Refusal::Revoked) gets past that
//! rule with a signature; this one has none to get past it with, because a re-founded fleet
//! revokes nobody and there is nothing to sign.
//!
//! What is left is the half that *is* this node's own: it dialled, it reached something, and it
//! was turned away. That is a different fact from `no answer`, and until this existed every
//! report said the second — a rekeyed-out daemon called its former fleet `dead` and went on
//! reporting `2 member(s) met`.
//!
//! The shape is [`offload_transport::sends::SendRefusals`]'s, deliberately: a total, a capped
//! breakdown, the peer's own words stored unreworded, and no decision anywhere.

use std::collections::HashMap;
use std::sync::Mutex;

use offload_core::NodeId;

/// How many peers are remembered by name.
///
/// [`SendRefusals`](offload_transport::sends::SendRefusals)'s number and its reasoning, one
/// layer up: a handshake refusal comes from something that identified itself, so these are
/// peers rather than addresses and there are fewer of them — but a seed list pointed at a
/// fleet this device has left produces one per seed. Past the cap the total still rises.
const REMEMBERED: usize = 16;

/// One peer that would not let this node in, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turnaway {
    /// Who refused. A `NodeId` rather than an address, unlike the send counter's: a handshake
    /// refusal comes back from a peer that presented a certificate, so the id is known — and it
    /// is what `offload nodes` and every membership command take.
    pub peer: NodeId,
    /// How many times, since this daemon started.
    pub refused: u64,
    /// The last thing the peer said, in its own words. Never reworded: it names both fleets,
    /// which is the whole diagnosis, and a sentence of ours in its place would be a second copy
    /// of a claim we are deliberately not resolving.
    pub last: String,
}

/// Handshake refusals this node has collected, shared with whoever reports them.
#[derive(Debug, Default)]
pub struct Turnaways {
    seen: Mutex<Counts>,
}

#[derive(Debug, Default)]
struct Counts {
    /// Every refusal, including those past [`REMEMBERED`] peers.
    total: u64,
    by_peer: HashMap<NodeId, (u64, String)>,
}

impl Turnaways {
    /// Note that `peer` refused this node's certificate, saying `reason`.
    pub fn record(&self, peer: NodeId, reason: &str) {
        // A poisoned lock costs the count and nothing else; the dial has already failed and
        // panicking through it would turn a report into an outage.
        let Ok(mut seen) = self.seen.lock() else {
            return;
        };
        seen.total = seen.total.saturating_add(1);
        let full = seen.by_peer.len() >= REMEMBERED;
        match seen.by_peer.get_mut(&peer) {
            Some(entry) => {
                entry.0 = entry.0.saturating_add(1);
                entry.1 = reason.to_string();
            }
            None if full => {}
            None => {
                seen.by_peer.insert(peer, (1, reason.to_string()));
            }
        }
    }

    /// Every refusal counted, including peers past the cap.
    ///
    /// Separate from summing [`Self::by_peer`] because those two can disagree, and the total is
    /// the honest one.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.seen.lock().map_or(0, |seen| seen.total)
    }

    /// The breakdown, worst first, then by id so a report is stable between calls.
    #[must_use]
    pub fn by_peer(&self) -> Vec<Turnaway> {
        let Ok(seen) = self.seen.lock() else {
            return Vec::new();
        };
        let mut out: Vec<Turnaway> = seen
            .by_peer
            .iter()
            .map(|(peer, (refused, last))| Turnaway {
                peer: *peer,
                refused: *refused,
                last: last.clone(),
            })
            .collect();
        out.sort_by(|a, b| b.refused.cmp(&a.refused).then_with(|| a.peer.cmp(&b.peer)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    #[test]
    fn the_last_thing_a_peer_said_replaces_the_one_before_it() {
        let seen = Turnaways::default();
        seen.record(
            node(1),
            "that certificate is for fleet aaaa, this is fleet bbbb",
        );
        seen.record(
            node(1),
            "that certificate is for fleet aaaa, this is fleet cccc",
        );
        let rows = seen.by_peer();
        assert_eq!(rows.len(), 1, "one peer, not one row per dial");
        assert_eq!(rows[0].refused, 2);
        assert!(rows[0].last.ends_with("cccc"), "{:?}", rows[0].last);
        assert_eq!(seen.total(), 2);
    }

    #[test]
    fn the_total_keeps_counting_past_the_peers_the_breakdown_can_name() {
        // The two numbers disagree on purpose, and the total is the one that is not short.
        let seen = Turnaways::default();
        for b in 0..40u8 {
            seen.record(node(b), "nope");
        }
        assert_eq!(seen.by_peer().len(), REMEMBERED);
        assert_eq!(seen.total(), 40);
        let named: u64 = seen.by_peer().iter().map(|r| r.refused).sum();
        assert!(named < seen.total(), "named {named} of {}", seen.total());
    }

    #[test]
    fn the_worst_peer_is_first_and_ties_are_stable() {
        let seen = Turnaways::default();
        seen.record(node(9), "a");
        for _ in 0..3 {
            seen.record(node(2), "b");
        }
        seen.record(node(4), "c");
        let rows = seen.by_peer();
        assert_eq!(rows[0].peer, node(2), "three beats one");
        // Two peers at one apiece sort by id, so the report does not shuffle between calls.
        assert_eq!(rows[1].peer, node(4));
        assert_eq!(rows[2].peer, node(9));
    }

    #[test]
    fn a_node_that_was_never_turned_away_reports_nothing() {
        let seen = Turnaways::default();
        assert_eq!(seen.total(), 0);
        assert!(seen.by_peer().is_empty());
    }
}
