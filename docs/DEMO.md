# Running the three-node demo

Everything below was hit while verifying arbiter failover. None of it is deep; all of it
costs twenty minutes if you meet it cold.

- **Plan the fifteen minutes.** A joining node's `host-runs` is dormant for `PROBATION`
  (ADR-0012), so `init`, `join`, `grant host-runs` come *first* and the demo comes later.
  The founder has it immediately, which makes it the natural holder.
- **`offloadd` does not read a config file it was not given.** `--config <path>` is the only
  way in; `OFFLOAD_STATE_DIR` alone leaves every other setting at its default, which is how a
  second node ends up binding the first one's socket path — or, under a deep state dir,
  failing to bind at all (`path must be shorter than SUN_LEN`).
- **Prompts that ask for one file per turn do not get one file per turn.** Every long prompt
  this session finished in two to five turns, whatever it said, which makes the states worth
  demonstrating — orphaned, held down, mid-migration — a matter of killing a node within a few
  seconds of the run starting. Pace by making each turn *depend* on the last, and expect to
  miss the window anyway.
- ~~**mDNS advertises the machine's LAN address regardless of what you bound.**~~ *Fixed.* An
  advertisement now carries the address that was actually bound: a wildcard bind fills in every
  interface (right for a laptop moving between wifi and a dock), a specific bind announces exactly
  that, and a **loopback bind announces nothing at all** and says so once, with `seeds` named as
  the way out. The old behaviour published this machine's *other* addresses and sent peers
  somewhere nothing was listening.
- ~~**A dead node is re-concluded dead once a period, for ever.**~~ *Fixed.* The conclusion is
  said once; the re-probing continues, because that is how a node comes back. Verified: one
  `marking dead` line across forty seconds where there used to be one per period.
- ~~**`offload fleet | head` panics.**~~ *Fixed, and the decision it needed turned out to be made
  already.* Restoring the default `SIGPIPE` disposition needs `unsafe`, which this workspace
  **forbids** — and that lint is worth more than the papercut. So the panic is caught instead: a
  panic whose message starts `failed printing to std…` is a reader that has gone away, and the
  process exits 0. The coupling is on a std message and fails backwards-safe — if it ever changes,
  the behaviour is today's panic rather than something worse — and the predicate is a tested
  three-line function rather than a string buried in a hook.
- **A peer's name comes from its *certificate*, not its config.** `record_name` adopts the name in
  the membership certificate on purpose — it is signed, so it is the one nobody else could have
  chosen — and `offload init` mints that certificate before any config exists, so it uses the
  hostname. The result: a daemon configured as `desktop` shows up on every peer as `fedora`. Not a
  placeholder and not replaced by gossip, which is what the earlier note here guessed. The daemon
  now says so at startup (`configured=desktop certified=fedora`) rather than leaving somebody to
  wonder; making the two agree would mean enrolment reading the daemon's config, which is a
  decision rather than a patch.
- **A daemon told to stop does not stop for up to five minutes.** Shutdown drains first
  (`drain_deadline_secs`, default 300), and a run still mid-turn is waited for — so it keeps its
  state-directory lock, and the `offloadd` you start next fails with "another offloadd is already
  using …" while the *old* one goes on answering the socket. Everything then looks like the new
  binary behaving strangely. Set `drain_deadline_secs` low in demo configs, and check
  `pgrep -x offloadd` before concluding anything.
- **`pgrep -f "offloadd --config…"` matches the shell that typed it**, which is the documented
  `pkill -f` trap one variant on: the bash process running the compound command has the pattern in
  *its* command line, so `kill -TERM $(pgrep -f …)` kills the walk. Record `$!` into a file when you
  start each daemon and signal that. Cost one restart of a two-daemon walk.
- **A one-shot dial has QUIC's handshake timeout as its grace period — 30 seconds, measured.** So a
  walk that starts the peer "a few seconds later" is not testing what it looks like: the first
  attempt is still in flight and succeeds, and the bug hides. Wait for the failure to appear in the
  log (`until grep -q "could not reach seed"`) before starting the other end.
- **`pkill -x offloadd` also kills a blocked `ask-hook`, which is the thing you were measuring.**
  The hook is `offloadd ask-hook` — the daemon's own binary (ADR-0017) — so it matches the process
  *name*. Killing the daemon that way releases the very question the run is blocked on, the agent
  finishes, and the run you were about to observe mid-turn is `completed` before you look. Keep the
  daemon's pid (`nohup … & DPID=$!`) and signal that.
- **…and `until ! pgrep -x offloadd` then waits on the hook rather than the daemon** — which is
  five minutes of patience, not the two seconds you meant. Same cause, one line further on: a walk
  that waits for a daemon to exit has to wait for a pid.
- **A blocked question keeps a `SIGTERM`ed daemon alive for its whole patience**, because the
  shutdown drains first and a run mid-tool-call reaches no turn boundary. Measured at 3m43s, ended
  in one second by answering the question — which is what ADR-0035 is about, and is also how to get
  a demo daemon to exit when it looks stuck.
- **`pkill -f offloadd` kills the shell that typed it.** The pattern is matched against every
  process's full command line, and the shell running the compound command has `offloadd` in
  *its* command line too — so the daemon and the demo both die, with an exit code that looks
  like a crash in something else. `pkill -x offloadd` matches the process *name* and is what to
  use. Cost twenty minutes twice in one session.
- **…and it is not only `offloadd` that is spelled in the shell's own command line.** A
  `pkill -f "agent-slow.sh"` to clear a stuck fake agent killed the compound command that
  contained the string, mid-walk, with exit 144 and a half-staged state dir. Any `pkill -f` whose
  pattern appears anywhere in the command you are typing kills the walk; `pkill -x <name>`, or a
  pid recorded when the thing was started, are the two that do not.
- **…and `pgrep -f <pattern>` piped into `kill` is the same trap wearing a different hat.** The
  entry above is about `pkill`, so `BPID=$(pgrep -f "offloadd --config /tmp/r/b.toml"); kill -9
  $BPID` reads as the safe version — and it is not: `pgrep -f` matches the shell running that
  line too, because the pattern is in its command line, so `$BPID` holds the daemon's pid *and the
  shell's*. The compound dies at that statement and everything after it silently does not run,
  which looks like the daemon failing to start. Hit twice in one session, once through each of the
  two spellings. The rule is about `-f`, not about which tool: match on `-x`, or on a pid recorded
  with `echo $! > pidfile` when the thing was started.
- **…and a fake agent that must *checkpoint* needs two more things.** The capture reads a real
  transcript, so the script has to write one — anywhere under `$CLAUDE_CONFIG_DIR/projects/*/`
  named `<session-id>.jsonl`, since `transcript::find` falls back to scanning. And the moment you
  relocate `CLAUDE_CONFIG_DIR` you also move `.credentials.json`, so the probe reports the node
  **unauthenticated** and it refuses every run with "ineligible: claude-code authenticated" —
  `echo '{}' > $CLAUDE_CONFIG_DIR/.credentials.json` is the whole fix. Both are the same sentence
  CLAUDE.md already has about the agent's state directory being movable, met from the demo's side.
  Append to the transcript per turn if you want the *growth* that makes superseded checkpoints
  worth looking at.
- **…and a fake transcript is only counted if its rows carry `message.id`.** The capture refuses a
  transcript holding no conversation, and "conversation" means what `usage::from_transcript`
  counts: an `assistant` row with **both** `message.id` and `message.usage`. A row with usage and
  no id parses fine, counts zero, and the run reports `the agent's transcript holds no
  conversation yet at turn N` at *every* boundary — so the run never checkpoints, never releases,
  and finishes on the node you were draining. It reads exactly like the drain failing. Cost one
  pass of the ADR-0041 walk. The shape that works, one line, one row per turn:
  `{"type":"assistant","message":{"id":"m1","role":"assistant","usage":{"input_tokens":100,
  "output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}` — and write
  it *before* the assistant text event that ends the turn, since that is the order a real agent
  writes them in and the capture reads the file at the boundary.
- **A fake agent can be paced from files rather than its environment.** The daemon is already
  running when you decide how long a turn should take, and its environment is fixed. A script that
  reads `$(dirname "$0")/turns`, `/longturn` and `/gap` can be re-paced between passes with
  `echo`, which is what makes "drain while it is mid-turn" a reliable window instead of a race: a
  thirty-second first turn against a ten-second `drain_deadline_secs` puts the deadline in the
  middle of the turn every time.
- **`offload drain` is one-way, so a repeated walk restarts the daemon between passes.** The
  second pass otherwise submits to a node that refuses everything, the run lands on the *other*
  node, and the drain you then type reports nothing to hand over — which looks like the fix
  failing and is the flag doing its job.
- **A timer can only be walked with the timer shortened.** ADR-0022's pass is every fifteen
  minutes with an hour of grace, so a walk means editing the two constants, proving the wiring,
  and putting them back — which is worth doing rather than skipping, because what the tests cannot
  check is that the loop is *spawned* and that a live run checkpointing through a pass is
  untouched.
- **A fake agent has to answer `--version` now.** The node probes the binary it will spawn
  (session twenty-one), so a script that does not is a node with no agent that refuses every run.
  One line: `case "$1" in --version) echo "9.9.9 (Claude Code)"; exit 0;; esac`. Before that fix
  the trick worked by accident of the two disagreeing — the script ran the runs and a real
  `claude` on `PATH` supplied the capability, which is that bug seen from the demo's side.
- **A fake MCP client has to hold its pipe open.** The resource proxy treats the agent's stdin
  closing as "finished with it" and tears the session down — correct, and it means a stand-in that
  pipes one line and exits gets nothing back and never even starts the program on the holder. Ten
  minutes in session twenty-two. `( echo '<line>'; sleep 5 ) | offloadd use-resource --socket …
  --run <full id> --service email` is the shape that works, and the run has to still be alive.
- **A fake agent can pose a real permission question.** The hook is `offloadd ask-hook` reading
  JSON on stdin, and the supervisor puts `OFFLOAD_ASK_{RUN,EPOCH,SOCKET}` in the agent's
  environment — so a script that pipes `{"tool_name":..,"tool_use_id":..,"tool_input":..}` into it
  drives the whole of ADR-0017 with no model spend. What it cannot fake is the *route*: with no
  `[[sinks]]` anywhere in the fleet no question is put at all, which is correct and is what made
  the budget look broken for ten minutes.
