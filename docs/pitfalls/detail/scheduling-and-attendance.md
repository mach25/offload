# Deadlines, urgency, attendance and Pending — full entries

The working rules are in `../scheduling-and-attendance.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A follower that goes away has to be *noticed*, not waited for.** A `logs -f` loop discovers
  a closed client when a write fails, and a quiet run writes nothing for minutes — so both follow
  paths watch the client's read half for EOF. Without it an abandoned follow polls a peer for
  ever and, much worse, keeps the run looking **attended**, which is exactly the input that
  decides whether a failed run resumes itself.

- **Attendance is knowable only where the stream is served** — including when the stream is
  served *to a peer*: a node asking for a run's log once a second is a follower somewhere else,
  and counting it is what makes attendance mean anything for the runs this project exists to
  move. It decides the failure case its observer is present for (an agent that stopped) and *not*
  the hold-down: that is the arbiter's decision about a holder that has vanished, so the one node
  that could answer is the one that is gone. Gossiping it hands the arbiter a value from before the
  silence and calls it current — the same confident wrong answer as replaying a stale bid.

- **Urgency is derived, never stored.** A run carries a `deadline`; urgency is
  `f(deadline, now)` and rises on its own as slack shrinks (ADR-0013), so every node agrees
  without a message and nobody has to remember to bump anything. `priority` says who yields;
  low priority never means "wait" on an idle fleet. Whether a human is watching is
  **observed** — some client is streaming the run — never declared at submit time.

- **An unspecified deadline means the moment of submission**, which makes slack negative age
  and aging free — and makes every ordinary run overdue within a second. So the uses of slack
  are not one knob: **ordering and pickiness** use it whatever its origin, while **patience** —
  how long to wait for a holder that has gone quiet — and **telling somebody the deadline is
  gone** both need a deadline somebody stated. Four call sites now (`grace_for`,
  `review_commitment`, `Run::prospect_at`, and the pressure deferral), which is why the third
  answers with a `Prospect::NoStatedDeadline` variant rather than a comment: a caller that
  matches only what it cares about cannot skip it by accident. Letting derived urgency cut the hold-down moves every run in the
  fleet fifteen seconds after a Wi-Fi handover, which is the thrash ADR-0007 exists to
  prevent, arrived at through the door marked "scheduling hint".

- **A deadline changes when we give up, never what we may do.** It feeds hold-down, retry and
  auto-resume — and no deadline, however close, may weaken fencing, skip a checkpoint,
  snapshot mid-turn, or shorten a lease. The moment it becomes the flag that makes things
  faster, the first thing it buys is double execution. In tension, the run misses the
  deadline.

- **A missed deadline is a sentence, not a decision.** `note_overdue` writes a typed
  `LogKind::Overdue` into the run's own log and changes nothing else: the run keeps being
  offered, keeps its checkpoint and its lease, and `ps` still says `pending`. That is the same
  boundary as the bullet above, in the place it is most tempting to cross — a rule that
  cancelled a run for missing a time somebody typed casually would destroy a night of agent
  turns on a scheduling hint. It is said **once per stated deadline** (latched in memory, so a
  moved deadline earns a fresh answer and a restart repeats itself in the safe direction), and
  it goes in the *log* rather than only to `tracing`, because the log is what `offload logs`
  reads from any node and what ADR-0010's delivery plane will fan out from. Two facts reach it:
  a queued run refused past its deadline, and a rate limit that lifts too late — judged the
  moment the agent reports it, not when the deadline arrives, since a reset two hours the wrong
  side of it has already decided the matter.

- **`Pending` has three entrances and one exit that can refuse.** A released checkpoint, a
  handed-back commitment and an operator reopening a failure all lead there, and `assign`
  refuses a pinned run a second holder — so a pinned run that reached the pool by any of the
  three was stuck for ever: not failed, not cancelled, and invisible to the hold-down, which
  only looks at `Orphaned`. Nothing failed, which is what made it worth a guard rather than a
  comment. The rule was already on `KeepReason::PinnedHere` in as many words and enforced in
  exactly that one place; it is enforced where `Pending` is entered now.

- **A bid round moves the run; the caller is still holding the copy it had before.**
  `Cluster::place` takes `&Run` and mutates a clone: it spends an epoch per grant attempt, hands
  the run over, and — on the way out of a round nobody confirmed — publishes the record at an epoch
  past everything it issued. `server::place` then acted on `run`, the copy `Supervisor::build`
  produced before any of that happened.

  Both arms did it. On **acceptance by this node**, `confirm_record(&run)` publishes what it is
  given straight into this node's own view, so the `Assigned`/epoch-1/holder-us record that
  `NodeHost::accept` had *just* published — deliberately, because "`evaluate` answers the next bid
  from the view, so a node whose view has not caught up with what it just accepted bids as though
  it were idle" — was replaced by `Pending`/epoch 0/nobody, until the next `report_local_facts`
  tick republished the store. On **refusal with `--queue`**, `record_run(&run)` and
  `confirm_record(&run)` wrote the pre-round copy to the store *and* back into the view, undoing
  the epoch the round had spent from one line up the stack.

  Measured on one daemon, `max_concurrent_runs = 1`, three submissions in the same second, with a
  clean `XDG_RUNTIME_DIR` so the device ledger held nothing:

  | | before | after |
  | --- | --- | --- |
  | first | starts now | starts now |
  | second | starts now | "when the run ahead of it finishes" |
  | third | starts now | "when one of the 2 runs ahead of it finishes" |

  Every one of those was accepted, so `offload ps` showed two runs `waiting for a slot` a moment
  after their operator was told they were starting — which is precisely the difference
  `Placement::Accepted`'s own doc comment says must not be blurred: "telling somebody the first when
  it is the second is how they close the laptop expecting output by morning". On a fleet it is the
  other half of the same fact — the second submission of a pair goes to the node that is already
  full instead of to the idle peer, which is the "six submissions in two seconds all went to the
  same node" symptom `accept`'s comment describes, restored one function up.

  The record leaves the round on the decision now (`Placement::Refused { spent }`, `None` when the
  round handed nothing out), and the accepted branch reads the run back from the store, which is the
  truth for a run this node holds and is where `take_run` has just written it.

- **…and "remembered" has to mean written down.** The same block ends "every token this round spent,
  remembered — even though nothing was confirmed, and *because* nothing was confirmed", and
  implemented it as `publish_run`. A `ClusterView` is in memory and is rebuilt from the store at
  startup, so an arbiter that spent three tokens in a refused round and was then restarted came back
  at the epoch it had started from and handed the same numbers out again — which is the thing that
  block exists to prevent, reached through the one door it did not cover. `record` beside the
  publish, in that order, the same way `hand_over` records a confirmed grant.

  It also removes the systematic half of a worry about `Host::edit_spec`, which publishes the
  *store's* row into the view. The two disagreed for exactly one reason that was not a race: the
  spent epoch lived in the view and nowhere else, so an edit typed after a refused round rolled the
  view back to it. What is left is the microsecond between `absorb` releasing the view lock and
  `record` writing the row, on another thread, for the same run — self-healing at the holder's next
  gossip, since `merge_run` prefers the holder's record. Checked, and not a finding.

- **A run nobody can take costs one bid round every thirty seconds.**

ADR-0049 hands a failed run back to the fleet when the holder's owner will not have it host. The
obvious worry, written down as a residual rather than guessed at: the fleet places it, this node
bids, its bid is refused for the same policy, and round it goes — for as long as the laptop is on
battery.

Staged on two daemons: `alpha` hosting, its link turned metered under it by the fake `nmcli`; and
`bravo` **enrolled but not granted `host-runs`**, which is the cheapest way to have a peer that
holds a replica and can never take the run — no shortened `PROBATION`, no second grant. The run
fails on alpha, is handed back, and lands in a fleet where nobody will have it.

Measured over two minutes:

```
17:04:16.244  still nobody for it  refusals=2  offering=LetGo { by: alpha }
17:04:46.406  still nobody for it  refusals=2  …
17:05:16.574  …
17:05:46.717  …
17:06:16.901  …
```

One round per **30.2 seconds**, which is `REASSIGN_RETRY` exactly; nothing at INFO in three
minutes; and `explain` at six minutes still reporting **epoch 2**, the epoch the handover spent.
So a refused round costs a canvass and no token — 120 rounds an hour, ~960 over an eight-hour
night, each two messages per peer.

`may_retry` is the rate limit and it is deliberately the same one a reassignment uses: *a fleet
that had no room a second ago still has none.* Nothing needed building, which is the outcome a
residual is measured for — and the shape of the check is worth keeping: **count the rounds, then
check the epoch.** A retry loop that is cheap in messages and expensive in epochs is the one that
would have mattered, and it is not visible in a log line count.

`offload explain` is where the answer lives for a person, and it says it per node rather than in
aggregate:

```
what each node says about taking it, asked just now:
    fedora       not granted host-runs by the fleet
    alpha        network is metered and policy disallows it
```
