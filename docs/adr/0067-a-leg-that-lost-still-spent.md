# ADR-0067: A leg that lost still spent — spend is recorded per leg and summed

**Status:** accepted · 2026-09-26 · session ninety-two · amends ADR-0040 §3 and makes ADR-0005's
"spend is cumulative from every leg" true for forked legs · no wire bump (see §4) · schema v13

## Context

A partition ran one run twice in session ninety-two. WARMUP finished on the Mac at epoch 1; the
laptop, which had the Mac `dead`, reassigned it and ran it again at epoch 2. Both nodes then agreed
on `completed`, one turn, **110 tokens**, when **220** had been spent.

That is two correct rules meeting a case neither covers:

- **Tokens are position** (ADR-0040 §3). They are read from the transcript, which is cumulative
  along a chain of resumed legs, so `RunProgress::absorb` takes the winning leg's number outright.
  Adding legs would count every earlier leg again.
- **Cost merges by `max`.** Each leg adds its own `result` cost to the row it writes, so along a
  chain the row already holds the sum.

Both assume the legs form a **chain**: each one resumed from the last. Two legs that both started
from nothing are not a chain, and each rule keeps one of them. ADR-0005 says spend is cumulative
from every leg "because a leg that lost still spent it", and for a fork that sentence was false.

The owner chose to count it: *keep a separate figure for spend by legs that lost, merged by max per
leg, and show it beside the run's total.*

## Decision

### 1. Each leg records what it started from and what it spent

`RunProgress` gains `legs`: one `LegSpend` per leg, keyed by `(by, epoch)`, the same stamp the
position already carries. Each entry holds:

- `base`: the tokens in the transcript the leg started from. That is zero for a fresh start. For a
  resume it is counted from the restored checkpoint transcript at the moment the leg starts.
- `tokens`: the leg's transcript as last captured. It is cumulative, like the position's.
- `cost_micro_usd`: the cost this leg's own `result` events added.

The writer is the one that stamps the position (`Supervisor::update_stats`). Relays pass entries
through untouched, like `at`, `by` and `epoch`.

### 2. What a run spent is the sum over legs

`spent()` = Σ (`tokens` − `base`), with Σ `cost_micro_usd` beside it. Along a chain that equals the
position's `tokens`, since each leg's base is where the previous one ended. For a fork it is both
legs. **Lost** is `spent − position`: zero along a chain, and in WARMUP's case 110.

### 3. The merge is a union, and within a leg the larger entry wins, totally ordered

Entries for different legs are all kept. For one leg, the entry with the larger `(tokens.total(),
cost_micro_usd, base.total())` wins, which is a total order, so every node settles the same way
(`pitfalls/gossip-and-merge.md`: a merge tiebreak has to be total). Only the leg's author writes
its entry, and it only grows, so "larger" is "later". The position rules do not change.
`absorb` still takes the winning leg's `tokens`, and that pitfall's warning against adding a `max`
to it stands.

**Owner, per ADR-0005:** the leg's author, for its own entry. The arbiter is the ordering above.
An entry stays true after its author is gone, because it describes spend that already happened.

### 4. No wire bump

The wire is JSON and the field is `#[serde(default)]`, exactly as `RunProgress::tokens` was added
unbumped. The test from `pitfalls/gossip-and-merge.md` passes: an older node relaying a record
without `legs` cannot erase them, because the merge is a union, and the default (no entries) is
the old behaviour, not a wrong answer.

### 5. Reports show it beside the total, not instead of it

`offload ps` prints the position's tokens as before, and under the row, where it already puts a
run's other notes, `+N tokens spent by a leg that lost` when some leg's spend is not in the
surviving conversation. Its cost column is the per-leg sum where that is larger, since money spent
is every leg's. (`offload explain` shows no spend today, so it gains nothing here.) The total a person reads is still
what the run *is*; the lost figure is what it *cost* besides.

## Consequences

- A forked run's cost is honest in both units. The dollars were the same bug (max of two legs), and
  the same entries fix them.
- Schema v13 adds `legs_json` to `runs`: a JSON column, in the shape `run_json` already uses.
- Legs are few: one per grant a run has ever had.

## What this deliberately leaves

- **A leg whose author never reached a peer again** is not counted anywhere but its own node. That
  is the same limit as every gossiped fact, and it is why the Mac's leg was counted only once the two
  met again.
- **A fork's lost work** (commits and transcript) is still ADR-0053's and ADR-0055's business. This
  is about what it cost.

## Amendment, 2026-09-26: walked on two daemons, and the case it does not reach

**The WARMUP shape, staged and measured.** Laptop and bravo, fake agent, a run preferring bravo.
Bravo's daemon was frozen (`SIGSTOP`) until the laptop reassigned the run to itself at epoch 2.
Then the laptop was frozen and bravo thawed, so bravo finished its leg at epoch 1 without ever
hearing it had lost, and then the laptop thawed and finished too. **Both nodes' `offload ps`:
`110`, and under it `+110 tokens spent by a leg that lost`.** The controls were in the same listing:
the same staging with no fork (the laptop frozen before it had taken over), and every ordinary
run, showed no line. The before is session ninety-two's own WARMUP reading on the Mac and the
laptop, which was 110 and nothing else.

**What it does not reach: a losing leg that is stopped mid-turn.** The first staging froze bravo and
thawed it only after the laptop had finished. Bravo then learned it was reassigned and, correctly,
stopped its agent (`this run has been reassigned; stopping the agent here`) before any turn
boundary. No checkpoint was captured, so its entry stayed at its base, and the tokens its agent had
already spent in that turn are counted nowhere. **Closed the same night:** a leg that may no
longer describe the run now reads its transcript once its agent is down (`note_leg_tokens`) into its
own entry only. The position and its stamp are untouched, which is why this does not go through
`update_stats`. Re-walked with the same staging: bravo's only line for the run was `this run has
been reassigned; stopping the agent here`, there was no checkpoint, and both nodes' `ps` said `+110
tokens spent by a leg that lost`.

**Verified by unit test only:** the chain case, a leg resumed from a checkpoint with a non-zero
base. The arithmetic is tested, and the base is counted from the restored transcript at start. No
walk has resumed a multi-turn fake agent under this build.

**And the chain, walked, which found the one bug this ADR's build had.** A three-turn fake agent
was drained on bravo at turn 2 and resumed on the laptop. The first pass printed `+110 tokens spent
by a leg that lost` on an *ordinary migration*, the false alarm this design most had to avoid. The
cause was the order of writes. Before its agent starts, the new leg makes ordinary stats writes
("workspace ready"), and the write path created the leg's entry lazily with base = the row's tokens
at that moment, which was the old leg's turn-1 figure (110) still in the row. The start then found
an entry already there and kept it, although the checkpoint it resumed from held 220. **Now only a
leg's start creates its entry**, and every other writer updates an existing one or does nothing, so
a missing entry can under-count but never raise a false alarm. Re-walked: 330 tokens, no line on
either node, and the resumed leg's base is 220.

A third staging, by accident, checked the arithmetic in the hardest case. Bravo's daemon was frozen
while its agent ran all three turns unsupervised (330). The laptop resumed from bravo's replicated
turn-1 checkpoint (base 110) and ran one turn (220). Both nodes print `+220`, which is exactly
440 spent against a surviving 220. The run completed once, at epoch 2, on both. Bravo, thawed
straight into a shutdown, checkpointed and released its stale epoch-1 leg, and the fence absorbed it.