- **A fake agent is the cheapest way to demonstrate anything the model would decide.** Point
  `agent.binary` at a shell script that prints `--output-format stream-json` lines — a `system`
  init, an `assistant` message, whatever event is under test, then a `result` — and the whole
  daemon runs against it with no model spend and no waiting. It is how session nine drove a
  `rate_limit_event` with a chosen `resetsAt`; `/bin/false` (session eight's dead agent) is the
  degenerate version of the same trick. One thing it cannot do by default: there is no transcript, so
  the first checkpoint fails loudly and harmlessly, and it never gets far enough to exercise
  resume.
- **…and the transcript half of that is fixable in three lines, so a fake agent can be
  checkpointed.** The adapter puts the config directory on the agent's environment and derives the
  project directory from the cwd by turning `/` into `-`, so the script can write its own:
  `slug=$(pwd | sed 's|/|-|g'); mkdir -p "$CLAUDE_CONFIG_DIR/projects/$slug"; echo '{"turn":1}' >
  "$CLAUDE_CONFIG_DIR/projects/$slug/s1.jsonl"` — with `s1` matching the `session_id` the script
  announces in its `system` init line. `transcript::find` scans the projects directory when the
  computed path misses, so the slug rule does not have to be exactly right. This is what let
  session thirty-one's turn-limit test assert on a real capture with no model spend.
- **Pick the model for the test, and start at the cheap end.** `offload run --model <id> …`, or
  `[agent] default_model` in the demo node config; the run's log names the model that answered, so
  there is never any doubt which one did. Cheapest first, list price per 1M tokens in/out:
  **Haiku 4.5** `claude-haiku-4-5` $1/$5, 200K context · **Sonnet 5** `claude-sonnet-5` $2/$10, 1M
  · **Opus 5** `claude-opus-5` $5/$25, 1M · **Fable 5** `claude-fable-5` $10/$50, 1M. The ids carry
  **no date suffix**. Measured here on one identical two-turn task: Haiku **$0.0205**, Sonnet
  **$0.048**.

  Default to Haiku, because what these walks test is *offload's* machinery — checkpointing,
  fencing, migration, what the reports say — and none of it depends on the agent's reasoning.
  Before paying for a bigger model, try fixing the **prompt**: Haiku batches its tool calls hard
  (26 files in 3 turn boundaries), which starves a walk of the boundaries it needs, and pacing it
  with a *dependent* chain — each step must read the previous file before it can write the next —
  fixes that for nothing. Escalate only for a reason you can name: many well-spaced turn
  boundaries under careful instruction-following (Sonnet); a transcript that will outgrow **200K**
  on a long run, which is a hard limit rather than a prompt problem (Sonnet or above); or the
  agent's own judgement being the thing under test (Opus). Session twenty-three's migration walk
  cost about $1.30 on the default, which is why "a migration read with a real agent" sat on the
  unwalked list for three sessions as something to *decide* rather than start. It is not that any
  more — the ungraceful re-walk that found the leftover-agent hole cost about fifteen cents.

- **A daemon that started before its node was a member has no mesh until it is restarted.**
  `offload grant` says "a daemon running on this device picks the new certificate up within a
  second, no restart", and that is true of the *certificate* and not of the mesh: a daemon that
  came up non-member logged "not a member of a fleet, so there is nobody to talk to" and stayed
  that way through `join`. `offload nodes` said "This node is not in a mesh" on the joiner while
  the founder listed only itself. Restart the joiner after `join`; probation is unaffected,
  because it is timestamped on the certificate.
- **Haiku finishes a twenty-step dependent chain in well under a minute**, which makes the window
  for draining a run mid-conversation *seconds* rather than tens of seconds — 29, 37 and 62 turns
  in three runs, two of which finished before the drain landed. The recipe that works: submit
  **without** `--follow`, poll `offload explain <run>` for `holder` (it is answered within about
  0.2s), and drain *that* node immediately. Do not assume the run landed where you submitted it —
  in all three attempts it was placed on the other node, and `offload drain` correctly reported
  "nothing to hand over" on the node that never had it.
- **A paced prompt must give every step a tool call.** Under `--print` a run ends at the first
  assistant turn that makes no tool call — that is what a final answer *is* — so a step that says
  "make no tool call this turn" ends the run, correctly and confusingly. Cost one run of the
  session-23 walk: step 1 said "think of a word and say it, no tool call", and the run was over.
- **A nested agent inherits the outer session's tool list, not just its shell rules.** Driving the
  demo from inside a Claude Code session, one run called `ScheduleWakeup` — a tool of *this*
  harness, not of a bare `claude` — ended its turn intending to resume in 105 seconds, and the
  `--print` run was therefore finished at step 1 of thirty-one. It reads exactly like the daemon
  stopping the agent early. Same family as the `sleep` note below, one layer up, and the reason a
  paced demo prompt is not a reliable instrument from in here. Check the transcript's `tools` list
  before believing what a stalled demo run appears to say.
- **An agent outlives the daemon that spawned it — and does *not* die on its own.** The first
  version of this note guessed it would take EPIPE on its next write to the closed stdout pipe,
  "which is a whole turn away". Wrong, and measured wrong: the orphan ran for another minute and
  **finished its entire task**. That turned out to be a real hole rather than a demo curiosity,
  and it is fixed — `crate::leftovers` writes the agent's process group down and a starting daemon
  sweeps what the last one left. What is worth keeping from the note is the walk technique: after
  a `kill -9` the agent really is still there, so check for it, and expect the restarting daemon
  to say `an agent outlived the daemon that spawned it; stopping it`.
- **The harness refuses `sleep N` followed by another command**, and rewrites what it does accept.
  A `sleep 12; kill -9 <pid>` written as one call ran the kill immediately, which is how the first
  ungraceful attempt killed a node that had not yet reached a turn boundary — so the reclaim
  started from no checkpoint at all, correctly, and proved nothing. Wait with an `until` loop on a
  condition (`until [ $(grep -c "checkpoint recorded" log) -ge 8 ]; do sleep 2; done`) and the
  pacing is honest.
- **Do not build a long-running demo prompt out of `sleep`.** Running the demo from inside a
  Claude Code session, the *nested* agent inherits the harness's shell rules and quietly
  rewrites a foreground `sleep` into a background one — so a prompt paced with sleeps runs
  flat out. Pace with turns instead: N small files, one per turn, is ~3 seconds each and is
  what the checkpoint cadence measures anyway.
- **…and the same trap catches a *waiter*, not just a killer.** `until ! pgrep -f "cargo test"; do
  sleep 10; done` never exits: the shell running the loop has `cargo test` in its own command line,
  so it waits on itself for ever, and meanwhile every other check that asks the same question is
  told the build is still running. `pgrep -x cargo` matches the process name. The `pkill -f`
  entries above are the same rule met from the other side; it is one rule.
- **A busy machine refuses every run, and a `cargo test` in another terminal is busy enough.** The
  policy reads live cpu load: `no node will take this run — cpu at 100%: 100% of the budget is
  free, the machine is not`. It reads like a broken fix and is the policy working. Check `uptime`
  before concluding anything about a walk that will not place.
- **A scratch directory is deeper than a unix socket is allowed to be, and `socket` is the way
  out.** The control socket lives under `state_dir` by default, so a state dir under a
  session-scoped temp path blows `SUN_LEN` (108) and the daemon fails to bind — the trap the
  `--config` entry above already names, met from the side where you cannot simply choose a shorter
  directory. `socket = "/tmp/<short>.sock"` is a top-level config field and moves only the socket,
  leaving worktrees, blobs and the database where they were. Remember to remove it with the rest.
- **`max_concurrent_runs` is under `[policy]`, and `drain_deadline_secs` is under `[cluster]`.**
  `deny_unknown_fields` catches both, loudly and at startup, which is the design working — but the
  error names the *field* and not the section it belongs in, so it reads as "this setting does not
  exist" when the truth is "not here". The parser's own error lists the valid keys for the level you
  put it at, and that list is the fastest way to find where it does go.
- **…and a daemon has to be restarted after `offload init` too, not only after `join`.** The
  founder's certificate is minted by the CLI, so a daemon already running logged `not a member of a
  fleet, so there is nobody to talk to` and stayed that way — no mesh, no bid round, and therefore
  no way to reach ADR-0006's held run on a fleet of one. After the restart: `member of fleet …
  grants=submit,deliver,host-runs,approve` and `listening for peers`. Same entry as the joiner's,
  one line further back in the sequence.
- **A memory claim is worth an A/B on the daemon, and it is cheaper than it sounds.** Reverting the
  one line under test with `sed`, rebuilding (incremental, six seconds) and re-running the same
  pass gives a before and an after on the real process rather than an argument. Two things make the
  numbers usable: read RSS from `/proc/<daemon pid>/statm` field 2 rather than anything that
  includes the CLI, and make the workload **exactly** N runs — `offload run` without `--follow`
  returns as soon as the run is submitted, so a loop of 200 against `max_concurrent_runs = 4` and
  no cluster has most of them *refused* (81 of 200 landed, and the per-run arithmetic was quietly
  wrong). `--follow` serialises the loop and costs nothing here, because the fake agent's run is a
  fraction of a second. Repeat each side once: two passes agreeing to 5% is what makes the
  difference believable.
- **Two daemons on one machine share an account and a device ledger, so `max_concurrent_runs` is
  not the machine's limit.** With both nodes at `max_concurrent_runs = 1`, node-a reported
  `runs 1/1 · accepting no — at capacity` while running *nothing*: the one run was on node-b, and
  both daemons resolve the same `acct:…` from the same `HOME` and share ADR-0013's device-local
  broker. So a submission to the idle node is held rather than started, and a walk that wants two
  runs in flight has to raise the number on both. It reads exactly like the capacity fix being
  broken. `offload status` is where it shows: `runs N/M` counts what the *machine* is doing.
- **Staging a run that comes *back* to a node needs four things and races past three of them.**
  A residue in `Supervisor::live` only matters when the same run is granted to the same node again
  with no restart in between, and a restart clears the map — so the drain path cannot produce it.
  The sequence that does: run it on A, `offload checkpoint` on A (which leaves the residue and
  parks the run), `offload resume` from **B** so it runs there while A stays up, fill A with two
  long local runs, then `kill -9` B. A's orphan timer is about 2m11s, and the run must arrive while
  A is still full or `take_run` takes the start branch and re-registers, which quietly bypasses the
  whole thing. Two of three attempts did exactly that before the fill was made deterministic.
- **`offload init` prints the passphrase once and re-running it refuses**, which costs a rebuild if
  the first output scrolled past. `offload join --passphrase` and `offload grant <g>` both read it
  from **stdin** (`printf '%s\n' "$P" | offload grant host-runs`), so a walk can script them — but
  `grant` takes the passphrase *only*, while a `yes` line in front of it is consumed as the
  passphrase and fails with `that passphrase derives fleet …`.
- **`PROBATION` is fifteen minutes and a walk with a second node has to shorten it**, the same way
  ADR-0022's pass does: `offload-core/src/fleet.rs`, rebuild, walk, put it back. It gates
  `host-runs` on the *check* rather than on the certificate, so shortening it works retroactively
  and restoring it leaves an already-enrolled node fine.
- **A fleet of one with `[cluster] enabled = false` cannot demonstrate a *held* run.** At capacity
  a submission is **refused** — `--queue` too, since with no cluster there is nothing to leave it
  pending for. ADR-0006's accept-without-starting is reached through a **grant**, so `take_run` is
  only entered from `place`. Turning the cluster on with `listen = "127.0.0.1:0"`, `mdns = false`
  and no seeds gives a one-node bid round that grants to itself, and then `--queue` behind a
  running run produces the real thing: `accepted; holding it until it can start`, `waiting for a
  slot`, and `there is room now; starting the run held for it` when the one ahead finishes. That
  is the whole of `start_held_runs` walked, on one machine, with no second daemon and no
  probation.

- **A whole walk fits on one daemon when what you are testing is a *record*.** ADR-0042's three
  cases all reduce to: release a run into a pool nobody will take from, then restart the daemon.
  A fleet of one with `[cluster] enabled = true`, `listen = "127.0.0.1:<port>"`, `mdns = false`
  and no seeds gives a real bid round that refuses (the draining node does not bid for itself), so
  `offload drain` and `kill -TERM` both reach the interesting state with no second node, no
  `join`, and no shortened `PROBATION`. The restart is the measurement: on the fix the run is
  placed within a second, and the same node bids for it because it is no longer draining.
- **…and the honest before-and-after is `git stash`, not a `sed` revert, when the fix spans
  crates.** A one-line `sed` in `supervise` is the right A/B for *one decision* and was used for
  exactly that. It cannot show what a whole shipped mechanism did, because the code it replaced is
  gone from the tree. `git stash push`, `cargo build`, a **fresh state dir and socket**, walk,
  `git stash pop` — twenty minutes, and it is what turned "the old build would have stranded this"
  into a timestamp. Use a separate state dir: the two builds write the same rows and only one of
  them has the new field.
- **`sed "s|$W/a|$W/old|"` over a config also rewrites `$W/agent.sh`.** The prefix matches, and
  the node comes up with `binary = "/tmp/olw/oldgent.sh"` — which is not a missing-file error but
  `ineligible: has agent claude-code (not installed); claude-code authenticated (not installed)`,
  from a probe that shells out and gets nothing. It reads exactly like the fleet refusing the run
  for a policy reason. Anchor the substitution or write the second config out in full.
- **The store is `state.db`, not `runs.db`.** A reset between passes written as `rm -rf
  $W/*/runs.db*` silently deletes nothing, and the next pass starts with every record from the
  last one still there. It reads as the new build picking up a run out of nowhere — which it did,
  and correctly: a `Pending { let_go_by }` record from the previous pass, republished the moment
  its daemon came back, is ADR-0042's whole property demonstrating itself by accident. Remove
  `state.db*`, or the state dir.
- **A fake agent that must *fail* needs to fail only on the first leg.** `exit 3` with no `result`
  row is what makes a run `Failed` — that is `fail_if_unfinished`, and it is the input to every
  recovery decision. But the same script runs again on whichever node picks the run up, so an
  unconditional failure produces a run that fails everywhere and proves nothing. Read `--resume`
  out of the argument list and succeed when it is present: the second leg then completes, and the
  turn count in `offload ps` (2 where it failed, 4 where it finished) is the evidence that the
  conversation was continued rather than restarted.
- **A failed run's recovery backoff is the window a walk has to act in**, and it is 30 seconds
  from `RecoveryPolicy::default()`. `offload explain` prints it — `recovery picking it up again in
  25.1s (try 1)` — which is both the countdown and the confirmation that the run reached the state
  under test: unattended, resumable, and waiting. Drain inside it. After it, the node resumes the
  run itself and the case is gone.
- **`offload init` and `offload join` read the *state directory*, not the socket.** `--socket` is
  accepted and produces a note rather than an error, and the command then answers about the
  default state dir — which on a machine that has walked before belongs to an old fleet, so
  founding fails with `this node already belongs to fleet …` about a node that does not. The flag
  is `--state-dir` and it goes **after** the subcommand (`offload init --state-dir $W/a`); clap's
  error says so, which is the fastest way through it.
- **Two daemons on one machine get independent capacity by moving `XDG_RUNTIME_DIR`.** The note
  above says the device ledger is shared and that `max_concurrent_runs` is therefore not the
  machine's limit; the missing half is how to separate them for a walk that needs one node full
  and the other free. `broker::default_path` is `$XDG_RUNTIME_DIR/offload/…`, and its own doc
  comment says `OFFLOAD_STATE_DIR` deliberately does not reach it but a single-machine rehearsal
  needs to say where it is. A different `XDG_RUNTIME_DIR` per daemon is that lever, and the proof
  is one command: node-a `runs 0/1 · accepting yes` while node-b is `runs 1/1 · at capacity`.
- **Give the nodes distinct names, or the whole walk is ambiguous.** A peer's name comes from its
  certificate, and both certificates default to the hostname — so `offload explain` says `holder
  fedora` on a two-node fleet where both nodes are `fedora`, and a commitment that landed on the
  peer reads exactly like one that landed here. `offload init --name alpha` and `offload join
  --name bravo`. Cost one pass of the `GiveBack` walk, which was staged, measured and only then
  found to have been measuring the wrong node.
- **`pkill -x offloadd` starts a drain, and a draining daemon keeps its socket.** The next daemon
  then fails to bind with `a daemon is already listening on … — unlinking it would leave that one
  running and unreachable`, which is the `statedir`/`bind` guard working — but the CLI happily
  talks to the *old* daemon, which answers `accepting no — drained` about a state directory you
  have just recreated. It reads as a fresh daemon coming up drained. `pkill -9 -x offloadd`,
  then `until ! pgrep -x offloadd`, then remove the sockets; and kill the fake agents separately,
  because an agent outlives the daemon that spawned it.
- **Staging a released commitment needs one node full, one peer that cannot host, and a slow
  round.** `offload run --deadline 30s` onto a node that is already at `max_concurrent_runs`
  reaches ADR-0006's accept-without-starting; a peer that joined and was never granted `host-runs`
  keeps `ready_elsewhere` at 1 while guaranteeing the round is refused; and `bid_window_ms = 10000`
  against a *killed* peer holds the round open long enough to kill the daemon inside it, which is
  the only way the take-back does not run. Poll the log for `offering it to the fleet` and kill on
  the match — it was 65ms wide in practice, and a `sleep` would have missed it.

- **Anything whose only symptom is on a *live* daemon has to be walked without a restart** — and
  the restart is the thing a walk reaches for when something looks stuck. Session forty-two's
  revocation hole is invisible the moment either daemon comes back, because what survives it is an
  inbound connection and there is no such thing after a restart. The A/B that shows it: revoke,
  watch the arbiter's log for the `no answer: suspecting` / `answered: no longer suspect` pair
  alternating once a second (**at `debug` since session ninety-two**, so run with
  `RUST_LOG=info,offload_cluster=debug`), and read `offload nodes` for a peer that is `alive ~N`
  with N climbing. The `~N` count is the part that needs no log level.
  A node still `alive` while its absence count runs away is a node something keeps re-contacting.
- **Staging a revocation against a run needs the run on the node you are going to revoke**, which
  is fiddly because placement will not oblige. `accept = "never"` under `[policy]` on the *other*
  node — the one submitting — makes it control-plane only, so the run has exactly one place to go;
  then restart that node without the line so it can take the run back, which is what you are there
  to watch. Two restarts, and both are of the node that is not holding anything.
- **…and there is a cheaper staging when what you are watching is the *revoked* node.** Revoke the
  **founder**. It has `host-runs` from the moment `offload init` returns, so it hosts without
  waiting; the second node joins with `submit, deliver` only, which is enough to type `offload
  revoke` at (that needs the passphrase, not a grant) and enough to refuse the founder's handshake
  afterwards. No `accept = "never"`, no restarts, and no fifteen minutes of probation — which is
  the tax the obvious arrangement pays, since `offload grant host-runs` and `offload invite --grant
  host-runs` both probate. The whole setup is `init`, `id`, `invite`, `join`, two daemons.
- **A walk about capacity has to start from a clean device ledger.** ADR-0013's broker is
  machine-wide and lives under `$XDG_RUNTIME_DIR/offload/`, outside every state directory a walk
  resets — so reservations left by daemons an earlier pass `kill -9`ed are still there, and a fresh
  fleet on an empty store opens with `starting when one of the 2 runs ahead of it finishes` about a
  node holding nothing. It reads exactly like the bug under test. `XDG_RUNTIME_DIR=$W/xdg` per pass,
  removed with the state directory. Cost one confusing before-and-after, where the *after* looked
  worse than the before.
- **A run submitted in the first seconds after a daemon starts is refused for CPU.** `cpu at 91%:
  100% of the budget is free, the machine is not` — the machine is still busy with whatever
  started the daemons, and the probe is honest about it. Eight seconds of patience, or a walk that
  reads as the policy being broken.

- **Staging a grant that is issued and never confirmed takes three daemons and no probation.**
  The founder hosts from the moment `init` returns; two joiners with `submit, deliver` cost
  nothing and need no `grant`. One of them submits — `accept = "never"` under `[policy]`, so it
  arbitrates and never bids — and the other exists only to be **frozen**, which is what holds the
  round open between the founder's bid and the grant that follows it. `kill -STOP` the spare,
  submit with `--queue`, `kill -STOP` the founder a second later, and the round comes back
  `did not confirm` with its epoch spent. No `accept = "never"` gymnastics on the host, no
  fifteen minutes, no restarts.
- **…but which peer holds the round open is decided by key bytes, not by you.** `candidates()`
  walks `ClusterView::nodes`, a `BTreeMap<NodeId, _>`, so the canvass asks peers in *node-id*
  order — and the frozen one has to be asked **after** the bidder, or the bidder is asked only
  once the stall is over and there is no window at all. Nothing configures this. `offload join`
  into a fresh state dir until the id sorts where it needs to (two tries, here); it is one line
  of shell and it is the difference between a deterministic walk and a race.
- **A frozen peer stalls a round for about three seconds, not `bid_window_ms`.** The timeout on
  `Cluster::ask` really is the window, but a `SIGSTOP`ped peer's QUIC connection dies before it
  expires and the exchange comes back `reading a frame: connection lost`. Measured at ~3.0s
  against `bid_window_ms = 10000`. Plenty for the freeze that matters — but a walk that plans
  around ten seconds will fire its second `kill -STOP` after the grant has already landed, which
  is a run *running* on the node you meant to make deaf. The grant's own wait is the full window,
  because by then the peer is frozen with its connection still up.
- **`SIGSTOP` on a daemon does not survive the shell that started it.** When the process group
  the daemons are in is orphaned — which is what the walk's shell exiting does — POSIX has the
  kernel send `SIGHUP` then **`SIGCONT`** to every stopped member. So the freeze silently lifts
  between one command and the next, the grant lands, and the run is `running` on a node the walk
  believes is frozen. `setsid --fork` puts each daemon in its own session and the freeze holds.
- **…and `setsid` forks, so `$!` is not the daemon.** It is the wrapper's, and signalling it does
  nothing while `ps` cheerfully shows a live `offloadd`. Diff `pgrep -x offloadd` across the start
  and write *that* down — the same rule as the `pkill -f` entries above, which is that a walk has
  to signal a pid it has actually confirmed.
- **A `cargo build` immediately before a pass makes the host refuse for CPU.** The eight-seconds
  note above is about a daemon that has just started; this is the same probe telling the truth
  about a machine that has just compiled — `cpu at 79%: 100% of the budget is free, the machine is
  not`, no bid, no grant, and a queued run at epoch 0 that looks exactly like the bug under test
  reappearing. Rebuild, then wait for `uptime` to settle, then walk.

- ~~**A work-policy refusal only binds a submission that went through a bid round.**~~ *Was true
  when written, and is the bug ADR-0046 fixed.* One daemon, no `[cluster]`, `accept = "never"`,
  submit at its own socket is now the **cheapest** staging there is: `offload run` is refused with
  the same sentence `offload status` prints. Keep the rest of that note, which is still true and
  still catches people: a daemon that is not a **fleet member** has no cluster at all, so
  `enabled = true` alone takes the no-cluster arm anyway, and a walk that needs a real *bid* wants
  `offload init`, a restart, `listen = "127.0.0.1:<port>"`, `mdns = false` and no seeds.
- **The A/B for a whole-mechanism fix is `git checkout <commit>^ -- <file>`, and it is cheap when
  the fix is one file.** ADR-0046 and ADR-0047 both changed one file each, so `git checkout` of
  the parent's copy, `cargo build`, a **fresh state dir**, walk, `git checkout` back is about
  ninety seconds — less than the `git stash` recipe above and with the same honesty. Under
  `-D warnings` a helper the reverted file no longer calls is what fails the build, which is a
  useful check that the revert was the whole of the change.
- **A one-daemon `offload resume` walk needs a run that is `pending`, and that needs a fake agent
  with turns to spare.** Submit, wait ~10s, `offload checkpoint <run>`, wait for the boundary: the
  run goes `pending` with a checkpoint and `here only`, and it stays there — nothing auto-resumes
  it. Then restart the daemon with whatever policy is under test, or `offload drain`, and type
  `offload resume`. A twelve-turn script with a four-second sleep per turn gives a comfortable
  window either side; one turn does not, because the run completes before the checkpoint lands.
- **A fake `nmcli` on the daemon's `PATH` turns the link metered under a running process.** The
  probe is `Command::new("nmcli")`, a `PATH` lookup, so a two-line script that `cat`s a file is the
  whole staging — `nohup env PATH=/tmp/w/bin:$PATH offloadd --config …`, then `echo yes > metered`
  and wait one re-probe cadence. Nothing about the machine's real network is touched, and unlike
  the nominated `metered = "yes"` (which is read at startup) this changes the answer **under** the
  daemon, which is the only way to walk anything about capabilities going stale. The cadence is
  `probe_interval_ms × REPROBE_EVERY` — thirty seconds on the defaults, so allow forty-five.
- **A recovery-tick walk needs the failure to land *after* the policy changes, which is a timing
  problem.** The tick decides from what the node is when the run fails, so a fake agent that fails
  too early measures nothing. Twelve turns at four seconds each puts the failure at ~50s; flip the
  fake `nmcli` at ~5s and the re-probe lands at ~35s, comfortably between. `offload status` in the
  middle of that is the check that the staging worked before waiting for the interesting part.
- **…and `offload probe` is the control, because it asks the machine rather than the daemon.**
  Two commands, two sources: `probe` shells out then and there, `status` reports the daemon's last
  probe. A walk that shows them disagreeing is showing something real; a walk that assumes they
  are the same source will conclude the fix did nothing.
- **Revoking the only node in a fleet of one is the cheap staging for a revoked device.**
  `offload init`, restart, submit, then `printf '%s\n' "$PASSPHRASE" | offload revoke <own node
  id>` — no second daemon, no `join`, no shortened `PROBATION`. Within a second the daemon halts
  the agent, fails the run with *this node was revoked from its fleet, so it stopped running it*,
  and `offload status` says so. Sessions forty-two and forty-three needed two daemons because they
  were walking what the *fleet* does; a walk about what the revoked node itself will still do needs
  only the one.
- **`offload init` prints the passphrase once, so redirect the whole thing to a file.** `| tail`
  eats it, and the second `offload init` refuses with *this node already belongs to fleet …* — a
  fresh `OFFLOAD_STATE_DIR` and `> init.out` is the fix, and it costs a re-found fleet to learn.
- **A deep `state_dir` fails to bind and the error names neither the path nor the limit clearly.**
  `path must be shorter than SUN_LEN` from *every* CLI call while the daemon looks fine in `ps`.
  The scratchpad directory a session is handed is comfortably too deep, so a walk on this machine
  wants `/tmp/<something short>` for the state dirs. Already noted at the top of this file for
  `--config`; this is the same limit met from the other side.
- **`metered` is nominated, so a walk needs no network changes at all.** `metered = "yes"` at the
  config's **top level** (not under `[policy]`, which holds `allow_metered` — the owner's
  permission, the other axis) is the whole staging: the three passes are `"yes"`, `"no"` and the
  key removed, and they are the three answers the capability can hold. Nothing touches
  NetworkManager, so there is no user connection to put back afterwards.
- **…and `sed -i "s|^metered = .*|…|"` inserts a duplicate when the key is absent.** The `||`
  fallback that adds the line then runs on the pass where it is already there, and
  `deny_unknown_fields` is not what catches it — `duplicate key metered in document root` is,
  which the daemon logs and the walk does not see, because the CLI's next word is "no daemon at
  …sock — is offloadd running?". Write the file out in full per pass, which is the same rule as
  the config-prefix trap above.

- **A two-daemon walk needs an idle machine, and an unrelated process can take that away.** The
  bid asks `admits`, which refuses for CPU pressure — `no node will take this run — alpha: cpu at
  100%: 100% of the budget is free, the machine is not` — so *nothing* can be placed while
  something else is pinning the load average. The existing note about `cargo build` is the same
  trap with a cause you control; this is the one you do not. It reads `/proc/loadavg`'s
  **one-minute** figure over the core count, so `until awk '{exit !($1<3.0)}' /proc/loadavg; do
  sleep 10; done` is the wait, and `ps -eo pcpu,comm --sort=-pcpu | head` says whether it is worth
  waiting for. Session forty-nine wrote a branch off as unwalkable on this and then walked it
  twenty minutes later, which is the right order: say what you could not do, and do it when you
  can.
- **The staging for a run that is handed over for a policy reason**, which is the shape any
  "this node will not host it but somebody will" walk needs. Two daemons in one fleet, both
  granted `host-runs` (`PROBATION` shortened); start **alpha alone**, submit — a one-node round
  puts it there without any bid-shaping — then start `bravo` and let the checkpoints replicate,
  which `ps` reports as `replicated` rather than `here only` and is what `Checkpoint::is_durable`
  is asking about. Flip the fake `nmcli` on alpha, wait a re-probe, and the failure lands on a
  node that may no longer host. Watch both logs: `handing the failed run back to the fleet` on
  alpha and `spawning claude code` on bravo, 412ms apart.
- **A peer that holds a replica and can never take the run is one `join` with no `grant`.** The
  cheapest way to stage "somebody else has a copy, and still nobody can host it": enrolment alone
  gives `{submit, deliver}`, which is enough to accept replicas and never enough to win a bid. No
  shortened `PROBATION`, no second grant, and `offload explain` names the reason per node —
  `fedora: not granted host-runs by the fleet`.
- **To measure what an idle fleet actually spends, count at DEBUG and then check the epoch.**
  `RUST_LOG=offload_cluster=debug,offload_node=debug` and a line count over sixty seconds says how
  often a round runs (`still nobody for it`); `offload explain` says whether those rounds are
  spending epochs. The second question is the one that matters and the first will not answer it —
  a retry that is cheap in messages and expensive in tokens looks identical in a log count.
- **…and give each daemon its own `[agent] config_dir`.** Two fake agents sharing one write their
  transcripts into the same directory under the same session id, so the second node's capture
  reads the first node's conversation. It looks like a successful migration and is two runs in one
  file.

Worth arguing with before building further: ADR-0012's 30-day certificate expiry is a guess,
the argon2id parameters are a guess calibrated on one desktop (280 ms there, so plausibly
~1 s on a phone — but nobody has run it on a phone), and ADR-0013's device-local capacity
broker is the one piece of shared mutable state in a design that has otherwise avoided it.
- **`drain_deadline_secs` is under `[cluster]`, not at the top level.** An unknown key is a *hard*
  config error, so a demo config that puts it where it reads naturally fails to start and the
  daemon's log is a serde message about expected fields rather than anything to do with draining.
  The error names the alternatives, which is the fastest way to place any of these.
- **The staging for `recover`'s sentence is `kill -9` and a changed config, and it takes twenty
  seconds.** Submit against a twelve-turn fake agent, wait ~14s so a checkpoint lands, `kill -9`
  the daemon *and* the agent, edit the config, restart. `recover` then fails the run with
  `daemon restarted while this run was active — resumable from turn N with 'offload resume <id>'`
  and the new config decides what the node will do about it. `[policy] accept = "never"` is the
  cheapest of the three refusals to arrange; a `drain` is one command on the running daemon; and
  revoking a fleet of one is the third (already noted above). All three end at the same door.
- **A `failed` run is invisible to `offload ps` without `--all`, and a `pending` one is not.** So a
  walk about a footnote on a failed run has to type `--all` or conclude the footnote is missing,
  and the two resumable states have to be staged separately if the display gate is what is under
  test: `kill -9` gives the `failed` one, `offload checkpoint` gives the `pending` one.
- **A revoked node is also a draining one**, since `stand_down` sets both latches — so any walk
  that reads a refusal on a revoked device has to check *which cause* it names, not merely that it
  refuses. Both answers refuse, and the wrong one sends somebody to restart a daemon that the
  fleet will throw out again. The cheap control is `offload run` beside `offload resume`: the
  submit door reaches the fleet state by a different route, so the two disagreeing is the symptom.
- **A walk under the scratchpad cannot bind a control socket.** The session scratchpad path is
  already ~120 characters, so `<state dir>/offloadd.sock` is past `SUN_LEN` before the walk adds
  anything and the daemon dies on `bind`. A short state dir (`/tmp/ow/{alpha,bravo}`) is the whole
  fix, and it is the same constraint the deep-state-dir note above names from the other side.
- **`offload init`, `join`, `grant` and `verify` do not take `--socket`.** They read the *state
  directory*, so `--socket` gets a note telling you so and the command then answers about
  `$OFFLOAD_STATE_DIR` — which on a development machine is the real fleet, not the walk's. It
  refuses to found a second fleet over it, which is the only reason this is a note rather than an
  entry in `sessions.md`. `--state-dir` on every fleet command, and put `--state-dir` *before* the
  subcommand's own arguments (`offload init --state-dir X --name Y`, since `offload --state-dir`
  is not a global).
- **…and `--passphrase` is a flag, not a value.** `offload join --passphrase "<six words>"` is an
  unexpected-argument error; the passphrase is prompted for, so `printf '%s\n' "$PASS" | offload
  join --state-dir … --passphrase` is the shape. `offload grant` prompts for it too and then
  prompts again for confirmation: `printf '%s\ny\n' "$PASS" | offload grant --state-dir … host-runs`.
- **A loopback-bound daemon is not discoverable, so the second node needs `seeds`.** "Two daemons
  with `[cluster]` and no seeds find each other" is true of a *LAN* bind; on `127.0.0.1` the mesh
  says `bound to loopback, so not announcing on the LAN` and the two never meet, which reads as
  gossip being broken. `seeds = ["127.0.0.1:<other port>"]` in the second config.
- **Shorten `PROBATION` for any two-node walk that needs the peer to host.** `offload grant
  host-runs` answers `dormant for another 15 minutes`, and a fifteen-minute wait in the middle of
  a staging is how a walk gets abandoned. `offload_core::fleet::PROBATION` to five seconds, and
  put it back — the same rule as the sweep timer below.
- **The checkout sweep is every fifteen minutes, so walking it means shortening it.** `main.rs`'s
  `let every` in the reclaim/prune loop; fifteen seconds makes the removal land while you are
  still looking at it. `reclaimed the checkout of a run this node is not running` in the log and
  the `offload audit <run>` row are the before and after of the same pass, visible together.
- **The staging for a checkout that gets swept: alpha alone, bravo late, then drain.** Submit to
  alpha while it is the only node, start bravo, wait for `ps` to say `replicated`, `offload drain`
  on alpha. The run migrates, alpha's worktree is left behind clean, and the sweep takes it. The
  fake agent must write **nothing into the worktree** — anything uncommitted and `holds_uncommitted`
  keeps the checkout, correctly, and the walk proves the guard instead of the sweep.
- **…and `pgrep -f <script>` in a wait loop matches the shell running the wait loop.** The `pkill
  -f` trap above, from the other side and one level meaner: `while pgrep -f batch.sh; do sleep 5;
  done` never exits, because the bash running that line has `batch.sh` in its own command line. It
  does not look like a hang — the loop is quiet, the tool call sits there, and **every command
  after it in the same compound never runs**, so the batch you thought you had reconfigured and
  started was neither. Wait on a marker file the script touches, or on a pid recorded when it was
  started. Cost one sixteen-pass batch.
- **Scraping the passphrase out of `offload init` with a loose pattern founds a second fleet.**
  `grep -Eo '([a-z]+ ){5}[a-z]+'` matches *"It is shown once and stored"* three lines above the six
  words you want, and `offload join --passphrase` does not fail on it: the passphrase **is** the
  fleet (ADR-0012), so a wrong phrase enrols the device into a different one perfectly happily. The
  mistake surfaces two steps later, on the daemon, as `could not reach seed … refused by <alpha>:
  that certificate is for fleet <X>, this is fleet <Y>` — which reads as gossip or seeds being
  broken and is neither. The indented line of *exactly* six lowercase words is the one:
  `awk 'NF==6 && /^ +[a-z]+( [a-z]+){5}$/ {print; exit}'`. Cost three passes of the ADR-0050 walk.
- **…and two `granted to` rows in `offload audit` is the *healthy* number, not the symptom.** A run
  submitted to alpha and then handed to bravo has a local grant at epoch 1 and a handover at epoch
  3, so `grep -c "granted to"` says 2 on a perfectly clean pass. The defect is two rows at **one
  epoch**: `sed 's/.*granted to/granted to/' | sort | uniq -d`. A count is the check that looks
  right and reports the fix working before it is written.
- **…and that same staging is how the double-grant in `fencing-and-epochs` shows up**, about one
  pass in five. It does *not* reproduce when bravo is up from the start: sixteen passes with both
  daemons running from the beginning, including four at `probe_interval_ms = 200`, were all clean.
  Whatever widens the window is in the late join. `offload audit <id> | grep -c "granted to"` is
  the check, and the run going `failed` with a `git worktree add … already used by worktree` error
  is the symptom to grep the logs for.
- **A repeated two-daemon walk resets by deleting `state.db*`, not the state directory.** The fleet
  lives in `fleet.json` and `node-key` beside the store, so `rm -rf $W/alpha` throws away the
  `init`, the `join` and the `grant` and pays the whole setup — `PROBATION` included — every pass.
  `rm -f $W/*/state.db*` with `rm -rf $W/*/worktrees $W/*/blobs` leaves the enrolment and clears the
  runs; keep `repos/`, which is the mirror cache and is most of what makes a pass forty seconds
  rather than a minute. Without the reset the *previous* pass's runs are still `assigned` in the
  store and the daemon starts them on the next one — extra agents, extra migrations, and a log that
  reads exactly like the defect under test.
- **A walk's state directory cannot live in the per-session scratchpad.** That path is ~95
  characters before `/alpha/offloadd.sock` is appended, and the daemon logs `member of fleet …`
  and *then* dies with `path must be shorter than SUN_LEN` — so the failure looks like enrolment.
  `/tmp/<something short>` for anything that binds a socket.
- **The deterministic way onto the accept-without-starting path is a cap and a `--queue`, not
  machine load.** Both occurrences of the two-workspace defect (`fencing-and-epochs`) followed
  `there is room now; starting the run held for it`, and session fifty-two reached that line only
  when the machine happened to be busy — which is why it was two hits in eighty passes.
  `max_concurrent_runs = 1` under `[policy]` on the submitting node, one run, then a second with
  `--queue` a second later, puts **every** pass through `start_held_runs`, and twice, since both
  runs end up going through it. Same staging otherwise — alpha alone, bravo late, drain — and both
  runs migrate, so the migration check gets doubled too.
- **The staging for "a node that has never been replicated to" is one line different from every
  migration walk here, and no walk had ever done it.** Start bravo **after** the checkpoint, not
  before. Alpha alone, submit, wait ~12s, `offload checkpoint <run>`, wait for the boundary — the
  run is `pending` and `here only` — and only *then* start bravo. Bravo learns the run by gossip
  within a second and holds **zero** blob files (`find $W/bravo/blobs -type f | wc -l`), which is
  the whole staging: `offload resume` there has to fetch the conversation from alpha. Every
  earlier migration walk started both daemons early enough that the checkpoints replicated, so the
  blobs were always already there and `ensure_blobs` was always a no-op. Cost: the defect sat
  behind it for as long as `resume` has existed.
- **…and the refusal arm is `kill -9` on alpha after bravo has the row.** Bravo keeps the run
  record — it is persisted, not view memory — so `offload resume` on bravo meets a checkpoint it
  cannot fetch and nobody to fetch it from: `no peer could supply the blob: no route to 0705e430`.
  Worth walking beside the happy path, because the interesting property is what the run looks like
  *afterwards* (`pending`, untouched, no epoch spent) rather than the sentence.
- **A shortened `PROBATION` is the only way for the *second* node to host, and it is a code
  constant.** `offload grant host-runs` and `offload invite --grant host-runs` both probate
  (ADR-0012), and the founder is the only node that hosts immediately — so any walk where the
  **joiner** must host means editing `offload-core::fleet::PROBATION`, building, walking, and
  putting it back. Build **both** arms of an A/B with the shortened constant, or the only
  difference between the binaries is not the fix. `git stash push -- <the changed files>` leaves
  the `PROBATION` edit in place while it reverts the fix, which is the whole recipe.
- **`RUST_LOG=offload_node=debug,offload_cluster=debug` silences `offloadd`'s own INFO lines**, so
  `grep -c ready <log>` — the check that the CLI is talking to the daemon you just started — comes
  back **0** on a daemon that is perfectly healthy. Add `offloadd=info` to the directive, or check
  the socket answers instead. *(Since session ninety-two the daemon's own lines, `ready` included,
  are target `offload_node::daemon`, so `offload_node=debug` now shows them and this trap is gone;
  `offloadd=` covers only the binary's hook and MCP modes.)*
- **The three-leg ping-pong is the staging for anything about what a checkpoint *carries*, and it
  takes ninety seconds.** Both daemons up, submit to alpha, `offload checkpoint`, `offload resume`
  on **bravo**, `offload checkpoint` there, `offload resume` back on **alpha**. The third leg is
  the one that matters: alpha's mirror still holds the run branch at leg one's tip, so it is the
  only staging where the branch is *behind* the checkpoint. It found ADR-0053.
- **…and the fake agent has to name its files per node, or the legs are indistinguishable.**
  Every spawn restarts the agent's own turn counter at 1, so `committed-$i.txt` collides across
  legs and a checkout holding six files looks identical whether the bundle travelled or not.
  `TAG=$(basename "$CLAUDE_CONFIG_DIR")` and `committed-$TAG-$i.txt` — with a `config_dir` per
  daemon, which is already required — makes `ls` the whole assertion. Cost one pass spent
  believing the commits had arrived when the count was a coincidence.
- **Read the account a resume gives even when it succeeds.** `offload logs <run>` prints one
  `workspace` line saying how the checkout came back — `rebuilt from bundle and patch`,
  `re-checked out, patch reapplied`, `adopted in place`, `the commits here already held this
  checkpoint`. ADR-0053 was a single word in that line being wrong on a resume that reported
  success and left half the work behind. It is the cheapest assertion in the whole system and no
  walk had been reading it.
- **To reach `restore` on the node that already ran the run, the *branch* has to outlive the
  *worktree*.** A same-node `offload resume` normally reports `adopted in place` and never calls
  `restore` at all: `adopt` reads the worktree's turn marker, and after a `kill -9` that marker
  sits at the last *capture*, so it equals `checkpoint.turns` and counts as current. So any walk
  about what `restore` decides needs the checkout gone and the branch still there. That is
  `[checkpoint] every_turns = 5`, a kill timed **between** captures, and then removing the
  checkout. It is the staging that closed ADR-0053's `AlreadyHere` residual.
- **…and `rm -rf` on a worktree is not how the product removes one.** Git keeps the registration,
  so the next `prepare` dies with `'offload/run-…' is already used by worktree at '…'` — the
  leftover already in `docs/pitfalls/checkpoints-blobs-and-workspaces.md`, met from the walk's
  side. `supersede` and `remove` both run `git worktree prune`; a walk deleting a checkout by hand
  has to run it too, or it is testing its own staging.
- **Time a kill off the worktree, not off `offload ps`.** `ps` reports stored progress and trails
  the agent by seconds: a pass that read `turn 7` and killed immediately caught the run at
  **turn 10**, on a capture boundary, where the branch equals the checkpoint and the walk proves
  nothing. `until [ -f "$WT/work-L1-8.txt" ]` on the file the fake agent has just committed is
  exact and costs one loop.
- **A build with one guard forced false is the sharpest control there is when the fix is a
  condition.** `if already && false` is a one-line edit, a `cargo build`, and a binary that is the
  product in every other respect — cheaper than `git stash` and it isolates the *condition* rather
  than the commit. Same discipline as the revert-check that tests get.
- **The staging for a run stranded on a departing node is "peer arrives too late".** Alpha alone,
  a fake agent that `exit 3`s after four turns so the run *fails* with its only checkpoint here,
  **then** start bravo, wait for both to call each other `alive`, then `offload drain` on alpha.
  The checkpoint stays `here only` however long bravo is up — `replicate` has one caller and no
  retry — so this is the one staging where a peer exists and a copy does not. It is what ADR-0054
  was built and measured against, and the before-arm's whole output is `nothing to hand over`.
- **An `ask` walk needs a `[[sinks]]` entry, and the sink can be two lines of shell.** With no sink
  anywhere in the fleet no question is *put* at all, so `offload asks` is empty and the drain's
  `Blocked` line never appears — which reads as the reporting being broken. `id`, `service`,
  `command`, `args`, `description`; a script that appends its arguments to a file is enough,
  because what the walk needs is a route to exist, not a person at the end of it.
- **…and the fake agent poses the question by piping one line into `offloadd ask-hook`**, with the
  daemon's own binary on its `PATH`. `{"tool_name":"Bash","tool_use_id":"toolu_walk_1",
  "tool_input":{"command":"…"}}` then `sleep 600` blocks the run mid-tool-call for as long as the
  walk needs, and `--ask --permission ask` on the submission is what makes the daemon gate it.
  Take one clean turn first, or there is no transcript and nothing to checkpoint from.
- **Set `drain_deadline_secs` *shorter* than the question's patience to walk the interesting
  case.** They are both 300 seconds by default, so which expires first depends on when the
  question was asked — 15 seconds against the default patience makes the drain reliably the nearer
  clock, which is the staging ADR-0035's residual is about.
- **The cheapest staging that reaches `supersede` on one daemon is a deleted turn marker.**
  `Adoption::Superseded` needs the worktree's marker to be *behind* the checkpoint, and on one
  machine a capture writes both together — a `kill -9` leaves them equal, which is why a same-node
  resume answers `adopted in place`. But `adopt` also answers `Superseded { at_turn: None }` when
  there is **no marker at all**, which is the real *run in flight across an upgrade* case, so
  `rm <state>/worktrees/<run>.turn` on a `pending` run is the whole staging. Two daemons and a
  ping-pong are not needed for anything about what `supersede` decides. Session sixty-four, for
  ADR-0055.
- **…and whether that checkout is *clean* is the fake agent's job.** Two scripts, one that
  `git commit`s each turn and one that leaves the file uncommitted, are the two arms of ADR-0055 —
  and the committing one also needs `git config user.{name,email}` set inside the worktree, since
  the worktree is fresh and the demo's `$HOME` may have none.
- **`cd /tmp/w && nohup offloadd … & echo $! > pidfile` records the wrong pid.** The `&` backgrounds
  the whole `cd && nohup` list, so `$!` is the subshell, and the `kill -TERM $(cat pidfile)` that
  follows kills nothing — then `until ! pgrep -x offloadd` waits for ever on a daemon nobody
  signalled. The documented pid trap in a new spelling: it is the *compound* that gets
  backgrounded, so start the daemon on its own line, or `pgrep -x offloadd` once to get the real
  pid before signalling. Cost one ten-minute timeout mid-walk.

#> **Checkouts go to `~/offload` by default (ADR-0074)**, and only one node may use it. Every walk
> daemon beside the first needs `[workspace] dir = "<its state dir>/worktrees"` in its config, or it
> refuses to start, naming the node that owns `~/offload`. On this laptop `/tmp/mw` owns it.

## Staging phase 8's demo on one machine (session sixty-eight)

The whole sentence — a task on a device with no agent, a schedule, a trigger, a notice-fired
escalation and a sink — on two daemons on this laptop. It needs no Mac: what makes the escalated
agent run land somewhere else is the *tier*, so one daemon with `agent.binary` pointing at
nothing and the `[[tasks]]` entries, and one with a fake agent and no tasks, is the placement
under test. Two machines would prove the network, which sessions sixty-five and sixty-six already
did.

- **`offload init`, `join`, `grant` and `fleet` read the *state directory*, not the socket** — and
  with neither `--state-dir` nor `OFFLOAD_STATE_DIR` they act on **your real fleet**. `offload
  --socket /tmp/p8/alpha/offloadd.sock init` prints a note saying so and then refuses with *"this
  node already belongs to fleet bb67203d"*, which is this laptop's actual fleet answering. The
  note is doing its job; read it. Set `OFFLOAD_STATE_DIR` once per shell and every membership
  command lands on the right node.
- **`--passphrase` is a flag, and `grant` has no such flag at all.** Both prompt, so a scripted
  walk pipes it: `printf '%s\n' "$PP" | offload join --name bravo --passphrase` and
  `printf '%s\n' "$PP" | offload grant host-runs`. `offload grant host-runs --passphrase …`
  exits 2 with clap's *"to pass '--passphrase' as a value, use `-- --passphrase`"*, which reads
  like a quoting problem and is not one.
- **The *founder* needs the restart too, not only the joiner.** The existing entry about a daemon
  that came up before its node was a member says to restart the joiner; the same is true of
  `offload init`, for the same reason. Measured one line apart: `offload nodes` on the founder
  said *"This node is not in a mesh"* while the joiner already listed itself. Restart both after
  the membership commands, then wait ~5s and check `offload nodes` **from both ends** before
  believing the mesh.
- **Plan the walk around the fifteen minutes, because one clause needs the joiner to host.** The
  escalated agent run has to land on the machine with the agent, and if that machine is the
  joiner its `host-runs` is dormant for `PROBATION`. So: `init`, `join`, `grant`, restart both,
  then walk the three clauses that land on the **founder** (a submitted task, a schedule, a
  trigger) while the clock runs, and the escalation last. Shortening `PROBATION` and building
  both arms is the other way and is not needed here — the wait is the staging.
- **The failing task that escalates must be one a *person* submitted.** A notice about
  machine-started work fires nothing (ADR-0057 §3), so the obvious staging — let the schedule fire
  a task that exits 2 — proves the loop guard and never the escalation. Two `[[tasks]]` entries:
  one that exits 0 for the schedule and the trigger, one that exits 2 for `offload run --task`.
- **A sink that echoes `"$*"` makes the plane look broken.** A task's `Notice::summary()` is the
  bare word `finished`, deliberately (ADR-0019 §2 — it is what keeps *"finished after 0 turn(s),
  $0.0000"* off a phone), so the run's identity is on the environment and not in the argv:
  `OFFLOAD_TITLE` (*"run 01a08b8228d0 finished"*), `OFFLOAD_RUN`, `OFFLOAD_ABOUT`, and the whole
  notification as JSON on stdin. A one-line sink worth having:
  `echo "$(date -u +%H:%M:%S) [$OFFLOAD_ABOUT] $OFFLOAD_TITLE — $*" >> phone.log`.
- **`offload every` takes `--note`, not `--description`.** The one-line label is spelled
  differently from `[[tasks]]`'s `description`, and an unknown flag exits 2 before the schedule is
  created, which in a scripted walk reads as the command failing.
- **Deleting `state.db*` between passes takes the rules and schedules with it**, which is the
  point when the last pass left a trigger firing every twenty-five seconds into the log you are
  about to read. Session fifty-three's reset recipe (keep `fleet.json`, `node-key`, `repos/`)
  needs nothing added for the standing-instruction tiers — it already clears them, and re-creating
  one rule is two seconds.
- **What the walk is *for* is the reports, so read every one on both nodes.** Both defects this
  staging found were reports rather than machinery: `offload logs` printed nothing on the node a
  task was submitted from, and `offload explain` promised a retry that could not happen. Neither
  is visible from the node that ran the work, which is the whole reason to submit from the other
  one.

### From the phase-5 walk on real devices (session sixty-four)

- **The default allowlist denies `git commit`, so a real-agent checkpoint carries only a patch.**
  Measured on the first run of the walk: 23 turns, `completed !6` in `offload ps`, and
  `offload logs` ending *"6 permission request(s) denied"* — every one of them git. The agent wrote
  `tests.py`, could not commit it, and said so in its own last message. `AcceptEdits` is right
  (ADR-0008: the worktree is disposable, so editing inside it is a small grant) and it means the
  **bundle** path is never exercised: with nothing committed there is nothing to bundle, and a
  migration then tests only the patch. That is the same blind spot the checkpoints pitfall file
  records for `bundle: None` in a fixture, arriving from the other direction. Any real-agent
  migration walk wants `allow = ["Bash(git add:*)", "Bash(git commit:*)"]` under `[agent]`, which
  is operator-set and uncapped. **Worth checking against the earlier real-agent walks**, which did
  not set it — session twenty-three's drain may have proved migration over a patch and not a
  bundle.
- **`setsid nohup offloadd … & echo $! > pidfile` records `setsid`'s pid, not the daemon's.**
  `setsid` forks when it is not already a process-group leader, so `$!` belongs to a process that
  has already exited — `ps -p $(cat pidfile)` says the daemon is not running while the log shows it
  cheerfully ready. The third spelling of the pid trap already in this file twice. `pgrep -x
  offloadd > pidfile` immediately after the start is the fix; the `-x` still matters.
- **Fedora Workstation needs no firewall change for any of this.** The default `FedoraWorkstation`
  zone already allows `1025-65535/udp` and `1025-65535/tcp` plus the `mdns` service, so QUIC on
  7433 and mDNS discovery both work as shipped. Worth knowing before reaching for `sudo
  firewall-cmd`, which is what the absence of a note here invites.
- **`git daemon` is not part of Fedora's `git` package**, so the recipe above it in this file fails
  with `git: 'daemon' is not a git command` on a machine that has git. The no-install substitute
  is dumb HTTP: `git clone --bare`, `git update-server-info` in the clone, then any static file
  server over the directory. Read-only is enough — `ensure_mirror` clones and captures travel as
  bundles and patches, so nothing ever pushes to the origin. It is also the shape that needs no
  package and no root.
- **A URL is not optional for a multi-machine walk, and the reason bites early.**
  `RepoSource::portability()` makes a local path an eligibility fact, so two machines with the repo
  at different absolute paths can never both be candidates. Serve it and use the URL from the
  first submission, rather than discovering this when the migration will not place.

### Linux ↔ macOS: the transport does not connect (sessions sixty-four to sixty-six, **resolved**)

**Two faults, and separating them took three sessions.** One is ours and is fixed. The other is
the Mac's, is not a bug in Offload or in quinn, and needs a click at that machine's console.

**What was actually wrong, in one paragraph each.**

* **Ours: `QuicTransport::accept` did every step of an inbound handshake inline in its own loop.**
  One of those steps is `accept_stream` — an unbounded `accept_bi()` on a connection whose peer
  may never open a stream — and quinn does not drive a handshake at all until the application
  takes the `Incoming`. So one dialer that completed TLS and then went quiet parked the loop, and
  every *later* dialer got neither a refusal nor a close but **silence**: an Initial sent,
  retransmitted on PTO, nothing back, an idle timeout. It is self-sustaining, which is what made
  it look like a network fault — two nodes redialling a peer they cannot reach keep supplying the
  stalled connections that keep them from reaching it. That is why the pair "meshed and died
  eight times and then stopped meshing at all". Fixed: each handshake runs on its own task and
  posts its outcome to `accept` over a channel. Reproduced first on **one machine in five
  seconds** by `a_stalled_dialer_does_not_block_the_next_peer_from_getting_in` — no Mac, no lossy
  link, a stalled dialer being a thing anybody can write.
* **Theirs: macOS refuses to let an ad-hoc-signed binary send to the LAN, and reports it as
  `EHOSTUNREACH`.** Every `sendmsg`/`sendto` from `offloadd` to the laptop fails; `ping` from the
  same machine at the same moment is 1.2ms and the route, the ARP entry and the socket are all
  healthy. It is the **binary's code signature** that decides, and nothing inside the process.

**The measurement that settled it, and it is the only one worth keeping.** Three senders to the
same address, from the same source address, same size, **interleaved in one loop** so no arm can
drift into a different window:

| sender | result |
| --- | --- |
| `/usr/bin/python3` — Apple-signed | **30 of 30 sent** |
| a 20-line C sender — `codesign`: `adhoc`, linker-signed | **0 of 30**, every one `EHOSTUNREACH` |
| `udp_probe` — Rust, ad-hoc signed, the same class as `offloadd` | **0 of 30**, every one `EHOSTUNREACH` |

Repeated with a **freshly compiled** C binary — a code identity the system had never seen — which
was denied from its first datagram, 40 of 40, while the Apple-signed control in the same window
was 10 of 10. So it is not a per-binary grace period either; it is simply that one class of
binary may send to the LAN here and the other may not.

**Everything inside the process was eliminated first, and each of these is a thing not to
re-test.** All interleaved against a known-failing control in the same window:

| held constant, and not the cause | how |
| --- | --- |
| socket options | python sockets carrying `IP_RECVTOS` + `IP_DONTFRAG` + `IP_RECVDSTADDR` — quinn-udp's whole macOS set — 40 of 40, blocking *and* non-blocking |
| the syscall | `sendto`, `sendmsg`, and `sendmsg` with an `IP_TOS` cmsg: 40 of 40 each |
| ECN | 100 ECT0-marked and 100 unmarked on one socket: **0 ok in both**, identical |
| socket lifetime | one socket held for 5 minutes and a fresh socket per datagram fail the same |
| the message header | `sendto` on the **same fd**, microseconds after each `sendmsg` failure: 0 of 599 |
| ICMP | the laptop's `OutDestUnreachs` is flat across a failing window |
| the duplicate address on the LAN | **the control was finally run** — laptop's Wi-Fi off, single-homed on the subnet — and it fails identically |

**Three claims in earlier versions of this section were wrong, and two of them were load-bearing.**

* **`udp_probe`'s "10 of 10" never measured delivery.** `UdpSocketState::send` returns `Ok(())`
  for every error but `WouldBlock` — it logs and swallows the rest. The probe was counting calls
  that cannot fail. On `try_send` the same path measures **0 of 40** in the same conditions. So
  *"everything below `quinn-proto` is proven good"* was never established, and the contradiction
  this section was built around — "quinn's traffic fails four times in five where raw UDP never
  fails at all" — **did not exist**. There was no contradiction; there was an instrument that
  always said yes.
* **"13 `sendmsg` errors in 2.5 hours" was a log rate limit, not a count.**
  `log_sendmsg_error` emits at most one line a minute. The socket was failing *every* send.
* **The duplicate-address condition is not the cause.** It was live, the control had never been
  run, and now it has been: it changes nothing.

**And one theory of this session's own died the same way, which is worth keeping as the shape of
the mistake.** "A plain socket succeeds where quinn-udp's fails, so it is the message header" was
a real, repeated measurement — 354 disagreements out of 360, every one in the same direction. It
was still wrong: `sendto` on the *same fd* straight after each failure fails too. Two senders
disagreeing is evidence about the senders, not yet about the syscall between them.

**What this leaves, and it is not code.** `offloadd` on the Mac needs **Local Network** access:
System Settings → Privacy & Security → Local Network, with `offloadd` (or the terminal it is
launched from) enabled. That is a click at the Mac's own console — it cannot be granted over ssh,
`tccutil` only resets, and the denial produces no line in `log show` that ssh can read. A binary
with a **stable** signature is what makes such a grant stick; an ad-hoc-signed build gets a new
identity every rebuild, which is worth knowing before wondering why an approval stopped working.

**The half of this that *is* still ours, and it is the reason the diagnosis took three sessions.**
When macOS muzzles the socket, `quinn-udp` swallows the error and returns `Ok`, so quinn believes
it sent every packet and Offload reports `no answer within 500ms` — *the peer did not reply* —
for what is really *this machine would not let me speak*. Nothing above the syscall can currently
tell those apart. `try_send` is the call that would, and whether the transport should use it is a
design question rather than a fix, so it is a roadmap item rather than a change made here.

**Reaching the Mac at all needs the source interface pinned.** `ssh macmini` times out
intermittently; `ssh -b 192.0.2.240 macmini` works, and retrying is normal. `ping` answers
either way, so a timeout is not evidence the machine is down.

**Two process traps worth carrying,** both of which cost real time here. `RUST_LOG` scoped to
crate targets silenced the lines being looked for **six separate times** — the filter that this
file recommended was itself missing `offload_transport`, where every inbound-path line lives. Use:

```bash
RUST_LOG=info,offloadd=info,offload_node=debug,offload_cluster=debug,offload_transport=debug
# (since session ninety-two the daemon's own lines are target `offload_node::daemon`, which
#  `offload_node=debug` covers; `offloadd=` now matches only the binary's hook and MCP modes)
```

And a `pkill -f` whose pattern appears in the invoking shell's command line kills the walk,
exit 144 — met three times in this file now.

**`setsid` does not exist on macOS**, so the backgrounding recipe above needs plain
`nohup … & disown` there. And a daemon stopped with `kill -TERM` can take longer than
`drain_deadline_secs` to release its state directory, so a replacement started too eagerly dies
on the lock while `pgrep` still shows the *old* pid — which looks exactly like the restart having
silently failed.

**A muzzled machine, without root, a firewall or a second host.** A UDP socket with `SO_BROADCAST`
unset — quinn never sets it — is refused `EACCES` by the kernel for every datagram addressed to
the broadcast address, and nothing leaves the machine. So one daemon with

```toml
[cluster]
enabled = true
listen  = "127.0.0.1:7451"
mdns    = false
seeds   = ["255.255.255.255:7433"]
```

is a node whose every send is refused, on the ordinary seed-dial path, in about a minute of
staging (`offload init`, restart, `offload status`). That is the staging for ADR-0059's line and
for anything else about a machine that cannot speak. The pair to run beside it is the *control* —
two daemons meshed on loopback, one `kill -9`'d — because the whole question is whether the two
look different, and before ADR-0059 they did not. `unshare -rn` is the other rootless muzzle
(`ENETUNREACH` for everything, since the namespace has no routes) and is heavier: the daemon and
the CLI both have to live inside it.

**A daemon's own log is not where to look for this.** At `RUST_LOG=info` a node refusing every
send logs **nothing at all** about it — no warning, no dropped-connection line — and `offload
nodes` reports a healthy fleet of one. Only `offload status` says so.

**The three-daemon-states walk for a schedule, on two daemons and in five minutes.** `offload
every` has a one-minute floor and the pass runs every five seconds, so `--every 1m` gives a tick
you can watch. What the walk needs staged is only a `[[tasks]]` entry both nodes nominate — and
**`id` is not optional**: a `[[tasks]]` block without one is dropped with `ignoring a task with no
id` at WARN and the schedule then fires into `nobody took this occurrence: ineligible: has tick as
execute (nothing offers it)`, once per five seconds, until the tick's attempt budget
(`TRIES_PER_TICK`, a dozen) runs out and it moves to the next tick and does it again. That is the
retry bound working exactly as ADR-0056 describes it, and it reads like the schedule being broken.
The give-away is the `run=` field in those lines changing when the `tick=` field does.

The three states worth seeing, in this order, because each is one `pkill` from the last:

1. **Home present** — `offload schedules` on the home says `fired from here right now`, on the peer
   `fired by <home>`. One firing per tick, on the home only.
2. **Home gone but remembered** — kill the home and leave the peer *running*. The successor rule
   hands the tick to the lowest-id live node and the peer starts firing. This is the one state
   session sixty-nine did **not** re-measure; it is session sixty-seven's, recorded in ADR-0056,
   and it is the step to take first if the walk is about succession rather than about the report.
3. **Home forgotten** — kill the home and then restart the peer. A fresh daemon's view is empty, so
   `steward_of` answers `None` and **nothing fires**. This is the state that used to read as
   `fired elsewhere`; the `home` column falling back to a bare node id is the tell that the view
   has no such node.

Bringing the home back closes state 3 on its own, but not quickly: the seed re-dial backs off
(30s, 40s, 50s, 70s), so budget a couple of minutes before deciding a mesh has failed to heal.
`grep "could not reach seed"` on the peer is what says which attempt it is on.

And **`offload unschedule` works from any node, not just the home** — which is what makes state 3
survivable when the home is gone for good, and is worth knowing before staging a walk around
getting to the home node first.

**A rule and a trigger can share a name, and that is the staging for the namespace they live in.**
`Service::Other` accepts any non-empty string, so `service = "failed"` in a `[[triggers]]` block is
legal and collides exactly with the notice kind `offload when failed --on-notice` binds to
(`RULE_NOTICE_KINDS` is `finished, failed, overdue, asked, answered` — all five are plausible names
for somebody's watcher). One daemon with `cluster.enabled = false`, a `[[triggers]]` block echoing
a line every few seconds, a `[[tasks]]` entry to fire, and **both** forms of `offload when failed`
is the whole staging, and it takes about a minute:

- the notice-bound rule must **not** fire on the trigger's lines (`fire_one`'s `fired_by` guard),
- the trigger-bound rule must fire on the next line,
- and `offload triggers` must count only the second (`RULES` and the `watching — but no rule is
  bound to it` line, which is the one sentence that report exists to say).

Read `offload rules` beside it: two rows with the same `ON` value, told apart by the `└─` line each
carries, is what the two namespaces look like on screen.

**`--task` takes a *service*, not the `[[tasks]]` block's `id`.** Both are required and only one
is the name you schedule or watch, so `--task <id>` produces a perfectly correct "nothing in this
fleet nominates a task for that" — which reads as a false positive and cost ten minutes in session
seventy-three. `offload status` prints a `task` line per nomination with **both** names on it —
which it did not when that entry was written, and the sentence has been corrected rather than
deleted: it said `offload status` listed the services and that was true of resources and of
nothing else. `offload probe --config node.toml` is the other place, and the one that shows all
four nominated kinds at once.

**A one-daemon staging reaches everything about `offload every` except the steward.** `[[tasks]]`
with a script that `exit 0`s, a `[[sinks]]` entry so the reach clause has something to be true
about, and `every 1m` — the floor is a minute and it is enforced, so a walk waits 60s per tick and
no less. Two schedules side by side, one for a nominated service and one for a service nobody
nominates, gives the measurement and its control in one pass; the refused one logs
`nobody took this occurrence … ineligible: has <svc> as execute` at INFO, a dozen times per tick,
and shows up in no report at all.

**Staging a rekey with the daemons up takes four minutes and is not what the record had.** Two
daemons meshed on loopback, `offload rekey --state-dir <alpha>` typed while both are running, then
`offload status` on **both** — the second is the control and it is where the finding was. Within
three seconds each node calls the other `dead` and the seed dial starts printing the real reason;
twenty seconds is enough to have a count worth reading. The phrase comes out of `offload init`'s
line 6, so `offload init … | sed -n '6p'` captures it without a human in the loop, and `offload
rekey` reads it from a pipe (`--passphrase` is a *mode*, never a value).

**…and `offload verify` is a one-line staging that needs no daemon at all**: `echo "<phrase>" |
offload verify --state-dir <dir>`. Every membership command takes `--state-dir` and runs with no
daemon — except **`offload nodes --history`**, which reads the fleet log the others *write* and
needs a socket. So on a machine where `offloadd` is not running you can add to that log and not
read it back.

**The whole staging for anything about run *ordering* is `max_concurrent_runs = 1` and three
submissions a second apart.** Under `[policy]`. The first runs, the other two sit in `assigned —
waiting for a slot`, and that queue is the only order this system has (`held_but_not_started`,
sorted by `urgency_order`). A fake agent that prints two lines, sleeps twelve seconds and exits
with a `result` gives each run a predictable slot, so the start order is readable from
`offload ps` on a five-second loop. **Measure the control arm first** — with no edit at all, the
older of the two goes next — or the reordering you then produce proves nothing.

**…and `offload deadline` wants `1h`, not `in 1h`** (`unknown unit 'i' — use s, m, h or d`). The
error names the units, so this costs one retry rather than a search.

**A fake agent must answer `--version`, or the daemon never finishes starting.** The probe runs
`agent.binary --version` in `deliver::capabilities`, *before* the control socket is bound — so a
stand-in that ignores its arguments and goes straight to `sleep 600` produces a daemon that logs
`member of fleet`, binds nothing, and answers no command. It looks exactly like a daemon wedged on
the state-dir lock, and the lock is the thing you check first; the lock fails fast and is not it.
Three lines at the top of the script (`for a in "$@"; case $a in --version) echo "2.1.237"; exit 0`)
are the fix. Cost ten minutes in session seventy.

**…and the fake agent needs a fake login, or every bid is refused.** `claude_auth` is satisfied by
`.credentials.json` **existing** in the nominated `[agent] config_dir` — it is deliberately never
read — so `echo '{}' > <config_dir>/.credentials.json` is the whole of it. Without it the
submission dies at the door with `ineligible: claude-code authenticated (installed 2.1.237,
authenticated = false)`, which is the probe being right and reads as the fleet being broken. Wait
one probe cadence (30s, ADR-0048) or restart the daemon.

**A state dir under the scratchpad is too long for a unix socket, and `[node] socket` is the way
out.** `path must be shorter than SUN_LEN` is the documented `SUN_LEN` trap in its likeliest new
spelling: a session-scoped temporary directory is already ~90 characters before `offloadd.sock` is
appended. `socket = "/tmp/ow/a.sock"` at the config's top level moves only the socket and leaves
the state where it belongs.

**`offload triggers` had never been typed in any walk in this repository** before session
sixty-nine, which is worth keeping as a *method* rather than as a list, because a list goes stale
and this does not:

```bash
for c in $(grep -oE "^    [A-Z][a-zA-Z]+ \{|^    [A-Z][a-zA-Z]+,$" \
             crates/offload-cli/src/main.rs | tr -d ' {,' | tr 'A-Z' 'a-z' | sort -u); do
  printf '%s %s\n' \
    "$(grep -o "offload $c\b" docs/DEMO.md docs/HANDOFF.md docs/sessions.md | wc -l)" "$c"
