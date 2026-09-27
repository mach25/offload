# ADR-0008: `AcceptEdits` is the default; `Ask` is refused until something can answer it

**Status:** accepted · 2026-07-25 · revisit when the approval channel exists (phase 5)
**Amended:** the per-tool allowlist listed below as "deferred" now exists — see
`offload_core::allowlist` and `.offload.toml`. The run-your-own-tests hole is closed
without granting general execution.
**Amended again, 2026-08-21:** part 2 — the blanket refusal of `PermissionMode::Ask` — is
**lifted for a run that can answer for it**. See the amendment at the end. Parts 1 and 3 stand
unchanged, and so does the refusal for every run without `--ask`.

## Context

Agents ask permission before doing things. Claude Code's permission modes are `manual`,
`acceptEdits`, `bypassPermissions`, and friends — and in an interactive session the human
sitting there answers the prompt.

Offload runs agents **headless**, under `--print`. There is nobody sitting there. A run
configured to ask does not block waiting for an answer; it records permission denials and
carries on degraded, or fails. Discovered empirically while building the adapter: the
`result` event carries a `permission_denials` array, and headless `manual` mode is how it
gets populated.

So the shipped default decides what an agent can do on someone's machine, unattended, with
nobody watching. Three bad options and no obviously good one:

- **Ask** — safe, and *useless headless*: the agent gets denied and produces nothing.
- **Bypass everything** — useful, and hands unattended arbitrary code execution to a model,
  on a fleet that includes the user's laptop.
- **Refuse to run headless at all** — safe and honest, but there is no product left.

## Decision

**Three parts.**

1. **`PermissionMode::AcceptEdits` is the product default**, set explicitly on the CLI so
   it is visible rather than implied. The agent may edit files without asking; anything
   else — running commands, fetching, pushing — still gates and, headless, is denied.

2. **`PermissionMode::Ask` is refused at submit time**, with an error that says why and
   points at the approval-channel work. Accepting a configuration we know cannot function
   and letting it fail 40 turns later is worse than refusing it in the first second.

3. **`PermissionMode::Full` requires opting in per run**, never per node and never by
   config default. It maps to `bypassPermissions` and the CLI flag is named to match what
   it does.

What makes `AcceptEdits` defensible here — and it would not be defensible in a user's real
checkout — is that **the worktree is already an isolation boundary**. Every run gets a
disposable worktree on a dedicated branch (see `offload-workspace`); teardown discards it,
and nothing is ever pushed automatically. Unrestricted editing inside a scratch directory
that starts as a copy and ends in the bin is a much smaller grant than it sounds.

`PermissionMode::default()` in `offload-core` stays `Ask`. The *type's* default should be
the conservative one — it is a security control, and a struct built by a future caller who
forgot to set it should not silently get edit rights. The product default lives at the
product edge, where it is written down.

Denials are recorded on the run and shown by `offload ps`. "The agent was denied twelve
times" has to be visible; silently degraded output that looks like a bad model is the
worst outcome of this whole design.

## Consequences

Good: the common runs work unattended — write code, refactor, draft, review. The blast
radius of the default is one throwaway directory. The impossible configuration fails in
the first second with a legible reason instead of after an hour. And nothing here needs
revisiting when approvals arrive; `Ask` simply stops being refused.

Bad, and this is the real cost:

- ~~**Runs that need to run their own tests do not work under the default.**~~ *Closed by
  the allowlist.* A repo declares `allow = ["Bash(cargo test:*)"]` in `.offload.toml` and
  the loop — write code, run tests, fix — works under `AcceptEdits`. Verified end to end.
  What remains true: anything the allowlist does not name is still denied, so an
  unanticipated command still needs `Full`.
- **`Full` is a real risk and the flag name is doing a lot of work.** An agent with
  `bypassPermissions` can run anything the daemon can, including reaching outside its
  worktree. Sandboxing is phase 7; until then this is a trust decision, not a technical
  control, and the docs should not imply otherwise.
- **Denials are visible only after the fact.** The user finds out the run was hobbled when
  they read the result, not while it happens.

## Alternatives

**Default to `Full`.** Everything works; the demo is better. Rejected — a default that
grants unattended arbitrary execution across someone's device fleet is not something to
arrive at by convenience, and a default is exactly the setting nobody revisits.

**Default to `Ask` and let runs fail.** Maximally conservative, and it is what
`PermissionMode::default()` still does at the type level. Rejected as the *product*
default because a system whose default configuration cannot complete a task isn't
conservative, it's broken — and the failure is confusing rather than instructive.

**Per-tool allowlists** (`--allowedTools "Bash(cargo test:*)"`). ~~Deferred.~~ **Adopted** —
and the "per-repo and per-task" observation that made it look premature turned out to be
the design: grants layer from node config, the repo's own `.offload.toml`, and per-run
`--allow`, so nothing fleet-wide has to be guessed. The one thing that needed care is that
a repo-supplied allowlist is *content*, so what it may grant is capped.

**Route prompts to whichever device the user is holding.** The actual solution, and the
reason this ADR is scoped as an interim: prompts become Offload-level events surfaced to
any connected client, so a phone can approve a laptop's run. Roadmap question #1. This ADR
exists so phase 1 can ship without pretending that work is done.

## Amendment, 2026-08-21: `Ask` is refused only while nothing can answer

This ADR always said the refusal was conditional — "nothing here needs revisiting when
approvals arrive; `Ask` simply stops being refused" — and ADR-0017 built the approval channel
without being able to collect on that, for one reason it recorded honestly: the hook's tool list
was `Bash` and `WebFetch`, and under `Ask` the agent gates *edits* too. Lifting the refusal then
would have produced the worst configuration in the design — a run that can be asked about a
command and is silently denied every `Write`.

So the fix was not the refusal, it was the list. **What is worth asking about is a function of the
permission mode**, because the mode is exactly what decides whether the agent gates something:

- Under `AcceptEdits` and `Full` the agent allows edits by itself, so matching `Write` would stop
  a run to ask about something nobody was ever going to be asked. ADR-0004's line, crossed from
  the noisy side.
- Under `Ask` the agent gates every edit, so *not* matching `Write` is the same line crossed from
  the silent side — and silence is worse, because the run looks like it is working.

A constant could only ever be wrong in one of those two directions. `ask_tools(mode)` is right in
both, and the refusal now reads `permission == Ask && !ask.enabled()`: refused for exactly the
runs where the original sentence is still true.

**And the question that widening forced.** ADR-0017 declined to extend the list because doing so
means answering "how many questions is a person willing to answer for one run" — a real design
question, since under `Ask` an ordinary coding run puts twenty or thirty of them on somebody's
phone and the twentieth is not read. The answer is a **budget**: `--ask=N`, default 20, counted on
the run rather than on a leg of it so a migration continues spending what the operator agreed to.
Running out is not a refusal — the run carries on with its calls decided by the agent's own rules,
which is what nobody-answering does and what every run did before the channel existed. That the
failure mode of running out is the behaviour we already shipped is what makes a budget safe.

Verified against a real agent under `--permission ask --ask=2`: the first two `Write` calls were
put to a person and approved, the files were written, the third was refused by the budget, the run
logged `that was question 2 of 2` once, and the agent reported the block in its own words and
finished — recorded as an ordinary permission denial, exactly as this ADR requires it to be
visible.
