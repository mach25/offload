//! What Offload nodes say to each other.
//!
//! Two things live here and nothing else: the **framing** that turns a stream of bytes into
//! messages, and the **meaning** of those messages — most importantly, the rule that decides
//! whether a stranger who has just connected is a member of this fleet.
//!
//! Both are synchronous and I/O-free on purpose (ADR-0015). Framing is an incremental parser
//! fed by whoever did the reading; admission is a pure function of the credentials presented,
//! the fleet key we already hold, and an injected `now`. The transport underneath can be
//! quinn, an in-memory pair of queues, or whatever answers phase 5, and none of this changes.
//!
//! ## Versioning
//!
//! Every message travels under a negotiated [`Version`]. Nodes upgrade at different times —
//! a phone lags a desktop by weeks — so the wire format has to tolerate a fleet that
//! disagrees with itself. The handshake exchanges what each side supports and picks the
//! highest version both understand, or refuses with both ranges in the message, because
//! "incompatible" without the numbers is the least useful thing a distributed system can say.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod cluster;
pub mod frame;
pub mod handshake;

pub use cluster::{ClusterMessage, Gossip};
pub use frame::{FrameError, FrameReader, MAX_FRAME_BYTES};
pub use handshake::{Handshake, Hello, Peer, Refusal, Welcome};

use serde::{Deserialize, Serialize};

