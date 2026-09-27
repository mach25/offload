# ADR-0080: A node advertises the models its agent reports, and the fleet can ask again

**Status:** accepted · 2026-09-27 · session ninety-four · wire v37

## Context

The app's model picker was a free-text box. The owner asked whether it could list the models that
are offered. Every node already advertised `AgentDetails::models`, but the list was a constant in
`offload-probe`, `CLAUDE_ASSUMED_MODELS`, whose own comment said "declared, not verified". It had
also gone stale: it said `claude-fable-5` when the current Fable was 5.1. Listing it would have
offered models that might fail on every host, the over-claim `capacity-policy-and-probing` warns
against.

Claude Code can say what it offers. The `initialize` request of its streaming-JSON control
protocol, which the Agent SDK's `supportedModels()` uses, answers with the account's model list:
a value to pass to `--model`, a display name, a description and the model the value resolves to.
Measured on `claude 2.1.283`: about 2 s, a clean exit when stdin closes, no prompt, and no session
transcript. The answer was the same with the network cut (`unshare -rn`), so the list is part of
each Claude Code version.

The owner's design: read the list on each node at start, and let a person ask the fleet to read
it again, "when a new model has been released". No model chosen means Claude Code's own default.

## Decision

1. **A node's models are what its agent reports.** `offload-agent` sends `initialize` to the
   binary a run would spawn, with the environment a run would get (`child_env`: the nominated
   config directory, auto-memory off), because the list is per account and per version. The
   constant is gone. A node that cannot read the list advertises **no** models, never a guess.
2. **`AgentDetails::models` carries names**, as `Vec<Model>` in the agent's own order:
   `value` (what `--model` takes), `name`, `about`, and `resolves_to`. `HasModel` matches either
   the value or the resolved model. The type changed on the wire, so this is **wire v37**.
3. **When it is read:** at startup; whenever the probe sees the agent's version change, since a
   new model arrives with a new Claude Code; and when somebody asks. A failed re-read keeps the
   previous list if the version is unchanged, because the fact it described has not changed. A
   failure after a version change leaves the list empty.
4. **Asking the fleet** is a counter on `Gossip`, `models_asked`, merged by maximum. Anybody may
   raise it (`offload models --refresh`, or the app's Refresh). A node re-reads on every rise it hears
   after its first gossip exchange. A count heard in that first exchange describes requests made
   before the node started, which its startup read answers, so it is adopted without a read.
   (The first cut said "the first *value* it learns", which skipped a real request: a node that
   restarted learned 0 silently, and the next Refresh raised it to 1, the first value it learned.
   Found by walking the rule before pressing the button.) The node that asked reads
   straight away, without waiting to hear its own request back. The counter has no owner, and it
   needs none: raising it is the only operation, and every node orders two values the same way.
5. **`reprobe` stays the one writer** of capabilities (ADR-0048). The model reader keeps its list
   beside the probe and nudges `reprobe`, which folds it into the capabilities in
   `deliver::capabilities`.
6. **In the app** the picker lists the models offered by devices that can take agent runs, as
   programs are listed. "Default" sends no `--model`, so Claude Code uses its own default. "Other"
   keeps free text for a name no device lists yet.

## Consequences

- An agent with no such request (`AgentKind::Other`) advertises no models, which `HasModel` reads
  as unable. That is the honest answer, and no such agent exists yet.
- The window after startup: a request made after a node started but before its first gossip
  exchange is treated as covered by the startup read. It is a few seconds, and a second Refresh
  closes it.
- Reading spawns the agent for about 2 s, and it writes the housekeeping any Claude Code start
  writes in its config directory (`policy-limits.json`, `.claude.json` and its backup). It does not
  write a session.
