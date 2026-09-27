# Work that arrives without its context: a mailbox, and routing it to what it needs

Status: **exploration.** Nothing here is decided and nothing is built, except where a line says
so. Explored against the tree in session eighty-eight (2026-09-22), from two ideas the owner
captured together and refined in conversation:

- *An IMAP poller*: when a particular sender mails — the motivating one is a contact at the shops
  client, about the shops — read it and do some investigation.
- *Agent-primed context*: the owner designates what counts as "my stuff", and a workload (the
  poller, or any other) routes or discovers the context a job needs. The context for the shops is
  `~/client-project` — ADR-0061's motivating directory: 6.5 GB, not a repository, about 12 MB of
  runbooks and `CLAUDE.md` carrying the meaning.

**Start from where the work should run, not from how to send it away.** The first draft of this
file designed archives, proxies and a separate priming run first, and the owner corrected it: the
job may perfectly well run on the machine it was submitted from, on the one that suits it, or on
the one they will be at later. When it lands beside its context, nothing has to move at all. So
the order below is: where it runs (ADR-0063), then what it reads, then only what happens when the
answer to the first is *somewhere else*.

Nothing below was walked on two daemons. Every claim is from reading the code at the cited line.

## Where the job runs

ADR-0063 (accepted, phase 10) is the placement half, and this use case is one of the four scenarios it was
written for. The rule a mailbox wants is:

```
offload when email --prefer here --use email --use context --allow … "…"
```

`here` resolves **when the rule is written** to the node holding the mailbox trigger, which is the
machine holding the context. So the job prefers the machine where `~/client-project` lives, and a
fired run still goes elsewhere when that machine is off or clearly worse off (§2 of the ADR). ADR-0063
§5 adds the same pull without being asked: a run granted `context` scores up on the node holding it.

The common case is therefore the cheap one: **the mail arrives on the main machine, the
investigation runs on the main machine, and the context is read from disk.** Everything from
*When the job lands somewhere else* on is for the other case.

## The mailbox is a trigger

A `[[triggers]]` entry is a long-lived program whose stdout lines are events; a rule bound to its
service submits a run per line (ADR-0020 §1–2). The split that matters: **the program checks
*who*; the agent decides *whether it is about the shops*.** A filter language inside the
orchestrator was refused twice (ADR-0019, ADR-0020; `operations-fleet.md` §*The filter belongs in
the program*), and "is this about the shops" is a judgement, not a match.

| The idea asks for | Where it lives | State |
| --- | --- | --- |
| Poll / IMAP IDLE | the program's own loop — no interval field, by decision (ADR-0020 §1). IDLE is the ideal shape | fits |
| Match on sender | the program. **Require a DKIM pass for the sender's domain** in the receiving server's `Authentication-Results` header — a `From` line is whatever the sender typed | fits |
| Is it about the shops? | the run's first turn: read the message, stop in one line if not | fits |
| Several accounts | one `[[triggers]]` per mailbox, an instance each (ADR-0011) | fits |
| Credentials | the program's, never gossiped. No credential store exists in this tree (`offload-node/src/config.rs:308`) | fits |
| Read / move / flag the message | the run, through an `email` **resource** (ADR-0011 — built, walked with `mcp__email__read_inbox`) | fits |
| Filters with different actions | every rule bound to a service fires on every line (`offload-node/src/trigger.rs:335`), so different grants need different services, so a trigger process — and an IMAP connection — each | small gap |

A poll-once-and-exit program is a hot loop: an exit is a restart with backoff.

### "Already handled" belongs to the mailbox

A program that marks mail seen when it prints the line loses mail:

- **One occurrence in flight per rule; the rest are dropped and counted** (ADR-0020 §3).
- **The program never learns how the run went**, and ADR-0057 deliberately fires nothing on
  notices about machine-started work.

So the predicate is **state-based** (`operations-fleet.md` §2): *a matching message exists that is
not handled*, re-announced every poll, so a dropped event is announced again. And *handled* is
what the run does to the message:

1. The run's first act sets a keyword (`$OffloadClaimed`); the program skips claimed mail.
2. Its last act moves the message.
3. Claimed and not moved is a run that failed or died — the dead-letter folder, visible in any
   mail client, with nothing in Offload.

A crash between claim and move is the at-most-once side, which is the right side.

### The investigation, and what it may touch

A matched mail is text from outside reaching an agent's prompt (ADR-0020 §4), and the run will hold
read access to hosting and shop APIs. The DKIM check is the cheap real gate; the grants are the
bound. **Read-only**: investigate, write findings, draft a reply for the owner, never send. Anything
that changes a shop goes behind `--ask`. `--notify` carries the findings to wherever the owner is.

Where the investigation's tools come from is worth checking before anything else: the owner's own
shop and hosting skills live in `~/.claude-alt`, and a fired run sees them only if the node's
`[agent] config_dir` nominates that directory (`offload-agent/src/claude.rs:100` sets
`CLAUDE_CONFIG_DIR` from it). Unverified end to end.

## Context: what "my stuff" is, and how a job finds its part

Two kinds, each with a mechanism the tree already has:

