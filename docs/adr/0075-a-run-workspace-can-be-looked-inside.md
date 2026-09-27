# ADR-0075: A run's workspace can be looked inside, read-only

**Status:** accepted · 2026-09-27 · session ninety-two · wire v34

## Context

The owner, on the phone: "I'm thinking if I could peek into a run's workspace, perhaps view a
file". What an agent wrote is in its workspace, and the workspace is a checkout on the one machine
that ran it: usually the laptop, never the phone asking. `offload logs` already fetched a remote run's
log from the node that ran it (`FetchEvents`); nothing could show a file.

## Decision

1. **Read-only, on the machine that has it.** `FetchFiles { run, path }` goes to the node `offload
   logs` would ask (`log_source`), unless the asking node has the checkout or the branch itself. The
   answer is `Files { view }` or `NoFiles { reason }` in words. New messages an older node would not
   understand, so **wire v34** and `MIN_VERSION` with it, as for v31.
2. **The checkout while it is there, the branch after.** `WorkspaceManager::peek` reads the
   checkout, which has uncommitted work. Once the checkout sweep has removed it, it reads the run's
   branch in the mirror with `git ls-tree` and `git cat-file`, which has what was committed. The view
   says which (`FilesSource`) and which node read it, and the clients say so in words.
3. **Held to the workspace.** Paths are relative to the root. `..`, absolute paths and `.git` are
   refused before anything is read, and a path is canonicalized and must stay under the canonical
   checkout, so a symlink the agent made that points outside is refused too. Directories list up to
   500 entries, directories first. Files are cut at 256 KiB, and one with a NUL in its first 8 KiB
   (git's own test) is shown as its size.
4. **Who may ask:** any member, as for a run's log. The files are the same work the log describes,
   and a member already sees the log. A fleet that is one owner's devices has no finer line to draw
   yet.

## Clients

`offload files <run> [path]` lists a directory or prints a file, to stdout so it can be piped, with
where it came from on stderr. The app's run sheet has **Files**: a browser with Up, a file's text
selectable, and a header naming the machine and whether it is the checkout or committed files only.

## Walked

On the laptop's stub node and the emulator's app:
- `offload files` on the laptop listed a run's checkout in `~/offload` and printed a file;
- `../../etc` and `.git/config` were refused in words;
- from the emulator's node, the same run's files were read over the fleet, "on laptop, from its
  checkout";
- the app's browser listed the root, opened the file, and went back up.

`a_workspace_is_shown_read_only_and_held_to_its_root` covers a symlink out, a binary file, `..`,
`.git`, and the branch fallback after the checkout is removed.

## Consequences

Every node has to be on v34. The Mac's `mac-peer` (v33 at the time) is refused until it is updated.
