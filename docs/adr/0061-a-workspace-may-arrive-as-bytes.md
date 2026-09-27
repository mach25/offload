# ADR-0061: A workspace may arrive as bytes, and the agent holding the context chooses them

**Status:** accepted, not built · 2026-09-11 · reopens ADR-0016's rejected *"push the repository
mirror too"* alternative · extends ADR-0003's portability rule · rests on ADR-0011's posture that
a grant is a decision somebody made

*§4 and §6 were amended in session seventy-eight, after the blob plane was measured rather than
assumed, and the fleet's owner resolved the fork that measurement opened: **the archive fits the
cap.** The alternative — growing the blob plane chunking and resumable transfer — was declined for
now and is a separate ADR if it is ever wanted. One open question remains and is marked below.*

## Context

`materialise the worktree` (ADR-0003) rests on an assumption from phase 0 that has never been
revisited: **the workspace is a git repository, and every node can obtain it.** `Portability`
(`core/src/repo.rs`) encodes the only two cases that allows — `Fleetwide`, clone it from origin,
and `NodeLocal`, *only the machine holding that path may host the run*.

Three different situations are refused by that rule and the operator is told the same thing about
all three. Measured on a daemon, one command apart:

```
$ offload run --repo /nowhere/at/all "hi"
  alpha        /nowhere/at/all is a path on another machine

$ offload run --repo ~/client-project "hi"
  alpha        /home/owner/client-project is a path on another machine
```

The second is wrong in the way that matters: that path is on **this** machine, it exists, and the
submission was typed there. The real reason is `WorkspaceError::NotARepo`, and
`NoBid::RepoUnavailable { portability: NodeLocal }` had one sentence for two causes, because
`WorkspaceManager::can_obtain` answered a `bool` and nothing downstream could recover what it had
seen. *(That half was a report defect and was **fixed in session seventy-five**, independently of
this ADR: `RepoReach::{Obtainable, NotHere, NotARepo}`, measured in `WorkspaceManager::reach` and
carried to the refusal. The second line now reads `/home/owner/client-project is here, but it is
not a git repository`. The refusal itself is unchanged — every node still declines — which is
exactly the gap this ADR is about, now stated honestly.)*

### The ordinary workspace is not a repository

Measured on a real workspace this fleet is wanted for (`~/client-project`):

| part | size | git? |
| --- | --- | --- |
| root — docs, runbooks, `CLAUDE.md`, plugin zips, `wp-cli.phar` | ~12 MB | **no** |
| `client-plugins` | 3.6 GB tree, **`.git` 4.5 MB**, 270 tracked files | yes |
| `master-shop` — `public_html` 601 MB, a 457 MB backup tarball, `db`, `conf` | 1.1 GB | **no** |
| whole directory | **6.5 GB** | **no** |

So the root cannot be submitted *at all* today — not migrated, not even started locally. And the
bytes and the meaning are in different places: 27,439 of the plugin tree's entries are already
ignored by git, and the history that matters is 4.5 MB.

### The fact that decides the shape

**The submitter is an agent that already has the context.** The operator is in a session on their
own machine, says *offload this*, and that session knows what the job is, which files it touches,
and what can be re-derived. `offload` is a CLI it drives like any other command.

This is what makes the design small. Deciding which 12 MB of 6.5 GB matter is a judgement no
manifest format will hold and no heuristic will get right — *"this is WordPress, uploads do not
matter, `vendor/` reinstalls, the 457 MB tarball is a backup"* — and the thing that can make it is
already present at exactly the moment it is needed. Offload does not have to be clever about the
directory. It has to accept bytes.

And the judgement is needed **once**, at the one boundary where work leaves the operator's
machine. After that, see §2: the workspace is a git repository like any other and every existing
mechanism applies unchanged.

### On size

ADR-0016 rejected shipping the repository because *"mirrors are hundreds of megabytes"*. That
objection was put to this fleet's owner with the 6.5 GB measurement and withdrawn: moving a lot of
data across their own network is acceptable — there are no data caps on any device here. So size
is not what decides this, that alternative is reopened rather than relitigated, and the ceiling in
§4 is **optional and unset by default**: it is there for a fleet with a metered link or a small
disk, not for this one.

What survives is **cadence**. A checkpoint is taken at every turn boundary (ADR-0004) and pushed
to a peer (ADR-0016 §3), so a design that treats the workspace's bulk as checkpoint content ships
it per turn. That is what §3 is for.

## Decision

**1. A workspace may be sourced from an archive rather than a repository.**

