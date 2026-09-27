# ADR-0045: Metered is a claim somebody has to make, and nobody was making it

**Status:** accepted · 2026-08-30 · fixes a control ADR-0011 already diagnosed once ·
builds the first half of phase 5's "mobile capability probing" · **wire v26**

## Context

`Capabilities::metered_network` is a `bool`. The probe sets it, once, unconditionally:

```rust
// Assumed until proven otherwise; a phone's daemon should override this from the
// platform's connectivity API, which is the only reliable source.
caps.metered_network = false;
```

Nothing else in the workspace ever writes it. There is no config field, no override, no platform
call — the only assignment to `true` anywhere is inside a unit test. So the field is a constant,
and everything downstream is machinery acting on a constant:

| reader | what it decides | what it does today |
| --- | --- | --- |
| `WorkPolicy::admits` | refuse to host a run here | `Refusal::MeteredNetwork` is a sentence no fleet can produce |
| `mesh::accepts_replica` | refuse to hold somebody's checkpoint blobs | never refuses — and this one is ADR-0016's durability |
| `bid::evaluate` | score this node down | never scores anything down |
| `Constraint::UnmeteredNetwork` | does this node satisfy `unmetered`? | **every** node satisfies it, always |

Two of those are operator-facing and say so out loud. `offload policy` prints

```
metered network   refused
```

on every device in the world, about a state no device can be in. And `offload match "unmetered"`
— a documented command whose whole job is to answer this question — said yes on every device there
has ever been.

**How far that constraint reaches, precisely.** `offload run` has no `--require` flag: every
submission is built with `Constraint::agent_ready(ClaudeCode, None)`, and `constraint_expr::parse`
has exactly one caller, which is `offload match`. So `Constraint::UnmeteredNetwork` is today
reachable only as a local question, and the wrong answer it gave was to a person asking rather than
to a placement decision. That makes it the *least* serious of the four and it is worth stating
plainly rather than leaving the impression that runs were being misplaced — the three that were
load-bearing are the policy refusal, the replica check and the bid penalty. What the constraint
case costs is the next thing built on it: the tree is evaluated by `bid::evaluate` for whatever
constraint a run carries, so the day a run can carry one, it would have inherited an answer that
was always yes.

**This project has diagnosed exactly this before**, and the sentence is four lines further down
the same function that prints the line above:

> It was on `WorkPolicy` and enforced by `admits` from the beginning, with nothing able to set it —
> so `Refusal::AgentNotAllowed` was a sentence no fleet could produce. **A policy rule that cannot
> be reached is worse than one that does not exist, because it reads as a control that is being
> applied.**

That is `PolicyConfig::allowed_agents`, ADR-0011's fix. The field beside it has the same disease,
and `offload policy` prints both, four lines apart.

### The other half: `false` is not what we know

The roadmap has always said metered detection is unbuilt — "mobile capability probing: battery,
thermal state, metered network, from platform APIs", phase 5. What is *not* recorded anywhere is
that the unbuilt state is spelled as a confident **no** rather than as an absence. The probe's own
comment says "assumed until proven otherwise", and then the type gives it no way to say so.

The field beside it already learned this. `PowerSource` is `Battery | Ac | Unknown`, and
`offload-probe` is explicit that "`Unknown` says so and is not folded into either. A device whose
battery would not parse is not [on mains]." One field over, a device whose network nobody asked
about is reported as unmetered.

This is the carried rule verbatim — *don't over-claim, in the probe* — and it is the probe the
rule is named after.

### What the platform actually offers

NetworkManager answers this, and answers it with more nuance than a bool can hold. Measured here:

```
$ nmcli -t -f METERED general
no (guessed)
```

`NMMetered` has five values: `UNKNOWN`, `YES`, `NO`, `GUESS_YES`, `GUESS_NO`. The `(guessed)`
suffix is load-bearing and is the reason this ADR does not simply pipe NM into a bool:

* NM guesses **yes** only for WWAN and modem devices, which really do cost money.
* NM guesses **no** for every ethernet and wifi link — *including a laptop tethered to a phone*,
  which looks like ordinary wifi or ordinary USB ethernet from below.

Tethering is the first case named in the field's own doc comment ("Mobile data, tethering, or
anything else where bytes cost money"). So NM's `GUESS_NO` is a confident answer about exactly the
case this field exists for, and it is the same guess the probe is making today.

## Decision

### 1. The capability is three-valued

```rust
pub enum Metered { Yes, No, Unknown }
```

`Capabilities::metered_network: Metered`, replacing the `bool`. `Unknown` is the default and is
what a device reports when nobody could tell — which today is every device.

Same shape as `PowerSource` in the same struct, for the same reason, and the type is what makes
the rest of this ADR possible: a bool has no room for the answer that is true.

### 2. The owner nominates, and that ends the question

```toml
metered = "yes"   # or "no"; omitted means "ask the platform"
```

