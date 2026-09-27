# The agent adapter: argv, grants, MCP, permissions

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/agent-adapter.md`, same order.

- **An allowlist entry can quietly be a shell, and the check must judge the *program*, not the
  text.** `Bash(sh:*)` looks scoped and isn't; nor are `python`, `xargs`, `env`, `find`, nor four
  spellings that walk past a name list — a **path** (`/bin/sh`), a **version suffix**
  (`python3.11`), a **quote** (`"sh"`), an **environment assignment** (`FOO=1 sh`).
  `program_named` normalises; over-matching is the safe direction.
- **A positional argument needs an option terminator, and the prompt is one.** The prompt goes
  last, behind `--`, so `--allowedTools` is the last *flag* rather than the last word.
- **`--print` ends at the first turn making no tool call, so the resume nudge decides whether a
  migrated run finishes.** `CONTINUE_PROMPT` names the task and asks for completion, and rules out
  the one-step reading — `Completed` is terminal, so an abandoned run looks like a success.
- **The agent decomposes a compound command, which is why prefix grants are sound at all.** Under
  `Bash(cargo test:*)`, `cargo test --version && cargo build --version` is refused naming the
  uncovered half. Load-bearing: this is why `risk` is a list of programs rather than a parser.
- **The agent's state directory is movable, and two things break silently when it moves.**
  `$CLAUDE_CONFIG_DIR` relocates transcripts *and* `.credentials.json`. Read once per crate; path
  functions take a config directory, never a home.
- **A permission hook fires for every tool call and never says whether permission was needed.** So
  the harness decides what to ask about — and *nobody answered* must print **nothing**, never
  `deny`, so the channel's failure mode is identical to not having it. A grant suppresses the
  question: a match means "let the agent decide", never "allow".
- **What is worth asking about depends on the mode**, so `ask_tools(mode)`, not a constant. Under
  `AcceptEdits` a `Write` matcher asks about something the agent allows itself; under `Ask` its
  absence denies every edit in silence.
- **A channel that can ask about everything has to be able to stop.** `--ask=N` budgets the
  *interruption*. Running out is **not a denial** — it is the behaviour already shipped. Spent when
  a question is *put* to a person, and counted on `RunProgress` so a migration continues the count.
- **…and a flag whose promise the fleet cannot keep says so at the keyboard.** Both ends read one
  predicate (`deliver::can_reach_a_person`), so the submission and the blocked agent cannot
  disagree. A configured route counts before it is known to work.
- **Credentials do not migrate.** Auth is a node capability, never payload in an assignment. A
  resource held by another node is reached by *proxying the call*, never by fetching its credential.
- **An MCP config without `--strict-mcp-config` is ambient authority.** The run inherits the host
  user's servers and whatever `.mcp.json` the repo ships. Unconditional on every argv, asserted for
  resumed and `Full` requests too — a resumed run that lost it is the more dangerous one.
- **A command line is world-readable, so a credential never goes on one.** `--mcp-config` gets a
  `0600` file the node writes and removes; `--settings` stays inline because it carries no secret.
  Set the mode explicitly — truncating an existing file keeps whatever mode it had.
- **A grant that hands over a tool and not permission to call it grants nothing.** The config and
  the `mcp__<id>` allowlist pattern come out of *one* function, resolved **on the holder**.
- **The agent's cost and its token counts come from different places with opposite merge rules.**
  A `result` event's `total_cost_usd` is **that leg's alone** and *adds* (measured across a
  `--resume`: $0.026998 then $0.006026) — which is why `saturating_add` per leg is right. A
  transcript total is **cumulative** and *replaces*. Mixing them doubles or erases (ADR-0040).
- **Never sum `usage` off the live stream.** The streamed assistant `usage` is a partial snapshot
  with `stop_reason: null` — measured `output_tokens` of 1, 2, 4 where the finals were 236, 85, 42.
  The transcript carries the final numbers; the stream does not.
- **One assistant message is several events.** One per content block, sharing a `message.id` and a
  `usage` object, so anything counting messages or tokens must deduplicate by that id — 726 tokens
  against a true 363. Deduplicated, the transcript reconciles *exactly* with the sum of every leg's
  result on all four counters.
- **The transcript lags the event stream, by an amount the agent does not promise.** A checkpoint
  at the turn-1 boundary captured 17KB of transcript with **no assistant rows in it** — only
  `queue-operation`, `user`, `attachment`, `atis-latch`, `ai-title` — while the same file held
  three afterwards. Anything reading the transcript mid-run is reading a turn or so ago, or
  nothing. Read again once the process is gone, and never overwrite a good number with an empty
  parse (ADR-0040's amendment). It is a **race, not a fixed lag** — the same turn 1 captured
  22,953 bytes one run and 17,087 the next — and a capture that lands on the empty side is
  refused rather than stored, because resuming from it loses the agent's half of the conversation.
- **"Nothing is waiting for that run" was also what a mistyped `tool_use_id` got.** `Asks::answer`
  filtered on the run **and** the id before asking whether anything matched, so one `(None, _)` arm
  covered two facts an operator acts on differently: *go and look at the run* versus *look at your
  own argument*. Measured against a live blocked agent — `offload asks` printed the question on the
  line above while `offload approve <run> toolu_typo` said nothing was waiting for it. Asked in two
  steps now, and `AnswerError::NoSuchQuestion` **names the ids that are waiting**, because an error
  telling somebody to try again has to be satisfiable from the screen it is printed on. `offload
  deny` shares the path and was wrong in the same words.
- **The agent keeps auto-memory outside the worktree, and nothing carries it.** Told to *remember*
  something, it writes `<config>/projects/<cwd slug>/memory/*.md` even under `--print` — node-local,
  keyed by the worktree path, in neither the checkpoint nor the transcript's bytes. Measured: a
  forked resume "remembered" the codeword by reading the parent leg's memory file **by absolute
  path**, which on another node does not exist. Before trusting a resume walk, check what the
  resumed leg *read*, not only what it answered. Offload turns memory **off** for every spawn now
  (`CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, applied last in `child_env` — ADR-0065); a walk that runs
  `claude` by hand does not get that, and needs the variable itself.
- **A raw transcript can be searched and not read.** One line of a 183 KB two-turn transcript was
  38k tokens, over the `Read` tool's 25k cap — the agent's own system and attachment rows are
  single lines. A continuation told only "consult it" opened it, was refused, and spent its turns
  grepping (ADR-0064's amendment). Anything handing an agent its own transcript format has to say
  what it is — JSON Lines, search it — or budget turns for the agent to find out.
- **…and the auto-memory directory is keyed by the repository, not the worktree — measured.** A
  run in one worktree of a mirror saved a codeword; a fresh run in a **second** worktree of the same
  mirror answered it in one turn, no tool call (ADR-0065). Every run of one repository on one node
  shared one memory until spawns turned it off. The lead sat for a session as "empty directories
  only" because every walk asked the *same* worktree; the test that finds a cross-run leak asks a
  different run. Repeat it after an agent upgrade — other machine-local state may appear.
