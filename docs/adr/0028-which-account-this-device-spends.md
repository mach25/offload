# ADR-0028: The agent account a device spends on is nominated, not inherited

**Status:** accepted · 2026-08-27 · extends ADR-0011's nominated-thing rule · supersedes nothing

## Context

Every fact about the agent that only its owner knows is nominated in `node.toml` — `binary`
(session twenty-one), `max_concurrent` (also twenty-one, and the doc comment there calls it "the
one number about an agent only its owner knows"). One fact was not: **which account it runs as.**

Claude Code's account is selected by its state directory — `$CLAUDE_CONFIG_DIR`, else
`~/.claude` — and that was read from the *environment of the process*, in two places, with no
config field able to say otherwise:

* `offload_probe::agents::claude_config_dir` — decides whether the node claims to be
  authenticated, and derives the `AccountId` every per-account cap is keyed on.
* `offload_agent::transcript::config_dir` — decides where a checkpoint looks for the transcript.

Both are three-line copies of one rule, and CLAUDE.md already names the failure of getting that
rule wrong: a node that assumes `~/.claude` where the owner moved it "either captures nothing all
night… or reports itself unauthenticated and refuses every run".

The consequence of it being *only* environmental is smaller-sounding and worse. A daemon started
from a terminal runs on one login; the same daemon started by a service manager, whose environment
is not the owner's, runs on another. Nothing in the config says which, and nothing in any report
said which either — `offload status` printed `agent claude-code 9.9.9, authenticated` and stopped
there. On a machine with two logins on it, that line is the same for both.

This is the third application of the same shape and the sharpest, because the wrong answer here
**spends the wrong account's money** and shows up on somebody's bill rather than in a log.

## Decision

### 1. `[agent] config_dir` — the owner nominates the state directory

```toml
[agent]
binary     = "claude"
config_dir = "/home/owner/.claude-alt"
```

Unset means the old rule exactly: ask `$CLAUDE_CONFIG_DIR`, fall back to `~/.claude`. Set, it
**wins outright** — not "wins if the variable is unset", which would leave the answer depending on
the environment of a service manager nobody reads.

It reaches **three** places, and has to reach all three, because they answer different questions
about the same directory:

| asks | wants to know |
| --- | --- |
| the probe | is this device authenticated, and as *whom* |
| the capture | where is the transcript to checkpoint |
| the spawn | where will the agent actually look |

Two out of three is the failure that hides: a node authenticated as the account the owner named,
checkpointing nothing all night because the agent wrote its transcript somewhere else.

### 2. The child is *told*, not left to inherit

`offload_agent::transcript::config_dir`'s doc comment argued the opposite, and was right until this
ADR: *"Read from this process's environment on purpose. The agent is spawned as a child and
inherits it, so the two cannot disagree."* That holds only while nothing else can set the path.
The moment an owner can nominate one, inheritance **is** the disagreement — so `spawn` now sets
`CLAUDE_CONFIG_DIR` on the child from the directory the adapter resolved, unconditionally.

It is set *before* `req.env`, so a caller that means to override it still can: that environment is
the owner's word about one resource server (ADR-0011), and this is the adapter's word about its own
state.

The test for it drives a real spawn, because the bug lives in the environment of a process and
nothing short of one has an environment. Watched red against the code it replaces, where it
reported the state directory of the shell that happened to be running the suite.

### 3. `[agent] account` — a guard, not a selector

```toml
[agent]
account = "acct:0123456789abcdef"      # as `offload probe` prints it
```

What *selects* an account is `config_dir`; this says which one the owner meant. If the resolved
account is anything else — a different login, or none this node can identify — the node advertises
the agent as **not authenticated**, with a description naming both:

```
logged in as acct:aaaa…, and this node is configured for acct:bbbb…
```

Three choices inside that, each borrowed from somewhere:

* **Not authenticated rather than not installed.** `authenticated = false` is already the state
  that means "refuses every run, with a sentence saying why". Reporting it as a missing agent
  would send somebody looking for a program that is right there.
* **Not a startup refusal.** A daemon that would not come up cannot be asked what it thinks the
  account is, which is exactly the question somebody has when this fires — and the mismatch is
  reachable by re-authenticating rather than by editing config, so it is a state to report and
  recover from.
* **The fingerprint, not an email or a uuid.** `AccountId` is already the value designed to be
  compared across machines and shown to people, and a config file holding a login's real
  identifier is a small leak with no purpose. It is `acct:` plus sixteen hex characters, which is
  short enough to copy out of `offload probe`.

### 4. The reports say which account, unconditionally

`offload status` gains a line, printed whether or not anything was configured — because "which
account does this machine spend on" is a question about the machine, not about whether somebody
wrote the answer down, and the default is the wrong guess on exactly the machine that has two
logins:

```
account     acct:0123456789abcdef  ·  from /home/owner/.claude-alt
```

`offload probe --config` names the setting alongside `agent.binary`, for the same reason session
twenty-one added that one: a report about the wrong login is indistinguishable from a report about
the right one.

## Consequences

* A device can be pointed at a work login without the daemon having to be started from a
  particular shell, and the fleet's per-account caps (`policy.max_concurrent_account`) are keyed on
  the account the owner *chose* rather than on an accident of the environment.
* Two logins on one machine are two `AccountId`s and share no rate limit, which is what that type
  has always promised and what the probe could not previously be asked.
* **Several accounts on one daemon is deliberately not built.** It is not a config field but a
  placement dimension: something would have to *choose* between accounts per run, the per-account
  caps would have to be evaluated per candidate rather than once, and a run naming an account would
  put a person's login into a spec that travels the fleet. What exists instead is the shape this
  project already uses for "one machine, two of something" — a second `offloadd` with its own state
  directory, which the device capacity ledger (ADR-0013) exists so that they do not over-commit the
  machine between them. The residual, stated plainly: two daemons on one machine must be in two
  fleets, so one fleet cannot today use two accounts on one device.
* Nothing gossiped changed. The `AccountId` already travelled on the agent capability; what changed
  is where the node reads it from.
