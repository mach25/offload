//! What nodes say to each other once they are talking: liveness probes, gossip, and blobs.
//!
//! Two things travel here and they travel together. A probe is a message asking "are you
//! there"; gossip is what this node believes about everybody else. Sending them separately
//! would double the packets for no benefit, so **every probe and every answer carries
//! gossip** — which is also why a fleet converges in a few probe periods rather than needing
//! a broadcast of its own.
//!
//! The failure detector is SWIM-shaped (ADR-0005, ADR-0007): a direct probe, then an indirect
//! probe through other members, then a suspicion that the subject may refute. What is *not*
//! here is any decision about work — a `Suspect` moves no runs, and the hold-down policy in
//! `offload-core` decides what an absence means.

use offload_core::{BlobHash, NodeId, NodeView, Run, RunId, RunProgress, Service};
use serde::{Deserialize, Serialize};

/// The largest blob this protocol will move in one exchange.
///
/// A checkpoint is megabytes — a transcript, a `base..HEAD` bundle and a patch — so this is
/// generous by two orders of magnitude. It exists because a size field is attacker-controlled
/// input: without a cap, a peer's `BlobFound { size }` is an allocation request.
pub const MAX_BLOB_BYTES: u64 = 512 * 1024 * 1024;

/// One event and where it sits in the holder's log.
///
/// A struct rather than a tuple because it crosses the wire: `(u64, LogEvent)` encodes as a
/// two-element array and reads as one at the other end only for as long as nobody adds a third
/// element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeqEvent {
    pub seq: u64,
    pub event: offload_core::LogEvent,
}

/// What one node believes about the fleet, as of now.
///
/// Whole `NodeView`s rather than deltas: a fleet is tens of nodes, the views are kilobytes,
/// and every node merges by ownership (ADR-0005) so a redundant copy costs a comparison. The
/// digest-and-fetch optimisation the roadmap mentions is worth having when a view grows large
/// enough to notice — and is a protocol version, not a redesign.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Gossip {
    pub nodes: Vec<NodeView>,
    /// Runs the sender holds, so the rest of the fleet — most importantly whoever submitted
    /// one — learns what became of them (ADR-0005).
    ///
    /// Only *live* runs and recently finished ones travel. A run record is a kilobyte and a
    /// probe happens every second, so gossiping the whole history would make this the largest
    /// thing on the wire by an order of magnitude; the number of live runs is bounded by the
    /// node's own concurrency cap, which is two or four.
    #[serde(default)]
    pub runs: Vec<Run>,
    /// How far those runs have got, and what they have cost.
    ///
    /// Beside the records rather than inside them: `Run` is the domain object and has no
    /// business carrying a bill (see `offload_core::progress`), and the two are merged by
    /// different rules. Tens of bytes each, so they ride along with every probe like
    /// everything else here.
    #[serde(default)]
    pub progress: Vec<ProgressReport>,
    /// Who the sender knows has been thrown out of the fleet (ADR-0012).
    ///
    /// The one membership fact that has to travel. Everything else about belonging is settled
    /// by a signature a peer can check offline at first contact; a revocation is the absence of
    /// one, and no certificate can say "except this one". Signed by the fleet key, so where it
    /// arrived from does not matter and a peer cannot forge one — and never refutable by its
    /// subject, which is the opposite of the rule that governs every other fact in this struct.
    #[serde(default)]
    pub revocations: Vec<offload_core::Revocation>,
    /// Standing instructions on a clock (ADR-0019 §3, ADR-0056).
    ///
    /// **The fourth gossiped fact, and the only one here that is not about a node or a run.** A
    /// schedule travels because that is the whole difference between it and `cron`: it outlives
    /// the device it was created on, and a fleet where only the creator knew about it would fire
    /// nothing while that laptop was in a bag.
    ///
    /// Relayed, not merely announced — a peer sends the schedules it has learned as well as its
    /// own, which is what makes the successor rule (`ClusterView::steward_of`) able to fire one
    /// whose home is away. That is also why a **removal has to be a tombstone** rather than an
    /// absence: a schedule that simply stopped being mentioned would be re-learned from the next
    /// peer for ever (`Schedule::removed_at`, ADR-0056 §5).
    ///
    /// A handful per fleet, each a spec and four numbers, so they ride along with every probe
    /// like the rest of this struct. There is deliberately no cursor **on the wire**: what has
    /// fired is recorded by the occurrence, which is already in `runs` above, and by a
    /// node-local mark that no peer could state anyway (ADR-0056 §2).
    #[serde(default)]
    pub schedules: Vec<offload_core::Schedule>,
    /// How many times somebody has asked the fleet to read its agents' model lists again
    /// (ADR-0080), as far as the sender knows. Merged by maximum, and raising it is the only
    /// thing anybody does to it, so it needs no owner: every node orders two counts the same way.
    #[serde(default)]
    pub models_asked: u64,
}

