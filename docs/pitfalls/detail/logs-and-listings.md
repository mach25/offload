# Logs and listings — full entries

The working rules are in `../logs-and-listings.md`, in this same order. These are the entries
they were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A hook with two callers has two meanings, and its doc comment will name one.** `Host::record`
  says "the arbiter calls this once a peer has taken the run" — true, and `Cluster::learn` also
  calls it for **every** record a gossip merge moved, which is how a node learns about work it has
  nothing to do with. An audit row for a *grant* written there therefore fired once per merge: an
  ordinary two-node run produced three rows saying "granted at epoch 1", which is precisely the
  audit log's signature for one arbiter having spent a fencing token twice — a false report of the
  worst failure in the design, on a run where nothing went wrong. And the same hook's other
  caller is why the row was *missing* in the commonest case: a grant to this node returns before
  `record` is reached, so on a fleet of one no grant was ever recorded at all. Over-reported where
  it must be exact, absent where it matters most, from one hook. `Host::granted` is the decision
  and `Host::record` is the record. Found by running two daemons and reading `offload audit`, which
  no test was going to do — the storm property that guards it now needed the bug pointed out first.

- **A refusal recorded on the ordinary path is not a refusal any more.** The audit log's
  `Refused` rows are the ones somebody reads when they suspect two agents on one repository —
  "every one of them is a moment when [that] was prevented rather than merely unlikely" — which
  is a property of how *rare* they are, and every graceful drain wrote one. The departing leg is
  superseded a moment after handing its run over, so `describes` is false by the time the leg
  ends, and the end-of-leg worktree summary recorded a fence: `refused: this node held epoch 1
  and tried to describe it`, one line under the `superseded` row that says the same event
  correctly. Closing a laptop is the path this product is named for, and it was reporting the
  worst failure in the design. Not being allowed to describe a run is a refusal only when the leg
  **believed it still could**: a superseded one was told, and wrote that down itself
  (`Halt::Superseded` says "nothing here is written down"); a checkpointed one handed the run
  back. `Stop::Finished` is the case that keeps the row, because there the run moved out from
  under an agent that was still finishing. The same false row is why `describes` exists at all —
  its doc comment says so about a *finished* run, and this was that mistake one case over.

- **Three logs, and the third is what a machine *did*.** A run's log is the run's output, served
  from whoever holds the run — so a line written by a leg that has just been fenced out is a line
  nobody reads, because that node is by definition not the holder. The fleet log is about
  membership. The **audit log** (`offload_core::audit`, `offload audit`) is per-node, never
  gossiped, and holds the three things nothing durable recorded before: every assignment, with
  the epoch it was made under; every write this node was **refused**; and every run the fleet took
  *away* from it — a leg being told it lost is not a refusal, because nothing was attempted, and
  the row carries the turn it had reached, because that is the number a person watched and the one
  that stops being the run's. Not gossiped for
  `fleet_events`' reason — two nodes recording one grant are not disagreeing, they are each
  describing what they saw, and merging them would need an owner for an event neither owns. It
  outlives the runs it describes (no foreign key), because the rows worth reading are about a run
  this node has stopped holding. Deliberately **not** metrics: nobody scrapes their phone, and a
  counter saying three epochs were rejected today answers none of the questions people ask.