done | sort -n | head
```

The commands at the top of that list are where an assumption can survive a whole ADR: nothing has
run them, so nothing has contradicted them. `offload triggers` came out at **zero** and was
carrying one. Re-run it rather than trusting a list written here — immediately after session
sixty-nine's three walks the top was `id`(1), then `deny`, `priority`, `unschedule` and `verify` at
two each. `unschedule` is only that high because that session's own schedule walk is what put it on
the board, which is the loop working: a command's count goes up when somebody actually types it.
Session seventy-four took `unschedule` and `unwatch`, the two at the top — **both sound**,
including every refusal of both resolvers, and the finding was one field over and structural: two
schedules sharing a tick boundary display **one run id for two runs**, for ever. After it the
list has flattened to a floor of five and the top is `id`(5), `priority`(5) and `revoke`(5) — of
which the first two have been walked and recorded sound, so **`offload revoke` is the genuine
top and has never been typed in a walk here.** A flat list is the loop working its way through;
what it means is that the next walk should pick the untyped command rather than the smallest
number.
Session seventy-three took `id` (**sound**: stable across calls, survives `init`, and `id`,
`fleet` and `status` all agree) and `every` (refusals all good; the finding was the precondition
note it never printed). Session seventy-one took `verify` off it and `rekey`, `id`, `invite` and `join` came along
with it — and `verify` came out **sound in six cases**, with the finding one command over in
`offload status`. Session seventy took `deny` off the top and `id`, `invite` and `join` came along
with it — a
two-daemon staging types half the membership set on the way to anything else, which is why the
list moves faster than the number of walks. `deny`'s own mechanism was **sound**, including the
cross-device half; three reports around it were not.

**The whole of `offload revoke` walks on two daemons on loopback in ten minutes, and the control
arm is the other daemon's screen.** `offload init --state-dir $W/alpha`, `printf '%s\n' "$PP" |
offload join --state-dir $W/bravo --passphrase`, both daemons up with `mdns = false` and each
seeded at the other, then `offload revoke <bravo>` from alpha. **No `PROBATION` shortening is
needed** — revocation does not go through the `host-runs` door — which makes this one of the
cheapest membership walks there is. What to read, and in this order, because the interesting half
is never on the node you typed the command on:

1. **alpha** — `offload nodes` (bravo goes `dead` within a second or two).
2. **bravo** — `offload status` *whole*, not grepped: the `accepting` line is honest and was the
   only line that was, and the fleet block under it is where three sessions' worth of defects sat.
3. **bravo** — `offload fleet`, which is the second opinion. Two reports of one device disagreeing
   is the thing to look for, and it is what found the certificate claim.
4. **both** — `offload status | grep revoked`, which is the control arm proper: the same fact
   counted on two nodes, and a disagreement means the node that *did* the thing is the wrong one.

`--state-dir` rather than `--socket` for every membership command: they run with no daemon on
purpose, and `--socket` prints a note saying so and then answers about `$HOME/.offload`.

**And the arms worth typing before anything is revoked**, all of which cost nothing: a non-hex
needle, an empty one, a wrong passphrase (it names both fleets), a node id this fleet has never
met, the same device twice, and the device itself. The last two are where the defects were.

**The id-collision staging is four minutes and one daemon, and it is the cheapest way to test any
listing that prints a run id.** One `[[tasks]]` block with an `id` and a `service`, `cluster
.enabled = true` on loopback, then `offload every 1m --task <service>` beside `offload every 2m
--task <service>`. They share every second boundary, and ADR-0056 derives an occurrence's `RunId`
from the tick — so two different runs come out with the same first twelve displayed characters, on
purpose and for ever. `offload ps --all` widening to fourteen is the tell that the collision has
happened; a listing that has *not* widened beside it is the defect. Wait for `ps --all` to show
four rows rather than watching the clock.

**It is a task-tier staging and it cannot reach a question**, which bounds what it tests: `offload
every` has no `--ask` (clap refuses it) and only schedules derive ids, so nothing here reaches
`offload asks`, `offload explain`'s `waiting` line or the drain's blocked step. Worth knowing
before staging something bigger to try.

### Staging a broken nomination, on one daemon in two minutes (session eighty-one)

The cheapest way to walk what a node claims it can do against what it can. Four `[[…]]` blocks
pointing at programs that are not there — a `[[tasks]]`, a `[[triggers]]`, a `[[resources]]` and a
`[[sinks]]` — plus the working ones beside them as the control, and then every report on the node.

- **Each of the four has its own report and they are not the same command.** `offload sinks` for a
  sink, `offload triggers` for a trigger, `offload status` for a resource *and now* for a task.
  Reading only one of them is what left the task tier with no report at all for two phases: the
  other three each said `not found on this device` one command away.
- **`offload probe --config <node.toml>` is the only command that shows all four at once**, and it
  needs the `--config`: without one it probes `claude` on `PATH` and a nominated block is invisible,
  because nominations live in the daemon's config and nowhere else.
- **The control arm is a second `[[tasks]]` entry with a working program**, because the interesting
  output is the pair: `task:quick — authenticated` on one line and `task:nightly — NOT
  authenticated` on the next is what makes a wrong clause under the second one visible.
- **A third arm costs one `chmod`**: a file that exists and is not executable. `which` answered
  `is_file()`, so that entry was advertised as usable, won its bid, and failed the spawn with
  `task: starting task 'noexec': Permission denied (os error 13)`. It is the same over-claim as a
  missing program with nothing on screen to distinguish it, and it is one line of staging. **Keep
  it in the pass after the fix**, because the obvious fix — an `Option` — makes every report say
  *its program was not found* about a file that is right there, which is how that arm earned its
  place: it is the control for the *wording*, not just for the gate. One `[[triggers]]` and one
  `[[sinks]]` block pointing at the same unreadable file gives the other two kinds for free.
- **Six door arms, and each is one command.** `offload run --task <broken>`, `--task <nobody
  nominates>` and `--task <working>`, on a daemon with `cluster.enabled = true` **and** on one with
  it false — the two take different code paths to the same question (the bid round, and
  `task_refusal`), and the second is the one every default node without a peer is on.
- **`offload every 1m --task <broken>` and `offload when <svc> --task <broken>` are the other two
  doors**, and they *warn* rather than refuse (ADR-0032). Type the service nobody nominates beside
  each as the control: before session eighty-one the broken one was accepted in silence and the
  unknown one printed a paragraph.
- **`pgrep -f 'offloadd --config /tmp/…'` has the same trap `pkill -f` has** and it is worth its own
  line, because the entry above says `pkill` and the shape that bit here was `for p in $(pgrep -f
  …); do kill $p; done`: the pattern matches the invoking shell's own command line, so the walk
  kills itself and exits 144 with nothing done. `pgrep -x offloadd` matches the process name and
  cannot match the shell. Met twice in one session.

### Two names for one device, and how to stage it (session eighty-two)

The staging that found the merge losing a signed name, and it is the **default** configuration
rather than a contrived one: name the devices at enrolment and say nothing in `node.toml`.

- **`offload init --name alpha` names the *certificate*; `[node] name` is a separate field that
  defaults to the machine's hostname.** So two daemons on one laptop are both `fedora` locally and
  `alpha`/`bravo` to the fleet, which is exactly the case where a report using the wrong one names
  nothing. The daemon says so at startup — `this node's certificate names it differently from its
  config` — and that WARN scrolling past is the only warning there is.
- **Watch the flip rather than reading one screen.** `offload nodes` right after the handshake
  shows the certified name; it is the *next* gossip merge that replaces it. A loop of
  `offload nodes` every five seconds shows `bravo` for about 25s and the hostname thereafter, and a
  single reading at either end looks like correct behaviour.
- **`pgrep -f` has bitten this walk in three different shapes now**, and the third is the one the
  existing entries do not cover: `kill -9 $(pgrep -f "offloadd --config …/bravo.toml")` run inside
  `$( )` matches the **subshell's own** command line, so it returns a pid that is not the daemon
  and kills something else entirely — here it left bravo running and the run went on to
  `completed`, which reads as the failure detector being broken. `pgrep -x offloadd` cannot pick
  one of two daemons, so the shape that works is a loop over the name and a look at each
  `/proc/<pid>/cmdline`:

  ```bash
  bravo_pid() { for p in $(pgrep -x offloadd); do
      tr '\0' ' ' < /proc/$p/cmdline | grep -q "bravo.toml" && echo $p; done; }
  ```
- **An orphaned run takes about 30 seconds and is worth the wait**, because it is the only way to
  type the one refusal `offload cancel` has that nothing else reaches. `kill -9` the holder, poll
  `offload ps --all` until the state reads `orphaned`, then cancel. Note that the *agent* outlives
  a `kill -9`'d daemon — `leftovers` is swept at startup — so a fake agent with enough turns left
  will keep writing into a worktree nobody is supervising.
- **`offload cancel ""` is worth typing on every command that takes a run id**, which is how the
  empty-needle hole was found: `cancel`, `explain`, `checkpoint` and `audit` resolved it to a real
  run and `logs`, `rm` and `resume` refused. Session seventy-six's revoke walk already listed "an
  empty one" among the arms worth typing; this is that arm on a different command.

### Staging the delivery plane, and the four routes worth having (session eighty-three)

`offload sinks` is read in most walks and had never been the subject of one. What it needs is four
`[[sinks]]` on **one** node and none on the other, so both halves of the report have rows:

- **A route that works** (`echo … >> phone.log`), **a second that works** (so dedup across two
  routes is visible), **one whose program exits non-zero**, and **one whose program is not there.**
  The third and fourth are different states and the report distinguishes them: `exited 7: no
  output` against `not found on this device`.
- **Put the sinks on the node that does *not* run the work.** ADR-0010's whole claim is that news
  travels to a route somewhere else, and the two tables — this node's routes, and the fleet's —
  only both have rows when one node has none. It also makes the two outboxes visible: each node
  owes its own news to a route, so the same route shows different counters on the two screens.
- **A fleet event is a cheaper trigger than a run.** `offload grant <g>` writes one and the
  delivery pass carries it to every route with no audience asked, so the counters move without
  placing anything. That matters when the machine is busy — see the next note.
- **Found by reading both tables on one screen**: they render the same three questions in
  different orders, and each looks right alone.
- **`offload sinks --test` is worth typing**, and not only as a check: it is what tells a route
  whose program *resolves* from one that actually delivers, so the state of a `exit 7` route
  changes between the two commands. It sends a real notification with `run 000000000000` and a
  body saying it is a test.
- **The load-average trap has a second face: somebody else's work.** The existing entry blames a
  `cargo build`; this walk was blocked by an unrelated Oracle container, a mockserver and a Spring
  Boot test suite the machine's owner was running, and every submission was refused with `cpu at
  100%: 100% of the budget is free, the machine is not`. Nothing about the fleet is wrong and there
  is nothing to wait for. Check `ps -eo pcpu,args --sort=-pcpu | head` before reading a refusal as
  a defect, and plan a delivery walk around fleet events rather than runs.
- **`git checkout -- <file>` to undo a deliberate A/B break discards the whole file.** Used to
  revert a one-line `if false` after proving a test goes red, it reverted every edit made to that
  file in the session. `cp <file> /tmp/x` before breaking it, and `cp /tmp/x <file>` after — which
  is the recipe the previous session used and this one forgot.

### The three local commands, walked as a family (session eighty-four)

`offload probe`, `offload policy` and `offload match` need **no daemon and no runs**, which makes
them the walk to do on a machine that is too busy to place anything — and they share `probed()`,
so they are a family in the sense session eighty-one means.

- **Stage a config that differs from every default**, or `--config` proves nothing: a `name`, an
  `agent.binary` that is not `claude`, a `[policy]` block with all three fields changed, a
  `[[tasks]]` entry, and a `[policy.light]` block. Then read each command twice, with the flag and
  without.
- **Set one field of `[policy.light]`, not three.** The block is three independent overrides and
  `None` inherits, so a block naming `accept` alone is the case that shows whether the report can
  tell what the owner said from what light work inherited. Three-of-three hides it.
- **Type back what the report printed.** `offload policy` says `accept work when_charging`; put
  that in a config and it must start. Every `{:?}` in operator output is a candidate: the sweep
  found `probe`'s `Laptop / Linux / X86_64 (Transient)` all parse into `offload match` because its
  parser is case-insensitive, and only `AcceptWork` could not be typed back.
- **Type the empty and the half-finished forms at `offload match`.** `""`, `","`, `cores`,
  `cores>=`, `tag=`, `toolchain:rust>=`, `nonsense`. Four of those were accepted; `""` exited **0**
  saying the device matched. A command documented as composable in scripts is one whose empty
  argument is worth typing before anything else.
- **`offload match` exits 1 for a failed match and 1 for a bad expression**, so a script cannot
  tell them apart by status alone — the refusal goes to stderr and the verdict to stdout, which is
  how they are distinguished today. Worth knowing before writing a gate around it.

### Walking the agent's own concurrency ceiling (session eighty-five)

The number `offload status` prints as *"claude-code sustains N, which is what binds"* is the
probe's, overridden by `[agent] max_concurrent`. Walking it needs nothing but a fake agent that
runs long enough to overlap:

- **A fleet of one with `cluster.enabled = false` is the sharp staging**, not a weakness of it:
  `WorkPolicy::admits` — the only function that had the ceiling — is reached by the bid round
  alone, so with no cluster it is never asked at all. Submit three runs against an install that
  sustains two and count `pgrep -c -x agent.sh`. Three is the defect; two is the fix.
- **Count the processes, not the rows.** `offload ps` said `running` three times and `offload
  status` said `runs 3/3 · claude-code sustains 2, which is what binds` — both honest about
  commitments, neither able to say that three agents were live. The process table is the only
  place the over-claim is visible.
- **Then the same thing on two daemons**, where a grant *holds* rather than refusing (ADR-0006):
  five submissions, two running, three `assigned — waiting for a slot`, draining one at a time.
  That is the regression check that matters — a new refusal on a start path can strand a held run,
  and the watch to run is `offload ps` every fifteen seconds until it empties.
- **The joiner is on probation for fifteen minutes**, so a two-node ceiling walk either waits it
  out or shortens `offload-core::fleet::PROBATION`. Until then every run lands on the founder,
  which looks like the fleet ignoring the peer and is `offload explain` telling you plainly:
  `bravo  host-runs granted but on probation for another N minutes`.
- **`pkill -f`/`pgrep -f` bit this session for the fourth time in three**, now with a pattern
  matching the *agent* script rather than the daemon. `pkill -x agent.sh` matches the process name
  and cannot match the invoking shell.

### Reading one run from two machines (session eighty-six)

`offload explain` is read in nearly every walk and had never been the subject of one. What makes it
a different walk from `offload ps`'s is that it does **not travel**: it answers from the node you
typed it on, and five of its lines are computed only where the run is. So the staging is not a set
of states, it is a set of states **times two vantages**, and the finding is always the same shape —
two screens about one run that do not agree.

- **The cheapest peer is one `join` with no `grant`.** Enrolment alone gives `{submit, deliver}`,
  which holds replicas and never wins a bid, so every run lands on the founder and the second
  daemon is a pure vantage. No probation to wait out, no `PROBATION` to shorten, and the canvass
  reads `not granted host-runs by the fleet` on every screen, which is a useful constant.
- **A node that refuses something is what makes the node-level lines appear at all.** `resume
  refused here right now` and `offload ps`'s resume footnote are `hosting_refusal`, which is `None`
  on a healthy node — so on the accepting node there is nothing to read and the defect is invisible.
  The peer without `host-runs` gives it for free; on a fleet of one, `offload drain` is the two-second
  version.
- **Walk these, in this order, and read the whole screen each time**: running, `assigned` behind
  `max_concurrent_runs = 1`, cancelled, completed, failed, a task at each of those, parked by
  `offload checkpoint`, blocked on an `--ask`, and orphaned by `kill -9` on the holder. The two that
  paid were **failed** (the verdict and the recovery line are adjacent and were contradicting) and
  **blocked on an ask** (the peer said nothing at all about the block).
- **A fake agent needs to answer `--version`**, or the probe's `probe_version` runs the whole
  script — a twelve-turn agent at four seconds a turn is a **48-second** startup during which the
  daemon has no socket and the log stops after `member of fleet …`, which reads exactly like a
  daemon that failed to start. `[ "$1" = "--version" ] && { echo "0.0.1"; exit 0; }` on line two.
  Without it `detect_agents` returns nothing and the node says `no authenticated agent`.
- **…and that invocation runs in the *daemon's* working directory, which is wherever you typed
  `offloadd`.** So a fake agent whose turn loop ends in `git add -A && git commit` — the shape every
  migration recipe in this file uses, because commits are how a leg proves it ran — committed twelve
  files onto the **project's own `main`** while the probe was asking it for a version string. Twelve
  real commits, author and all, between the last session's and this one's. Two guards, and the first
  is enough on its own: **answer `--version` and exit before anything else**, and `cd` to a
  scratch directory at the top of the script so a stray invocation cannot touch the tree you are
  working in. A fake agent is a program the daemon runs for its own reasons as well as yours, and
  the probe is not the only one — it is simply the one that runs *first*, before there is a socket
  to notice anything with.
- **…and the same script's `CLAUDE_CONFIG_DIR` is your own when nothing set it.** The per-leg tag
  `TAG=$(basename "$CLAUDE_CONFIG_DIR")` — session fifty-seven's recipe for telling two legs apart —
  came out as `.claude-alt` on that probe invocation, because the daemon inherits the shell's
  environment and `[agent] config_dir` is only applied when it spawns a *run*. A tag that names your
  home directory instead of the node is the tell that the script ran outside a run.
- **`pgrep -x agent.sh` counts zero for a fake agent with a `#!/usr/bin/env bash` shebang.** The
  kernel runs it as `bash /path/agent.sh`, so the process *name* is `bash` and `-x` never matches —
  the instrument session eighty-five recommended reads 0 on a machine with the agent very much
  alive, and `pkill -x agent.sh` is a silent no-op. `pgrep -fa agent.sh` to look, a recorded pid to
  kill, and remember that `-f` is the trap this file has recorded four times. The shebang decides
  it: `#!/bin/bash` gives comm `agent.sh`, `#!/usr/bin/env bash` gives comm `bash`.
