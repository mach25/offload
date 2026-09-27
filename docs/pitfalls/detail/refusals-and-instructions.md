# Refusals and instructions — full entries

The working rules are in `../refusals-and-instructions.md`, in this same order. These are the
entries they were compressed from: the mechanism, the measurement, and how each one was found.
Read one when the rule alone is not enough to act on.

- **One refusal, two renderings, and mixing them up breaks a different rule each way.** The
  sentence an operator reads is computed fresh and carries a duration
  (`supervisor::describe_refusal`); the one **stored and gossiped** beside the run must carry none
  (`summarise_refusal`), because a number written down is a sentence about a moment read back as a
  claim about now — the bug `offload rules` and `offload explain` each had. And the stored one must
  fit the **18 characters** of `offload ps`'s `WORKSPACE` column, whose longest existing value
  (`waiting for a slot`) is exactly that: twenty pushed every following column off the end of the
  table, which is the entry below reached from the other direction. Checked by a test that measures
  every phrase against the column and refuses any with a digit in it.

- **…and that column gossips, so a wrong reason in it is the whole fleet's answer.**
  `note_workspace(run_id, "waiting for a slot")` is written whenever a run is accepted and not
  started, and it was the fleet's account of a run held by a *rate limit* on a laptop with four
  free slots. The refusal was in hand at that line and discarded. Same family as `offload rm`
  writing `workspace: "removed"` about another machine's disk: this is the column somebody reads at
  07:00 to find out whether there is uncommitted work.

- **A stored reason is a sentence about a moment, and printing it with no tense makes it a claim
  about now.** `offload rules` replayed the reason the last event was dropped — `still running
  01a03ad8ef23` — one line under `last fired 01a03ad8ef23`, naming one run twice and saying two
  different things about it, an hour after that run had *failed*. The same family as `offload
  explain` counting a finished run down towards a deadline it had met, and the same fix: ask the
  question at the instant it is answered (`rule_run_in_flight`), and word what is written down in
  the past tense because it will be read back later.

- **Three commands offered an answer to a question and each offered a different half.**

  **What was measured.** With one question pending, the three places that tell somebody how to
  answer it said:

  | where | run id | offers deny |
  | --- | --- | --- |
  | `offload logs` | `<run>`, literal | yes |
  | `offload asks` | yes | **no** |
  | `offload drain`'s `Blocked` step | yes | yes |

  `print_event` took only a `&LogKind`, so the run id was not in scope — while `logs` itself had
  it as its own argument the whole time. It is now threaded through as **what the operator typed**
  rather than the resolved id: that is the one spelling known to resolve on this machine, and a
  prefix pastes.

  **Why `asks` is the one that mattered.** It is the dedicated report — the command the other two
  point people at — and its `WANTS TO` column is as often `rm -rf /important` as `cargo test`. The
  report whose entire job is the question named only the *yes*. That also shows up in the mention
  counts the `DEMO.md` loop produces: `approve` at 9, `deny` at 2.

  **The general rule.** When one piece of advice is rendered in more than one place, the sites do
  not usually diverge on whether the advice is *true*. They diverge on which half got left out,
  because each was written by somebody looking at a different screen. Grepping for the command
  name finds all of them in one line and is worth doing whenever one of them is edited.

