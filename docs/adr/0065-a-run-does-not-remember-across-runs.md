# ADR-0065: A run does not remember across runs — the agent's auto-memory is off

**Status:** accepted · 2026-09-24 · built and measured in the session that decided it (ninety) ·
closes ADR-0064's auto-memory residual · extends the argument of ADR-0011's `--strict-mcp-config`
from what a run can *reach* to what it *knows* · amends no ADR

## Context

Claude Code keeps an **auto-memory**: notes the agent writes itself, loaded into every later
session's context. ADR-0064's first amendment found it writing under `--print`; its second found
the directory created under the **mirror's** slug rather than the worktree's, observed only as
empty directories. Claude Code's own documentation settles the key: *"The `<project>` path is
derived from the git repository, so all worktrees and subdirectories within the same repo share
one auto memory directory"*, and *"Auto memory is machine-local."*

Every run is a worktree of one per-repository bare mirror (`offload-workspace`), so on one node
**every run of a repository shares one memory, and no other node has it.** Measured on `claude
2.1.281` with Haiku, on the `~/.claude-alt` login ($0.093), a bare mirror with two worktrees:

| Leg | Where | Asked | Answer |
| --- | --- | --- | --- |
| 1 | worktree A | save to your memory that the codeword is HERON-3 | saved, under `projects/<mirror slug>/memory/` |
| 2 | worktree **B**, fresh session | the codeword, from what you know, no tools | **`HERON-3`**, 1 turn, no tool call |
| 3 | worktree B, `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` | the same | `UNKNOWN` |
| 4 | worktree A, the variable set | save EGRET-9 to your memory | wrote **`memory.md` in the worktree**; no memory directory created |

Leg 2 is a run answering from another run's conversation. That is three problems at once:

- **Placement changes the answer.** A run behaves differently depending on which node won the bid
  and what ran there before — exactly the argument that made `--strict-mcp-config` unconditional
  (ADR-0011): a run's reach must not be an accident of placement, and nor must its knowledge.
- **Migration loses it.** The memory is outside the worktree, the checkpoint and the transcript, so
  a run resumed on another node has forgotten what it would have remembered on the first.
- **Runs leak into each other.** Two concurrent runs of one repository on one node read each
  other's notes, and a rule-fired run's notes steer the operator's next one — a channel nobody
  granted and no report shows.

## Decision

**Every spawned agent runs with `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`**, unconditionally. It is
applied **last** in `offload_agent::claude::child_env`, after the request's own environment, so
nothing a resource or a caller passes turns it back on; `CLAUDE_CONFIG_DIR` stays overridable, as
it was. Not configurable, for the reason `--strict-mcp-config` is not: there is no setting under
which what a run knows should depend on where it landed.

What a run *should* carry forward already has a home that travels:

- **Within a run**, the conversation — a resume continues it, and the transcript migrates.
- **Between runs**, a continuation (ADR-0064) hands the new run the parent's transcript as a file
  and its work as the base.
- **Anything the agent wants kept**, a file in the worktree: leg 4 is the agent doing exactly that
  unprompted once memory is off. It is committed or captured, migrates, and shows in the diff.

## Considered

- **Point `autoMemoryDirectory` into the run** (per-run memory that migrates). Declined: `.offload/`
  is excluded from git (ADR-0064 §3), so the capture would not carry it without a new path through
  the checkpoint; and a memory scoped to one run holds nothing the run's own conversation does not.
- **Leave it on and document it.** Declined: the leak is silent, and the one person who could notice
  it is reading an answer that looks right.

## Consequences

- The owner's own sessions at their keyboard are untouched: the variable is set on the child only.
  Memory an owner already built up for a repository is no longer loaded into runs of it — that is
  the point, and `CLAUDE.md` in the repository is the durable, reviewable form of the same thing.
- A resume walk that "remembers" something should now be trusted: ADR-0064's first amendment had
  to delete a memory folder to get a clean result, and that confound is gone.
- Residual: Claude Code may grow other machine-local state that crosses runs. The pitfall entry in
  `docs/pitfalls/agent-adapter.md` says how this one was found — ask a *second* worktree — which is
  the test to repeat after an agent upgrade.
