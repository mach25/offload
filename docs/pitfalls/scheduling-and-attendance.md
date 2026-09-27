# Deadlines, urgency, attendance and Pending

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/scheduling-and-attendance.md`, same order.

- **A follower that goes away has to be noticed, not waited for.** Both follow paths watch the
  client's read half for EOF; otherwise an abandoned follow keeps the run looking **attended**,
  which is the input deciding whether a failed run resumes itself.
- **Attendance is knowable only where the stream is served** — including when served to a *peer*.
  It decides the failure case its observer is present for, never the hold-down, and it is never
  gossiped: a gossiped copy is a value from before the silence presented as current.
- **Urgency is derived, never stored.** `f(deadline, now)` rises on its own, so every node agrees
  without a message. `priority` says who yields.
- **An unspecified deadline means the moment of submission**, so slack is negative age and every
  ordinary run is overdue within a second. Ordering and pickiness use slack whatever its origin;
  **patience** and **telling somebody the deadline is gone** need a deadline somebody *stated* —
  hence `Prospect::NoStatedDeadline` as a variant rather than a comment.
- **A deadline changes when we give up, never what we may do.** It may not weaken fencing, skip a
  checkpoint, snapshot mid-turn or shorten a lease. In tension, the run misses the deadline.
- **A missed deadline is a sentence, not a decision.** `note_overdue` writes `LogKind::Overdue` and
  changes nothing else — said once per stated deadline, in the log rather than only in `tracing`.
- **`Pending` has three entrances and one exit that can refuse.** A released checkpoint, a
  handed-back commitment and a reopened failure all lead there, and `assign` refuses a pinned run a
  second holder — so the guard belongs where `Pending` is *entered*.
- **A bid round moves the run; the caller is still holding the copy it had before.** `place`
  spends epochs and hands the run over, and `server::place` then wrote its own pre-round copy back:
  into the view for a run it had just accepted, and into the store *and* the view for a queued one.
  Measured on a node capped at one run — the second and third submissions in the same second were
  each told they could start **now**. The outcome carries the record now (`Refused { spent }`), and
  the accepted branch reads it back from the store.
- **…and "remembered" has to mean written down.** The refused arm published its spent epoch into
  the view and recorded nothing, so a restart came back at the epoch it started from and handed the
  same numbers out again — the bug that block is named for, through the one door it did not cover.
  A `ClusterView` is memory; the store is what a restart believes.
- **A run nobody can take costs one bid round every thirty seconds, and that was already the
  answer.** ADR-0049 left it as a residual to measure — a laptop that hands a run back for a
  policy reason, on a fleet where nobody else will take it either. Measured on two daemons for two
  minutes: five rounds, one per 30.2s, `still nobody for it refusals=2` at DEBUG and nothing at
  INFO, and **the epoch never moved** — a round that grants to nobody spends nothing (ADR-0046's
  sibling property, from the other end). 120 rounds an hour, ~960 over a night, on a canvass of
  the live fleet. `may_retry`/`REASSIGN_RETRY` is the backoff, and it is the same one a
  reassignment uses because the reason is the same: a fleet that had no room a second ago still
  has none. **Nothing to build**, which is what the measurement was for.
