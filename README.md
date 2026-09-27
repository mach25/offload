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
by an AI agent** (Claude Code), across more than ninety working sessions, with a human directing
rather than typing. That is the honest description, and there are two sides to it worth stating.

The uncomfortable side: nobody has read every line. There is no second pair of human eyes on
most of it, no production deployment, no users but its author, and no security review. Do not
put anything you care about behind it yet.

The other side, which is why the project exists at all: an agent that writes a distributed system
this fast will also confidently write one that loses your work. So the discipline is aimed
squarely at that. Decisions are written down as ADRs *before* they are built. Every rule that can
be mechanically enforced is (clock injection, allowlist scoping, migration digests, one daemon per
state directory). And every claim the tree makes is walked on real machines: two daemons, a Mac,
an Android phone and tablet, read line by line. That has found a long list of real bugs, most of
them work the fleet was silently throwing away: two agents able to run on one repository, a blob
collector deleting the checkpoints of live runs, a run resumed from a checkout nineteen turns out
of date, a finished run that a device coming back online could quietly reopen elsewhere, a
phone's notifications given up on in ten seconds while it was merely asleep, and a battery probe
reporting the *touchscreen's* battery as the machine's, 0% while the real one sat at 98%.

Each of those mistakes is written down in `docs/pitfalls/`, now several hundred rules long,
beside how it was found. That method, writing down what the tree *claims* and then testing the
sentence, is the actual subject of this repository, more than the scheduler is.

---

## Status

**Not released, and not packaged.** Build it from source. There are no prebuilt binaries, no
installer, no crates.io release, and no stability promise about the wire protocol or the on-disk
schema. The wire is at v37, and every version bump so far has meant "upgrade every node".

What is built and walked end to end:

- **Runs that move between machines mid-conversation** (phases 0–4): the domain model, a
  single-node agent runner, checkpoint and resume, a real mesh (QUIC, ed25519 identities, gossip,
  an adaptive SWIM failure detector, mDNS discovery), and placement by bidding.
- **A fleet you can own** (phase 5, most of it): enrolment by invitation, revocation that
  travels, certificate renewal, `offload rekey`, approvers (including a key held in an Android
  device's secure hardware), and the delivery plane, which gets a run's news to a person on
  whichever device can reach them. Walked off the LAN, over IPv6 and mobile data.
- **Work that is not an agent run** (phase 8): *tasks*, programs a device's owner nominates,
  placed by the same bid round; *rules* that fire runs when something happens; *schedules* that
  outlive the device that wrote them.
- **Workspaces that are not repositories** (phase 9), and **work you dispatch and come back to**
  (phase 10): `offload continue`, placement preferences and holds.
- **Apps and hosts**: an Android app that hosts a node, submits agent runs and programs, lists the
  models the fleet's agents offer, answers an agent's questions and shows what each run did and
  where. macOS runs the daemon as a launchd service and keeps the machine awake while it may take
  work. iOS has a host library; its app is not started.

What is not built: relays and hole punching (so off the LAN needs a routable address or a node
that can be dialled), push notifications (a phone hears its news when the app is opened), the
iOS app, metrics, and any packaging. `docs/ROADMAP.md` is the authority on what is left and what
is still to decide; `docs/phases.md` says what each completed phase shipped. `docs/HANDOFF.md`
says where the last session stopped, and `docs/sessions.md` what each one got wrong on the way.

## Try it

Rust stable (see `rust-toolchain.toml`; the workspace's MSRV is 1.85), and
[Claude Code](https://claude.com/claude-code) for anything that runs an agent.

```bash
cargo build --workspace
cargo test --workspace          # over a thousand tests
```

Local questions need no daemon at all:

```bash
cargo run -p offload-cli -- probe                 # what is this device?
cargo run -p offload-cli -- policy                # would it take work right now?
cargo run -p offload-cli -- match "cores>=8"      # …and does it qualify, clause by clause
```

A real run, on one machine. `cargo build` puts `offload` and `offloadd` in `target/debug/`;
put that on your `PATH` or spell the commands `cargo run -p offload-cli -- …`. The daemon reads
`~/.config/offload/node.toml` if there is one and keeps its state in `~/.offload`:

```bash
offloadd                                                      # the daemon
offload run --repo ~/dev/foo --follow "add tests for the parser"
offload ps --all                                              # survives a daemon restart
offload logs -f <run>
offload checkpoint <run>                                      # capture at the next turn boundary
offload resume <run> --follow                                 # continue the same conversation
offload continue <run> "now add a changelog entry"            # a new run that picks up from it
offload models                                                # what the agents here offer
```

A fleet. Membership is a passphrase, not a server: a certificate verifies against the fleet key
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

Programs instead of agents: nominate one in a device's `node.toml` as a `[[tasks]]` entry, then
`offload run --task <name>` from any device, `offload when` to fire it on an event, or
`offload every` to put it on a clock.

`OFFLOAD_STATE_DIR` relocates everything, which is how you run two nodes on one machine, and how
most of the multi-node walks in `docs/DEMO.md` were run. The Android app builds with
`scripts/build-android-product.sh`; `scripts/install-macos-service.sh` installs the daemon as a
launchd service on a Mac.

## How it is put together

Two binaries: `offloadd`, the node daemon, one per device; and `offload`, the operator CLI.

Thirteen crates, with the dependency direction strictly downward. `offload-core` is the whole
domain model — the run state machine, epoch fencing, leases, bidding, constraints, capacity — and
it has no I/O, no tokio, no networking and **no clock**, which is what makes the simulation tests
possible. There is a test that reads the crate's own source to keep it that way.

| | |
|---|---|
| `offload-core` | domain types, and every decision as a pure function of (view, run, now) |
| `offload-agent` | agent adapters: spawn, stream events, turn boundaries, resume, the agent's model list |
| `offload-workspace` | per-run git worktrees over a shared per-repo bare mirror |
| `offload-store` | SQLite: run registry, event log, blobs, absence history, audit log |
| `offload-proto`, `offload-transport` | wire messages and version negotiation; QUIC and in-memory transports |
| `offload-cluster` | membership, gossip, failure detection, discovery, and the bid round |
| `offload-node` | the daemon: config, control socket, run supervision, delivery |
| `offload-probe`, `offload-power` | what a device is and has; keeping a host awake |
| `offload-cli`, `offload-mobile`, `offload-ios` | the CLI; the apps' client; the iOS host library |

Read `docs/ARCHITECTURE.md` for the system model, and `docs/adr/` for the decisions and the
reasoning behind them: eighty of them, each one a thing that was easy to get wrong.

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
