# Offload

<p align="center">
  <img src="assets/offload-logo.svg" alt="Offload logo" width="180">
</p>

Orchestration for **coding agents** across a personal device fleet.

You submit an agent run — a repo, a prompt, an agent — and the fleet negotiates over who takes
it. Devices (phones, laptops, desktops, VMs) join and leave freely. When a device leaves, its
in-flight runs **migrate**: the session transcript and the workspace move to another capable
device and the agent resumes mid-conversation.

Close the laptop, the agent keeps working on the desktop.

---

## This is a vibe-coded project

Up front, because it changes how you should read everything below: **this codebase was written
by an AI agent** (Claude Code), across seventeen working sessions, with a human directing rather
than typing. That is the honest description, and there are two sides to it worth stating.

The uncomfortable side: nobody has read every line. There is no second pair of human eyes on
most of it, no production deployment, no users but its author, and no security review. Do not
put anything you care about behind it yet.

The other side, which is why the project exists at all: an agent that writes a distributed system
this fast will also confidently write one that loses your work. So the discipline is aimed
squarely at that — decisions are written down as ADRs *before* they are built, every rule that
can be mechanically enforced is (clock injection, allowlist scoping, migration digests, one
daemon per state directory), and the last three sessions have done nothing but aim property tests
at surfaces the code makes claims about. Those found sixteen real bugs so far, most of them work
the fleet was silently throwing away: two agents able to run on one repository, a blob collector
deleting the checkpoints of live runs, a run resumed from a checkout nineteen turns out of date,
`offload rm` telling the whole fleet it had deleted a worktree on a machine it had never touched,
and — on the laptop this is written on — a battery probe reporting the *touchscreen's* battery as
the machine's, 0% while the real one sat at 98%.

Every one of those was found by writing down what the tree *claimed* and then testing the
sentence. That method is the actual subject of this repository, more than the scheduler is.

---

## Status

**Not released, and not packaged.** Build it from source. There are no prebuilt binaries, no
installer, no crates.io release, and no stability promise about the wire protocol or the on-disk
schema — the wire is at v20 and every version bump so far has meant "upgrade every node".

What is built and verified end to end:

- **Phases 0–4** — the domain model, a single-node agent runner, checkpoint and resume, a real
  mesh (QUIC, ed25519 identities, gossip, SWIM failure detection, mDNS discovery), and runs that
  **move between machines mid-conversation**.
- **Phase 5, most of it** — enrolment by invitation, revocation that travels, certificate
  renewal, `offload rekey`, and the delivery plane: a run finishing on a machine with no way to
  reach a person, reported by a phone that hosts nothing.
- **Phase 6, in progress** — property tests, and the bugs they keep finding.

What is not built: NAT traversal and relays (so it is a LAN today), the mobile host process,
approver keys in secure hardware, `turmoil` simulation, and metrics.

`docs/ROADMAP.md` says what is not built yet and what is still to decide; `docs/phases.md` says what
each completed phase shipped. `docs/HANDOFF.md` says where the last session stopped, and
`docs/sessions.md` what each one got wrong on the way.

## Try it

Rust stable (see `rust-toolchain.toml`; the workspace's MSRV is 1.85), and
[Claude Code](https://claude.com/claude-code) on `PATH` for anything that runs an agent.

```bash
cargo build --workspace
cargo test --workspace          # 713 tests
```

Local questions need no daemon at all:

```bash
cargo run -p offload-cli -- probe                 # what is this device?
cargo run -p offload-cli -- policy                # would it take work right now?
cargo run -p offload-cli -- match "cores>=8"      # …and does it qualify, clause by clause
```

A real run, on one machine. `cargo build` puts `offload` and `offloadd` in `target/debug/`;
put that on your `PATH` or spell the commands `cargo run -p offload-cli -- …`:

```bash
offloadd                                                      # the daemon
offload run --repo ~/dev/foo --follow "add tests for the parser"
offload ps --all                                              # survives a daemon restart
offload logs -f <run>
offload checkpoint <run>                                      # capture at the next turn boundary
offload resume <run> --follow                                 # continue the same conversation
```

A fleet. Membership is a passphrase, not a server — a certificate verifies against the fleet key
alone, so enrolling needs no network:

```bash
offload init                        # found a fleet; prints the passphrase once
offload id                          # on the joining device
offload invite <that id>            # on one that already belongs
offload join --token <token>        # …and take it up
offload nodes                       # who else is out there, and what they are holding
offload drain                       # hand this node's runs to the fleet, mid-conversation
offload explain <run>               # why it is where it is
```

`OFFLOAD_STATE_DIR` relocates everything, which is how you run two nodes on one machine — and how
every multi-node test in `docs/DEMO.md` was actually run.

## How it is put together

Two binaries: `offloadd`, the node daemon, one per device; and `offload`, the operator CLI.

Ten crates, with the dependency direction strictly downward. `offload-core` is the whole
domain model — the run state machine, epoch fencing, leases, bidding, constraints, capacity — and
it has no I/O, no tokio, no networking and **no clock**, which is what makes the simulation tests
possible. There is a test that reads the crate's own source to keep it that way.

The pieces that carry the design:

| | |
|---|---|
| `offload-core` | domain types, and every decision as a pure function of (view, run, now) |
| `offload-agent` | agent adapters — spawn, stream events, turn boundaries, resume |
| `offload-workspace` | per-run git worktrees over a shared per-repo bare mirror |
| `offload-store` | SQLite: run registry, event log, blobs, absence history |
| `offload-cluster` | membership, gossip, failure detection, discovery |
| `offload-node` | the daemon: config, control socket, run supervision |

Read `docs/ARCHITECTURE.md` for the system model, and `docs/adr/` for the decisions and the
reasoning behind them — seventeen of them, each one a thing that was easy to get wrong.

A few of the rules those settle, as a flavour of what the problem actually is:

- **Never reimplement an agent.** Offload spawns Claude Code and speaks its own protocol. If we
  find ourselves parsing model output, we have taken a wrong turn.
- **Mid-turn is not a safe checkpoint.** An agent halfway through a tool call has state in the
  tool, not the transcript.
- **Double execution is the failure mode that matters.** Two agents on one repo, both committing,
  is worse than no agent — so every side-effecting path checks a fencing token, and a run stalled
  beats a run duplicated.
- **Credentials do not migrate.** A run moves to a node that already has its own auth.
- **Decisions carry reasons.** "Why didn't that happen" is nearly every question anyone asks
  here, and a decision that discarded its reasoning cannot answer it.

## Licence

MIT or Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