- **An `--ask` walk needs the daemon started with `offloadd` on its `PATH`**, since the hook the
  fake agent pipes into is the daemon's own binary and the agent inherits the daemon's environment.
  `PATH="$B:$PATH" nohup offloadd --config …`.
- **…and a fake agent that sleeps after posing its question keeps the slot after the answer.** The
  `sleep 600` that holds the run mid-tool-call does not end when somebody approves, so the next
  submission sits `assigned — waiting for a slot` and the walk looks stuck. Kill the recorded pid,
  or make the sleep short enough to be patient with.
- **Truncating the output hides the canvass.** `offload explain | head -14` cuts at the canvass
  header on a run with a `checkpoint` line, which reads as *the fleet answered nothing* — ten
  minutes chasing an empty canvass that was `head`. Sample the tail as well as the head, or drop
  the pipe.

### Staging the audit log's own rows (session eighty-seven)

`offload audit` is the third log and the one nobody had walked. Four of its six kinds are cheap to
produce on **one** daemon; the other two need a fence to fire, which is a two-daemon migration.

- **`accepted` alone means no cluster.** With `[cluster] enabled = false` a fleet of one records
  `accepted here at epoch 1` and no `granted` row at all, which is correct — nothing arbitrated —
  and reads as a missing row if you were expecting the pair. `enabled = true` with `listen` on
  loopback and no seeds gives a real one-node bid round, and then both rows.
