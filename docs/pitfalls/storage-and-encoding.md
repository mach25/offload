# The store, ids and encodings

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/storage-and-encoding.md`, same order.

- **A whole-row write is a claim about every field, including ones you never read.** `save_run`
  restores a moment. `Store::update_run` is the read-modify-write under the store's own lock, and
  every writer that changes a run it did not just create uses it. The window needs no `await`.
  Three callers stay on `save_run` deliberately: a record arriving from the fleet is *meant* to be
  written whole.
- **…and "the row does not exist yet" is not a reason to reach for it.** That was `start_run`'s
  excuse and it cost an erased cancel — `submit` builds a run and starts it in the same breath, so
  the start's own write is the row's first existence, and `update_run` reads first and gives up.
  `Store::upsert_run` is that case: it hands the closure whatever is stored, `None` when nothing
  is, and writes what comes back. An error writes nothing, which is why the closure gets the
  stored copy rather than a mutable handle on the caller's.
- **An id prefix is a prefix, not a pattern.** `LIKE` reads `%` and `_` as wildcards, so
  `offload cancel %` resolved the only run in the store. A run id is hex; anything else is refused
  by name.
- **Internally-tagged enums can't wrap a sequence.** `Response::Runs(Vec<_>)` compiles and fails at
  runtime. Use a struct variant, and round-trip *both* directions in tests.
- **A hash in a run's JSON is an array of integers, not hex.** `hex()` in SQL cannot find it — the
  blob collector matched nothing and deleted live checkpoints. Ask the domain type
  (`Checkpoint::blobs`), never the text.
- **A domain type's shape is a storage format, and the wire version does not cover it.** A whole
  `Run` is one JSON blob in `runs.run_json`, so changing `RunSpec` changes what a node reads back
  from *its own disk* after an upgrade — and there is no migration to hang that on, because the
  schema did not move. `MIN_VERSION` tracking `VERSION` protects the wire and nothing else. The
  `Work` split therefore carries a hand-written `Deserialize` accepting both shapes; without it,
  upgrading a daemon turns every stored run and rule into `StoreError::Decode`. **Ask what reads
  the old bytes before changing a serialized type**, and remember the answer is usually "this
  same node, tomorrow".
- **A compatibility fixture must be written by hand, not generated.** A test that builds its
  "old" document by re-serialising today's type changes shape in lockstep with the code it is
  supposed to pin, so it passes for ever and proves nothing. Spell the old JSON out as a literal
  — and then, because a literal is still only a guess about what the old build wrote, check it
  against a row a real old binary produced. Both were done for the `Work` split and the literal
  had the serde spellings right; that was luck, and the walk is what turned it into evidence.
- **What may be an abbreviation of an id is a rule, and there were two copies of it.**
  `Store::resolve_run` refuses an empty needle and a non-hex one, with the reasoning beside it —
  resolving rather than refusing is "the helpfulness that cancels the wrong job". `server::
  find_run` falls back to scanning the **cluster view** when the store says no, so that a run this
  node has heard of but not stored is still findable, and it re-implemented the prefix match with
  neither check. The guard was therefore missing exactly where it was the only one:
  `"".starts_with("")` is true of every run, so on a node with one run in view an **empty**
  argument resolved to it. Measured: `offload cancel ""`, `offload explain ""`, `offload
  checkpoint ""` and `offload audit ""` each acted on a real run, while `offload logs ""`,
  `offload rm ""` and `offload resume ""` refused — one argument, validated on three commands and
  not on four, and the four included the two that stop work. An unset shell variable is how an
  empty argument arrives. `offload_core::needle` is the one rule now (`Needle::{Empty, NotHex,
  Prefix}`), asked by both.
- **…and "nothing was typed" is not "nothing matched".** The first cut answered the empty case
  with a `NoSuchRun` holding an empty needle, which renders ``state store: no run matching `` `` —
  a sentence that reads as a lookup that failed and sends somebody to check their ids, with a
  prefix naming the database as the place to look. `StoreError::NoRunGiven` is its own variant and
  `SubmitError::Needle` carries it out **transparently**, without the `state store:` prefix,
  because nothing about the store went wrong.
- **A field on a domain type that travels is only on the wire if the message carrying it has it.**
  ADR-0063 put `ScoreTerms` on `Offer` and `Bid`, and a peer's offer crosses as
  `ClusterMessage::Bid { score, available }`, which the arbiter turned back into an `Offer` with
  `terms: None`. Every unit test passed, because each built its opinions directly; the walk printed
  *"preferred bravo, which was outscored (116 against 116) — placed on bravo"*. **When a field is
  added to something that crosses the wire, grep the proto enum for the variant that carries it**,
  and walk it on two daemons — a test that constructs the input skips the conversion that drops it.
- **"The view has settled it" expires with the view.** `record_run` writes a fleet record whole
  because the merge happened in the view, and the view forgets finished runs after `GOSSIP_TAIL`.
  Past that, nothing judged a stale copy, and it overwrote a stored `cancelled`. Ask of any
  whole-row write whose justification is "the view merged it" what happens once the view has
  forgotten. The rule and its walk are in `gossip-and-merge`.
