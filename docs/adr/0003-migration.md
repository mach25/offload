# ADR-0003: Migration = transcript + workspace bundle, captured cooperatively

**Status:** accepted · 2026-07-25
**Amended:** 2026-08-21 — two of the consequences below hold only while a run has never left the
node it is resuming on, and the untracked-file heuristic defers to what the repository tracks.
See the amendment at the end.

## Context

"Close the laptop, the agent keeps working on the desktop" is the headline feature. The
question is what actually moves.

An agent run's meaningful state is smaller and cleaner than a general process's:

- **Conversation state** — the transcript. Agents already persist this and already support
  resuming from it (Claude Code: session files plus `--resume <session-id>`). We are not
  inventing a mechanism; we are relocating files the agent already wrote.
- **Workspace state** — a git repo at a ref, a branch of commits the agent made, and a dirty
  working tree. Git already has the primitives: `git bundle` for history, a diff for
  uncommitted changes.
- **Run metadata** — cwd, env allowlist, model, turn count, agent version.

Everything else — process memory, open fds, subprocess state — is either reconstructible or
belongs to a turn that is in flight.

## Decision

A checkpoint is `{ transcript_hash, bundle_hash, patch_hash, metadata }`, all
content-addressed with BLAKE3 and stored as blobs. Migration is "fetch these hashes,
materialise the worktree, resume the agent from its session id."

Runs declare `Restartability`:

- **`Resumable`** (default for agent runs) — checkpoint/restore as above. Resumes
  mid-conversation.
- **`Idempotent`** — safe to re-run from the original prompt on a clean workspace. Cheaper, no
  transcript to move; correct for stateless "review this diff" style runs.
- **`Pinned`** — cannot migrate; fails if its node leaves. Named explicitly so it is a choice
  and not a surprise (a run bound to local-only resources, say a device-attached GPU dataset).

Live migration of process state (CRIU and friends) is out of scope.

Ungraceful loss uses the same path minus cooperation: lease expires, run resumes from the last
durable checkpoint. Cadence is one checkpoint per turn by default, so a crash costs at most
one turn.

## Consequences

Good: the mechanism is boring and portable. No kernel features, no privileges, no
architecture matching — an arm64 laptop and an x86_64 VM exchange checkpoints happily, because
a transcript and a git bundle are architecture-neutral. Content addressing gives dedup (the
same repo bundle shared across runs) and integrity for free. Because the transcript is the
agent's own format, resume fidelity is the agent's problem, not ours.

Bad, and worth being clear-eyed about:

- **Uncommitted work is the fragile part.** The dirty patch must include untracked files that
  matter, and "matter" is a heuristic — `.gitignore` exists precisely because some untracked
  files shouldn't move. Build artefacts and `node_modules` must not be bundled; a
  half-written source file must. Getting this list wrong loses exactly the work the user
  cared about.
- **Resume format is not portable across agent versions.** Handled at eligibility (nodes
  advertise agent version; downgrade targets are ineligible), not discovered at resume.
- **The environment is not part of the checkpoint.** A run that worked on the desktop because
  a particular tool was installed will fail on the laptop. This is what constraints are for,
  and constraints are only as good as what the run declared.
- **Checkpoints contain source code and conversation content.** They are as sensitive as the
  repo. Blobs are cluster-internal and transport is encrypted, but there is no per-run access
  control within the fleet.

## Amendment, 2026-08-21: local state is authoritative only while the run never left

Two consequences of this design turned out to be stated one qualifier short, and both were
found by pointing properties at `offload-workspace` — the module this ADR is about.

**"A worktree that survived is never staler than the checkpoint" is true of a run that stayed.**
The resume path used the existence of a checkout as proof of its currency, which is sound while
this node is the only one that has ever held the run. It is not sound once a run *comes back*:
nothing removes a worktree when a run leaves (teardown is manual and terminal-only, for the
reason written in `cleanup`'s own doc — the checkout holds the output), so the earlier leg's
files sit there indefinitely. A run that checkpointed here at turn 1, did nineteen turns
elsewhere and returned was resumed from turn 1, with its bundle, its patch and its transcript
all skipped — the transcript for the same reason at the same path, since a transcript's location
is derived from the worktree's. Silently, and reported as "adopted in place".

What makes it decidable with no clock and no message is a number this design already has: turn
counts belong to the run rather than to a leg, so they continue across a migration. The worktree
records the turn it holds, adoption compares, **unknown is not current**, and a checkout the
fleet has moved past is moved aside rather than deleted — it may hold the only copy of a
mid-turn edit, which is this ADR's own "uncommitted work is the fragile part" applied to the
copy that lost.

**"Build artefacts and `node_modules` must not be bundled" needs the same qualifier: unless the
repository tracks them.** The heuristic was a list of twenty directory names, which is a guess
about somebody else's repository — and measured against the repositories on one laptop, the
guess is wrong often: `build/` holds hand-written Dockerfiles and scripts in an ingress chart,
WordPress ships 132 tracked files under `wp-includes/js/dist/`, and two more commit `vendor/`
and `node_modules/` outright. So a new file the agent wrote in `build/` was dropped for the name
of the directory it was in. The name now loses to what the repository says: a directory git
tracks content in is content, asked per directory so an artefact directory *inside* a tracked
one is still caught. `.gitignore` remains the first filter and the names remain the second; what
changed is that both now defer to the repo's own tracked content.

## Alternatives

**CRIU-based live migration.** Transparent, no agent cooperation. Rejected: Linux-only, needs
privileges, breaks across architectures — and a heterogeneous fleet is the entire premise.
Also does not survive the network connections an agent holds open to its API.

**Restart from the original prompt.** Trivial. Rejected as the default: throwing away an hour
of conversation because a lid closed is the exact failure the project exists to prevent. Kept
as `Idempotent` for runs where it is genuinely fine.

**Re-derive state by replaying the transcript into a fresh agent.** Rejected: it re-executes
tool calls, i.e. re-runs side effects. Strictly worse than moving the file the agent already
wrote.

**Ship the whole workspace directory as a tarball.** Simple and dumb. Rejected as the default
because repos are large and mostly identical to what the target can fetch from git — but it
is a reasonable fallback for non-git workspaces, and phase 3 should leave room for it.
