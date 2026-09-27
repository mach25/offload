# Tests, sweeps and checks that guard a rule — full entries

The working rules are in `../testing-and-sweeps.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A fixpoint is not "one pass changed nothing".** SWIM probes one peer per period, so a node's
  opinion of any particular peer is revisited once every `peers` rounds — which makes two
  identical consecutive passes the *ordinary* state of a fleet mid-recovery rather than evidence
  it has settled. Written the eager way, `storm.rs`'s `settle` declared a fleet converged at round
  1 while a node it had marked `Dead` came back at round 4 and everybody agreed by round 5. It
  waits a whole rotation of quiet now. Same lesson `churn.rs` learned from the other end — its
  first version stopped when the run record stopped moving and called a fleet still arguing about
  liveness converged.

- **A simulation that breaks the rule it is modelling reports the product as broken.** `storm.rs`
  let any node call `place`, and found a run granted at epoch 1 to a node that already knew about
  epoch 3 — real, and reachable only because an epoch is monotonic **per arbiter** and two
  arbiters are two counters. `arbiter_for` is the rule, so the simulation has to honour it; two
  earlier versions of the same mistake had a node publishing runs it had merely been told to
  *record*, and conjuring a second run under one `RunId`. Each looked like an invariant violation
  and each was the harness. The tell is that the counterexample needs a step the daemon has no
  code path for.

- **A property test checks a sentence; a unit test checks a case.** Which is why the two live
  the way they do here: `no run lost across transitions` and `fencing rejects the second
  writer` are claims about *every* sequence and *every* pair, and nothing that arranges a case
  was ever going to check them. Everything the properties have found so far was a rule written
  down in one place and honoured only there. `tests/properties.rs` in `offload-core` drives one
  run and one view; `tests/churn.rs` drives a fleet of them — separate views, gossip,
  supervision passes, crashes — and the counterexamples proptest shrank are checked in beside
  both. Two habits earned there: **aim a property at what the tree claims**, not at what might
  break, and **revert each fix to watch its property go red**, because a property can pass for
  the wrong reason (the churn sim's first version stopped gossiping as soon as the *run record*
  settled, and reported a fleet still arguing about who was alive as converged).

- **A sweep for pub items nothing calls is worth repeating; it found three in one pass.** Mechanical
  and cheap: list every `pub fn` in a crate, count references outside its own declaration and test
  modules, read what is left. 7 of 334 in `offload-core`, 19 of 384 elsewhere, and reading them
  found a **second start gate** (`WorkPolicy::admits_start`: five tests, no callers, and *no rate
  limit in it*, so the next person to wire a start gate would have reached for the function whose
  name says so and reopened ADR-0029's hole), a **second copy of the absence average**
  (`Store::record_return`, whose own doc comment asked somebody to keep two copies in step), and a
  **whole module nothing called** (ADR-0031). It needs *reading*, not acting on: `serialize` was a
  serde `with =` helper reached by the derive macro, and `is_overdue` is a predicate that exists to
  make assertions legible, which is a legitimate reason for a small public item to exist.

- **…and the same sweep over *enum variants* asks a different question: what can this fleet never
  say?** For every enum, count references in construction position rather than pattern position
  (the discriminator is whether the reference sits before or after a `=>` on its line), exclude
  thiserror's `#[from]`, and read what is left — 16 of 512, mostly constructed by serde or clap.
  It is the mirror of `Refusal::AgentNotAllowed`, which was *enforced* and unsettable: these are
  sentences nothing can produce, and two of them named states their layer can never be in.
  `handshake::Refusal::Draining` is the one to remember — **a draining node needs its
  connections**, since handing runs over is `place()` dialling peers and the fleet only learns it
  is leaving because `NodeStatus::Draining` gossips out of it. A refusal reason naming a state the
  product really is in is exactly what somebody wires up, and wiring that one breaks the feature it
  is named after.

