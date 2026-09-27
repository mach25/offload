# Capacity, work policy, capability and probing

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/capacity-policy-and-probing.md`, same order.

- **A refusal has to name a clause wherever it sits in the list.** `failures()` descends only
  through unsatisfied branches, so a `Not` whose children all held reports itself — and the check
  for that asked whether *anything* had been collected rather than anything of its own.
- **A policy field nothing can set is worse than one that does not exist.** `allowed_agents` was
  enforced and settable by nothing. Refused at **deserialize** time, because `work_policy` is
  called from the gossip tick and must not be fallible. An empty list is refused too: it reads as a
  restriction and means "refuse everything".
- **…and it has a sibling, which `offload policy` printed four lines away.** `metered_network`
  was a `bool` the probe set to `false` unconditionally, under a comment calling it an
  assumption, and nothing else could write it. So `Refusal::MeteredNetwork` was unreachable,
  `offload match "unmetered"` said yes on every device there has ever been, `accepts_replica`'s
  metered check never fired, and `offload policy` said `metered network refused` on every device.
  (No submission can carry that constraint yet — `offload run` has no `--require` — so the
  constraint half was a wrong answer to a person, and the other three were load-bearing.) Three-valued now
  (ADR-0045), nominated in config and asked of NetworkManager.
- **`Unknown` is not resolved once, at the type.** The four readers disagree on purpose: a policy
  refusal claims the bytes cost money, a constraint claims they do not, and unknown proves neither
  — so **the burden of proof lies with whoever makes the claim**. Refusing work on unknown would
  stop every machine without NetworkManager from hosting or holding replicas, which is the same
  over-claim pointing the other way. "Unknown is not good news" means unknown must not be
  converted into the convenient answer, not that it is always the pessimistic one.
- **A guess the platform labels as a guess is not an answer.** NetworkManager reports
  `no (guessed)` for every wifi and ethernet link — including a laptop tethered to a phone, which
  is the first case the field's own doc comment names. That maps to `Unknown`, not `No`: it is
  the same guess the old `bool` was making, and promoting it would have left the bug in place
  with a nicer implementation.
- **…and a node that says `accepting no` has to mean it at every door that starts an agent.** A
  daemon reporting `accepting no` accepted a submission at its own socket and ran it to completion:
  the no-cluster arm asked the drain and the grant and not `WorkPolicy` (ADR-0046). Then the same
  measurement on `offload resume`, which asked **none** of the three — including on a **revoked**
  node whose run ADR-0044 had just halted, which resumed and went on taking turns (ADR-0047). One
  `hosting_refusal`, called by both doors, and `permits` rather than `admits` because the queue
  half is each path's own. The rule underneath: **a gate assembled one clause at a time, on one
  door at a time, is the shape that hides** — every one of those fixes was correct and none asked
  what *else* starts an agent.
- **…and a probed fact is only as good as the last probe, which for five phases was the first
  one.** `Ctx::capabilities` was built once in `main` and never replaced; the re-probe told
  `Cluster::set_capabilities` and nobody else, from inside the loop a fleet of one never enters.
  Harmless while nothing local decided with it, and ADR-0046 and ADR-0047 made three readers do
  exactly that. Measured on one daemon with the link turned metered underneath it: `accepting
  yes`, a submission refused by the bid round for being metered, and `offload resume` starting an
  agent — seconds apart. `deliver::Current` is the one copy now and `reprobe` the one writer,
  spawned with or without a fleet (ADR-0048). **The two clauses that can change under a running
  daemon are the battery floor and metered**; `accept` and `allowed_agents` come from config and
  need a restart, so those two were the whole reachable surface of both earlier ADRs.
- **A grant constrains placement only while it cannot be honoured remotely.** Once a call can be
  proxied, `Constraint::CanUse` pins a run to the one device that cannot host it. What survives is
  the question asked at submission: does *anybody* have one.
- **Don't over-claim in the probe.** An unverifiable auth is `false`. A node that claims capability
  it lacks wins bids and then fails every run it takes — much worse than idling.
- **…and the probe has to be asked about the program that will actually run.** It asked `claude` on
  `PATH` while the supervisor spawns `agent.binary`. Both directions were live: a wrapper or pin
  advertised another program's version, and with nothing on `PATH` a working node never bid.
  `probe_with_agent`, and `deliver::capabilities` is the single place probed facts meet nominated
  ones.
- **…and which *account* it runs as was a fact about whatever shell started the daemon.**
  `[agent] config_dir` nominates it and wins outright rather than only when the variable is unset.
  It must reach **three** places — the probe, the capture, the spawn — and two of three is the
  failure that hides. `spawn` **tells** the child rather than trusting inheritance.
- **…and a nominated directory answers for its own account or not at all.** `account_fingerprint`'s
  `~/.claude.json` fallback is a sound hedge for a *guessed* path and a wrong answer for one
  somebody wrote down. Skipped exactly where the directory was nominated.
- **The account's own usage limit is a signal something has to remember** (ADR-0029). Latest wins
  per kind; a block with no stated reset is not remembered at all. It gates **starting** — that is
  not the "load gates accepting" rule, which is about a number nobody controls; this lifts at an
  instant the agent stated.
- **A `From` impl that defaults a decision is a forgotten call site with no compiler error.**
  `From<Capacity>` filled the rate limit with `None`. `Supervisor::room` is the funnel, and the
  right owner anyway: the limit is this node's own observation.
- **A capability only the owner can state still has to be settable.** `max_concurrent` is a probe
  guess of 2 and a hard cap, so the third run was refused by a limit no config could express.
  `[agent] max_concurrent`. Three ceilings and the lowest happens — the status line says which
  binds, because printing one is a node refusing work at 2 under a line that says 4.
- **Not every battery on a machine powers the machine.** A mouse, keyboard, touchscreen and
  controller each register with a `capacity`. The kernel has the discriminator (`scope=Device`,
  `present=0`) and nothing asked it. `settle` is a pure function of the parsed directory, because
  the bug was reachable only through `read_dir` order. Several system batteries report the
  **lowest**; one that will not parse is `Unknown`, never `Ac`.
- **Capacity gates how many agents *run*, not how many runs are *held*.** A commitment fills a slot
  for accepting more work and occupies nothing for deciding what to start next. Answering the
  second with the first deadlocks a node that took accept-without-starting at its word.
- **A cap that gates accepting but not starting is decorative.** The per-account cap gates both —
  unlike observed *pressure*, which gates accepting only, because a run waits behind the owner's
  build (undrainable) rather than behind runs that finish. Collapsing the asymmetry either way is a
  bug.
- **The run being started must never count against its own ceiling.** A held run is `Assigned` and
  visible, so counting it made every commitment the reason it could not begin.
- **A node must not ask the view about its own runs.** Gossip is a tick stale.
  `AccountUse::elsewhere` is peers-only and the name says so; the local number comes from the
  caller, which is also how the two different local questions each get answered correctly.
- **The account fingerprint is a wire format.** Compared only *across* nodes, so two nodes
  computing it differently never disagree loudly — accounting silently stops. It hashes the account
  **uuid**, never the email; the API-key path hashes the key's *value*, because the label it
  replaced was an env var name and made every key-authenticated node one account.
- **Capacity belongs to the device, not the fleet.** Two `offloadd` instances cannot see each
  other's runs, so the broker's ledger counts for the machine — and it has to reach the **bid**,
  not only admission. An unreadable ledger is a loud warning and the old behaviour.
- **A budget limits what runs *beside* something, never what runs at all.** A lone `Heavy` run
  always fits, or a concurrency number has become an eligibility rule. Hardware a run genuinely
  needs is a `Constraint`, which fails loudly and names itself.
- **Load gates accepting, never starting.** A committed run has nowhere else to be. The way out is
  `review_commitment` giving it back — offering it first — and only for a deadline somebody
  *stated*.
- **Being full and being under pressure are two different sentences.** Full is this node's own
  queue and empties by itself, so it commits (`Availability::WhenFree`). Under pressure is the
  owner's build, so it defers (`NoBid::Busy`) and only when slack can afford the wait.
- **An unreported load average is not an idle machine.** `cpu_load_percent` is an `Option`; the
  rules that read it skip rather than assume.
- **Capability match is necessary, not sufficient.** Policy, capacity, battery and rate limits gate
  separately, and each refusal must stay distinguishable in the output.
- **A battery floor is about running out, so it never applies on mains.** A second copy of the rule
  that forgot had a plugged-in laptop refuse every checkpoint replica, with a `WARN` as the only
  symptom.
- **Scoring resources linearly drowns out every other signal.** Cores and memory score per
  *doubling*; agent runs are model-latency bound, and a warm workspace beats a bigger box.
- **Agents cost money and rate limit.** Concurrency caps are per-node *and* per-account; nodes
  sharing an account share one limit.
- **`ready_elsewhere` read ability, and hosting needs permission.** It counted nodes that are
  alive, match the run's constraint and have room by gossiped capacity — `capabilities` and
  `policy`, which is every gate `NodeView` carries, and which is exactly the vocabulary's own
  split: capabilities are *ability* and a work policy is what the *owner* permits. The fleet's
  `host-runs` grant is a third thing and was not consulted, so a **submit-only member counted as
  ready** — the default at the door, deliberately. Measured: `ready=1` on a run that peer could
  not take, `refusals=2` a millisecond later, and on the backoff for ever. Inside
  `review_commitment`'s stated tolerance — its docs say being wrong costs a bid round — but that
  tolerance was written for a *stale* estimate that self-corrects, and this one was permanent: a
  release, a refused round and a take-back, two epoch bumps, every thirty seconds all night.
- **…and "the view has no field for it" was a claim about the view, when the question was never
  about the view.** The first write-up of the entry above concluded it was unfixable from the
  reading end and needed a new gossiped fact with an owner (ADR-0005). That was wrong, and one
  function further on in `place.rs` says so: the arbiter **already holds every peer's
  certificate**, from the handshake, and `hosting_objection` reads it to refuse a bid it does not
  believe. The fix is `Cluster::peer_hosts_runs` beside it and one shared `permits_hosting`
  predicate, so the estimate and the enforcement cannot disagree — no gossip, no wire bump, no
  ADR. **Before concluding that something needs a new gossiped field, look at what the connection
  already carries**; a new field is the expensive answer and this codebase's handshake is richer
  than its view on purpose (ADR-0015 §1 keeps addresses there too).
- **Unknown is not `false` here, and that is the exception rather than the rule.**
  `peer_hosts_runs` answers `None` where there is no live connection, and the caller **counts it
  as ready anyway** — the opposite of the codebase's usual "unknown is not good news". It is right
  for this one question because `review_commitment` says which way to be wrong: being wrong costs
  a bid round, being unwilling to estimate costs the run its night. Read it as the rule working
  rather than as an exception to it: the safe direction is a property of the decision, not of the
  word *unknown*, and here it points the other way. The change is then strictly subtractive — it
  can turn a doomed round into no round, and can never turn a possible handover into a run left
  on a busy machine.
- **A fallback for "the platforms that have no DMI" was written with a `/sys` read.** The
  device-class fallback named containers, macOS and BSD in its comment and then called
  `power::has_battery`, which reads `/sys/class/power_supply` — so on macOS it answered *no
  battery* unconditionally and **every Mac came out `DeviceClass::Unknown`**, a MacBook included.
  The MacBook half was invisible because `Unknown` and `Laptop` both map to `Transient`: the right
  answer for the wrong reason. The Mac mini half is not invisible — a mains-only desktop scored
  `Transient` bids below what it is. `PowerSource` is the signal that travels, because
  `settle`'s own arms define `Ac` as *mains present and no battery* and `detect_macos` reaches it
  through `pmset`. **A cross-platform fallback has to be reached by a cross-platform question**;
  check the one you already have before adding a branch.
- **…and `Desktop` is claimed on macOS only, which is the asymmetry worth keeping.** `Ac` there is
  a positive answer about a lineup where no battery means a Mini, Studio, Pro or iMac. On Linux,
  reaching that fallback at all means there was no DMI — a container or an odd board — and
  `Stability::Stable` is exactly the claim this crate must not make on a guess, since it moves bid
  scores and a node that over-claims wins bids and then fails the runs it took.
- **A refusal written for "nothing can construct this yet" outlives the yet.** `bid::evaluate`
  refused every `Work::Task` at its first line, with a comment saying so honestly — and the tier
  went on to gain a submission path, a supervisor and a config, so the last thing left refusing it
  was the round. The effect: `offload run --task` worked on a node with `cluster.enabled = false`
  and was refused at the keyboard on every default node, `this node cannot host task work`. **A
  placeholder is only as honest as its last reading**; when a type stops being unreachable, grep
  for what refused it while it was.
- **A gate takes a whole `Tier`, not the agent it used to assume.** Three of `WorkPolicy`'s
  clauses are about an *agent* — `allowed_agents`, the agent's own `max_concurrent`, an
  account's ceiling — and the rest are about the device. `permits`/`admits` take
  `Tier::{Agent(&kind), Task}` (from `Work::tier`) so the three are asked only of work that has
  one, and **an enum rather than `Option<&AgentKind>`**: a `None` that skips three of the owner's
  controls is one an agent-run caller reaches by accident from a `RunSpec::agent()` that happened
  to be empty, and `allowed_agents` has already been a control defeated by an ordering once.
  What must *not* be skipped is the other direction: `accept`, the battery floor, `allow_metered`
  and the demand budget are facts about the machine, so a phone that will not work on battery
  does not run a shell script on battery either — a tier that skipped those would be a way
  around the owner's own answer.
- **…and a field that has no meaning for this kind of work must not be read as though it did.**
  `LocalFacts::repo_available` is computed for a task too — `can_obtain("")` — and comes out
  `false`, so one hoisted `if` refuses every task on every node for a repository nobody named.
  Same shape one line down: `account_limited_until` is an *agent account's* rate limit, and a
  node blocked until 09:00 can run a shell script now. Both are asked inside the arm that has an
  agent, not above it.
- **A gate that can answer two ways makes every report of it a claim about one of them.**
  `[policy.light]` gives `accept`, `min_battery_percent` and `allow_metered` a second answer for
  `Demand::Light` (ADR-0019 §4), and the moment it existed `offload status` was wrong: measured on
  a daemon configured `accept = "never"` with `[policy.light] accept = "always"`, `accepting no —
  node is not accepting work` printed one command after that node ran a task to completion. Both
  sentences were true; the pair was the answer. **Every field is an override whose `None`
  inherits**, so a config that says nothing behaves exactly as it did — and a stated floor of `0`
  is how an owner says *any charge will do*, since `None` is spoken for. The three resolvers
  (`accept_for`, `battery_floor_for`, `allows_metered_for`) are what the gate *and* the report
  read; a report that worked out for itself whether the light form applies is the second copy of
  the rule this file is mostly about.
- **…and the demand has to reach every door, which means finding out what each door is about.**
  `permits` took a tier and now takes a demand too, and the callers split cleanly in two: where a
  run is in hand its own `spec.demand` is the question (`submit`, `submit_task`, `resume`,
  `explain`, and the recovery tick, which now asks **per run** through a closure rather than once
  for the pass), and where there is no run the honest question is about an ordinary one —
  `ORDINARY_RUN`, named so the choice is visible rather than implied by a literal. The two that
  had to be looked up rather than guessed: `hosting_allowed` decides whether a *failed run* is
  resumed, so it takes the run; and `ready_elsewhere` is deliberately **not** given the standing
  doors at all, because its documented tolerance says which way to be wrong (over-estimate
  readiness; being unwilling to estimate costs the run its night) and a peer's gossiped battery
  is a second old.
- **A knob that may only tighten is refused when it loosens, not clamped.** `policy.max_archive_bytes`
  (ADR-0061 §4) sits under a structural ceiling nobody's config can raise, so a larger value is a
  config error naming both numbers — *"there is no setting here that would make a larger archive
  travel"*. Clamping would leave an owner believing their fleet moves more than it does until a run
  failed somewhere else entirely, which is the expensive way to learn a constant.
- **…and `None` on a new ceiling means the existing limit, never *no limit*.** ADR-0061 said the
  opposite until the blob plane was measured, and its reasoning — a limit invented for a case
  nobody has hit is a knob nobody understands — was sound and aimed at the wrong ceiling. Before
  adding an `Option` ceiling, ask what is already enforcing one.
- **Two limits, one reported number.** `offload status` prints the *effective* archive limit —
  the fleet's, tightened by the owner's — because one number is what an agent packs to. Printing
  both and leaving somebody to work out which binds is the report shape this tree keeps removing.
- **A refusal by one owner's policy must not read like a refusal by the fleet.** `NoBid::
  ArchiveTooLarge` ends *"another node may still take it"*; the structural cap is refused before a
  run exists and says the archive has to be smaller. They send somebody to opposite places, and
  collapsing them would be `NoBid::RepoUnavailable`'s own defect repeated.
- **A `None` size is not a zero size.** `WorkspaceSpec::archive_bytes` is absent for anything that
  did not know to state it, and refusing on absence would turn a missing field into a policy
  decision. The node finds out at acquisition instead — slower, and correct.
- **A clause that asks whether somebody *nominated* a program is not the one that asks whether they
  have it.** `build_task` placed the cheap tier on `Constraint::HasService { role: Execute }` —
  ADR-0019 §1's own words — and a `[[tasks]]` block is three fields of which one is a path. So a
  node whose `command` names nothing on disk satisfied the constraint, bid, won its own round and
  failed the run a millisecond later; on a fleet of more than one it bids *against* the node that
  can do the work. The bit was already measured on every probe (`task_capabilities` sets
  `authenticated` from `which`) and the clause that reads it already existed and was constructed
  nowhere: `Constraint::ServiceAuthenticated`. The **resource** tier one ADR earlier gets this
  right (`CanUse` is `is_resource() && authenticated`), which is the tell — two applications of one
  rule, and the newer one dropped the bit. ADR-0019 is amended.
- **…and `authenticated` is one bit with a different meaning per role, so the sentence about it
  cannot be one sentence.** For a sink it is a credential; under `Role::Execute` it is *this device
  can run the program its owner nominated*. `explain` said `has nightly as execute, authenticated`
  and the observation beside it said `task:nightly (execute, unauthenticated)` — both sending
  somebody to look for a login for a shell script. Worded per role now, in both halves, and **two
  cuts of it were wrong before the walk agreed**: `runs nightly, program present` printed a
  *requirement* as though it were the finding, one word after `ineligible:`; `with its program
  there` then asserted a cause `offload-core` cannot know, since it holds the bit and not the
  filesystem. It says `and can run it` / `this node cannot run it`, and the node-local reports name
  which. **A clause printed in front of what was observed has to read as the thing asked for, and
  must not claim more than the layer it is in can see.**
- **The probe checked a file was there and not that it could be run.** `which` answered
  `is_file()`, so a nominated program without its execute bit was advertised as usable by all four
  kinds that name a command. Measured: `offload status` called it fine, the bid was won, and the
  spawn failed with `Permission denied (os error 13)`. Same over-claim as a missing program, one
  permission bit in, with nothing on screen to tell them apart. `executable` is `cfg(unix)` — said
  as a `cfg` rather than assumed, so the day this daemon runs elsewhere the omission is visible.
- **…and the first fix returned an `Option`, which threw the measurement away.** Every report then
  said *its program was not found* about a file sitting right there with mode 644 — a confidently
  wrong cause, in six places at once, introduced **by** the fix. `resource::Program::{Runnable,
  NotFound, NotExecutable}` with a `why()` for the gossiped sentence and a `refusal()` for the
  node-local one, which may name the command. **Third time in this tree that a two-valued answer
  to a three-valued question has cost a session** — `can_obtain` before `RepoReach`,
  `steward_of(..) == Some(me)` before `Steward`, and `task_for(..).is_some()` before `Nomination`
  in this same walk. The tell is always the same: the fix is at the *decision*, and the decision
  only needs to know **whether**.
- **…and there were two resolvers, which is why only one of them knew anything.**
  `deliver::resolve` and `resource::which` were the same function with two drifts: that one
  understood a relative path with a separator in it and this one did not, and neither asked about
  the execute bit. One resolver now, for every nominated program in the daemon. **Two spellings of
  one fact is how they come apart**, and the tell is that the fix for one of them is invisible in
  the other.
- **One field carried two different things depending on who set it.** `Capability::description` is
  the owner's one-line *label* for the four nominated kinds and a *reason* for the one arm that
  refuses an agent whose login does not match its config — and `offload probe` prints it as a `└─`
  clause under `NOT authenticated`, which its own comment calls *why*. So a missing program printed
  `└─ posts the nightly report to slack`: a sentence that reads as the explanation, is not one, and
  sends somebody to the wrong thing. `Capability::unusable(reason)` sets the bit and keeps both, in
  one order, in one place. The reason gossips, so it states what the device cannot do and never a
  path — ADR-0010's rule, which is also why the label is the owner's words and never the command.
  **`offload status` reads that description from the capability rather than from the config**, so
  the one line that states the cause is the one the measurement wrote; the `bool` beside it says
  only `NOT usable`, because a `bool` cannot say which of two causes it is and spent a phase
  asserting the wrong one.
- **A report has to print the spelling its own config accepts.** `offload policy` printed
  `accept work WhenCharging` with `{:?}`, and a config saying `accept = "WhenCharging"` is refused:
  *"unknown variant, expected one of `never`, `when_charging`, `always`"*. That refusal is good and
  self-correcting, which is why this cost a detour rather than a dead end — it is still the one
  command whose whole job is saying what the policy in force **is**, printing it in a form its own
  input rejects. `AcceptWork` has a `Display` now, asserted against **serde's own** rendering so a
  renamed variant cannot part them again. The rest of the `{:?}` in operator output round-trips:
  `offload match`'s parser is case-insensitive, so `class=Laptop`, `arch=X86_64` and
  `stability>=Transient` all parse — checked rather than assumed.
- **…and "it parses" was the wrong bar for the rest.** `Constraint::explain` printed `os == MacOs
  (have Linux)` under `--require os=macos`, and `have Battery { percent: 57, charging: false }` for
  `mains` — a struct literal, which nothing parses. The capability enums and `PowerSource` have a
  `Display` in the grammar's spelling now (session ninety), and `offload policy` and `offload
  status` print `laptop`; `DeviceClass` and `Stability` are asserted against serde's.
- **A heading that claims the owner said something must be true of every line under it.**
  `[policy.light]` is three independent overrides and `None` inherits (ADR-0019 §4) — so a block
  naming `accept` alone still printed three lines under *"the owner has said what this device does
  with it"*, with the **ordinary** battery floor among them. Measured with `accept = "always"` and
  nothing else: `battery floor 40%`, which is the main policy's number and changes when that one
  does, without the light block being touched. Each line says `(inherited)` now, in a column. The
  values still come from the `*_for` resolvers the gate itself calls; what the report reads from
  `policy.light` is only *whether the owner stated it*, which is a different fact and is the one
  the heading was wrong about.
- **The agent's own ceiling bound the bid and stopped nothing.** `WorkPolicy::admits` refuses with
  `Refusal::AgentAtCapacity` when `caps.agent_details(kind).max_concurrent` is reached, and
  `Room::for_one_more` — which **every path that starts an agent** goes through — has no such
  clause. `admits` is called by exactly one thing: `bid::evaluate`. So the number `offload status`
  prints as *"claude-code sustains 2, which is what binds"* shaped what a node **bid** and bound
  nothing, and `AgentAtCapacity` was a refusal that could be computed and could never stop a run.
  Measured on a fleet of one with `cluster.enabled = false`, where `admits` is never reached at
  all: three submissions accepted without a word, `runs 3/3 · claude-code sustains 2, which is
  what binds`, and **three agent processes** — 50% over an install's stated ceiling, on the
  configuration most of this project's testing uses. `set_agent_concurrency`'s own doc calls that
  number *"a cap"*.
- **…and it is four doors, which is the count that keeps being the lesson.** `Supervisor::
  agent_full` is asked by the submission (`submit_built`, the whole of placement with no mesh), by
  the start (`start_refusal`, which `take_run` now calls instead of its own second copy of the
  capacity arithmetic), and by `resume` — the door a person types at *and* the one the recovery
  tick uses unasked, which is ADR-0047's argument met again one ceiling over. `restart_task` is
  the fourth caller of that shape and needs nothing, because a task names no agent. **The count
  each door passes is its own** — a submission asks what is *committed*, a start asks what is
  *going* — which is the split `Room::for_one_more`'s callers already draw for the machine's own
  capacity; passing the count in is what keeps the ceiling from becoming a third opinion about
  which runs to count.
- **…and the supervisor had to be told what the device is.** The ceiling is a *capability*, and
  the supervisor had none — it is `Arc<deliver::Current>` now, wired at startup beside the mesh,
  the same way `Supervisor::room` overlays the rate limit it alone observes. `None` means nothing
  has told it, which is tests and nothing else; that is written down at the call site because it
  is the line that goes quiet if another construction path ever appears.
- **A capability measured to the unit is an incarnation every probe.** Any change to
  `Capabilities` bumps the node's incarnation and re-gossips the lot, and `disk_free_mb` was probed
  to the megabyte, so every machine that wrote a log line "changed" every thirty seconds, for ever.
  Whole GiB now, **rounded down** (the probe must not over-claim). Before adding a probed number to
  `Capabilities`, ask how often it moves while nothing about the device has changed.
- **A score term cannot reach past the gate in front of it.** `load_penalty` is 30, but a
  normal-demand node under 25 % idle does not bid at all, so no bid carries more than −22 for load.
  A table of what a weight can do has to be read against `admits`, which decides who is scoring.
- **…and free disk was measured where the daemon was started, not where it writes.** `probe` read
  the process's working directory beside a comment saying worktrees live under the state dir. The
  daemon now asks `free_disk_under(state_dir)`; the config-less `offload probe` still asks the cwd,
  because that is the only path it has.
- **A phone app may not read `/sys/class/power_supply`, so a real phone's power was unknown.** Under
  Termux on Android 16 the directory is `Permission denied` (the emulator's shell reads it fine),
  and a default phone (`when_charging`) then refused all work while plugged in, saying it was *not
  charging*. Now `Refusal::ChargingUnknown` says it cannot tell, and `termux-battery-status` (killed
  after 5 s, since it hangs without the Termux:API app) supplies the answer where it is installed.
  **Check a platform fact from an app's sandbox, not from a shell with more rights.**
- **A load average the process may not read is not 0 %.** An Android app is denied `/proc/loadavg`,
  and sysinfo answers a failed read with 0.0, so the phone app claimed `cpu 0%` for ever and scored
  as idle. `cpu_load_percent` asks the file itself on Linux and Android and says `None` when it is
  denied. The Windows guard beside it had covered the only platform anyone had thought of.
- **Every Android device was a phone.** `detect_device_class` mapped `Os::Android` to `Phone`
  outright, so a Samsung tablet failed `--require class=tablet`. The app now reports `form` in
  `host-facts.json` by Android's own line (smallest width ≥ 600 dp), and only a phone can become a
  tablet that way. The two share a policy and a stability, which is why nothing but a constraint
  noticed.
- **A host's facts go stale because the phone is asleep, so staleness cannot mean "not quiet".**
  `host_facts` refuses a file older than 120 s, which is right for battery good news. But a
  suspended phone stops the app's writer too. Read that way, the quiet decision (ADR-0078) would
  flip out of quiet at every packet that woke the phone. Quiet reads `last_host_facts`, whatever
  its age, and the app rewrites the file on every screen-on and screen-off. Checked by nothing but
  this entry and the doc comment.