- **A refusal computed from the *string* cannot tell two machines' reasons apart.**

  `offload run --repo` on a directory that is not a git repository, and on a path that exists
  nowhere, on the same daemon, one after the other:

  ```
  $ offload run --repo /tmp/ow/notarepo "say hi"
  Error: no node will take this run
    alpha        /tmp/ow/notarepo is a path on another machine
  $ offload run --repo /tmp/ow/no-such-path-anywhere "say hi"
  Error: no node will take this run
    alpha        /tmp/ow/no-such-path-anywhere is a path on another machine
  ```

  One sentence, two causes, and it is wrong for the first: `/tmp/ow/notarepo` is on alpha, and
  alpha is the machine saying otherwise. Somebody reading that line goes looking for the node that
  has the path, and there isn't one — the directory is under their cursor.

  The mechanism is one field. `WorkspaceManager::can_obtain` answered a `bool`, and
  `LocalFacts::repo_available` carried it, so by the time `bid::evaluate` had to word a refusal the
  measurement was gone and the only thing left to word it *with* was the repo string:

  ```rust
  return Err(NoBid::RepoUnavailable {
      repo: work.workspace.repo.clone(),
      portability: RepoSource::parse(&work.workspace.repo).portability(),
  });
  ```

  `Portability` is a fact about the string. It is the same on every machine in the fleet, by
  design — that is what makes it safe to reason about before placement — and it answers
  `NodeLocal` to both causes. The message was therefore right about the *class* of repo and had
  no way to be right about *this node*, which is the only thing the reader is standing in front
  of.

  `RepoReach::{Obtainable, NotHere, NotARepo}` now, measured in `WorkspaceManager::reach` — the
  one place in the path that can look at a disk — and carried on `LocalFacts::repo_reach` to
  `NoBid::RepoUnavailable { repo, reach }`. Nothing else reads the fact, and `NoBid` is rendered
  to a string before it travels, so there is no wire or schema change. Re-walked on one daemon,
  with both controls:

  ```
  /tmp/ow/notarepo             is here, but it is not a git repository
  /tmp/ow/no-such-path-anywhere is a path on another machine
  /tmp/ow/plainfile            is here, but it is not a git repository   # a file, not a directory
  <this repository>            cpu at 83%: 100% of the budget is free…   # the gate passes
  ```

  and `offload explain` on the queued case says the same thing, because it re-canvasses through
  the same `evaluate`.

  Two things worth keeping beyond the fix. The first is that `""` — what a **task** hands
  `reach`, since a task names no repo — comes out `NotHere` and not `NotARepo`, because
  `Path::new("").exists()` is false; the guard that stops a task being refused for a repository it
  never named is the `if let Some(work) = agent` above, and it is unchanged, but the field it
  guards now has a third value and the comment beside it says which one a task gets.

  The second is where this came from: it was found while drafting ADR-0061 — *a workspace may
  arrive as bytes* — and not by a walk, because the ADR's premise is precisely that the ordinary
  workspace is not a git repository. The refusal is the first thing anybody in that situation
  meets, and it sent them to look for another machine. The fix is worth having whether or not the
  ADR is ever built; it does not close any part of it, and a directory that is not a repository is
  still refused everywhere. It is now refused with a sentence that says what to do about it.

- **Giving a command an identifier of the *wrong type* is sound across the board.**

  Session seventy-eight's `offload approve` finding generalised to a sweep: for a command taking an
  opaque identifier, what happens when the operator supplies a **plausibly wrong** one rather than
  a missing one? The sharpest form of "plausibly wrong" here is an id of a *different kind*, and
  the shapes invite it — a rule id and a schedule id are both sixteen hex characters, and only a
  run id (twelve) looks different.

  Staged on one daemon holding one rule (`f37ab0bc0eb9167b`), one schedule (`e8a48983822ea5fb`) and
  one run (`01a090e1df38`), then crossed:

  ```
  unwatch    <schedule>  → no rule matching `e8a48983822ea5fb`
  unschedule <rule>      → no schedule matching `f37ab0bc0eb9167b`
  unwatch    <run>       → no rule matching `01a090e1df38`
  logs       <rule>      → state store: no run matching `f37ab0bc0eb9167b`
  explain    <schedule>  → state store: no run matching `e8a48983822ea5fb`
  cancel     <rule>      → state store: no run matching `f37ab0bc0eb9167b`
  ```

  All six are true, and each names the type it was looking for, which is the part that makes them
  act as a hint: *no rule matching* tells somebody they are in the wrong command. Nothing claims a
  wrong cause, and nothing tells the operator to try harder without saying at what.

  A cross-type hint — *"that is a schedule id; did you mean `offload unschedule`?"* — is
  deliberately **not** built. It couples each command to the other subsystem's table for a
  convenience, and `offload rules` / `offload schedules` resolve it in one command. Recorded so the
  next session does not re-walk it, and so the decision not to build it is a decision rather than
  an omission.

- **An empty name in a config block is a *missing field*, not a wrong one.**

  Found by accident while staging the sweep above: a `[[triggers]]` block written without a
  `service` line. The daemon behaved correctly — it refused the trigger, kept running, and said so
  every probe — but the sentence was:

  ```
  WARN ignoring a trigger trigger=ticker error=invalid config: : not a service this build can offer
  ```

  `TriggerConfig` is `#[serde(default, deny_unknown_fields)]`, so a *misspelled field name* is
  caught by serde and a *missing* one silently becomes `""`. `Service::from_str` handles the empty
  case deliberately (`"" => return Err(UnknownService(String::new()))`) — somebody thought about
  it — but the error type had a single `#[error("{0}: not a service this build can offer")]`, and
  interpolating an empty string opens the sentence with a colon and names nothing to act on. It
  reads as a service this build does not support, which sends the operator to the service list
  rather than to the line they omitted.

  Two renderings now, and the fix is at the one place all four blocks pass through
  (`[[sinks]]`, `[[triggers]]`, `[[resources]]`, `[[tasks]]` each parse a defaulted `service`):

  ```
  WARN ignoring a trigger trigger=ticker error=invalid config: no `service` was given, and one is required
  ```

  The general form is worth keeping: **`#[serde(default)]` on a required field turns "absent" into
  a value, and every message downstream then describes the value instead of the absence.** Where a
  default exists only so deserialisation succeeds, the validator is the thing that has to tell the
  two apart.

