# ADR-0063: A preference is scored, never filtered — and "here" is a place a run may prefer

**Status:** accepted, built (session eighty-nine, wire v30) except §2's walk · 2026-09-22 · phase 10 · answers `docs/use-cases/operations-fleet.md`'s
item 4 (*"the one genuinely missing primitive"*) · adds `--require` beside it · leaves ROADMAP questions 7 and 9
where they are, and says why

*Written at the owner's request in session eighty-eight, from four scenarios they described, and
accepted by them the same day. Nothing in it has been walked. The numbers in §2 are arithmetic on
`BidWeights::default()`, not measurements, and are the first thing to check when it is built.*

## Context

The owner's words, from the session that produced this: *"it could end up on same node. Especially
if I'm sitting on my machine. But let's say I have a workload that is more suited for my mac it
could be the one to process the workload. Or … an Offload app on my phone … sending work to an
available node, or perhaps I want my desktop/laptop to do it because I'm going to be at that machine
later."*

That is four placements, and today the fleet can express exactly one of them by accident:

| Wanted | What the tree does | Why |
| --- | --- | --- |
| **here**, because I am at this machine | only when the repo is a local path, which *forces* it | `RepoReach::NotHere` refuses every other node; nothing in `score` knows where a run was submitted |
| **the Mac**, because the work suits it | only as `Os(macos)`, which means *only* the Mac, waiting while it sleeps | there is no soft form of a `Constraint` |
| **any available node**, from the phone | works — it is placement's ordinary job | `phone-as-a-member.md` |
| **the desktop, later**, because I will be there | only as a hard tag pin | same as the Mac, plus nothing that gives up waiting |

And the hard form cannot be typed either. `RunSpec.constraint` exists and every constructor of it
in `offload-node` builds `Constraint::Always`, `agent_ready(..)` or the task tier's
`ServiceAuthenticated` — **`offload run` has no `--require`**, which
`docs/pitfalls/capacity-policy-and-probing.md` already notes in passing. The grammar exists
(`offload-cli/src/constraint_expr.rs`, used by `offload match`) and has no submission surface.

Three facts from the code decide the shape below.

1. **Bids are ranked availability first.** `bid::better` compares
   `(available.is_now(), score, Reverse(node))` (`offload-core/src/bid.rs:663`). A score term can
   never beat a node that is free now with one that is merely willing later, and a node that is
   asleep does not bid at all. So *"prefer the desktop"* as a score cannot make a run **wait** for
   the desktop, whatever its weight. Waiting is a different decision (§4).
2. **Every bidder scores itself** (`bid::score`). A preference evaluated by the bidder against its
   own capabilities and its own id needs no new gossiped fact and no belief about a peer — the run
   spec carries the question, and each node answers it about itself.
3. **Attendance is about a *run*, not a *place*.** ADR-0013's attendance is *somebody is streaming
   this run*, observable only where the stream is served and never gossiped. "The machine I am
   sitting at" is a different fact about a node, which nothing observes today. It must not borrow
   the word.

## Decision

### 1. `RunSpec` gains `prefer: Constraint`, consumed by scoring and never by eligibility

Same type, same grammar, same `explain` as `constraint` — the reuse `operations-fleet.md` argued
for, because a second, smaller preference language is a second copy of the rule that drifts. A
bidder that satisfies `prefer` adds `weights.preferred` to its score; one that does not adds
nothing and **is not refused**. `Always` is the default and adds nothing to anybody, so a run that
states no preference scores exactly as it does now.

Binary rather than proportional: a node matching two of three clauses scores the same as one
matching none. Partial credit makes `mem>=16G,class=desktop` a sum the submitter never wrote down.
Several ranked preferences are a residual (below), not a proportional score.

**Refused: preference as a filter that relaxes when nothing matches.** It is the tempting reading
of *"prefer"*, and it is a second eligibility pass whose result depends on who happens to be up —
a run placed on the laptop because the desktop was asleep for thirty seconds, with nothing in the
record to say the preference was dropped rather than never met. A score says the same thing in the
one place bids are already compared, and `explain` can print it.

### 2. The weight is a constant of the protocol, set to beat any one soft factor and not two

