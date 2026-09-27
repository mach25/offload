# Commands

`CLAUDE.md` carries the four that run on every change; this is the rest.

```bash
cargo build --workspace
cargo test --workspace
PROPTEST_CASES=20000 cargo test -p offload-core --test properties  # search harder
PROPTEST_CASES=20000 cargo test -p offload-core --test churn       # …and the fleet of them
PROPTEST_CASES=600 cargo test -p offload-cluster --test storm      # …and a fleet of real ones,
                                                                   #   over the wire. Slow: each
                                                                   #   case builds four clusters,
                                                                   #   so the default is 24.
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# Local questions — no daemon needed
cargo run -p offload-cli -- probe [--config node.toml]      # what is this device, and whose
                                                           #   account does it spend on? — and,
                                                           #   with `--config`, the only command
                                                           #   that lists all four nominated
                                                           #   kinds at once, with the reason
                                                           #   beside any it cannot use
cargo run -p offload-cli -- policy [--config node.toml]    # would it take work right now? —
                                                           #   per demand, per tier, and with
                                                           #   the owner's `[policy.light]`
                                                           #   answer where they gave one
                                                           #   (ADR-0019 §4)
cargo run -p offload-cli -- match [--config node.toml] "agent=claude-code, cores>=8, mem>=16G"
#   …all three take the daemon's config for the same reason: without it they answer about
#   `claude` on PATH and the class default, and the daemon uses neither once a config exists.

# Membership — also local, because a certificate verifies against the fleet key alone
cargo run -p offload-cli -- init                            # found a fleet, print the phrase once
cargo run -p offload-cli -- id                              # who is this device, before it belongs
cargo run -p offload-cli -- invite <node> [--grant host-runs]  # …enrol it from a device that does
cargo run -p offload-cli -- join --token <token>            # …and take the invitation up
cargo run -p offload-cli -- join --passphrase               # enrol this device (recovery path)
cargo run -p offload-cli -- fleet                           # what this node knows about its fleet
cargo run -p offload-cli -- nodes --history                 # …and what it saw happen to it
cargo run -p offload-cli -- grant host-runs                 # a deliberate act, prompts for the phrase
cargo run -p offload-cli -- verify / revoke <node>
cargo run -p offload-cli -- rekey [--evict <node>]          # the revocation nothing has to receive

# The daemon and a real run
cargo run -p offload-node                                  # start offloadd
cargo run -p offload-cli -- status                          # …and what this node offers: a
                                                           #   `resource` and a `task` line per
                                                           #   nomination, each saying whether
                                                           #   its program is usable and why not
cargo run -p offload-cli -- run --repo ~/dev/foo --follow "add tests for the parser"
cargo run -p offload-cli -- ps --all
cargo run -p offload-cli -- nodes                          # who else is out there
cargo run -p offload-cli -- models                         # which models the fleet's agents offer,
                                                           #    and who offers each (ADR-0080)
cargo run -p offload-cli -- models --refresh               # every device reads its agent's list again
cargo run -p offload-cli -- explain <run>                  # why it is where it is, right now
cargo run -p offload-cli -- audit [<run>]                  # …and what this machine decided, then
cargo run -p offload-cli -- deadline <run> 2h|none         # I need it by a different time
cargo run -p offload-cli -- priority <run> 10|+10|-5       # and let it go ahead of the others
cargo run -p offload-cli -- logs -f <run>                  # from any node, including a peer's run
cargo run -p offload-cli -- cancel <run>                   # stop it, wherever in the fleet it is
cargo run -p offload-cli -- run --demand heavy ...         # this one should keep a machine
cargo run -p offload-cli -- run --max-turns 10 ...         # …and stop after ten turns whatever
                                                           #    state it is in (ADR-0039); the
                                                           #    count spans migrations, and no
                                                           #    resume gets an eleventh
cargo run -p offload-cli -- run --notify push ...           # …and tell me this way
cargo run -p offload-cli -- run --ask ...                    # stop and ask me rather than be denied
cargo run -p offload-cli -- run --permission ask --ask=5 ... # …about edits too, five times at most
cargo run -p offload-cli -- asks                            # …what is waiting for an answer
cargo run -p offload-cli -- approve <run> [tool-use-id]     # …and answer it (or `deny`)
cargo run -p offload-cli -- run --task webhook              # the cheap tier: a program the
                                                           #    owner nominated, no model, no
                                                           #    prompt, no workspace, no tokens
                                                           #    (ADR-0019). `--arg` passes
                                                           #    arguments after the owner's own.
                                                           #    Needs a [[tasks]] entry in
                                                           #    node.toml; refused at the
                                                           #    keyboard if nobody has one.
                                                           #    A failed one is **run again** —
                                                           #    unattended, up to max_resumes,
                                                           #    from its spec (ADR-0058), which
                                                           #    is a restart and not a resume:
                                                           #    `offload resume` refuses a task,
                                                           #    because there is no conversation
                                                           #    to continue.
cargo run -p offload-cli -- run --prefer here ...           # land on this machine if it can take
                                                           #    it, anywhere otherwise (ADR-0063);
                                                           #    `--require node=bravo` pins it,
                                                           #    `--hold 8h` (implies --queue) makes
                                                           #    the preference a requirement until
                                                           #    then
cargo run -p offload-cli -- continue <run> "now add tests"  # a finished run, continued by a new
                                                           #    one from its branch and its
                                                           #    uncommitted files, handed its
                                                           #    prompt, closing words and
                                                           #    transcript (ADR-0064). Typed on
                                                           #    any node; the one that ran the old
                                                           #    run's last leg must be up.
                                                           #    `--session` forks its conversation
                                                           #    instead
cargo run -p offload-cli -- run --use email ...             # …and let it reach the mailbox,
                                                           #    wherever in the fleet it is
cargo run -p offload-cli -- sinks                          # routes to a human, and their state
cargo run -p offload-cli -- sinks --test                   # …and prove one actually fires
cargo run -p offload-cli -- triggers                       # what this device is watching
cargo run -p offload-cli -- when schedule --repo ~/dev/foo -- "check the nightly build"
cargo run -p offload-cli -- when webhook --task sync --arg --once
cargo run -p offload-cli -- when failed --on-notice -- "look at what broke"
#   …or bind it to a **notice** instead of a trigger (ADR-0057): when a run on this node
#   fails, submit this one and let the fleet place it. A run a *machine* started fires
#   nothing, so an escalation cannot escalate itself.
#   …and a trigger can fire the cheap tier too (ADR-0019). The event is **not** passed to a
#   task — a trigger's line is text from outside and a task runs a command its owner wrote
#   down — where an agent rule appends it to the prompt under a heading saying it is data.
cargo run -p offload-cli -- rules                          # …what it will do, and has done
cargo run -p offload-cli -- every 15m --task webhook        # run something on a clock, fleet-wide
                                                           #    (ADR-0019 §3). Gossiped, so it
                                                           #    outlives the device you typed it
                                                           #    on — that is the difference
                                                           #    between this and cron. Aligned to
                                                           #    the clock; `--at 3h` with
                                                           #    `every 24h` is 03:00 **UTC**;
                                                           #    `--prompt` schedules an agent run
                                                           #    instead. No catch-up.
cargo run -p offload-cli -- schedules                      # …what runs on a clock, and who fires
                                                           #    each one right now
cargo run -p offload-cli -- unschedule <id>                # …and stop, everywhere
cargo run -p offload-cli -- unwatch <rule>                 # …and stop
cargo run -p offload-agent --example smoke                 # adapter alone, no daemon
```

`OFFLOAD_STATE_DIR` relocates everything (mirrors, worktrees, logs, identity, socket), which
is how you run two nodes on one machine. `[agent] config_dir` relocates the *agent's* state, which
is a different question with a different answer: it selects **which account** this device spends on
(ADR-0028), and `[agent] account` is how an owner says which one they meant.