- **An escape hatch named in a doc comment is a feature claim, and this one had no code at all.**
  The `--constraint` grammar ANDs a flat list and pointed elsewhere for the rest: "anything
  genuinely tree-shaped goes in a run spec file". There is no run spec file — that sentence was its
  only mention anywhere in the CLI — so `Constraint::Not` is handled correctly by `matches`,
  `explain` and `failures` and producible by nobody. Left unreachable on purpose (the domain model
  is a boolean tree; what is missing is a *surface*, and a syntax people will live with is a
  decision) and the sentence fixed, because naming a hatch that does not exist is how a gap stops
  being visible.

- **A doc comment in the past tense is not evidence that anything calls the code.**
  `offload-store::observations` opens by describing the restart bug it fixed — and had no production
  reference at all, so every sentence was still true in the present. Same shape as
  `blobs::collect_garbage` being "deliberately manual" and `cleanup` assuming a reader: the file
  argues for itself convincingly and nothing wires it. When a module explains why it matters, check
  the call graph before believing it.

- **Dead code that asserts a mechanism is worse than dead code.** `bid_delay`, plus the two
  `BidWeights` fields that fed it, implemented ADR-0006 step 3 — bids broadcast after a
  score-proportional delay — which the same ADR's implementation amendment replaced *before it
  shipped*, because a transport that addresses peers directly lets the arbiter ask and nodes
  answer. Nothing called it but its own test, and its doc comment described the delay in the
  present tense, so a reader learned something false about the protocol; ADR-0006's own
  consequences still named the delay as what mitigates a bid storm. Deleted, with the reasoning
  kept where the fields were, because the next person to want a delay should have to make the
  decision rather than find the field.

- **A machine-wide ledger makes the test suite depend on the machine.** The device reservation
  ledger is per-user and per-machine *by design* (ADR-0013: one laptop, two fleets, one set of
  slots), so `Supervisor::new` finds the real one — and every capacity test read whatever
  `offloadd` happened to be running on the machine executing the suite. Four failed whenever the
  demo was up, `two_commitments_on_a_one_slot_node_do_not_block_each_other` reporting `left: 0,
  right: 1` about a slot an unrelated agent held, and the obvious suspect is whatever you just
  changed. It runs the other way too: a test that drives a run *reserves* against that ledger, so
  `cargo test` could tell a live daemon its machine was full. `Supervisor::with_private_ledger`,
  the sibling of `with_home` and for the same reason — a test must not be quietly handed the
  state of the machine it happens to be running on.

- **A check scoped by a claim about where a mistake matters is scoped by a claim with a date on
  it.** `offload-node/tests/messages.rs` looked in one crate because "the daemon's refusals are the
  longest sentences in the workspace" — and `Escalation`'s reasons live in `offload-core`, render
  straight into the daemon's log, and one shipped with **thirty spaces** in the middle of it. It
  walks `crates/*/src` now, and immediately found a second hit in `offload-agent`. Both were written
  in the session that widened it, by the very route its module docs describe, and neither was found
  by the check — one was found by reading a walk's output. It also asserts it found at least eight
  crates, because a walk looking in the wrong place passes by finding nothing, which is how a check
  like this dies quietly. Contrast `no_clock.rs`, which is *rightly* one crate: that rule is a
  statement about `offload-core`, and this one is a statement about the workspace.