`BidWeights::preferred`, default **60**. What it is weighed against, computed from the defaults
for the case that prompted this — the laptop being sat at, against a desktop:

| term | laptop: 8 cores, 16 GiB, transient | desktop: 32 cores, 64 GiB, stable, mains | Δ toward desktop |
| --- | --- | --- | --- |
| stability (×10) | 10 | 20 | 10 |
| cores (6 per doubling) | 18 | 30 | 12 |
| memory (4 per doubling) | 16 | 24 | 8 |
| **hardware, on mains** | | | **30** |
| battery at 50 %, not on mains (40 × 50 %) | −20 | 0 | +20 → 50 |
| load 100 % (30) | | | +30 → 60 |
| `workspace_warm` on one side only | | | ±40 |

At 60, the preferred node wins against a bigger machine while it is on battery at half charge, and
loses — or ties, broken by node id — once it is both on battery below ~25 % *and* busier, or cold
while the other is warm and loaded. That is the intent in one sentence: **a preference outweighs
any single reason the fleet has to go elsewhere, and yields to two.** A preference that always
wins is a pin, and a pin is `--require`.

**A constant, not configuration**, which keeps ROADMAP question 7 closed rather than answering it:
*"two nodes running different `BidWeights` bid in different currencies"* is true of every term, and
most acutely of this one, because it is the term the *submitter* asked for. `BidWeights::default()`
is still the only constructor. The day a weight becomes configurable, question 7 is answered first
— this ADR does not add the knob that forces it.

### 3. `here` and a node's name are preferences about identity, resolved at submission

`Constraint::Node(NodeId)` — one new variant — and two spellings for it in the grammar:

- **`here`** resolves to the id of the daemon that took the submission, **when it is submitted**.
  Not "wherever the submitter is now": nothing observes that (Context 3), and a spec that changed
  meaning after it was written is the drift ADR-0019's amendment already paid for.
- **`node=<name>`** resolves against the view's member names at submission. An unknown name is
  refused; a name two members share is refused naming both ids. Stored as the id, because a name
  is the node's own label and ids are what the fleet agrees on.

Each bidder asks whether *it* is the node — a question about itself, which it may answer, rather
than a belief about a peer.

The four scenarios in those terms:

```
offload run --prefer here "…"                  # I am at this machine
offload run --prefer os=macos "…"              # the Mac if it is up, anything if not
offload run "…"                                # from the phone: any available node
offload run --prefer node=desktop --hold 17:00 --queue "…"   # the desktop, later (§4)
```

And the rule a mailbox wants (`docs/use-cases/primed-work-from-a-mailbox.md`):
`offload when email --prefer here …` resolves `here` **when the rule is written**, so a fired run
prefers the machine that holds the mailbox's context and still goes elsewhere if it is off.

A `here` typed on a node that cannot host — the phone — names a node that will never bid. That is
not an error, it is a preference that cannot be met, and **the submission says so** before it
returns: *"preferred: this phone, which hosts no runs — placed on laptop"*. Silence would be a run
landing somewhere unexpected with nothing to explain why.

### 4. Waiting for a preferred node is a requirement with an expiry, and only a stated one

`--hold <time>`: until then, `prefer` is **ANDed into eligibility**; after it, it is a preference
again. Pure in core: the effective constraint is `f(spec, now)`, no clock, no stored state.

- **It needs a time somebody stated.** An implied expiry is the unspecified-deadline trap in
  `docs/pitfalls/scheduling-and-attendance.md` — *patience needs a deadline somebody stated* —
  and a hold without one is `--require`.
- **It implies `--queue`.** A run held for an absent desktop has no bidder at submission, and
  ADR-0014's answer to no bidder is a refusal to the operator's face.
- **A hold later than a stated `deadline` is refused at submission.** It asks the fleet to wait
  past the point where waiting was said to be pointless.
- **It changes when we give up, never what we may do** — the deadline rule in the same pitfall
  file. It relaxes one clause; it does not shorten a lease or weaken a fence.
- **At expiry nothing happens by itself** until the next round, which for a pending run is at most
  one `REASSIGN_RETRY` (measured at 30.2 s in that pitfall file). That delay is written down rather
  than engineered away.

