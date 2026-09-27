# Refusals and instructions

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/refusals-and-instructions.md`, same order.

Refusals, errors and config-validation messages, and the instructions a person acts on: what a
refusal names (a machine, a command, a cause) and what it must never claim without checking.

- **One refusal, two renderings.** The operator's is computed fresh and carries a duration
  (`describe_refusal`); the one **stored and gossiped** carries none (`summarise_refusal`), because
  a number written down is read back as a claim about now. It must fit `ps`'s **18**-character
  column — checked by a test that measures every phrase and refuses any with a digit in it.
- **…and that column gossips, so a wrong reason in it is the whole fleet's answer.** `waiting for a
  slot` was written for a run held by a *rate limit*, with the real refusal in hand at that line.
- **A stored reason is a sentence about a moment.** Print it in the past tense, and ask the live
  question at the instant it is answered (`rule_run_in_flight`).
- **"Refused at the keyboard" has to be arranged on the fleet-of-one path separately.** With a
  cluster, a constraint nobody satisfies is refused by the bid round; with no cluster there is
  nobody to evaluate it against, so `offload run --task push` on a node nominating no such
  program was accepted, started and failed a millisecond later. ADR-0014's promise is that a
  submission is accepted or refused while the operator is still there, and a path that has no
  round in it has to ask the question itself. Name what the node *does* offer while refusing:
  the commonest cause is a service spelled differently in `node.toml`.
- **…and the second walk of a task found four more sentences written when an agent was all a run
  could be.** The rule above said to run one and read the output; doing it again found `offload
  ps` printing `0` in `TURNS` for work that has no turn boundary (a dash now — `TOKENS` and
  `COST` were already right by accident, since a task spends neither), `offload explain` labelling
  `Work::summary` as `prompt` over a shell script's service (`kind` and `work` now, two lines,
  because `agent` as a label would collide with the agent's own *name*), the run's closing log
  line and the **notification projected from it** saying `finished after 0 turn(s), $0.0000` — on
  somebody's phone — and `offload policy` answering `would accept work: no — no agents installed`
  on a device that nominates a task, which is **ADR-0019's own scenario** told the opposite of the
  truth. The tier rides on the event (`LogKind::Finished::work`, a defaulted field rather than a
  variant, for `CaptureFailed::run_continues`'s reason) because `notify::notable` is a pure
  function of one event and cannot look up a spec. **A kind column is not one column**: it is
  every column, label and sentence beside it that was a claim about one tier.
- **`offload when --help` documented a flag clap refuses.** `--on-notice`'s own doc comment gave
  the example `offload when failed --on-notice --prompt "look at what broke"`; the prompt is
  positional, so that line exits 2 with *"unexpected argument '--prompt' found"*. Written one
  session earlier in the ADR's own vocabulary and never typed. The help text of a flag added with
  its feature is the one string nothing exercises — `--help` is not a test and neither is an ADR.
- **Three commands offered an answer to a question and each offered a different half.** `offload
  logs` printed `offload approve <run> …` — a literal placeholder, in the only line of that log
  that is an instruction — while `offload asks` printed the id but named only **approve**, in
  front of a `WANTS TO` column that is as often `rm -rf` as `cargo test`. The drain's line had both
  and was the only one. When the same advice is rendered in more than one place, the divergence is
  not in whether it is *true*; it is in which half got left out, and each site is written by
  somebody looking at a different screen.
- **A refusal computed from the *string* cannot tell two machines' reasons apart.** `offload run
  --repo <a directory that is not a git repository>` was refused with *"is a path on another
  machine"* — said by the node it was typed on, about a path sitting right there. `LocalFacts::
  repo_available` was a `bool`, so the only thing left to word the message with was
  `RepoSource::parse(repo).portability()`, which is derivable by anybody anywhere and answers
  `NodeLocal` to both *nothing of that name is here* and *it is here and there is nothing to
  clone*. Measured on one daemon: the same sentence for `/tmp/ow/notarepo` and for a path that
  exists on no machine at all. `RepoReach::{Obtainable, NotHere, NotARepo}` is measured by
  `WorkspaceManager::reach` — the only thing in the path that can see a filesystem — and travels
  to the refusal, so `NoBid::RepoUnavailable` carries what was observed rather than what can be
  re-derived. The tell is the same as the `Option<T> == Some(x)` one two entries up, and the
  general form of both: **a report built from a second derivation of the fact is a report that
  cannot see what the decision saw.** Found while drafting ADR-0061, not by a walk.
- **Giving a command an identifier of the *wrong type* is sound across the board, and the sweep is
  worth not repeating.** Rule and schedule ids are the same shape — sixteen hex characters — so
  confusing them is plausible, and run ids are twelve. Six crossings measured on one daemon holding
  one of each: `unwatch <schedule>`, `unschedule <rule>`, `unwatch <run>`, and `logs`, `explain` and
  `cancel` each given a rule or schedule id. Every one answers `no <thing> matching <id>`, which is
  true and names the type it was looking for. None claims a wrong cause. A *"that is a schedule id,
  did you mean `offload unschedule`"* hint would be nicer and is deliberately not built: it couples
  each command to the other's table for a convenience, and the listing that resolves it is one
  command away.
- **An empty name in a config block is a *missing field*, not a wrong one.** Every block carrying a
  service (`[[sinks]]`, `[[triggers]]`, `[[resources]]`, `[[tasks]]`) declares `service: String`
  under `#[serde(default)]`, so omitting the line yields `""` rather than a deserialisation error —
  and `UnknownService`'s one sentence rendered it as `invalid config: : not a service this build
  can offer`. A sentence opening with a colon, about a name nobody wrote, reading as *this build
  does not know that service* when the truth is *you left the line out*. One `Display`, four blocks.
- **A number in a help string is a number that drifts.** `offload run --archive`'s help said *"the
  fleet moves at most 512 MiB in one exchange"* — true, hardcoded, and one constant change away
  from being a confident lie in the one place somebody reads before doing the expensive thing. The
  help now says the limit exists and where to read it; `offload status` prints the **constant**.
  Same rule as a stale count in a doc, met in argument help, where it is worse: nothing ever
  re-measures a `--help`.
- **…and a new `#[serde(default)]` field reads as a real value, not as silence.** `NodeStatus::
  max_archive_bytes` is zero on a daemon too old to send it, and `archives up to 0 bytes` is a
  wrong sentence rather than a missing one — it says the fleet will take nothing. The line is
  omitted at zero. The CLI and daemon ship together so this is only the mixed-version moment, and
  that moment is exactly when somebody is confused already.
- **One refusal covering four causes is right about one of them.** `offload rm` on a node with no
  checkout said *"it was removed already, or the run finished on another machine, where `offload
  rm` is what discards it"* — for a **task** (no workspace on any machine by construction,
  ADR-0019 §2), for a run **cancelled out of the queue** (nothing was ever made anywhere), for a
  checkout **already torn down here**, and for a run that finished **on a peer**. Three are false
  in both halves, and the fourth named no machine while the answer sat in this node's own `runs`
  row — `RunProgress::by`, which `log_source` one function up already reads to route `offload
  logs`. `Leg::{Never, Here, Peer}` plus the gossiped worktree note, in a pure function. **When a
  refusal offers two possibilities, count the states that reach it.**
- **…and a report must not name a command it has not checked has an answer.** The fix's own
  `Leg::Here` arm then said *"`offload audit <id>` says what happened to it"* — measured on a
  daemon against a run whose last leg was still this node, and the log printed `granted`,
  `accepted` and not one word about a directory. `AuditEvent::Reclaimed` is what would be there
  and the sweep is what writes it, and the sweep runs on the leg that has **lost** the run
  (`note_reclaimed`'s own doc comment), which renders `Leg::Peer` — the arm that names it is the
  one arm it is never in. The row is **read** now (`Supervisor::reclaimed_here`) and said in the
  audit log's own words via `Reclamation::why`, and its absence is stated rather than papered
  over. A defect introduced *by* a fix for the same class, caught only because the walk typed the
  command the sentence pointed at.
- **A variant with no constructor is a fix that did not land.** `SubmitError::Task` exists, and its
  doc comment says why — *"its own variant rather than `SubmitError::Agent`, which prefixed the
  first walk's refusal with the word agent for a run that has none"*. Nothing ever constructed it.
  So the sentence it was written to prevent was still the one a failing task printed, in three
  reports at once: `offload ps`, `offload logs` and `offload explain` all said `agent: the program
  for task 'nightly' was not found`. The fix is one word at the one site that maps the error. The
  rule is the cheap check: **when a variant is added to fix a sentence, grep for the site that has
  to produce it** — the variant and the mapping are two edits and only the first is satisfying.
- **Two commands warned about a task nobody could run and neither warned about one nobody could
  run.** `offload every --task` and `offload when --task` both go through `nominates_task`, which
  asked `task_for(config, ..).is_some()` locally and `playing(Role::Execute)` for peers — neither
  reading `authenticated`. So a schedule or a rule bound to a `[[tasks]]` entry whose program is
  missing was accepted **in silence** and refused at every firing for ever, while the control — a
  service nobody nominates at all — printed the whole paragraph. That is the exact silence session
  seventy-three removed from `offload every`, arriving again one cause over, in the same command,
  through the same predicate. The note names both causes now, because they are fixed by different
  actions and the note cannot know which node it is about. **When a predicate gains a second way to
  be false, the sentence it feeds has gained a second meaning.**
- **One machine, two names, and the reports that name *another* machine used the local one.** A
  device's config name defaults to its **hostname**; the fleet gave it a name on its certificate.
  Every fleet-facing sentence — `offload nodes`, `offload explain`'s holder, arbiter and canvass
  rows, `offload cancel`'s `on <node>`, and the `cancelled from <node>` line in a run's own log —
  read the gossiped config name. Measured on two daemons neither of which set `name` in
  `node.toml`: **both were `fedora`**, so `offload explain` printed one name for two machines and
  the canvass listed `fedora` twice, once bidding and once already holding the run. The mechanism
  is in `gossip-and-merge`; what belongs here is the shape — **a report that names a machine has
  to use the name the fleet knows it by**, and `offload status` showing the local one is the
  documented other half rather than the same bug.
- **A refusal that names a node by its id, one line from two reports that name it.** `offload
  cancel` on a run whose holder is out of contact is correctly refused rather than forwarded into
  a timeout — and said `21177c09 holds run …`, while `offload explain` two lines later and
  `offload nodes` one command away both said `bravo`. `node_name` was already in the same file and
  used four lines below. Session seventy-six's finding from the other side: there, a command
  demanded a form no report printed; here, a report printed a form no other report uses.
- **An empty expression is not a constraint that matches everything.** `offload match ""` printed
  *"this device satisfies the constraint"* and exited **0** — a gate that opens on an unset shell
  variable, in a command whose own help says to compose it in scripts. `parse("")` answered
  `Constraint::Always`, and a test named `empty_input_matches_everything` pinned it, so this was a
  decision rather than an oversight: what the decision missed is that `parse` has exactly **one**
  caller and its `expr` is a required positional, so there is no caller for whom that answer is
  useful. `ParseError::Empty` existed and was **unreachable** — `clause()` raised it for an empty
  string and `parse` filtered every empty clause out before calling `clause`. Same root as session
  eighty-two's empty run id, two commands over.
- **…and the same parser required a value only where the value had to be a number.** `number()` and
  `size_mb()` refused an empty one; every clause taking a string did not. So `toolchain:rust>=`
  quietly meant *any* version and `tag=` meant a tag named nothing, while `cores>=` was correctly
  refused — one rule applied to a third of the clauses. It is in `split_op` now, where every clause
  passes. **When a check lives in the arms, count the arms.**
- **…and a bare word was told it is a clause that needs a value.** Anything with no operator got
  *"clause `nonsense` needs a value like `cores>=8`"*, which sends somebody to put a value on a
  clause that does not exist. The parser cannot tell a known clause from an unknown one without a
  second copy of the clause-name list, so it says what is true of both and names the two that
  legitimately stand alone (`mains`, `unmetered`) — which is the fact that distinguishes them.
  `qualified` keeps `NeedsValue`, because there the clause *is* known and an example of its shape
  is the useful thing.
- **A sentence in one node's first person, printed on another, names the wrong machine.** Twice in
  session ninety: a continuation's base refusal, *"its worktree is no longer on this node"*, built
  on bravo and printed at alpha's keyboard, and the handshake's *"this node speaks v30, the peer
  speaks v31"*, rendered on the node that spoke v31 — which sent the walk looking for a stale
  binary on the wrong machine. **Before a refusal crosses the wire, find every place it is
  printed**: a reason a peer composes is worded by name (`Supervisor::fleet_name`) or from neither
  end (*the refuser speaks …*), never as *this node*. ADR-0060's `WrongFleet` is the deliberate
  exception — stored as the peer's own words, and rendered as a quotation.
- **An invitation cut short in copying must say so.** `decode_invite` answered a truncated token
  with serde's "EOF while parsing a list at line 1 column 93", which points nobody at the fix. The
  commonest way an invitation fails is losing its tail between printed and pasted: a terminal
  wrap, a chat app, `adb input text` (which drops the end of a long string). EOF and a bad base64
  length now read "is incomplete — it was cut short … Copy the whole line". Tested by
  `an_invitation_cut_short_says_so`.
- **A field a client can leave empty is checked where the spec is built, not where it is used.**
  The app sent `repo = ""`. The daemon accepted it and placed it, and the holder failed a minute
  later at `git clone ''`, on another machine, with git's words. `RunSpec::check` refuses it now,
  before placement. A second client is what finds this: the CLI always filled the field with `.`.
- **A queued task printed no id.** `offload run --task … --queue` said `task queued — ` and the
  canvass, so the one thing somebody types next (`cancel`, `explain`) had to be found with `ps`. The
  agent path always named it. Worded the same now; two copies of one outcome drift apart.
