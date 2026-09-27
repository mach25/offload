# Contributing

This is a small, opinionated repository, and most of the opinions are about *how a change is
justified* rather than about how it is formatted. Read `CLAUDE.md` first — it is the orientation
document and it is written for whoever is doing the work, human or agent. This file is the
shorter thing: the rules a change has to satisfy before it lands.

The project's actual subject is the discipline, not the scheduler. It is a distributed system
written very fast, and the whole defence against that is that decisions are written down before
they are built and that every rule which *can* be mechanically enforced *is*. If you take one
thing from this file, take that.

---

## How to contribute

Most changes here are written by an AI agent with a person directing it, and yours probably will
be too. That is welcome; the rules below are written so an agent can follow them.

1. **Branch from `main`**, one topic per branch. Fork the repository if you cannot push to it.
2. **Point your agent at `CLAUDE.md` first.** It is the orientation document, and it is written
   for whoever is doing the work.
3. **Read before you touch**: `docs/HANDOFF.md`, the phase in `docs/ROADMAP.md`, the ADRs for
   what you are changing, and the pitfall file for it (`CLAUDE.md` has the table).
4. **Before you push**, run the four commands in `CLAUDE.md` (build, test, clippy with
   `-D warnings`, fmt). A PR that fails them is not ready for review.
5. **Open a pull request.** Say what changed and why, the phase and ADR it touches or `none`, the
   test and clippy state, and what a reviewer should check by hand. If you walked it on real
   machines, say what you ran and what it printed. `main` changes only by a merged PR.
6. **Write down what went wrong**, not only the fix: a pitfall entry, an ADR for a decision, a
   paragraph in `docs/sessions.md`. The table under *Where to write things down* says which.
7. **Keep your own setup out of it.** No real addresses, home paths, serials, account or fleet
   ids, device models or client names in tracked files; *Publishing* below says what to write.

Some items in `docs/HANDOFF.md` name the owner's own devices ("check the Mac overnight"). Those
are theirs to walk; leave them unless the owner asks.

## Before you write code

**Read `docs/HANDOFF.md`.** It is the live head: what is true now, and what to pick up. It is
short on purpose, so there is no excuse for skipping it — and skipping it is how a gap that closed
two commits ago gets "fixed" again. When you need the reasoning behind something rather than its
current state, `docs/sessions.md` has it session by session.

**Check the phase.** `docs/ROADMAP.md` says what is not built yet; `docs/phases.md` says what each
completed phase shipped.
`crates/offload-sched` was planned for phase 4 and was never created — the bid round lives in
`offload-cluster::place`, and `CLAUDE.md` says why. Assuming a crate exists is a wasted afternoon.

**Read the ADR.** `docs/adr/` holds the settled decisions *and the reasoning that is easy to
lose*. If your change contradicts one, you are not fixing a bug — you are relitigating a
decision, and that needs a superseding ADR rather than a patch.

---

## Decisions get an ADR

An ADR is required when a change decides something that a future reader would otherwise have to
reverse-engineer:

* a new field that **travels** — gossiped, on the wire, or on a `RunSpec`
* a new axis of control (who is told, what is thrown away, what may be resumed)
* a change to who owns a fact, or how two copies of it are settled
* a decision **not** to build something (ADR-0025 is one, and it is one of the more useful ones)

An ADR is *not* required for a bug fix that restores what an existing ADR already claims. Those
belong in the commit message, at length — see below.

Numbering is sequential (`docs/adr/00NN-short-slug.md`). The shape:

```markdown
# ADR-00NN: One sentence that is the decision, not the topic

**Status:** accepted · YYYY-MM-DD · amends ADR-00XX · supersedes nothing

## Context
What is actually happening, with **numbers you measured** where numbers exist.

## Decision
Numbered, with the rejected alternatives and why each is wrong. "Why this rather than the two
obvious alternatives" is the most valuable section in most of these files.

## Consequences
Including the residuals — what this deliberately does *not* reach, and why that is a decision
rather than an oversight.
```

Three habits that make these worth having:

