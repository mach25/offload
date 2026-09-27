# ADR-0068: A hot device defers like a busy one — thermal status is observed pressure

**Status:** accepted · 2026-09-26 · session ninety-two · completes phase 5's "mobile capability
probing: battery, thermal state, metered network, from platform APIs" for Android · amends no ADR

## Context

Phase 5 lists thermal state as the one platform fact nothing reads. A phone that is already
throttling is slow at whatever it takes on, and it gets hotter doing it, and it is a phone somebody
is holding. Android states it directly: `PowerManager.getCurrentThermalStatus()` (API 29+) returns
0 `NONE`, 1 `LIGHT`, 2 `MODERATE`, 3 `SEVERE` ("throttling is significant"), 4 `CRITICAL`,
5 `EMERGENCY` or 6 `SHUTDOWN`. The Android host (ADR-0066) is where that call can be made, and
`host-facts.json` is how its answers already reach the daemon.

The question is not how to read it but what kind of fact it is. It is the same kind as CPU load:
**observed pressure**, which changes by the minute and says what the device is going through, not
what it is. ADR-0013 already settled how that kind of fact acts, and the thermal status should act
the same way.

## Decision

### 1. A local fact, never gossiped, beside the load average

`NodeLoad::thermal: Option<Thermal>`, filled from the host's `host-facts.json` (while fresh, ADR-0066
§3) at the same three places the load average is read: the bid, `offload status` and
`offload policy`. They read it through one function, so the report and the decision cannot disagree.
It stays out of `Capabilities`, which would make every temperature change an incarnation and a
gossip round, the churn session ninety-two removed for free disk. `None` is the answer everywhere
no host says anything: Linux, macOS, Termux. An unreported temperature is not a cool device, and the
rule then does not apply, which is how an unreported load average is treated.

### 2. It gates accepting, never starting, graduated by demand

`admits` refuses with `Refusal::TooHot` when the status is at or past a demand's line:

| demand | refused from |
| --- | --- |
| light | `SEVERE` |
| normal | `SEVERE` |
| heavy | `MODERATE` |

`SEVERE` is where Android itself says throttling is significant. Heavy work is the kind that makes a
device hotter, so it stops one step earlier. It is the same graduation `needs_idle_percent` applies
to load. Like load, **it gates accepting and never starting**. A run the device already committed to
has nowhere else to be, and the way out is `review_commitment` (ADR-0013's existing rule, unchanged).
Like load, it defers (`NoBid::Busy`) when the run has slack, and otherwise refuses, through the arm
`evaluate` already has for `UnderPressure`.

### 3. It does not score

A device below its line bids as it would have. A score term would mean choosing a weight, and ADR-0063
§2 has just shown what a weight table needs before it can be trusted. The gate says what the owner
cares about, which is that a hot phone does not take work. A term can be added the day a walk shows
a warm device winning rounds it should lose.

## Consequences

- A phone that is throttling stops taking new work and says why, in `offload status` and in a
  refusal, and it resumes by itself when it cools. Nothing is stored and nothing gossips.
- The Android host writes `thermal` beside `battery_percent` and `metered`.

## What this deliberately leaves

- **Linux and macOS thermal zones.** `/sys/class/thermal` has temperatures, not a status, and
  choosing thresholds per machine is exactly the guess the probe must not make. Android is the
  platform that states a status, so it is the one read.
- **Throttling that is not reported.** A device that is slow and cool is load's business.

## Amendment, 2026-09-26: walked on the phone

Android's own test hook, `adb shell cmd thermalservice override-status N`, works from the shell on
the phone. It sets what `getCurrentThermalStatus` reports, and so what the app writes. At `SEVERE` (3)
the phone's `offload status` said `thermal severe` and `accepting no — the device is too hot: thermal
status severe, and this work is refused from severe`, and a task from the laptop was refused with the
same sentence. At `MODERATE` (2) its `offload policy` said light and normal yes and heavy no. A normal
task ran on the phone, and a heavy one was refused. `cmd thermalservice reset` put it back to 0.
