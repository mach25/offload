//! Who to probe, who to ask about them, and when silence becomes a conclusion.
//!
//! Pure and clock-injected, like everything that decides anything in this project: the loop
//! in [`crate::Cluster`] does the I/O and asks this what it means. Failure detection is the
//! part of a mesh that is hardest to test after the fact and easiest to get subtly wrong —
//! and every one of its inputs is a timestamp, so the only way to test it honestly is to hand
//! it the times.
//!
//! The shape is SWIM's: a direct probe, an indirect probe through other members if that goes
//! unanswered, a `Suspect` the subject may refute, and only then `Dead`. Two departures from
//! the paper, both deliberate:
//!
//! * **Targets are chosen round-robin rather than at random.** SWIM randomises to bound
//!   detection time at hundreds of nodes; at the tens this design assumes (ADR-0005), a
//!   rotation covers everyone in a bounded number of periods and is reproducible in a test.
//! * **`Dead` is not terminal.** The subject can come back with a higher incarnation, because
//!   the usual cause of silence here is a laptop in a bag (ADR-0007).
//!
//! What this module never does is decide anything about *work*. A `Suspect` moves no runs; the
//! hold-down policy in `offload-core` reads the observation and decides separately. Collapsing
//! those two is how a fleet gets migration thrash.

use offload_core::{ClusterView, Millis, NodeId, NodeStatus};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectorConfig {
    /// One probe per period. Detection time is roughly `probe_interval × members`, since the
    /// rotation visits everyone.
    pub probe_interval: Millis,
    /// How long a direct probe waits before we ask somebody else to try.
    pub probe_timeout: Millis,
    /// How many peers to ask. More is more robust to a second node being unreachable, and
    /// costs a message each.
    pub indirect_peers: usize,
    /// How long a node stays `Suspect` before it is called `Dead`. Long enough for the subject
    /// to hear the suspicion and refute it, which is the whole point of the state.
    pub suspect_timeout: Millis,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        DetectorConfig {
            probe_interval: Millis(1_000),
            probe_timeout: Millis(500),
            indirect_peers: 3,
            suspect_timeout: Millis(5_000),
        }
    }
}

/// The failure detector's own memory: what it is currently suspicious of, and where the
/// rotation has got to.
#[derive(Debug)]
pub struct Detector {
    config: DetectorConfig,
    round: u64,
    suspect_since: BTreeMap<NodeId, Millis>,
    /// When each quiet peer was last chosen, so it is probed at most every [`QUIET_PROBE_EVERY`].
    last_probed: BTreeMap<NodeId, Millis>,
    /// Each peer's round trips, for its probe timeout (see [`Detector::timeout_for`]).
    rtt: BTreeMap<NodeId, Rtt>,
}

/// A peer's smoothed round trip and its variation, TCP's estimator (RFC 6298), in ms, and how
/// many timeouts in a row have doubled its wait since it last answered.
#[derive(Debug, Clone, Copy, Default)]
struct Rtt {
    smoothed: u64,
    variation: u64,
    backoff: u32,
}

/// The longest a probe waits, however slow a peer has been. Under `suspect_timeout` (5 s), so a
/// suspected node still has time to hear it and refute.
pub const MAX_PROBE_TIMEOUT: Millis = Millis(3_000);