`WorkspaceSpec`'s source gains a form that is *these bytes* — a content-addressed archive carried
on the **existing blob plane** (ADR-0016): hash-verified, fetched by asking, pushed to a replica.
No new transport, no new trust decision; the hash is the authority, as it already is for every
checkpoint.

The submitting agent produces it. `tar` is a tool it already has, so Offload adds a way to submit
such a workspace and nothing more.

**2. The materialised workspace is a git repository, created by the receiving node in its own
worktree.**

If the archive contains a `.git`, it already is one. If it does not, the receiving node runs
`git init` and commits the contents as the base — **under the node's state dir, never in the
submitter's directory.**

This is the load-bearing clause. From turn 1 the run has a base commit, a run branch, and a
history, so *everything that already exists applies with no change*: the bundle of `base..HEAD`,
the patch of uncommitted work, `restore`'s `merge-base` reasoning (ADR-0053), replication,
`here only` durability, and migration to a third node. The agent's judgement is needed at
submission and never again.

**3. Acquisition is separated from checkpointing.**

The archive moves **once per node per run**, when that node takes the work. The per-turn
checkpoint then carries only what changed — which after §2 is the existing bundle and patch, so
this costs nothing new. Without the split, "send the workspace" means sending it every turn.

**4. The agent chooses the contents; the receiving node decides whether to take them.**

Not a hedge — the rule this project already settled. `allowlist.rs:363`: *"a checked-in
`.offload.toml` is content, and content should not be able to grant."* An agent-chosen payload is
that shape, so the decision belongs to the receiver. It is also structurally free: acceptance is
already a step (ADR-0014), and a node that will not take an archive simply does not bid.

**There is a size ceiling, it is not the owner's, and the archive is therefore a *subset*.**

*(Amended after the measurement in session seventy-eight, and this paragraph previously said the
opposite: "there is no size ceiling by default". That was written without checking the plane it
rests on, and it was wrong — the reasoning about knobs nobody can turn was sound and applied to
the wrong ceiling.)*

`offload_proto::cluster::MAX_BLOB_BYTES` is **512 MiB**, enforced on both the push and the fetch
side, and it is not a policy knob: a peer's announced `size` is an allocation request, so a blob
plane without a cap is a node that can be asked to allocate anything. `~/client-project` is
6.5 GB — **thirteen times over** — so the whole directory was never going to travel, whatever any
owner said about data caps. Measured: 256 MiB moves in 6.36 s over loopback and costs ~300 MB of
RSS **at each end**, because `Blobs::get` hands back a `Vec<u8>`; an interrupted transfer leaves
nothing and resumes from nothing.

The fleet's owner was given that measurement and the two ways out — make the archive fit, or grow
the blob plane chunking and resumable transfer — and **chose to make the archive fit.** So:

- **The archive is a subset the agent selects to fit under the cap, not a snapshot of the
  directory.** This is not a retreat from the decision; it is the decision's own argument applied
  one step further. §*The fact that decides the shape* already says the judgement worth having is
  *"which 12 MB of 6.5 GB matter"*, and the measurement in §*The ordinary workspace is not a
  repository* answers it: ~12 MB of root documents and 4.5 MB of history against a 3.6 GB tree and
  a 457 MB backup tarball. An agent that has to fit under 512 MiB is being asked to do the thing
  this ADR exists to let it do. A `tar` of everything would have been the lazy path, and it is the
  one the cap forecloses.
- **The cap has to be discoverable before the expensive step.** An agent that builds a 6.5 GB
  archive and is refused afterwards has spent minutes and a disk to learn a constant. Offload must
  say the number *before* the bytes are made, not only in the refusal after.
- **`max_archive_bytes: Option<u64>` survives, and only tightens.** `None` now means *the
  structural cap and no opinion beyond it*, never *no limit*. A fleet with a metered link or a
  small disk may set something lower; nothing may set something higher, because the ceiling above
  it is the protocol's rather than the owner's. `max_concurrent_account`'s precedent still holds
  for what `None` means — no opinion — it simply is not an opinion about whether a ceiling exists.
- **Raising the structural cap is a separate ADR and must not be smuggled in under this one.** It
  means framing a blob in pieces, a resumable transfer and a streaming store API: a wire version,
  not a constant.

**`allow_metered` is not that knob and still applies.** It exists already, it is a decision an
owner has already made about that device (ADR-0045), and a path that quietly bypassed it would
pull a whole workspace down somebody's cellular connection. Checked with `accepts_replica`'s
posture rather than admission's — holding somebody's bytes is not hosting their run.

