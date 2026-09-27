---
name: delegate
description: How to hand work to the Sonnet subagents in this repo (implementer, scout) and — the part that matters — how to verify what comes back before it reaches main. Use when a task is large enough to split, when several independent searches or bounded changes could run at once, or when about to call the Agent tool here at all.
---

# Delegating in this repository

Two subagents are defined in `.claude/agents/`, both pinned to Sonnet:

- **`implementer`** — a bounded, already-decided code change, proven with the four commands.
- **`scout`** — read-only investigation, returning `file:line` citations.

You stay on Opus and you verify. The split is not "hard work for me, easy work for them" — it is
**work whose result I can check without redoing it** versus work whose only proof is the
subagent's own prose.

## The one test

> Can I confirm this is right by running something or reading a diff?

If yes, delegate it. If the only available proof is the subagent telling me it went well, do it
myself. That is the whole rule, and everything below is it applied.

### Delegable

- A mechanical refactor with a named target: rename, extract, thread a parameter through, split a
  module. The diff is the proof.
- A bug fix where the mechanism is already understood and the failing test already exists or is
  easy to state.
- Added cases on an existing test harness — the property suites, the storm fixture, a unit module.
- Doc and index sync: a new row in the `CLAUDE.md` pitfalls table, ADR numbering, a
  `docs/COMMANDS.md` entry for a command that now exists.
- Wide read-only sweeps: every caller of a thing, whether a doc claim still matches the code,
  which ADR covers a subject. Fan several of these out at once.

### Not delegable

- **Anything whose evidence is a walk.** Two daemons, a real or fake agent, output read by eye.
  `CLAUDE.md` is blunt about this: nearly everything in `docs/pitfalls/` was found by walking it
  and *none* of it by reasoning about the code. A subagent's report that a walk looked fine is a
  claim, and checking a claim about a walk means re-walking — which costs more than delegating
  saved. Walk it yourself.
- **Anything that writes `docs/HANDOFF.md`, `docs/adr/`, `docs/sessions.md`, or
  `docs/pitfalls/`.** Those record a judgment about what the rule *is*. Delegating that is
  delegating the reasoning the repository exists to preserve.
- **The load-bearing decisions**: epoch and fence ordering, who owns a gossiped field and what
  arbitrates it, what a `Refused` reason should say, whether a guard is on the right side of an
  `await`. These are exactly where a plausible-looking wrong answer is cheapest to produce and
  most expensive to catch.
- **Commits.** The message is where the reasoning lives (`CONTRIBUTING.md`), and the reasoning is
  mine.

## Writing the brief

A Sonnet subagent is only as good as its boundary. Every `implementer` brief carries five things:

1. **The pitfall file to read first** — name it explicitly, by path. Do not rely on the agent
   finding the right row in the index table.
2. **The files in scope**, by path. Say what is out of scope if it is nearby.
3. **The change**, stated as an outcome, not a procedure — and any ADR it must not contradict.
4. **The passing bar**: the four commands, plus the specific property run if `offload-core`
   domain logic is involved (`PROPTEST_CASES=20000 cargo test -p offload-core --test properties`).
5. **What to report verbatim** — the tail of each command, not a summary.

Fan out independent work in a single message so it runs concurrently.

## In place or in a worktree

`isolation` is an argument to each `Agent` call, not a property of the agent — so this is decided
per dispatch, and it should be. The default is in place, sharing this working tree.

**Use a worktree (`isolation: "worktree"`) when:**

- **Two or more agents will write at once.** They share one checkout otherwise, so their edits
  interleave into a diff that cannot be attributed, and their `cargo` runs serialize on the
  target-dir lock — which is the parallelism, gone.
- **This tree already has unrelated changes in it.** The gate says read the whole diff; a diff
  mixing the agent's work with yours cannot be read that way, and separating them afterwards is
  worse than isolating first.
- **The change is speculative** — a shape being tried rather than a decision being carried out.
  An unchanged worktree is cleaned up on its own, so a throwaway costs nothing to throw away.
- **The point is a before-and-after.** Session forty used `git stash` for this when a fix spanned
  crates; a worktree is the same measurement without putting the current tree at risk.

**Stay in place when** one agent is working, this tree is clean, and the result is meant to be
read and committed straight away. That is the common case, and it is the cheap one: no merge
step, `git diff` is already the answer.

**The cost that decides most of it**: a fresh worktree gets a fresh `target/`, so the first
`cargo test --workspace` there is a cold build of the whole workspace rather than a warm one.
For a single bounded change that is likely to dominate the work itself. Sharing `CARGO_TARGET_DIR`
between worktrees does not rescue it — cargo takes a lock, so concurrent runs wait on each other
instead, which is the contention the worktree was for. This is asserted, not measured; measure it
before leaning on worktrees for routine work.

## The verification gate

Non-negotiable, and it is the reason this arrangement is safe. A subagent's report is a claim.
`CLAUDE.md`'s "measure rather than argue" applies to subagent reports exactly as it applies to
doc comments. It does not get cheaper because the work ran in parallel: three agents mean three
diffs read in full and three sets of commands re-run here. Parallelism moves the bottleneck onto
verification, so fan out only as wide as I am willing to check.

1. **Re-run the four commands yourself.** Every time. A pasted-green tail is evidence the agent
   ran something, not evidence the tree is clean — and the tree it reports on is the one I am
   about to commit.

   ```bash
   cargo build --workspace
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all
   ```

2. **Read the whole diff** — `git diff` and `git status`, not the agent's summary of it. Look for
   files touched outside the brief, and for a fix that widened into a redesign.

3. **Check the rule myself for the path that was touched.** Open the pitfall file and read the
   diff against it. The subagent was told to; whether it did is what I am checking.

4. **Check the three holes the tooling does not close:**
   - `unwrap`/`expect` inside a `#[cfg(test)]` module in a lib — clippy is exempt there.
   - a clock reference reaching `offload-core` in a form `no_clock.rs` does not pattern-match.
   - an edited migration, or a new one whose digest was not added.

5. **Revert the fix and watch its test go red** when the change was a bug fix. `CONTRIBUTING.md`
   calls this the step that has caught the most self-deception, and a subagent has no more
   defence against a test that passes for the wrong reason than I do.

6. **Never accept a claim about daemon behaviour.** If the deliverable needed a walk, either the
   subagent captured raw output to a file that I read myself, or the result does not count.

## When it comes back wrong

Fix it myself if the fix is smaller than re-briefing. Re-brief — via `SendMessage` to the same
agent, so its context survives — if the boundary was wrong rather than the work. Do not iterate
more than twice: a brief that needs a third attempt is a task that was not delegable, and the
honest move is to take it back.

## Escalating off Sonnet

Omit the `model` override and an agent inherits the parent model. Do that when the work is
delegable by the test above but genuinely subtle — a refactor across the fencing paths, say. The
model choice does not change the verification gate; nothing does.