/// The wire protocol version this build speaks.
///
/// Bump it when a message changes shape in a way an older node would misread. Adding an
/// optional field that older nodes ignore is not that; removing one, or changing what a field
/// means, is.
///
/// **v2** reshaped `Capabilities` into capability instances with roles (ADR-0011). A v1 node
/// would decode a v2 gossip message into a device with no agent and place nothing on it —
/// silently, which is exactly the failure a version number exists to convert into a refusal.
///
/// **v3** added `available` to a bid (ADR-0006's accepting-without-starting). Strictly this is
/// a new field an older node could ignore — and ignoring it is the failure: a v2 arbiter reads
/// a busy node's "I'll take it, but not yet" as "I'll take it now" and hands it work ahead of
/// a node that was free. Quietly slower is the kind of wrong a version number is for.
///
/// **v4** made a run's deadline changeable after submission (ADR-0013): `SetDeadline` and its
/// two answers, plus `deadline_rev` on the run record. This one *is* the "optional field an
/// older node ignores" case, and it is a bump anyway, because ignoring the revision is how a
/// v3 node relays the deadline it started with and quietly undoes an edit every second — the
/// one failure the counter exists to prevent.
///
/// **v5** put a `Demand` on the run (ADR-0013's budget). Another optional field an older node
/// ignores, and another bump for the same reason as v4: a v4 node decodes a heavy run as an
/// ordinary one, takes two of them onto a machine that should hold one, and serves both badly
/// — while `explain` on either node shows a fleet that looks healthy. Overcommitting quietly
/// is the failure this number exists to turn into a refusal.
///
/// **v6** made priority editable alongside the deadline, which turned `deadline_rev` into
/// `spec_rev` and `SetDeadline` into `EditSpec` — one owner, one counter and one message for the
/// whole editable part of a spec, because they are one mechanism and a second copy of it is a
/// second merge rule to get wrong. A rename in a serialised struct is a bump whatever else it is.
///
/// **v7** carries a run's event log between nodes (`FetchEvents`/`Events`), which is what lets
/// somebody follow a run that is not on the machine they are typing at. New messages an older
/// node would simply not understand, so the bump is the point rather than a formality.
///
/// **v8** added a run event for a deadline that is not going to be made (ADR-0013), and a
/// reset instant to the rate-limit event that was always sent and always dropped. A log page
/// is a sequence of `LogKind`, which is internally tagged, so a v7 node asked to decode one
/// containing the new variant fails the whole page rather than skipping a line — a follower
/// would see an error where the log used to be. In practice `MIN_VERSION` tracks `VERSION`, so
/// no v7 node is in the fleet to try; the bump is recorded properly anyway, because the day
/// those two numbers stop moving together is not the day to discover that a log format is part
/// of the protocol.
///
/// **v9** split a failed *capture* out of `LogKind::Failed`, which is the run's own state. A v8
/// node decoding the new variant fails the log page it is in, the same as v8's own addition —
/// and the reason this is a protocol change at all is the reason it was a bug: a log kind is
/// read by everything downstream, so borrowing the terminal one for an operation inside a
/// healthy run hung up every follower and notified a failure that had not happened.
///
/// **v10** put an *audience* on a run's spec (ADR-0010): which service a run's news is to be
/// carried by, or nobody. A gossiped `RunSpec` is what a node reads to decide how to deliver
/// news about a run it took over, and a v9 node decodes the field's absence as the default —
/// which is *tell everybody*. So the failure is quiet and in the wrong direction: a run whose
/// owner said "just push it to my phone" migrates to an older node and mails, chats and toasts
/// them instead. Widening an audience is not as bad as dropping a run, and it is exactly the
/// kind of silent difference this number exists to prevent.
///
/// **v11** is the approval channel (ADR-0017): a spec that says whether a run may stop and ask a
/// person, and two log kinds — the question and the answer. Both halves are the usual reasons
/// rather than new ones. A v10 node decodes the missing `ask` as `false`, so a run that migrated
/// there would silently stop being able to ask and would be denied as it always was; and a log
/// page containing the new kinds fails to decode wholesale on a v10 follower, the same as v8's
/// and v9's additions. The first is quiet and the second is loud, which is exactly the pair this
/// number exists to keep out of one fleet.
///
/// **v12** lets an answer travel (ADR-0017): `Answer` to the node whose agent is blocked, and
/// `FetchAsks` for what a peer is waiting on. New messages an older node would not understand,
/// which is the bump being the point rather than a formality — and the reason the pair is worth
/// having is that without it a question reaches a phone that cannot act on it.
///
/// **v13** carries an account's ceiling: `WorkPolicy::max_concurrent_account`, the number of
/// runs a device will have going on one agent *account* at once, fleet-wide (ADR-0013). It rides
/// in the policy rather than in the agent capability because it is the owner's permission rather
/// than a fact about the hardware — and it has to be gossiped at all because enforcing an
/// account's limit means every node on that login can see it, and the binding limit is the
/// lowest any of them claims.
///
/// Bumped rather than waved through on `serde(default)`, which it does carry: an older node
/// would deserialise the field away and then *start runs past a cap it cannot see*. That is the
/// quiet kind of divergence this number exists for, and the distinction from `Gossip::progress`
/// below is the one that decides it — ignoring progress costs a dash in `ps`, ignoring this
/// costs an account its rate limit while every node believes it is behaving.
///
/// **v14** widens the approval channel to cover a run's edits, which turns `RunSpec::ask` from a
/// flag into a *budget* (`AskPolicy`) and adds the log kind that says a run has spent it. The
/// budget travels because the interruption it bounds belongs to the run rather than to a leg of
/// it — so `RunProgress` grows an `asks` count beside the denials, and a migration continues
/// spending what the operator agreed to instead of handing the new node a fresh twenty.
///
/// A shape change in a serialised field, so it is a bump whatever else it is; and the direction
/// of the failure without one is the quiet one. A v13 node reads `"never"` where a number was
/// meant — every run it takes over silently stops being able to ask, which is precisely what a
/// run under `--permission ask` cannot survive, because under that mode the agent gates every
/// edit and the whole run is denied one call at a time.
///
/// **v15** carries what a run may *use*: `RunSpec::resources`, the services a run was granted
/// the use of (ADR-0011), and `Constraint::CanUse` in the tree that places it. Both directions
/// of the failure without a bump are bad in the way this number exists for. A v14 node decodes
/// the grant away and starts a run that reaches nothing — silently, since the agent simply has
/// no such tool and says so in prose nobody reads until morning. And a v14 node decoding the new
/// constraint variant fails the whole message, which is the loud half arriving in the same
/// change.
///
/// **v16** carries revocations in gossip (ADR-0012). The one membership fact that has to
/// travel: everything else about belonging is a signature a peer checks offline at first
/// contact, and a revocation is the *absence* of one, so no certificate can carry it. It rides
/// with every probe like the rest of the view, and the failure without a bump is the quiet
/// direction again — a v15 node deserialises the field away and goes on talking to a device the
/// fleet threw out an hour ago, while `offload fleet` on the machine that typed the command says
/// it is revoked.
///
/// **v17** is certificate renewal (ADR-0012): `RenewMe`, and the two answers. New messages an
/// older node would not understand, so the bump is the point rather than a formality — and the
/// reason the pair is worth having is that without it a fleet does not decay towards safety
/// after thirty days, it stops existing, on every device at once, with the passphrase as the
/// only way back on each of them.
///
/// **v18** puts a *subject* on a notification (ADR-0012 mitigation 4). `ClusterMessage::Deliver`
/// carries one, and it is no longer necessarily a run: an enrolment and a use of the passphrase
/// are news about the fleet, and the plane could only address runs. A v17 node fails to decode
/// the message rather than mis-reading it, which is the loud direction — and the right one here,
/// because the notification this exists to carry is the one that says somebody may have enrolled
/// a device you did not.
///
/// **v19** carries a call to a resource on another machine (ADR-0011): `UseResource` and the
/// four messages that follow it. New messages an older node would not understand, so the bump is
/// the point rather than a formality — and what makes the pair worth having is the rule it
/// implements, which is that the *call* moves and the credential never does.
///
/// **v20** is the field that made a renewal checkable: `MembershipCert::authority`, the
/// fleet-signed certificate a renewal restates, which is what lets a verifier tell *minting* a
/// grant from *restating* one and so keep `HostRuns` behind the passphrase even against a
/// compromised approver. It is not a message and not negotiable — a credential's signed bytes
/// are a wire format in the same sense the fleet key derivation is, and adding a field to them
/// invalidates every signature in every fleet. **There is no migration for a signature**, so the
/// bump is a marker rather than a compatibility story: every device re-joins.
///
/// The entry is written here late, and that is worth saying rather than tidying away. The commit
/// that made the change bumped `VERSION` and neither wrote this paragraph nor moved
/// [`MIN_VERSION`], which is how the range came to admit a v19 peer whose certificates this
/// build cannot verify — refused a moment later for a membership reason, so the symptom was a
/// confusing message rather than a fault. Both halves are the same lesson: this list and that
/// constant are the whole of what the number means, and a bump that touches only the number is a
/// bump that has not been made.
///
/// **v21** lets a cancel travel: `Cancel`, and the two answers. The last operator command that
/// acted on the machine it was typed at rather than the machine the run is on — it read the local
/// process table, so a run on the desktop was reported "not running" while it spent money, and
/// nothing was forwarded. New messages an older node would not understand, which is the bump
/// being the point rather than a formality.
///
/// **v22** lets a checkpoint request travel: `Checkpoint`, and the two answers. The sibling of
/// v21 and the last operator command that acted only where it was typed — `offload checkpoint`
/// answered "it is not running here, so there is no turn boundary coming", which is true about
/// this machine and silent about the one the run is on. Unlike a cancel it has no record half: a
/// turn boundary is a moment in a process, so the holder is the only node this can mean anything
/// to. New messages an older node would not understand, so the bump is the point.
///
/// **Not bumped for `Gossip::progress`** (a run's turns, cost and denials), which is the rule
/// above being applied rather than recited: a node that ignores it shows a dash where a
/// number would be, and every decision this protocol exists to make — placement, fencing,
/// migration — is taken from fields it still reads. A version bump costs a device falling out
/// of the fleet, and "your `ps` is less informative" does not buy that.
///
/// **Nor for the leg on it** — `RunProgress::by` and `RunProgress::epoch`, which say which node
/// produced a run's numbers and under what fencing token, so that two legs of a run granted twice
/// can be *ranked* rather than merely compared for size (ADR-0005's second amendment). Same rule,
/// and this time the compatibility story is explicit rather than incidental: the body is JSON, so
/// a build that predates the fields drops them and relays the numbers without them, and
/// `RunProgress` reads an unstamped record as exactly that — a record whose leg is unknown, which
/// loses to any stamped one and falls back to the forward-only comparison against another
/// unstamped one. That branch is load-bearing for the **store** rather than for the wire, since
/// schema v7 leaves existing rows unstamped, and it is the same branch either way.
/// **Nor for `Run::origin`** — whether a person or a rule started a run (ADR-0024), which is what
/// lets a node that has never heard of the rule reclaim the record and the checkout an occurrence
/// left on it. Same rule again, and this one is the easiest of the three to check: the field is
/// `#[serde(default)]` and the default is `Operator`, so a build that drops it reads every run as
/// somebody's and *keeps* everything — which is exactly today's behaviour. Failing towards
/// keeping is the whole design of the field, and it is why a mixed-version fleet reclaims
/// unevenly and correctly rather than losing anything.
/// **v23** adds a bid that says *when* rather than how far back in a queue:
/// `Availability::NotBefore`, carried on an `Offer` when the account's rate limit is holding new
/// runs until a stated instant (ADR-0029). Unlike the three fields above it this is a new **enum
/// variant** on the bid exchange, so a v22 node cannot decode the offer at all rather than
/// dropping a number it does not understand — which is the difference between "your `ps` is less
/// informative" and a bid lost in silence, and the reason it earns the bump the fields did not.
/// **Not bumped for deleting `handshake::Refusal::Draining`**, which nothing had ever sent — so
/// there is no message an older build could have produced that this one now fails to decode, which
/// is the only thing a bump buys. Recorded here rather than left to inference, because "the wire
/// changed and the number did not" is exactly the drift the v20 note above is about: a removal
/// nothing could observe is still a removal somebody will come looking for.
/// **Not bumped for `RunProgress::legs`** (ADR-0067) — each leg's own spend. `#[serde(default)]`
/// on a JSON body like `tokens`, and the merge is a union, so an older node relaying a record
/// without it cannot erase what a newer one knows: the question that decides a bump.
///
/// **Not bumped for `RunProgress::tokens`** — what a conversation has consumed, in tokens
/// (ADR-0040). The third field of exactly the shape the two notes above describe: `#[serde(default)]`
/// on a JSON body, so a build that predates it drops it and relays the rest, and the default is
/// four zeros, which is already what a run reads as before its holder has captured a checkpoint.
/// The one thing worth checking rather than assuming is whether a relay could *erase* it, since
/// unlike `by` this field carries a quantity somebody reads: it cannot, because it moves only
/// inside `RunProgress::absorb`'s position branch, and a relay repeating what it heard has the
/// same position and therefore does not supersede. A record that *does* supersede came from a leg
/// that had the field.
/// **v24** puts `let_go_by` inside `RunState::Pending` — which node released a run, when a node
/// did rather than a person parking it (ADR-0042). By the letter of the three notes above this is
/// the "optional field an older node ignores" case: `#[serde(default)]` on a JSON body, and the
/// default is `None`, which is the conservative reading and exactly today's behaviour.
///
/// It is a bump anyway, and for the reason `origin` was not. That field is immutable and
/// identical in every copy, so a build that drops it cannot contradict anybody; this one is
/// *mutable within a state a peer also gossips*. A v23 node decoding the release, relaying it,
/// and winning an equal-epoch tiebreak re-states the run as `Pending` with no `let_go_by` — and
/// what that erases is the only thing that makes the run placeable. The result is a run stopped
/// in front of a fleet that would continue it, which is the precise failure ADR-0042 exists to
/// end, reached by a node that meant nothing by it and said nothing about it. Failing towards
/// keeping is not available here: the safe-sounding default *is* the stranded case.
/// **v25** puts the signed `Revocation` inside `handshake::Refusal::Revoked`, beside the short
/// name that was all it carried (ADR-0044). A **required** field on a message a v24 node still
/// sends without it, so a v24 refusal does not decode here at all — which is the whole reason for
/// the bump, and the shape it has to have: the point of the field is that the refused node acts
/// on it, and a refusal that decodes with the proof missing is one it would have to act on
/// without. That is the failure the field exists to prevent, arrived at politely. The refusal is
/// also the one message a node receives *after* being thrown out, so there is no later exchange
/// to carry the proof instead.
/// **v26** makes `Capabilities::metered_network` a three-valued `Metered` rather than a `bool`
/// (ADR-0045). `Capabilities` is gossiped inside `NodeView` and owned by the node it describes
/// (ADR-0005), so the field's type is on the wire. A v25 node sends `false` — and `false` there
/// never meant "this link is free", it meant the probe set a constant and nothing could set it
/// otherwise. Accepting it would carry that claim across the version boundary and hand this build
/// a fact it would then act on: `--require unmetered` would match the old node, and its metered
/// link would score and refuse like a free one. So it does not decode, which is the same argument
/// v25 makes about a refusal missing its proof.
/// **v27** splits `RunSpec` into shared fields and a tagged `Work` enum (ADR-0019 §2). A spec
/// crosses the wire inside a grant and inside a gossiped `Run`, so its shape is on the wire. A
/// v26 node writes the flat form, and this build *can* read that — `RunSpec`'s `Deserialize`
/// accepts both, because a node's own stored rows are written by the version before it. The
/// version still moves, and the range still refuses v26, for the direction that has no answer:
/// a v26 node cannot read what **this** build writes, so a mixed pair would mesh, exchange
/// grants, and fail at the older end with a decode error instead of at the handshake with a
/// reason. Refusing early is the same argument v25 and v26 make.
/// **v28** gives the owner's three standing doors a *light* form: `WorkPolicy::light`
/// (ADR-0019 §4), so that "take the light watcher always, heavy work only while charging" is a
/// thing a config can say. A `WorkPolicy` is gossiped inside a `NodeView`, and a `NodeView` is
/// **relayed** — a peer sends what it has heard about a third node — so this is the v24 case
/// exactly: a v27 node that decodes the field away, relays the policy without it, and wins the
/// merge leaves the fleet reading the *ordinary* doors for light work. In the relaxing
/// direction that costs a bid round; in the restricting direction it counts a node as willing
/// to do work its owner has refused, which is the estimate `review_commitment`'s tolerance was
/// never written for. The safe-sounding default is the wrong answer in one of the two
/// directions, which is the test that decides a bump.
/// **v29** carries **schedules** (ADR-0019 §3, ADR-0056): `Gossip::schedules`, a list of
/// standing instructions on a clock. New in kind rather than in degree — it is the first gossiped
/// fact that is neither a node nor a run — and the bump is what the *relaying* makes necessary. A
/// schedule survives its home node going away only because peers pass on what they have learned,
/// and a v28 node decodes the field away: it would fire nothing, relay nothing, and — the half
/// that is not merely a missing feature — **drop tombstones**, so a schedule its owner had
/// removed would be re-learned by every node that still had it and fire for ever. Same test as
/// v24 and v28: could an older node relay something that erases this, and is the default the safe
/// answer? It is not.
/// **v30** is phase 10's one bump (ADR-0063, ADR-0064): `RunSpec::prefer`, `RunSpec::hold_until`
/// and `Constraint::Node`. A spec travels inside a grant and a gossiped `Run`, and the variant is
/// the decisive half: a v29 node cannot decode `{"node": …}` at all, so it could not read a run
/// carrying one — and the two fields fail the v24 test on their own, since a v29 node relaying a
/// held run without `hold_until` re-states it as merely *preferring* the desktop, and the laptop
/// takes it at the next round. Stored rows decode with `prefer = Always`, `hold_until = None`,
/// which is what every run written before this meant.
/// **v31** lets a continuation be typed anywhere (ADR-0064's residual): `ContinueBase`, and the
/// two answers. `offload continue` refused on any node but the one that ran the parent's last
/// leg — true, and a sentence sending somebody to another machine for a request this one could
/// forward. New messages an older node would not understand, so the bump is the point, as v21
/// and v22 were for `Cancel` and `Checkpoint`.
/// **v32** is ADR-0069's certificates: `Grant::Renew` at the door, `approved_at`, and the v2
/// signing bytes every new enrolment and renewal is issued with. A v31 node cannot decode
/// `"renew"` in a grant set and would check a v2 certificate against v1 bytes and call it a bad
/// signature — about a valid member, in the one message that has to be right. Certificates
/// already issued keep their v1 bytes and verify on both sides of the bump; what needs every node
/// on v32 is the first enrolment or renewal after it.
/// **v33** is ADR-0069 §4's hardware approval key: a `Delegation` may name a P-256 key, signed under
/// v2 delegation bytes, and a certificate issued under it carries a P-256 signature. A v32 node
/// would verify such a delegation against v1 bytes and call a valid approver's papers forged, so
/// it is refused by version instead, with the sentence that says to upgrade. Node-key delegations
/// keep their v1 bytes on both sides.
/// **v34** is ADR-0075's workspace viewer: `FetchFiles` and its two answers, new messages a v33
/// node would not understand, so the bump is the point, as for v31.
/// **v35** is ADR-0076's gossiped addresses, `NodeView::addresses`: a field an older node would
/// drop when it relays a record, and a record at the same incarnation never replaces the one held,
/// so an address erased in transit would stay erased (the rule in `gossip-and-merge`).
/// **v36** is ADR-0078's `NodeView::quiet`, a field an older node would drop in relaying, for v35's
/// reason. (ADR-0079 briefly added `NodeView::wake` under the same number and removed it the same
/// session; no node ever set it, and serde ignores it from a build that still sends it.)
/// **v37** is ADR-0080: `AgentDetails::models` became the agent's own list with names, a changed
/// type an older node could not read, and `Gossip::models_asked`, the fleet's request to read it
/// again, which an older node would drop in relaying.
pub const VERSION: Version = Version(37);

