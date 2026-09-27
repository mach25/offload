# ADR-0077: A host keeps its machine awake while it may take work

**Status:** accepted · 2026-09-27 · session ninety-two

## Context

Overnight the Mac mini kept dropping out of the fleet. The cause was macOS putting it to sleep a
minute after the last keystroke (`pmset -g`: `sleep 1`). Its daemon was suspended, peers marked it
dead, and it answered only in the seconds after a packet woke it (`pmset -g log`: Sleep, DarkWake).
Programs that must keep running tell macOS so by holding a power assertion; Offload did not. The
owner asked why other services work (they hold assertions, or are woken by the sleep proxy for the
TCP services they announce), and chose that Offload should handle it itself.

## Decision

1. **The daemon holds a `PreventSystemSleep` assertion (amended below; it was
   `PreventUserIdleSystemSleep`) while the node holds a run, or while
   it would take one.** "Would take one" is `hosting_refusal`, the rule behind `offload status`'s
   `accepting` line, asked for an agent run and then for a program. A node its owner said may not
   work, or one the fleet has not let host, or a laptop on battery whose policy says "only while
   charging", is left to sleep as a machine that is not a host should be. Re-asked every 30 s.
2. **Idle sleep only.** A person choosing Sleep or closing a lid is obeyed.
3. **Offload does it itself, through IOKit, in a crate of its own: `offload-power`.** The owner
   declined both Apple's `caffeinate` as a helper process and an outside crate (`keepawake`). The
   call needs `unsafe`, so `offload-power` alone has `unsafe_code = "deny"` rather than the
   workspace's `forbid`, with `#[allow]` in `src/macos.rs` only. A test in the crate fails if it
   appears in any other file. `offload-node` and every other crate keep `forbid`. No dependencies:
   IOKit and CoreFoundation are on every Mac.
4. **Reported from what is held**: `offload status` has a `sleep` line, "kept awake — it may take
   agent runs", or "allowed — this node would take no work". It is absent on a platform where the
   daemon cannot keep the machine awake, which is currently everything but macOS.

## Walked

On the Mac mini, `macos_lists_the_assertion_while_it_is_held` (ignored by default, since it changes
the machine's power state) found "Offload: power test" in `pmset -g assertions` while the guard was
held, and not after it was dropped.

## Consequences

- The Mac stays awake only once it may host: it has no `host-runs` yet, which needs the fleet
  passphrase. Until then it sleeps, correctly by this rule.
- Linux and Windows hosts are not covered. On Linux the equivalent is systemd's inhibitor over
  D-Bus, a separate decision if a Linux host is ever found asleep.

## Amendment (session ninety-four): `PreventSystemSleep`, because the first walk read the wrong thing

The Mac got `host-runs`, and its daemon took the assertion at 12:08:49. `pmset -g` said "sleep
prevented by offloadd". The Mac was asleep six seconds later, and peers marked it dead.
`pmset -g log` showed `Entering Sleep state due to 'Maintenance Sleep'` at 12:08:59 and again at
12:19:10, with the assertion held throughout. `PreventUserIdleSystemSleep` stops idle sleep on a
machine that is fully awake. A Mac mini with nobody at it is already asleep, and it wakes only
into a *dark wake* when a packet arrives. From there it goes back to sleep whatever that assertion
says.

The daemon now holds **`PreventSystemSleep`** (`caffeinate -s`), which holds through a dark wake.
Walked on the same Mac, untouched for five minutes: one return to dark wake at 12:23:12 and no
sleep entry after it, where each dark wake had ended in sleep within about 45 s before. The laptop
saw it `alive` throughout. Five minutes, not a night: the overnight check is on the pick-up list.

§2 still holds: a lid, the Sleep menu and a low battery still sleep the machine.

**Residual: mains power only.** macOS ignores `PreventSystemSleep` on battery. A MacBook host on
battery whose policy lets it work will sleep, and `offload status` still says "kept awake". That is
an over-claim, and it is left unfixed until such a host exists. The fix would be to have the `sleep`
line read the probed power source.

**The walk above was the mistake.** `macos_lists_the_assertion_while_it_is_held` proved that macOS
*listed* the assertion. It never showed that the machine stayed awake, and no test that reads
`pmset -g assertions` can show that. The check that can is `pmset -g log` over minutes with nobody
touching the machine, beside a peer's `offload nodes`.
