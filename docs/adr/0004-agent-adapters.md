# ADR-0004: Drive agents as external processes; checkpoint only at turn boundaries

**Status:** accepted · 2026-07-25

## Context

Offload orchestrates coding agents. There is a tempting alternative shape where Offload *is*
the agent — talks to the model API directly, owns the tool loop, owns the conversation — and
therefore has perfect control over pausing, snapshotting and resuming.

That control is real. It is also a trap: it means reimplementing and then perpetually chasing
Claude Code's tool set, permission model, context management, and hooks, forever, badly.

The second question is when a run can be safely interrupted. An agent alternates between
*thinking* (model call) and *acting* (tool calls). Its transcript records completed turns. In
the middle of a tool call, state lives somewhere the transcript cannot see: a file half
written, a subprocess running, an HTTP request in flight, a git operation partway through.

## Decision

**Agents are external processes.** `offload-agent` defines an `Agent` trait — spawn, stream
events, request checkpoint, resume from session — with one adapter per agent kind. The Claude
Code adapter shells out to `claude`, consumes its streaming JSON event output, and resumes via
`--resume <session-id>`. Everything agent-specific (flags, event schema, session file
locations) is confined to that adapter. Nothing above `offload-agent` knows an agent's CLI.

**Checkpoints happen at turn boundaries only.** The adapter observes the event stream and
knows when a turn completes. A checkpoint request sets a flag; capture happens at the next
boundary. There is no mid-turn snapshot, and we do not pretend otherwise.

A drain deadline expiring mid-turn has exactly two honest outcomes, both supported:

- **wait** — extend past the deadline until the turn completes (default; turns are usually
  seconds to low minutes), or
- **abandon the turn** — kill the agent, resume from the previous boundary, re-doing that
  turn's work. Only permitted when the run is marked as tolerating it, because re-doing a turn
  means re-running its tool calls.

## Consequences

Good: Offload stays small and stays current. Claude Code improves and we get it for free.
Adding another agent is one adapter, not a fork of the scheduler. The transcript we checkpoint
is written by the agent itself, so resume fidelity is the agent's problem — a much better
place for it to be. And "checkpoint at turn boundary" turns out to be a natural,
well-defined, observable point, which is more than most systems get.

Bad:

- **We are coupled to an external CLI's interface.** Flags and event schemas change under us.
  Mitigated by the adapter boundary and by pinning agent versions in capabilities, not
  eliminated.
- **Checkpoint latency is unbounded in principle.** A turn that runs a 40-minute test suite
  cannot be checkpointed for 40 minutes. Drains block, or lose the turn. There is no third
  option and we should not invent a fake one.
- **We see the agent as an event stream, not a call graph.** Less introspection than owning
  the loop; partial credit within a turn is invisible.
- **Process supervision is now our problem** — zombies, orphaned subprocesses, stdout
  backpressure, exit codes that mean different things per agent.

## Alternatives

**Build the agent loop into Offload** (direct Messages API, own the tools). Maximum control:
mid-turn interruption becomes possible, checkpointing gets fine-grained. Rejected — it makes
the project "write a coding agent" with orchestration as a side quest, and the resulting agent
would be worse than the one already installed on the machine. Revisit only if turn-boundary
granularity proves genuinely insufficient in practice.

**Agent SDK / library embedding** rather than subprocess. Better structured than parsing a
stream, worse for isolation, and ties the daemon's lifecycle and language runtime to the
agent's. Worth reconsidering per-agent where a good SDK exists; the `Agent` trait deliberately
does not assume subprocess, only that adapters can spawn, stream, and resume.

**Checkpoint on a timer regardless of turn state.** Uniform cadence, no waiting. Rejected: it
produces checkpoints that cannot be resumed correctly, which is worse than no checkpoint —
resuming into a half-applied tool call is how you get duplicated side effects.