| Context kind | Where the bytes come from | Mechanism |
| --- | --- | --- |
| **A folder** (`~/client-project`) | read on its machine; over the proxy from anywhere else | a `[[resources]]` MCP server the owner nominates, service `context` (ADR-0011, proxied calls built) |
| **A remote git repository** | cloned by **the node running the job**, with its own credentials | `RepoSource::Remote` and the shared per-repo bare mirror (`offload-workspace`) — a second job is a fetch, and bidding already asks `WorkspaceManager::reach` |

**The registry lives on one machine and is not gossiped.** The context server's own config is the
list: folders and git URLs. Gossiping it would need an owner, a successor and tombstones
(ADR-0056's cost), and a URL sometimes carries a credential.

**Discovery is a tool on the context server**: `find_project(text)` beside `read` and `search`,
answering with a **pointer** — `folder: client` or `git: <url>@<ref>`. How it decides is the
server's business; the orchestrator learns none of it. **Routing** is the same thing done earlier:
the trigger prints `project=client` and the run starts from it.

**Nothing an event says can widen a grant** (ADR-0020 §4–5; `offload when`'s own help: *"Nothing the
trigger reports can add to [the grants]"*). So routing and discovery choose **within** what the rule
granted: grant `context` as a whole, let discovery narrow it. A spoofed mail can name a project; it
cannot reach a folder nobody designated.

Worth doing to the directory itself, and nothing to do with Offload: whatever of
`~/client-project` can be git — `client-plugins` already is, and the runbooks could be — becomes a
git context and travels on its own; only what genuinely cannot (`master-shop`, the backups) stays a
folder context.

## When the job lands somewhere else

Only then does any of this cost something:

- **A folder context is read over the proxy.** Reading a runbook is fine; grepping a 3.6 GB plugin
  tree through it is unmeasured and probably not. The context server should grep and return
  matches, not files.
- **The context's machine has to be up for those reads.** The run migrates; a run whose context
  holder is gone stalls on its next read — the trade phone-holds-the-mailbox setup already
  makes.
- **A git context discovered mid-run meets placement after placement.** A node that cannot reach
  the repo has already won the bid. With a pointer, the job asks **its own node** to fetch it
  (`offloadd` already serves MCP to a run, for the proxy) and a node that cannot reach it refuses
  with a reason. Named statically on the rule instead, placement can check reach before the job
  lands.
- **A run has exactly one repository.** `WorkspaceSpec.repo` is one string
  (`offload-core/src/run.rs:102`). A job working in one place and *consulting* one or two contexts
  needs read-only checkouts beside its worktree — a new `RunSpec` field, a wire bump, and three
  questions every workspace field has to answer: a checkpoint does not carry a context (re-fetched
  from its ref), migration must re-fetch **the same commit**, and `restore` must know which is which.
- **The run still needs a workspace of its own.** For an investigation, an empty scratch one: the
  optional-workspace gap `operations-fleet.md` §1 raised for a different workload.

### A workspace per mail

The message and its attachments as the run's own workspace — `message.eml` and `attachments/` —
solves the one part of the mailbox that does not fit a line: an event is capped at 4 KiB
(`trigger.rs:37`) and a path on the trigger's node means nothing where the run lands. As an
archive (ADR-0061, `offload run --archive` built) it travels with the run, and its output lands as
commits on that mail's own branch. Two routes:

- **Today**: the program packs the tar and calls `offload run --archive` itself. Works now, and is
  ADR-0020's rejected *"the trigger submits the run itself"*: no drop count, no `offload rules`,
  recorded `Origin::Operator`, and no gate on how many are in flight. Good for trying it.
- **Properly — an ADR**: the daemon hands the program a spool directory; a line names a
  subdirectory; the daemon packs it with fixed metadata and submits the rule's run against it,
  with the `RunId` **derived from the archive's digest** the way schedules derive theirs from the
  tick (`offload-core/src/schedule.rs:171`) — so a mail announced twice is one record, with no
  parsing of the line. It forces a per-subject gate (`operations-fleet.md` §5) and a bound on it.

### The priming run, and why it is the last resort

The owner's first framing: a separate agent that resolves the request and prepares the job. It is
ADR-0061's submitter run unattended — choose a subset, `offload run --archive` it — and it is the
only route here that has a run **submit a run**. Nothing forbids that and nothing records it, and
three things follow (read, not walked):

- **It is recorded as an operator's.** Only rule and schedule firings set `Origin::Rule`
  (`trigger.rs:708`, `schedule.rs:216`, `api.rs:1551`).
- **That walks around ADR-0057's loop guard**, which fires notice rules only for operator work
  (`deliver.rs:764`): mail → rule → primer (`Rule`) → primed run (`Operator`) → …
- **A primer resumed across the submitting turn can submit twice** — double execution. A child id
  derived from *(parent, n)* would converge it.

Context routing (above) makes it unnecessary for this workload: the job discovers its context
itself, wherever it runs. It stays here as the answer for work that must leave with a *snapshot*,
and it needs an ADR before it is safe.

## The same want without a mail: a fresh session, in the right context, from anywhere

The owner's framing, which generalises everything above: *"I really like the Claude Code remote
control feature. The problem with it is that I have to have a context open to use it. I can get an
email from any customer or need to check on a project … It would be better if I could spawn a
Claude Code instance in the right context to do some work with the right tools already there and
don't have a context that is full already."*

`phone-as-a-member.md` §*It is not Claude Code's Remote Control* already draws the line: Remote
Control **attaches** to a session somebody started; Offload **dispatches** into a fleet where
nothing has to be running. This adds the three things a dispatched session needs to be *useful*
the moment it starts, and each maps onto this file:

| Needs | Which is | State |
| --- | --- | --- |
| **The right context** | the project, found by name or by what the request says | context routing, above — `find_project` on the context server. Not built |
| **The right tools already there** | the owner's skills, grants and resources | `[agent] config_dir` for skills (unverified end to end), `--allow`, `--use`. Built; the gap is a *profile* per project so they need not be retyped from a phone |
| **An empty context window** | a fresh agent session | every run is one. Free |
| **The right machine** | here, the Mac, where I will be later | ADR-0063, accepted, phase 10 |
| **Talking to it after** | a conversation, not one prompt | **the gap** |

The last row is the one attach does better, and it is sharper than `phone-as-a-member.md` says:
there is no `Say { run, text }`, and **a finished run cannot be resumed at all** —
`offload resume` refuses it, *"run has already finished"* (`offload-core/src/run.rs:953`). So a
dispatched session answers once, and the follow-up is a new run that has lost the conversation.
The agent can already continue one: Claude Code resumes a session by id, and can fork one, so a
**continuation** — a new run whose session starts from a finished run's transcript and whose
workspace starts from its run branch — is Offload spawning the agent's own resume rather than
reimplementing anything. A new run rather than a reopened one, because a finished run's record,
epoch and lease are closed facts and reopening them is the kind of edit this tree fences against.
Worth an ADR of its own; it is what turns a dispatch into a session. **Accepted as ADR-0064,
phase 10**, and the owner moved it on while it was being written: the default should not be the
whole conversation but *"a summary that can be injected into a new context"*, which is *"more
portable than a resume that loads the entire context"*. So the default is a **handoff** — a fresh
session given the parent's workspace, its original prompt and its own closing message verbatim —
and the forked session is `--session`, on request. Then one step further, again the owner's: the
parent's **whole transcript is saved as a file** in the continuation's workspace (excluded from
git, named by the spec so it is kept), so the continuation starts empty and reads the history only
if it needs to.

A **project profile** is the other half, and it may be nothing more than the context server's
entry for a project carrying its default grants and resources, so `offload run --project client
"…"` from a phone means the same as the rule in *Where the job runs*. Whether a profile may carry
grants at all is the `.offload.toml` question again — content should not be able to grant — so a
profile would live in node config, written by the owner, never in the project.

## The idea doc's open questions, against the tree

| Question | Answer today |
| --- | --- |
| How are folders designated? | The context server's config, on its machine, never gossiped. Not built |
| Does the primer run on the main machine? | There need not be a primer: the job prefers the main machine (ADR-0063) and discovers context itself |
| Ambiguous request — ask or guess? | Guess and show the pick; ADR-0017 questions are permission asks for a `tool_use_id`, not free-form |
| Context cached or rebuilt? | The context server's business |
| How big is too big? | Only matters for a snapshot: `MAX_BLOB_BYTES`, 512 MiB, printed by `offload status` |
| Poll interval / IDLE? | The program's. IDLE fits best |
| Credentials? | The program's, on its node |
| Already handled? | The mailbox: claim keyword, move on success |
| Where do attachments land? | In a per-mail workspace, or fetched by the run through the resource |
| Failed action? | Claimed-and-not-moved is the dead letter |
| Several accounts? | One trigger per mailbox |

## What would be worth doing, in order

1. **Build ADR-0063** (accepted, phase 10) — it is the placement half and has no other prerequisite.
2. **The trigger program and a rule, with no Offload change**: DKIM-checked sender match,
   claim-and-move, read-only grants, `--notify`. Walk it with a mail arriving mid-run and a run
   that fails. Check the config-dir question first.
3. **A minimal context server** (list projects, search, read, `find_project`) over
   `~/client-project`, used locally, then proxied to a run on another node with read and search
   times measured.
4. **Extra read-only checkouts on a run** — measure what a checkpoint, a migration and `restore` do
   with them before designing the field.
5. **The per-mail workspace**, today's route first, the ADR if it earns one.
6. **A continuation** — a new run from a finished one's session and branch — which is what makes a
   dispatched session conversational. An ADR, after checking how Claude Code's own resume and
   fork behave on a transcript that has moved nodes.
7. **Nested submission** stays unbuilt until something needs a snapshot to leave; walk it before
   writing that ADR.

## Open questions

- Should a nested `offload run` be refused until its ADR exists, rather than recorded wrongly?
- Does the context server want to be agent-backed for `find_project`, or is a keyword index over
  the roots enough for one person's projects? Probably the second, and it is the server's choice.
- Does a mailbox trigger want the per-subject gate — one in flight per sender? Measure per-rule
  first.
