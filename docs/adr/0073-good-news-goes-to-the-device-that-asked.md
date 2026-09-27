# ADR-0073: Good news goes to the device that asked; problems go everywhere

**Status:** accepted · 2026-09-26 · session ninety-two · amends ADR-0010's default audience ·
builds on ADR-0026 (notices) and ADR-0071 (the app as a sink)

## Context

`Audience::Everyone` is the default: a run's news goes to every route that works, anywhere in the
fleet. That was right while routes were a script or two on a laptop. ADR-0071 made every phone and
tablet running the app a push route, and the owner then noticed: "now all devices are notifying on
all completed tasks". The laptop's delivery record showed it was agent runs finishing. Each one
went to the phone and to the emulator, whoever had submitted it. Schedules were already quiet: they
default to `Notices::Problems`.

## Decision

The owner chose, from four options: **that a run finished goes only to routes on the device it was
submitted from; its problems (failed, overdue, needs a decision, undecided) go to every route its
audience allows.** "It finished" matters to whoever asked. "It failed" and "it is waiting for you"
need somebody, wherever they are.

- The device a run was submitted from is `Run::home`, the node that accepted the submission. It is
  already on every record and gossiped with it.
- `offload_core::notify::reaches(notice, route_node, home)` is the rule. It is asked in the delivery
  pass beside `Audience` and `Notices`, where the news is **noticed**, so an outbox row is never
  owed to a route the rule excludes (the plane's standing rule). A route whose node cannot be told
  is told, and a rule's route (ADR-0057) is not a person and is not asked.
- `Fleet::me()` gives the pass its own id, which a local route leaves unsaid. A node with no mesh
  has no peers, so every run it notices is its own.
- `audience_note`, the sentence at submission for `--notify <service>`, now says where "finished"
  goes as well as where the rest goes. Otherwise it would promise a finish on another device.

## Consequences

- A run submitted from a laptop terminal tells no phone that it finished, unless the laptop has a
  route itself. Somebody at a terminal is usually following the run already. Its failure still
  reaches every phone.
- A machine-started run's home is the node that fired it. With `Problems`, the default for rules
  and schedules, nothing about its success is sent anyway.
- A person who wants every finish everywhere has no switch yet. That would be a per-route setting,
  if it is ever asked for.

## Walked

On the stub laptop and the emulator's app, with the phone in the fleet: a scratch run from the
laptop's CLI finished and was delivered nowhere. One submitted through the emulator's daemon
finished and was delivered to `emu-fresh/app` only, and the emulator posted it. The phone was sent
neither. `that_a_run_finished_goes_to_the_device_it_came_from_and_a_failure_to_all` fails with the
check disabled (control run).