- **A displayed run id names a *tick* when the id was derived, and twelve characters of it name
  two runs for ever.**

  `RunId::short` shows six bytes, and `id.rs`'s own comment says why: a `RunId` is a UUIDv7 whose
  first six bytes are a 48-bit millisecond clock, so four bytes are "constant for 65_536 ms" and
  six is the whole clock. That comment ends on the lesson it was written to record — *"the one
  thing `RunId::short` had to learn: an id that names two things."*

  `Schedule::occurrence_id` (ADR-0056) reintroduced it from underneath. An occurrence's id is
  derived so that two nodes firing one tick converge on one *record*: bytes 0..6 are the tick's
  milliseconds and the rest is a digest of the schedule id. Both halves are deliberate and both
  are right. The consequence nobody took is that the six bytes `short` displays are now entirely
  the **tick** — so two *different* schedules that share a boundary produce two different runs
  whose displayed id is character-for-character the same, on every shared boundary, for ever.
  `every 1m` beside `every 2m` is one every other minute.

  Measured on one daemon in four minutes, `[[tasks]]` firing a script that exits 0:

  ```
  $ offload ps --all | awk 'NR>1{print $1}' | sort | uniq -c | sort -rn | head -3
        2 01a08f52bf80
        1 01a08f53a9e0
  $ sqlite3 state.db "select lower(hex(id)) from runs where lower(hex(id)) like '01a08f52bf80%'"
  01a08f52bf80790aa2b9ddbb2a50abbd
  01a08f52bf80744b8e8aff3d897b37d6
  $ offload logs 01a08f52bf80
  Error: state store: no run matching `01a08f52bf80 (ambiguous — use more characters)`
  $ offload explain 01a08f52bf80
  Error: more than one run matches 01a08f52bf80
  ```

  Both resolvers refuse rather than pick one, which is the correct and important half — no wrong
  run is acted on. What was wrong is everything an operator can see: the listing printed an
  identifier that every command rejects, and told them to lengthen an id the screen holds no more
  of. Two spellings of that refusal, neither actionable, which is also the "each site is written
  by somebody looking at a different screen" entry above arriving again.

  **Session nineteen found this and left it on purpose**, and was right at the time: the flake was
  a test using `short()` for a lookup, and the product half was judged "a papercut — the daemon
  can print an id that a person cannot then paste back", with the note that *"printing fourteen
  characters would reach one random byte and make it a 1-in-256 papercut instead, which is a worse
  kind of rare."* That argument depends on the tail being random. It was, for forty-eight
  sessions. ADR-0056 removed it and nobody re-took the measurement — the same shape as an
  inherited `[x]` on the roadmap, and the reason `CLAUDE.md` says a record can be wrong in the
  direction of confidence.

  Three changes, and the first is the one that matters:

  * `offload ps` takes the width from **its own rows** (`id_width`) — the shortest whole number of
    bytes at which everything on screen is distinct, never below twelve. An ordinary listing is
    unaffected and still twelve; the colliding pair is fourteen. Whole bytes because half a byte
    names nothing, and one number for the header and every row, which is the mistake the `ID`
    const was introduced to fix and is now computed rather than fixed.
  * both ambiguity refusals **name the candidates in full**, so an id that came from somewhere
    other than `ps` — a notification's `OFFLOAD_RUN`, a log line, a colleague — is still a line to
    copy rather than a dead end. `resolve_run` selects three so "or others" can be said honestly.
  * `offload explain` prints the id **whole**. It had been re-collapsing to twelve the id somebody
    had just typed in fourteen, so the header of each of the two explanations was identical.

  Pinned by a test on the two real ids above, including that the ordinary case does not widen.
  And the backticks in those messages moved off the whole string and onto the needle: the ids an
  operator is now meant to copy were inside a span that began before them.

- **A width computed for one listing is not a width.**

  Two schedules sharing a tick boundary, one daemon, `every 1m` beside `every 2m` firing a
  `[[tasks]]` entry. `offload ps --all` widens, which is session seventy-four's fix working and
  is the control that proves the collision is real:

  ```
  RUN              STATE         KIND
  01a090304e4070   completed     task
  01a090304e4075   completed     task
  ```

  `offload audit`, on the same two runs, at the same moment:

  ```
  2026-09-11 11:18  01a090304e40  granted to 6b6d8a5b at epoch 1
  2026-09-11 11:18  01a090304e40  accepted here at epoch 1
  2026-09-11 11:18  01a090304e40  granted to 6b6d8a5b at epoch 1
  2026-09-11 11:18  01a090304e40  accepted here at epoch 1
  ```

  Four rows, one id, two grants and two accepts **at the same epoch**. Read it as what it appears
  to be and it is an arbiter that granted one run twice — the thing `docs/pitfalls/fencing-and-
  epochs.md` is about, the thing this log was built to surface, and the reason somebody opens it
  at all. It is two ordinary runs and a rendering. And the id it prints is then refused by every
  command that takes one:

  ```
  $ offload audit 01a090304e40
  Error: more than one run matches 01a090304e40 — 01a090304e407095b074047456d6a7bc or 01a090304e40752fbb9fd4afa00ae5cd
  ```

  The refusal naming both candidates is seventy-four's other fix, so this is not a dead end — but
  the *listing* still cannot be read, and the refusal only arrives if somebody thinks to type it
  back rather than believing what they just saw.

  The fix is `id_width` over its own rows: the same function, one call site further on. Re-walked
  on the live collision, the merged block becomes two runs with one grant and one accept each.

  What is worth keeping is not the fix. Session seventy-four found the defect in `offload ps`,
  measured it there, fixed it there, and wrote an entry saying the width "is now a fact about the
  listing" — singular, and true of the listing it was looking at. `id_width` had exactly one caller
  for two sessions. **When a rendering rule turns out to be wrong, the question is not how to fix
  the screen you are looking at; it is which other screens render the same fact.** One `grep` for
  `short_id(` answers it in ten seconds and would have answered it then.

  Residual, deliberately: `offload audit <run>` filtered to one run prints twelve, because the
  width is a fact about the rows shown and one run's rows are distinct at twelve. That is
  internally consistent and it is a smaller version of the same trap for anyone copying the column
  onward rather than the argument they typed. Nobody has met it; the fix if they do is to stop
  printing a column that repeats the argument.