/// The oldest version this build still understands.
///
/// Equal to [`VERSION`] here, which means older nodes are refused rather than accommodated.
/// That is a decision about which devices fall out of the fleet, and it is easy at the moment
/// because no older node exists outside this repository's history. Widening the range later,
/// when phones are running an older build, is a much more serious edit — and should be made by
/// changing this line deliberately rather than by letting `VERSION` drift away from it.
///
/// Which is what happened at v20: only `VERSION` moved, and the range silently started admitting
/// a v19 peer this build has no compatibility story for. There is a test below that fails if the
/// two part company without an accompanying decision, because "deliberate" has to be checkable
/// by something other than whoever reads the diff.
pub const MIN_VERSION: Version = Version(37);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct Version(pub u16);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// What a node supports, as a range rather than a number.
///
/// A single "my version" field forces every node to upgrade at once, which is precisely what
/// a fleet of personal devices will never do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    pub min: Version,
    pub max: Version,
}

impl VersionRange {
    #[must_use]
    pub const fn ours() -> VersionRange {
        VersionRange {
            min: MIN_VERSION,
            max: VERSION,
        }
    }

    #[must_use]
    pub const fn exactly(version: Version) -> VersionRange {
        VersionRange {
            min: version,
            max: version,
        }
    }

    /// The highest version both sides understand.
    ///
    /// Fails rather than guessing: a connection that proceeds under a version one side does
    /// not implement produces a parse error somewhere far from here, and a node that looks
    /// broken rather than old.
    #[must_use]
    pub fn negotiate(self, other: VersionRange) -> Option<Version> {
        let max = self.max.min(other.max);
        let min = self.min.max(other.min);
        (min <= max).then_some(max)
    }