- **`rescued` and `reclaimed` can be staged in twenty seconds on one daemon**, without the
  three-leg ping-pong. `adopt` decides from a turn marker beside the worktree —
  `<state>/worktrees/<run>.turn` — and answers `Superseded` whenever the marker is *behind* the
  checkpoint. So: submit, wait for a capture, `offload checkpoint` to park it, then
  `echo 1 > <that>.turn` and `offload resume`. Write an untracked file into the worktree first and
  you get `Rescued { Checkout }`; leave it clean and the same steps give
  `Reclaimed { Redundant }`, which is the control. That is the shape a migration away and back
  produces, arranged directly.
- **The worktree directory is the *full* 32-character run id**, while every listing abbreviates to
  twelve — so `ls $STATE/worktrees/<what ps showed>*` finds nothing and looks like the rescue not
  having happened. `ls -d $STATE/worktrees/${FULL}*`.
- **A hundred rows is the cap, so a walk about truncation needs sixty submissions.** A one-turn
  fake agent with `gap` at 0 does it in under a minute, and each run contributes two rows. Read the
  *tail* of the output: that is where the notice is, and where its absence was.
- **`superseded` needs two daemons and one `kill -STOP`, and takes about ninety seconds.** Start
  both with `setsid --fork` (the freeze does not hold otherwise — see above), submit on alpha with
  `--prefer node=alpha`, `kill -STOP` alpha's daemon three seconds later, and poll `offload explain`
  **on bravo** until its `holder` line says `bravo` — measured at 45–70 s. The agent is a separate
  process and keeps going through the freeze, which is the point. Then `kill -CONT` and read
  `offload audit <run>` **on alpha**. The fake agent's `turn` file decides which row you get: a
  turn longer than the freeze (400 s) gives `superseded … had not finished a turn`; one that ends
  *inside* it (50 s) gives `superseded after finishing` — alpha reads its agent's result before
  the gossip, completes the run at the lower epoch, and the merge replaces that. Session ninety.
  Neither staging produces `refused`: no write is attempted after alpha learns, so that row is
  still unread on a screen.