**Refused: a preference that decays with urgency.** Proposed in the conversation that produced
this — let `weights.preferred` shrink as slack falls. It is still a score, so by Context 1 it can
never make a run wait for anybody; it would only make it *less* likely to land on the preferred
node the closer the deadline got, which is the opposite of what "I will be there later" means.

### 5. A granted resource held on the bidder is a preference the run did not have to state

`weights.resource_local`, default **20**, per resource the run was granted with `--use` that the
bidder itself holds. ADR-0011's proxy made a grant stop constraining placement
(`capacity-policy-and-probing`: *"a grant constrains placement only while it cannot be honoured
remotely"*), which was right and dropped a signal: a run placed beside its context reads it
locally, and one placed elsewhere reads it over the fleet. 20 is `checkpoint_local`'s weight and
the same kind of fact — cheaper here, not required here. It is not a `Constraint` and it is not
the submitter's; it is scored exactly like `workspace_warm`.

### 6. `--require` ships with `--prefer`, in the same grammar

A preference whose hard twin cannot be typed will be used as one, and then filed as a bug when the
desktop was asleep. `--require <expr>` sets `RunSpec.constraint` (ANDed with what the daemon adds
itself — `agent_ready`, the task tier's clause), on `offload run` and on `offload when`.
`offload match` accepts `here` and `node=` too, so the command that answers *would this device
match* reads the same parser the submission does.

### 7. Every report reads the score it is reporting

`explain` shows, per bidder, whether `prefer` matched and what it added — from the `Score`
`bid::score` returned, never recomputed. `offload run` names the preferred node when it was not the
one placed and says which of three it was: *not available*, *available and outscored* (with the
terms that did it), or *could not host*. `offload ps` shows a held run as *held for desktop until
17:00*, from the same `f(spec, now)` placement reads. Three readers, one function each — the rule
this tree keeps relearning.

## Why not the alternatives

**A pin with a timeout and nothing else.** Covers the Mac and the desktop, not *here*: a pin to the
laptop you are at is wrong the moment you are at it on 8 % battery. The scenarios want a score *and*
a wait, and they are two decisions (§1, §4).

**Ranked preferences (`--prefer node=desktop --prefer os=macos`, descending weights).** Deferred,
not rejected. It is the natural next step and doubles the surface, and one binary preference plus
`--require` covers every scenario the owner named.

**A presence signal — the node the owner is at, observed.** The most attractive version of *here*,
and it is not available: nothing observes it, and borrowing ADR-0013's attendance for it would
conflate *somebody is watching this run* with *somebody is at this keyboard*. If it is built, it is
a node's own fact, scored by that node like `cpu_load_percent` and **never gossiped** — attendance's
rule, for the same reason. A residual below.

**Reclaim — a run moves back to its preferred node when it returns.** ROADMAP question 9.
`migration_penalty` (30) already guards a healthy run against a better offer, and `prefer` does
not change when a round happens; it changes only who wins one. `operations-fleet.md` argues
affinity and reclaim are one mechanism. They share a score; they do not share a trigger, and the
trigger is the whole of question 9.

## Consequences

Good:

- All four scenarios are expressible, and three of them in one flag.
- A mailbox, a schedule or any rule can prefer the machine that holds its context without being
  stranded when it is off — the shape `primed-work-from-a-mailbox.md` needed.
- No gossiped fact is added about a node. The run spec carries the question; each node answers it
  about itself.
- The hard form finally has a surface, and `Constraint::Not` is still the only variant nobody can
  type.

Bad, and to be clear-eyed about:

- **Wire version bump**: two `RunSpec` fields (`prefer`, `hold_until`) and one `Constraint`
  variant. A node too old to decode `Constraint::Node` cannot decode the run at all — the handshake
  gates it, and `storage-and-encoding` must be read before building. Stored specs decode with
  `prefer = Always`, `hold_until = None`.
- **60 and 20 are reasoned, not measured.** The table in §2 is arithmetic. The walk: two daemons of
  unequal size, `--prefer` on the smaller, battery and load varied, the winner recorded against the
  table. Amend the numbers rather than the table.
- **`here` means the submitting daemon, and from a phone that is rarely what anybody means.** The
  phone app will want `node=` and a picker. §3's refusal to guess is the price of not having
  presence.
- **A preference that is never met is silent in the fleet**, visible only in `explain` and the
  submission's own line. That is §7 doing its job, and it is still somebody having to look.

## Residuals — named, not built

- Ranked preferences.
- A presence signal, scored locally and never gossiped.
- Reclaim when a preferred node returns (ROADMAP question 9).
- Configurable weights, which would answer ROADMAP question 7 first.

## Amendment, 2026-09-24: built, and what building it changed

Session eighty-nine. Walked on two daemons on one machine with a fake agent, and with Haiku for
the continuation demo that uses it.

- **`--hold` takes a duration, not a clock time** — `--hold 8h`, like `--deadline`. §3's
  `--hold 17:00` would need a timezone, and session eighty-seven declined adding one to this
  crate for `offload audit`'s timestamps. A duration says the same thing without a guess.
- **A held-off node refuses with `NoBid::Held`**, not the `Ineligible` the ANDed clause produced.
  The first walk printed `ineligible: is node 2a3b5d5d (this is 29be31be)` — two ids and nothing
  saying it was a hold that ends by itself. `evaluate` asks the requirement first and the hold
  second, through `RunSpec::eligibility`, so a node failing the requirement still says *that*.
- **The hold's expiry needs nothing to happen**, measured: a 90 s hold for a node that could not
  host queued the run, alpha refused as held, and alpha took it **4 s after the hold ended** — well
  inside one `REASSIGN_RETRY`.
- **§7's three readers read `ScoreTerms`**, which `score` now sums and which travel on `Offer`,
  `Bid` *and* the wire's `ClusterMessage::Bid`. The last was missed on the first build — the
  arbiter rebuilt a peer's offer with no terms, and the note called the winning node outscored by
  itself. The note now asks the named ids before any terms.
- **A rule resolves `here` when it is written** (`pin_rule_placement`) and refuses `--hold`: a
  hold is an instant and a rule fires for ever. `offload when` therefore has `--require` and
  `--prefer` and no `--hold`.
- **Still not done: §2's table.** Two daemons on one machine probe the same hardware, so the walk
  it asks for — unequal size, battery and load varied — needs the Mac or the phone. The numbers
  are pinned as arithmetic by `a_preference_beats_one_reason_to_go_elsewhere_and_yields_to_two`.

## Amendment, 2026-09-25: §2 walked on two machines, and one number amended

Session ninety-two, on the first Linux↔macOS fleet: this laptop (8 cores, 15.5 GiB, `Transient`,
on mains at its charge limit throughout) against the Mac mini (8 cores, 8 GiB, `Stable`, mains).
A fake agent on both, each round read from `offload explain`'s canvass. With a stated preference
the canvass shows the other node's bid, less the 30-point migration penalty, and a preference that
loses prints both totals with the terms that decided them. Every arm matched the arithmetic.

| arm | laptop load | Mac warm | `--prefer` | laptop | Mac | placed |
| --- | --- | --- | --- | --- | --- | --- |
| A | 13 % | no | — | 37 | 44 | Mac |
| A′ | 18 % | no | laptop | ~95 | 44 | laptop |
| Dn | 64 % | no | — | 21 | 46 | Mac |
| D | 66 % | no | laptop | ~81 | 46 | laptop |
| E | 66 % | yes | laptop | 81 | 86 | **Mac** — `outscored (81 against 86: preferred -60, warm workspace +40, load +15, stability +10 to the winner)` |
| F | 34 % | yes | laptop | ~90 | 86 | laptop |

So **a preference outweighs one reason and yields to two** on real hardware (D and F against E), and
the table's shape stands. Two of its numbers do not:

- **The load row cannot reach 30.** `admits` refuses a bidder under 25 % idle for a `Normal` run
  (`Demand::needs_idle_percent`: 10 / 25 / 50 for light / normal / heavy), so the worst load term
  a node that *bids* can carry is **−22** for a normal run, −27 light, −15 heavy. The row reads
  `load ≤ 75 % (≤ 22)` for the default demand, and the `+30 → 60` line is `+22 → 52`. A node past
  that line is not outscored; it answers `NoBid::Busy` or `Refused(UnderPressure)`. The first
  attempt at arm A met exactly this: a property run in the background put the laptop at 100 %,
  and it declined rather than bidding low.
- **Memory doublings are floored, and a "16 GiB" Linux machine has fewer than 16.** The laptop
  reports 15 862 MB, `ilog2(15) = 3`, so it scores 12 for memory, **the same as the 8 GiB Mac**,
  which reports its memory whole. The table's laptop row (16 GiB → 16) is therefore 12 on the
  hardware it describes, and a 64 GiB Linux desktop scores 20, not 24. Its Δ of 8 happens to survive,
  since both lose one doubling. Recorded here and not changed: rounding to the nearest doubling
  would change every bid in the fleet, and that is a decision, not an amendment.

**The battery row: its input walked, its score not.** The laptop's battery is worn to half its
design capacity and drains fast, so the owner unplugged it for 38 seconds. The probe said
`on battery, 97%` at once, and the daemon logged `changed=power` five seconds later and again on
replugging. So the fact the term reads is right on this hardware, a worn battery included. The bid
round in that window is void, because the Mac had dropped 34 seconds earlier and the laptop bid alone. A
retry would read −1: at 97 % the term is `40 × 3 %`, and a legible reading needs about half an hour
unplugged. One number amended from the policy rather than a walk, the same shape as the load row:
**the default laptop battery floor is 20 %, so the worst battery term a laptop that bids can carry
is −32, not −40.** A bid at 50 % (−20) stays as the table has it; the next paragraph measures it.

**Later the same session: the battery row, measured on an emulated phone.** The SDK's Pixel 3a image
(Android 14, x86_64, 4 cores, 2.4 GiB) ran the x86_64 Android build as a fleet member, with its
battery set from the emulator console (`power ac off`, `power capacity N`), which is what the probe
reads. Tasks carried the rounds, since the Android shell has no `git` and the battery term is
device-level. The laptop was the other bidder.

| arm | emulator power | policy | `--prefer` | emulator | laptop | placed |
| --- | --- | --- | --- | --- | --- | --- |
| P0 | battery 50 % | phone default | emulator | refused: `accepts work only while charging` | — | laptop |
| P1 | charging 50 % | phone default | emulator | 76 | 36 | emulator |
| Pn2 | battery 50 % | `accept = "always"` | — | **−4** (hardware 16, **battery −20**) | ~37 | laptop |
| P2 | battery 50 % | always | emulator | 56 | 34 | emulator |
| P3 | battery 30 % | always | emulator | refused: `battery at 30%, policy floor is 40%` | — | laptop |
| P4 | battery 41 % | always | emulator | 53 | 36 | emulator |

The term reads what the table says: −20 at half charge. Two more facts come from the policy rather than the score:

- **On a default phone the battery term is always zero.** A phone accepts work only while charging,
  and charging counts as mains, so a default phone either bids with no battery term or does not
  bid at all. The term only exists where an owner has said `accept = "always"`.
- **Its ceiling is the floor.** At the phone's default 40 % the worst term a bidder carries is −24,
  and at the laptop's 20 % it is −32. It is never the table's −40.

So on this pair the battery alone never beats a preference (P4, the worst case). The arm that
would add load as the second reason cannot be isolated, because the emulator is a process on the
laptop and loading the guest loaded the host (P5: emulator 50 %, laptop 39 %, emulator still ahead,
38 against 29, on the arithmetic). A real phone is the place for it. Not established here: whether Termux on
a real phone can read `/sys/class/power_supply` at all. The emulator's shell can; a real phone's SELinux
policy may not let an app, and the probe would then say `Unknown`.

## Amendment, 2026-09-26: memory scores to the nearest doubling

The owner's decision on the floor this ADR's walk found. `bid::memory_doublings` rounds in log
space: `k + 1` once memory reaches `√2 · 2^k` GiB, decided in integers so every node computes the
same number for the same machine. A Linux laptop reporting 15 862 MB scores as the 16 GiB machine
it is (4 doublings, 16 points), a 64 GiB desktop reporting ~62 GiB scores 24, and the 8 GiB Mac
stays at 12. So §2's table is right again as written, with 16 for the laptop row. Cores are
unchanged: they are counted exactly, and the machines in this fleet are powers of two or close.
