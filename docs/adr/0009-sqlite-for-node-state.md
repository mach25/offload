# ADR-0009: SQLite for node state

**Status:** accepted · 2026-07-25 · supersedes the `redb` choice sketched in the roadmap

## Context

Phase 1 keeps the run registry in memory and appends events to per-run JSONL files. That
was right for one machine with no failover, and its limits are already visible: restarting
`offloadd` forgets every run it was tracking, even though the events are sitting on disk.

Phase 2 needs durable state for:

- the run registry — spec, state, epoch, lease, checkpoint reference, attempt count
- run events, currently JSONL
- the content-addressed blob store's index — hash → local path, size, refcount
- per-node absence history, which the drop-off policy (ADR-0007) depends on and which
  currently exists only in memory and so resets on every restart

The roadmap said `redb` — pure Rust, embedded, no C dependency. Revisited before building
on it.

## Decision

**SQLite** (via `rusqlite` with the bundled feature, so there is no system dependency),
one database per node under the state directory.

The deciding argument is **inspectability**. This project's stated priority is that
decisions carry reasons and that "why didn't that happen" is always answerable — it is why
`Constraint::explain` exists, why `Hold` carries a reason, and why `NoBid` is structured.
A state store that can only be read by code we write is in tension with that. When a run
is stuck on a laptop at midnight, `sqlite3 ~/.offload/state.db 'select id, state, epoch
from runs'` is a genuinely different debugging experience from writing a program to find
out. Across a fleet, that difference compounds.

Supporting reasons:

- **Queries and indexes come free.** "Active runs for this node, newest first", "blobs with
  no referencing run", "this node's last ten absences" are one statement each. With a
  key-value store they are hand-rolled index keys that must be maintained transactionally,
  and each hand-rolled index is a chance to leave one stale.
- **Phones.** SQLite is native on Android and iOS, which matters given phones are workers
  (not clients) in this design.
- **Migrations are a solved problem.** `user_version` plus ordered scripts is boring and
  well understood. Schema evolution across a fleet where nodes upgrade at different times
  is exactly the situation phase 3 creates.
- **Crash semantics are known.** WAL mode's durability behaviour is documented, tested by
  approximately everyone, and survives the power-loss cases a laptop actually sees.
- **Event logs become a table.** `offload logs` after a restart starts working, and
  following becomes "read rows after id N" rather than tailing a file and parsing it.

## Consequences

Good: state is inspectable with tools that already exist on every machine. Queries express
intent rather than index bookkeeping. One store covers runs, events, blobs, and
observations, so there is one thing to back up, one to migrate, and one place to look.

Bad, and worth being clear about:

- **A C dependency.** `bundled` compiles SQLite from source, so there is no system library
  to install, but build times grow and cross-compiling to Android/iOS gains a step.
  Acceptable; it is the most portable C library in existence.
- **The API is blocking.** Every call has to go through `spawn_blocking` or a dedicated
  connection thread, or it stalls the async runtime. This is a real ongoing discipline
  cost, and getting it wrong produces latency that is hard to attribute.
- **One writer at a time.** Irrelevant today — a single daemon owns the state dir — but it
  forecloses a future where two processes share it, and that constraint should be stated
  rather than discovered.
- **SQL is easy to write badly.** A missing index turns a fleet query into a table scan.
  Key-value stores make that cost obvious; SQLite hides it until the data grows.

## Alternatives

**`redb`.** Pure Rust, no C, no blocking-in-async concern, very fast. Rejected on
inspectability: an opaque file is a poor fit for a system whose main debugging question is
"why is this in the state it's in". Also considered: the indexes we would hand-roll for the
queries above are precisely the bookkeeping SQLite exists to remove.

**Keep JSONL files and rebuild in memory on start.** Zero new dependencies, and the event
log already works this way. Genuinely tempting for the run registry alone. Rejected once
the blob index and absence history joined the picture: those want lookup and aggregation,
not replay, and rebuilding all state by scanning every file on every start does not survive
a node that has run for months.

**Postgres or another server database.** Rejected outright — a daemon that requires a
database server cannot run on a phone, and this is a single-writer embedded workload.

**Sled.** Pure Rust, more featureful than redb. Rejected for the same inspectability reason
as redb, and its stability history is less reassuring than either alternative.
