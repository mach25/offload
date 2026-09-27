# Tests, sweeps and checks that guard a rule

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/testing-and-sweeps.md`, same order.

- **A fixpoint is not "one pass changed nothing".** SWIM revisits a peer once every `peers` rounds,
  so two identical consecutive passes are the *ordinary* state mid-recovery. `settle` waits a whole
  rotation of quiet.
- **A simulation that breaks the rule it is modelling reports the product as broken.** `storm.rs`
  let any node call `place`; `arbiter_for` is the rule, so the simulation has to honour it. The
  tell is that the counterexample needs a step the daemon has no code path for.
- **A property test checks a sentence; a unit test checks a case.** Everything the properties found
  was a rule written down in one place and honoured only there. Two habits: **aim a property at
  what the tree claims**, and **revert each fix to watch its property go red** — a property can
  pass for the wrong reason.
- **Sweep for `pub` items nothing calls; it found three in one pass.** List every `pub fn`, count
  references outside its own declaration and test modules, *read* what is left. It found a second
  start gate with no rate limit in it, a second copy of the absence average, and a whole unused
  module. Some survivors are legitimate — a serde helper, a predicate that makes assertions legible.
- **…and the same sweep over *enum variants* asks what this fleet can never say.** Count
  references in construction position. `handshake::Refusal::Draining` is the one to remember: **a
  draining node needs its connections**, so wiring up that refusal breaks the feature it names.
- **An escape hatch named in a doc comment is a feature claim.** `Constraint::Not` is handled by
  `matches`, `explain` and `failures` and producible by nobody — the "run spec file" its grammar
  pointed at does not exist. Left unreachable on purpose; the *sentence* was fixed.
- **A doc comment in the past tense is not evidence that anything calls the code.** When a module
  explains why it matters, check the call graph before believing it.
- **Dead code that asserts a mechanism is worse than dead code.** `bid_delay` described a protocol
  step replaced before it ever shipped, in the present tense. Deleted, with the reasoning kept
  where the fields were: the next person to want a delay should have to make the decision.
- **A machine-wide ledger makes the test suite depend on the machine.** The device reservation
  ledger is per-user and per-machine *by design*, so tests read whatever `offloadd` is running —
  and a test that drives a run can tell a live daemon its machine is full.
  `Supervisor::with_private_ledger`.
- **A check scoped by a claim about where a mistake matters is scoped by a claim with a date on
  it.** `messages.rs` looked in one crate and missed `offload-core`. It walks `crates/*/src` now,
  and asserts it found at least eight crates — a walk looking in the wrong place passes by finding
  nothing. Contrast `no_clock.rs`, rightly one crate: that rule is *about* `offload-core`.
- **The text is part of the behaviour, and a mangled message is invisible in review.** A tool
  rewriting source through Python bakes indentation into a continued string literal. Checked
  (`messages.rs`): four or more spaces beginning twenty characters into a literal. **It has fired
  on three separate sessions' new strings, twice on the session that widened it** — the author is
  the person least able to see it. A literal that wants two lines is written as two literals.
- **A hand-written fixture can agree with the code about a world neither lives in.** The GC test
  inserted a shape `save_run` has never produced and passed for two phases while the real path
  deleted work. Build fixtures through the writer the daemon uses.
- **A guard that fails for the right reason is still a flaky guard.** The sweep race passed 40/40
  before its fix, 10/10 alone afterwards and 24/24 under synthetic CPU load — then went red **2 in
  8 full-suite runs**, because under real parallelism a 1ms racer can outlast a 3ms subprocess and
  the sweep then removes the checkout *correctly*. Isolated repetition and synthetic load are not
  substitutes for running the whole suite repeatedly; only the suite has the right contention.
- **…and making it deterministic can quietly stop it testing anything.** Replacing that race with
  "assert the predicate before the pickup, assert it after" **passed with the fix removed**: the
  defect is that state changes *inside* the loop, and a test that changes it beforehand only ever
  exercises the first call. Always watch a guard go red — deleting the fix and re-running is two
  minutes and is the only thing that distinguishes a guard from a decoration.
- **Widen the window instead of abandoning the race.** `git status` must re-hash a file whose mtime
  moved, and still reports the worktree **clean** — so rewriting a 16 MB file with identical bytes
  buys an **83ms** window against a 1ms racer, where the two-file fixture bought 3ms. A race made
  1000x more likely is deterministic in practice and still tests the real code path.
- **…and where there is a trait at the seam, gate it rather than widen it.** `start_run`'s one
  `await` is `Peers::fetch`. A stand-in whose `fetch` signals it has been entered and then waits
  for the test to let it go turns "a network round trip is a window" into an exactly-placed one,
  with no sleeping and no repetition: the race is *arranged*, on a current-thread runtime, and
  the whole loop is still the real one. Widening is for the seams with no trait to stand in — a
  git subprocess.
- **A test double that models one end of a two-ended thing tests one end.** `MemoryConnection::close`
  set a flag on the closing side only, so a peer that had been hung up on went on opening streams
  and gossiping — no real transport behaves that way. The flag and its `Notify` are shared between
  the ends now. Session forty-two's lesson, one layer down: the half a double leaves out is the
  half the bug is in — and the revocation test written against the lenient double **passed with
  the fix removed**.
- **A test harness that cannot express the staging is why nothing caught it.** Every
  `Store::open_memory` shared **one** blob directory — `$TMPDIR/offload-memory-store` — because a
  blob lives on disk rather than in the connection. So `has_blob` was not a question about *this*
  node, "a node that does not have this checkpoint's blobs" could not be staged at all, and the
  hole in `resume` was invisible to 900 tests. It also outlived the process, so a blob one test
  wrote was visible to another test in a later run. The root is per-store now, and the three
  tests that were resuming from a checkpoint naming a blob that existed **nowhere** — and passing —
  say `checkpoint_here` instead. **Ask what the harness makes unsayable**; that is where the
  defect is.
- **A race test that asserts after both racers have joined is asserting an ordering nothing
  promises.** `a_rebuild_never_adopts_a_checkout_a_teardown_is_removing` failed about **1 run in 5
  at load 6** and three sessions each waved it through as "the machine", from one observation
  apiece. It was neither the machine nor the lock: made to say which side of the guard it landed
  on, it answered **`rebuild adopted, teardown Removed`** — the rebuild won the lock, adopted a
  checkout that was genuinely there, released, and the teardown then removed it, which is correct.
  The guarantee is *while the guard is held*, so the check moved inside the task that holds it.
  **0 failures in 60 runs at load up to 10.7** afterwards, against 2 in 10 before, and it still
  catches what it was written for: without serialisation the removal overlaps the check instead of
  following it.
- **Make a flaky assertion name which case it was, before theorising about it.** One line carried
  out of the task — did the rebuild adopt or rebuild, did the teardown remove anything — turned
  three sessions of "probably load" into one reproduction that said the answer outright. The same
  move ADR-0051 made for `register`: if you cannot fix it now, make the next occurrence identify
  itself.
- **An assertion can name an invariant it is structurally unable to check.**
  `a_checkpoint_is_copied_off_this_machine_and_the_run_records_where` compared what replication
  was asked to copy against `checkpoint.blobs()` with the message *"every blob, not some of
  them"* — over a `checkpoint_at` fixture whose bundle and patch are `None`, so it read
  `[transcript] == [transcript]`. Measured: making `Supervisor::replicate` send
  `vec![checkpoint.transcript]` passed the **whole workspace, 909 tests**, with `mesh.rs`'s "all
  or nothing" invariant gone — a peer holding the conversation and not the commits recorded as a
  replica, and `is_durable` then calling a run safe that nobody else can rebuild. The fixture
  carries all three now, and the revert goes red.
- **The sweep question that finds these: "what is set to `None` here, and what would `Some`
  do".** Run it over the fixtures of anything you have just fixed. It found the above, and it
  found `Capture::summary` — the one sentence an operator is ever told about what a checkpoint
  *carried* — with no test in either direction; no assertion anywhere mentioned `byte bundle`.
  What it **cleared** is worth recording too, because a sweep that only reports hits is not a
  sweep: `Checkpoint::blobs()` is exercised with all three (`bid.rs`), the blob collector really
  does notice (reverting `referenced_blobs` to the transcript alone fails `gc.rs`), and the
  `bundle: None` in the `mesh.rs`, `explain.rs`, `view.rs` and `run.rs` fixtures is irrelevant to
  what those tests assert.
- **The revert is the only thing that tells a guard from a decoration, and it is two minutes.**
  Both findings above are inferences until the fix is removed and the suite is run; one inference
  was right and the other — that the collector was uncovered — was wrong. Do not write down
  either without running it.
- **A theory about local code does not need the remote machine that provoked it.** A
  cross-platform mesh failure produced a tidy suspect — quinn discarding a reply when the
  responder drops an unfinished stream — and two ten-minute build-and-redeploy cycles went into
  circling it. The test that settled it needed **one machine and thirty seconds**: write a 64 KiB
  reply, finish, drop the stream the way the production task does, read it back; then delete the
  `finish()` and watch it pass anyway. **Ask what the smallest thing that could disprove this is,
  and where it lives** — the answer is usually not where the symptom was seen.
- **…and the existing test's workaround was the tell.** `two_nodes_connect_over_real_sockets…`
  carries a comment about holding the endpoint open because "dropping a quinn endpoint closes its
  connections at once, discarding anything still in flight, which in a test looks exactly like a
  protocol bug" — so the hazard was known one level up, and that workaround is precisely why no
  test had ever exercised the drop. **A comment explaining why a test avoids something names the
  case nobody is covering.**
- **An intermittent failure makes every single observation nearly worthless.** Three separate
  conclusions about a cross-platform mesh were drawn from one reading each, and each was
  contradicted later by the same pair behaving differently. Counting is the fix — the entry above
  about a flake dismissed from one observation, met again in a place where it cost a whole
  afternoon. **Make it reproducible on demand before theorising about the mechanism.**
- **A control that does not change the thing it controls for retires a live hypothesis.** The
  same-4-tuple collision was "ruled out" by moving one daemon from port 7433 to 7434 — and both
  daemons use their **listening port as the source port for outgoing dials**, so the tuple stayed
  symmetric and the control tested nothing. Breaking it properly needs one side never to dial
  (`seeds = []`, `mdns = false`). **Before believing a control, say out loud which variable it
  moved.** The postscript is the entry above, and it is the more useful half: run properly, five
  attempts per arm, that hypothesis **died anyway** — 1 handshake in 5 listen-only against 0 in 4
  symmetric. So the bad control and the truth agreed by luck, and for a while the write-up said
  the hypothesis was "right and incomplete" on the strength of one observation each way. **A
  control being invalid is not evidence that the theory it dismissed was correct.**
- **A retraction has to land everywhere the claim was written, and it usually does not.** The
  4-tuple correction rewrote `docs/DEMO.md`'s main section and left the retracted sentence
  standing in a later paragraph of the same file *and* in this one — so the dead theory stayed
  alive in the two places a future session would actually read it, for a full session, while the
  commit message said it was dead. **After retracting something, grep for the claim rather than
  for the file you remember editing.**
- **A diagnostic needs a positive control on a path known to work, or its zero means nothing.**
  `gro_probe` was written to ask whether a quinn-configured socket loses datagrams a plain socket
  keeps, and its first run said **0 of 16 over a path carrying all 16** — the exact shape of the
  cross-platform failure, reproduced on one machine with no second host. It was an artifact:
  `quinn_udp::UdpSocketState::new` puts the socket in **non-blocking** mode as its first act
  (`unix.rs:109`), which silently makes `set_read_timeout` inert, so the probe read once, got
  `WouldBlock` before the sender had started, and exited having measured nothing. What caught it
  was running **the same probe over loopback**, where the answer was already known — and it
  reported 0 there too. The comparison had also drifted: the quinn receiver and the python one
  were never once run on the same port. **Run the instrument against a known-good path first;
  when a new tool and a real bug have the same signature, assume the tool.** Fifth false zero in
  this one investigation, after the four from `RUST_LOG` below.
- **A socket option can disable the timeout you set on it.** Specific enough to be worth naming,
  because nothing warns: `set_read_timeout` does nothing on a non-blocking socket, and
  `UdpSocketState::new` makes the socket non-blocking. Any wait built on a read timeout over a
  quinn-configured socket has to be an explicit deadline with a retry on `WouldBlock` instead.
- **Check the log filter before believing a zero.** `RUST_LOG` scoped to crate targets silences
  `offloadd`'s own INFO lines *and* `offload_node::mesh`'s seed messages, so "no seed attempts" and
  "nothing arriving inbound" were filter artifacts **four times in one session** — each time
  producing a confident wrong conclusion about where a failure was. `RUST_LOG=info,offload_node=debug,…`
  keeps the binary's own lines. Already recorded in `docs/DEMO.md`; recorded here too because it
  is a *measurement* fault, not a demo detail.
- **…and the filter that was written down as the remedy caused the fifth and sixth.** A crate-scoped
  filter is only as good as its list of crates, and `info,offload_node=debug,offload_cluster=debug`
  — the one both `docs/DEMO.md` and the entry above recommend — omits **`offload-transport`**, which
  is where every inbound-path line lives (`quic.rs:315` onward: *inbound connection arriving*,
  *inbound TLS done*, *inbound peer admitted*). A cross-platform walk then ran for 2.5 hours and
  logged **zero** `offload_transport` lines, and "the Mac logged no inbound connections" was read
  off that as evidence. What makes it convincing rather than obviously empty is that the *adjacent*
  lines do appear: `stream accepted` and `request decoded` are in `offload-cluster`, which is in the
  filter, so the log reads as a complete picture of the receive path with the receive path missing.
  **Before believing a zero, grep the source for the line you expected and check its crate is in
  the filter** — the count of times this has now cost a wrong conclusion here is six.
- **A remedy in a doc is a claim with a shelf life.** The same file said the duplicate-address
  condition on the LAN was "since removed", and it was live on the laptop the whole time — both
  addresses up on one subnet, and the marker the file itself names for it sitting in the peer's
  routing table. A fix recorded in the past tense is not a measurement; **re-check the environment
  claim before building on it**, especially the ones written as asides.
- **An instrument that cannot fail is not an instrument, and `quinn-udp`'s `send` cannot fail.**
  `UdpSocketState::send` returns `Ok(())` for **every** error but `WouldBlock` — it logs and
  swallows the rest, because UDP loss is the caller's problem to retransmit around. So
  `udp_probe`'s "10 of 10 delivered, Mac → laptop" was never a measurement of anything: it
  counted calls that returned `Ok`, which they always do. That single line was the basis for
  *"everything below `quinn-proto` is proven good"* in `docs/DEMO.md` for two sessions, and when
  the probe was switched to **`try_send`** the same path measured **0 of 40**. Use `try_send` in
  any probe; and before trusting a 100% result, check the call you are counting is one that can
  return an error at all.
- **`log_sendmsg_error` is rate-limited to one line a minute**, so a count of `sendmsg error`
  lines in a log is a count of *minutes that had at least one*, never a count of errors. "13 in
  2.5 hours" was read here as a rare transient; the same socket was in fact failing every send.
  A log line whose emitter has a rate limit is a presence/absence signal and nothing more.
- **Hold the instrument constant across arms, not just the subject.** Two arms were compared
  here where one used `send` and the other `try_send`, and the "0 failures / 123 failures" split
  that produced was entirely the instrument. Where an arm needs a different tool, run a third arm
  with the new tool and the old conditions before believing the comparison.
- **A negative result needs a positive control *in the same window* when the fault is
  intermittent.** Ten socket-option variants all passed 40/40 here, which looked like ten dead
  theories; the condition had simply lifted between runs. Re-run with the known-failing call
  interleaved into the same loop and the picture inverted — every variant 40/40 ok, the control
  0/40. Interleave the arms; never run them back to back.
- **`pgrep -f` self-matches too, not just `pkill -f`.** The `pkill -f` trap is recorded twice in
  `docs/DEMO.md`; the read-only sibling is worse, because it does not kill the walk — it lies
  about it. `pgrep -f slow.sh` matched its own shell and made a correctly-cancelled task look
  like it had leaked its process group, which was reported as a defect before being checked.
  Use `ps -eo pid,cmd | grep <a path the command line does not contain>`, or match on the
  program rather than the pattern. Met four times in one session.
- **A test that reads the machine it runs on is a test whose answer depends on the suite.**
  `offload status` measures this device's own load average, and `admits` refuses work under
  pressure — so a new test asserting that a light-work-only node reports `light work: yes` passed
  alone and failed inside `cargo test --workspace`, which pegs the CPU: `cpu at 100%: 100% of the
  budget is free, the machine is not`. Correct behaviour, wrong assertion. The fix is not a
  tolerance or a retry but asking the question the test is actually about — here, that the owner's
  *second* answer is reported and is its own sentence, which is true at any load — and leaving the
  exact verdict to the pure `admits` test next door, where the `NodeLoad` is an argument.
- **…and two `cargo test --workspace` runs at once share `target/`, so one of them lies.** A run
  reported `error[E0463]: can't find crate for offload_transport` in `offload-cluster`'s doctests
  — a crate that plainly exists — because a concurrent build had replaced the `.rlib` rustdoc had
  been handed by name. `cargo test -p offload-cluster --doc` on its own: clean. The wasted cycle
  is cheap; believing it would not have been. **Run the suite serially**, and when a failure names
  something impossible, check what else was writing to the target directory before reading the
  message as evidence. Related and worth knowing: `grep -c FAILED` over a captured run misses a
  **doctest** failure entirely, which is why the exit code is the thing to read.
- **The `bool`-shaped-answer grep is the wrong test, and running it is how that was established.**
  Four of session seventy-eight's findings were one shape — a function that decided something with
  a reason and answered *whether* rather than *why*, leaving an operator-facing sentence to guess
  (`Adopted`, `Pushed`, `Replicated`, and `Asks::answer`, which did it with a filter chain and no
  `bool` at all). Grepping return types afterwards found **83** `-> bool` functions outside tests
  and **nothing**: the rest are *"did this change"* and internal control flow, and `Probe::refresh`
  carries a doc comment saying why it must not become a second answer. **Start from the sentence,
  not the signature** — find an operator-facing line that states a cause, then walk back and ask
  whether that cause was measured or guessed. All four were found that way; none by a signature.
- **A background sweep is a load average, and load is an input.** A 20 000-case property run put
  the laptop at `cpu 100%`, which made it *refuse* a bid round a weights walk was measuring. Pause it
  by exact pid (`kill -STOP <pid>`, `-CONT` after), never through `pgrep -f`: its pattern is in your
  own shell's command line, and `kill -STOP` on that stops the walk, not the sweep.
- **An emulated device shares its host's load.** Two nodes where one is an emulator on the other
  cannot be loaded independently: burners in the guest raised the laptop's own `cpu` by 25 points.
  Any arm whose point is "this one busy, that one idle" needs two machines.
- **A negative assertion passes for any reason.** `a_grant_issued_mid_connection_reaches_the_next_round_when_the_peer_dialled`
  asserts its first round accepts nobody, "the old certificate says no". It went on passing while
  that round was refused for a different reason, the missing certificate lookup in
  gossip-and-merge. Its second round then dialled out, which only works because nothing in it is
  firewalled. When a test's point is *why* something is refused, assert the reason. When it
  exercises a path, block the other path.
- **A keep-awake is walked by the machine staying awake, not by the assertion being listed.**
  ADR-0077 was walked by finding its assertion in `pmset -g assertions`. The first real host held it
  with `pmset -g` saying "sleep prevented by offloadd", and slept anyway: `PreventUserIdleSystemSleep`
  does nothing for a machine that is already asleep and only dark-woken. Read `pmset -g log` for
  `Entering Sleep` over several untouched minutes, beside a peer's `offload nodes`. Any ssh to the
  machine wakes it and spoils the reading, so leave it alone for the whole window.
- **A battery reading taken over wireless adb measures the walk's own connection.** Wireless
  debugging holds `AdbMulticastLock` (uid 1000) the whole time it is connected. ADR-0078 read a
  night's "multicast lock held all night" as Offload's, and half of it was adb's. Read the per-uid
  lines of `dumpsys batterystats` and `dumpsys wifi`'s lock holders, not the total.
- **A test that writes a program and then runs it can find it "busy".** Another thread of the
  test process forks between the write and the exec, the child holds a copy of the write handle
  until its own exec, and the kernel refuses to run a file open for writing (`ETXTBSY`). Rare on
  a laptop, and a four-core CI runner hit it on the first rerun. Every program the node starts goes
  through `offload_agent::claude::spawn_when_not_busy`, which retries for half a second. It is
  pinned by `a_program_held_open_for_writing_a_moment_longer_is_waited_for`, whose control run with
  no retries fails with CI's exact `Text file busy (os error 26)`.
- **Signal a process group as `kill -s <sig> -- -<pgid>`, never `kill -<sig> -<pgid>`.** Without
  `--`, what a negative number means depends on which `kill` is installed. util-linux's (this
  laptop's Fedora) reads it as a group; procps-ng's (Ubuntu, so every GitHub runner) sent a test's
  SIGTERM to the runner's own `Runner.Listener`, and each CI run ended "The operation was
  canceled" mid-test, with no failing test to show for it. The arguments are built by
  `kill_args` in `offload-agent`'s `claude.rs` and pinned by
  `a_process_group_is_signalled_with_the_options_ended_first`. A job canceled with no failure
  is something in the job signalling the runner. Find it by tracing signals, not by bisecting tests.