- **A number in a help string is a number that drifts.**

  ADR-0061 §4 asks for the archive cap to be *"discoverable before the expensive step"* — an agent
  that packs 6.5 GB and is refused afterwards has spent minutes and a disk to learn a constant.
  The first cut of `offload run --archive` put the number in its own help text:

  > **Choose what goes in it.** The fleet moves at most 512 MiB in one exchange…

  True when written, hardcoded, and one change to `MAX_BLOB_BYTES` away from being confidently
  wrong in the one place somebody reads *before* doing the expensive thing. It is the stale-number
  rule this tree already applies to docs, met in argument help — where it is worse, because a
  session re-measures a doc and nothing ever re-measures a `--help`.

  So the help says the limit exists and where to read it, and `offload status` prints the constant:

  ```
  workspace   archives up to 512.0 MiB  ·  a workspace over that has to be a smaller selection
  ```

  Walked with the cap lowered to 128 KiB, which is the check that matters — the number an agent
  **reads** and the number that **refuses** it are the same one, because both come from
  `MAX_BLOB_BYTES` rather than from two copies of a fact:

  ```
  workspace   archives up to 131072 bytes  ·  …
  Error: that archive is 307200 bytes and the fleet moves at most 131072 in one exchange …
  ```

  That is *a report has to come from where the decision reads*, applied before the decision rather
  than after it.

- **…and a new `#[serde(default)]` field reads as a real value, not as silence.**

  `NodeStatus::max_archive_bytes` is a control-socket field, so a daemon too old to send it leaves
  it `0` — and `archives up to 0 bytes` is not a missing sentence, it is a **wrong** one: it says
  this fleet will not take an archive at all, which is precisely the opposite of what the field
  exists to say. The line is omitted at zero instead.

  The CLI and the daemon ship together, so this is only ever the mixed-version moment — and that
  moment is exactly when somebody is already confused about which half is out of date. Same shape
  as every `#[serde(default)]` field before it, and worth restating because the trap is specific to
  fields whose *zero is meaningful*: a default `None` reads as silence on its own, and a default
  `0` on a size reads as a policy.

- **One refusal covering four causes, and the audit pointer that replaced it.**

  `offload rm` acts only where it is typed — deliberately, unlike every other run-targeted command,
  since a directory cannot be torn down over the network. When `Removal::NothingHere` came back it
  said one thing:

  ```
  no checkout for 01a0923bcb30 on this node — it was removed already, or the run finished on
  another machine, where `offload rm` is what discards it
  ```

  Walked on two daemons, four states reach it:

  | what was typed | what is true | what it said |
  | --- | --- | --- |
  | `rm <a task>` | no workspace on **any** machine (ADR-0019 §2) | go and look on another machine |
  | `rm <cancelled out of the queue>` | nothing was ever made anywhere | go and look on another machine |
  | `rm <already removed here>` | torn down here, and `ps` says `removed` fleet-wide | *or* it was removed already |
  | `rm <finished on a peer>` | on bravo's disk | the machine is not named |

  Three are false in both halves. The fourth is the only one the sentence describes, and even there
  it named no machine while `RunProgress::by` sat in this node's own `runs` row — the field
  `log_source`, one function up in the same file, already reads to route `offload logs` to the
  machine that ran it.

  `Leg::{Never, Here, Peer}` off `progress_leg`, crossed with the gossiped worktree note, in a pure
  function (`no_checkout_here`) for the reason the `agent claude-code , model` line became one:
  wording inside the arm that prints it is wording no test can fail on. The note matters because it
  gossips — `removed` is the same string on every node once the leg holding the checkout has torn
  one down — so a peer that has already removed its checkout is not somebody to send anybody to.

  **Then the fix introduced the same class of defect one arm over.** `(Leg::Here, not removed)` said:

  ```
  no checkout for … — it ran here and its worktree is gone; `offload audit <id>` says what
  happened to it
  ```

  Typed, on a real daemon, against a run whose last leg was still this node:

  ```
  2026-09-11 20:58  01a09243fe03  granted to 328223d2 at epoch 1
  2026-09-11 20:58  01a09243fe03  accepted here at epoch 1
  ```

  Not one word about a directory. `AuditEvent::Reclaimed` is the row that would be there, the
  checkout sweep is what writes it, and `note_reclaimed`'s own doc comment says the sweep **runs on
  the leg that has lost the run** — so a node holding a `Reclaimed` row is a node whose
  `progress_leg` names the *peer*, which renders `Leg::Peer`. The arm that named the command is the
  one arm the row is never in.

  So the row is read rather than pointed at (`Supervisor::reclaimed_here`), and said in the audit
  log's own words — `Reclamation::why`, lifted out of `AuditEvent::describe` so the two renderings
  cannot drift. Where there is no row, that is stated:

  ```
  no checkout for … — it ran here, its worktree is gone and nothing here recorded a teardown:
  the leg died before one was made, or it went from outside offload
  ```

  The general form, and the reason this one is worth the space: **a report that names a command is
  a promise that command has an answer.** It was caught only because the walk typed the command the
  sentence pointed at — the arm's unit test asserted `said.contains("offload audit")`, which is a
  test of the wording agreeing with itself.