/// What a node showed of a run's workspace (ADR-0075).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesView {
    /// The path shown, relative to the workspace root, `""` for the root itself.
    pub path: String,
    /// Which copy it was read from: the checkout, with uncommitted work, or the run's branch,
    /// once the checkout has been cleaned up.
    pub source: FilesSource,
    /// The node that read it, so a person knows which machine they are looking at.
    pub node: String,
    pub content: FilesContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesSource {
    Checkout,
    Branch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FilesContent {
    /// A directory: its entries, directories first, and whether the list was cut.
    Listing {
        entries: Vec<FileEntry>,
        truncated: bool,
    },
    /// A text file, cut at the cap, with its whole size.
    Text {
        text: String,
        bytes: u64,
        truncated: bool,
    },
    /// A file that is not text, which is shown as its size.
    Binary { bytes: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub dir: bool,
    pub bytes: u64,
}

/// One run's numbers, as the sender knows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressReport {
    pub run: RunId,
    #[serde(flatten)]
    pub progress: RunProgress,
}

impl Gossip {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.runs.is_empty()
            && self.progress.is_empty()
            && self.revocations.is_empty()
            && self.schedules.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cluster", rename_all = "snake_case")]
pub enum ClusterMessage {
    /// "Are you there?"
    Ping { seq: u64, gossip: Gossip },
    /// "I cannot reach `target` — can you?"
    ///
    /// The indirect probe is what stops one bad path from looking like a dead node: a laptop
    /// on a flaky access point is unreachable *from here* and perfectly fine from the desktop.
    PingReq {
        seq: u64,
        target: NodeId,
        gossip: Gossip,
    },
    /// "Yes." From the subject itself, or relayed by whoever we asked.
    Ack { seq: u64, gossip: Gossip },
    /// "I asked, and got nothing either."
    ///
    /// A negative answer is worth a message of its own: without it the asking node waits out
    /// its whole timeout to learn something the helper knew immediately.
    Nack { seq: u64, target: NodeId },
    /// "Do you have this blob?" — the question on its own, with no bytes attached.
    ///
    /// A `BlobRequest` answers it too and drags the payload across to do so. This exists
    /// because a declined push can mean *already held*, which is a success, and the sender
    /// has to be able to tell the difference before reporting a checkpoint durable.
    BlobHave { hash: BlobHash },
    /// The answer to [`ClusterMessage::BlobHave`].
    BlobHeld { hash: BlobHash, held: bool },
    /// "Do you have this blob, and may I have it?"
    ///
    /// Answered with [`ClusterMessage::BlobFound`] followed by exactly `size` raw bytes, or
    /// [`ClusterMessage::BlobMissing`]. The bytes are *not* framed: this is the traffic the
    /// JSON framing was explicitly not designed for, and it gets a stream of its own so that
    /// a hundred megabytes cannot delay a failure-detector probe.
    BlobRequest { hash: BlobHash },
    /// "Yes — here it comes." Followed by `size` raw bytes and the end of the stream.
    BlobFound { size: u64 },
    /// "No." Not an error: a node that garbage-collected it is behaving correctly, and the
    /// asker moves on to the next peer.
    BlobMissing { hash: BlobHash },
    /// "I am holding a checkpoint you may need. Want it?"
    ///
    /// The replication half of ADR-0016: a checkpoint that exists only on the node that died
    /// cannot be migrated from. Answered with [`ClusterMessage::BlobAccepted`], after which
    /// `size` raw bytes follow, or [`ClusterMessage::BlobDeclined`].
    BlobPush { hash: BlobHash, size: u64 },
    /// "Send it." The receiver has agreed to spend the bytes and the disk.
    BlobAccepted,
    /// "It is on my disk." Sent after the bytes have been stored and hashed.
    ///
    /// The reason a push waits for an answer at all: "accepted" means the receiver was
    /// willing, and durability means the bytes are *there*. A sender that stopped at accepted
    /// would report a checkpoint safe while it was still in flight — which is precisely the
    /// claim ADR-0016 exists to make true.
    BlobStored { hash: BlobHash },
    /// "Not now." Carries why: already held, too large, or this device's owner said no.
    BlobDeclined { reason: String },
    /// "Would you take this run, and how badly do you want it?"
    ///
    /// ADR-0006 has nodes broadcast bids after a score-proportional delay; with a transport
    /// that can address a peer directly, the arbiter asks instead. Same information, same
    /// number of messages, and the delay — which existed to stop a broadcast storm — is not
    /// needed. What is preserved is the part that matters: the node decides *locally*, with
    /// facts gossip cannot carry.
    WillYouTake { run: Box<Run> },
    /// "Yes, this much, and I can start now / when my own work frees up."
    ///
    /// The score is the node's own self-assessment. `available` is the other half of the
    /// answer and not a footnote to it: a node at its concurrency cap bids *and* says it
    /// cannot begin yet, because declining would leave nobody committed (ADR-0006).
    Bid {
        run: RunId,
        score: i64,
        available: offload_core::Availability,
        /// What `score` was summed from (ADR-0063 §7), so the arbiter can say why a preferred
        /// node lost from the numbers that node read. Defaulted: an answer without it is a bid
        /// with nothing to explain, never a refusal.
        #[serde(default)]
        terms: Option<offload_core::ScoreTerms>,
    },
    /// "No, because —". A refusal is a sentence, because "no eligible nodes" is the least
    /// useful thing this system could tell somebody (ADR-0014).
    WillNot { run: RunId, reason: String },
    /// "It is yours." The record travels with its new epoch, so the grant *is* the fencing
    /// token rather than something to be looked up.
    Grant { run: Box<Run> },
    /// "Taken." The run is this node's problem now.
    Granted { run: RunId },
    /// "On reflection, no."
    ///
    /// A grant is an offer, not an instruction (ADR-0006 step 6): a bid describes a moment
    /// and the grant arrives later, by which time the node may have won two other bids or
    /// been unplugged. A decline must never strand the run — the arbiter tries the next bid.
    Declined { run: RunId, reason: String },
    /// "Send me that run's event log, from after this sequence number."
    ///
    /// What makes `offload logs -f` work for a run that is not here (ADR-0003's promise, read
    /// from the operator's end): the node the person is talking to asks the holder, repeatedly,
    /// and forwards what comes back. **A poll rather than a subscription**, which is the same
    /// trade every other loop in this daemon makes — a poll cannot miss an event, needs no
    /// long-lived stream held open across a migration, and costs a second of lag on output whose
    /// unit is an agent turn.
    ///
    /// Sequence numbers are the *holder's*, and per node: a run that has moved has its log split
    /// across the machines that ran it, and this asks one of them.
    FetchEvents {
        run: RunId,
        /// Everything after this sequence. `0` means from the beginning.
        after: u64,
        /// How many at most, so a reply fits a frame whatever a run has been up to.
        limit: u32,
    },
    /// The answer: some events, and whether this node has anything more to come.
    ///
    /// `done` means *this holder* is finished with the run — its log ends here — and is what
    /// stops a follower waiting for output that will never arrive. It is not the same as the run
    /// being over: a released checkpoint ends this leg and leaves the run for somebody else.
    Events {
        run: RunId,
        events: Vec<SeqEvent>,
        done: bool,
    },
    /// "I have no log for that run." Not an error: a node that never ran it never wrote one, and
    /// saying so is how a follower learns to ask somebody else rather than waiting.
    NoEvents { run: RunId, reason: String },
    /// "Show me what is at this path in that run's workspace" (ADR-0075). Asked of the node that
    /// ran it, since a checkout is on one machine; read-only, and the path is relative to the
    /// workspace root, which the answering node enforces.
    FetchFiles { run: RunId, path: String },
    /// The answer: a directory's entries or a file's text, and which copy it was read from.
    Files { run: RunId, view: FilesView },
    /// "I cannot show that", in words: no checkout and no branch here, a path outside the
    /// workspace, or one that does not exist.
    NoFiles { run: RunId, reason: String },
    /// "The operator changed when this run is due, or which runs it goes ahead of."
    ///
    /// Sent to the node that *owns* the editable part of the spec — the run's home node, or its
    /// arbiter successor once the home node is gone — rather than applied where it was typed
    /// (ADR-0013). One owner counting its own writes is what makes `spec_rev` a total order; two
    /// nodes editing locally and gossiping would each undo the other at the same revision, for
    /// ever.
    ///
    /// One message for both editable fields rather than one each, for the same reason there is
    /// one counter: they have one owner, and an edit is an edit.
    EditSpec {
        run: RunId,
        edit: offload_core::SpecEdit,
    },
    /// "Done, and this is the revision I wrote." The sentence is for the person waiting.
    SpecEdited { run: RunId, rev: u32, note: String },
    /// "No." A finished run has no deadline worth arguing about, and a node that is not the
    /// owner says so rather than writing a revision nobody would honour.
    EditRefused { run: RunId, reason: String },
    /// "The operator wants this run stopped."
    ///
    /// Sent to whichever node the run is *on* — its holder, or the node that owns the record
    /// when nobody holds it — rather than applied where it was typed. A cancel has to reach a
    /// process, and a process is on one machine: `offload cancel` used to read the local
    /// process table and answer "run ab12… is not running" about a run running perfectly well
    /// on the desktop, spending money, with nothing forwarded.
    ///
    /// No epoch, and unlike [`ClusterMessage::Answer`] that is not fencing by construction —
    /// it is a decision. `Run::cancel` is unfenced on purpose (an operator action always wins,
    /// and a person who typed this does not care which leg of a migration they reached), and
    /// the terminal state it writes beats anything at the same epoch in `merge_run`. What the
    /// receiver checks is not authority but whether there is anything left to stop.
    Cancel {
        run: RunId,
        /// Who asked, for the run's log — the name of the node it was typed at, like
        /// [`ClusterMessage::Answer`]'s `by`. Not an authorisation: that was settled by the
        /// handshake.
        by: String,
    },
    /// "Stopped, and this is what was stopped." The distinction is the point: an agent that was
    /// mid-turn had spent money and a commitment that had not started had not, and the person
    /// who typed the command is owed the difference.
    Cancelled { run: RunId, note: String },
    /// "There is nothing here to stop, because —." A run that finished a second ago, or a
    /// record this node does not have.
    CancelRefused { run: RunId, reason: String },
    /// "Build the base for a continuation of this finished run" (ADR-0064 §2).
    ///
    /// Sent to the node that ran the parent's **last leg** — `RunProgress::by`, which is what
    /// `offload logs` routes by too — because that is where the parent's worktree is, and the
    /// base is a capture of it. Only the base travels back: the continuation itself is made by
    /// the node it was typed at, so its home and a `--prefer here` mean the machine the person is
    /// at. Nothing is written into the parent's checkout; the capture touches only the index.
    ContinueBase {
        run: RunId,
        /// `--session`: the parent's agent session goes into the base, and a parent whose
        /// transcript cannot be found is refused rather than handed over without it.
        session: bool,
    },
    /// "Here is the base." The blobs it names are on the answering node; the asker fetches them
    /// before it makes the run.
    ContinueBaseBuilt {
        run: RunId,
        base: Box<offload_core::ContinuationBase>,
    },
    /// "Not from here, because —." Its worktree was removed, its last leg was elsewhere after
    /// all, or `--session` found no transcript.
    ContinueBaseRefused { run: RunId, reason: String },
    /// "Capture this run at its next turn boundary and hand it back."
    ///
    /// Sent to the **holder**, and to nobody else there is any point sending it to: what is being
    /// asked for is a pause in a *process*, at a moment only that process reaches (ADR-0004), so
    /// a node that is not running the agent has nothing it could do with this. The sibling of
    /// [`ClusterMessage::Cancel`] with the record half removed — a run nobody holds has no turn
    /// boundary coming and is already back in the pool, which is where a checkpoint would put it.
    ///
    /// Nothing else travels: not "now", which is unsafe, and not a deadline, because the holder
    /// already knows its own drain deadline and the operator's patience is the CLI's business.
    Checkpoint { run: RunId },
    /// "Asked for. It will be taken at the next boundary." Future tense on purpose — the agent
    /// may be minutes into a turn, and saying it is done would be a lie for the rest of it.
    CheckpointRequested { run: RunId },
    /// "No, because —." Most often a run that has stopped being ours between the operator typing
    /// and this arriving.
    CheckpointRefused { run: RunId, reason: String },
    /// "Tell somebody this, through your route called `sink`." (ADR-0010)
    ///
    /// Sent by the node that *logged* the event to the node that can reach a person. The
    /// notification is already projected — a small typed subset of run events — because deciding
    /// what deserves a human's attention happens at the source, which is what keeps every sink
    /// dumb and interchangeable.
    ///
    /// **What is absent is the point**: no command, no credential, no address. The asker chose
    /// this route from the capability the peer advertises, and names it by that id; how the peer
    /// then reaches the person is its own business and never leaves it. A design that needed to
    /// ship a push token to a peer would be the one ADR-0002 tells us to stop at.
    ///
    /// The sender keeps the outbox, so this message carries no retry state: being asked twice is
    /// the at-least-once contract working, and a peer that answers `Undelivered` will be asked
    /// again until the sender gives up and says so.
    Deliver {
        sink: String,
        note: Box<offload_core::Notification>,
    },
    /// "It went to a route." Not "a human has read it", which nothing can promise.
    Delivered { sink: String },
    /// "No, because —." Goes back into the sender's outbox and surfaces in `offload sinks` on
    /// the machine that was trying to tell somebody.
    Undelivered { sink: String, reason: String },
    /// "Somebody answered a question your agent is blocked on." (ADR-0017)
    ///
    /// Forwarded to the **holder**, because that is where the blocked process is — an answer
    /// applied anywhere else is an answer nobody is waiting for. That makes this the mirror of
    /// `EditSpec`, which goes to the field's owner for the same kind of reason.
    ///
    /// No epoch, and that is not an omission. The identity of a question is the agent's own
    /// `tool_use_id`, which only exists while *that* agent process is blocked on *that* call: a
    /// run that has migrated has a new leg, a new agent and new ids, so a stale answer matches
    /// nothing and is refused by having nowhere to go. Fencing by construction rather than by a
    /// check that could be forgotten.
    ///
    /// `tool_use_id` is optional for the same reason the CLI's argument is: a run blocked on one
    /// thing needs no disambiguation, and a run blocked on three refuses to guess.
    Answer {
        run: RunId,
        tool_use_id: Option<String>,
        allow: bool,
        /// Who answered, for the run's log — the name of the node it was typed at. Not an
        /// authorisation: that was settled by the handshake, which is where a revoked device
        /// stops being able to say anything at all.
        by: String,
    },
    /// "Done, and this is what you just decided." The question, said back.
    ///
    /// An agent can be blocked on several calls at once, so "approved" without saying what would
    /// be the wrong kind of reassuring.
    AnswerTaken {
        run: RunId,
        tool: String,
        detail: String,
        allowed: bool,
    },
    /// "There is nothing here waiting for that." Includes the answer that arrived a moment too
    /// late, which is a race rather than a fault: patience ran out while somebody was reaching
    /// for their phone.
    AnswerRefused { run: RunId, reason: String },
    /// "What are your agents waiting for?" (ADR-0017)
    ///
    /// Asked when somebody types `offload asks`, rather than gossiped, for `explain`'s reason: a
    /// blocked process stops being blocked the moment somebody answers, so a remembered copy is
    /// a question that has already been dealt with, presented as current.
    FetchAsks,
    /// The answer: what this node's agents are waiting on, with the clocks read here.
    Asks { asks: Vec<offload_core::PendingAsk> },
    /// "One of my runs was granted a resource you hold — open it." (ADR-0011)
    ///
    /// The first message of a stream that then carries the agent's own protocol, one JSON-RPC
    /// message per frame, until either end finishes. What travels is the *call*, never the
    /// credential: the holder learns which run is asking and which service it was granted, and
    /// the caller learns nothing about how that service is reached.
    ///
    /// Named by **service** and not by the holder's own id for the resource, for the reason an
    /// audience is: an id is a node's private name for one of its own things, and a run naming
    /// one would be a run pinned to a device.
    UseResource { run: RunId, service: Service },
    /// "It is open." Carries the holder's own id for it, for the log at both ends — the asking
    /// node shows it in `offload status`, and it is what tells two mailboxes apart afterwards.
    ResourceReady { id: String },
    /// "No, because —." A sentence, like every other refusal here: a run that cannot reach the
    /// mailbox it was granted has to be able to say which of the several reasons it was.
    ResourceRefused { reason: String },
    /// One line of the agent's protocol, in whichever direction.
    ///
    /// Opaque on purpose. This is the point at which Offload stops understanding what is being
    /// said and starts carrying it, which is the same line `offload-agent` draws around an
    /// agent's own stream: parsing it here would be reimplementing somebody else's protocol in
    /// the middle of a proxy.
    ResourceData { line: String },
    /// "That is all." Sent by whichever end finishes first; the other tears down.
    ResourceClosed { reason: String },
    /// "My certificate is running out — will you re-issue it?" (ADR-0012)
    ///
    /// **Carries the asker's current credentials** (ADR-0069 §2). It used to carry nothing, so
    /// that the certificate renewed was the one the handshake authenticated — and a live session
    /// never handshakes again. Measured with four-minute certificates on two daemons: the first
    /// renewal worked, and every later one was refused as `already lapsed`, because the renewer
    /// was still looking at the certificate the connection opened with. On a real fleet, any
    /// pair of nodes holding one connection longer than a certificate's life renews once and then
    /// lapses. A renewer verifies what it is shown and takes it only for the asker itself; a
    /// certificate minted for anybody else would be useless to the asker anyway, since the
    /// handshake demands the key it names. `None` falls back to the handshake's certificate.
    ///
    /// Asked rather than offered, because the node whose certificate is lapsing is the one that
    /// knows and the only one that suffers. An approver volunteering renewals would have to
    /// track every peer's expiry, and would renew nothing for the device that has been shut for
    /// three weeks and is the one that needs it.
    RenewMe {
        #[serde(default)]
        credentials: Option<Box<crate::handshake::Credentials>>,
    },
    /// "Here." The new certificate, and the delegation that authorised whoever signed it.
    ///
    /// The chain is not optional in practice: an approver signs with its own node key, so a peer
    /// verifying the result needs the fleet-signed statement that this node may issue. Sending
    /// the certificate without it would hand somebody papers nobody can check.
    Renewed {
        credentials: Box<crate::handshake::Credentials>,
    },
    /// "I cannot." Not an error and not unusual: most members are not approvers, and a fleet
    /// where every device could re-issue certificates would be a fleet with no approvers at all.
    NotRenewed { reason: String },
    /// "I am going away." Believed immediately — a node knows when it is leaving (ADR-0005),
    /// and this is what turns a shutdown into a graceful drain rather than a detected failure.
    ///
    /// Answered like a probe, because a departing node wants to know it was heard: the whole
    /// value of announcing is that nobody spends a detection timeout on you, and an
    /// announcement that arrived after the process exited bought nothing.
    Leaving { seq: u64, gossip: Gossip },
}

impl ClusterMessage {
    /// The gossip riding along with this message, if any.
    #[must_use]
    pub fn gossip(&self) -> Option<&Gossip> {
        match self {
            ClusterMessage::Ping { gossip, .. }
            | ClusterMessage::PingReq { gossip, .. }
            | ClusterMessage::Ack { gossip, .. }
            | ClusterMessage::Leaving { gossip, .. } => Some(gossip),
            _ => None,
        }
    }

    #[must_use]
    pub fn seq(&self) -> Option<u64> {
        match self {
            ClusterMessage::Ping { seq, .. }
            | ClusterMessage::PingReq { seq, .. }
            | ClusterMessage::Ack { seq, .. }
            | ClusterMessage::Nack { seq, .. }
            | ClusterMessage::Leaving { seq, .. } => Some(*seq),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{AgentWork, Work};
    use offload_core::{Arch, Capabilities, DeviceClass, Millis, Os, WorkPolicy};

    /// A run, for the round-trip test. Placement messages carry the whole record — the grant
    /// *is* the fencing token — so this is the largest thing on the wire and the one most
    /// worth proving decodes.
    /// A schedule, as the fourth gossiped fact (ADR-0056). On the wire because a schedule
    /// outlives the device that created it, and relayed because that is what makes the
    /// successor rule able to fire one whose home is away.
    fn a_schedule() -> offload_core::Schedule {
        offload_core::Schedule {
            id: offload_core::ScheduleId::from_bytes([6; 8]),
            home: NodeId::from_bytes([1; 32]),
            every: offload_core::Millis::from_mins(15),
            offset: offload_core::Millis::ZERO,
            spec: a_run().spec,
            note: "watch the API".into(),
            created_at: offload_core::Millis(1_700_000_000_000),
            // A tombstone on the wire, because that is the copy whose loss is not merely a
            // missing feature: a removal that does not travel is a schedule that fires for ever.
            removed_at: Some(offload_core::Millis(1_700_000_600_000)),
        }
    }

    fn a_run() -> Run {
        use offload_core::{
            AgentKind, Constraint, PermissionMode, Restartability, RunSpec, ToolAllowlist,
            WorkspaceSpec,
        };
        Run::new(
            RunId::from_bytes([4; 16]),
            RunSpec {
                work: Work::Agent(AgentWork {
                    agent: AgentKind::ClaudeCode,
                    model: None,
                    prompt: "add tests for the parser".into(),
                    workspace: WorkspaceSpec {
                        repo: "https://example.com/me/api.git".into(),
                        archive_bytes: None,
                        git_ref: None,
                        branch: None,
                    },
                    permission_mode: PermissionMode::AcceptEdits,
                    allow: ToolAllowlist::default(),
                    max_turns: None,
                    ask: offload_core::AskPolicy::Never,
                }),
                constraint: Constraint::agent_ready(AgentKind::ClaudeCode, None),
                restartability: Restartability::Resumable,
                priority: 0,
                queue: false,
                deadline: None,
                demand: Default::default(),
                notify: Default::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: offload_core::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            NodeId::from_bytes([1; 32]),
            Millis(0),
        )
    }

    fn view(b: u8) -> NodeView {
        NodeView::new(
            NodeId::from_bytes([b; 32]),
            Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
            WorkPolicy::for_class(DeviceClass::Desktop),
            Millis(0),
        )
    }

    #[test]
    fn every_message_round_trips_in_both_directions() {
        // Encoding alone proves nothing: `Response::Runs(Vec<_>)` compiled and failed at
        // runtime for exactly this shape of enum.
        let gossip = Gossip {
            nodes: vec![view(1), view(2)],
            runs: vec![a_run()],
            progress: vec![ProgressReport {
                run: a_run().id,
                progress: RunProgress {
                    at: offload_core::Millis(1_700_000_000_000),
                    turns: 8,
                    denials: 1,
                    cost_micro_usd: 83_000,
                    workspace: "2 modified".into(),
                    // The leg these numbers came from, which is what makes them rankable against
                    // another leg's rather than merely larger. On the wire because a relay has to
                    // pass it through untouched.
                    by: Some(offload_core::NodeId::from_bytes([3; 32])),
                    epoch: offload_core::run::Epoch(4),
                    ..RunProgress::default()
                },
            }],
            // Signed with an arbitrary key: what is under test here is the shape on the wire,
            // and a revocation whose signature does not verify still has to decode — otherwise
            // a forgery would be a parse error on the whole probe rather than one fact refused.
            revocations: vec![offload_core::Revocation::issue(
                &ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
                offload_core::FleetId::from_bytes([9; 32]),
                NodeId::from_bytes([5; 32]),
                offload_core::Millis(1_700_000_000_000),
                1,
            )],
            schedules: vec![a_schedule()],
            // Non-zero, so a codec that dropped it would be caught rather than read as default.
            models_asked: 3,
        };
        let messages = vec![
            ClusterMessage::Ping {
                seq: 1,
                gossip: gossip.clone(),
            },
            ClusterMessage::PingReq {
                seq: 2,
                target: NodeId::from_bytes([3; 32]),
                gossip: Gossip::default(),
            },
            ClusterMessage::Ack { seq: 3, gossip },
            ClusterMessage::Answer {
                run: RunId::from_bytes([4; 16]),
                tool_use_id: Some("toolu_01ABC".into()),
                allow: true,
                by: "phone".into(),
            },
            // The un-named form, which is what `offload approve <run>` sends: an `Option`
            // inside an internally-tagged enum is the shape that has failed at runtime here.
            ClusterMessage::Answer {
                run: RunId::from_bytes([4; 16]),
                tool_use_id: None,
                allow: false,
                by: "laptop".into(),
            },
            ClusterMessage::AnswerTaken {
                run: RunId::from_bytes([4; 16]),
                tool: "Bash".into(),
                detail: "cargo build --release".into(),
                allowed: true,
            },
            ClusterMessage::AnswerRefused {
                run: RunId::from_bytes([4; 16]),
                reason: "nothing here is waiting for that".into(),
            },
            ClusterMessage::Cancel {
                run: RunId::from_bytes([4; 16]),
                by: "phone".into(),
            },
            ClusterMessage::Cancelled {
                run: RunId::from_bytes([4; 16]),
                note: "the agent was stopped mid-turn".into(),
            },
            ClusterMessage::CancelRefused {
                run: RunId::from_bytes([4; 16]),
                reason: "it completed 4s ago".into(),
            },
            ClusterMessage::ContinueBase {
                run: RunId::from_bytes([4; 16]),
                session: true,
            },
            ClusterMessage::ContinueBaseBuilt {
                run: RunId::from_bytes([4; 16]),
                base: Box::new(offload_core::ContinuationBase {
                    checkpoint: offload_core::Checkpoint {
                        session_id: None,
                        transcript: BlobHash::from_bytes([7; 32]),
                        bundle: Some(BlobHash::from_bytes([8; 32])),
                        patch: None,
                        base_commit: "eedc48d1".into(),
                        turns: 0,
                        taken_at: Millis(1),
                        agent_version: "0".into(),
                        replicas: std::collections::BTreeSet::new(),
                    },
                    transcript: None,
                    closing_message: Some("done".into()),
                    summary: "1 commit(s)".into(),
                }),
            },
            ClusterMessage::ContinueBaseRefused {
                run: RunId::from_bytes([4; 16]),
                reason: "its worktree is no longer on this node".into(),
            },
            ClusterMessage::Checkpoint {
                run: RunId::from_bytes([4; 16]),
            },
            ClusterMessage::CheckpointRequested {
                run: RunId::from_bytes([4; 16]),
            },
            ClusterMessage::CheckpointRefused {
                run: RunId::from_bytes([4; 16]),
                reason: "it is not running here any more".into(),
            },
            ClusterMessage::FetchAsks,
            ClusterMessage::Asks {
                asks: vec![offload_core::PendingAsk {
                    run: RunId::from_bytes([4; 16]),
                    node: None,
                    node_name: None,
                    tool_use_id: "toolu_01ABC".into(),
                    tool: "Bash".into(),
                    detail: "cargo build --release".into(),
                    waiting: Millis(12_000),
                    left: Millis(288_000),
                }],
            },
            ClusterMessage::Nack {
                seq: 4,
                target: NodeId::from_bytes([5; 32]),
            },
            ClusterMessage::Leaving {
                seq: 5,
                gossip: Gossip::default(),
            },
            ClusterMessage::BlobRequest {
                hash: BlobHash::from_bytes([7; 32]),
            },
            ClusterMessage::BlobFound { size: 4_096 },
            ClusterMessage::BlobHave {
                hash: BlobHash::from_bytes([7; 32]),
            },
            ClusterMessage::BlobHeld {
                hash: BlobHash::from_bytes([7; 32]),
                held: true,
            },
            ClusterMessage::BlobMissing {
                hash: BlobHash::from_bytes([7; 32]),
            },
            ClusterMessage::BlobPush {
                hash: BlobHash::from_bytes([8; 32]),
                size: 12,
            },
            ClusterMessage::BlobAccepted,
            ClusterMessage::BlobStored {
                hash: BlobHash::from_bytes([8; 32]),
            },
            ClusterMessage::BlobDeclined {
                reason: "already held".into(),
            },
            ClusterMessage::WillYouTake {
                run: Box::new(a_run()),
            },
            ClusterMessage::Bid {
                run: a_run().id,
                score: 42,
                available: offload_core::Availability::WhenFree { behind: 2 },
                terms: Some(offload_core::ScoreTerms {
                    preferred: 60,
                    ..offload_core::ScoreTerms::default()
                }),
            },
            ClusterMessage::WillNot {
                run: a_run().id,
                reason: "battery 22%, policy floor is 40%".into(),
            },
            ClusterMessage::Grant {
                run: Box::new(a_run()),
            },
            ClusterMessage::Granted { run: a_run().id },
            ClusterMessage::Declined {
                run: a_run().id,
                reason: "busy since bidding".into(),
            },
        ];

        for message in messages {
            let frame = crate::frame::encode(&message).expect("encode");
            let mut reader = crate::FrameReader::new();
            reader.feed(&frame);
            let body = reader.next_frame().expect("read").expect("complete");
            assert_eq!(
                crate::frame::decode::<ClusterMessage>(&body).expect("decode"),
                message
            );
        }
    }

    #[test]
    fn a_probe_carries_gossip_and_a_refusal_to_answer_does_not() {
        // Nack is the one message with nothing to say beyond "no": it is sent by a node that
        // could not reach the target, so it has no news about it worth spreading.
        let gossip = Gossip {
            nodes: vec![view(1)],
            runs: Vec::new(),
            progress: Vec::new(),
            revocations: Vec::new(),
            schedules: Vec::new(),
            models_asked: 0,
        };
        assert!(ClusterMessage::Ping { seq: 1, gossip }.gossip().is_some());
        assert!(ClusterMessage::Nack {
            seq: 1,
            target: NodeId::from_bytes([2; 32])
        }
        .gossip()
        .is_none());
    }
}
