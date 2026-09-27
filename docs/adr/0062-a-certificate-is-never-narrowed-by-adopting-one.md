# ADR-0062: A certificate is never narrowed by adopting one

**Status:** accepted · 2026-09-11 · built and measured in the session that decided it · completes
ADR-0012 mitigation 1, which bounded an approver in one direction only · amends no other ADR

## Context

ADR-0012 mitigation 1 is the bound that makes approvers safe to have. An approver may enrol a
device without the passphrase — that is the entire point, since a fleet whose only enrolment path
is a secret in a drawer is a fleet where the drawer gets opened routinely — but it may **not** mint
`HostRuns`, because that grant is what turns membership into "runs agents on your repositories with
your credentials". `FleetState::invite` issues `default_grants()` and nothing more;
`FleetState::issue_for` is the same act with the root signing, and it is the one that can widen.

That bound is stated, enforced and tested. What nobody stated is the other direction.

Two paths hand this node a certificate, and both end at `FleetState::adopt`:

- **`offload join --token`**, when the device is already a member. The documented purpose is
  widening: ADR-0012's "`offload grant` gains a `<node>` argument", by the route that does not need
  the two devices to be on speaking terms.
- **the renewal loop in `mesh.rs`**, which calls `adopt` with whatever an approver peer answered
  `RenewMe` with. `renew_for` restates the grants of the certificate it descends from, so an
  honest renewal never changes them.

`adopt` decided by expiry alone, and answered a `bool`:

```rust
if cert.expires_at <= self.membership.expires_at {
    return Ok(false);
}
```

Its doc comment said "Refuses anything that is not an improvement", and the test beside it was
named `a_renewal_is_taken_up_only_if_it_is_ours_and_an_improvement`. Both were true of the axis the
test varied, and the test only ever varied expiry. A certificate that expires later and grants
**less** was an improvement by the only measure anything took.

Measured on two state directories, with `bravo` holding `HostRuns`:

```
$ offload invite 580993bf… --name bravo          # on alpha — no passphrase, no prompt
$ offload join --token offload-invite-1.…        # on bravo
Took up a new certificate in fleet 21e2074c….
  grants      submit, deliver
```

Nine seconds earlier, holding `HostRuns`, the same command had printed:

```
  grants      submit, deliver
  host-runs   dormant for another 14 minutes (probation)
```

The `grants` line is **identical in both**, because `grant_list` renders what is in force and
probation renders `HostRuns` on its own line. The only visible difference is a countdown
disappearing, which reads as probation elapsing rather than as the grant being removed. The
screen could not be read, and two commands with no passphrase had taken away the grant the
passphrase exists to protect.

The founder form is worse. `offload invite <its own id>` is accepted, and taking that token up
took alpha from `submit, deliver, host-runs, approve` to `submit, deliver` — the fleet's only
approver, de-granted in two commands, with the fleet then holding no approver at all and no way to
make one without the passphrase. It silently replaced the device's **name** too (`alpha` →
`448dd027`), because `invite` defaults `--name` to a short form of the id.

And the renewal path makes it worse again: it arrives over the network, from any approver peer,
with nobody pasting anything.

## Decision

**A certificate is an improvement only if it takes nothing away.** `adopt` compares the two
certificates' grants before it looks at the clock, and refuses one that leaves any out.

Neither path ever wants to narrow. `renew_for` restates what it descends from; `invite` exists to
widen. So a narrower certificate is a mistake or a downgrade, and there is no third thing it could
be. It is refused by name:

```
that certificate takes host-runs away from this node, so it was not taken up. A renewal restates
what it descends from and `offload invite` widens a certificate; neither narrows one. If this
device really should stop hosting, `offload revoke` on the device with the passphrase says so.
```

**The comparison is between the certificates' own grants, not `grants(now)`.** Probation suppresses
`HostRuns` for fifteen minutes without removing it, so comparing what is *in force* would read the
certificate that carries the grant as dropping it — and refuse every invitation that grants
`HostRuns`, which is the one this decision exists to protect.

**`adopt` answers `Adopted::{Taken, NoBetter, Narrower { lost }}`, not a `bool`.** This is session
seventy-five's fix in a second file: a `bool` threw the measurement away before anything had to
word a sentence about it, so the refusal had nothing left to say. `Narrower` carries what was left
out, which is what both callers print — the CLI in the error above, the renewal loop at `warn` with
`lost` as a field.

**Widening is decided on the grants, not on the clock.** The first cut of this argued that every
issuing path stamps `now`, so a certificate carrying a new grant was minted later and therefore
expires later, and needed no arm of its own. That is true of one machine and false of two: the
issuer is a different device — the whole point of `offload invite` — and a few seconds of clock
skew behind this node makes a deliberately widened certificate expire *earlier* than what is held,
which the expiry test alone refuses as `NoBetter`. Refusing a grant somebody typed the passphrase
to issue, with a sentence saying it buys nothing, is the same class of wrong as the narrowing this
ADR is about. A superset with more in it is `Taken` whatever its clock says.

## Alternatives rejected

**Accept it and report the loss loudly.** The obvious fix, and it was the first draft: keep
adopting, print `was submit, deliver, host-runs` above the new line. It fails on the path that has
no screen — the renewal loop takes certificates from peers with nobody watching, and a `warn` line
is not consent. It also leaves the ADR-0012 asymmetry standing: an approver that may not mint
`HostRuns` could still strip it, which makes the mitigation a bound on one direction of a two-way
door. Reporting is kept where it is cheap and correct: `join --token` now prints a `was` line when
the grants actually changed, so a renewal that moves nothing is distinguishable from one that does.

**Let the passphrase narrow, and refuse only an approver's narrowing.** Tempting, since the root
may legitimately do anything. But there is no command that expresses "remove a grant" — `offload
grant` only adds, and `offload revoke` removes the device. Narrowing by re-issue is therefore not a
supported operation by either signer, and the certificate arriving at `adopt` is the wrong place to
infer an intention nothing typed. If ungranting is ever wanted it is a command that says what it is
doing, and an ADR of its own.

**Refuse `offload invite` for a node that is already a member.** Closes the CLI path and not the
network one, and breaks the documented use: re-inviting an existing member with `--grant host-runs`
is how a device is widened without walking to it.

## Consequences

- An approver's delegation is now bounded in both directions: it may not mint `HostRuns` and it
  may not remove one. ADR-0012 mitigation 1 reads as written rather than as half-enforced.
- A device cannot be de-granted by anything it is handed. The only way for a device to stop
  holding a grant is `offload revoke`, which is signed by the fleet key, gossips, and says so.
- `offload rekey` is unaffected: it re-founds the fleet, so the certificates it prints are for a
  *different* fleet and go down `accept_invite`'s succession path, not `adopt`'s.
- The second finding beside this one was a report, and it is fixed with it rather than by it:
  `offload fleet`'s `approver` line read `FleetState::approver` — the delegation — while the door
  (`FleetState::invite`), the note, and `offload status` all read the `Approve` **grant**. A
  de-granted founder's screen therefore said `approver this node may enrol others` four lines above
  `note: this device is not an approver`, with `offload invite` refusing. The line now reads both
  halves in the order the door asks for them, and names the third state the door already
  distinguishes: granted with no delegation to prove it. That desync is unreachable now that
  `adopt` cannot narrow, which is the reason to fix the line rather than rely on it.