* **Write down what you measured, not what you reasoned.** Every ADR in here that carries numbers
  earned them on a real daemon. Several ADRs' most important paragraphs exist because a walk
  disagreed with a reading.
* **Name the residual.** A "does not reach" note is a claim with a date on it, and the commit that
  closes a gap is not the commit that will remember to delete the note. Say what is left, and
  expect to have to go back and delete the sentence.
* **Record where the implementation corrected you.** Several ADRs have amendment sections for
  exactly this. Design ahead of code is an accepted mode here (ADR-0019 is accepted and unbuilt);
  pretending the design was right all along is not.

---

## Working agreements

These are in `CLAUDE.md` in full, with the reasoning. The short forms:

* **`offload-core` is synchronous, deterministic, and has no clock.** Every decision — placement,
  bidding, reassignment, recovery — is a pure function of `(view, run, now)`. Take a `Millis`;
  never call `SystemTime::now()`. Enforced by `offload-core/tests/no_clock.rs`, which reads the
  crate's own source, because the rule is about what the code may *call* and no type says so.
* **Dependency direction is strictly downward**: `core` ← `proto` ← everything else.
* **`offload-store`'s API is synchronous.** SQLite's is. Callers on an async task use
  `spawn_blocking`.
* **Migrations are append-only and pinned.** Never edit one that has shipped: a known-answer test
  digests every shipped migration and fails on an edit. Adding one adds a digest.
* **Never reimplement an agent.** `offload-agent` shells out and speaks the agent's own protocol.
  If you find yourself parsing model output or managing conversation state, stop.
* **Decisions carry reasons.** Return `Hold { until, reason }` and `NoBid::Refused(..)`, not
  `bool` and `None`. Almost every user-facing question here is "why didn't that happen".