- **Check the clock against `date`, not against plausibility.** `offload audit` renders UTC; on any
  machine that is not on UTC the column is hours off and looks perfectly reasonable. `date '+%H:%M
  %Z'` beside the command is the whole test, and it is worth running against any report that prints
  an absolute time.

### Three daemons, and reading a run from a node that is neither holder nor home (session ninety-one)

- **The staging.** alpha founds and submits, with `agent.binary = "/bin/false"` so it cannot host;
  charlie is the only node with a fake agent (the one in this file's fake-agent notes, forty turns
  at three seconds); bravo is enrolled but never granted `host-runs`. `PROBATION` at five seconds so
  charlie can host. `offload run` on alpha lands on charlie; `offload explain` on bravo is the
  third vantage.
- **On loopback, two joiners seeded only at the founder never meet each other directly.** A
  loopback bind announces no address, so bravo knows charlie's only through alpha's gossip and has
  nothing to dial — `offload nodes` says `alive`, and a canvass from bravo says `not reachable from
  this node` about charlie (before session ninety-one it said `no answer`). **And the moment alpha
  dies they are partitioned from each other**: each marks the other dead and bravo, now the lowest
  live id, *orphans charlie's run* while charlie carries on. Correct for a partition, and a
  different walk. Seed every node with every other if the walk is not about this.
