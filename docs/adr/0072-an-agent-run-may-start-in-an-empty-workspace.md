# ADR-0072: An agent run may start in an empty workspace

**Status:** accepted · 2026-09-26 · session ninety-two · builds on ADR-0061 (archive workspaces)

## Context

The owner submitted an agent run from the phone app with a simple prompt and no repository. The
app sent `repo = ""` and the daemon accepted it. The laptop took the run and failed it a minute
later at `git clone --bare --quiet '' …` ("The empty string is not a valid path"). The owner's
verdict on the dialog: they will "99% of the time not have a repository to enter there".

An agent run needs a workspace: the workspace is what checkpoints and migrates. What does not
follow is that the workspace has to come from somewhere. ADR-0061 already makes a repository out of
bytes that are not one, deterministically, so that every node builds the same base commit.

## Decision

1. **`scratch:` names an empty workspace** (`offload_core::SCRATCH`, `RepoSource::Scratch`). The
   whole string, spelled like `archive:`, so a node too old to know it refuses it as a path that is
   not here rather than misreading it. Fleetwide, and `Obtainable` everywhere: any node can make an
   empty repository. No wire bump, for the same reason `archive:` needed none.
2. **The holder makes it** the way ADR-0061 §2 makes an archive with no `.git` into a repository:
   `git init` with the pinned branch, one empty commit with fixed identity, dates and message ("an
   empty workspace"), bare-cloned into the node's mirror as `repos/scratch.git`. The commit is the
   same on every node (`a869ccfe…`, pinned by
   `a_scratch_workspace_is_the_same_empty_commit_everywhere_and_prepares`), so a scratch run
   migrates like any other. One mirror per node, a branch and a worktree per run. It is never
   refreshed, since there is no origin.
3. **An agent run naming no workspace is refused at submission**
   (`SpecProblem::NoWorkspace`, "an agent run needs a workspace: a repository, an archive
   (`--archive`), or an empty one (`--scratch`)"). The same goes for a scratch workspace with a
   `--ref` (`ScratchWithRef`). Both are in `RunSpec::check`, which every submission and resume
   passes through.
4. **Clients:** `offload run --scratch`. The app's repository field is optional, and an empty one
   means scratch. The mapping is in `offload-mobile::submit_agent`, not in Kotlin. The run log says
   "submitted   an empty workspace", not the spelling.

## Consequences

- What the agent writes comes back as commits on the run's branch in the holder's scratch mirror,
  as for an archive. There is no origin to push to, and nothing is ever pushed (ADR-0008).
- A node on an older build declines to bid on a scratch run ("not here"). Hosts have to be updated
  before a phone's repository-less runs can land on them.

## Walked

On the laptop's walk node (the stub agent), from the CLI and from the product app on the emulator:
- `offload run --scratch` ran on `branch offload/run-… from a869ccfe` and completed;
- `--repo ""` was refused with `NoWorkspace` before anything was placed;
- from the app, a prompt with no repository was accepted by the laptop, ran in an empty workspace,
  and completed.

The app also **remembers repositories** now (the owner: "it is not trivial to type that stuff
out"): URLs of accepted submissions, most recent first, up to eight, shown as chips under the
field (`Remembered.kt`, on the device only), plus the last model used. Agent run is the dialog's
default, with the prompt first.

**On the phone, by the owner** (20:3x): "Write a short story about a horse" with no repository, the
run that failed at `git clone ''` earlier. It was taken by the laptop, started `from a869ccfe`, and
completed. Its finish was delivered to the phone's routes only (ADR-0073), and the owner confirmed
the notification arrived.