* **Every new gossiped field needs an owner.** Write down who is authoritative and what arbitrates
  it (ADR-0005's table). Sometimes the answer is that it should not be a gossiped field.
* **No `unwrap`/`expect` outside tests and `main`.** `thiserror` in libraries, `anyhow` in
  binaries. Enforced as workspace lints with `-D warnings`. Note where that does not reach: a
  `#[cfg(test)]` module inside a lib is exempt, an integration test under `tests/` is not.
* **Tracing, not `println!`.** Structured fields (`run_id = %id`). Agent stdout is data, not
  logging — it belongs in the run's event stream.
* **A report has to say what the decision says.** If a command exists to tell somebody what the
  system will do, the value it prints must come from the same place the system reads it. Several
  bugs here were a command computing a default while the daemon used a configured value.

### Two rules that have each cost a night

* **Never over-claim in a probe.** An unverifiable capability is `false`. A node that claims what
  it lacks wins bids and then fails every run it takes, which is much worse than idling.
* **Prefer a run stalled to a run duplicated.** Double execution is the failure mode this design
  is organised around. Every side-effecting path checks the epoch, and a guard separated from the
  thing it guards by an `await` is on the wrong side of it.

---

## Tests

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Search harder when you have touched the domain logic:

```bash
PROPTEST_CASES=20000 cargo test -p offload-core --test properties
PROPTEST_CASES=20000 cargo test -p offload-core --test churn
PROPTEST_CASES=600  cargo test -p offload-cluster --test storm   # slow: real clusters over the wire
```

What each kind of test is for, because they are not interchangeable:

* **A unit test checks a case. A property test checks a sentence.** "No run is lost across
  transitions" and "fencing rejects the second writer" are claims about *every* sequence and
  *every* pair, and nothing that arranges a case was ever going to check them.
* **Aim a property at what the tree claims**, not at what might break. Everything the properties
  have found so far was a rule written down in one place and honoured only there.
* **Revert each fix and watch its test go red.** A test can pass for the wrong reason. This is not
  optional here; it is the step that has caught the most self-deception.
* **Build fixtures through the writer the daemon uses.** A hand-written `INSERT` can agree with
  the code about a world neither lives in — one did, for two phases, while the real path deleted
  live runs' checkpoints.
* **A simulation that breaks the rule it is modelling reports the product as broken.** If a
  counterexample needs a step the daemon has no code path for, the harness is the bug.

---

## Walking it

A great deal of what has been found here was found by running two daemons and reading the output,
not by reading the code. `docs/DEMO.md` has everything that costs twenty minutes if you meet it
cold. Highlights:

* **A fake agent is the cheapest way to demonstrate anything the model would decide.** Point
  `agent.binary` at a shell script that prints `--output-format stream-json` lines. It must answer
  `--version`, and if it needs to checkpoint it has to write a transcript under
  `$CLAUDE_CONFIG_DIR/projects/*/<session-id>.jsonl`.
* **Default to the cheap model** when a real agent is needed (`--model claude-haiku-4-5`). What
  these walks test is *offload's* machinery, and none of it depends on the agent's reasoning.
* **`OFFLOAD_STATE_DIR` relocates everything**, which is how you run two nodes on one machine. Set
  `socket` explicitly and keep it short — a unix socket path has a length limit.
* **`pkill -x offloadd`**, never `pkill -f`: the pattern matches the shell that typed it.

---

## Publishing

`main` is the public history; it changes only by a merged pull request. Tracked files must not
carry a device owner's own setup: no real network addresses, home paths or usernames, device
serials, account fingerprints, fleet or node ids, device models, or client project names. Keep
those in a gitignored `local/` directory. A maintainer's local hooks check every commit and every
push against that directory's list of real values.

Write documentation ranges (`192.0.2.x`, `2001:db8::`), `/home/owner`, `phone-app`, `tablet`,
`PHONESERIAL`, "the phone", "a Samsung phone". Test fixtures use `example.com` and obviously fake
ids.

**Screenshots** come from a demo fleet, never the owner's: a throwaway daemon with a stand-in agent
and the Android emulator, its status bar frozen with SystemUI demo mode. Found it with
`offload init --name …`: without `--name`, the node's certificate is named after the machine's
hostname, and that name is on every screen that lists devices.

## Commits

Work goes on **a branch, merged by pull request** (*How to contribute*, above). Nobody commits to
`main` directly, the owner included.

**One commit per fact.** If a commit does two things, split the working tree rather than write a
message with "and" in it. Sessions here routinely produce five or six commits that share nothing
but the walk that turned them up.

**The message is where the reasoning lives.** Subject line is a sentence that says what was wrong,
not a category (`fix: notifications`). The body should answer:

* what the observed behaviour was, with numbers
* why it happened, in terms of the mechanism
* why the fix is the right shape, and what else was considered
* **why it shipped** — which guard was missing, or which guard passed for the wrong reason

End the message with:

```
Co-Authored-By: <the agent and model> <its address>
```

…if an agent wrote it, which is the honest description for most of this repository. Claude Code
writes `Co-Authored-By: Claude … <noreply@anthropic.com>`.

**A schema or wire change says so loudly.** A wire bump means every node upgrades. A change to any
signed message means **every device re-joins**, and there is no migration for a signature.

---

## Where to write things down

| Thing | Goes in |
| --- | --- |
| A decision, and why the alternatives are wrong | `docs/adr/00NN-*.md` |
| What is true now, and what to pick up next | `docs/HANDOFF.md` (replace, don't append) |
| What this session did, and why | a paragraph at the top of `docs/sessions.md` |
| That a thing is not built yet, or an open question | `docs/ROADMAP.md` |
| What a phase shipped, once it is done | `docs/phases.md` |
| The system model | `docs/ARCHITECTURE.md` |
| A rule the next person will otherwise break | `docs/pitfalls/<subject>.md`, indexed from `CLAUDE.md` |
| Why *this change* is right | the commit message |

That last table row is the one people skip. `docs/pitfalls/` is the most valuable reading in the
repository, and every entry in it is there because somebody wrote down what went wrong instead of
quietly fixing it. Add the *rule* to the file whose subject the mistake belongs to and the full
entry — mechanism, measurement, how you found it — to `docs/pitfalls/detail/` at the same position;
the two files are read in step. A new subject file needs a row in `CLAUDE.md`'s index too.
