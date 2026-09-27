# The store, ids and encodings — full entries

The working rules are in `../storage-and-encoding.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A whole-row write is a claim about every field, including the ones you never read.**
  `save_run` serialises a `Run` and overwrites the row, so a caller's copy does not save a change
  — it restores a moment, and whatever another task wrote in between is gone with no error
  anywhere. The window needs no `await` in it: two tokio tasks and one store are enough, which is
  why "the load and the save are three lines apart" is not the defence it reads as. `heartbeat`
  renewed leases from a listing taken before the loop began, so a run that finished mid-loop was
  written back as `Running` — *and then kept alive*, because the same loop goes on renewing the
  lease of the run it resurrected, and nothing orphans a run whose holder is heartbeating for it.
  `Store::update_run` is the read-modify-write under the store's own lock, and every writer that
  changes a run it did not just create uses it. `Supervisor::replicate` had the rule right from
  the start ("turns keep happening while bytes are in flight") and re-read before it wrote, which
  is the same thing one lock later. Two callers stay on `save_run` deliberately: a record
  arriving from the fleet is *meant* to be written whole (`record_run`, `take_run`, `start_run`),
  and `resume` validates a dormant run against a room check that cannot run under the store lock
  — no agent is writing that row, which is the reason it is safe rather than an oversight.

- **An id prefix is a prefix, not a pattern.** `resolve_run` matched `lower(hex(id)) LIKE
  needle || '%'`, and `LIKE` reads `%` and `_` as wildcards — so `offload cancel %` resolved to
  the only run in the store and `ab_d` reached a run whose id has no underscore in it. It
  *resolves* on a single match rather than refusing, which is the "helpfulness that cancels the
  wrong job" its own doc comment warns about. A run id is hex; anything else is refused by name.
  Found by sweeping every place a hash or an id is compared as text, which is worth repeating
  after any change to how one is stored.

- **Internally-tagged enums can't wrap a sequence.** `Response::Runs(Vec<_>)` compiles and
  then fails at runtime with a bare encoding error. Use a struct variant, and round-trip
  *both* directions of a protocol in tests — testing only the request side is how this one
  shipped.

- **A hash in a run's JSON is an array of integers, not hex — and `hex()` in SQL cannot find
  it.** `BlobHash` and `NodeId` derive serde over `[u8; N]`, so a stored run spells its
  transcript `[163,188,121,…]`. The blob collector matched `instr(run_json, lower(hex(...)))` and
  therefore matched *nothing*: every blob looked unreferenced and it deleted the checkpoints of
  live runs, while its own comment claimed a substring match "can only be over-cautious". The
  schema knows this — `fleet_events.subject` is denormalised out of the JSON for exactly this
  reason — four hundred lines from the query that did not. Ask the domain type
  (`Checkpoint::blobs`), never the text.

- **What may be an abbreviation of an id is a rule, and there were two copies of it.**
  `Store::resolve_run` normalises and judges the needle, with the reasoning written out beside it:

  > *A prefix, not a pattern. `LIKE` reads `%` and `_` as wildcards … and with one match this
  > function resolves rather than refusing, which is the "helpfulness that cancels the wrong job".*

  `server::find_run` asks the store first and, when the store says no, scans the **cluster view**
  instead — deliberately, because the view holds every run this node has heard of and the store
  holds the ones it wrote down, so a run placed on a peer a second ago is findable in one and not
  the other. That fallback re-implemented the prefix match: `run.id.to_string().starts_with(&needle)`,
  with neither the empty check nor the hex check. It is reached **exactly when the store refused**,
  so the guard was absent precisely where it was the only one.

  Measured on two daemons with one run in view:

  ```
  cancel "":     Error: run 01a094732964 cannot be cancelled: it is already cancelled
  explain "":    run  01a0947329647d61905612f39940533d …
  checkpoint "": Error: nobody is running 01a094732964 — it is cancelled …
  audit "":      2026-09-12 07:09  01a094732964  granted to 21177c09 at epoch 1
  logs "":       Error: state store: no run matching ``
  rm "":         Error: state store: no run matching ``
  resume "":     Error: state store: no run matching ``
  ```

  Four commands acted on a real run for an argument nobody typed, three refused it, and the
  difference is whether the command goes through `find_run`. Had that run been *running*, `offload
  cancel ""` would have stopped it — which is the store's own comment, arriving through the door it
  was written to shut. An unset shell variable in `offload cancel "$RUN"` is how an empty argument
  reaches a daemon.

  `offload_core::needle` is the rule now — `Needle::{Empty, NotHex, Prefix}`, three-valued because
  the two refusals need different sentences — and both sites ask it. `find_run` asks it *before*
  the scan rather than inside it, and returns the store's own refusal, so there is still one
  sentence explaining why a needle is not an id.

- **…and "nothing was typed" is not "nothing matched".** The first cut of that fix answered the
  empty case with ``StoreError::NoSuchRun("`` (no run id was given)")``, which renders as

  ```
  Error: state store: no run matching `` (no run id was given)
  ```

  Three clauses fighting: a prefix naming the database as the place to look, an empty pair of
  backticks, and the reason. Nothing about the store went wrong and no lookup failed.
  `StoreError::NoRunGiven` is its own variant — *"no run id was given — `offload ps` lists them"* —
  and `SubmitError::Needle` carries it out with `#[error(transparent)]` so the `state store:`
  prefix does not attach. Caught by re-running the seven commands after the fix, which is the
  habit: a fix creates new sentences and they can be false too.

*The entries below were backfilled in session ninety-one from `docs/sessions.md` and the commits
that added each rule — sourced, not reconstructed from the rule text.*

- **…and "the row does not exist yet" is not a reason to reach for it.**

  Session thirty-five (commit `53bcb40`). `submit` calls `build` then `start_run`, so
  `Store::update_run` — which reads first and gives up when there is nothing — could not express the
  transition, and `save_run` writes the whole row. `Store::upsert_run` is `update_run` plus the row
  that does not exist yet: it hands the closure whatever is stored, `None` when nothing is, and
  writes what comes back; an error writes nothing, which is why the closure receives the stored copy
  rather than a mutable handle on the caller's.

- **A domain type's shape is a storage format, and the wire version does not cover it.**

  Session sixty-six, second half (commit `917c00d`), the `RunSpec` split. A whole `Run` is one JSON
  blob in `runs.run_json`, so changing `RunSpec`'s shape changes what a node reads back from its own
  disk after an upgrade — no migration to hang it on, because the schema never moved, and
  `MIN_VERSION` protecting only the wire. Without a compatibility deserializer, upgrading a daemon
  turns every stored run and rule into `StoreError::Decode`. Checked with a pre-split binary writing
  a real run row and the new binary reading it back intact.

- **A compatibility fixture must be written by hand, not generated.**

  Same session. A generated fixture changes shape in lockstep with the code it is meant to pin. The
  hand-written one beside the split had the serde spellings right, *which was luck* — the walk with
  the old binary is what made it evidence.

- **A field on a domain type that travels is only on the wire if the message carrying it has it.**

  Session eighty-nine (commit `dde26f8`), phase 10. A peer's offer lost its score terms crossing
  the wire — `ClusterMessage::Bid` had no field for them — and the note called the winner outscored
  by itself. Every unit test built its opinions directly and so never crossed.
- **"The view has settled it" expires with the view.** Session ninety-four; the full entry,
  with the walk that found it, is the second of that session's two in `gossip-and-merge`.