- **…and an id printed to be *typed* is not a column, so it has no width to trade.**

  Four places print `offload approve <run> <tool_use_id>`, and they disagreed about how much of
  the id to print — the *"each site is written by somebody looking at a different screen"* entry
  for the third time, now with a correctness consequence rather than a wording one:

  | site | what it printed |
  | --- | --- |
  | `offload logs` | what the operator typed — right by construction |
  | `offload asks` | `short_id` — twelve |
  | `offload explain`'s `waiting` line | `short_id` — twelve |
  | the drain's `DrainStep::Blocked` | `RunId::short` — twelve, chosen on the daemon |

  `offload explain` is the sharpest of the three: session seventy-four made its **header** print
  the id whole precisely so that an id somebody typed in fourteen characters was not collapsed
  back to twelve, and the instruction four lines below it kept the abbreviation. One screen, two
  widths, and only the top one is guaranteed to resolve.

  All three print it whole. There is nothing to trade: an instruction line is not a table, it has
  nothing to line up with, and a pasted command that the command it names then refuses is the
  worst outcome available. The drain's is fixed on the daemon rather than in the CLI, because that
  is where the string is chosen.

  **None of it is reachable today**, and the reason is the part worth having, because nothing in
  the tree states it: derived ids come only from `Schedule::occurrence_id` (rules use
  `Uuid::now_v7`), and `offload every` has no `--ask` — clap refuses it, measured. So a run that
  can be blocked on a question can never have an id that names a tick. That is a coupling between
  two features that know nothing about each other, held by the absence of one flag, and it was
  found by asking *which report prints this command's argument* rather than by meeting the bug.

## An unmarked wall-clock time is read as the reader's own, and `when` renders UTC

`commands::when` turns a unix millisecond into `YYYY-MM-DD HH:MM` with the usual civil-from-days
algorithm, straight off the millisecond. That is **UTC**. Its doc comment said *"A local timestamp
for a person reading a list, rather than a unix millisecond"*, and the column carried no marker of
any kind — no `Z`, no offset, no `UTC`.

Measured on this machine, which is CEST. A run submitted at **15:20:19** local:

```
$ offload audit
2026-09-12 13:20 UTC  01a095c6a6dc  accepted here at epoch 1     ← after
2026-09-12 13:20      01a095c6a6dc  accepted here at epoch 1     ← before
```

Two hours out, in the two commands whose whole purpose is placing an event in time after the fact:
`offload audit`, read beside the daemon's own `tracing` output when somebody is working out what
happened, and `offload nodes --history`, ADR-0012 mitigation 4's durable half. Somebody asking *did
anything happen around three* reads 13:20 and concludes nothing did — a silence, which is the
failure mode this file is mostly about.

Found by reading the function while walking `offload audit`, and then typing `date` beside it. The
doc comment is the part worth carrying: it asserted the one thing the code did not do, which is
exactly what stops a reader checking. A comment saying "local" is a claim, and this tree treats a
doc comment in the past tense as not-evidence for the same reason.

The fix is a label rather than a conversion, and that was a decision. There is no timezone in
`std`, so local time means a dependency (`time`, `chrono`, or `libc` for `localtime_r`) in a crate
that has none of the three, or parsing TZif by hand — and each then has new ways to be wrong (no
tzdata in a container, `TZ` unset, DST boundaries) where a label cannot be. The marker also makes
the column directly comparable with `offloadd`'s log lines, which are already `Z`-suffixed UTC and
are the other thing open on the screen. If somebody wants local time later, the dependency is the
decision and `when` is the only place it lands.

## A listing that stops at its limit has to say so, and this one said the wrong incompleteness

`offload audit` requested `limit: 100`, printed what came back, and closed with *"What this device
did, not the fleet's — ask the others too."* That sentence is about a real gap — the log is
per-node by design, so a fleet's answer is the union of every device's — and it is **not the gap in
front of the reader**. Measured with sixty submissions against one daemon: exactly 100 rows, the
output simply stopping, and nothing anywhere saying that this node had more.

The command's own `--help` made it worse by promising the opposite: *"One run, by id or prefix.
Omit for everything this node has recorded."* The unfiltered listing has never shown everything.

Two halves to the fix. Ask for **one more than you show**, so that "there are older rows" is a fact
rather than a hedge — with `limit: 100` the two cases that matter (a node that did exactly this
much, and a node whose older rows were cut) are indistinguishable, and this is a log read by
somebody with a specific moment in mind. And say the *other* thing too, since the sentence that was
already there addresses a different incompleteness and reads as though it were the only one.