    #[must_use]
    pub fn supports(self, version: Version) -> bool {
        self.min <= version && version <= self.max
    }
}

impl std::fmt::Display for VersionRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.min == self.max {
            write!(f, "{}", self.min)
        } else {
            write!(f, "{}–{}", self.min, self.max)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_ranges_settle_on_the_highest_common_version() {
        let old = VersionRange {
            min: Version(1),
            max: Version(2),
        };
        let new = VersionRange {
            min: Version(2),
            max: Version(4),
        };
        assert_eq!(old.negotiate(new), Some(Version(2)));
        assert_eq!(new.negotiate(old), Some(Version(2)), "and symmetrically");
    }

    #[test]
    fn a_gap_between_ranges_refuses_rather_than_picking_something() {
        // The failure this avoids: proceeding under a version one side does not implement,
        // and surfacing it as a parse error somewhere unrelated.
        let ancient = VersionRange::exactly(Version(1));
        let modern = VersionRange {
            min: Version(3),
            max: Version(5),
        };
        assert_eq!(ancient.negotiate(modern), None);
        assert_eq!(modern.negotiate(ancient), None);
    }

    #[test]
    fn a_build_is_always_compatible_with_itself() {
        assert_eq!(
            VersionRange::ours().negotiate(VersionRange::ours()),
            Some(VERSION)
        );
        assert!(VersionRange::ours().supports(VERSION));
        assert!(MIN_VERSION <= VERSION, "the range cannot be empty");
    }

    #[test]
    fn the_range_admits_only_versions_this_build_has_a_story_for() {
        // `MIN_VERSION`'s doc comment says the two are equal and that widening the range is a
        // deliberate edit. At v20 only `VERSION` moved, so the range quietly started admitting a
        // v19 peer whose certificates this build cannot even verify — the sort of drift that is
        // invisible in a diff and shows up as a peer refused for a reason unrelated to its age.
        //
        // A widening is legitimate the day a phone is running an older build. When that day
        // comes, this test is the place the reasoning goes: name the versions the range now
        // accepts and what this build does about each, then change the assertion.
        assert_eq!(
            MIN_VERSION, VERSION,
            "no older version is accommodated yet, so admitting one is a bug rather than a policy"
        );
    }
}
