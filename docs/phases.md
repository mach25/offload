# What each phase shipped

The roadmap's completed detail, verbatim: every item a phase delivered, the demo that closed it,
and what was learned building it. `docs/ROADMAP.md` is the live view — what is *not* built, and
the open questions. Read this when you need to know what a phase actually delivered and why.

## Phase 0 — Foundations ✅ done

Domain model, workspace, no networking, no agents. All of it pure and unit-tested.

- [x] Cargo workspace, toolchain pin, lints, docs, ADRs
- [x] `NodeId` / `RunId` / `BlobHash`, `Millis` time type, version comparison
- [x] `Capabilities` incl. agents/toolchains + local probe (`offload-probe`)
- [x] `WorkPolicy` — owner permission, separate from capability, with per-class defaults
- [x] `Constraint` tree with `matches` and `explain`
- [x] Run state machine: epoch fencing, leases, `Orphaned`, reclaim
- [x] `ClusterView` with ownership-based merge
- [x] Bidding: self-evaluation, scoring, score-proportional delay, deterministic winner
- [x] Drop-off hold-down policy with per-node absence history
- [x] CLI: `probe`, `policy`, `match` with a compact constraint syntax
- [x] Property tests: no run lost across transitions, epochs monotonic under churn
      (`offload-core/tests/properties.rs`, proptest). Ten properties, and they found three
      things: a pinned run could be put back in a pool that cannot place it (`Pending` has
      three entrances and one exit that refuses), a failing clause went unexplained depending
      on where it sat in the list, and — the one that matters — **an epoch orders the grants of
      one arbiter and said nothing about the grants of two**, so two agents could run on one
      repository with neither ever told. See ADR-0002's 2026-08-21 amendment.

**Demo:** `offload probe` prints this device's capabilities — including which agents are
installed and authenticated. `offload policy` says whether it would take work right now and
why not. `offload match "agent=claude-code, cores>=8"` says whether it qualifies, naming the
clause that failed.

---

## Phase 1 — Run one agent, locally, well ✅ done

`offload-agent`, `offload-workspace`. Still one machine — no cluster yet.

- [x] `Agent` trait: spawn, stream events, resume from session
- [x] Claude Code adapter: `stream-json` event parsing, `--resume`/`--fork-session`,
      permission mode, model selection — verified against `claude 2.1.220`, not inferred
- [x] `TurnTracker`: turn-boundary detection, the testable form of ADR-0004
- [x] Transcript location, with a scan fallback so a changed slug rule degrades to slow
      rather than to silent data loss
- [x] Run supervision: process groups, SIGTERM→SIGKILL cancel, bounded event channel
- [x] Worktree management: shared per-repo bare mirror + per-run worktree and branch,
      dirty/untracked/commits-ahead status, idempotent teardown that preserves commits
- [x] `offloadd` daemon: TOML config with working defaults, persisted node identity,
      unix-socket control API, SIGTERM/SIGINT shutdown
- [x] `offload run` / `ps` / `logs -f` / `cancel` / `rm` / `status`
- [x] ADR-0008: `AcceptEdits` default, `Ask` refused at submit, `Full` opt-in per run
- [x] Per-tool allowlists (ADR-0008 amendment): node config + repo `.offload.toml` +
      `--allow`, with repo-supplied grants capped to scoped commands. A run can write code
      and execute its own tests without `Full`.

**Demo:** `offload run --repo ~/dev/foo --prompt "add tests for the parser"` starts a Claude
Code run in a worktree. `offload logs -f` streams turns live. `offload cancel` stops it
cleanly with no orphaned processes.

**Demo, verified end to end:** `offloadd` starts, `offload status` reports the device and
its agent, `offload run --follow` prepares a worktree, runs Claude Code in it, streams turns
live and reports cost; `offload ps` lists it; `offload rm` discards the worktree and keeps
the branch. `cargo run -p offload-agent --example smoke` exercises the adapter alone.

**Carried into phase 2 — known gaps, not surprises:**

- ~~The run registry is in memory.~~ Closed: SQLite, ADR-0009.
- ~~Absence history is in memory too.~~ Closed: the `node_observations` table.
- ~~**`max_turns` is unenforced.**~~ Closed in session thirty-one, ADR-0039 — seven phases
  later, which is the point of the entry. The agent still has no such option (re-measured on
  2.1.251), so the supervisor counts boundaries and stops the run itself; `--max-turns` is on
  `offload run` and `offload when`, and the limit is also checked by `decide_recovery` and by
  `resume`, because a cap enforced only where it is reached is one the auto-resume undoes.
- ~~Runs that need to run their own tests need `--permission full`.~~ Closed by the
  allowlist. Anything the allowlist does not name is still denied, which is the point.
- **Shutdown cancels rather than drains.** There is nowhere to hand a run to yet. Phase 4
  replaces this with a checkpoint at the next turn boundary.

**Learned while building, and now load-bearing:**

- The transcript path is derived from the **cwd**, so migration must rewrite it to match
  the receiving node's worktree. Same session id, different location. Worktrees live under
  the node's state dir, which differs per machine — so this relocation is the normal case,
  not an edge case.
- A `--mirror` repo cache would delete every run branch on its next `fetch --prune`. The
  cache is a bare clone with upstream refs under `refs/remotes/origin/*` instead.
- A repo named by local path can only be placed on nodes holding that path
  (`Portability::NodeLocal`). That is an eligibility fact and needs to reach the
  constraint system before placement, not at resume time.
- `--session-id` lets us assign the session, so the transcript is findable by a key we
  chose rather than one scraped out of a stream.
- There is no `--max-turns` in 2.1.220, and still none in 2.1.251. `RunSpec::max_turns` has to
  be enforced by the supervisor counting boundaries, not by a flag — which is what ADR-0039
  finally did.
