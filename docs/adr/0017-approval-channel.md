# ADR-0017: A permission prompt is a hook that blocks, a notification that leaves, and an answer that is fenced

**Status:** accepted · 2026-08-20, after building and running it · the reply direction ADR-0010
left open · **amended 2026-08-21**: the tool list follows the permission mode, and ADR-0008's
refusal of `PermissionMode::Ask` is lifted for a run that can answer for it

## Context

ADR-0008 refuses `PermissionMode::Ask` at submit time, and says why: headless, there is nobody to
answer, so the agent is denied and produces nothing. Its own conclusion names the fix — "route
prompts to whichever device the user is holding" — and files it as roadmap #1.

Everything that fix needs now exists except the fix. The delivery plane carries a notification off
the node that logged it and onto a device that can reach a person, including a device that hosts
nothing (ADR-0010, both halves built). Runs have epochs, leases and an event log. A run's news can
name the route it wants. What is missing is the direction that comes *back*: a question the agent
is blocked on, and an answer that reaches it.

The shape of that has been open because the agent's own half was unknown. An agent that cannot be
made to *wait* cannot be asked anything — the prompt would arrive after the tool call had already
been denied, and an approval channel that answers questions nobody is still asking is theatre.

So the first thing this ADR does is record what was measured, on `claude 2.1.237`, because
ADR-0004's rule is that we speak the agent's own protocol rather than invent one:

- **A `PreToolUse` hook runs headless and blocks the tool call while it thinks.** Declared in a
  settings block (`--settings <json>`), matched per tool, given a `timeout` in seconds.
- **It can turn a denial into an execution.** `dd if=/dev/zero of=… bs=1 count=1` under
  `--permission-mode acceptEdits` is denied headless and recorded in `permission_denials`. With a
  hook that slept three seconds and then answered `{"hookSpecificOutput": {"permissionDecision":
  "allow", …}}`, the same command ran, and the result reported **no** denials.
- **A hook that does not answer fails closed.** Killed at its `timeout`, the stream reports
  `hook_response` with `outcome: "cancelled"`, `exit_code: 1`, no output — and the tool call is
  denied exactly as if no hook existed. Nothing was written. *This is the property the whole
  design rests on*, and it is measured rather than assumed: a channel whose failure mode was
  "allow" would be a remote-execution grant with a network timeout for a policy.
- **The hook's stdin carries what both halves need**: `session_id`, `cwd`, `tool_name`,
  `tool_input` and `tool_use_id`. The first identifies the run, the last identifies the *request*.
- **`--include-hook-events` puts `hook_started` and `hook_response` in the stream-json output**, so
  the daemon sees the ask and its outcome in the event stream it already parses.

One measurement is worth keeping for whoever probes this next: `echo hello` runs under every
permission mode, hook or no hook, because trivial commands are allowed by the agent's own
judgment. A probe built on `echo` measures nothing. `--permission-mode manual` also reports
itself as `default` in the `init` event, so what the flag was set to cannot be read back from
there.

## Decision

**A prompt is a blocked hook, a notification, and a fenced answer.** Three mechanisms this
project already has, in that order.

### 1. The ask

A run whose spec says it may ask (`offload run --ask`) spawns with a settings block declaring a
`PreToolUse` hook: a program on the holding node, invoked with the run id in its environment. The hook is **not** a
new protocol — it reads the agent's payload on stdin, asks the daemon over the control socket it
is already sitting next to, and prints the agent's own answer shape.

The hook and the agent are always on the same machine, which is what makes this simple: the
question travels by unix socket to a daemon that is already the run's holder, and nothing about
the ask crosses the network unauthenticated.

### 2. The question leaves

