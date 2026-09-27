# ADR-0001: Workspace split, with a pure core

**Status:** accepted · 2026-07-25

## Context

Offload has a lot of moving parts that are easy to tangle: membership, scheduling, execution,
storage, transport. The failure modes we care about (double execution, lost tasks under
churn) are *logic* bugs, not I/O bugs — but they only show up under specific timing. If the
logic is welded to tokio and sockets, the only way to test it is to run real nodes and hope
the interleaving you need happens.

## Decision

Split into a cargo workspace where `offload-core` and `offload-sched` are **pure**: no tokio,
no networking, no clock access, no filesystem. They take a state snapshot and a timestamp and
return decisions. All I/O lives in `offload-cluster`, `offload-runtime`, `offload-store`,
`offload-transport`, and the two binaries.

Dependency direction is strictly downward, enforced by review:
`core ← proto ← {transport, cluster, store, sched, runtime} ← node ← cli`.

## Consequences

Good: scheduling and state-machine invariants become property-testable with no async runtime.
A partition test is a function call, not a container. Phase 5's simulation harness becomes
feasible rather than aspirational.

Bad: more plumbing. Deciding something in `sched` and applying it in `runtime` means passing
an explicit decision value around instead of just doing the thing. Time-as-a-parameter is
mildly viral through signatures.

Accepted, because the alternative is untestable distributed-systems code, which is the
category of code that most needs testing.

## Alternatives

**Single crate.** Fastest to start, and honestly fine for phase 0–1. Rejected because the
discipline is much harder to retrofit than to start with, and the boundary between "decides"
and "does" is exactly the boundary we need for testing.

**Crate per concern with async everywhere.** The default shape. Rejected for the reason above:
`Instant::now()` inside a scheduling decision makes the decision untestable.

## Amendment, 2026-08-28: `offload-sched` was never created, and should not be

The layout above names `offload-sched` as the second pure crate. It does not exist and nothing is
waiting on it: the bid round is `offload-cluster::place`, because it needs the view, the
connections and the serve loop, all of which the cluster already holds — and putting it elsewhere
would have meant passing all three across a crate boundary to gain nothing.

What the decision was actually *for* survived intact: the pure, clockless, synchronous half is
`offload-core`, and `offload-core/tests/no_clock.rs` enforces it. Splitting "decides" from "does"
is the rule; two crates was only one way of spelling it. A separate scheduling crate becomes worth
creating when migration *policy* grows past what `offload-core` already decides, and not before.