`offload nodes --history` had the identical cap and got the identical treatment. It matters more:
it is the log somebody opens to find out whether a device was ever enrolled without their
knowledge, so a listing that silently stops is answering that question with the wrong half of its
evidence.

## The durable copy of a fact was the vague one

`AuditEvent::Rescued`'s rendering read, verbatim on screen:

```
2026-09-12 13:23  01a095c903c7  kept an earlier leg's checkout here: it holds uncommitted work,
                                so it is at <state>/worktrees/<run>.superseded*, and nothing
                                will remove it
```

The run's own log, for the same rescue, printed the real absolute path — and that is the log which
does not survive. `Rescued`'s own doc comment says so: the leg that did the rescuing is often not
the node anybody can read the run's log from, and once the run is deleted nobody can. The audit row
is the copy that lasts, and it was the one with the holes in it.

The id is the part that stings. It is on the audit row already, abbreviated to twelve characters by
`id_width`, and the directory is named by the **full thirty-two** — so a reader who did understand
the placeholder and substituted what was in front of them got a path that does not exist. Measured:

```
$ ls -d .../worktrees/01a095c903c7.superseded*
ls: cannot access ...: No such file or directory
$ ls -d .../worktrees/01a095c903c77b13a7ef728245b2c39f.superseded
.../01a095c903c77b13a7ef728245b2c39f.superseded          ← the uncommitted file is in here
```

`run` is in the variant, so it is one interpolation. The **state directory** is not and cannot be —
`offload-core` has no filesystem and no config — so it is named in words (*"under this node's state
directory"*) rather than spelled as a token: a reader who knows their state directory can finish the
path, and one who does not has a phrase to search for instead of an angle bracket.

**How the row was staged**, because it is not obvious and the ping-pong is expensive. `adopt` reads
a turn marker beside the worktree (`<state>/worktrees/<run>.turn`), and answers `Superseded` when
the marker is behind the checkpoint. So: run, `offload checkpoint` to park it, write a file into the
worktree that git does not know about, lower the marker by hand, `offload resume`. That is exactly
the shape a three-leg migration produces, arranged in twenty seconds on one daemon. The clean
variant — same thing without the uncommitted file — gives `Reclamation::Redundant` instead, which is
the control.

## …and the same shape, a third time, in the command whose comment said it could not happen

The sweep that found the entry above — *grep for every site that renders it before writing the fix*,
this file's own rule — turned up one more, in `offload logs`:

```
run released — resume it with: offload resume <run>
```

Forty lines away in the same `match`, the `Asked` arm carries this comment: *"The run, spelled,
rather than `<run>`: this is the one line in the log that is an instruction, and it was handing over
a command that could not be pasted."* There were two. The sentence asserting there was one is what
stopped anybody looking, and the fix in session seventy-four was applied to the line that was in
front of somebody. The value was in scope in both arms the whole time.

Three of these now, each in the one line of its output that somebody is meant to copy. What they
share is not a subject — a rescue, a resume, a permission answer — so a reviewer reading the arm in
front of them will not find the next one. What they share is a *shape*, and there is no type that
says "this string reaches a person". Hence `offload-cli/tests/no_placeholders.rs`, modelled on
`offload-core/tests/no_clock.rs` for its reason: it reads the crate's source and fails on a bracketed
placeholder inside a `println!`/`format!`/`write!`. Proved red by putting the line back.

It is a lint and not a proof, which the file says out loud: a placeholder assembled from pieces, or
built in `offload-node` and printed here, goes past it. It catches the way all three were actually
written, which is the useful thing, and the list of bracketed words is deliberate rather than
`any <word>` — `<` is ordinary in prose, and a check that cries wolf gets deleted.

- **`offload status` on the Android emulator blamed the kernel for quinn learning the NIC.** The
  first status of the emulated phone read `sends 1 refused by this machine's kernel — that is not
  the peer's silence` and named `10.0.2.2:7601 · I/O error (os error 5)`. Its log had the cause one
  line earlier: `libc::sendmsg failed with I/O error (os error 5); halting segmentation offload`.
  quinn-udp treats `EIO` or `EINVAL` on Linux/Android as "this adapter cannot do GSO", turns it off
  and carries on. So the line reported a muzzled machine for the rest of the daemon's life, about
  one lost batch at startup on a virtual NIC. `EMSGSIZE` had been excluded for the same reason
  (quinn probing the MTU), and this was the other fall-back quinn names in the same function.
  Mirrored narrowly: only when `Transmit::segment_size` is set, because the same errno on a single
  datagram is a real refusal. Measured after the fix on the same emulator: one `halting
  segmentation offload` in the log, and no `sends` line.