- `rate_limit_event` gives real per-account rate-limit state — the signal the probe's
  per-machine fingerprint cannot provide (open question #4).
- Headless `--print` has nobody to answer a permission prompt, so `PermissionMode::Ask`
  records denials rather than blocking. Open question #1 is now blocking phase 1's
  usefulness, not just phase 5.

---

## Phase 2 — Checkpoint and resume on one machine ✅ done

Migration mechanics, before any network complicates them.

- [x] SQLite state store (ADR-0009): schema + migrations, run registry, event log,
      blob index, absence history — all inspectable with plain `sqlite3`
- [x] Content-addressed blob store (BLAKE3): dedup, verify-on-read, sharded layout,
      conservative GC that spares anything a run references
- [x] Daemon on durable state: runs, stats and logs survive a restart; interrupted runs
      are recovered as `failed` with a reason rather than left claiming to be running
- [x] Checkpoint capture at turn boundary: transcript + git bundle + dirty patch + metadata
- [x] Untracked-file policy — what moves, what doesn't (the fiddly part; see ADR-0003).
      Phase 1's demo made this concrete: one agent run that executed `cargo test` left
      **50 untracked files** in `target/`. Bundling those would dwarf the work they
      surround, and the repo had no `.gitignore` to lean on. Limits are node config now.
- [x] Restore: materialise worktree from blobs, resume agent from session id
- [x] `offload checkpoint <run>` / `offload resume <run>`
- [x] Cadence decided: **one checkpoint per turn boundary**, `[checkpoint] every_turns` to
      override. The reasoning is in `CheckpointConfig` — a turn is minutes of model time
      and real money, and content addressing makes the repeated blobs nearly free.

**Demo:** start a run, let it complete three turns, `offload checkpoint`, kill `offloadd`,
restart it, `offload resume`. The agent continues turn four with full context and the
uncommitted edits from turn three still in the tree.

**Demo, verified end to end** (both halves, on a real agent):

1. *Cooperative.* `offload checkpoint <run>` mid-run. The agent finishes its turn, the
   checkpoint is captured and the run is released to `Pending`; `offloadd` is killed and
   restarted; `offload ps` still shows it pending, not failed; `offload resume` starts it
   again in the same session.
2. *Ungraceful.* `kill -9` the daemon four turns into a run. On restart `ps` reports
   `failed — daemon restarted while this run was active — resumable from turn 4 with
   offload resume <id>`. `offload resume --follow` adopts the surviving worktree, reuses
   the transcript, and the agent continues **at turn 5 in the same conversation** — it
   verified its own four uncommitted files rather than rewriting them.

**Learned while building, and now load-bearing:**

- **A resumed agent counts turns from one**, because it is a new process. Left alone, a
  checkpoint taken after a resume claims fewer turns than the one it replaces, and `ps`
  shows a run losing progress. Turn numbers are offset once, on the way in.
- **A released checkpoint ends a holder's output without ending the run**, so it has to
  count as terminal for `logs --follow` but not for the state machine. And "did the
  backlog contain a terminal event" is the wrong question for a resumed run — only the
  *last* event can end a stream.
- **`Failed` needed a way back.** A daemon that died mid-run leaves exactly the state that
  is most worth resuming, so `Run::reopen` moves `Failed` → `Pending` at a fresh epoch.
  `Completed` and `Cancelled` stay closed: those are decisions, not accidents.
- **The base commit belongs in the `Checkpoint`.** A bundle and a patch have nowhere to
  land without it, and it cannot be re-derived from either — nor from the worktree, whose
  branch has moved on.
- **A worktree that survived beats a worktree rebuilt from blobs** — while the run never left
  this node, which is the qualifier session sixteen had to add: a checkout from an earlier leg
  is *older* than the checkpoint, and it is still here because nothing removes one when a run
  leaves. So adoption compares the turn the checkout holds against the checkpoint's, and
  restore falls back to the run branch in the mirror, unpacking the bundle only when neither
  exists.

---

## Phase 3 — Two nodes see each other

`offload-transport`, `offload-cluster`.

- [x] `NodeId` is a real ed25519 public key, and membership is real cryptography (ADR-0012):
      `offload_core::fleet` has certificates, delegations and revocations with canonical
      signing bytes, chain verification, and expiry against an injected `now`. The node holds
      a keypair in an owner-only `node-key`.
- [x] The fleet key derives from a generated passphrase (argon2id over a diceware phrase,
      ADR-0012), and enrolment works with no mesh at all, because a certificate verifies
      against the fleet public key alone: `offload init` founds a fleet and prints the phrase
      once, `offload join --passphrase` enrols a device, and `offload grant`, `offload
      revoke`, `offload verify` and `offload fleet` do the rest. The cheap mitigations ship
      with it — joining grants `{Submit, Deliver}`, and `HostRuns` waits out probation.
- [x] `Transport` trait addressed by `NodeId`, with an in-memory implementation for partition
      and churn tests and QUIC via `quinn` for real nodes (ADR-0015). TLS 1.3 with RFC 7250
      raw public keys, so the peer's credential *is* its node key; nothing above the transport
      learns an address, and dialling pins the key it meant to reach.
- [x] Membership certificates verified on connect, and bound to the TLS credential, so an
      unenrolled node is refused at the handshake rather than filtered later — and a copied
      certificate is not a bearer token (ADR-0015). There is no way to obtain a connection
      without having been admitted.
      The *approver-mediated* half of enrolment (`offload join` without the passphrase,
      `offload invite`) stays in phase 5: it needs a peer to ask, which is the one thing local
      enrolment does not.
- [x] `offload-proto` past the handshake: gossip, assignments, blob fetch — each a message type
      with an owner in ADR-0005's table before it is a struct. Built across phases 3 and 4 and
      left unticked here: the whole view rides on the probe rather than travelling as deltas
      (ADR-0005's digest-and-fetch, which the LAN has not yet made worth doing), and blob
      transfer is ADR-0016's six messages. The wire is at v15.
- [x] SWIM membership: direct + indirect probes, `Alive`/`Suspect`/`Dead`/`Draining`, with
      suspicion refutable by incarnation and a departure believed immediately. The detector is
      pure and clock-injected; `Cluster::probe_round` takes the time, so partitions are tested
      rather than provoked.
- [x] Capability gossip, piggybacked on every probe and answer. Whole `NodeView`s rather than
      a digest: a fleet is tens of nodes and the merge is by ownership, so a redundant copy
      costs a comparison. Digest-and-fetch is a protocol version away when it is worth it.
- [x] Static seed list — addresses, not keys, because a human writes it. The key is learned
      from the handshake and membership decides whether the answer counts.
- [x] mDNS discovery, so a LAN needs no seed list at all. The advertisement carries the node
      id and *nothing about the fleet*: any deterministic function of the fleet key is a
      verifier for an offline passphrase search, so multicasting one would turn discovery into
      a membership oracle for everybody on the wifi.
- [x] Run gossip: what a node is holding travels with every probe, and its capabilities are
      re-probed and re-gossiped as they change. Both bump the incarnation only when something
      actually changed, since that is the version peers merge on.
- [x] Peer blob transfer — fetch by asking, push to replicate, integrity from the hash
      (ADR-0016). Availability is deliberately *not* gossiped: the set changes every turn, an
      advertisement is stale by construction, and the ask has to exist regardless.
- [x] `offload nodes`

**Demo:** three `offloadd` on the LAN. `offload nodes` lists all three with their agents,
versions and auth status. Kill one — the others mark it `Suspect` then `Dead` within 10s.

**Demo, verified end to end** (two daemons on one machine, separate state dirs, real QUIC):
`offload init` on one and `offload join --passphrase` on the other; both start with **no
seed list at all** and find each other over mDNS; each `offload nodes` lists both with cores,
memory and agent version, marking itself. `kill -9` one: the other reports `suspect` within a
second and `dead` at six. Restart it: `alive ~1`, the absence on its record where the
hold-down policy will read it. `SIGTERM` instead: `draining` immediately, with no detection
timeout at all. Submit a real agent run on one node and the *other* node's `offload nodes`
shows it holding a run, and shows it finish.

---

## Phase 4 — Runs move between machines

The headline. Planned as `offload-sched`, and built instead in `offload-cluster::place`: the bid
round needs the view, the connections and the serve loop the cluster already has. A separate crate
is worth creating when migration *policy* grows past what `offload-core` already decides.

Three things that belonged to phase 4 landed early, because each was cheaper before the
scheduler existed than it would have been after:

- [x] `Capabilities` reshaped into instances with roles (ADR-0011), so a delivery route is a
      capability rather than a tag and two mailboxes are two entries. Protocol version 2.
- [x] `Portability::NodeLocal` reaches eligibility: `LocalFacts::repo_available` refuses the
      bid, so a run whose repo is a path on the closed laptop is refused at placement rather
      than at 03:00. `WorkspaceManager::can_obtain` is what answers it.
- [x] `HostRuns` has teeth: a node the fleet has not granted it refuses submissions and says
      so in `offload status`, read fresh from `fleet.json` so a grant needs no restart.
- [x] …and peers enforce it at the bid exchange (ADR-0012's amendment): a bid from a node
      whose certificate does not grant `HostRuns` is refused with the reason, checked against
      the certificate at that moment — so probation lifts on a live connection — and the
      objection drops the connection, so a grant issued mid-connection is believed one
      re-handshake later rather than never.

- [x] One bounded bid round per submission: the arbiter asks every available peer, each node
      decides locally, and `winner()` picks. Unicast rather than broadcast, so the
      score-proportional delay is unnecessary — see ADR-0006's 2026-07-27 amendment.
- [x] Grant confirmation (ADR-0006 step 6): the granted node accepts or declines, silence is
      a decline, a declined grant falls to the next bid rather than stranding the run, and a
      decliner is not reconsidered in that round.
- [x] Lease expiry and hold-down (ADR-0007): a holder that goes quiet makes its runs
      `Orphaned`, a returning holder reclaims at the same epoch for free, and the hold-down
      decides when that becomes a reassignment. Every node runs the loop; only the arbiter
      acts. Merge asks *who is speaking* — only the holder may claim it is still there, only
      the arbiter may say it has gone.
- [x] Fencing that acts: learning a run has moved to a higher epoch elsewhere stops the agent
      here.
- [x] Per-run arbiter *failover* when the home node itself dies: arbitration moves to the
      lowest-id available node, every node computes the same successor, and the decision that
      follows is `offload_core::supervise` — a pure function of (view, run, now), so the
      cases can be arranged rather than provoked. A suspected home node keeps arbitrating,
      because failing over on a refutable guess is how one run gets granted twice.
- [x] Run state gossip: live runs and recently finished ones ride along with every probe, so
      the node that submitted a run sees it start and finish. A node's own store stays the
      truth for runs it holds — a peer's older copy cannot roll one back.
- [x] Run *stats* travel too: turns, cost, denials and the worktree summary ride beside the
      records as `RunProgress`, owned by whoever is running the turns and arbitrated
      **forward only** — a record claiming less than what is known is a node that saw part of
      the story. Ties go to the author's own timestamp, which travels with the record, because
      a finished run has no holder to defer to and its last word is the summary of the
      worktree it leaves behind. Starting a run no longer resets its numbers, and cost and
      denials accumulate across legs the way turns already did.
- [x] **Accept without starting** (ADR-0006): a full node bids anyway with `Availability`
      (wire v3), takes the grant into `Assigned`, heartbeats its lease, and starts the agent
      when a slot frees — instead of declining and leaving nobody committed. `offload run`
      says which: "starting when the run ahead of it finishes". A node that is *draining* hands
      an unstarted commitment straight back (`Run::release`), because a promise is worth as
      much as the node that made it. Verified on real daemons: one slot, two submissions, the
      second accepted-and-held and then started by itself 23 seconds later.

      And now the other half, which needed a deadline to exist first: **a node gives back a
      run whose start it can no longer make in time** (`review_commitment`), so optimism cannot
      look like capacity. It offers before it lets go — a refused round means this node takes
      its own commitment back rather than leaving the run in nobody's hands — and only a
      *stated* deadline moves it, because with an unspecified one every held run is overdue
      within a second and the rule would bounce every commitment in the fleet from queue to
      queue. A held run still says how many are ahead of it and nothing about when: a count is
      what a node actually knows, and an invented ETA is the number everybody would quote back.
- [x] **Checkpoints durable off the holding node** (ADR-0016): capture replicates to a peer
      that could plausibly take the run over, the run records where its copies are, and
      `offload ps` says `here only` when there are none. Verified against two daemons and a
      real agent run — and then found to have been refusing every push on a plugged-in laptop
      whose battery reads 0%, because the replica check had its own copy of the battery floor
      and forgot that it does not apply on mains. Durability that degrades silently is worth
      one loud test each; there is now one.
- [x] **Submissions durable off the submitting node** before `offload run` reports success,
      because the next thing that user does is close the laptop. A run placed on a peer needs
      nothing — the peer has it — so this is about the run a node keeps: it waits for somebody
      else to write the record down before reporting acceptance, and says "no other node has a
      copy" when there is nobody. No new message; a probe carries the whole view and a peer
      persists what it learns *before* it answers, so the `Ack` is the receipt.
- [x] **Accepted, or refused to your face** (ADR-0014). `offload run` runs one bounded bid
      round and returns either the node that committed — "accepted by bravo, starting when one
      of its 2 runs finishes" — or the per-node reasons nobody would. `--queue` opts into
      pending-anyway, and *is offered again* by the run's arbiter until somebody takes it:
      "later" is not a state a run reaches on its own. A run a human parked with `offload
      checkpoint` is not picked up on their behalf — the checkpoint is what tells the two
      apart. Nothing times out. The remaining half is the reporting one: an accepted run that
      becomes unplaceable should be *reported* over the delivery plane, which is phase 5.
- [x] Run registry persisted (SQLite, ADR-0009), including runs this node placed elsewhere —
      which is also why "how loaded am I" has to mean *held* rather than *known*.
- [x] Lease grant / renew / expire; epoch enforcement on **every** effect path. One duration
      (`offload_core::LEASE`) whatever way a run arrives, and a heartbeat that runs whether or
      not there is a fleet to answer to — a lease nobody renews is a countdown.
- [x] `Orphaned` handling: reclaim path, hold-down policy, absence-history tracking, with
      deadline slack as an input (ADR-0013) — and **attendance deliberately not** one, which is
      an amendment to that ADR rather than an omission: attendance is observable only where the
      stream is served, the hold-down is decided by the arbiter about a holder that has
      vanished, and the one node that could answer is the one that is gone. Gossiping it would
      hand the arbiter a value from before the silence and call it current. Attendance decides
      the failure case its observer is present for — a run whose agent stopped — where slack
      sets the retry ceiling and attendance decides whether to spend it at all.
- [x] **Follow a run wherever it is**: `offload logs -f` and `offload run --follow` work for a
      run held by a peer, by asking the holder for everything after the last sequence seen, once
      a second (`FetchEvents`/`Events`, wire v7). A poll rather than a stream held open — it
      cannot miss an event and needs nothing kept alive across a migration — and the reply says
      whether that holder has more *to come*, separately from whether the page filled, because
      an empty page means "nothing new yet". `LogEvent` moved to `offload-core` for it: the run's
      output has to cross the wire typed, and `offload-proto` cannot depend on `offload-node`.
      Being asked is also how a holder learns somebody is watching, which is what makes
      attendance mean anything for a run that moved. Sequence numbers are per node, so a
      migrated run's log is a concatenation by leg and not a merge by clock.
- [x] Per-node and **per-account** concurrency caps. The per-node half mostly existed and was
      enforced against nothing: every test built capabilities with no agent in them, so the
      per-agent branch was unreachable while looking covered, and `offload status` fed it
      `held.runs` — every agent conflated — so it disagreed with the number `bid::evaluate`
      computes from the view. The per-account half needed a fingerprint that two machines on one
      login actually agree on (see below); it is `WorkPolicy::max_concurrent_account` (wire v13),
      folded to the **lowest** ceiling any node on the account claims, gating *starting* as well
      as accepting because a cap that only gates accepting is decorative — and best-effort by
      construction, which ADR-0013's amendment argues is tolerable here and nowhere near
      placement.
- [x] Capacity as a budget, not a count (ADR-0013): `Demand` on the run (wire v5), a share
      budget beside the count that stays a hard ceiling, and observed load as the override —
      `offload-probe` measures the one-minute load average per core, `None` where the platform
      will not say, because a plausible zero wins bids the machine should lose. Two rules that
      the writing found: a **lone run always fits**, or a budget its owner picked for
      concurrency has quietly become an eligibility rule and a heavy run is unplaceable
      fleet-wide; and **pressure gates accepting, never starting**, because a committed run has
      nowhere else to be and the way out of a commitment is to release it. `NoBid::Busy
      { retry_after }` is for pressure only: being *full* is this node's own queue, which it
      knows the depth of and which empties by itself, so a full node still commits (ADR-0006) —
      being under pressure is the owner's build on the same machine, which this node never
      promised and cannot drain. Judged against the run's slack, so an overdue run is told to
      look elsewhere now rather than wait. Never preemption: a busy node refuses, defers, or
      finishes.

      Not built, deliberately: **re-bidding pushed by the node that freed capacity.** A freed
      node is not the arbiter of anything, so it cannot grant itself work; the arbiter's own
      pass re-offers a pending run within its 30-second backoff, which is what makes "later"
      arrive today. A push would buy up to thirty seconds and cost the thundering herd
      ADR-0013's consequences section warns about — worth having once there is a demo it
      visibly helps.
- [x] **Deadline** (ADR-0013), and urgency derived from it rather than stored: `f(deadline,
      now)`, so nothing is gossiped, no node disagrees, and a waiting run gets more urgent with
      nobody bumping anything. An unspecified deadline means the moment of submission — *as soon
      as you can* — which makes aging free and starvation impossible without an anti-starvation
      mechanism, and which is also the trap: derived urgency orders runs and must **not** cut
      the hold-down, or every run in the fleet moves fifteen seconds after a Wi-Fi handover.
      Only a stated deadline shortens patience, never below `min_grace`, and the reassignment
      says which of the two numbers decided. `offload run --deadline 2h`, `offload deadline
      <run> 45m|none` forwarded to the field's owner and arbitrated by `deadline_rev` (wire v4),
      `ps` and `explain` reporting slack. Ordering exists in exactly one place — the runs a
      single node holds — and it is least-slack-first with priority as the tiebreak.
- [x] Attendance (ADR-0013): **observed**, not declared — is any client streaming this run —
      and it decides autonomy in failure, which is the half of the split deadlines do not
      answer. An unattended run auto-resumes; an attended one escalates to the person sitting
      there. Sampled when the run is written `Failed` rather than when the decision is taken,
      because a failure ends the stream a follower was reading and asking later would find
      nobody watching every time. Held in memory on the node that failed the run and gossiped
      nowhere — a `Failed` run is terminal to `supervise`, so this is the holder's decision and
      the holder is the node with the checkpoint, the worktree and the stream. A daemon that
      restarted knows nothing, and knowing nothing means leaving the run for a person: the
      alternative hands a crash-looping daemon every run on the machine on every start. The
      backoff grows per resume, is floored, and is shortened by a *stated* deadline only.
      `offload explain` says which of three things is true, because a node that cannot see the
      stream must not report "unattended".
- [x] `priority` as a real input, and editable: `offload priority <run> 10|+10|-5`, forwarded to
      the same owner the deadline has and arbitrated by the same counter — `deadline_rev` became
      `spec_rev` and `SetDeadline` became `EditSpec { edit: SpecEdit }` (wire v6), because one
      owner needs one counter and a second copy of that mechanism is a second merge rule to get
      wrong. Absolute rather than a delta: a delta has to be added to a value that may be a
      gossip tick old, so `+10` twice would sometimes mean 20. It now reads in two places — the
      runs a node starts, and the order the arbiter offers the runs it can place, which was
      `RunId` order and therefore *roughly* submission order by accident.
- [x] A run that cannot make its deadline **says so** (ADR-0013's last two rows, wire v8): one
      core function, `Run::prospect_at(at)`, asking where a run stands at the instant a delay
      ends — a rate-limit reset, or `now` for a wait with no end in sight, which is the honest
      case rather than a degenerate one. Two callers: a queued run refused past its deadline,
      and a rate limit that lifts too late, judged the moment the agent reports it rather than
      when the deadline arrives. Only a *stated* deadline is announced (the trap's fourth
      appearance, and the place it would have been loudest — otherwise every ordinary run in
      the fleet announces itself), said once per deadline so a moved deadline earns a fresh
      answer, and written as a typed `LogKind::Overdue` in the run's own log rather than only to
      `tracing`: the log is what ADR-0010's delivery plane will fan out from, and a `WARN` on a
      machine nobody is logged into is not telling anybody. It **changes nothing** about the run
      — still offered, still holding its checkpoint and lease, still `pending` in `ps` — because
      a deadline decides when we give up, never what happens to the work.
- [x] Device-local capacity broker (ADR-0013, roadmap #8): reservations as expiring leases in
      a ledger every `offloadd` on the machine shares, so two fleets cannot over-commit one
      laptop. It carries the number and nothing else — the second fleet learns the device is
      busy, never with what. Its own SQLite database under a per-user runtime path (not
      `state.db`, which is per-fleet), reconciled from the heartbeat because a reservation is a
      lease, and read by the *bid* as well as by admission — a node that consulted it only when
      accepting offered to start now and then held the run on arrival. An unreadable ledger is
      a loud warning and the old over-committing behaviour, never a daemon that will not start.
- [x] Graceful drain: `offload drain` and SIGTERM. Each run is checkpointed at its next turn
      boundary — never mid-turn — then offered to the fleet like a fresh submission; the
      winner fetches whatever blobs it lacks and resumes the same conversation. A run still
      mid-turn at the deadline keeps its turn and stays put. (Lid-close hook: still to do.)
- [x] A node's held runs start **most urgent first**, and a commitment no longer blocks the
      one behind it: capacity gates how many agents *run*, not how many runs are held. Three
      runs granted to a one-slot node used to leave two commitments each reading the other as
      the reason it could not start — leases renewing for ever, `ps` saying `assigned`, nothing
      failing. Two submissions could not show it, which is what the commitment demo had.
- [x] `offload explain <run>` — every node's bid, its reason for declining, or when it says
      to come back, plus who arbitrates the run and what they currently make of it. Two
      halves, because "why is my run still pending" and "why did nobody move it" are the same
      question at the two ends of a run's life: the top is `offload_core::supervise` asked one
      extra time for a human, and the bottom is a fresh round (`Cluster::canvass`) that grants
      nothing. Not a replay of the round that placed the run — a bid describes one second, and
      a stored one shown as current is a confident wrong answer. Works from any node: a run
      known only through gossip explains the same, naming its arbiter rather than pretending
      to be one.

**Demo:** four demos, because this phase has four distinct failure modes.

1. Submit a run needing `agent=claude-code, mem>=16G`. The desktop and laptop both bid; the
   desktop wins. `offload explain` shows both bids and the phone's reason for declining.
2. `offload drain` the desktop mid-run. The agent resumes on the laptop from its last turn
   with uncommitted work intact, and `offload ps` shows the epoch bump.

   **Verified end to end.** A run started on node A — "create eight files, one per step" —
   reached turn 3 and was drained. It resumed on node B *in the same conversation*, with the
   two files A had already written reapplied from the patch, and ran to turn 20 producing all
   eight. What it exposed on the first attempt is in `docs/sessions.md`: a transcript-slug bug
   that made the agent start a fresh conversation while reporting it had resumed one.

   **Re-read in session twenty-three, and this time with a proof.** The two daemons were given
   *separate* `$CLAUDE_CONFIG_DIR`s, so the receiving node could not find the transcript on the
   shared disk and the conversation had to arrive as a blob; and the prompt made the agent choose
   a word in turn 1 and write it into **no file** until the last step. Drained at turn 7 with the
   word only ever spoken, the run wrote it out on the other machine twenty-eight turns later. Four
   findings came with the walk — a drain recording a false fence in the audit log, `offload
   explain` counting a finished run down towards a met deadline, the resume nudge asking a
   migrated agent to continue rather than to finish, and a `ps` header four columns out of line.
3. Suspend the desktop for 10 seconds. The run goes `Orphaned` and then **reclaims** on
   wake — no migration, no epoch bump, no lost turn. Then `kill -9` instead, and confirm it
   moves after the hold-down and loses at most one turn.

   **Verified, both halves.** Three nodes: C submits (it may not host — probation), A takes
   the run. `SIGSTOP` node A: C marks the run `Orphaned` within six seconds, and `SIGCONT`
   puts it straight back to `running` on A — no migration, no epoch bump, and all eleven turns
   on the original node. `kill -9` instead: C holds it down for ~36 seconds and reassigns to
   B, which resumes *from turn 5 in the same conversation*, patch reapplied, and finishes.

   **Re-walked in session twenty-three, and the resume still holds** — `HolderDead` after 15s,
   transcript restored, resumed from turn 3 of the same session, and a word chosen before the kill
   and written into no file came out at the end on the other machine. What it exposed is that the
   killed node's *agent* does not stop: it finished the entire task over the following minute,
   into a worktree the fleet had moved the run out of, unreachable by `offload cancel` and beyond
   fencing's reach. Fixed by `offload-node`'s `leftovers`, which writes the agent's process group
   down and sweeps it on the next start.

   **And the arbiter's own funeral.** `kill -9` the submitter *and* the holder at the same
   instant, so the node that arbitrates the run is one of the ones that died. C submits, A
   runs it, both are killed at turn 5; B — neither home nor holder — concludes both are dead,
   orphans the run ten seconds later, reassigns on `HolderDead { absent: 15.2s }`, restores
   the transcript from the replica it was already holding, and finishes all 25 files from
   turn 8 of the same conversation. Restarting A and C shows the run `completed` on both.
4. **Overnight, and the reason the project exists.** From the laptop at 23:00, submit a run
   due 08:00 while the desktop is mid-build. The desktop *commits* — accepts the grant and
   stays `Assigned` without starting — rather than merely deferring. `offload run` does not
   report success until the run is durable on a node other than the laptop. Shut the laptop
   down. The desktop starts the run when its build finishes, completes it at 02:00, and the
   notification is waiting in the morning. Open the laptop: `offload ps` shows the run
   completed on the desktop, reconciled by the ordinary epoch-ordered merge.

---

## Phase 5 — Off the LAN

- [ ] Relay + hole punching for NAT'd devices. ADR-0015 defers the `iroh`-versus-build-it
      choice to here and writes down what decides it: whether the fleet needs to leave the
      LAN at all, whose relay it would use (no third-party relay by default), whether iroh's
      pre-release `ed25519-dalek` has settled, and how much of it we would otherwise write.
- [x] **The half of enrolment that needs a peer** (ADR-0012). `offload id` on the joining
      device, `offload invite <that id>` on one that already belongs, `offload join --token`
      back on the first — an approver signs under its delegation, so the passphrase stays in the
      drawer, which is the entire reason approvers exist. A grant beyond the door needs the root:
      `offload invite <node> --grant host-runs` prompts for the passphrase and is this ADR's
      "`offload grant` gains a `<node>` argument", by the route that does not need the two
      devices to be on speaking terms.

      The token is **not a bearer credential**, which is what makes it simple: it is a
      certificate naming the joining device's key, useless to anybody who does not hold it, and
      it therefore needs no expiry of its own and can be pasted anywhere. And probation follows
      the *grant* rather than the path — probation only ever suppresses `host-runs`, so "an
      invite skips probation" is a statement about the door, and mitigation 2 has no exemption
      for the passphrase.

      **Also built, and both were claims the code did not honour.** A revocation now travels
      (wire v16) — the one membership fact that has to, because it is the *absence* of a
      signature and no certificate can carry it — and a live daemon now re-reads `fleet.json`,
      so `offload grant` and `offload revoke` reach it without a restart. And certificates are
      **renewed on contact** (wire v17), which this ADR leans on twice and nothing implemented:
      the real behaviour at thirty days was not decay towards safety, it was every device
      dropping out at once with the passphrase as the only way back on each of them.

      **And "immediate and local" took two more sessions to be true of a live daemon.** A hang-up
      walked the map of sessions this node *dialled*, which is the half a revoked device does not
      use — it is the one still dialling in — so it kept an inbound connection and went on
      gossiping for the rest of its run (session forty-two). Then the subject's own half: the
      handler for "this node was revoked" needed a gossip the hang-up had just made impossible,
      while the refusal that does arrive, on every dial, was matched nowhere. `Refusal::Revoked`
      carries the signed `Revocation` now (ADR-0044, wire v25) and the subject verifies it against
      the fleet key it already holds — never the refusal, which is a peer's claim about this node.
      Measured, before and after: 48 turns of agent after eviction, against 2.4 seconds.

      Still open: the *online* `offload join`, where an approver is asked over the network and
      the owner confirms on a device they are holding. The offline artifact makes it optional
      rather than necessary, so it is a convenience now rather than the missing half.
- [x] **"A new device joined the fleet" as a `Notification`** (ADR-0010, ADR-0012 mitigation 4),
      and the same for every use of the passphrase. With no per-join approval this is the
      compensating control the whole posture rests on — probation's fifteen minutes were being
      bought for an alarm that did not exist. It needed the delivery plane to address something
      other than a run, so a notification carries a `Subject` and the store has a second log
      (schema v5, wire v18).

      Two producers: the CLI writes the event, because these commands run with no daemon and an
      alarm needing one would be missing on the machine somebody just walked up to; and every
      *other* node writes one when it meets a member it has no record of, because the machine
      that issued the invitation may hold no route to a person and may be the machine an
      attacker used. That makes it a log of what a node **witnessed** rather than the fleet's
      memory — this ADR's "written durably to every node's store" would be a gossiped fact, and
      a gossiped fact needs an owner (ADR-0005) that observations do not have. `offload nodes
      --history` reads it back and says so.
- [x] **`offload rekey [--evict <node>]`** as convergent revocation, and fleet health in
      `offload status` — members met, approvers met, certificates running out, how long since
      the passphrase was read back, and the nags. Counted from certificates handshakes *proved*
      rather than from gossip, because grants live on a certificate precisely because a node's
      claims about itself are not to be believed; bounded honestly to peers this node has met,
      and the output says so.

      Rekey needed a mechanism the ADR did not name. The first version printed an invitation per
      member and every one was refused, because a device will not let an unfamiliar fleet
      replace the one it belongs to — that is exactly what an attacker would send. So a rekey
      carries a **succession**: the new fleet's identity signed by the key being replaced, which
      every member already holds. Only somebody with the old passphrase can move a device, which
      is the authority that could already do anything to it.
- [ ] Android/iOS host process for `offloadd` — background execution, doze constraints
- [ ] Mobile capability probing: battery, thermal state, metered network, from platform APIs
- [ ] **Approvers in hardware** (ADR-0012): `Grant::Approve` and its delegation exist and do
      real work — they are what issues an invitation and what renews a certificate without the
      passphrase. What is left is the platform half: the approver key in
      secure hardware where there is any — non-extractable, biometric-gated, and probed
      honestly (`hardware_backed` is `false` unless verified). Approval is a signed artifact
      peers check offline, never a live call to the phone. This is the phone's best job in
      the fleet and the reason to enrol one before it can host anything.
- [x] Phone as a **worker**, gated by owner policy rather than by class — the *model* half, which
      is what can be finished without a phone in hand. Swept for anywhere a device's class decides
      eligibility rather than its owner's policy, and the answer is nowhere: `WorkPolicy::for_class`
      is defaults only and every field is overridable, `arbiter_for` and the capacity rules are
      class-blind, and `Stability` (which comes from the class) is a *preference* in `score` and in
      `replica_for` — which falls back to any candidate rather than excluding one, exactly as
      CLAUDE.md claims.

      Two holes, both in the direction of the owner not being heard:

      **`offload policy` answered about the wrong policy.** The command whose whole job is "would
      this device take work right now?" computed the class default and reported that, so an owner
      who had written `[policy]` was told about a policy their daemon does not use — `accept =
      "always"` on a phone reported as charging-only, a battery floor of 80 reported as 40. It
      takes `--config` now, through `Config::work_policy` rather than a second copy of the
      layering, and it names which policy it is answering about.

      **`WorkPolicy::allowed_agents` was enforced and unsettable.** Checked by `admits`, gossiped
      in the struct, and set by no config field and no flag — so `Refusal::AgentNotAllowed` was a
      sentence no fleet could produce. It is the one owner decision a class cannot express ("this
      machine is for light work, not an expensive agent session"), which makes it the field this
      item is *about*. Settable now, validated at deserialize time so `work_policy` stays
      infallible for the gossip tick, with an unknown agent name and an empty list both refused
      where somebody is watching. With one adapter every run still names `claude-code`, so the
      knob's useful settings arrive with the second one — but the path is real: `bid::evaluate`
      passes `run.spec.agent`, not a hard-coded kind.

      What is left of this item needs a phone: the platform host process and probing, both below.
- [x] **Delivery plane, the local half** (ADR-0010): `Notification` as a small typed subset of
      run events — finished, failed, will-miss-its-deadline — projected from the event log rather
      than emitted as a side effect, with per-sink cursors and an at-least-once outbox keyed on
      `(sink, seq)`. The first sink is a **command the node's owner nominated**, which is the only
      route a general-purpose machine can honestly claim: there is no push credential, no mail
      transport and no HTTP client here, and the one thing it can verify is that the program
      exists. The service is the owner's declaration and the command is only how it is invoked, so
      `Constraint::HasService` can still ask for "push" later. `offload sinks [--test]`.
      Delivery is its own tick — never anything a turn boundary or a checkpoint waits on.
- [x] **Delivery plane, the fleet half**: a notification carried by a *peer's* sink — the phone
      that holds the push credential, for a run that finished on the desktop. The sender keeps the
      outbox, because the node that logged the event is the only one that knows what it has
      already said; the peer is told *what* to say and never *how*, so nothing about a route ever
      travels. `Deliverer` is a registration of its own beside `Host`, which is this ADR's whole
      claim in code: the interesting device answers `NoHost` to every run and implements this one.
      **Away is not gone** — a queued notification for a device the fleet still knows waits
      indefinitely, with nothing spent from its retry budget, and only a route the fleet no longer
      lists at all is given up on. `offload sinks` shows the fleet's routes and explains a silence.
- [x] **Which route a run asks for**: `offload run --notify push|email|chat:team|all|none`, on the
      spec (wire v10) so it travels with the run rather than being remembered by the process that
      typed it. A **service**, never a route id — an id is a node's own name for one of its sinks,
      so naming one would pin a run's news to a device, which is the thing this plane exists to
      stop mattering. And **not** a `Constraint`, which was this ADR's own wording and pointed the
      wrong way: a constraint selects a *node*, and a phone holding a push route and a mailbox
      satisfies `HasService { push }` and then tells somebody twice. The filter runs where the news
      is *noticed*, so a route a run never asked for is never owed an outbox row. Two silences
      answered at the keyboard (ADR-0014's argument on this plane): a service no device in the
      fleet offers is reported at submission rather than discovered at breakfast, and `none` is
      said back — neither a refusal, because this project does not cancel a run over reporting.
      Not editable: two editable spec fields share one counter on purpose.

      Verified on two daemons with three routes — push and email on the desktop, `chat:team` on a
      phone that hosts nothing. A default run reached all three; `--notify chat:team` was carried
      by the phone alone.
- [x] Capabilities a run can *act on* (ADR-0011, `Role::Resource`): granted per run, projected
      into the agent's own tool protocol — for Claude Code an MCP config at spawn time. A resource
      held by another node is a proxied call, never a shipped credential.

      - [x] **The fence, first.** `--strict-mcp-config` on every spawn, unconditional and not
            configurable, so a run reaches nothing the fleet did not grant it. It read as part of
            the granting work and is not: without it the fleet had the *opposite* of this ADR's
            rule. Measured on an ordinary laptop — a run inherited five MCP servers, four of them
            the owner's own and three connected, mail and calendar among them, plus whatever
            `.mcp.json` the repo shipped. So a repo could grant itself a service by committing a
            file, and what a run could touch depended on which machine won the bid. Verified
            through the daemon with both kinds of ambient server present: the spawned process
            carries the flag and the agent reports no `mcp__` tools.
      - [x] **Nominating a resource, granting it per run, and projecting it.** `[[resources]]`
            in node config — an MCP server the owner nominated, the sink rule applied to the
            other direction — and `offload run --use email`, by service and never by id. The
            grant reaches placement (`Constraint::CanUse`, because until a resource can be
            reached across the mesh only its holder can honour one) *and* the spawn, where it
            becomes both an MCP config and permission to call it. One function returns both after
            the first end-to-end run showed why: the config alone gave the agent a tool it could
            see and was then denied. Verified against a real agent and a real stdio MCP server —
            granted, it read the inbox; ungranted, the same prompt found no such tool; and a
            service nothing in the fleet offers was refused at submit, naming the constraint.
      - [x] **A resource on another node, reached by proxying the call to it.** ADR-0011's
            motivating case, and the substantial piece its consequences section said was hiding
            behind a small role: the phone holds the mailbox and hosts nothing, so the desktop
            runs the agent and forwards the call. The agent gets an MCP server whose program is
            `offloadd` itself and which carries the agent's own protocol opaquely at every hop
            (wire v19). A proxied server has **no `env`** — that is the whole feature — so it is
            named by service rather than by the holder's own id, which this node does not know.

            It retires `Constraint::CanUse` from what `--use` builds, and the ADR's own words
            are what retire it: the grant constrained placement "until a resource can be reached
            across the mesh". Keeping it would have pinned the run to the one device that cannot
            host it. The refusal moves to submission instead — a service nothing in the fleet
            offers is answered at the keyboard (ADR-0014), and one somebody offers is reached
            from wherever the run lands.

            Verified with a real agent and a real stdio MCP server on two daemons: the run ran
            on the desktop, called `mcp__email__read_inbox`, and wrote what the phone's mailbox
            said. Ungranted, the same prompt found no `mcp__` tools at all.
- [x] **The approval channel** (ADR-0017): a run stops and asks rather than being denied.
      `offload run --ask`, the question out over the delivery plane, `offload approve|deny` back,
      `offload asks` for what is waiting. The agent's half is a `PreToolUse` hook whose program is
      the daemon's own binary — the agent's own protocol, and no discovery or configuration to get
      wrong. Measured rather than assumed, on `claude 2.1.237`: the hook blocks the tool call, an
      `allow` executes a command that was denied without it, and a hook killed at its timeout
      leaves the call **denied** — which is the property the whole design rests on.

      Three things the building decided. The hook fires for *every* matching call and the agent
      never says whether permission was needed, so the tool list is short (`Bash`, `WebFetch`) and
      a run's own grant suppresses the question — safe only because a match means "let the agent
      apply its own rules", never "allow". Nobody answering is a **pass-through**, not a denial, so
      the channel's failure mode is exactly today's behaviour. And a question nobody could answer
      — no route in the fleet, nobody streaming the run — is declined instantly instead of stalling
      the run for minutes to reach the same conclusion by clock.

      Verified with a real agent, all three outcomes: approved (the command ran), denied (the agent
      quoted the reason back and wrote nothing), and unanswered (54.9s of patience under a stated
      one-minute deadline, then the pre-existing denial).

      **And the answer travels** (wire v12): `ClusterMessage::Answer` goes to the holder — the
      mirror of `EditSpec` going to a field's owner, and stricter, because a blocked *process*
      exists on exactly one machine. No epoch on the message: a question's identity is the agent's
      own `tool_use_id`, which only exists while that process is blocked on that call, so a stale
      answer matches nothing. `offload asks` canvasses the fleet rather than reading gossip, for
      `explain`'s reason. Verified on two daemons: a run blocked on the desktop, both questions
      pushed to a phone that hosts nothing, both approved *from the phone*, and the desktop's log
      recording `allowed (an operator on phone)`.

      **And now it covers the mode it is running in** (2026-08-21). What the hook matches is a
      function of `PermissionMode`, not a constant — under `AcceptEdits` the agent allows edits by
      itself so asking about them is noise, and under `Ask` it gates every one so *not* asking is
      a silent denial. A constant was wrong in one direction or the other and this one was wrong
      in both. So ADR-0008's refusal of `Ask` is lifted for a run that can answer for it, which
      is the last thing that ADR was waiting on, and still refused (naming `--ask` as the way
      out) for a run that cannot.

      Widening forced the question ADR-0017 said it would: how many questions is a person willing
      to answer for one run? A **budget** — `--ask=N`, default 20, on `RunProgress` so it travels
      and a migration continues the count. Running out is not a refusal: the run carries on with
      its calls decided by the agent's own rules, which is what nobody-answering does and what
      every run did before the channel existed, so the failure mode of running out is the
      behaviour we already ship. Spent when a question is *put* to somebody, never when a grant
      or an unreachable fleet meant nobody was shown one. Verified with a real agent under
      `--permission ask --ask=2`: two `Write` questions approved and written, the third refused
      by the budget, one line in the log saying so, and the agent reporting the block itself.
- [x] Real per-account fingerprinting, so fleet-wide rate-limit accounting works. Derived from
      the account uuid the agent itself records, hashed with a versioned salt and a pinned
      known-answer test, because the value is compared across nodes and rendered into
      human-facing text. The uuid rather than the email address beside it: a digest of a
      guessable string is reversible in practice however opaque it looks, and `AccountId`
      promises not to be.

**Demo:** phone on mobile data joins from outside the LAN. On battery it declines work with a
legible reason but still submits and observes runs. Plugged in on Wi-Fi it wins a bid for a
review run and executes it.

**Both demos re-run in session twenty**, on two daemons with a shell script standing in for a push
credential and the sink on the node that has no `host-runs` at all: alpha reports "no delivery
routes on this node — it uses the fleet's, below", the run finishes there, and beta's script is
what fires. `sinks --test` works and audience selection is real — `--notify none` delivered
nothing. Three output bugs came out of the same pass and are in the handoff; all three were a
*report* disagreeing with the *decision* it describes.

**Second demo, the one that makes a phone worth enrolling:** a run finishes on the desktop
while nobody is at it, and the phone — which cannot host an agent at all — is the node that
delivers the notification, because it is the only one holding a push credential.

**Verified**, with a fake agent on the desktop and a shell script standing in for a push
credential on the phone. The phone joined with `{submit, deliver}` and was never granted
`host-runs`; the desktop has no route of its own and reports "no delivery routes on this node — it
uses the fleet's". A run finished on the desktop and the phone said so. Then the phone was
`kill -9`'d, two more runs finished, and the desktop reported "phone is not answering — 2 waiting
for it to come back" without spending a retry; the phone was restarted thirty seconds later and
was told about both, in order.

---

## Phase 6 — Hardening

- [x] **A machine that will not send says so** (session sixty-nine, **ADR-0059**).
      `quinn_udp::UdpSocketState::send` answers `Ok(())` to every send error but `WouldBlock` —
      soundly, because a UDP send error is non-fatal — so a node whose kernel refuses every
      datagram believes it sent them all, and the only report anywhere is that the *peer* did not
      answer. That asymmetry is the whole of what made session sixty-six's macOS fault take three
      sessions to find, and the only evidence that ever named it was one `EHOSTUNREACH` under
      `strace`.

      `QuicTransport` now owns its socket (`Endpoint::new_with_abstract_socket` over
      `sends::WatchedSocket`) and calls `try_send`, which hands the error back. It counts the
      refusal per destination and then answers exactly what quinn's own socket would have — the
      same three arms, `EMSGSIZE` still ignored as an MTU probe. It is a count, deliberately not
      a decision: acting on a send error would turn a two-second network change into a
      membership event, and would be a second liveness opinion beside the detector.

      One line in `offload status`, printed only when the number is above zero. Measured on a
      live daemon whose seed was `255.255.255.255:7433`: `sends 9 refused by this machine's
      kernel`, climbing to 16 in twenty seconds, with the daemon's own INFO log saying **nothing
      whatsoever** and `offload nodes` reporting a healthy fleet. The control is in the same
      walk — bravo `kill -9`'d, three `no answer` lines, `dead ~2` in `offload nodes`, and no
      `sends` line at all. Two failures that were indistinguishable, told apart.

      The socket swap under it was walked separately and heavier: a run submitted from the node
      with no agent, placed on the other over QUIC, eight turns, eight checkpoint blobs
      replicated across the wire, and `offload logs` forwarded back in full.
- [x] Deterministic simulation over the real cluster (`offload-cluster/tests/storm.rs`): a
      generated script of ticks, partitions, isolations, bid rounds, declines and lost
      acceptances, against four real `Cluster`s on the in-memory transport with the clock as an
      argument. Not `turmoil`, and the reason is that the seams it would provide already exist
      here — `probe_round(now)` takes the time, `MemoryNetwork` cuts and heals at an exact
      instant, and the transport is behind a trait. What `turmoil` would add is a simulated
      *socket*, which is `quinn`'s business rather than this crate's. Clock skew is the one axis
      not varied, deliberately: it would exercise `Millis` arithmetic that `offload-core`'s own
      properties already drive, and make every failure here ambiguous between the two.

      Five properties: one arbiter never spends an epoch twice, a settled fleet agrees who holds
      the run, at most one node is left holding it, a grant only goes to a node that answered,
      and a healed fleet agrees everybody is alive.

      Found: **a refused round handed its tokens out again.** "A grant spends a token whether or
      not it is confirmed" was implemented within a round and not across rounds — `place` works on
      a local copy and publishes only a confirmed grant, so a round nobody confirmed left the
      arbiter's view where it started and the next round re-issued the same epochs, to nodes that
      were already running under them. Two grants at one epoch from one arbiter is what the epoch
      exists to prevent.

      Found second, once it grew a blob store: **a checkpoint on a merely-suspected node could
      not be fetched.** `fetch_blob` swept only `Alive` peers, which is the one status filter its
      own doc comment argues against — the moment a checkpoint is needed is the moment somebody
      has gone quiet. Ordered rather than filtered now.

      Also worth keeping: three of the five "invariant violations" it reported first were the
      *harness* breaking a rule the daemon does not — any node placing a run, a host publishing
      what it was only told to record, and two nodes minting one `RunId`. And the fixpoint was
      declared a full rotation too early, which is `churn.rs`'s own lesson from the other end.
- [x] Property tests over `offload-core`: generated transition sequences, the merge, the bid
      round's winner and `explain`. Three bugs, one of them the double-grant at a single epoch
      that ADR-0002 claimed fencing already handled — reachable with no partition at all,
      through a few missed probes or one arbiter re-offering inside a round. The fix is in that
      ADR's amendment; what is *not* fixed, and cannot be without quorum, is a partition that
      persists: two agents run, and fencing guarantees only that exactly one leg survives the
      moment the records meet.
- [x] The multi-node half of it (`offload-core/tests/churn.rs`): several `ClusterView`s,
      gossip delivery, supervision passes, crashes and restarts, all from a generator. Five
      properties — at most one holder once the gossip stops, a settled fleet agreeing about who
      holds it and at what epoch, two arbiters not leaving two holders, order-independence, and
      a run outliving the machine it ran on — plus two invariants after every event: no view
      walks an epoch back, and nobody who is up believes *it* is the run's absent holder.

      Found: **a holder took its arbiter's word for its own absence.** Two missed probes and it
      has no lease, cannot renew, and cannot record its own agent finishing. `merge_node` had
      the rule from the start; `merge_run` did not.

      Written down rather than fixed: an `Orphaned` record is its arbiter's observation and
      nobody may relay one, so an orphan half-delivered when its author crashed stays that way.
      Benign — an orphan grants no authority and any action bumps the epoch — and the property
      says so precisely instead of skipping it.
- [x] Reconsidered negotiated arbitration vs `openraft` over `Stable` nodes (ADR-0002, ADR-0006).
      **Answer: keep it** — ADR-0018, argued in `storm.rs` rather than from unease. What the
      simulation shows is that there are *two* paths to two live legs of one run, and quorum
      removes only one of them: two arbiters, yes; a grant whose acknowledgement was lost, no. The
      second needs no partition — proptest shrank it to `[Swallow(3), Place(0), Turn(2), Turn(3)]`
      on a fully connected four-node fleet — and no consensus protocol reaches it, because the
      decision is replicated and the side effect is on one machine. Meanwhile the price is a
      quorum floor that is either one node (ADR-0002's "fixed coordinator by config", rejected
      outright) or a fleet that stops placing when a laptop shuts.

      Found on the way, and the reason the item is worth more than its answer: **the leg that lost
      a run set the run's reported position for the rest of its life.** `RunProgress` was merged
      forward-only on the stated grounds that its numbers have "a single author by construction",
      which is exactly what a second grant breaks — so a run on turn 3 reported turn 19, with the
      losing machine's worktree summary beside it, and the surviving leg could never correct
      either because everything it said was smaller and refused. Two kinds of number, two rules
      now (ADR-0005's second amendment, schema v7): position follows the leg the record settled
      on, spend stays cumulative because the money left the account on both legs.

      And a second, smaller one from the same property: the ordering was not **total**. Two legs
      writing different worktree summaries in one millisecond were settled by whichever gossip
      arrived first, so a fleet at rest disagreed for ever about a string — no partition, no fork.
- [x] Turn boundaries in the simulation (`storm.rs`'s `Ev::Turn`): the capture onto the record,
      the turn count, and the worktree summary beside it, published by whichever leg its own view
      says holds the run — which during a fork is two of them, each correctly. One property (the
      position the fleet reports is the surviving leg's) and one hand-arranged case for the half
      the generated search finds only with a deep run.

      Worth keeping, because it nearly passed for the wrong reason twice: the harness first did
      not stamp its records at all, so the merge fell back to the rule for records written by an
      older build and the property tested the fix out of existence. And the accidental fix — the
      worktree summary joining the comparison to make it total — was enough to make the saved
      counterexample green on its own, which is what reverting each half separately is for.
- [x] Property tests over the allowlist, and the measurement they forced. `ToolPattern::risk`
      compared the text of a pattern against a list of program names, so `Bash(/bin/sh:*)`,
      `Bash(python3.11:*)`, `Bash("sh":*)` and `Bash(FOO=1 sh:*)` all read as `Scoped` — which is
      what `require_scoped` accepts from a *repository's* `.offload.toml`. Verified against
      `claude 2.1.238`: such a run executed `/bin/sh -c "cargo build --version"` while being
      refused the same command directly.

      The same probing confirmed the property the feature rests on and had never checked: the
      agent **decomposes a compound command** and refuses
      `cargo test --version && cargo build --version` under `Bash(cargo test:*)`, naming the
      uncovered half. A prefix grant is therefore sound, and an interpreter in first position is
      the only leak of its shape.
- [x] Property tests over membership, and the escalation they found. ADR-0012's delegation
      sketch has a `may_issue` that never reached the code, so an approver could mint `HostRuns`
      — the grant mitigation 1 keeps behind the passphrase — for any device, and every peer
      admitted it. Fixed by making renewal distinguishable from minting rather than by a flat
      bound, since a flat bound stops an approver renewing a host node and brings back the
      monthly-passphrase failure. See ADR-0012's 2026-08-21 amendment. **Wire v20, and every
      device re-joins**: the fix changes the bytes a signature covers, and there is no migration
      for a signature. There is now a pinned known-answer test over all four credential types,
      because the change broke every existing credential with nothing in the tree going red.
- [x] Property tests over the outbox (`offload-store/tests/outbox.rs`), against a real SQLite.
      The dedup identity was already sound — `run_events.seq` is node-global, `topic` separates
      the logs, the projection is total, and the key is enforced. The **queue** was not: a route
      that is away keeps its rows for ever *and* they are the oldest, so one page across all
      sinks starved every working route once a sleeping phone had more than a page's worth. Per
      sink now, and ordered by when this node noticed rather than by a sequence that spans two
      independently-numbered logs.
- [x] Property tests over the blob collector (`offload-store/tests/gc.rs`), which found it
      **deleting the checkpoints of live runs**. It matched a blob's hex hash inside `run_json`,
      and a stored run spells its hashes as arrays of integers — so nothing ever matched and
      every blob looked unreferenced. Its own comment claimed a substring match "can only be
      over-cautious". It survived because the test hand-wrote a JSON shape `save_run` has never
      produced. References come from `Checkpoint::blobs` now, and an undecodable run row fails
      the whole pass rather than being skipped.
- [x] Property tests over the workspace, the other place work is silently not kept — and both
      halves of it were. The **untracked-file policy** matched a list of twenty directory names
      against the path text, and on the repos of one laptop `build/` holds hand-written
      Dockerfiles, WordPress tracks 132 files under `wp-includes/js/dist/`, and two more commit
      `vendor/` and `node_modules/`: so an agent's new file in `build/` was reported as build
      output and left behind. A name now loses to what the repository tracks, asked per
      directory. Also `Selection::summary`, which reported a file dropped for the checkpoint's
      *total* budget as one that was too big — two settings, and the message named the wrong one.
      This closes open question #3 by measurement.

      And **`WorkspaceManager::adopt` inferred "current" from "exists"**, so a run that
      checkpointed here, migrated away, did more turns and came back was resumed from the
      earlier leg's checkout, with its bundle, patch and transcript all skipped, silently, the
      log saying "adopted in place". The worktree records the turn it holds; adoption is a
      comparison now, unknown is not current, and a superseded checkout is moved aside rather
      than deleted.
- [x] Property tests over the agent's argv. The prompt is positional and sat in the middle of
      the line, so a prompt beginning with a dash — `offload run -- "--version should be
      documented"`, which the CLI accepts — reached `claude 2.1.238` as an option and the run
      died at spawn with `error: unknown option`. It goes last, behind `--`; both that `--`
      terminates the variadic `--allowedTools` and that the grants before it still apply were
      measured, and the smoke example now spawns a dash-leading prompt end to end.
- [x] Property-hunting the run registry's writers, which found the answer upstream of the store:
      `absorb` skipped a peer's whole record for a run this node holds, and the deadline and
      priority on it belong to the run's *home* node. A run submitted on the laptop and running
      on the desktop never heard `offload deadline` — the number that orders the runs waiting for
      a slot there. Split into `merge_run` for the record and `ClusterView::merge_spec_edit` for
      the edit, with the settling rule written once; and what is handed to the store is now the
      record as settled rather than as it arrived.
- [x] Property tests over `offload-store`'s writers — the surface the last session named and
      left unanswered. The question it left ("do any two of those windows overlap across an
      `await`") had a better answer: none of them do, and two of them never needed one. A
      **whole-row write from a copy that has been sitting** is the shape, and it loses whatever
      landed in between with no error anywhere. The heartbeat renewed leases from a listing taken
      before its loop began, so a run that finished mid-loop was written back as `Running` — and
      then kept that way, since the same loop renews the lease of the run it resurrected and
      nothing orphans a holder that is heartbeating. And the *gossip* path was worse: last
      session's fix handed the store a peer's whole record for a run held here when all it had
      taken from it was the edit, and the copy it came on is a gossip tick old — so an edit
      arriving a second after a checkpoint undid it, blobs and all. `Store::update_run` is the
      read-modify-write under the store's own lock; `Host::record_spec_edit` is the edit
      travelling as an edit. Ten writers converted, two left on `save_run` for stated reasons.
- [x] Property-hunting `offload-node::server`'s forwarding — the second of the three surfaces.
      The question was whether a command is ever applied locally *and* forwarded; the answer is
      that two are neither. `offload cancel` reported "not running" from this node's process
      table about a run running on another machine, and `offload rm` **succeeded** on a node with
      no checkout: idempotent teardown returns `Ok`, so `cleanup` wrote `workspace: "removed"`
      about somebody else's disk — and progress gossip breaks a tie on the author's clock, so
      that copy won the whole fleet. A finished run names no node, so only this node's own disk
      can answer "is it here": `Removal::{Removed, NothingHere}`. Cancel now names the machine
      rather than lying about it; forwarding it is a new cluster message and therefore a wire
      version, which is why it is a follow-up and not this commit.
- [x] …and the follow-up taken: **a cancel travels** (wire v21). `ClusterMessage::Cancel` to the
      run's holder, or to the node that owns the record when nobody holds it — the same
      `arbiter_for` a `SpecEdit` uses, because a run nobody holds is a record rather than a
      process. Two more falsehoods went with the process table: a commitment this node was
      *itself* holding and had not started also read as "not running", and the answer says which
      of the two it stopped, because a mid-turn agent cost money and a commitment did not. It
      turned up a second bug on the way — the cancel channel was the only way to stop an agent,
      so a node that had **lost** a run stopped its agent and then wrote `Cancelled` into the
      record it had just accepted from the new holder: unfenced, at their epoch, terminal, and
      therefore winning everywhere `absorb` does not refuse it. `Halt::{Cancelled, Superseded}`.
- [x] `offload-probe`, the last of the three surfaces — the one crate whose whole job is not
      over-claiming, asked what it reports when a file it reads is present but says something
      else. It was reporting **this laptop's touchscreen battery as the machine's**: four
      entries under `/sys/class/power_supply`, three of them `type=Battery`, and the loop kept
      whichever `read_dir` yielded last. `offload probe` printed 0% while `BAT0` sat at 98%.
      `scope=Device` is the kernel's own discriminator and nothing asked it. Fixed with a pure
      `settle` over the parsed directory (order-dependence cannot be tested against a real
      `/sys`), the lowest of several system batteries, and `Unknown` rather than `Ac` for a
      battery whose level will not parse.
- [x] …and the last command that acted only where it was typed: **`offload checkpoint` travels**
      (wire v22). It refused with "it is not running here, so there is no turn boundary coming" —
      true about this machine and silent about the one the run is on. One kind of target rather
      than the cancel's two, because a turn boundary is a moment in a process (ADR-0004): a run
      nobody holds has none coming and is already in the pool, which is where a checkpoint would
      have put it, so that is said instead of forwarded into nowhere.
- [x] **`drain` did not stop accepting**, which its own doc comment has claimed since it was
      written. It handed its runs over and returned, so a laptop about to be closed went on
      bidding and winning — measured, one second after `offload drain` said "nothing to hand
      over", and the run it had just moved could come straight back. One flag, refused at the bid
      *and* at the grant (a grant can arrive after the bid that earned it), reported by `offload
      status` because a node that silently takes nothing looks exactly like a broken one.
- [x] A displayed run id that named every run of the preceding minute. `RunId::short` printed
      eight hex characters; UUIDv7 bytes 0..6 are a 48-bit millisecond clock, so bytes 0..4 are
      the top 32 bits and hold still for 65_536 ms. `CLAUDE.md`'s twelve-character rule was
      implemented in the CLI's own `short_id` and not in the type, so a daemon refusal named two
      runs at once beside a CLI line that had disambiguated them. One number, `RunId` alone
      (`NodeId` is a key and `BlobHash` a digest), and the first test the length has ever had.
- [x] A running run reported its worktree as `preparing`. `RunProgress::workspace` was written
      at launch and then not again until the leg ended, so `offload ps` said `preparing` for the
      whole of an overnight run — beside the column saying whether that work is replicated, and
      gossiped, so the fleet said it too. Refreshed at every turn boundary rather than at every
      checkpoint, since the checkpoint cadence is configurable and can be off.
- [x] The test suite's answer depended on whether a daemon was running. The device reservation
      ledger is machine-wide by design (ADR-0013), so `Supervisor::new` opens the real one and
      four capacity tests read an unrelated agent's slots — failing only sometimes, and blaming
      whatever had just changed. The reverse too: a test that drives a run reserved against the
      live ledger. `Supervisor::with_private_ledger`, the sibling of `with_home`.
- [x] …and the rule made structural rather than remembered: `fail` itself refuses a run this node
      does not hold, so a future caller cannot repeat any of the four. Verified by deleting the
      caller-side guard and watching the test stay green.
- [x] …and a fourth, on the numbers rather than the states: a leg that had lost a run still
      published its own worktree summary as the run's, which wins the fleet because progress ties
      break on the author's clock. `Supervisor::holds` gates it.
- [x] …and a third door into the same room: `fail_if_unfinished`'s `still_ours` tested the run's
      state rather than its holder, so an agent dying at the moment its run was reassigned failed
      the run for the machine that had just taken it. Holder and epoch now.
- [x] A leg the fence refused marked somebody else's run failed. `drive`'s guards were right;
      `launch` treated their refusal as an ordinary failure and called `fail`, which uses the
      unfenced `abandon` — writing `Failed` into the new holder's row at the new holder's epoch,
      and queueing the run for auto-resume. `SubmitError::LostTheRun`. Survived because the
      existing test drives `drive` directly and never exercises its caller.
- [x] A cancel that lost its race still said the run was cancelled. `Run::cancel` refuses a
      terminal run; the log line was written regardless, and `LogKind::Cancelled` is terminal to
      everything that reads it — so a run that had just succeeded had "cancelled" as the last word
      in its own log. Written only when the transition applied.
- [x] The delivery plane's third route state. A peer whose credential stops verifying was
      filtered out of the fleet's routes, so everything already queued for it was abandoned as
      "no longer exists in the fleet" while `offload sinks` listed it. `Route::unusable` waits
      like `reachable` does, and both the pass and the report derive routes from one walk.
- [x] **Audit log** of every assignment and every epoch rejection (`offload_core::audit`, schema
      v6, `offload audit [<run>]`). A third log, per-node and never gossiped: a run's log is served
      from whoever holds the run, so a line written by a leg that has just been fenced *out* is one
      nobody reads. It answers the two questions `offload explain` cannot — which machine a run was
      granted to at the time (with the epoch, so two grants at one number are visible), and whether
      a write about it was ever refused. Outlives the runs it describes on purpose.

      It found one thing on its way in: collapsing "may I conclude this run" with "may I describe
      its worktree" reported a *completed* run as a refused write, because a finished run has no
      holder. `holds` and `describes` are two questions now.

      Two bugs in the *writing* of it, and one in the fix, all found in session twenty by running
      two daemons and reading the output rather than by any test — plus a third the fix's own new
      property caught within the hour: the local branch of a grant reported it *before* `accept`
      could refuse, so a node declining its own grant recorded giving it away. `AuditEvent::Granted` was written from `Host::record` —
      whose doc names only the arbiter's post-grant persistence, while `Cluster::learn` calls the
      same hook for every record a gossip merge moved. So an ordinary two-node run produced three
      rows saying "granted to X at epoch 1", which is the log's own signature for an arbiter
      spending a fencing token twice: a false report of the worst failure in the design. And the
      other caller is why the row was **missing** whenever an arbiter granted to *itself* — that
      path returns before `record` is reached, so a fleet of one recorded no grants at all.
      `Host::granted` is the decision, `Host::record` is the record; the storm property that guards
      it needs a four-step script and 300 cases to find, which is why the daemons found it first.

      A third kind since: **`Superseded`**, a leg being told the fleet gave the run to somebody
      else. Not a refusal — nothing was attempted, this node was informed — and it carries the
      turn the leg had reached, which is what makes it the answer to a question the position rule
      creates: a run's turn count follows the leg its record settled on, so it drops back to the
      survivor's, correctly and indistinguishably from a fault. The superseded leg is the only
      node that can say the higher number belonged to work that is no longer the run's, because
      the surviving leg never learns there was another one. No migration: a new `kind` value in a
      table that already has the column.
- [x] A flake that pointed at the wrong thing. `server.rs`'s routing test resolved runs by
      `RunId::short()`, whose own doc comment says never to use it for a lookup — and twelve hex
      characters is exactly the 48-bit millisecond clock at the front of a UUIDv7, so two runs
      minted in one millisecond print the same twelve and `resolve_run` correctly refuses both as
      ambiguous. It failed as often as the two `now_v7()` calls landed in one tick, with a message
      about an ambiguous id rather than about the routing under test. The product half is left
      alone: refusing is the right answer, and a fourteen-character prefix would reach one random
      byte and turn a rare confusing refusal into a rarer one.
- [x] **`Role::Trigger`: something arrives, work starts** (ADR-0011, settled and built by
      ADR-0020). A program the owner nominated whose stdout is an event stream, and a **rule**
      binding one of its events to an ordinary run — so bidding, leases, epochs, migration,
      `explain`, the audit log and the delivery plane all apply with nothing added, and `RunSpec`
      is untouched. `offload triggers`, `offload when`, `offload rules`, `offload unwatch`;
      schema v8, no wire change.

      Three shapes, each a rule from elsewhere read in a new direction. **The cadence is the
      program's** — no `interval` field, because a daemon polling on a schedule is a scheduler
      inside an orchestrator. **A rule is node-local and never gossiped**, which is what makes
      this the small half of ADR-0019: a trigger belongs to one machine's owner, so it fires on
      one machine. **One occurrence in flight**, with anything arriving meanwhile dropped and
      counted, which is ADR-0019's no-catch-up rule from the other direction.

      What the walk added: a rule **reclaims its last occurrence's checkout** when there is
      nothing uncommitted in it. `cleanup` is never automatic on the sound premise that somebody
      will read the output, and a triggered run has nobody by construction — eight worktrees in
      forty-five seconds, measured.

- [x] **Pruning what a rule leaves behind** (ADR-0021, session twenty-five; schema v9, no wire
      change). The record half of the item above, and the reason it needed a decision: `delete_run`
      cascades to a run's outbox rows, and an outbox row is the whole of at-least-once. A record is
      **spent** once the delivery plane has finished with it — nothing pending in the outbox, and
      every live route's scan past its last event, which is the half with no row to point at —
      it **completed** rather than failed, and it has been **quiet** for as long as a finished run
      is gossiped. Asked about every occurrence at every firing, because each of those is a
      *temporary* no and a prune that gets one chance per record is a prune that mostly does not
      happen. Measured: 193 firings, 101 kept, flat; 190 failures, 190 kept. `offload rules` prints
      the number.

      What reading it for exposure added (§7): **a node does not learn of its own run from
      somebody else.** Every clause guarding a prune is about a peer that is present, so a laptop
      that saw an occurrence running and then closed could teach the pruned record back to the
      node that deleted it — as a run held by itself, no agent, lease renewed for ever.

      What the walk added: `offload unwatch` prunes **past** the quiet period, because that period
      buys the ability to re-ask and there is no next firing to re-ask — it used to strand 101
      records. And what it declined: alpha settles at 101 while **beta grows 81 → 181 in five
      minutes**, because the prune is node-local and gossip is not. That fix is a retention policy
      for a finished run a node neither hosted nor submitted, which is a decision about every run
      and changes what a peer can answer about work it never did.

      **Answered by ADR-0025** (session twenty-six): the bystander keeps it, because that record
      is the index `offload logs` resolves an id in before forwarding to whichever leg ran the
      work. Measured: a peer holding 127 records with 0 events, 0 blobs and 0 outbox rows answers
      `offload logs` byte-identically to the machine that ran it.

- [x] **The blob collector runs by itself** (ADR-0022, session twenty-five). `collect_garbage`
      had existed since phase 2, careful and property-tested, and was called by nothing because it
      was "deliberately manual" — the same premise `cleanup` and `delete_run` were built on, and
      the third one to meet a machine nobody logs into. Measured: one six-turn run left six blobs
      totalling 851 KB, of which the surviving checkpoint referenced **one**, at 243 KB, because
      `record_checkpoint` replaces the checkpoint and a transcript is the whole conversation so
      far. Every run, every node, quadratic in the conversation. A tick of its own beside
      `tend_own_runs`, every fifteen minutes with an hour of grace, because a blob is written
      before the row that references it. Deleting a superseded replica is safe for a reason worth
      remembering rather than trusting: recovery fetches the blobs the *record* names, so a
      checkpoint the record has moved past is already unreachable.

- [x] **Checkouts of runs that are not this node's any more** (ADR-0023, session twenty-five).
      Nothing has ever removed a worktree when a run *leaves* — `remove` has one production
      caller and it is terminal-only — so a migration cost a permanent checkout on every node a
      run visited, and a rule whose occurrences the fleet places elsewhere cost one per firing.
      Measured: **62 firings, 62 worktrees, 145 MB on a peer in five minutes.** Same tick as the
      blobs. The guard worth knowing is the one that says a run has moved on: "not the holder" is
      true of *every finished run*, so it would delete the checkout `cleanup` is manual for —
      `RunProgress::by` is the durable form of "which leg ran this". What it did not reach when
      written — an occurrence that finished on a *peer* — **ADR-0024 reached two items down**, and
      this line said otherwise for a session. Walked in session twenty-six: alpha holds the rule
      and refuses to host, every occurrence is placed on beta and finishes there, and beta's
      worktrees oscillate between 0 and 5 over five minutes of firing every three seconds.

- [x] **A run says who started it** (ADR-0024, session twenty-five; schema v10, no wire bump).
      The three reclaiming ADRs above all stopped at the same edge: `cleanup` is manual because
      somebody will read the worktree, and the only node that can tell an occurrence from
      somebody's run is the one holding the rule — which is node-local. So a peer kept a record
      **and** a checkout per firing (81 → 181 records in five minutes; 62 worktrees, 145 MB).
      One bit on the record fixes both: `Run::origin`, `Operator` or `Rule`, set at creation and
      changed by nothing, so there is no owner to name and no merge rule to get wrong. What
      travels is *not* the rule id — that is one machine's name for one of its own things — but
      the fact that a machine started it. No wire bump: a build that drops the field reads
      `Operator` and keeps everything, which is today's behaviour.

- [x] **A checkpoint stops being one when the run stopped by decision** (ADR-0022 §5, session
      twenty-six). The amendment ADR-0022's own §3 argues for and does not draw: recovery fetches
      the blobs the *record* names, so a checkpoint the record has moved past is unreachable —
      and so is the one the record **still** names, once nothing can start an agent from it.
      `Supervisor::resume` refuses `Completed` and `Cancelled` by name, and nothing else fetches a
      checkpoint at all, so the tick was reclaiming the five cheap superseded transcripts and
      keeping the biggest one for ever. Measured: **472 KB a run** — and session twenty-five wrote
      the same number down as success ("the store settling at exactly one blob per completed
      run"). `Run::resumable_checkpoint` over `RunState::may_resume_later`, whose arms are written
      out so a new terminal state has to decide. `Failed` is the exception and the reason it is
      not `is_terminal`: it is the one terminal state that reopens.

      It reaches a **peer** with no new mechanism, which is most of what the peer-records question
      above was reaching for: two daemons, three operator runs placed on alpha, and beta — which
      hosted nothing — held 18 blobs and **4.9 MB** of replicas. Both went to zero on the next
      pass. What is left on a bystander is the ~1.5 KB record, at whatever rate a person submits.

      Same predicate corrected two reports that were asking `is_terminal` and going quiet about
      the run that reopens: `offload ps` suppressed its `here only` warning for a failed run whose
      checkpoint is on one machine, which is the only row that warning was written for, and
      `offload explain` printed a bare turn number. `Checkpoint::is_durable` takes an
      `Option<NodeId>` so extending them did not introduce a new wrong answer about a run with no
      holder.

- [x] **`offload drain` reported an idle laptop about a run it had just stopped** (session
      twenty-six). The observation session twenty-five left unconfirmed, reproduced deliberately
      and not the race it guessed at: `left` came from `held_count()`, a *capacity* question, and a
      checkpoint captured with `release = true` hands the run back to the pool, so it has no holder
      by design. A run at turn 3, drained where no peer can host, checkpointed at turn 7, released
      and refused by everybody — `nothing to hand over`, the words an idle node prints, while the
      daemon logged `nobody would take it; it stays here`. Counted inside the pass now
      (`mesh::Drained`): every run ends a drain handed over, mid-turn at the deadline, or refused,
      and counting them as they go is exact. All three arms walked.

- [x] **A node keeps what it was told, and says how much of it there is** (ADR-0025, session
      twenty-six). The question ADR-0021 deferred in its strongest terms, answered by measuring
      what a bystander's record is *for* rather than by choosing a retention number: it is the
      index `offload logs` resolves an id in before forwarding to `RunProgress::by`, so a peer
      with 127 stub records — 0 events, 0 blobs, 0 outbox rows, ~1 KB apiece — answers
      byte-identically to the machine that ran the work. Deleting it would not narrow what a
      device can answer, it would end it. And it is the only answer §7's standing allows: a node
      pruning somebody else's record cannot refuse the peer that teaches it back.

      Three supplies, and only one is unbounded: operator runs are human-paced, completed
      occurrences are pruned everywhere (ADR-0024), and **failed** occurrences are unbounded by
      ADR-0021 §2's own decision — measured holding exactly on both machines, 127 and 127. So what
      was missing was never a sweep but that nothing said so on the machine where it piles up.
      `offload status` prints it, plus the same silence one layer down: a **checkout with no run
      record**, kept on purpose and possibly the only copy of an agent's uncommitted work, which
      ADR-0023 named as a residual and nothing pointed at. Counts, not sizes — the cheapest command
      in the CLI must not be the one that walks the most disk.

- [x] **A run says which news is worth interrupting for** (ADR-0026, session twenty-six; no wire
      bump). ADR-0010's audience says *who*, and nothing said *what* — which only mattered once
      ADR-0020 made runs arrive by themselves. Found by walking a rule with a sink configured, the
      one combination no trigger walk had ever used: **13 firings → 11 notifications**, every one
      "finished after 2 turn(s), $0.0005", and the only way to quieten it (`--notify nobody`)
      delivered **0 of 11 failures** on the same rule failing every time. Two settings, both
      wrong, and no third could exist because `Audience` selects routes. `RunSpec::notices`
      selects kinds; `offload when` defaults to `problems` and `offload run` does not. Deliberately
      not keyed on `Origin`, which ADR-0024 keeps out of the delivery plane — the author decides
      and it travels. After: 37 succeeding firings → 0, 14 failing firings → 14.

- [ ] Metrics and a Prometheus endpoint. **Deliberately still open, and worth arguing before
      building**: nobody scrapes their phone, and a counter saying three epochs were rejected today
      answers none of the questions this project's users ask. The audit log above was the half of
      this item with a stated purpose; the metrics half needs one.
- [x] One daemon per state directory, enforced. Two used to both start — the second unlinked the
      first's socket, leaving it running, holding runs, renewing leases and unreachable. A SQLite
      exclusive transaction on the directory (the broker's mechanism, for the broker's reasons),
      plus a connect-probe before `bind` removes what looks like a leftover, because a socket
      shared by two *different* directories is the case a directory lock cannot see.

**Demo:** 10k random churn/partition events in simulation, zero invariant violations.

---

## Phase 7 — Nice to have

Not scheduled. Recorded so they don't get invented mid-phase.

- Run DAGs: fan out a refactor across repos, gate on review
- **Reaching the fleet from outside the LAN** — decided (ADR-0037), mostly unbuilt. In order:
  measure IPv6, then one reachable end (the laptop, forwarded, with the phone always dialling out),
  then a relay the owner hosts, then iroh for punching. **§2's two build items are done** (session
  twenty-nine): a seed may be a **name**, resolved on every attempt, and the list is **re-dialled**
  while there is no live peer — where before a name was refused outright and the list was dialled
  once at startup, so the self-healing existed for multicast and not for the internet. Measured
  before: one attempt, given up after 30s, and two daemons that never met. The walk then found what
  the re-dial exposed — **an impolite death healed and a polite departure was permanent** — now
  fixed. Everything the laptop needs to *say* already leaves through a sink, so nothing needs the
  phone to be reachable. `offload-relay` is a separate non-member binary that forwards ciphertext
  and verifies registrations by node key alone: no certificate, no fleet key, and therefore none of
  the trust a VPS joined as a member would hold. Not a cloud function — it needs a long-lived UDP
  socket.
- **A node that does no work** (ADR-0038, unbuilt): the rendezvous half, as an ordinary member
  rather than a service — one binary, one enrolment, one trust model, and it shows up in `offload
  nodes`. Being a member is *not* enough to be dialable, because ADR-0015 §1 keeps addresses inside
  the transport: what it adds is **introduction**, a third `Resolver` source behind mDNS and the
  seeds. Cheap because an address is routing and not authority — a hostile introduction wastes a
  dial and can never impersonate, since the handshake still demands a fleet-signed certificate and
  proof of the key. Its cost is the run plane: a member reads every prompt the fleet gossips, so
  `offload invite --introducer` mints the first certificate that carries **less** than the door
  grants, and the paths serving run content withhold from it.
- Additional agent adapters
- **Work that is not an agent run** (ADR-0019, accepted and unbuilt): a program the node's owner nominated,
  addressed by service exactly as a sink and a resource are, fired by a schedule whose occurrences
  have a **derived** `RunId` so two firings of one tick merge into one record instead of racing.
  The `RunSpec` split it needs is the largest single edit since capabilities became instances.
  Its smaller half — `Role::Trigger` — is **built** (ADR-0020, session twenty-four), which leaves
  this item as the `Work` enum and the agent-agnostic supervisor.
- ~~**Pruning a triggered run's record**~~ — **built** (ADR-0021, session twenty-five). A rule
  prunes every occurrence the delivery plane has finished with, at every firing: completed rather
  than failed, nothing pending in the outbox, every route's scan past its last event, and quiet
  for as long as a finished run is gossiped. What earns the right to call `Store::delete_run` is
  that last-but-one clause pair — by the time a record goes, there is no news left to cascade away
  with it.
- **Which account a device spends on** — **built** (ADR-0028, session twenty-seven). `[agent]
  config_dir` nominates the agent state directory, which is what selects the account, and it
  reaches all three places that ask about that directory — the probe, the capture, and the spawn,
  which is now *told* rather than left to inherit. `[agent] account` is the guard half: a node
  logged in as anybody else reports the agent as not authenticated with both fingerprints named.
  Several accounts on **one** daemon is deliberately not built — it is a placement dimension, not
  a config field, and the ADR says what the residual is.
- ~~**Retrying a rule's occurrence on the node that hosted it**~~ — **built** (ADR-0030, session
  twenty-seven). ADR-0027's stated residual, and its stated bound was wrong: a peer retried a rule's
  occurrences 23 times over 29 firings while the rule's own node reported zero events dropped.
  Recovery of a machine-started run belongs to the machine that started it, answered from `origin`
  (which travels) and `runs.rule` (which deliberately does not survive a merge), so no `RuleId`
  leaves the machine that owns it.
- **The account's own usage limit** — **built** (ADR-0029, session twenty-seven, wire v23). The
  agent reports a blocking rate limit with a reset instant every turn, and nothing remembered it,
  so the next run was started into the same wall. A node now holds new runs until the limit lifts
  and bids `Availability::NotBefore`, which commits like any full node and says *when*. What is
  **not** built and is argued rather than deferred: gossiping it, because an account's position is
  a moving value and ADR-0013's rule about attendance applies.
- ~~**Absence history that survives a restart**~~ — **built** (ADR-0031, session twenty-seven).
  ADR-0007's hold-down reads `typical_absence`, the store module built to persist it had no
  production caller, and the same history was a gossiped field with no owner — relayed on first
  contact from whichever peer happened to teach it. Per-node and durable now, with one learner
  instead of two.
- ~~**A rule says what its flags will amount to on this fleet**~~ — **built** (ADR-0032, session
  twenty-eight). The two flags whose promise the fleet may not be able to keep — `--notify
  <service>` and `--ask` — were answered on `Response::Submitted` and on nothing else, so
  `offload run` warned and `offload when` printed a reassurance naming three notifications it
  could not deliver. The warnings were on the command somebody is sitting in front of and absent
  from the one that fires unattended for months. `Reach::{Somebody, NobodyWanted, NoRoute}` is the
  third answer, because a silence the author asked for is not a silence the fleet imposed.
- ~~**A question says how long there is, and something says when the window shut**~~ — **built**
  (ADR-0033, session twenty-eight). `--ask` and rules had never been walked together though both
  are justified by a person. `Notice::NeedsDecision` promised a clock in its own doc comment and
  carried none, and `notable` excluded the whole `Answered` kind on the grounds that the run's
  *result* reports what happened — which `Notices::Problems`, a rule's default, throws away. Two
  firings put two questions on a phone and nothing else, ever.
- ~~**A drain drains a node with no fleet**~~ — **built** (ADR-0034, session twenty-eight).
  `stop_accepting()` was the first line of `Mesh::drain`, which `Request::Drain` skips entirely
  when there is no mesh — so on a fleet of one, which is how phases 1 and 2 run, `offload drain`
  set no flag, `offload status` said `accepting yes`, and the next submission started. Two faults
  at once: the flag, and the refusal it turns on not being on the path a fleet of one takes.
- ~~**A drain says what it is waiting for, while it waits**~~ — **built** (ADR-0035, session
  twenty-nine). ADR-0034's stated residual, walked: `offload drain` printed **nothing at all** for
  forty measured seconds and up to five minutes, because a run blocked mid-tool-call on `--ask`
  reaches no turn boundary until somebody answers — and `offload asks` two windows away had the
  answer that released it in one second. It streams now (`Response::Draining`), and the walk added
  two more from the same command: a run that **finished** while the drain waited was counted as one
  left behind and logged as `still mid-turn at the drain deadline` (`wait_for_checkpoint` answered
  `None` for three reasons and the caller read one), and ADR-0034's own move of `stop_accepting()`
  into the operator's handler left the **`SIGTERM`** path accepting work for its whole shutdown —
  measured, `spawning claude code` three seconds after the signal. `mesh::depart` is the one way
  out, with `Mesh::drain` private so the compiler keeps both callers in step.
- ~~**A rule says what `--use` will amount to on this fleet**~~ — **built** (ADR-0036, session
  twenty-nine). ADR-0032's third flag, missed because on `offload run` it is a *refusal* rather than
  a note: `--use email` where nothing offers a mailbox stops the submission, so a rule with it fires
  and has every occurrence refused — 13 firings, 0 runs, thirty seconds, under `Nothing else to do:
  the next event fires it`. Warned rather than refused, through the same `missing_resources` the
  refusal uses. The pair that walk set out to test came up clean: a rule's occurrence reaching a
  resource on a peer projects the proxy, named by service, for a run nobody submitted.
- Cost and token accounting per run, per account
- Sandboxing (containers/namespaces per run)
- Web dashboard
- Human-in-the-loop approvals routed to whichever device you're holding

---

## Open questions, answered

Kept for the reasoning; the live ones are in `docs/ROADMAP.md`.

1. ~~**What happens to a permission prompt when the run migrates?**~~ Settled and built by
   ADR-0017. The question leaves through the delivery plane and `offload approve` answers it
   from any device; the fencing turned out to need no epoch at all, because a question's
   identity is the agent's own `tool_use_id`, which exists only while that process is blocked
   on that call — so an answer for a leg that has ended matches nothing and is refused by
   having nowhere to go. Patience is bounded by `Run::approval_patience`, and nobody answering
   is a pass-through rather than a denial.

2. ~~**Untracked-file policy.**~~ Answered in phase 2: `.gitignore` first, a built-in
   never-source directory list second, then per-file and total size caps — and everything
   dropped is named in the run's log. Limits are node config (`[checkpoint]`). What is
   still open is whether the built-in list survives contact with real repos.

3. ~~**What happens when no node matches?**~~ Settled by ADR-0014: a submission is accepted
   by a node or refused to the operator's face, because the alternative asks them to notice
   something at the exact moment they were promised they could stop watching. `--queue` opts
   into pending-anyway. Nothing times out — an accepted run that becomes unplaceable is
   reported over the delivery plane, never discarded. The starvation half is closed too: an
   unspecified deadline means *the moment of submission*, so slack is negative age and
   urgency rises with waiting — aging for free, with no aging mechanism (ADR-0013).

4. ~~**Per-account rate limiting across the fleet.**~~ Closed in session twelve. The blocker
   was the fingerprint, which derived from `$USER` and `$HOME` and could never match across
   machines; it hashes the account uuid the agent records in its own settings now, with a
   versioned salt and a pinned known-answer test, because a value compared across nodes is a
   wire format. `WorkPolicy::max_concurrent_account` is the ceiling it made possible.

8. ~~**A device in two fleets over-commits itself.**~~ Built in session twelve: the device
   broker, a SQLite ledger under a per-user runtime path that both instances reconcile from
   their heartbeats, consulted at the **bid** as well as at admission — because consulted only
   when accepting, a node offers to start now, wins, and holds the run on arrival. The stated
   limit that remains is two *users* on one machine, which a device-wide ledger would have to
   be world-writable to solve. The original framing follows.

   ADR-0012 makes multi-fleet membership a
   matter of running one `offloadd` per fleet, each with its own identity and state dir, so
   that nothing is shared and nothing can leak. The cost is that neither instance knows what
   the other has accepted: two fleets each allowed two concurrent runs can leave one laptop
   hosting four, and battery floors and thermal limits have the same blind spot. Leaning: a
   device-local resource broker both instances consult — a file or socket carrying what the
   machine is already committed to — rather than merging the daemons, which would recreate
   every leak the split exists to prevent. (Blocks nothing yet; bites the first time somebody
   joins a second fleet.)

## Phase 7 items that were built

- ~~**Pruning a triggered run's record**~~ — **built** (ADR-0021, session twenty-five). A rule

- **Which account a device spends on** — **built** (ADR-0028, session twenty-seven). `[agent]

- ~~**Retrying a rule's occurrence on the node that hosted it**~~ — **built** (ADR-0030, session

- **The account's own usage limit** — **built** (ADR-0029, session twenty-seven, wire v23). The

- ~~**Absence history that survives a restart**~~ — **built** (ADR-0031, session twenty-seven).

- ~~**A rule says what its flags will amount to on this fleet**~~ — **built** (ADR-0032, session

- ~~**A question says how long there is, and something says when the window shut**~~ — **built**

- ~~**A drain drains a node with no fleet**~~ — **built** (ADR-0034, session twenty-eight).

- ~~**A drain says what it is waiting for, while it waits**~~ — **built** (ADR-0035, session

- ~~**A rule says what `--use` will amount to on this fleet**~~ — **built** (ADR-0036, session
