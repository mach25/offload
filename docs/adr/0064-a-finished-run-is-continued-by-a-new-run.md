# ADR-0064: A finished run is continued by a new run — handed its work, its last word, and its transcript to read if it needs to

**Status:** accepted, built (session eighty-nine) · 2026-09-22 · phase 10 · leaves `RunState::may_resume_later` exactly as it is · reuses
the migration path's fork (`offload-agent/src/claude.rs:530`) · pairs with ADR-0063 for where the
continuation runs

*Written at the owner's request in session eighty-eight and accepted by them the same day. The four facts
§*Consequences* said must be measured were measured the same session — see the amendment at the
end, which corrects one of them. The decision survives all four.*

## Context

The owner, on why Claude Code's Remote Control is not enough: *"I have to have a context open to
use it. I can get an email from any customer or need to check on a project … It would be better if
I could spawn a Claude Code instance in the right context to do some work with the right tools
already there and don't have a context that is full already."* And then, while this was being
written: *"Or perhaps the continuation doesn't actually need the whole context but rather a summary
that can be injected into a new context."*

Offload already dispatches a fresh session anywhere (`phone-as-a-member.md` §*It is not Claude
Code's Remote Control*). What it cannot do is the second message. **A dispatched session answers
once**:

- **A finished run cannot be resumed.** `offload resume` refuses it — *"run has already finished"*
  (`offload-core/src/run.rs:953`) — because `may_resume_later` answers `false` for `Completed` and
  `Cancelled` (`run.rs:810`). Those are decisions, and that refusal is right.
- **Its conversation does not outlive it for long elsewhere.** Nothing references a finished run's
  checkpoint — `referenced_blobs` asks `resumable_checkpoint()` (`offload-store/src/runs.rs:521`) —
  so its transcript and bundle are collected after `GRACE_MS`, **one hour** (`offload-node/src/main.rs:533`).
  What survives is on the node that ran it: the agent's own transcript in its config directory, the
  run branch in that node's mirror, and the worktree, which `cleanup` never removes by itself.
- **The mechanism for carrying a conversation somewhere new already exists.** Migration restores
  the transcript under the new worktree's project slug (`offload-agent/src/transcript.rs:60`) and
  resumes with `--resume <id> --fork-session` (`claude.rs:530`), so a still-alive previous holder
  cannot interleave writes into the same file.
- **The agent's own last word is already an event.** A `result` event carries `result:
  Option<String>` (`offload-agent/src/event.rs:71`) — under `--print`, the final assistant message.

So a continuation needs three things decided: what it *is* (it cannot be a reopened run), what it
*starts from* (the work, and how much of the conversation), and where that is when the run that
did the work has finished.

## Decision

### 1. A continuation is a new run with a parent, never the old run reopened

`RunSpec.parent: Option<RunId>` — immutable, set at submission, owned by the submitter the way
`Origin` is (ADR-0024), so it gossips cleanly. The parent's record, epoch and lease are closed facts
and stay closed: reopening a `Completed` run would re-arm every fence this tree has built around a
terminal state, for a want that is actually *new work that starts where the old work stopped*.

Which runs may be continued:

| Parent is | `offload continue` | Why |
| --- | --- | --- |
| `Completed` | yes | the case this exists for |
| `Cancelled` | yes | *I stopped it — now do this instead* |
| `Failed` | **refused, naming `offload resume`** | a failed run reopens (ADR-0013), and continuing it as well would put two lines of work on one base — one resumed by the recovery tick, one continued by a person |
| non-terminal | refused | it is still somebody's run |
| a task (ADR-0019) | refused | no conversation and no workspace, so nothing to continue from |

Two continuations of one parent are legal and are two runs on two branches from one base — a fork,
the thing `--fork-session` already is, not double execution of one run.

### 2. The workspace starts from where the parent's work actually ended, read and never written

The base is the parent's run branch **plus whatever its worktree held uncommitted at the end** — a
finished agent's last edits are often not committed, and a continuation that silently dropped them
would be `offload rm`'s old *"the run's branch and commits are kept"* defect in a new place.

It is built **on the node holding the parent's worktree**, as the checkpoint machinery already
builds one: a bundle of `base..HEAD` and a patch including untracked files. It is **read-only on
the parent's checkout** — nothing is committed into it, because that worktree is what a person may
still be reading — and it becomes the continuation's starting point exactly as a migrated run's
checkpoint is. *A continuation is a migration whose conversation is replaced*; every materialising
path (`Restorable`, `restore`'s `--is-ancestor`, ADR-0053) applies with nothing new.

**So the parent's node must be reachable when the continuation is submitted.** Not when it runs —
the base is blobs from then on and travels like any checkpoint. A parent whose node is gone is
refused at the keyboard with that sentence, or held with `--queue`. Retaining finished runs'
workspaces fleet-wide to avoid this is §*Residuals*, not this decision.

### 3. A fresh session, handed the parent's last word — with the parent's whole transcript beside it, to read if it needs to

*Rewritten the same session, at the owner's word: the summary should not be all there is — "perhaps
the transcript can be saved so that if a continued workload wants to read it it can."* That turns
the choice this section first offered (a summary **or** the whole conversation) into both at once,
at the cost of neither: the context starts empty, and the history is one file read away.

**The default — `handoff`.** A new agent session, empty context window, whose prompt is the new
prompt followed by one fixed heading carrying:

- the parent's original prompt;
- the parent's final message (`ResultEvent.result`), **verbatim**, capped at 16 KiB with a note if
  cut, labelled as *the previous run's own closing words*;
- one sentence saying where the parent's **full transcript** is, and that it is there to consult
  when the summary is not enough — not to read first.

And the workspace from §2, which is where the real state is: the diff and the git log say what was
done more exactly than any summary. **Offload writes no summary and parses nothing.** It moves one
string the agent wrote and one file the agent wrote, and the agent decides whether to open it.

**The transcript is a file in the workspace, excluded from git.** `.offload/parent/transcript.jsonl`,
the agent's own file **as it wrote it** — not rendered, not trimmed, because rendering it is
reading model output and any cut is a guess about what the next run will need. Excluded through the
mirror's `info/exclude` (`.offload/`, Offload's own namespace), so:

- it is **never committed** onto the continuation's branch, and never reaches the owner's repository
  through a patch;
- the checkpoint's untracked capture skips it — `ls-files --others --exclude-standard`
  (`offload-workspace/src/checkpoint.rs:89`) honours `info/exclude` — so it is not re-shipped at
  every turn boundary, which is ADR-0061 §3's cadence rule applied to a second bulky file;
- instead it is a **blob named by the continuation's spec**, materialised into the worktree by
  whichever node holds the continuation — at the first start *and* after a migration — exactly as
  an archive workspace is.

**Naming it in the spec is what solves retention, and only for the runs that need it.** A finished
run's transcript is collected after the hour's grace (Context) because nothing references it. A
continuation that names it **is** a reference, so `referenced_blobs` keeps it for as long as the
continuation exists, and every other finished run is collected exactly as before. No new retention
policy, no storage that grows with every run ever finished — the byte cost is paid by the runs
somebody chose to continue.

**Where the file comes from**, at submission on the parent's node (§2 already requires it to be
reachable): the parent's last checkpoint blob when it is still held, else the agent's own transcript
file in that node's config directory. When **neither** exists, the continuation still starts — the
summary and the workspace are the default's substance — and its first log row says, in words, *"the
previous run's transcript could not be found here; only its closing message and its workspace were
handed over."*

**Why this is the default**, beyond it being what the owner asked for:

- **It is the stated goal** — a context that is not full already — without giving anything up.
- **It is portable, and a resumed transcript is not** — the owner's word for it. A prompt and a
  file run on any node that can run the agent at all: after an agent upgrade (ADR-0003's
  resume-format concern does not arise for a fresh session), under a different account's config
  directory, with a different agent kind. A transcript *resumed* is one agent's private format,
  restorable only by that agent at a compatible version, which is why migration carries
  `MinAgentVersion`; a transcript *read* is text, and any agent that can read a file can read it.
- **The expensive part is lazy.** A continuation that needs one detail from turn 14 greps for it;
  one that does not never pays for it.

**`--session` — the parent's conversation resumed, forked.** Kept for the case the file does not
serve: the reasoning *as live context*, not as something to look up — *"you were halfway through
understanding the checkout bug; carry on"*. The migration path verbatim: restore under the
continuation's worktree, `--resume <parent session> --fork-session` with the new prompt. Same
sources as the file; **if neither exists it is refused, never quietly downgraded to `handoff`** —
unlike the default, this mode was asked for by name.

**Refused: Offload summarising the parent with a model call.** A summariser run Offload orchestrates
to produce context for another run is Offload managing conversation state (CLAUDE.md: *"if we find
ourselves parsing model output or managing conversation state, we have taken a wrong turn"*). With
the transcript on disk, the continuation can summarise it for itself if it wants to.

**Refused: rendering the transcript into Markdown for readability.** It would be Offload parsing
the agent's private format into a second one, and the first thing it would drop is what nobody
predicted the next run would want. `offload-agent` reads the transcript already, for usage
numbers only, and that is where it should stay.

**Refused: letting the agent compact on resume.** `/compact` is the agent's business, and
`--session` gets whatever the agent does with a long conversation.

### 4. The continuation inherits the parent's spec, and the person continuing is its submitter

Agent, model, allowlist, resources, permission mode, notify and ADR-0063's `prefer`/`require` are
the parent's unless a flag overrides them; `--allow` adds. What does **not** inherit: `Origin`
(the continuer's — a person continuing a rule-fired run makes an `Operator` run, which is true),
the deadline and `--hold` (the parent's were about the parent), and the budgets (`--max-turns`,
`--ask=N` count afresh — they are this run's). `Grant::Submit` is checked against the continuer's
certificate when it is submitted, as for any run.

### 5. Where it runs is the ordinary bid round

Nothing special. The parent's node holds the base warm, so `workspace_warm` (40) already pulls the
continuation back to it; ADR-0063's `--prefer` overrides that in either direction. The owner's four
placement scenarios apply unchanged: continue on the laptop you are at, on the Mac, from the phone
to anywhere.

### 6. The continuation's log says what it was given

`LogKind::Continued { parent, mode, base, handed_over_bytes }` as its first row — and when the
parent's final message was **empty or absent**, the row says so in words: *"the previous run ended
with no closing message; only its workspace was handed over."* Silence there would read as a
continuation that knew what came before and did not. `offload ps` and `offload logs` show the
parent; `offload continue <run>` on a parent that already has continuations names them, because
the question after *continue the last one* is *which one was last*.

## Why not the alternatives

**Reopen the finished run.** One record per conversation is tidy, and it costs every terminal-state
guarantee: a `Completed` run that can become `Running` again is one the collector, the checkout
sweep, the rule reclaim (ADR-0020 §6) and `describes` all have to reconsider. A new run costs a row.

**`Say { run, text }` into a running agent.** A different want — talking to a run *before* it
finishes — and still open in `phone-as-a-member.md`. A continuation is what you do after; a
`Say` would be delivered at the next turn boundary (ADR-0004) and is not this.

**Keep every finished run's transcript and bundle durable, so a continuation needs nobody.** The
honest cost is storage that grows with every run ever finished, against a want that reaches a few
of them. §3's reference does the same job for exactly the runs that were continued; what it does
not reach is continuing a run whose node is gone, which is deferred until that has actually been
annoying.

**`--session` as the default.** It fills the context the owner wanted empty, and with the transcript
on disk it buys only one thing the default lacks — the reasoning live rather than looked up.

## Consequences

Good:

- A dispatched session becomes a conversation: dispatch from the phone, read the answer, continue
  it — each continuation a fresh context on whichever machine suits, starting from the work.
- No new mechanism for moving work: the base is a checkpoint and `--session` is a migration.
- No conversation state in Offload: one string moves, under a heading, and the agent does the rest.

Bad, and to be clear-eyed about:

- **Wire bump**: `RunSpec.parent`, the mode, the parent-transcript blob reference, and
  `LogKind::Continued`. Stored runs decode with
  `parent = None` — a `#[serde(default)]` field, for `LogKind::Finished`'s stated reason — and the
  compatibility fixture is hand-written (`storage-and-encoding`).
- **A handoff is only as good as the parent's last message.** Under `--print` it is usually a
  summary and sometimes *"Done."* §6 makes the thin case loud; it does not make it rich. Measure
  first: the closing messages of the runs already in a store, by length.
- **The transcript travels with every continuation**, and it holds whatever the parent's tools
  printed — a `.env` read in turn 3 is in it. Not a new exposure: checkpoint transcripts already
  replicate to peers. But it is now also a *file in a workspace*, so the acquisition report names
  it, as ADR-0061 requires of anything that travelled.
- **Size is the parent's**: a long run's transcript is megabytes (the session that wrote this ADR, for
  scale, was 1.7 MB). Under `MAX_BLOB_BYTES` by two orders of magnitude, and paid
  once per node per continuation, not per turn.
- **The parent's node must be up at submission.** From a phone, about a run the laptop did, with
  the laptop shut, the answer is `--queue` or wait. Stated rather than engineered away.
- **Unverified, and must be before building**: that the mirror's `info/exclude` is what a
  *worktree's* `ls-files --exclude-standard` reads (git keeps it in the common directory, which
  should make it apply to every worktree of the mirror — walk it); that `ResultEvent.result` is persisted in the run's
  event log rather than only streamed; that the agent's own transcript file on the parent's node
  outlives the checkpoint blob (Claude Code's own retention, not ours); and that `--fork-session`
  on a transcript restored under a *different* project slug behaves as it does for migration — the
  migration walks say yes for a running conversation, and nobody has tried one that finished.
- **Chains accumulate**: each continuation is a record and a worktree, and the parent's worktree
  stays until `offload rm`. Visible in `offload ps`; nothing prunes it, as for any run.

## Residuals — named, not built

- Durable retention of finished runs' bases or transcripts, so the parent's node need not be up.
- A chain's older transcripts: a continuation of a continuation gets its **parent's** file, not
  every ancestor's. The parent's own transcript records whether it read the grandparent's, which
  is enough until somebody measures that it is not.
- A standing instruction asking every run to close with a handoff note — it would change every
  run's prompt, and should be measured against §*Consequences*' first measurement before anyone
  decides it is needed.
- `Say`, for a run that has not finished.
- Idempotent continuation: a phone that retries a submission makes two continuations, exactly as
  `offload run` makes two runs today.

## Amendment, 2026-09-22: the four checks, measured

Run the same session, at the owner's request. Git 2.55.0, Claude Code 2.1.280, Haiku 4.5, on the
`~/.claude-alt` login; four agent calls, **$0.119** in all. Every scratch directory and agent
project folder the checks made was removed afterwards.

**1. `info/exclude` reaches a worktree — holds.** A bare mirror, `git worktree add`, `.offload/` in
the **mirror's** `info/exclude`, and in the worktree `.offload/parent/transcript.jsonl` beside an
ordinary new file. `rev-parse` puts the worktree's git dir under `mirror.git/worktrees/wt` and its
common dir at `mirror.git`, and all three readers skipped the transcript and listed the new file:
`ls-files --others --exclude-standard` (what the checkpoint capture runs), `status --porcelain=v1
--untracked-files=all` (what `holds_uncommitted` and the reclaim doors run), and `add -A --dry-run`.
§3 stands as written.

**2. The closing message is not persisted — corrects §3's source.** `Outcome.result` reaches the
supervisor and is dropped: `AgentEvent::Finished` becomes `LogKind::Finished { success, turns,
denials, cost_micro_usd, tier }` (`offload-node/src/supervisor.rs:5019`), with no text. What *is*
stored is every assistant text block, as `LogKind::Text` (`supervisor.rs:4993`), so the closing
words are recoverable today as the last `Text` rows before `finished`. **Building this adds
`result: Option<String>` to `LogKind::Finished`**, capped, `#[serde(default)]` — the field's own
doc comment already gives the reason a defaulted field is the safe shape — rather than having the
handoff infer "last" from row order.

And what those messages hold, measured on the only real-agent runs left on this machine: none of
the 372 `state.db` files here holds one (all are test fixtures), so the agent's own transcripts were
read instead — 1,027 under `~/.claude-alt/projects/*worktrees*`, of which **20** were written by a
real model (19 Haiku, 1 Opus; the rest by the fake agent). Closing message length: **median 13.5
characters**, 11 of 20 under 40 (`DONE`, `ok`), 1 `No response requested.`, maximum 895. These were
walk prompts that *ask* for a terse answer, so the sample is biased toward exactly this — and it
still settles the point that matters: **a closing message is not a handoff on its own.** It is a
pointer; the transcript file (§3) and the workspace (§2) are the substance. That makes the §3
rewrite load-bearing rather than a nicety, and it strengthens the residual *a standing instruction
asking every run to close with a handoff note* from "measure first" to "worth doing when this is
built".

**3. The agent's own transcript outlives Offload's copy — holds, with a number.** Claude Code's
documented default is `cleanupPeriodDays` **30**, and what it deletes includes session transcripts
([claude-directory](https://code.claude.com/docs/en/claude-directory.md)). This profile does not
set it. Its oldest transcript is 28.9 days old across 1,101, which is about the profile's own age, so
the cleanup itself was not observed. So §3's second source exists for up to 30 days after the run,
at a setting that is the owner's and not Offload's — against Offload's one hour.

**4. A finished session resumes, forked, from another directory — holds; and the first attempt was
confounded.** Leg 1 in directory A: `--session-id`, prompt *remember PELICAN-7, reply OK*, finished
(`OK`, $0.022). Its transcript was then **moved** to directory B's project folder and A's deleted.
Leg 2 in B: `--resume <id> --fork-session`, *what was the codeword?* → `PELICAN-7`, a new session
id, $0.029 — **but not from the conversation**: leg 1 had written an auto-memory file,
`<config>/projects/<A>/memory/codeword.md`, and leg 2, remembering from the resumed conversation that
it had done so, **read that file by its absolute path**. With the memory folder deleted and a fresh
fork of the same session, leg 2 answered `PELICAN-7` in **1 turn with no tool call** ($0.026).
That is the clean result.

Two further facts from the same check:

- **The default mode works as §3 describes.** Leg 3, a fresh session in a third directory with the
  parent's transcript at `.offload/parent/transcript.jsonl` and only `OK` as the closing message,
  answered `PELICAN-7` in 2 turns — its one tool call a `Read` of that file ($0.042). Dearer than
  the fork for a one-line conversation, which is expected: it read the whole file. The saving is
  on long parents, where a fork carries every token and the file costs only what is read.
- **`--resume <id>` searches every project folder on the machine**, not only the current one
  ([sessions](https://code.claude.com/docs/en/sessions.md#resume-a-session)). The adapter's
  restore-under-the-new-slug is therefore belt and braces on one machine; it is still what makes a
  transcript *present* on a node that never ran the parent, which is the case that matters here.

**New, and not the continuation's alone: the agent writes auto-memory that Offload does not
carry.** Asked to *remember* something, the agent saves it under `<config>/projects/<cwd slug>/memory/`
even under `--print`, keyed by the worktree's path. That is node-local state outside the worktree,
outside the checkpoint, and outside `--strict-mcp-config`'s reach. A resumed or migrated run can
remember, from its conversation, a memory file that is not on the node it now runs on. A
continuation in `handoff` mode is unaffected (a fresh session, and the file is in the transcript it
can read). Recorded in `docs/pitfalls/agent-adapter.md`; whether spawns should turn memory off is a
separate decision.

## Amendment, 2026-09-24: built — five things building it decided

Session eighty-nine. Walked on two daemons with a fake agent, then with Haiku on the
`~/.claude-alt` login for the demo ($0.13 in all; the agent project folders were removed after).

1. **The base is a checkpoint at turn zero.** `build_continuation` captures the parent's worktree
   with the ordinary `capture` — which touches only the index, and reverts it — into a
   `Checkpoint { turns: 0, … }` on the new run. No real capture is at turn zero, so
   `Run::continuation_base` tells the parent's work from the continuation's own first capture
   without a flag, and from that first capture on the run is ordinary: a migration resumes its
   *own* conversation. Walked: continued on alpha, placed on bravo by `--prefer node=bravo`,
   rebuilt there with the parent's commit **and** its uncommitted file, the parent's checkout left
   as it was.
2. **It is typed on the node that finished the parent**, and refused elsewhere — *"its last leg did
   not run on this node … continue it from the node that finished it"*. §2 said the parent's node
   must be *reachable*; building the base for a request from another node needs a new peer
   request, and is a residual. The test for *finished here* is a terminal row in this node's own
   log, since a worktree from an earlier leg would otherwise be captured as if current.
3. **The agent's own transcript file comes first**, the checkpoint's copy second — the reverse of
   §3's order. The file is the whole conversation; the last checkpoint stops at the last boundary
   it was taken at.
4. **The handoff heading is composed at spawn**, from `Continuation::{parent_prompt,
   closing_message}`, so the spec's prompt stays what the continuer typed. The first cut folded the
   heading into the prompt and `offload ps` printed `continue: … --- ## Continuing an earlier…`.
5. **A raw transcript cannot be *read*, only searched.** Measured with Haiku: one line of a
   183 KB two-turn transcript is 38k tokens, over the `Read` tool's 25k cap. The continuation that
   needed a fact only the transcript held (the parent's `date +%N` output, in no prompt, closing
   message or file) opened it, was refused, grepped, and ran out of the five turns it was given.
   The heading now says what the file is — JSON Lines, some lines far too long to read whole,
   search it — and the re-run answered `784133523`, correct, in 3 turns ($0.044). A control
   continuation that did not need it called only `Write`. That is the demo, and §3's premise held:
   the file was opened exactly when it was needed.

Residuals added: building the base for a request typed on another node; and the agent's
auto-memory,
which Claude Code created (empty) under the **mirror's** slug rather than the worktree's, which
suggests it is keyed per repository on a node — observed as empty directories only. *(Closed by
ADR-0065, session ninety: measured as a leak between runs of one repository, and turned off for
every spawn.)*

A handoff continuation that fails **before its first capture** holds only its base, which has no
session — so `resume`, and the recovery tick through it, refused it for want of one. It is started
again from the base now, exactly as its first start was. Walked: a fake agent that died at once,
`offload resume`, completed with the transcript file in place.


## Amendment, 2026-09-24: typed anywhere — the base travels, the run does not

Session ninety closed the residual of the amendment above: **`offload continue` works on any node.**
Wire v31 (`ClusterMessage::ContinueBase` and its two answers).

1. **Only the base is built on the parent's node.** `Supervisor::build_continuation` is split in
   two: `continuation_base` — the state checks, the capture, the transcript, the closing message —
   runs where the parent's last leg ran, and `continuation_from` makes the run where it was typed.
   Forwarding the whole request was the smaller change and the wrong one: the continuation's home,
   and `--prefer here`, would have been the machine that held the parent's checkout instead of the
   one the person is at. Routed by `RunProgress::by`, as `offload logs` is.
2. **The asker fetches every blob the base names before the run exists.** The building node's
   collector owes those blobs nothing — no run there names them until gossip says so — so a base
   left only there could be collected under a run that depends on it.
3. **The parent's state is checked where the base is built**, not by the asker: a gossiped copy
   can say `running` a second after the run completed there.
4. **Refusals name the machine.** A peer's reasons are worded with its fleet name and returned
   bare; the first cut printed *"run … cannot be continued: bravo answered: run … cannot be
   continued: its worktree is no longer on this node"* at alpha's keyboard.
5. **A parent whose node the view calls `dead` or `departed` is refused without dialling.** The
   dial does not fail; it times out, and the walk waited 30 s to be told what `offload nodes`
   already said.

Walked on two daemons with the fake agent: a parent run on bravo, continued from alpha and pinned
to alpha — the parent's commit and its uncommitted file arrived, the transcript file was in place,
and bravo's checkout was untouched; the same with `--session`, which resumed bravo's session on a
machine that never ran it; refused by name after `offload rm` on bravo; refused in 8 ms with bravo
killed. The `--session` refusal for a parent with no transcript, which the amendment above had not
walked, was walked on one daemon: refused with the sentence naming the way out, and the handoff
continuation beside it logged that the transcript could not be found.
