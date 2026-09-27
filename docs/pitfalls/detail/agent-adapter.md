# The agent adapter: argv, grants, MCP, permissions — full entries

The working rules are in `../agent-adapter.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **An allowlist entry can quietly be a shell, and the check has to judge the *program* rather
  than the text.** `Bash(sh:*)` looks scoped and isn't; nor are `python`, `xargs`, `env`,
  `find`. And nor are four spellings that compared against a list of names and walked past it:
  a **path** (`/bin/sh`, `./sh`), a **version suffix** (`python3.11`), a **quote** (`"sh"`,
  `/bin/"sh"`), and an **environment assignment** (`FOO=1 sh`). `Scoped` is what
  `require_scoped` accepts from a *repository's* `.offload.toml`, so each was a way for content
  to grant itself a shell while `offload run` reported it as scoped — measured on `claude
  2.1.238`, where `Bash(/bin/sh:*)` ran a command the same run was refused on its own.
  `program_named` normalises; over-matching is the safe direction, because a false positive is
  a refusal that names itself. An allowlist that admits one is worse than no allowlist, because
  it is trusted.

- **A positional argument needs an option terminator, and the prompt is one.** The prompt sat
  in the middle of the argv, so `offload run -- "--version should be documented"` reached the
  agent as a flag: `error: unknown option`, exit at spawn, measured on `claude 2.1.238`. A run
  that never starts for a reason nothing in the fleet can explain, and an unattended one retries
  it to the same end. It goes last, behind `--` — which also means `--allowedTools` is the last
  *flag* rather than the last word, because `--` terminates a variadic option instead of
  becoming another pattern (measured, along with the grants still applying after it).

- **`--print` ends at the first turn that makes no tool call, so the resume nudge decides
  whether a migrated run finishes.** `CONTINUE_PROMPT` is the only thing the fleet says to an
  agent it has just moved — re-sending the original prompt would restart the task — and the first
  version said "continue from where you left off", which a model can satisfy with one step and a
  sentence about where it is. Watched on a real migration: resumed on the new machine, did one
  more step, reported, and the run was recorded `Completed` at six steps of thirty-one. The
  daemon did nothing wrong; the agent reported success. What makes it worth a change of words is
  that `Completed` is **terminal** — nothing retries it, nothing resumes it, and nothing says the
  rest was abandoned, so a failed run is looked after and an abandoned one looks like a success.
  The nudge names the task and asks for completion now, and rules out the reading that ended the
  run.

- **The agent decomposes a compound command, which is why prefix grants are sound at all.**
  Under `Bash(cargo test:*)`, `cargo test --version && cargo build --version` is refused naming
  the uncovered half — measured on 2.1.238. Load-bearing and easy to assume the other way: a
  plain prefix match would make every scoped grant a shell via `;` and the whole feature
  decorative. It is also why an interpreter in the *first* position is the only leak of its
  shape, and therefore why `risk` is a list of programs rather than a parser.

- **The agent's state directory is movable, and two things break silently when it moves.**
  `$CLAUDE_CONFIG_DIR` relocates transcripts *and* `.credentials.json`, so a node that assumes
  `~/.claude` either captures nothing all night (checkpoints fail, the run is unmigratable, and
  only the run's own log says so) or reports itself unauthenticated and refuses every run. The
  environment is read once per crate — `ClaudeCode::config_dir()` for transcripts,
  `claude_config_dir` in `offload-probe` for credentials — and the path functions take a config
  directory rather than a home, so nothing resolves it twice and no test can accidentally write
  into somebody's real agent state.

- **A permission hook fires for every tool call, and never says whether permission was needed.**
  So the harness decides what is worth asking about, and both halves of that are traps. Asking
  about everything means asking a person about reads the agent would have allowed by itself
  (ADR-0004's line, crossed from the other side). And *nobody answered* must print **nothing**
  rather than `deny`: an explicit denial would refuse calls the agent would have allowed, so the
  pass-through is what makes the channel's failure mode identical to not having it. Same reason a
  grant suppresses the question — a match means "let the agent decide", never "allow", which is
  what makes a second matcher safe to have at all.

- **What is worth asking about depends on the mode, so a constant list is wrong somewhere.**
  `ask_tools(mode)`, not `ASK_TOOLS`: the permission mode *is* what decides whether the agent
  gates a call. Under `AcceptEdits` a `Write` matcher stops the run to ask about something the
  agent allows by itself; under `Ask` the absence of one denies every edit in silence while the
  run looks like it is working. The old constant was wrong in both directions at once, and the
  silent direction is what kept ADR-0008's refusal of `Ask` alive for two phases.

- **A channel that can ask about everything has to be able to stop.** `--ask=N` is a budget on
  the *interruption*, because under `Ask` an ordinary run puts thirty questions on somebody's
  phone and the thirtieth is not read. Running out is **not a denial** — the calls after it are
  decided by the agent's own rules, which is what nobody-answering does and what every run did
  before the channel existed, so the failure mode of running out is the behaviour already
  shipped. Spent when a question is *put* to a person, never when a grant or an unreachable fleet
  meant nobody was shown one; counted on `RunProgress`, so a migration continues the count rather
  than handing the new node a fresh twenty.

- **…and a flag whose promise the fleet cannot keep has to say so at the keyboard.** The clause
  above — an unreachable fleet spends nothing — is the right behaviour and it was completely
  silent: `offload run --ask` on a fleet with no route to a person printed the run id and nothing
  else, and the run then behaved exactly as if the flag had not been passed. Nothing in the log,
  nothing in `offload asks`, and one `tracing::debug!` on a holder nobody is logged into.
  `--notify push` has answered this since session nine, and `--ask` needed it more: an audience
  decides who *hears* about a run, this decides what the run *does* when it is blocked. Both ends
  now read one predicate (`deliver::can_reach_a_person`), so the submission and the blocked agent
  cannot say different things about one fleet — and a configured route counts before it is known
  to work, because broken or asleep is a route and "nobody to ask" is a different sentence.

- **Credentials do not migrate.** A run moves to a node that already has its own auth. Auth
  is a node capability, never payload in an assignment. If a design requires shipping a token
  to a peer, stop. The corollary for capabilities a run *uses* (ADR-0011): a resource held
  by another node is reached by proxying the call to it, never by fetching its credential.

- **An MCP config without `--strict-mcp-config` is ambient authority.** The run inherits
  whatever servers the host user configured for themselves *and* whatever `.mcp.json` the repo
  ships, so what a run can reach depends on which machine won the bid and a repo can grant
  itself a service by committing a file. Same failure as an allowlist with a shell in it, and
  much harder to notice — it was true here until it was measured, on an ordinary laptop, and the
  run had the owner's mail and calendar. The flag is now on every argv this adapter builds,
  unconditional and not configurable, and the test asserts it for resumed and `Full` requests as
  well as fresh ones: the failure is silent, and a resumed run that lost it would be the more
  dangerous one for having been trusted once already.

- **A command line is world-readable, so a credential never goes on one.** A resource's `env`
  is where the token for the service it reaches lives, and `/proc/<pid>/cmdline` is readable by
  any local user on a default Linux — measured, not assumed. So `--mcp-config` is handed a
  `0600` file the node writes and removes when the leg ends, while `--settings` beside it stays
  inline because it carries no secret. The mode is set on the file rather than left to the
  umask: truncating an existing one keeps whatever mode it had.

- **A grant that hands over a tool and not permission to call it grants nothing.** Projecting a
  `Role::Resource` into `--mcp-config` gave the agent a tool it could see and was then denied, so
  `--use email` made a mailbox visible and unusable. The config and the `mcp__<id>` allowlist
  pattern come out of *one* function for that reason. And they are resolved **on the holder**,
  never at submit: the pattern is spelled with a node's own name for its own resource, which is
  the same sentence as a grant travelling as a service and nothing about a resource travelling
  at all.
- **The agent's cost and its token counts come from different places with opposite merge rules.**
  Measured on two legs of one conversation (claude 2.1.251, Haiku): the fresh leg's `result`
  reported `total_cost_usd` $0.026998 and the `--resume` leg's reported $0.006026 — **per leg, not
  cumulative**. `Supervisor` adds each result's cost into the run's total once per leg, which is
  correct *because of* that and would silently double every migrated run's cost if the agent ever
  changed it; the assumption was load-bearing and unverified until this. The transcript is the
  opposite: it accumulates across legs, so a total derived from it **replaces** rather than adds.
  One field taking both would double-count or erase, which is ADR-0005's one-field-two-facts shape
  and the reason ADR-0040 gives tokens their own field and their own rule.

- **Never sum `usage` off the live stream; it is a partial snapshot.** The three assistant messages
  of a real run reported `output_tokens` of **1, 2 and 4** on the wire, with `stop_reason: null` on
  every copy; the same three messages in the transcript reported **236, 85 and 42**, and the
  `result` event's total of 363 agrees with the transcript. Anything costed or bounded from the
  streamed numbers is out by roughly fifty times, in the direction that looks harmless — a run that
  appears to have consumed almost nothing. The stream is for *events*; the transcript is for
  *numbers*.

- **One assistant message is several events, and they share a `usage` object.** The agent emits one
  event per content block — a thinking block and a tool call are two events and one message — each
  carrying the same `message.id` and the same usage. Summing rows gives **726** where the truth is
  **363**, exactly double for a two-block message. Deduplicated by `message.id`, the transcript
  reconciles **exactly** with the sum of every leg's `result` on all four counters: input 44,
  output 517, cache-creation 9,474, cache-read 104,639. That exactness is what makes the transcript
  usable as a source at all, and it is worth re-measuring rather than trusting if the agent is
  upgraded.
- **The transcript lags the event stream, by an amount the agent does not promise.** Found by
  building ADR-0040's token column and walking it: a run capped at one turn reported a blank
  column, and the reason was not the parser. The checkpoint taken at the turn-1 boundary captured
  **16,999 bytes** of transcript whose rows were `queue-operation`, `user`, `attachment`,
  `atis-latch` and `ai-title` — and **no assistant rows at all**; the same file on disk after the
  run held three. The stream had already delivered the turn; the file had not caught up. So a
  transcript read mid-run is a read of some earlier moment, and for a short run it can be a read
  of nothing. ADR-0040 reads it twice for this reason — once at the checkpoint, on bytes already
  in hand, and once when the leg has ended and the process is gone, which is the only moment the
  file is known to be final — and neither read overwrites a good number with an empty parse,
  because unknown is not none. **The part this does not answer**, and which is worth a walk of its
  own: a checkpoint stores that same lagging transcript, so a checkpoint taken at an early
  boundary carries one missing the most recent turns. What a resume from it does was not measured.

- **"Nothing is waiting for that run" was also what a mistyped `tool_use_id` got.**

  `Asks::answer` takes the run and an optional `tool_use_id` and built one candidate list:

  ```rust
  .filter(|(_, p)| p.question.run == run)
  .filter(|(_, p)| which.is_none_or(|id| p.question.tool_use_id == id))
  ```

  then answered an empty list with `AnswerError::NothingWaiting(run.short())`. Two different facts
  reach that arm. *This run has no question* sends an operator to `offload ps` to find out what
  happened to the run. *This run has a question and you named a different one* sends them to their
  own argument. Only the first sentence was ever printed.

  Measured in session seventy-eight on one daemon with a fake agent blocked in `offloadd ask-hook`,
  which is the staging `docs/DEMO.md` describes — a `[[sinks]]` entry so the question is actually
  *put*, `--ask --permission ask` on the submission, and a `tool_use_id` the walk chooses. The two
  commands, seconds apart:

  ```
  $ offload asks
  01a090d7602e   Bash   5.0s   4m55s   here   rm -rf /important
                 offload approve 01a090d7602e7711a2f3c26a90ff60fe toolu_walk_1

  $ offload approve 01a090d7602e7711a2f3c26a90ff60fe toolu_typo
  Error: nothing on this node is waiting for an answer about run 01a090d7602e
  ```

  The agent was blocked mid-tool-call the whole time and the question had four minutes left on it.
  Believing the error, an operator concludes the question expired or the run moved on — and the
  thing that would have corrected them is the line `offload asks` had just printed.

  Asked in two steps now: the run filter alone decides `NothingWaiting`, and only then is the id
  applied. `AnswerError::NoSuchQuestion { run, tool_use_id, waiting }` names what *is* waiting:

  ```
  Error: run 01a090d7602e is waiting for an answer, but not about `toolu_typo`
         — it is waiting on `toolu_walk_1`
  ```

  which is `reports-and-cli`'s rule — an error that tells somebody to try harder must be
  satisfiable from the screen — met from the same side session seventy-six met it on `offload
  revoke`. `offload deny` goes through the same `Asks::answer` and was wrong in identical words;
  session seventy walked `deny` and found it sound, because a walk that supplies the right argument
  never sees this.

  The rest of the walk found the mechanism sound: `offload asks` reports tool, waiting time, time
  left, where and what it wants; the instruction line prints the full run id and the
  `tool_use_id`; a **short** run id is accepted too, so seventy-seven's widening was belt-and-braces
  rather than load-bearing; the sink received both the question and the completion; and approving
  unblocked the agent, which finished its two turns.

  One staging note worth carrying: the walk's first "verification" ran against the **old binary**,
  because `kill $(cat pidfile)` used a pid written by a relaunch that had failed to bind the
  already-held socket. The old daemon, its agent and its blocked `ask-hook` were all still up, and
  the fix appeared not to work. `pgrep -a offloadd` before believing a before/after.
- **The agent keeps auto-memory outside the worktree, and nothing carries it.** Found by ADR-0064's
  fourth check (session eighty-eight), which set out to prove a *finished* session resumes with
  `--fork-session` from a different directory and appeared to on the first try. Leg 1 was told
  *remember this codeword for later: PELICAN-7* and answered `OK` under `--print`; unasked, it also
  wrote `~/.claude-alt/projects/<leg-1 cwd slug>/memory/codeword.md` (frontmatter, `type:
  reference`, `originSessionId` the leg's session). Leg 2, forked from leg 1's transcript in another
  directory, answered correctly — and its only action after the prompt was a `Read` of that memory
  file **by its absolute path**, which the resumed conversation remembered writing. So the answer
  came from a node-local file, not from the conversation: on a second machine that path is absent
  and the same resume answers from nothing. With the memory folder deleted, a fresh fork answered in
  one turn with no tool call, which is the result the check was for. Nothing in `offload-agent`
  mentions memory, so spawns leave it on; none of the ~1,000 worktree project folders from earlier
  walks has a `memory/` folder, because none of those prompts asked for anything to be remembered.
  The lesson for a walk is the general one: **read what the resumed leg did, not only what it said.**
- **…and the directory is keyed by the repository, which makes it a leak between runs — measured.**
  The lead (session eighty-nine) was an empty `<config>/projects/<mirror slug>/memory/` beside real
  runs. Claude Code's memory documentation settles the key — *"derived from the git repository, so
  all worktrees and subdirectories within the same repo share one auto memory directory"*, and
  *"machine-local"* — and session ninety measured it on `claude 2.1.281` with Haiku ($0.093): a
  bare mirror with worktrees A and B; in A, *save to your memory that the codeword is HERON-3*
  (saved under the mirror's slug, 4 turns); in B, a fresh session asked for the codeword from what
  it knows with no tools → `HERON-3`, one turn, no tool call; the same with
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` → `UNKNOWN`; and a save request with the variable set wrote
  `memory.md` **into the worktree** and created no memory directory. ADR-0065 turns it off for every
  spawn in `claude::child_env`, applied after the request's own environment so nothing re-enables
  it, and a walk with the fake agent recording the variable confirmed the daemon's spawn carries it.
  Why it sat a session as a lead: every earlier check asked the worktree that had written the file.


*Backfilled in session ninety-one from `docs/sessions.md` and the commit that added each rule —
sourced, not reconstructed from the rule text.*

- **A raw transcript can be searched and not read.**

  Session eighty-nine, phase 10's demo with Haiku (commit `dde26f8`). The continuation that needed
  the parent's transcript could not `Read` it — one line was 38k tokens, over the tool's 25k cap,
  because the agent's own system and attachment rows are single lines — and ran out of turns
  grepping. One sentence in the heading saying *search it* took it to the right answer in three
  turns; the control continuation never opened the file, which was the demo. ADR-0064 is amended
  with the heading's wording.