- **Reaching `ArbitratedBy(None)`** — the one `explain` arm with no staging until now: after the
  above, `kill -9` alpha, wait twenty seconds, restart bravo. Its view holds only itself. Adding
  charlie to its seeds and restarting again is the control: it meets charlie, learns alpha from
  it, and becomes the arbiter.


### Two machines: the laptop and the Mac mini (session ninety-two)

- **The Mac meshes now.** An ad-hoc-signed sender gets through on v4 and v6, and so does
  `offloadd` (ADR-0037, second amendment). Check before assuming the old block: compile a
  twenty-line C sender on the Mac (`cc -o snd-$RANDOM snd.c`, which is ad-hoc signed and new to
  the system) and send to a `python3` listener here, interleaved with the Apple-signed `python3`
  as the control.
- **`ssh -b 192.0.2.240 macmini` is stale**, since the laptop no longer has that address. It is
  dual-homed on one subnet (ethernet `.5`, wifi `.23`), and unpinned ssh times out intermittently.
  `-b 192.0.2.23` or `-b .5` works and each fails at moments; retry across both.
- **The Mac's own link drops.** `ifconfig en0` reads `status: inactive` at moments and Apple's
  `ping` says `sendto: Network is down`, so the mesh goes `dead` for up to ten minutes and heals.
  A round placed while the Mac is dead has placed nothing worth reading. Script each round to wait
  for `offload nodes` to say `alive` for both, and read the canvass to confirm both were asked.
- **Build on the Mac from a bundle, and do not install over a running binary.** `git bundle create
  src.bundle main`, `scp`, then `git fetch /tmp/src.bundle main:laptop-main && git checkout
  laptop-main`. Then run `cargo build --release -p offload-node -p offload-cli` there, about a minute
  incremental, and use `target/release/` directly. `scripts/build-macos.sh` copies into `~/bin`, and
  an old daemon from 10 September was still running out of it (`~/.offload-w6`, an older fleet). It
  was left alone. The Mac's checkout had been on a detached `4622eb6d wip: quinn-udp probe` that no
  branch held; it is branch `mac-wip-quinn-udp-probe` there now, and the checkout is `laptop-main`.
- **Enrol the Mac by invitation, not by passphrase over ssh.** `offload id --state-dir /tmp/mw/b`
  on the Mac, `offload invite <id> --name macmini --grant host-runs` here (the passphrase on stdin,
  because `host-runs` is beyond an approver), then `offload join --token <token>` there. The token
  is safe on a command line; the passphrase is not. Probation is fifteen minutes from the
  invitation's `issued_at`, so invite first and build while it runs.
- **A weights walk reads scores from the canvass.** `offload explain` on a run *in flight* asks
  every node, and the holder answers `already holds this run`, so it shows only the other's bid,
  less the 30-point migration penalty. Pair each arm with a second one preferring the loser, which
  shows the winner's bid. A preference that loses prints both totals and the deciding terms in
  `offload run`'s own output. Raise the fake agent's turn to forty seconds so the run is still in
  flight three seconds later.
- **A round has to refuse to run when its peer is missing, not wait and then submit.** The
  battery watcher waited thirty seconds for the Mac and submitted anyway, spending the one
  38-second window the owner could give off mains on a round with one bidder. Check `alive` in
  the same step that submits, and exit loudly if it is not.
- **`cpu N%` is the one-minute load average over cores**, not utilisation, so on this laptop four
  busy loops on eight threads read 89 % (a refusal) and three read 64–69 %. Aim below 75 % for a
  normal run, since past it the node declines rather than bidding low. Wait for the figure to
  settle before submitting, because it lags by a minute.

### An Android node on the SDK emulator (session ninety-two)

- **Build for the emulator's architecture:** `ANDROID_ARCH=x86_64 scripts/build-android.sh`. The
  x86_64 images run at native speed under KVM; an aarch64 build will not run there.
- **The headless emulator segfaults on this laptop**, with every AVD, every `-gpu` mode, and with or
  without snapshots (emulator 37.1.11, kernel 7.2, `qemu-system-x86_64-headless`, SIGSEGV within
  seconds of "full startup"). The windowed one runs, so drop `-no-window` and a window appears.
  `-no-snapshot` avoids the stale `default_boot` snapshot, which is "incompatible" and was the
  first crash.
- **`adb root` before starting the daemon.** As the `shell` user, SELinux refuses the control socket
  under `/data/local/tmp` (`binding control socket … Permission denied`). Termux keeps its files in
  its own app directory, so this is an `adb shell` problem and says nothing about a phone.
- **Start it without holding adb's terminal:** `adb shell 'cd /data/local/tmp/ow && nohup
  ../offloadd --config node.toml > d.log 2>&1 < /dev/null &'`. Without the redirects the call
  hangs until the daemon exits.
- **Networking is the emulator's NAT.** The guest reaches the laptop at `10.0.2.2`. For the way
  back, `adb emu redir add udp:7602:7602` and seed the laptop at `127.0.0.1:7602`. The guest's own
  dial is what meshed first.
- **The battery is the console's:** `adb emu power ac off`, `adb emu power status discharging`,
  `adb emu power capacity 50`. The probe reads it straight away and the daemon within its 30-second
  re-probe. A phone's default policy accepts only while charging, so a battery walk needs
  `[policy] accept = "always"`.
- **Tasks, not agent runs.** The Android shell has no `git`, so an archive or repo workspace cannot
  be made there. A `[[tasks]]` entry with `#!/system/bin/sh` and a `sleep 40` gives a run that is
  still in flight when `offload explain` canvasses.
- **The emulator's load is the laptop's load.** Loading the guest raised the host's figure too
  (guest 50 %, laptop 39 %), so an arm that needs one node busy and the other idle cannot be staged
  this way.

### A real phone under Termux (session ninety-two) — the debugging harness, not the product

- **Termux from F-Droid, sshd on 8022, key login.** `pkg install openssh curl`, `passwd`, `sshd`.
  `whoami` gives the user (`u0_a176`). `ifconfig`/`ip` are denied to apps, so get the phone's
  address from its wifi settings. Install the laptop's key once with `ssh-copy-id -p 8022`, and
  keep the password out of transcripts.
- **`scp` does not expand `$PREFIX`.** Copy to `/data/data/com.termux/files/usr/bin/`.
- **Start the daemon with `ssh -f -n … 'nohup offloadd … < /dev/null &'`**, or the ssh call holds
  the terminal, and a tool call wrapping it runs its *remaining* commands whenever it finally
  returns. That is how a later `rm -f state.db*` landed under a live laptop daemon here.
- **A half-upgraded Termux breaks `curl`** (`cannot locate symbol ngtcp2_…`). `pkg upgrade` fixes it.
- **Tasks start with a clean environment**, which under Termux means no `PATH` and no
  `termux-exec`, so `getprop` inside a task is `Permission denied`. A `[[tasks]]` entry meant for
  Termux has to set `env`.