- **The text is part of the behaviour, and a mangled message is invisible in review.** A string
  literal continued with a trailing backslash drops the newline *and* the indentation — but a tool
  rewriting source through a language whose own strings treat backslash-newline as a continuation
  (Python's do) keeps the indentation, baking a run of spaces into the literal. Three of the
  daemon's operator-facing refusals printed with twenty-space gaps mid-sentence, including the one
  this file holds up as the model of a good refusal. Nothing is wrong with the code and the diff
  looks like formatting, so it is only wrong when printed — which is why it is checked
  (`offload-node/tests/messages.rs`): four or more spaces beginning twenty characters into a
  literal. Deliberate column padding sits near the front, behind a short label, which is what
  makes the two separable. **It has now fired on three separate sessions' worth of new strings,
  including twice on the session that widened it** — most recently `every other node          —` in
  `offload drain`'s own report, written the same way for the same reason. That is the argument for
  it being a test rather than a habit: the failure is invisible in review, the diff looks like
  formatting, and the author is the person least able to see it. A literal that wants two lines is
  written as two literals.

- **A hand-written fixture can agree with the code about a world neither lives in.** The GC test
  inserted `{"transcript":"<hex>"}`, a shape `save_run` has never produced, and passed for two
  phases while the real path deleted work. It is the reason nobody looked. Build fixtures through
  the writer the daemon uses — `save_run`, not an `INSERT` — or the test is checking your idea of
  the schema against your idea of the query.

- **A test double that models one end of a two-ended thing tests one end.** Session forty-two made
  `MemoryConnection::close` do something, having found that a `close` which did nothing could not
  tell a caller that *hung up* from one that merely *forgot*. Each end still held its **own**
  `AtomicBool` and its own `Notify`, so closing one end left the other perfectly able to `open` a
  stream and go on gossiping — which is not what QUIC does, and is the same failure shape one layer
  down: the half that mattered was the half nothing reached.

  Found by writing ADR-0044's test. A revocation staged the obvious way — connect, revoke, hang up,
  watch the subject learn — **passed with the fix removed**, because the subject's cached outbound
  session survived the peer's `disconnect` and the peer's reply gossip carried the revocation like
  any other. The test was measuring the path the fix does not touch. Revoking *before* the two have
  ever spoken is what isolates it: there is no session to carry anything, and the only contact the
  subject has with its fleet is the dial it is refused on. The test then goes red without the fix,
  which is the point.

  The double is faithful now — `closed` and `hung_up` are `Arc`s created in `dial` and handed to the
  accepting end through `Incoming` — and the whole suite still passes, which is the other half of
  the answer: nothing was relying on the leniency.

- **A test harness that cannot express the staging is why nothing caught it.**

  `Store::open_memory` puts the connection in memory and the *blobs* on disk, because a blob is a
  file under `root/blobs`. Its root was `std::env::temp_dir().join("offload-memory-store")` — a
  constant. So every in-memory store in a process shared one blob directory, and the directory
  outlived the process.

  Two consequences, and the second is the one that cost two sessions. A blob written by one test
  was visible to a different test, in a different crate, in a later run. And `has_blob` stopped
  being a question about *this* node — so **"a node that does not have this checkpoint's blobs"
  was not a staging the harness could express**, which is exactly the migration `Supervisor::resume`
  failed on, and exactly why nothing in 900 tests noticed for as long as `resume` has existed.

  The tell was there to read: three tests — `resuming_a_failed_run_reopens_it_at_a_fresh_epoch`,
  `two_resumes_at_once_start_one_agent`, `a_failed_run_picks_itself_up_only_if_nobody_was_watching`
  — resumed from a checkpoint naming `BlobHash::from_bytes([3; 32])`, a hash of nothing, stored
  nowhere, and passed. They pass now because they say `checkpoint_here(&store, n)`, which puts the
  transcript where a resumable run really has one; a test that wants a *missing* blob has to say
  so, and two now do.

  The root is `<tmp>/offload-memory-store/<pid>-<n>` now. Nothing removes it, exactly as before —
  the change is isolation, not hygiene. **Ask what the harness makes unsayable.** A rule that
  cannot be violated in a test is not a rule the tests check.

- **An assertion can name an invariant it is structurally unable to check.**

  Session fifty-seven's handoff proposed one deliberate pass over the checkpoint tests asking
  *what is set to `None` here, and what would `Some` do*. This is what it returned.

  **The hit.** `a_checkpoint_is_copied_off_this_machine_and_the_run_records_where` (ADR-0016's
  test) does:

  ```rust
  let checkpoint = checkpoint_at(3);
  let blobs = checkpoint.blobs();
  sup.replicate(id, checkpoint);
  assert_eq!(lock(&asked).clone(), blobs, "every blob, not some of them");
  ```

  `checkpoint_at` sets `bundle: None, patch: None`, so `blobs` is one element and the assertion
  compares `[transcript]` with `[transcript]`. It cannot fail, and its message claims the exact
  property it cannot check. Measured rather than assumed: changing `Supervisor::replicate` to
  `peers.replicate(run_id, vec![checkpoint.transcript])` passed **`cargo test --workspace`, exit
  0, 909 tests**. The invariant that would have been gone is the one `mesh.rs` states in a
  comment — *"All or nothing: a peer holding two of three blobs cannot materialise the run, so
  recording it as a replica would make `is_durable` a lie"* — enforced, until now, nowhere. The
  fixture takes a bundle and a patch now, and the same revert fails it.

  **The second, smaller one.** `Capture::summary` — `"1442 byte bundle + 146 byte patch (1
  untracked file(s))"`, the only sentence an operator is ever told about what a capture carried —
  had no test at all; nothing in the workspace asserted on `byte bundle`. It is also the line the
  two defects of sessions fifty-six and fifty-seven were both read off. Its sibling
  `Selection::summary` already has a pitfall entry for naming the wrong reason. Four shapes and
  the `has_work` agreement are pinned now.

  **What the sweep cleared, which is half of its value.** `Checkpoint::blobs()` is exercised with
  all three blobs in `bid.rs` (`a_checkpoint_names_every_blob_a_receiver_needs`), so the root is
  solid and it was a *consumer* that was unchecked. The blob collector is genuinely covered:
  reverting `referenced_blobs` from `checkpoint.blobs()` to `checkpoint.transcript` fails
  `gc.rs::a_blob_a_run_references_is_never_collected` — which was the sweep's leading hypothesis
  and was wrong. And the `bundle: None` in the `mesh.rs`, `explain.rs`, `view.rs` and `run.rs`
  fixtures is irrelevant to what those tests assert: drain semantics, reporting, merge.

  **The method matters more than either finding.** Both were inferences from reading until the
  revert was run, and the ratio was one right to one wrong. `cargo test --workspace` against a
  deliberately broken line is two minutes and is the only thing that separates a guard from a
  decoration.

- **A race test that asserts after both racers have joined is asserting an ordering nothing
  promises.**

  `a_rebuild_never_adopts_a_checkout_a_teardown_is_removing` is ADR-0052's forced pair: `prepare` a
  checkout, spawn a teardown, sleep `delay` ms, spawn a rebuild, join both, then assert the
  rebuild's checkout is on disk. It failed about **1 run in 5 at load 6.09** and **0 in 14 idle**,
  and three sessions running attributed it to the machine — each from a single red run, none of
  them counting.

  Counting made it worth explaining; **instrumenting explained it in one reproduction**. The task
  now carries out which side of the guard it landed on, and the failure reads:

  ```
  at delay=0ms the rebuild handed back a checkout that is not there
      (rebuild adopted, teardown Removed)
  ```

  So: the **rebuild** won `hold(run)`, `adopt` found the checkout present and current, it returned
  and released — and the teardown then took the lock and removed it. The two never overlapped. The
  lock did exactly what ADR-0052 built it to do, and the test then asserted something the design
  does not promise.

  What made that assertion look reasonable is the product, where it *is* true: `drive` holds the
  guard until the run is in `live` with an agent handle, and every teardown door asks about that
  handle — `cleanup` reloads the run and refuses a non-terminal one, `reclaim_departed_checkouts`
  asks `reclaimable` twice. The staging has no such handle; its teardown is `remove` with nothing
  in front of it. A rebuild that wins the lock and then releases into *that* is asking to have its
  checkout deleted.

  The fix is to ask the question where the guarantee holds: the intact-check moved **inside** the
  rebuild task, before the hold is dropped. **0 failures in 60 runs at load up to 10.7**, against
  2 in 10 before. It keeps its teeth — without serialisation the removal overlaps the check rather
  than following it, which is the 10-of-10-within-2ms case it was written for — and it gained an
  assertion that the teardown removed *something*, since a teardown that found nothing would have
  raced past what it was meant to tear down.

  **Two rules, and the second is the cheaper.** A race test's assertion has to live inside the
  window whose invariant it is testing, not after it. And when something is flaky, spend the one
  line that makes the next failure say which case it was — three sessions of "probably load" against
  one reproduction that answered outright. Same move ADR-0051 made for `register`.

  Residual: **one unexplained failure in ~80 post-fix runs**, whose message was not captured. If it
  returns it now names its own case.

- **The `bool`-shaped-answer grep is the wrong test, and running it is how that was established.**

  Session seventy-eight produced four findings of one shape, across three subsystems:

  - `FleetState::adopt` returned `bool` for "did I take this certificate", so a narrowing
    certificate had nothing to word a refusal with (ADR-0062).
  - `Cluster::push_blob` returned `bool` for "did the peer take it", flattening *already held*
    into *over the size limit*.
  - `Peers::replicate` returned `Option<NodeId>`, so a fleet of one, a refusing peer, a transport
    error and an unmovable blob were the same `None` — and the operator got one guessed sentence.
  - `Asks::answer` filtered on the run **and** the `tool_use_id` in one chain, so a mistyped id got
    the sentence for a run with no question. Same defect with no `bool` anywhere in it.

  The obvious generalisation is "grep for `-> bool` on anything that refuses", and it was written
  into the handoff as a job worth doing. Doing it is what showed it does not work. Outside tests
  there are **83** `-> bool` functions; the candidates that are not plainly predicates
  (`is_`/`has_`/`can_`…) are `heard_from`, `set_running`, `set_capabilities`, `refresh`, `absorb`,
  `probe`, `probe_indirect`, `peer_has_blob` and a handful of others — and every one is either
  *"did this change"* or internal control flow with no operator-facing sentence hanging off it.
  `Probe::refresh` is the clearest: its doc comment already says the `bool` "is only worth a log
  line" and that whether to gossip the change "is `Cluster::set_capabilities`' own question … and
  this must not become a second answer to it." A grep would flag it; the code is right.

  So the shape is not a return type. It is: **an operator-facing sentence downstream has to state a
  cause, and the function that knew the cause answered whether instead of why.** The tractable test
  runs the other way — start from a sentence a person reads, and walk back asking whether what it
  claims was *measured* or *guessed*. All four were found that way: three by walking a command and
  reading the whole screen, one by reading the call site of a walk's finding. None by reading a
  signature.

  Worth keeping beside seventy-seven's identifier sweep as the other outcome: that one was a `grep`
  that found a defect in ten minutes, this one is a `grep` that was worth running precisely because
  it came back empty and retired an idea that reads well.

*The fourteen entries below were backfilled in session ninety-one from `docs/sessions.md` and the
commits that added each rule — sourced, not reconstructed from the rule text. Seven rules in this
file still have no detail entry because no source beyond the rule and its commit diff was found:
the dead-theories group from commit `bd1a5f6` (a theory about local code needs no remote machine;
the existing test's workaround was the tell; an intermittent failure makes each observation nearly
worthless), the log-filter group from `0bf9dcf` and `72a6356` (check the filter before believing a
zero; the remedy filter caused the fifth and sixth; a remedy in a doc has a shelf life), and "hold
the instrument constant across arms" from `91a3581`. Their commit messages are the best record.*

- **A guard that fails for the right reason is still a flaky guard.**

  Session thirty-four, the checkout sweep's guard test (commit `20221ed`). The racing form that
  found the defect — a 1ms pickup against the 3ms `git status` of a two-file fixture, forty
  attempts — measured 40/40 before the fix and passed 10/10 alone and 24/24 under synthetic CPU
  load after it. It then went red **2 times in 8 full-suite runs**: under real parallelism the
  sleep can outlast the subprocess, and the sweep removes the checkout *correctly*. Isolated
  repetition and synthetic load both said it was fine; only running the whole suite repeatedly
  found it. Flaky guards get deleted, whatever the reason they fail.

- **…and making it deterministic can quietly stop it testing anything.**

  Same test, second attempt. Asserting `reclaimable` before the pickup and after it stays green
  **with the fix removed**, because the defect is that state changes *inside* the loop, and a test
  that changes it beforehand only ever exercises the first call. Caught by deleting the fix and
  re-running — two minutes, and the only thing that separates a guard from a decoration.

- **Widen the window instead of abandoning the race.**

  Same test, third attempt, the one that stayed. `git status` re-hashes a file whose mtime has moved
  and still reports the worktree **clean**, so rewriting a 16 MB file with identical bytes before
  each attempt buys **83ms** against the 1ms pickup instead of 3ms. Red with the fix removed, green
  across nine full-suite runs, three of them under full CPU load.

- **…and where there is a trait at the seam, gate it rather than widen it.**

  Session thirty-five (commit `53bcb40`), a cancelled run that came back as failed and then as
  resumable. The cancel test gates `Peers::fetch`: because it is a trait method, a stand-in can
  signal that it has been entered and then wait for the test to release it, which *arranges* the
  race exactly on a current-thread runtime with the whole real loop still running. The cheaper
  cousin of session thirty-four's widened window, and available wherever the seam is a trait
  rather than a subprocess. Its partner, the grant test, was checked the same way from the other
  side — red with the lower-epoch arm disabled, confirmed by disabling it.

- **A control that does not change the thing it controls for retires a live hypothesis.**

  Session sixty-five, the Linux↔macOS mesh hunt (commit `0bf9dcf`). The 4-tuple-collision theory
  was "tested" by moving the Mac's port to break the tuple symmetry — which could not work, because
  both daemons dial from their own listening port, so the tuple was unchanged. The theory was
  recorded dead on that control. The same day it had looked causal on one observation each way and
  then measured 1 success in 5 against 0 in 4, which is nothing. Session sixty-six's GRO experiment
  was written down with a control that does move its variable (`ethtool -K <nic> gro off`), for this
  reason.

- **A retraction has to land everywhere the claim was written, and it usually does not.**

  Session sixty-five (commit `b98a573`). After the 4-tuple claim was retracted and the `docs/DEMO.md`
  section rewritten, a trailing paragraph in the same file still asserted it — found only while
  writing the session's notes. A retraction that lands in one place leaves the dead theory alive
  where somebody will read it; grep for the claim's distinctive words before calling it retracted.

- **A diagnostic needs a positive control on a path known to work, or its zero means nothing.**

  Session sixty-five's last turn (commit `19d7c71`). `examples/gro_probe.rs`, written to test the
  UDP_GRO hypothesis, reported **0 of 16 datagrams over a path carrying all 16** — the reported
  symptom exactly, on one machine, with no Mac. For about ten minutes it looked like the find of the
  session. The control that caught it was the same probe over **loopback**, a path already known to
  work, which also reported 0. The comparison had drifted a second way too: the quinn receiver and
  the python one never once ran on the same port. The fifth false zero in one investigation, and the
  first from an instrument built to end it: *when a new tool and a real bug have the same signature,
  assume the tool.*

- **A socket option can disable the timeout you set on it.**

  The mechanism behind the entry above. `UdpSocketState::new` makes the socket non-blocking as its
  first act, which silently voids the `set_read_timeout` the probe then waited on. The probe read
  once, got `WouldBlock` before the sender had started, and exited having measured nothing.

- **An instrument that cannot fail is not an instrument, and `quinn-udp`'s `send` cannot fail.**

  Session sixty-six (commit `91a3581`). Fifteen theories had died across two sessions and the record
  had settled on a contradiction — quinn's traffic fails four times in five where raw UDP on the
  same 4-tuple never fails — with instructions to derive the next theory from it. It rested on
  `udp_probe`, which called `UdpSocketState::send`; that returns `Ok(())` for **every** error but
  `WouldBlock`, so the probe counted calls rather than datagrams. Switched to `try_send`, the same
  path measured **0 of 40** where it had measured 10 of 10. Two sessions of reasoning were built on
  an instrument that could only report success. The same property is why ADR-0059 counts refused
  sends rather than trusting the send path to say so.

- **`log_sendmsg_error` is rate-limited to one line a minute.**

  Same session, same family. "13 `sendmsg` errors in 2.5 hours" had been read as a rate; it is
  `quinn-udp`'s log limit of one line a minute, not a count, and the socket had been failing every
  send. A log line's count is a count of *log lines*.

- **A negative result needs a positive control *in the same window* when the fault is
  intermittent.**

  Same session. Isolating macOS 26's refusal of an ad-hoc-signed sender, socket options, syscall
  family, ECN, socket lifetime and the message header were each eliminated with the failing call
  **interleaved** as a positive control — python 30 of 30, an ad-hoc C sender 0 of 30, ad-hoc Rust 0
  of 30, in one loop. It mattered: ten socket-option variants first "passed" 40/40 only because the
  condition had lifted between runs. A control run before or after is a different window.

- **`pgrep -f` self-matches too, not just `pkill -f`.**

  Session sixty-six, third half. `pgrep -f slow.sh` matched the shell running it and made a
  correctly-cancelled task look as though it had leaked its process group — reported as a defect
  before it was checked. The `pkill -f` trap was recorded twice in `docs/DEMO.md` already; the
  read-only sibling is worse, because it does not kill the walk, it misinforms it. Match on `-x`,
  or on a pid recorded at start.

- **A test that reads the machine it runs on is a test whose answer depends on the suite.**

  Session sixty-seven (commit `0fcc290`). The status test first asserted `light work: yes`, which
  passed alone and failed inside the suite: `status` reads the machine's real load average, and
  `cargo test --workspace` pegs the CPU, so `admits` refused under pressure. Correct behaviour,
  wrong assertion — the fix was to ask what the test is *about*, not to add a tolerance.

- **…and two `cargo test --workspace` runs at once share `target/`, so one of them lies.**

  Same session. A workspace run that overlapped another reported `can't find crate for
  offload_transport` in a doctest — impossible, and it was: the two runs share `target/`, and the
  concurrent build had replaced the `.rlib` rustdoc was handed by name. Serially, clean. It also
  exposed that `grep -c FAILED` over a captured run does not see a doctest failure at all, so **the
  exit code is the thing to read**.

- **The weights walk's first arm was void, and the pause that fixed it stopped the wrong process.**
  Session ninety-two ran the three hard property runs in the background while walking ADR-0063 §2
  on two machines. `offload explain` then showed the laptop refusing (`cpu at 100%`) rather than
  bidding low, and the Mac winning arm A for that reason, not for the one the arm was testing. The
  pause was `P=$(pgrep -f 'deps/churn-' | head -1); kill -STOP $P`. `pgrep -f` matched the bash
  process running that line first, so the tool call's own shell was stopped and hung until it was
  found with `ps -eo stat | awk '$2 ~ /T/'`, continued and killed (exit 144). It is the `pkill -f`
  trap DEMO.md already carries three times, met with a fourth verb. The churn run had in fact
  finished; the load was storm, which was paused by the pid read off `ps` and resumed after the
  walk.

- **The last weights arm could not be isolated on an emulator.** ADR-0063 §2's "yields to two" with
  battery as one reason wanted the emulated phone on battery *and* loaded while the laptop stayed
  idle. Two busy loops in the guest took it to 50 %, and the laptop, the emulator's host, went from
  13 % to 39 % in the same minute. The round came out 38 to 29 for the emulator, on the arithmetic,
  and so was not the case it was staged for. The same session also hit the `-f` trap twice more:
  `pkill -f 'bvune6wiv'`, a background task's id, sat in the invoking command line and killed the
  shell (exit 144). And `pkill -f "while :"` inside the guest took its own `adb shell` with it, so
  no confirmation line printed. `ps … | grep '[w]hile :'` and a kill by pid is the form that
  works on both sides.
- **A negative assertion passes for any reason.** `a_grant_issued_mid_connection_reaches_the_next_round_when_the_peer_dialled`
  asserted `accepted_by() == None` with the message "the old certificate says no". Once the
  certificate lookup missed inbound sessions, the round was refused as "connection closed". That
  outcome was the same, and the test stayed green through the regression. Its second round then
  succeeded by dialling out, which the test had never meant to allow. Rewritten to assert that the
  refusal mentions `host-runs`, with `net.firewall(desktop)` so every round has to use the desktop's
  own connection. Control-run: with the old `peer_certificate` it fails with the "connection closed"
  reason printed. The same rewrite exposed the closed-dialled-session hole (gossip-and-merge).
- **A keep-awake is walked by the machine staying awake, not by the assertion being listed.**
  ADR-0077's walk ran `macos_lists_the_assertion_while_it_is_held` on the Mac mini and found "Offload:
  power test" in `pmset -g assertions`. In session ninety-four the Mac got `host-runs`. Its daemon
  held `PreventUserIdleSystemSleep` from 12:08:49, and `pmset -g` printed `sleep 1 (sleep prevented by
  offloadd, powerd)`. `pmset -g log` still showed `Entering Sleep state due to 'Maintenance Sleep'`
  at 12:08:59 and 12:19:10, and the laptop and the tablet marked it dead. It was found from the
  app, where "Can take agent runs: laptop" left the Mac out, and the tablet's `offload nodes` said
  why. The mechanism: a headless Mac is asleep, a packet dark-wakes it, and a dark wake returns to
  sleep regardless of an idle-sleep assertion. With `PreventSystemSleep` the same Mac, untouched for
  five minutes (12:23–12:28), logged no sleep entry after its return to dark wake. Before, every
  dark wake had ended in sleep within about 45 s. The ssh calls used to check it were themselves
  waking it, which is why the old reading looked fine whenever somebody looked.
- **A battery reading taken over wireless adb measures the walk's own connection.** Session
  ninety-three read `Total WiFi Multicast wakelock time: 10h 55m` in the phone's overnight dump as
  Offload's. The per-uid entries in the same file: u0a160 (`se.mach25.offload.app`) 5h33m, uid 1000
  5h11m. On 2026-09-27, with Offload's daemon stopped, `dumpsys wifi` listed exactly one holder,
  `Multicaster{AdbMulticastLock uid=1000}`, which is wireless debugging, and the afternoon's total
  was 4h19m of which adb's was 4h00m and Offload's 3m.
- **A test that writes a program and then runs it can find it "busy".** Found by the first CI
  workflow, a GitHub Actions `cargo test` on a four-core runner, on its rerun:
  `an_agent_that_refuses_to_list_its_models_says_why` failed with `could not start the agent: Text
  file busy (os error 26)`. The same shape had been carried in HANDOFF as an unexplained `spawn`
  failure in `the_agent_is_told_where_its_state_is_rather_than_left_to_inherit_it`, "`ETXTBSY` …
  fits, and is a guess". It is the fork race: Rust opens the script with `O_CLOEXEC`, but between
  another thread's `fork` and `exec` the child still holds the descriptor. The retry went into
  the code rather than the tests because the failing spawn is inside the code under test, and
  because Claude Code updating itself in place is the same error in production. The five spawn
  sites (agent, model list, task, trigger, delivery route, resource server) share one helper. After
  the fix, 20 runs of `offload-agent`'s tests at 16 threads had no failures.

- **`kill -15 -N` cancelled every CI run.** Once the `ETXTBSY` fix was in, PR #2's `cargo test
  --workspace` job still ended "The operation was canceled" partway through `offload-node`'s tests.
  No test had failed and nobody had pressed cancel. A throwaway draft PR ran the tests under a
  root `bpftrace` watching `sys_enter_execve` for `kill` and `signal:signal_generate` for signals
  to `Runner.Worker` and `Runner.Listener`. It printed
  `EXEC kill from thread [supervisor::tes] … argv: kill -15 -13747`, then
  `SIGNAL 15 to Runner.Listener pid=1871, sent by kill pid=13766`. A supervisor test was cancelling its fake agent,
  and `signal_group` had shelled out to `kill -<signal> -<pid>`. On Ubuntu's procps-ng `kill`,
  that SIGTERM did not reach the child's group; it reached the runner's listener, which cancelled the job.
  util-linux's `kill` on the development laptop parses it as a group, so no local run ever showed
  it. Both man pages document `kill -s SIGNAL -- -PGID`. The fix builds exactly that, and a second
  test checks that a group leader and its child both stop.

- **The third CI failure was the laptop's own git config.** With the `kill` fix in, the whole
  suite ran to the end on a runner for the first time, and six `offload-workspace` tests failed
  at `.expect("commit")`. `source_repo` sets an identity in the fixture repository, but the tests
  then commit inside the run's *worktree*, which is a different checkout with no identity of its
  own. On the laptop the global config supplied one. Under `GIT_CONFIG_GLOBAL=/dev/null
  GIT_CONFIG_NOSYSTEM=1` the same six failed locally, with the same message, and the other 58
  passed. After the fix, the whole workspace under those variables with `--no-fail-fast` had no
  failures. That matters because `cargo test` stops at the first failing test binary, so a CI run
  that fails in one crate says nothing about the crates after it.