impl Detector {
    #[must_use]
    pub fn new(config: DetectorConfig) -> Detector {
        Detector {
            config,
            round: 0,
            suspect_since: BTreeMap::new(),
            last_probed: BTreeMap::new(),
            rtt: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn config(&self) -> DetectorConfig {
        self.config
    }

    /// The node to probe this period, if there is anybody to probe.
    ///
    /// Includes nodes already suspected: a suspicion is a question, and the way to answer it
    /// is to keep asking. Excludes nodes that told us they were leaving — they are not lost,
    /// they are gone, and probing them is noise in a log at the moment somebody is reading it.
    ///
    /// **A dead node gets one round in [`DEAD_EVERY`].** It is still probed, which is one of the
    /// two ways it comes back (the other is its own handshake, `Cluster::introduce`). But a node
    /// that has been gone since the afternoon took a turn in every rotation, and the phone
    /// helping probe it dialled a stale address over LTE for nothing (session ninety-two).
    ///
    /// **A quiet node (ADR-0078) is left out until [`QUIET_PROBE_EVERY`] has passed** since it was
    /// last chosen here: every probe wakes a phone, and a fleet taking turns at one every second
    /// kept the phone from sleeping all night.
    pub fn next_target(&mut self, view: &ClusterView, now: Millis) -> Option<NodeId> {
        let round = self.round;
        self.round = self.round.wrapping_add(1);
        let rested = |id: &NodeId| {
            !view.nodes.get(id).is_some_and(|n| n.quiet)
                || self
                    .last_probed
                    .get(id)
                    .is_none_or(|at| now.saturating_sub(*at) >= QUIET_PROBE_EVERY)
        };
        let (dead, live): (Vec<NodeId>, Vec<NodeId>) =
            probeable(view).filter(rested).partition(|id| {
                view.nodes
                    .get(id)
                    .is_some_and(|n| n.status == NodeStatus::Dead)
            });
        // The dead take turns among themselves, counted in their own rounds — unless nobody is
        // alive, when every round is theirs and they rotate round by round. Counting in dead
        // rounds then picked one node thirty times running: the Mac probed a node it had no
        // address for every second while the laptop it could reach waited (session ninety-two).
        let (pool, turn) = if live.is_empty() {
            (&dead, round)
        } else if round % DEAD_EVERY == 0 && !dead.is_empty() {
            (&dead, round / DEAD_EVERY)
        } else {
            (&live, round)
        };
        if pool.is_empty() {
            return None;
        }
        let index = usize::try_from(turn % pool.len() as u64).unwrap_or(0);
        let chosen = pool[index];
        if view.nodes.get(&chosen).is_some_and(|n| n.quiet) {
            self.last_probed.insert(chosen, now);
        }
        Some(chosen)
    }

    /// Who to ask about `target` when a direct probe goes unanswered.
    ///
    /// Anybody alive but the target and ourselves, taken from the rotation so that two nodes
    /// asking about the same peer do not always ask the same helper.
    #[must_use]
    pub fn helpers(&self, view: &ClusterView, target: NodeId) -> Vec<NodeId> {
        let alive: Vec<NodeId> = view
            .nodes
            .values()
            .filter(|n| n.id != target && n.id != view.local && n.status == NodeStatus::Alive)
            .map(|n| n.id)
            .collect();
        if alive.is_empty() {
            return Vec::new();
        }
        let start = usize::try_from(self.round % alive.len() as u64).unwrap_or(0);
        alive
            .iter()
            .cycle()
            .skip(start)
            .take(self.config.indirect_peers.min(alive.len()))
            .copied()
            .collect()
    }

    /// A probe was answered — directly or through a helper, which count the same.
    ///
    /// Returns `true` if this cleared a suspicion, which is worth logging: it is the evidence
    /// that the indirect path is doing its job.
    pub fn heard_from(&mut self, node: NodeId) -> bool {
        self.suspect_since.remove(&node).is_some()
    }

    /// Nobody could reach it. Start (or continue) suspecting.
    ///
    /// Returns the status to record, or `None` if nothing changed — the caller applies it to
    /// the view and gossips it, and re-declaring the same suspicion every period would be a
    /// change nobody made.
    pub fn unreachable(&mut self, node: NodeId, now: Millis) -> Option<NodeStatus> {
        if self.suspect_since.contains_key(&node) {
            return None;
        }
        self.suspect_since.insert(node, now);
        Some(NodeStatus::Suspect)
    }

    /// Suspicions old enough to be conclusions.
    ///
    /// Separate from [`Self::unreachable`] because the timeout is what gives the subject a
    /// chance to refute: `Suspect` exists to be argued with, and a detector that went straight
    /// to `Dead` would be one that never listened.
    ///
    /// A quiet node gets [`QUIET_SUSPECT_TIMEOUT`]: it speaks about once a minute, so it needs
    /// that long to hear it is suspected and refute.
    #[must_use]
    pub fn expired(&self, view: &ClusterView, now: Millis) -> Vec<NodeId> {
        self.suspect_since
            .iter()
            .filter(|(node, since)| {
                let quiet = view.nodes.get(node).is_some_and(|n| n.quiet);
                let patience = if quiet {
                    QUIET_SUSPECT_TIMEOUT.max(self.config.suspect_timeout)
                } else {
                    self.config.suspect_timeout
                };
                now.saturating_sub(**since) >= patience
            })
            .map(|(node, _)| *node)
            .collect()
    }

    /// Stop tracking a node we have concluded about, or that has left.
    pub fn forget(&mut self, node: NodeId) {
        self.suspect_since.remove(&node);
        self.last_probed.remove(&node);
        self.rtt.remove(&node);
    }

    /// How long to wait for `node` to answer a probe.
    ///
    /// **Its own round trips decide, not one number for the fleet.** 500 ms suits a laptop on the
    /// LAN (16 ms median to the emulator) and cut off the phone on LTE, whose radio sleeps between
    /// packets: median 82 ms, p90 414, the 104 answers bunched up against 500, and 14% of probes
    /// unanswered in time. It was suspected twice a minute, all day (session ninety-two). So:
    /// smoothed RTT + 4 × its variation, never under the configured timeout, never over
    /// [`MAX_PROBE_TIMEOUT`], doubled for each timeout in a row, since a missed answer is a
    /// sample the estimator never sees.
    #[must_use]
    pub fn timeout_for(&self, node: NodeId) -> Millis {
        let floor = self.config.probe_timeout.0;
        let learned = self.rtt.get(&node).map_or(floor, |r| {
            let base = (r.smoothed + 4 * r.variation).max(floor);
            base.saturating_mul(1u64 << r.backoff.min(4))
        });
        Millis(learned.clamp(floor, MAX_PROBE_TIMEOUT.0.max(floor)))
    }

    /// `node` answered a probe after `rtt`.
    pub fn answered_in(&mut self, node: NodeId, rtt: Millis) {
        let sample = rtt.0;
        let r = self.rtt.entry(node).or_default();
        if r.smoothed == 0 && r.variation == 0 {
            r.smoothed = sample;
            r.variation = sample / 2;
        } else {
            r.variation = (3 * r.variation + r.smoothed.abs_diff(sample)) / 4;
            r.smoothed = (7 * r.smoothed + sample) / 8;
        }
        r.backoff = 0;
    }

    /// A probe to `node` got no answer in time: wait twice as long next time, until it answers.
    pub fn no_answer_from(&mut self, node: NodeId) {
        let r = self.rtt.entry(node).or_default();
        r.backoff = r.backoff.saturating_add(1).min(4);
    }

    #[must_use]
    pub fn suspects(&self) -> usize {
        self.suspect_since.len()
    }
}

/// How often a quiet node is probed, by each peer, and how often it probes (ADR-0078).
pub const QUIET_PROBE_EVERY: Millis = Millis(60_000);
/// How long a quiet node may be suspected before it is called dead: long enough to hear it and
/// refute at its next probe, with a missed one to spare.
pub const QUIET_SUSPECT_TIMEOUT: Millis = Millis(150_000);

/// How many rounds pass for each one spent on a node already concluded dead.
pub const DEAD_EVERY: u64 = 30;

/// Nodes worth probing: everybody but us, minus those who told us they were going.
fn probeable(view: &ClusterView) -> impl Iterator<Item = NodeId> + '_ {
    view.nodes
        .values()
        .filter(|n| n.id != view.local)
        .filter(|n| !matches!(n.status, NodeStatus::Draining | NodeStatus::Departed))
        .map(|n| n.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{Arch, Capabilities, DeviceClass, NodeView, Os, WorkPolicy};

    fn id(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    /// A LAN peer keeps the configured timeout, a slow one earns more, a timeout doubles the
    /// wait until the next answer, and nothing waits past `MAX_PROBE_TIMEOUT` (the phone on LTE).
    #[test]
    fn a_probe_timeout_is_learned_per_peer() {
        let mut d = Detector::new(DetectorConfig::default());
        let (lan, phone) = (id(2), id(3));
        assert_eq!(
            d.timeout_for(lan),
            Millis(500),
            "unknown: the configured timeout"
        );
        for _ in 0..20 {
            d.answered_in(lan, Millis(16));
        }
        assert_eq!(
            d.timeout_for(lan),
            Millis(500),
            "a fast peer never goes under it"
        );
        for rtt in [82, 400, 60, 450, 90, 480, 70, 410, 85, 495] {
            d.answered_in(phone, Millis(rtt));
        }
        let slow = d.timeout_for(phone);
        assert!(slow > Millis(700), "a jittery peer earns more: {slow:?}");
        d.no_answer_from(phone);
        assert_eq!(
            d.timeout_for(phone),
            Millis((slow.0 * 2).min(3_000)),
            "doubled after a miss"
        );
        for _ in 0..5 {
            d.no_answer_from(phone);
        }
        assert_eq!(d.timeout_for(phone), MAX_PROBE_TIMEOUT, "capped");
        d.answered_in(phone, Millis(90));
        assert!(
            d.timeout_for(phone) < MAX_PROBE_TIMEOUT,
            "an answer ends the backoff"
        );
    }

    /// ADR-0078: a quiet node is probed at most once per `QUIET_PROBE_EVERY` while the others keep
    /// their rotation, and it is given `QUIET_SUSPECT_TIMEOUT` before a suspicion is a conclusion.
    #[test]
    fn a_quiet_node_is_probed_once_a_minute_and_suspected_patiently() {
        let mut view = view_with(1, &[2, 3]);
        if let Some(node) = view.nodes.get_mut(&id(3)) {
            node.quiet = true;
        }
        let mut detector = Detector::new(DetectorConfig::default());
        let mut quiet_turns = 0;
        for second in 0..120u64 {
            if detector.next_target(&view, Millis(second * 1_000)) == Some(id(3)) {
                quiet_turns += 1;
            }
        }
        assert_eq!(quiet_turns, 2, "once at the start, once a minute later");

        detector.unreachable(id(3), Millis(0));
        detector.unreachable(id(2), Millis(0));
        let at = |ms| detector.expired(&view, Millis(ms));
        assert_eq!(
            at(6_000),
            vec![id(2)],
            "the ordinary node after 5 s, the quiet one not"
        );
        assert!(at(149_000).contains(&id(2)) && !at(149_000).contains(&id(3)));
        assert!(at(150_000).contains(&id(3)), "the quiet one after 150 s");
    }

    /// A node already concluded dead gets one round in `DEAD_EVERY`, and the live share the
    /// rest. It took a full turn in every rotation, hours after it had gone (session ninety-two).
    #[test]
    fn a_dead_node_is_visited_one_round_in_dead_every() {
        let mut view = view_with(1, &[2, 3, 4]);
        if let Some(node) = view.nodes.get_mut(&id(4)) {
            node.status = NodeStatus::Dead;
        }
        let mut detector = Detector::new(DetectorConfig::default());
        let rounds = 2 * DEAD_EVERY;
        let mut seen: BTreeMap<NodeId, u64> = BTreeMap::new();
        for _ in 0..rounds {
            if let Some(target) = detector.next_target(&view, Millis(0)) {
                *seen.entry(target).or_default() += 1;
            }
        }
        assert_eq!(seen.get(&id(4)).copied(), Some(2), "{seen:?}");
        assert_eq!(
            seen.get(&id(2)).copied().unwrap_or(0) + seen.get(&id(3)).copied().unwrap_or(0),
            rounds - 2
        );
        assert!(
            seen.get(&id(2)).copied().unwrap_or(0) >= 28,
            "the live alternate: {seen:?}"
        );

        // With nobody alive, every round is the dead's, and they rotate round by round: counted in
        // dead rounds, one of them was picked thirty times running (the Mac, session ninety-two).
        let mut lonely = view_with(1, &[4, 5]);
        for b in [4, 5] {
            if let Some(node) = lonely.nodes.get_mut(&id(b)) {
                node.status = NodeStatus::Dead;
            }
        }
        let first = detector.next_target(&lonely, Millis(0));
        let second = detector.next_target(&lonely, Millis(0));
        assert!(first.is_some() && second.is_some());
        assert_ne!(
            first, second,
            "both dead nodes are visited, one after the other"
        );
    }

    fn view_with(local: u8, others: &[u8]) -> ClusterView {
        let mut view = ClusterView::new(id(local));
        for b in std::iter::once(&local).chain(others) {
            view.upsert_node(NodeView::new(
                id(*b),
                Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
                WorkPolicy::for_class(DeviceClass::Desktop),
                Millis(0),
            ));
        }
        view
    }

    #[test]
    fn the_rotation_visits_everybody_and_never_ourselves() {
        // Detection time is bounded by the rotation covering the fleet; a target that can be
        // skipped is a node that can be unreachable for ever without anybody noticing.
        let view = view_with(1, &[2, 3, 4]);
        let mut detector = Detector::new(DetectorConfig::default());

        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(detector.next_target(&view, Millis(0)).expect("a target"));
        }
        seen.sort();
        assert_eq!(seen, vec![id(2), id(3), id(4)]);
        assert!(!seen.contains(&id(1)));
    }

    #[test]
    fn a_fleet_of_one_has_nobody_to_probe() {
        let view = view_with(1, &[]);
        let mut detector = Detector::new(DetectorConfig::default());
        assert_eq!(detector.next_target(&view, Millis(0)), None);
    }

    #[test]
    fn a_node_that_said_it_was_leaving_is_not_probed() {
        // It is not lost, it is gone. Probing it produces log noise at exactly the moment
        // somebody is reading the log to find out what happened.
        let mut view = view_with(1, &[2, 3]);
        view.nodes
            .get_mut(&id(2))
            .expect("node")
            .set_status(NodeStatus::Draining, Millis(0));

        let mut detector = Detector::new(DetectorConfig::default());
        for _ in 0..4 {
            assert_eq!(detector.next_target(&view, Millis(0)), Some(id(3)));
        }
    }

    #[test]
    fn a_suspect_is_still_probed_because_a_suspicion_is_a_question() {
        let mut view = view_with(1, &[2]);
        view.nodes
            .get_mut(&id(2))
            .expect("node")
            .set_status(NodeStatus::Suspect, Millis(0));

        let mut detector = Detector::new(DetectorConfig::default());
        assert_eq!(detector.next_target(&view, Millis(0)), Some(id(2)));
    }

    #[test]
    fn helpers_exclude_the_target_and_ourselves_and_the_already_suspected() {
        // Asking a node we cannot reach to relay a probe wastes the period we were trying to
        // save.
        let mut view = view_with(1, &[2, 3, 4, 5]);
        view.nodes
            .get_mut(&id(4))
            .expect("node")
            .set_status(NodeStatus::Suspect, Millis(0));

        let detector = Detector::new(DetectorConfig {
            indirect_peers: 3,
            ..DetectorConfig::default()
        });
        let helpers = detector.helpers(&view, id(2));
        assert_eq!(helpers.len(), 2, "3 and 5, and nobody else");
        assert!(!helpers.contains(&id(1)));
        assert!(!helpers.contains(&id(2)));
        assert!(!helpers.contains(&id(4)));
    }

    #[test]
    fn with_nobody_to_ask_there_is_no_indirect_probe() {
        // A two-node fleet has no second opinion available, so an unanswered probe is all the
        // evidence there is going to be.
        let view = view_with(1, &[2]);
        let detector = Detector::new(DetectorConfig::default());
        assert!(detector.helpers(&view, id(2)).is_empty());
    }

    #[test]
    fn suspicion_starts_once_and_becomes_a_conclusion_on_a_timer() {
        let mut detector = Detector::new(DetectorConfig {
            suspect_timeout: Millis(5_000),
            ..DetectorConfig::default()
        });

        assert_eq!(
            detector.unreachable(id(2), Millis(1_000)),
            Some(NodeStatus::Suspect)
        );
        // Saying it again is not news, and gossiping it again would be a change nobody made.
        assert_eq!(detector.unreachable(id(2), Millis(2_000)), None);

        assert!(
            detector
                .expired(&ClusterView::new(id(1)), Millis(5_999))
                .is_empty(),
            "not yet"
        );
        assert_eq!(
            detector.expired(&ClusterView::new(id(1)), Millis(6_000)),
            vec![id(2)]
        );
    }

    #[test]
    fn hearing_from_a_node_clears_the_suspicion_and_the_clock_with_it() {
        // The flapping case: a laptop that goes quiet for four seconds every few minutes must
        // not accumulate its way to `Dead` one suspicion at a time.
        let mut detector = Detector::new(DetectorConfig {
            suspect_timeout: Millis(5_000),
            ..DetectorConfig::default()
        });

        detector.unreachable(id(2), Millis(1_000));
        assert!(detector.heard_from(id(2)));
        assert!(detector
            .expired(&ClusterView::new(id(1)), Millis(100_000))
            .is_empty());
        assert_eq!(detector.suspects(), 0);

        // And a fresh suspicion starts its timer from now, not from the old one.
        detector.unreachable(id(2), Millis(100_000));
        assert!(detector
            .expired(&ClusterView::new(id(1)), Millis(104_000))
            .is_empty());
        assert_eq!(
            detector.expired(&ClusterView::new(id(1)), Millis(105_000)),
            vec![id(2)]
        );
    }
}