A top-level config key, read into the probe the way `agent.binary` and `agent.config_dir` already
are. This is the project's standing pattern for a fact only the owner holds — `[policy]
allowed_agents` (ADR-0011), the agent's account (ADR-0028), a sink, a resource, a trigger,
`agent.max_concurrent` ("the one number about an agent only its owner knows, which is why it is
nominated here rather than probed").

Whoever tethered the laptop knows. Nothing on the machine does.

### 3. Where the owner said nothing, ask NetworkManager — and keep its uncertainty

`nmcli -t -f METERED general`, one shell-out, in `offload-probe` where every other shell-out
lives. The mapping, with a reason per row rather than a cast:

| NetworkManager | ours | why |
| --- | --- | --- |
| `yes` | `Yes` | stated on the connection profile, by the owner or by the provider |
| `yes (guessed)` | `Yes` | NM guesses yes only for WWAN, which does cost |
| `no` | `No` | stated on the connection profile |
| `no (guessed)` | **`Unknown`** | NM guesses no for every ethernet and wifi link, tethered ones included — the one case this field exists for |
| `unknown`, no nmcli, error | `Unknown` | nobody could tell |

`no (guessed)` becoming `Unknown` rather than `No` is the whole point of doing this properly: it is
the same guess the old `bool` was making, and promoting it to a fact would leave the bug in place
with a nicer implementation.

**Only NetworkManager.** No `/sys` heuristic about device names, no "is it a wwan interface"
guessing. This project's history says a probe that guesses wins bids and then fails runs; a
platform that does not answer leaves `Unknown`, which is now a thing we can say.

### 4. What `Unknown` means to each reader — the burden of proof lies with the claim

The four readers do **not** all resolve `Unknown` the same way, and the asymmetry is this ADR's
substance rather than an oversight:

| reader | the claim being made | `Unknown` |
| --- | --- | --- |
| `WorkPolicy::admits` | "this link costs money, so refuse the work" | **does not refuse** |
| `accepts_replica` | "this link costs money, so refuse the blobs" | **does not refuse** |
| `bid::evaluate` | "this link costs money, so score it down" | **no penalty** |
| `Constraint::UnmeteredNetwork` | "this link is free, so the run may have it" | **not satisfied** |

One rule, applied consistently: **whoever makes the claim carries the burden of proving it.** A
policy refusal claims the bytes cost something; a constraint claims they do not. `Unknown` supplies
neither claim, so it supports neither.

This is "unknown is not good news" read correctly rather than mechanically. The rule is not
"unknown is always the pessimistic answer" — it is that unknown must never be quietly converted
into whichever answer the caller found convenient. Converting it to *refuse everything* would be
the same error pointing the other way: a datacentre box with no NetworkManager would stop hosting
runs and stop holding replicas, which is a large, certain cost incurred to avoid a possible one.

It has a second property that is worth stating because it is what makes this ADR safe to ship: **on
every existing deployment, behaviour is unchanged.** Today every node reports `false` and nothing
refuses; after this, every node reports `Unknown` and nothing refuses. The only nodes that change
are the ones where somebody says so, or where NetworkManager does.

`Constraint::UnmeteredNetwork` is the exception and it is a real behaviour change: `offload match
"unmetered"` stops saying yes everywhere and starts saying yes only where it can be proved. Since
nothing puts that constraint on a run today, no placement changes with it — but the tree is what
`bid::evaluate` reads, so this is the answer any future `--require` inherits. It says why —
`metered = unknown, and this run needs it known to be free` — which is the difference between this
and a run that mysteriously does not place.

### 5. The reports say which of the three it is

`offload probe` printed nothing at all when the bool was false, so silence meant both "not metered"
and "nobody asked". It now names the state and, for `Unknown`, where the answer would have come
from. `offload policy`'s `metered network refused` becomes a line that says whether the guard can
fire at all. `Constraint::explain`'s `metered = false` becomes the three-valued word.

## Consequences

- A control that read as applied and could not fire now fires, and the fleet can produce
  `Refusal::MeteredNetwork` for the first time.
- **Wire v26.** `Capabilities` is gossiped inside `NodeView` and owned by the node it describes
  (ADR-0005), so the field's type is on the wire. A v25 node sends `metered_network: false`, which
  does not decode as a `Metered` — deliberately, on this project's standing argument: a peer
  claiming *not metered* when it means *nobody asked* is the bug, and silently accepting it from an
  old node would carry the bug across the version boundary.
- A node with NetworkManager and a tethered link still reports `Unknown` rather than `Yes`, because
  NM cannot tell either. The owner's `metered = "yes"` is the only thing that gets that right, and
  that is honest rather than a gap.
- `offload-probe` gains no dependency: `nmcli`'s absence is an answer.

## What this deliberately leaves

**No Android half.** Termux has no `nmcli`, so a phone reports `Unknown` and its owner nominates.
The platform call — `ConnectivityManager.isActiveNetworkMetered` — is the roadmap item this ADR is
half of, and it needs the host process phase 5 has not built.

**Not re-probed on a network change.** The capability is refreshed on the gossip tick like every
other, so unplugging the tether is noticed within a probe interval rather than instantly. An
event-driven version wants NM's D-Bus signal and is not worth a dependency yet.

**`allow_metered` keeps its shape.** Whether an owner's "don't spend my data" should also mean
"don't *migrate* to me" versus "don't run here" is a separate question, and `accepts_replica`
already answers it one way. Not reopened here.

## Amendment, 2026-09-26: the platform half, measured on a phone

The phone's host process this ADR waited for exists (ADR-0066). On a Samsung phone the app writes
`ConnectivityManager.isActiveNetworkMetered` to `host-facts.json`: `unmetered` on home wifi, and
**metered on mobile data**. A default phone then refused a task with `network is metered and policy
disallows it`, the refusal §1 made reachable and nothing had ever produced for a real reason. With
`[policy.light] allow_metered = true` the same phone took light work and still refused normal work,
on the same link, one command apart.

**…and the owner's nomination over the platform's answer, the same day.** Android calls every
cellular link metered, whatever the plan, and this owner's plan is unmetered. With `metered = "no"`
in the phone app's config, the host facts still said `"metered":true` while the probe said
`network unmetered`, and a *normal* task was placed on the phone over mobile data and ran. That is
§2's rule, that a nomination ends the question, working on the case it was written for: something the
owner knows and the machine cannot see.