- **Claude Code does not run in Termux**: `Native binaries for linux-arm64-android are not available
  on this release channel` (2.1.282). The probe correctly reports no agent.
- **Backgrounded Termux gets the little cores** (`Cpus_allowed_list: 0-2`), so `cpu_cores` flips
  between 8 and 3 with the app's state. **Wifi power save** puts ping at 5–366 ms, so probes past
  the 500 ms timeout make brief suspicions that never become a death.

### The Android app on a real phone (session ninety-two) — ADR-0066

- **Build and install:** `scripts/build-android-app.sh`, then `adb install -r
  android/app/build/outputs/apk/debug/app-debug.apk`. `adb shell pm grant se.mach25.offload
  android.permission.POST_NOTIFICATIONS` saves a tap. Opening the app starts the daemon, so
  `adb shell am start -n se.mach25.offload/.MainActivity` is enough.
- **Drive it with `scripts/android-offload.sh -s <serial> -- <command>`**, which runs the app's own
  `offload` through `run-as`, choosing `--socket`, `--state-dir` or `--config` for the subcommand.
  Wireless adb devices count as emulators to `adb -e`, so always use `-s` when an emulator is up too.
- **Config goes in through `/data/local/tmp`:** `adb push node.toml /data/local/tmp/`, then `run-as
  se.mach25.offload sh -c 'cp /data/local/tmp/node.toml files/node.toml'`, then `am force-stop` and
  `am start`. The state dir is `/data/user/0/se.mach25.offload/files/s`.
- **A task on the app uses the system shell**, `command = "/system/bin/sh"`, with full paths inside
  (`/system/bin/getprop`), because a task's environment is clean.
- **Pairing:** Android Studio's Device Manager (QR code), or `adb pair <ip>:<port> <code>` followed
  by `adb connect` to the port on the main Wireless debugging screen. A wifi toggle drops wireless
  adb, and it does not come back by itself.
- **Watching off the LAN without adb:** `/tmp/mw/watch-phone.sh` records the laptop's `nodes` line
  for the phone every 5 s, since adb and ssh both drop when the phone leaves wifi.
- **Gradle 8.14 needs a JDK ≤ 24.** `/usr/lib/jvm/java-21-openjdk` here is an empty stub, and
  Android Studio's bundled JBR is 25. The script uses `~/.jdks/jbr-17*`.

### Phase 8's sentence with the phone off the LAN (session ninety-two)

- **The phone as the node with no agent**: `[[tasks]]`, `[[triggers]]` and `[[sinks]]` in the app's
  config, each `command = "/system/bin/sh"` with the program in `args`. A trigger is a `while true;
  do sleep 25; echo …; done` loop. The sink appends `$OFFLOAD_TITLE` to `files/phone.log`, which
  `adb shell run-as se.mach25.offload cat files/phone.log` reads.
- **Rules go in on the phone** through `scripts/android-offload.sh -s <serial> -- when …`. The
  escalation's `--repo` has to be a URL the *laptop* can clone, since that is where the agent run
  lands. The phone does not open it.
- **On a metered link only light work is taken**, unless the owner says otherwise. So either
  `--demand light` on `every`, `when` and `run --task`, or `metered = "no"` in the phone's config
  when the plan is unmetered, which Android cannot know.
- **Two failures, one escalation**: a failing task is restarted once (ADR-0058) and fails again
  while the first escalated run is still going, so the rule counts it as `DROPPED`.
- **Heat on demand:** `adb shell cmd thermalservice override-status 3` (0–6) sets the status the
  platform reports on the phone from a plain `adb shell`, and so what the app writes to
  `host-facts.json` within 15 s. `cmd thermalservice reset` puts it back. That is ADR-0068's walk.

### The iOS app in the Simulator (session ninety-two) — ADR-0070

- **Where:** the Mac mini, in a clone of its own (`~/mach25-offload-ios`, cloned from a `git bundle`
  of `main`), so the Mac's older checkout and its WIP branch stay untouched. Xcode 27. The "iPhone
  15" device runs the iOS 17.5 runtime.
- **Build:** `scripts/build-ios-sim.sh`, which makes `dist/ios-sim/Offload.app`, about 27 MB. There
  is no Xcode project: `swiftc` with a bridging header, then `codesign -`. A change of
  `IPHONEOS_DEPLOYMENT_TARGET` rebuilds every C dependency, which takes the Mac about ten minutes.
- **Run:** `xcrun simctl boot "iPhone 15"`, `simctl install booted dist/ios-sim/Offload.app`,
  `simctl launch booted se.mach25.offload`. Booting the Simulator took the Mac's load to 29 for a
  minute, and `offload status` on the iOS node said `cpu 100%`. That was honest: a Simulator app
  reads the Mac's load average.
- **Drive it from the Mac's own CLI**, built from the same clone:
  `offload --socket /tmp/offload-ios.sock status`. Commands that read the state directory take
  `--state-dir "$(xcrun simctl get_app_container booted se.mach25.offload data)/Library/s"`.
- **A peer to mesh with** is a second `offloadd` on the Mac in `/tmp/omac` with `listen =
  "[::]:7436"`. The iOS node takes 7435, and a walk daemon may already hold 7433. Found a fleet
  there with `offload init --state-dir /tmp/omac/s`, then run `offload invite <ios node id>`, then
  `offload join --state-dir <container>/Library/s --token …`. **Restart both daemons after that.**
  A daemon that started outside a fleet joins the mesh only when it starts again. For the app, that
  means sending it to the background (`simctl launch booted com.apple.Preferences`) and launching it
  again: the background stop and the foreground start are the restart.
- **Some `ssh macmini '…'` calls returned long after the work inside them had finished.** A build
  that cargo timed at 1m22s returned after about twenty minutes, with nothing left running. The
  cause is not known. Run long steps on the Mac under `nohup … &`, and poll their output file.
- **The app icon** is `ios/Assets.xcassets`: one opaque 1024×1024 PNG rendered from
  `assets/app-icon.svg` with its rounded corners removed, because iOS applies its own mask. It was
  rendered on the laptop with `inkscape`. The build compiles it with `actool` and merges the keys
  actool writes into `Info.plist`. A fresh install lands on the second home page, which `simctl`
  cannot swipe to. `xcrun assetutil --info Assets.car` and the bundle's `AppIcon60x60@2x.png` show
  what SpringBoard is given.
- **Keep one log per daemon instance** (`daemon-$round.log`) when a walk restarts one. The one round
  that went wrong could not be read afterwards, because the next start had overwritten its log.

### A hardware approval key on an Android device (session ninety-two) — ADR-0069 §4

- **Make the key** with the app's "Key" button. It is on purpose, not on install: the key does
  nothing until a delegation names it. `run-as se.mach25.offload cat files/s/approval-key.json`
  shows the public key and the level Android verified. The tablet gave `TRUSTED_ENVIRONMENT`, the
  the phone `STRONGBOX`. The device needs a screen lock, or the key cannot be made.
- **Name it.** Either `scripts/android-offload.sh -s <serial> -- init --hardware-key --name <name>`
  founds a fleet that approves with it (no passphrase to type), or `grant approve --hardware-key`
  names it in an existing fleet (asks for the passphrase). Restart the app after `init`, because a
  daemon that started outside a fleet does not join one.
- **Invite through it** from the device itself: `scripts/android-offload.sh -s <serial> -- invite
  <id> --name <name>`. The command waits up to 120 s. Keep the app open: a notification says a
  request is waiting, and the open app shows the system prompt, worded from the certificate. The
  token that prints has been checked against the delegation already.
- **The other node** joins with the token and needs the device as a seed when mDNS is off
  (`seeds = ["<tablet ip>:7433"]`). A mesh on each side is the proof that both verified
  TEE-signed papers.
- **Re-approval through the key** needs a member whose approval is due, so a walk build: set
  `APPROVAL_LIFETIME` (6 h), `REAPPROVAL_WINDOW` (350 min) in `offload-core/src/fleet.rs` and
  `RENEW_RETRY` (20 s) in `offload-node/src/mesh.rs`. Build the laptop binaries and the product app,
  copy them aside, `git checkout` the two files, and install and run from the copies. **Both sides
  need it**, because the approver checks the window too. Pick a lifetime longer than the walk
  measured from the *founder's* approval: the approver's own approval lapses on the walk build.
  Then `reapprove <member>` on the device; the prompt appears at the member's next ask, and the
  signed certificate is served at the one after. Reinstall the normal build afterwards.
- **Pressing an app button from adb:** take a screenshot to find it (`adb exec-out screencap -p`)
  and `input tap` the screenshot's coordinates. `uiautomator`'s bounds put the Tab's button row
  under the status bar before the insets fix. `adb logcat -b crash` is where an app crash's cause
  is. The first prompt's crash (a missing `USE_BIOMETRIC`) was there and nowhere else.
- **Wireless adb on a paired phone** comes back by itself: `adb mdns services` lists
  `_adb-tls-connect` once Wireless debugging is on, and `adb devices` already has it. Use the whole
  serial (`adb-<serial>-<suffix>._adb-tls-connect._tcp`) with `-s`.
- **A launcher can keep an app's old icon after a reinstall.** On the tablet, the running app's
  taskbar entry kept the first install's stock icon while the home screen showed the new one. The
  APK was right (`aapt2 dump badging`). A new task, a set task description and a new versionCode
  each changed nothing. `adb shell am force-stop com.sec.android.app.launcher` did: the launcher
  restarts at once and rebuilds its cache.
- **Do not reinstall on a device under measurement.** `install -r` stops the app and its daemon.
  One phone death in the doze walk was that, not doze.

### The product app (session ninety-two) — ADR-0071

- **Build:** `scripts/build-android-product.sh` builds the daemon, the CLI and `liboffload_mobile.so`
  for both ABIs, generates the Kotlin, and assembles `android-app/`. Install with `adb install -r
  android-app/app/build/outputs/apk/debug/app-debug.apk`. The harness has its own script and is
  untouched.
- **One daemon per device.** Both apps host a daemon on port 7433. Before opening the product app,
  run `adb shell am force-stop se.mach25.offload` on a device that has the harness running.
  **Since session ninety-three neither the phone nor the tablet has the harness installed**: the owner
  removed it from both. A walk that needs it builds it (`scripts/build-android-app.sh`) and installs
  it, starting a fresh node.
- **Package:** `se.mach25.offload.app`, activity `.ui.MainActivity`. The state directory is the
  app's own (`/data/user/0/se.mach25.offload.app/files/s`), so it is a new node with no fleet until
  it joins one.
- **Moving a device from the harness to the product app, as the same node** (done on the phone):
  1. install the product app;
  2. force-stop the harness (`am stopservice` is refused from the shell);
  3. `adb exec-out "run-as se.mach25.offload sh -c 'cd files && tar cf - --exclude s/offloadd.sock --exclude s/daemon.lock --exclude s/daemon.lock-journal --exclude s/approval-key.json --exclude s/host-facts.json s node.toml phone.log'"`;
  4. rewrite `se.mach25.offload/` to `se.mach25.offload.app/` in `node.toml`;
  5. push the archive and `run-as se.mach25.offload.app tar xf … -C files`, then `chmod 771 files`
     (extraction left it 777);
  6. **rename the harness's `files/s` to `s.moved-to-app`**, so opening the harness later starts a
     fresh node rather than a second copy of this identity;
  7. open the product app.

  The laptop then saw `phone-app` alive under the same id. The approval key does not move: Android binds
  a Keystore key to the app that made it.
- **Joining the product app by link:** `offload invite <id>` prints an `offload://join?token=…` line.
  `adb shell am start -a android.intent.action.VIEW -d "'<link>'"` opens it the way a tap would (the
  inner quotes keep the shell from eating `?` and `=`). Do not paste a token with `adb input text`,
  which drops the end of a long string. `pm clear se.mach25.offload.app` gives a fresh node, but wait
  for it to finish before `am start`, or the launch sticks on the splash ("failed to attach").

### The owner's fleet as left, and updating each member (end of session ninety-two)

Fleet `f1ee7001`. Every member is real now: the laptop and the Mac have authenticated Claude Code on
one account, so **an agent run costs usage**. Walk with the fake agent in a fleet of its own, not here.

- **The laptop's node** is `/tmp/mw`, config `/tmp/mw/a.toml`, `[agent]` naming
  `~/.local/bin/claude` with `config_dir = ~/.claude-alt`, and it owns `~/offload` (ADR-0074). Restart
  with `kill $(cat /tmp/mw/a.pid)`, then `nohup ./target/debug/offloadd --config /tmp/mw/a.toml > /tmp/mw/aN.log
  2>&1 &` and write the new pid back. The fake agent's config is `a.toml.fake-agent`. `/tmp/hw` and
  `/tmp/hw2` pin `[workspace] dir` to their own state directories, since only one node may own
  `~/offload`.
- **The Mac mini** (`macmini`) runs from the worktree `~/mach25-offload-host`, **not** the clone
  `~/mach25-offload-ios`, which has the owner's uncommitted changes. To update: `git bundle create
  src.bundle main`; `scp` it to `macmini:/tmp/src.bundle`; there, `git pull --ff-only /tmp/src.bundle main`
  in the worktree, `cargo build --release -p offload-node -p offload-cli`, then
  `scripts/install-macos-service.sh`. Since session ninety-four the daemon is a **launchd LaunchAgent**
  (`se.mach25.offloadd`, running `~/bin/offloadd`), so the script *is* the update: it installs the
  binaries by rename and restarts the service. Restart alone with `launchctl kickstart -k
  gui/501/se.mach25.offloadd`. There is no `daemon.pid` any more; `launchctl print
  gui/501/se.mach25.offloadd` names the pid. The log is `~/.offload/daemon.log`. It starts at *login*,
  not boot, and the Mac has no auto-login, so after a reboot somebody logs in. The first minute after
  a restart can show `No route to host` sends and a fleet of one. That is the Mac's link, not Local
  Network privacy: the new daemon met the laptop about 40 s in. **But a rebuild can lose it.**
  After the ADR-0080 build (session ninety-four) every send was `No route to host` for minutes,
  while Apple's `/usr/bin/python3` on the Mac reached the laptop's UDP port at the same moment: a
  new ad-hoc signature is a new app to macOS's Local Network privacy. The fix is at the Mac's
  console (System Settings → Privacy & Security → Local Network, or the prompt if one is showing).
  Run the python control before blaming the link. A stable signing identity would make the grant
  survive rebuilds. It reads `~/.config/offload/node.toml`: the agent named in full (an ssh-started daemon has no
  login `PATH`), mDNS on, seeds for both laptop addresses. **ssh from the laptop needs a retry**
  (`-b 192.0.2.5`): the first attempt wakes the Mac, which sleeps after a minute idle until
  ADR-0077 holds it awake, and that happens only once it may host.
- **The phones and the emulator** take `android-app/app/build/outputs/apk/debug/app-debug.apk`
  (`scripts/build-android-product.sh`) with `adb install -r`, on the phone over wireless adb
  (`PHONESERIAL`), then `am start -n se.mach25.offload.app/.ui.MainActivity`. Since ADR-0079 the
  app's daemon **stops itself** about 90 s after the app leaves the screen, unless it holds a run.
  So a phone that looks `draining` in `offload nodes` is normal. Open the app to bring it back. The laptop's `/tmp/hw` and `/tmp/hw2` daemons now write `node.pid`.
- **Every node must be on the same wire version** (v36 since session ninety-three), since `MIN_VERSION` tracks `VERSION`: update
  all of them in one pass after a bump, the Mac included.
- **Profiling a phone's daemon:** Samsung's user build refuses perf events (`simpleperf stat` from
  `adb shell` says "not supported"). Profile the laptop's node in the same fleet instead: `eu-stack -p
  <pid>` sixty times, half a second apart, and count the busy threads' frames. That is how the gossip
  cost was found.