The daemon writes the pending prompt into the run's event log — `LogKind::Asked { tool, summary,
tool_use_id }` — and the projection in `offload_core::notify` grows one variant,
`Notice::NeedsDecision`. From there it is the existing plane: an outbox row per route in the run's
audience, at-least-once, deduplicated on `(run, seq, sink)`, carried by a peer where the
credential lives on a peer.

That is the whole reason this is cheap now. A prompt is news of a particular kind, and news
already leaves the fleet.

### 3. The answer comes back, fenced

An answer is `(run, epoch, tool_use_id, decision)` and every part of that is load-bearing:

- **`epoch`** — a run that has migrated does not accept an approval addressed to the leg it was
  on. Same rule as every other side-effecting path.
- **`tool_use_id`** — the agent's own identity for the request. An approval is for *one tool
  call*, so it cannot be replayed against the next one, and "yes" cannot be broadened by
  arriving twice.
- **`decision`** — allow or deny, with a reason that reaches the agent, so a denial is
  informative rather than a wall.

An answer typed on another node is **forwarded to the holder**, the way a `SpecEdit` is forwarded
to a field's owner: the blocked process is on the holder, and an answer applied where it was typed
is an answer nobody is waiting for.

### 4. Nobody answering ends the wait, and the wait is bounded

The tool call is in flight, so the run is **mid-turn** — which ADR-0004 says is not a safe
checkpoint. A prompt therefore cannot be allowed to hold a run open indefinitely: while it waits,
the run cannot be checkpointed, cannot be drained, and its lease is being renewed for a machine
that is doing nothing.

So patience is bounded, and the bound is a **fixed default of minutes**, shortened by a *stated*
deadline and never below a floor. Not derived from the deadline alone: an unspecified deadline
means the moment of submission (ADR-0013), so a rule that read slack directly would give every
ordinary run zero patience and deny every prompt instantly — the trap's fifth appearance, and the
one place it would be indistinguishable from the feature not working.

When patience runs out the daemon grants nothing and says so, and — this is the part that makes
the design safe to ship — **what happens next is exactly what happens today**. The agent's own
rules decide: a gated command is refused and recorded as a denial, and the run carries on degraded.
The failure mode of the whole channel is the current behaviour, which is what makes it something
that can be built incrementally rather than all at once. (The first draft of this section said the
daemon answers *deny*, which is subtly worse and is corrected in the amendment: an explicit denial
would refuse calls the agent would have allowed.)

### 5. What it does not do

- **It does not make `Full` unnecessary.** A run that needs to do something twenty times still
  needs a grant, not twenty questions. The allowlist and `Ask` are complements: name what you
  know, be asked about what you did not anticipate.
- **It does not answer on the human's behalf.** No "allow anything like this" that outlives the
  run, no learned policy. A remembered approval is a grant, and grants are written down where
  somebody can read them (node config, `.offload.toml`, `--allow`).
- **It does not become a second scheduler.** Which device is asked is the audience decision
  ADR-0010 already made. A prompt is news; the plane routes news.

## Consequences

Good:

- **`PermissionMode::Ask` stops being refused**, which is the one configuration ADR-0008 had to
  turn away. The most conservative mode becomes the most useful one for work you would not leave
  unattended.
- **A denial gains a reason.** Today a run comes back having been denied twelve times with no
  record of what it wanted. The pending prompt is that record, whether or not anybody answered.
- **The reply path is reusable.** "May I run this" and "which of these two should I do" are the
  same road; ADR-0010 predicted this and it costs nothing extra to keep the answer generic.

Bad, and to be clear-eyed about:

- **A blocked run is a stalled run.** Minutes of a lease spent waiting for a person, mid-turn,
  undrainable. This is a real cost and the reason patience is bounded; it is also why `Ask` must
  not become the default. A fleet full of runs waiting to be asked about is worse than one full of
  runs that were denied and said so.
- **A prompt can arrive after the run stopped caring.** The notification is at-least-once and the
  agent's patience is finite, so somebody will approve something that has already been denied. The
  answer is refused by `tool_use_id`, and the person is told it was too late — but they will see
  it, and it will be annoying.
- **The hook is per-tool matching, and matching is a pattern language.** `ToolPattern::risk`'s
  lesson applies unchanged: a matcher that looks scoped and is not is worse than no matcher.
  Ask about everything by default, narrow deliberately.
- **The settings block is inherited authority in the other direction.** A run's `--settings` is
  *additional* to whatever the host user has configured on that machine, and there is no flag
  that says "only mine" — `--safe-mode` and `--bare` both disable hooks, which is the thing we
  are relying on. So what a run may do without asking still depends partly on the machine that
  won the bid. Same family as the MCP hazard already recorded, and not fixed by this ADR. What
  can probably fix it is a `permissions.deny` list in our own settings, since deny beats allow —
  unverified, so it is written here as the next probe rather than as a decision.

## Alternatives

**An MCP permission-prompt tool** (`--permission-prompt-tool`). The mechanism the SDK documents,
and the flag does not exist in this CLI version's `--help`. It would also mean running an MCP
server per run to answer one question, on top of a config surface this project already treats as
a hazard when it is ambient. The hook is smaller and it is a command the daemon nominates, which
is the same shape a sink already has.

**The SDK's `canUseTool` callback.** Requires embedding the TypeScript or Python SDK, i.e.
reimplementing the harness around the agent instead of shelling out to it. ADR-0004 says no, and
the reason applies precisely here: the permission protocol is the agent's, and a second
implementation of it is a security control with two sources of truth.

**Poll instead of block: let the tool be denied, then retry the turn once approved.** Tempting,
because nothing has to wait. Rejected: a denied tool call is already in the transcript, so
"retry" means either re-prompting the agent to do the thing it was just told it could not do, or
forking the conversation. Both spend turns to simulate the wait, and neither can be made to look
like an approval to the agent.

**Let a prompt wait indefinitely, since the whole point is that people are away.** Rejected on the
mid-turn rule. A run waiting for an answer cannot be checkpointed, so an unanswered prompt on a
laptop that is about to close is a lost turn — and the wait is precisely when a laptop closes. The
promise "you will be told when you pick your phone up" is about a *notification*, which is durable
and can wait for hours; a blocked tool call is not durable and cannot.

**Remember the answer and stop asking.** The obvious ergonomic improvement, and it is a grant with
a friendlier name. Grants belong in the allowlist, where they are visible, layered and capped —
not accumulated invisibly by whoever clicked "always allow" on a phone at midnight.


## Accepted, after building it — and the measurement that shaped it

The channel works end to end against a real agent: a run submitted with `--ask` stops, the question
leaves the node through the delivery plane, `offload approve` answers it, and the agent runs the
command it was blocked on. Five things the building settled, and the first one changed the design.

**A `PreToolUse` hook fires for *every* matching tool call, and the agent does not say whether
permission was needed.** Measured: with `--allowedTools "Bash(dd:*)"` the hook still ran for a call
that grant covers. That one fact rules out the obvious design — "hook everything, ask about
whatever comes through" — because it would have us asking a person about reads and about calls the
operator has already granted. Three consequences, and they are the shape of the feature:

- **A short tool list**, not a matcher-less hook: `Bash` and `WebFetch`. It is the list to argue
  with rather than to extend quietly, because a longer one does not buy coverage — it buys asking
  about things the agent would have allowed by itself, which is the harness second-guessing the
  agent (ADR-0004).
- **A grant suppresses the question.** If the run's own allowlist covers the call
  (`ToolAllowlist::covers`), the daemon does not ask: the operator has already decided, and asking
  again teaches them to stop reading. The safety of having a second matcher at all comes from what
  a match *does* — it means "let the agent apply its own rules", never "allow" — so a matcher that
  is wider than the agent's costs a question, and can never grant something the agent would have
  gated. That is the opposite of the usual risk with a reimplemented pattern language, and it is
  the only reason this is acceptable.
- **Nobody-answered is a pass-through, not a denial.** The hook prints *nothing*, which leaves the
  agent's own rules in charge — a gated command is refused, a harmless one is not. An explicit
  `deny` on timeout would have been a regression: it would refuse calls the agent would have
  allowed. So the failure mode of the whole channel is exactly today's behaviour, which is what
  makes it safe to ship and safe to leave running.

**The three outcomes, verified with a real agent.** *Allowed*: the file was written, and the log
says `allowed (an operator)`. *Denied*: the agent was told "denied by an operator", quoted the
reason back in its own words, wrote nothing, and recorded a `permission_denial`. *Unanswered*: with
a stated deadline a minute out, patience came to 54.9 seconds, after which the hook was told
nothing and the agent reported "This command requires approval" — the pre-existing behaviour,
arrived at deliberately.

**Asking is opt-in per run (`--ask`), and it is a separate axis from the permission mode.** The
mode says what needs permission; this says what happens when something does. Opt-in because the
cost is real and falls on the run: it is stopped mid-turn, so it cannot be checkpointed or drained
while it waits.

**A question nobody could answer is not asked.** If no route in the fleet can reach a person *and*
nobody is streaming the run, the daemon declines instantly with a reason rather than stalling the
run for minutes to reach the same conclusion by clock. It is the delivery plane's knowledge and the
attendance observation (ADR-0013) being used for something they were not built for, and it costs
nothing: both were already there.

**`PermissionMode::Ask` is still refused at submit.** *(Superseded by the second amendment
below — kept because the reasoning is what the fix had to answer.)* Deliberately, and this is the
honest part.
The hook covers commands and fetches; under `Ask` the agent also gates *edits*, which this list
does not cover — so lifting ADR-0008's refusal today would produce a run that can be asked about
`Bash` and is silently denied every `Write`. The valuable half works without it: under the product
default (`AcceptEdits`), a command the run has no grant for used to come back as a denial in the
morning and can now be approved from a phone. Widening to edits means answering "how many
questions is a person willing to answer for one run", which is a real design question and not a
list to extend on the way past.

**One rough edge, recorded rather than fixed.** `offload ps` shows a blocked run as `running` —
true from the state machine's point of view and useless to somebody wondering why nothing is
happening; `offload status` counts them and `offload asks` lists them, which is where the answer
is.

## Amendment, 2026-08-20: the answer travels

The half that made the rest worth having: an answer typed on any device reaches the agent blocked
on another. Without it the question arrived on a phone that could not act on it, which is the
delivery plane's own failure mode — telling somebody something they cannot do anything about — with
an extra step.

**`ClusterMessage::Answer` goes to the holder** (wire v12), the mirror of `EditSpec` going to a
field's owner and for a stricter reason: an edit is a record that could in principle be reconciled
anywhere, while a blocked *process* exists on exactly one machine. The daemon resolves the run,
answers locally when it holds it, and forwards otherwise; a run nobody holds has no agent and
therefore nothing waiting, which is said rather than searched for.

**No epoch on the message, and that is not an omission.** The identity of a question is the agent's
own `tool_use_id`, which exists only while *that* process is blocked on *that* call. A run that
migrated has a new leg, a new agent and new ids, so a stale answer matches nothing and is refused
by having nowhere to go — fencing by construction rather than by a check somebody has to remember
to write. The same property covers the answer that arrives a second after patience ran out.

**`offload asks` canvasses the fleet** rather than reading gossip, for `offload explain`'s reason: a
blocked process stops being blocked the moment somebody answers, so a remembered copy is a question
that has already been dealt with, presented as current. A peer that does not answer contributes
nothing rather than an error row — a question this node cannot see is one it cannot answer either.
The clocks in a `PendingAsk` are read *at the holder*, because a laptop subtracting timestamps
across two machines would be reporting clock skew as urgency.

**Who may answer is settled by the handshake**, not by a new grant. A device in the fleet can
already submit work and be told things; being able to say "yes, run that command" is the same
authority arriving through a different door — and the device this plane exists for, a phone that
hosts nothing, is exactly the one that must be able to. What the message carries beyond the verdict
is the answering node's *name*, so the run's log on the desktop says `allowed (an operator on
phone)`.

**Verified on two daemons.** The phone joined with `{submit, deliver}` and never got `host-runs`;
the desktop has no route of its own. A run on the desktop blocked twice, each question arrived as a
push on the phone, `offload asks` on the phone showed both with `WHERE: desktop`, and
`offload approve` on the phone unblocked the desktop's agent both times. The run finished, the file
was written on the desktop, and the desktop's log records who approved it and from where.

## Amendment, 2026-08-21: the list follows the mode, and the questions have a budget

The half this ADR named and left: `PermissionMode::Ask` refused at submit, because the hook
covered commands and fetches while `Ask` also gates every edit. Two things closed it, and the
second is the one the ADR said was a real design question rather than a list to extend on the way
past.

**The tool list is a function of the permission mode** (`ask_tools`), not a constant. The mode is
what decides whether the agent gates a call, so a fixed list is wrong in one direction or the
other and this one was wrong in both at once: matching `Write` under `AcceptEdits` asks about
something the agent allows by itself, and *not* matching it under `Ask` denies every edit in
silence. Measured on `claude 2.1.238` before it was built, this ADR's own rule: under
`--permission-mode manual` a `Write` is denied headless with no hook and lands in
`permission_denials`; with a `Write` matcher the hook fires carrying that call's `tool_use_id`, an
`allow` writes the file, and the result reports no denials.

**A run has a budget of questions** (`AskPolicy::UpTo { questions }`, `--ask=N`, default 20). That
is the answer to "how many questions is a person willing to answer for one run", and the shape of
it matters more than the number: it bounds the *interruption*, not the run. A run past its budget
stops asking and its remaining calls are decided by the agent's own rules — which is precisely
what nobody-answering already does, and what every run in the fleet did before this channel
existed. **The failure mode of running out is the behaviour we shipped**, which is the same
property that made the pass-through safe, applied a second time.

Three details the building settled. The budget is spent when a question is **put to somebody**,
not when the hook fires: a call an existing grant covers, or one nobody could have been shown,
interrupted no one and must not spend attention that was never asked for. It is counted on
`RunProgress` beside the denials rather than in the holder's registry, so it belongs to the run
and not to a leg of it — a migration continues the count instead of handing the new node a fresh
twenty. And running out is said **once**, latched on the log rather than in memory, for
`LogKind::Overdue`'s reason and with the extra one that the log is the only latch a migrated run
carries with it.

**Verified against a real agent** on one daemon, `--permission ask --ask=2`: two `Write` questions
put to a person and approved, both files written, the third refused by the budget, one
`that was question 2 of 2` line in the log, and the agent reporting the block in its own words and
finishing — with the call recorded as an ordinary permission denial. The whole of what this
amendment exists for is the first line of that: before it, that question was never asked and the
file was never written.

What is left of ADR-0008's refusal is the run that did not ask for a channel: `--permission ask`
without `--ask` is still refused at submit, and the message names `--ask` as the way out.