- **A variant with no constructor is a fix that did not land.** `SubmitError::Task` is in the enum
  with a doc comment stating its whole purpose: *"A task could not be started. Its own variant
  rather than `SubmitError::Agent`, which prefixed the first walk's refusal with the word 'agent'
  for a run that has none."* `grep -rn 'SubmitError::Task\b'` returns the definition and nothing
  else. Meanwhile `drive_task`, four hundred lines away, maps the spawn failure with
  `.map_err(|e| SubmitError::Agent(e.to_string()))`.

  So the sentence the variant was added to prevent was still what every failing task said, in three
  reports at once:

  ```
  $ offload ps --all
  01a0942db7e8   failed   task   -  -  -   nightly
             └─ agent: the program for task `nightly` was not found: /tmp/…

  $ offload logs 01a0942db7e8
  failed: agent: the program for task `nightly` was not found: /tmp/…

  $ offload explain 01a0942db7e8
  state       failed — failed 8.6s ago: agent: the program for task `nightly` was not found: /tmp/…
  ```

  One word at one call site. The reusable part is the check: adding a variant and using it are two
  edits, the first is the one that feels like the work, and nothing fails if the second is skipped
  — the old variant still compiles and still renders a sentence.

- **Two commands warned about a task nobody could run and neither warned about one nobody could
  run.** `nominates_task` is the one fact behind `offload when --task`'s note and `offload every
  --task`'s, deliberately (*"the fact is asked here so a rule and a run cannot disagree about one
  fleet"*). It asked `task_for(&ctx.config, service).is_some()` for this node and
  `playing(Role::Execute)` for peers. Neither reads `authenticated`.

  Measured, with the control in the same pass:

  ```
  $ offload every 1m --task nightly          # nominated here, program missing
  schedule d96f542f2b43fd73
  first tick in 28.0s
    Quiet while it works: …
                                              ← nothing else. No warning at all.

  $ offload every 1m --task neverheardof     # the control
  schedule ecc3060940f73664
    Nothing in this fleet nominates a task for `neverheardof`, so every tick will be refused…
    Nothing counts a refused tick, so `offload schedules` will go on saying nothing has fired…
  ```

  `offload when tick --task nightly` was silent in the same way. Every tick and every firing would
  be refused for ever, and a schedule's refused tick is counted by nothing — which is the sentence
  the control gets and the broken one did not.

  This is the silence session seventy-three removed from this exact command, arriving again one
  cause over, through the same predicate, one session after ADR-0058 and two after the tier's own
  walk. The note now names both causes, because they are fixed by different actions and it cannot
  know which node it is about; the assertion in
  `a_schedule_for_a_task_nobody_nominates_says_so_at_the_keyboard` grew the second half.

  It says *cannot **run** the program it names*, not *cannot find* it, which was the first wording
  and was wrong for the arm the same session added: a file that is there without its execute bit
  is found perfectly well. A note that has to cover two causes has to be worded against the wider
  of them.

- **One machine, two names, and the reports that name *another* machine used the local one.** The
  mechanism and the measurement are in `detail/gossip-and-merge.md`; what belongs here is the
  screen. On two daemons that had both defaulted their config name to the hostname, `offload
  explain` for a run placed on the other machine printed:

  ```
  attendance  only fedora can tell; it is running there
  holder      fedora
  arbiter     fedora (this node)

  what each node says about taking it, asked just now:
    → fedora       bid 2, starting when the run ahead of it finishes
      fedora       already holds this run
  ```

  Five occurrences of one name for two machines, in the report whose job is saying which machine
  has the run — and the canvass listing the same name twice, once bidding and once already
  holding, which reads as a bug in the canvass. `offload nodes` beside it said `bravo`, for about
  twenty-five seconds. **A report that names a machine has to use the name the fleet knows it by**,
  and `offload status` showing the configured one is the documented other half rather than the same
  defect: the daemon warns at startup that the two differ and says which is which.

- **A refusal that names a node by its id, one line from two reports that name it.** `offload
  cancel` on a run whose holder is out of contact refuses rather than forwarding into a timeout —
  the right decision, documented, and never typed until this walk. What it said:

  ```
  Error: 21177c09 holds run 01a0947bca96 and is out of contact, so a cancel cannot reach its
         agent yet — it will be reclaimed or reassigned, and cancelling then reaches whoever answers
  ```

  with `offload explain` two lines later saying `holder bravo` and `offload nodes` saying `bravo
  dead`. `node_name(cluster, node)` was already in the same file, four lines below, used for the
  success line. The other two `holder.short()` in that handler are honest and stay: both are on the
  no-mesh path, where there is no view to look a name up in.

- **An empty expression is not a constraint that matches everything.** Measured:

  ```
  $ offload match ""
  match: this device satisfies the constraint
  $ echo $?
  0
  ```

  `offload match`'s own help says to compose it in scripts — *"Exit non-zero so this composes"* —
  so that is a gate that opens on an unset shell variable, answering the permissive way and
  silently. `parse("")` split on commas, filtered the empty pieces, found nothing left and
  returned `Constraint::Always`.

  **It was tested.** `empty_input_matches_everything` asserted exactly that, so this is a decision
  being changed rather than an oversight being fixed, and the commit says so. What the decision
  missed is the caller count: `constraint_expr::parse` has exactly **one** caller, `offload
  match`, whose `expr` is a required positional — so the only way to reach the empty case is to
  type an empty string, and there is no caller for whom "matches everything" is the useful answer.

  `ParseError::Empty` already existed and was **unreachable**: `clause()` returned it for an empty
  string, and `parse` filtered every empty clause out before calling `clause`. Second time in three
  sessions that a variant with no constructor turned out to be the fix that had not landed —
  `SubmitError::Task` was the first. It belongs to the whole expression, and that is where it is
  raised now.

- **…and the same parser required a value only where the value had to be a number.**

  ```
  cores>=            Error: `` is not a number          ← refused, by `number()`
  toolchain:rust>=   match: this device satisfies…      ← accepted: any version
  tag=               - has tag  (tags: docker, gh, …)   ← accepted: a tag named nothing
  ```

  `number()` and `size_mb()` each refuse an empty value; every clause whose value is a string
  passed it through. The check is in `split_op` now, which every clause goes through, and
  `ParseError::NoValue` names the clause the operator typed. `qualified` catches its own case
  first and keeps `NeedsValue`, because there the clause *is* known and an example of its shape —
  `toolchain:rust>=1.80` — is more use than the generic sentence. **When a check lives in the
  arms, count the arms.**

- **…and a bare word was told it is a clause that needs a value.** `split_op`'s no-operator arm
  answered `NeedsValue { clause: input, example: "cores>=8" }` for anything, so:

  ```
  $ offload match nonsense
  Error: clause `nonsense` needs a value like `cores>=8`
  ```

  — telling somebody to put a value on a clause that does not exist. The parser genuinely cannot
  tell a known clause from an unknown one at that point: the clause names live in one `match` in
  `clause()` and enumerating them in `split_op` would be a second copy of that list, which is the
  thing this tree keeps unpicking. So `ParseError::Bare` says what is true of both cases and names
  the two clauses that legitimately stand alone, which is the fact that distinguishes them:

  ```
  Error: `nonsense` is not a clause on its own — only `mains` and `unmetered` are.
         Give it a value, like `cores>=8` or `os=linux`
  ```

  Same posture as session eighty-one's `offload-core` arm: where a layer cannot see which cause it
  is, say neither and give the reader the fact that lets them tell.
- **A sentence in one node's first person, printed on another, names the wrong machine.** Twice in
  session ninety, both on two daemons. `offload continue` typed on alpha for a run bravo finished,
  after `offload rm` on bravo, printed *"run 01a0d4cc46c1 cannot be continued: bravo answered: run
  01a0d4cc46c1 cannot be continued: its worktree is no longer on this node"* — the building node's
  `SubmitError::Refused` rendered whole, prefix and all, and its *this node* read at alpha's
  keyboard. Now `continuation_base` takes the builder's fleet name (`Supervisor::fleet_name`) when a
  peer asked, the mesh hands back the bare reason, and the asker prints it once. The handshake had
  the same shape and cost more: a stale v30 alpha refused a v31 bravo, and bravo's log said
  *"refused by 29be31be: no common protocol version: this node speaks v30, the peer speaks v31"*
  — `Refusal`'s Display, written by the refuser and rendered by the refused, so the reader
  concluded bravo was the old binary. `the refuser speaks {ours}, and was offered {theirs}` reads
  right from both ends, and `handshake::tests::a_version_gap_refuses_with_both_ranges_in_the_message`
  asserts it contains no *this node*. The refusal travels as a structure and is rendered by the
  reader's build, so a v30 node still prints the old sentence. `WrongFleet` is left in the peer's
  words on purpose (ADR-0060).

*The entries below were backfilled in session ninety-one from `docs/sessions.md` and the commits
that added each rule — sourced, not reconstructed from the rule text.*

- **"Refused at the keyboard" has to be arranged on the fleet-of-one path separately.**

  Session sixty-six, third half (commit `5d34733`). `offload run --task push`, on a node
  nominating no such program, was *accepted* — then started and failed a millisecond later. With a
  cluster the bid round refuses an unsatisfiable constraint; with no cluster there is nobody to
  evaluate it against, so ADR-0014's promise has to be arranged on that path by hand. Otherwise: a
  run in the log, a notification, and a person finding out at breakfast what they could have been
  told at the keyboard.

- **…and the second walk of a task found four more sentences written when an agent was all a run
  could be.**

  Session sixty-seven (commit `32a93ef`). The roadmap item was a *kind* column in `offload ps` —
  one line of `println!`. Walking it found `TURNS` printing `0` for work with no turn boundary;
  `offload explain` labelling `Work::summary` as `prompt` over a shell script's service; the
  closing log line and the notification projected from it saying `finished after 0 turn(s),
  $0.0000` — a sentence that goes to somebody's phone; and `offload policy` answering `would
  accept work: no — no agents installed` on a device that nominates a task, which is ADR-0019's own
  scenario told the opposite of the truth.

- **`offload when --help` documented a flag clap refuses.**

  Session sixty-eight (commit `73ce8e3`). `--on-notice`'s help text gave the example `offload
  when failed --on-notice --prompt "…"`. The prompt is positional, so that line exits 2. Written one
  session earlier in the ADR's own vocabulary and never typed.
- **An invitation cut short in copying must say so.** Found on the product app's Join card: an
  invitation typed in with `adb input text` came through missing its end, and Join showed
  `invitation is not valid fleet state: EOF while parsing a list at line 1 column 93`, under a
  heading that said "Done". The base64 decoded, since it was cut on a 4-character boundary, and
  serde hit the end of the JSON. `serde_json::Error::is_eof()` and base64's `InvalidLength` and
  `InvalidLastSymbol` now map to one sentence that names the fix. The app shows refusals under
  "Not done".

- **A field a client can leave empty is checked where the spec is built, not where it is used.**

  Session ninety-two. The product app's submit dialog had a "Repository URL" field. The owner left
  it empty for a simple prompt, and the run was accepted by the laptop and failed there with `git
  clone --bare --quiet  /tmp/mw/a/repos/.repo-….partial failed (128): fatal: The empty string is
  not a valid path`. The CLI had never sent an empty repository, because it defaults to `.`, so no
  path had ever met one. Fixed as `SpecProblem::NoWorkspace` in `RunSpec::check` (ADR-0072), which
  names the three things a workspace can be.
- **A queued task printed no id.** Seen on session ninety-four's delivery walk, submitting a task held with `--require os=windows --queue`. `offload run --task … --queue` said `task queued — ` and the
  canvass, so the one thing somebody types next (`cancel`, `explain`) had to be found with `ps`. The
  agent path always named it. Worded the same now; two copies of one outcome drift apart.
