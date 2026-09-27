# ADR-0074: Config in `~/.config/offload`, checkouts in `~/offload`

**Status:** accepted · 2026-09-26 · session ninety-two

## Context

The owner: "I want the offload workspace to be somewhere I can read, like `~/offload`, by default
anyway, and can be configured", and then "separate config from workspace: config can be in
`~/.config/offload` and workspace in `~/offload`". Until now, with nothing set, `offloadd` read no
config file (only `--config`) and kept everything in `~/.offload`: identity, database, mirrors and
the run checkouts. That is the one directory a person wants to open, hidden among the ones they
should not touch.

## Decision

| What | Default | Override |
| --- | --- | --- |
| Config | `$XDG_CONFIG_HOME/offload/node.toml`, else `~/.config/offload/node.toml`, read if it exists | `--config` |
| Run checkouts | `~/offload/<run id>` | `[workspace] dir` (`~/` expands) |
| State (identity, database, mirrors, blobs, turn markers) | `~/.offload`, **unchanged** | `OFFLOAD_STATE_DIR`, `--state-dir` |

1. **State does not move.** It holds the node's identity, and moving it would make every existing
   node a new one.
2. **`~/offload` is filled in by `offloadd` at start only** (`Config::with_readable_checkouts`),
   never by `Config::default()`. Every test builds a `Config`, and a default applied there would put
   their checkouts in their author's home. Not on Android, where `~` is the app's private directory.
   The iOS host never runs `offloadd`'s main.
3. **Nothing is moved.** A checkout made before this stays at `<state_dir>/worktrees/<run>`.
   `WorkspaceManager::worktree_path` finds it there, and `checkouts()` lists both places, so it is
   adopted, checkpointed and reclaimed where it is. Turn markers stay in the state directory, so
   `~/offload` holds only checkouts and rescued (superseded) ones beside them.
4. **A shared directory has one owner.** The checkout sweep reclaims any checkout in its directory
   that no run of this node's needs. That is right for a directory of its own and destructive for a
   shared one: two daemons each read the other's live checkouts as abandoned, and `~/offload` is
   shared by default. So a directory outside the state dir is claimed with `.offload-node` (the node
   id, created exclusively), and another node refuses to start there, naming the owner and
   `[workspace] dir`. Under the state directory the existing lock already means one daemon.
5. **The local commands read the same file.** `offload probe`, `policy` and `match` default to the
   daemon's config file (`config::config_path`), because they report what `offloadd` would do.

## Walked

On the laptop, with neither directory present before:
- the stub walk node (`/tmp/mw`, the one phone's runs land on) logged `run checkouts go here
  checkouts=/home/owner/offload`, and a scratch run's checkout appeared as
  `~/offload/01a0df08e97b…`;
- `/tmp/hw` and `/tmp/hw2` pin `[workspace] dir` to their state directories and logged that;
- a throwaway node with no `[workspace]` was refused: "/home/owner/offload holds the checkouts of
  another node (aaaa11112222) — two nodes sharing it would each remove the other's as abandoned.
  Set `[workspace] dir` in this node's config to a directory of its own."

**Walk daemons on one machine need a `[workspace] dir` each**, now in `docs/DEMO.md`.
