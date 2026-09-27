# Logs and listings

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/logs-and-listings.md`, same order.

The after-the-fact record: `offload audit`, `offload nodes --history`, and the timestamps,
truncation and column widths that make a listing readable rather than misleading.

- **A hook with two callers has two meanings, and its doc comment will name one.** An audit row for
  a *grant* written in `Host::record` fired once per gossip merge — the signature of an arbiter
  spending a token twice, on a run where nothing went wrong — and was missing on a fleet of one.
  `Host::granted` is the decision; `Host::record` is the record.
- **A refusal recorded on the ordinary path is not a refusal any more.** `Refused` rows are read by
  somebody who suspects two agents on one repo, so their value is being rare — and every graceful
  drain wrote one. Not being allowed to describe a run is a refusal only when the leg **believed it
  still could**.
- **Three logs.** The run's (served by its holder, so a fenced-out leg's line is read by nobody),
  the fleet's (membership), and the **audit log** — per-node, never gossiped, holding every
  assignment with its epoch, every write this node was *refused*, and every run taken away from it.
  Deliberately not metrics: nobody scrapes their phone.
- **A displayed run id names a *tick* when the id was derived, and twelve characters of it name
  two runs for ever.** `RunId::short` shows six bytes because six bytes is the whole 48-bit
  UUIDv7 clock, so two ordinary runs collide only inside one millisecond. `Schedule::occurrence_id`
  (ADR-0056) puts the tick in exactly those six bytes and a digest of the *schedule* after them —
  so `every 1m` beside `every 2m` puts two different runs on screen under one id on every boundary
  they share. Measured: two collisions in four minutes, `offload ps --all` printing each twice,
  and `offload logs` and `offload explain` then refusing both with *"ambiguous — use more
  characters"* and **no more characters anywhere on the screen to use**. Session nineteen weighed
  this and left it deliberately, correctly for what existed then — a random tail made fourteen
  characters "a worse kind of rare" — forty-eight sessions before the derivation that removed the
  tail. An inherited "not worth fixing" is a measurement with a date on it, like an inherited
  `[x]`. The width is now a fact about the listing (`id_width`, pinned by a test on the two real
  ids), the ambiguity refusals **name the candidates** in full at both spellings of that error,
  and `offload explain` prints the id whole — it had been re-collapsing to twelve the id somebody
  had just typed in fourteen.
- **A width computed for one listing is not a width, and the listing that was left behind is the
  one whose whole job is telling runs apart.** Session seventy-four made `offload ps` compute
  `id_width` from its own rows; `offload audit` kept a fixed twelve. Measured on one daemon with
  `every 1m` beside `every 2m`: `ps` widened to fourteen and the audit log **merged**, printing
  four rows under one id — `granted … accepted … granted … accepted`, every one at epoch 1. That
  is not a duplicate-looking listing, which is what `ps` showed before the fix; it is the
  signature of *an arbiter spending a token twice*, which is the single thing the audit log exists
  to let somebody detect, rendered out of two ordinary runs. `offload audit <that id>` was then
  refused as ambiguous. The fix is `id_width` over its own rows, which is the same function; the
  finding is that the first fix was applied where the defect was *seen* rather than everywhere the
  fact was read. **When a rendering rule turns out to be wrong, grep for every site that renders
  it before writing the fix.** Residual, deliberately: `offload audit <run>` filtered to one run
  prints twelve, which is honest for a one-row listing and is a smaller version of the same trap
  if somebody copies it onward.
- **…and an id printed to be *typed* is not a column, so it has no width to trade.** Four places
  print `offload approve <run> <tool_use_id>`. `offload logs` echoes what the operator typed and
  is right by construction; `offload asks`, `offload explain`'s `waiting` line and the drain's
  `Blocked` step each abbreviated to twelve — and `explain` did it four lines under a header that
  session seventy-four had deliberately made print the id **whole**. One screen, two widths, and
  only the top one is guaranteed to resolve. All three print it whole now. **Not reachable today**,
  and the reason is worth writing down because nothing states it: derived ids come only from
  `Schedule::occurrence_id`, and `offload every` has no `--ask`, so a run that can be blocked on a
  question can never have an id that names a tick. That coupling is one flag away from being
  false, holds nowhere in a type, and was found by asking the sweep's question rather than by
  meeting the bug.
- **An unmarked wall-clock time is read as the reader's own, and `when` renders UTC.** Both
  after-the-fact logs — `offload audit` and `offload nodes --history` — printed
  `2026-09-12 13:20` bare, under a doc comment saying *"A local timestamp for a person reading a
  list"*. Measured on a CEST machine: a run submitted at **15:20:19** local. Two hours out, in the
  two commands whose entire job is placing an event in time, so anybody asking *did anything happen
  around three* was silently told no. The comment asserting the one thing the code did not do is
  why nobody looked. It says ` UTC` now, which also makes the column directly comparable with
  `offloadd`'s own `Z`-suffixed lines; converting to local needs a dependency this crate does not
  have or a TZif parser, and both have ways to be wrong that a label does not.
- **A listing that stops at its limit has to say so, and this one said the wrong incompleteness.**
  `offload audit` asks for 100 rows and printed exactly 100 with no notice, while its closing line
  — *"What this device did, not the fleet's — ask the others too"* — addressed the **other nodes**,
  which is a different gap from the one immediately in front of the reader. Measured with sixty
  submissions. Its own `--help` said *"Omit for everything this node has recorded"*, which the
  command has never done. Ask for one more than you show and the difference between *this node did
  exactly this much* and *the rest was cut off* becomes a fact rather than a hedge; `offload nodes
  --history` had the same cap and matters more, being what somebody opens to find out whether a
  device was enrolled without their knowledge (ADR-0012 mitigation 4).
- **The durable copy of a fact was the vague one.** `offload audit` rendered a rescue as *"it is at
  `<state>/worktrees/<run>.superseded*`"* — literal angle brackets, in the line that says where the
  only copy of somebody's uncommitted work is. The run's own log printed the **real absolute path**
  for the same rescue, and that is the log which does not survive: `AuditEvent::Rescued`'s own doc
  comment says the rescuing leg is often not the node anybody can read the run's log from, and once
  the run is deleted nobody can. The id was the sharp part — it is on the audit row already,
  abbreviated to twelve by `id_width`, while the directory is named by the full thirty-two, so a
  reader substituting what was in front of them got a path that does not exist (measured, with
  `ls`). `run` is in the variant; the state directory is not and is named in words instead.
- **…and the same shape, a third time, in the command whose comment said it could not happen.**
  `offload logs` printed `run released — resume it with: offload resume <run>` — forty lines in the
  same `match` from the `Asked` arm whose comment reads *"The run, spelled, rather than `<run>`:
  this is the one line in the log that is an instruction"*. It was not; there were two, and the
  sentence asserting there was one is what stopped anybody looking for the other. Found by the
  sweep this file already prescribes — *grep for every site that renders it before writing the fix*
  — run three sessions late. There is a lint now (`offload-cli/tests/no_placeholders.rs`), because
  what these three share is a **shape** and not a subject: a reviewer reading the arm in front of
  them will not find the next one, and there is no type that says "this string reaches a person".
  Proved red against the line it was written for.
- **A transport fall-back is not a refusal, even when it arrives as an errno.** quinn answers a
  GSO batch the NIC cannot offload with `EIO`/`EINVAL` and switches GSO off. Counted as a
  refusal, one such send at startup printed `sends 1 refused by this machine's kernel` for the
  daemon's whole life. `classify` mirrors quinn-udp's own rule, for segmented transmits only.