**A ceiling that is set and is hit is reported with the number**, which is `UntrackedPolicy`'s
existing rule and the reason `Selection::summary` needs three reasons rather than two. Silence
here would send somebody to look for a network fault.

**5. The bid carries facts about the job; the transfer happens on acceptance.**

No per-part negotiation and no discovery sub-round — ADR-0050 keeps it to one round per run per
node. The facts a node bids against are the ones it can already reason about: the run's
`Constraint`s, which the submitting agent composes, plus the archive's size and digest. Acceptance
is already a distinct step with the operator still present (ADR-0014), and that is where the bytes
move.

**6. Four refusals stop sharing one sentence.** `NoBid` distinguishes *the path is not here*, *the
path is here and is not a repository*, *the archive is larger than **this node** will take*, and
*the archive is larger than **any** node will take* — because the only question anybody asks is
*why did that not happen*, and those last two send somebody to opposite places. A node-local
ceiling is somebody's policy: try another node, or change it. The structural cap is every node's:
there is no other node, and the archive has to be made smaller. Reporting both as *too large*
would be `NoBid::RepoUnavailable`'s own defect repeated — one sentence for two causes, which is
what session seventy-five had to unpick.

The structural one is also the only refusal here that can be known **before** a bid: a size
against a constant, needing no peer, which is what makes §4's *discoverable before the expensive
step* implementable rather than aspirational.

## Why not the alternatives

**A manifest of parts, each declaring how it is obtained (`Git` / `Copied` / `Provisioned`).** The
first draft of this ADR, and it was Offload trying to be clever about a directory only the agent
understands. Rejected once the premise above was stated: it invents a format to encode a judgement
that is already being made, one round earlier, by something better at it.

**`Provisioned { command }` — the receiver reproduces a part by running `composer install`.**
Dropped with the manifest. It makes placement depend on a toolchain, so the fleet's phone stops
being eligible for work it could otherwise host, and the agent can either put what it needs in the
archive or run setup in its own first turn — where a failure is visible in `offload logs` instead
of inside placement.

**Peer-served `git upload-pack`, so a cold node can fetch a repository it has never seen.** Two
drafts wanted this and it is not needed: if history matters, `.git` goes in the archive, measured
at 4.5 MB for the repo in question. One mechanism covers the non-repo directory, the local-only
repo, and the `Fleetwide` repo whose origin the receiving device cannot actually reach.

**`git init` the submitter's own directory.** Rejected on the measurement: `client-plugins` is a
nested repository, which git records as a gitlink with no objects, so the 270 tracked files of the
part being developed would not travel. It also writes into somebody's working directory, which is
not a trade this project makes. §2 avoids both by initialising the *materialised copy* instead.

**Keep requiring a repository.** Honest, and it refuses the ordinary case outright.

**Sync results back into the submitter's directory.** Deliberately out of scope; see below.

## Consequences

Good:

- **The ordinary workspace becomes submittable**, which is the case that does not work at all
  today.
- **A run on an unreachable repository can migrate**, which is the headline sentence — *close the
  laptop, the agent keeps working on the desktop* — for a local-only or VPN-only repo.
- **Nothing new on the wire**, and that now has a bound on it: the archive is a blob on a plane
  that is built and measured (1.3–47.3 ms to 11.5 MB, 30 of 30 landed; 6.36 s for 256 MiB), and it
  holds **only up to `MAX_BLOB_BYTES`**. Under the cap this clause is true as written. Over it
  there is no wire at all, which is why §4 is a subset rather than a snapshot.
- **A fleet with no data caps configures nothing.** The only gate that fires out of the box is the
  `allow_metered` one an owner already set — and the structural cap, which is not configuration.
- **The agent's judgement is needed once.** Every migration after the first is ordinary Offload,
  with no agent in the loop and nothing new to go wrong.

Bad, and worth being clear-eyed about:

- **What the receiver gets is a *subset*, and that is worse than a snapshot.** A snapshot is
  merely stale; a subset is missing things, chosen by an agent under a 512 MiB budget, and the
  failure it produces is a run dying on the receiver for want of a file that exists on the
  operator's disk. The mitigation is the same as the one below — cheap and legible, `offload logs`
  names what was missing — but the *frequency* is now a function of how well the agent budgeted,
  not of whether it thought about a file at all. **The acquisition report naming what travelled
  therefore stops being a nicety and becomes the thing that makes a failure diagnosable.**
- **Work does not sync back.** What is done there returns as commits and a patch on Offload's run
  branch, not as files appearing in `~/client-project`. Nothing syncs back, and the operator has
  to be told that rather than discover it.
