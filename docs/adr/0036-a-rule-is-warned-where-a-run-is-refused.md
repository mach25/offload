# ADR-0036: A rule is warned where a run is refused, because the fleet it fires into changes

**Status:** accepted · 2026-08-27 · extends ADR-0032 to the third precondition · supersedes nothing

## Context

ADR-0032 found that the two flags whose promise the fleet may not be able to keep — `--notify
<service>` and `--ask` — were answered on `Response::Submitted` and on nothing else, so `offload
run` warned and `offload when` printed a reassurance. It fixed both. There is a **third** flag in
that family, and it was missed because on `offload run` it is not a note at all but a *refusal*:

```
$ offload run --repo … --use email -- "read my mail"
Error: no node in this fleet offers email for a run to use — `offload status` on each device
lists what it offers, and `[[resources]]` in its config is where one is nominated

$ offload when schedule --repo … --use email -- "read my mail"
rule d4a3ee6ccd4bc6d4 — on `schedule`
  Quiet while it works: you will hear about a failure, a missed deadline or a question, …
  Nothing else to do: the next event fires it.          ← about a rule that can never run
```

`unreachable_resource` lives inside `submit_run`, which is deliberately the **one copy** both
callers use (ADR-0020 §5) — so it is enforced correctly at every firing, and a rule with `--use`
for a service nobody offers is refused *per occurrence*, for ever. Measured, on a trigger ticking
every three seconds:

```
RULE               ON            FIRED  DROPPED  KEPT  WHAT
d4a3ee6ccd4bc6d4   schedule          0       13     0  read my mail
                   └─ no node in this fleet offers email for a run to use — …
```

Thirteen firings, no runs, thirty seconds. Nothing on the phone — a refused firing is not a run,
so there is no run log and nothing for the delivery plane to project. Nothing at the keyboard. One
`WARN` per firing in the daemon's log, on the machine nobody is logged into. Left overnight that is
28,800 refusals and zero work, and the rule *looks* busy while producing nothing, which the
missing-trigger case at least does not: that one fires zero times and says so.

The comment on the arm directly below the one that printed the reassurance had already written the
principle down, about the trigger:

> Written rather than refused, and said rather than left to be discovered. A rule for a service
> nothing here watches is inert, and inert-and-silent is how somebody spends a week wondering why
> their trigger never fired.

## Decision

`Response::Watching` gains `resources: Option<String>`, computed by `resource_note` — which shares
`missing_resources` with the refusal `submit_run` raises, so a rule and a run cannot say different
things about one fleet. Same rule as `Reach` being computed through `Audience::admits` rather than
described a second time.

**A warning, not a refusal**, and the asymmetry with `offload run` is the point rather than an
inconsistency:

* A run is submitted now, so refusing it is actionable now — ADR-0014.
* A rule fires for months, and the fleet it fires into changes. The mailbox is on the phone
  (ADR-0011's whole example), and the phone may not have enrolled yet. Refusing would make the
  order of two setup commands matter, and would make `offload when --use` unusable in exactly the
  arrangement the resource plane was built for.
* It is the same call the trigger question already gets, one line down, for the same reason.

So the wording is future tense and says what the silence costs: *"Nothing in this fleet offers
email, so a firing will be refused rather than run — `offload rules` counts those under DROPPED,
with the reason. `[[resources]]` in a node's config nominates one, and the next firing after that
will work."* And the closing line stops lying: `Nothing else to do: the next event fires it` becomes
`The next event will fire it, and be refused, until then.`

## Consequences

- Measured after the change, and silent where the fleet can honour the flag — a second node
  nominating `[[resources]] email` makes the note disappear and the rule fire: **11 firings, 0
  dropped**, each occurrence spawned with `--mcp-config` and the `mcp__email` allowlist pattern.
- No wire or schema change: the control protocol is unversioned.
- The guard pins the two answers to one fact — for the same input, either both speak or neither
  does — which is the drift this could only ever fail by. Watched red on a `resource_note` that
  returned `None`.
- The gossip-stale residual is the same as ADR-0032's and smaller: a peer whose resource
  capability has not been learned yet reads as missing, so the warning over-warns for a tick. It
  is a warning now rather than a refusal, which is the direction that costs nothing.

## What this walk also settled, and it is a clean result

The pair this session set out to walk — **a rule whose occurrence uses a resource on a peer** —
holds. Two daemons, the mailbox nominated on the one that cannot host at all, the rule on the one
that can, and eleven firings in twenty-five seconds each projected the *proxy* rather than the
holder's command:

```
{"mcpServers":{"email":{"command":"…/offloadd",
  "args":["use-resource","--socket","…","--run","01a0445a4148…","--service","email"]}}}
```

No `env`, no `/bin/cat`, nothing about how beta's mailbox works — named by **service**, because the
caller does not know the holder's id and has no business learning it (ADR-0011's amendment). For a
run nobody submitted, on a node where nobody is watching. That is the fifth pair walked this way and
the first to come up clean on the mechanism itself; what it produced was the precondition above.