- **The base commit is synthetic.** A `git init`'d workspace shares no history with the operator's
  own repository, so results come back as patches to apply rather than commits to merge. Fine, and
  not what somebody used to `git pull` will expect.
- **The agent can choose wrong.** The failure is a run dying on the receiver for want of a file.
  The honest mitigation is that it is cheap and legible — `offload logs` names what was missing —
  not that it is prevented.
- **Secrets travel because the agent put them there.** The measured workspace has
  `master-shop/conf` and a `config/` holding credentials, and a `.env` under the size cap already
  travels today inside a checkpoint's untracked files. The fleet is one person's own devices
  (ADR-0012) rather than a trust boundary, so this widens an existing exposure rather than opening
  a new one — and it must be **said**: the acquisition report names what travelled. A default
  exclude list is deliberately not proposed, because a name list is a guess about somebody else's
  directory and that guess losing is already a pitfall entry.
- **The archive is opaque to deduplication.** A one-byte change means a new digest and a fresh
  transfer. Acquisition is once per node, so this bites on re-submission rather than per turn.
- **Disk, per node**, and a large archive makes acquisition slow and visible: the run is neither
  here nor there for minutes, which needs a state an operator can see or it reads as a hang.

## What is not decided

- **Whether results ever return to the submitter's directory.** One-way here, deliberately. A run
  whose output the operator wants *in place* is a real want and a separate decision. **Still
  open** — and sharper now that the payload is a subset: an operator who sent 12 MB of a 6.5 GB
  directory is *more* likely to want the result back in place, not less, because the receiving
  worktree is nobody's working copy.
- ~~**Whether acquisition is resumable.**~~ **Measured in session seventy-eight, and the answer is
  worse than the question assumed.** Three facts, all of them on two daemons:

  1. **The motivating workspace cannot be sent at all.** `offload_proto::cluster::MAX_BLOB_BYTES`
     is **512 MiB**, enforced on both the push and the fetch side. `~/client-project` is 6.5 GB
     — **13× the cap** — so an archive of it is refused before any question of resumability
     arises. The cap exists for a good reason (a peer's `size` field is an allocation request),
     so raising it is not a one-line change.
  2. **It is not resumable, and nothing partial survives.** Measured by killing the receiver
     mid-transfer of a 256 MiB bundle: the receiving node held **363 bytes** afterwards — the
     small transcript blob from the previous turn — and nothing of the transfer. That is correct
     rather than sloppy: `recv_bytes` fills one `Vec<u8>`, and the store writes temp-then-rename,
     so there is no partial file to resume *from*. Resuming would need a protocol that frames a
     blob in pieces, which is a wire change and a new decision.
  3. **A blob costs its whole size in memory, at both ends.** Peak RSS moving a 256 MiB bundle
     between two daemons: **308 MB on the sender, 298 MB on the receiver**, from a 33 MB
     baseline. `Blobs::get` returns `Vec<u8>` and `store` takes one, and the trait's own doc
     comment already says it: *"If something ever puts a repository in here, this is the
     signature to change first."* At 6.5 GB this is fatal on a phone and unpleasant on a laptop.

  Throughput for scale: 256 MiB in **6.36 s** over loopback, about 40 MiB/s. 6.5 GB would be
  ~2.7 minutes over loopback and considerably worse over wifi, with the run neither here nor there
  throughout — which is the "needs a state an operator can see" consequence above, now with a
  number on it.

  **So the blob plane could not carry this decision as it was written**, which was a prerequisite
  rather than a detail: §*Consequences* said "nothing new on the wire", and that is true only under
  512 MiB. Two ways out were put to the fleet's owner — the archive stays under the cap, or the
  blob plane grows chunking, a resumable transfer and a streaming store API. **The owner chose the
  cap**, so §4 is amended to a subset and the alternative is left as a separate ADR for whoever
  ever needs a workspace that genuinely cannot be cut down. It must not be smuggled in under this
  one: it is a wire version, not a constant.

  One defect was found on the way and is fixed: a checkpoint whose blob is over the cap was
  reported as *"checkpoint is on this node only; nobody would take a copy"*, which reads as *no
  peer was available* — a transient condition — while the real reason went to a `tracing::debug!`
  inside `offload-cluster`. It now says what it measured and that no retry will change it.
- **Whether an archive workspace can be refreshed mid-run** — the operator changes a file on their
  laptop and wants the running job to see it. Deliberately out of scope: it reintroduces the
  cadence problem §3 exists to avoid.
