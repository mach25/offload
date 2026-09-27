# ADR-0078: A phone set down asks to be left alone

**Status:** accepted · 2026-09-27 · session ninety-three

## Context

Overnight, unplugged, the phone went from 100% to about 50%. The owner's usual figure is 98%.
`dumpsys batterystats` for those 10h55m:

- Offload's own CPU was 187 mAh of the 1967 mAh drained.
- The rest was the radio. The Wi-Fi multicast lock was held for the whole 10h55m. `wlan_rx` and
  `wlan_tx` wakes were each about 6 h, 27–28k times: a probe or an answer roughly every second,
  each one waking the chip and then the CPU.

The fleet was doing what it was built to do: probe everybody about once a second, so that a holder
that goes silent is noticed within seconds. That is the right rate for a node holding a run. It is
the wrong rate for a phone lying on a table that holds nothing. The owner put it plainly: "if the
phone is set down and does not have any active tasks it can go into a sort of hibernation mode.
notification etc don't have to appear fast". The system was designed from the start for nodes that
go away for long periods: orphaned is not revoked, and a returning holder reclaims at the same
epoch. So a phone with nothing in hand loses nothing by being heard from rarely.

## Decision

1. **A node is *quiet* when its host says it is on battery with the screen off, and it holds no
   run.** Nothing else makes it quiet. Unknown is not quiet: a host that does not report the screen
   (every non-Android host today) keeps the ordinary rate. A held run keeps it too, because the
   arbiter's patience with a silent holder is what fences that run.
2. **`NodeView::quiet` is gossiped.** It is owned by the node itself and merged on a higher
   incarnation, like `addresses` (ADR-0076). An older peer would drop the field when relaying the
   record, so this is **wire v36**, with `MIN_VERSION` equal.
3. **A quiet node is probed about once a minute** (`QUIET_PROBE_EVERY`, 60 s), and only directly:
   no indirect probes through third parties, which would wake it just the same. It is suspected
   only after `QUIET_SUSPECT_TIMEOUT` (150 s), so a minute's silence is not a failure. Its own
   probe loop also runs once a minute, and it stops redialling peers' gossiped addresses.
4. **The Android host holds the multicast lock only while the screen is on.** Discovery with the
   screen off is not needed: known peers are dialled directly, and a new one is found the next time
   somebody looks at the phone. The host writes `interactive` into `host-facts.json`, and rewrites
   the file the moment the screen turns on or off.
5. **The quiet decision reads the host's facts whatever their age** (`last_host_facts`). A
   suspended phone stops its fact writer along with everything else. Its facts go stale *because*
   it is set down, and the 120 s freshness rule would read that as "no answer", flipping it out of
   quiet at every packet. Battery good news still uses the fresh-only reader.

## Consequences

- A phone set down is noticed as gone within about two and a half minutes, not seconds. It holds
  nothing, so nothing waits on that.
- News for it (a notification, a question) arrives up to a minute or so late. The owner accepted
  that. A question the phone itself asked keeps it non-quiet, because the run is held.
- Waking costs one 15 s tick plus up to one quiet probe interval before the ordinary rate returns.
- **Residual:** being quiet could go further. A node holding nothing does not need liveness at all:
  it could be left unprobed and rejoin when it wakes. Whether 60 s is enough is for the overnight
  re-measure to say. If the drain is still well above the owner's 2%, the next step is "not probed
  at all while quiet", not a longer constant.
- **Residual:** whether a quiet phone should *bid* is unchanged: work policy still decides. A phone
  that is set down and allowed to work on battery can still take a run, and stops being quiet as
  soon as it holds one.

## Correction (session ninety-four): half of the night's multicast lock was adb's

The context above says the Wi-Fi multicast lock was held for the whole 10h55m, and reads it as
Offload's. The per-uid lines of the same dump say otherwise: `se.mach25.offload.app` (u0a160) held it
for 5h33m, and system uid 1000 for 5h11m. uid 1000's lock is `AdbMulticastLock`, **wireless
debugging**, the connection walks use to reach the phone (`dumpsys wifi`: `Multicaster{AdbMulticastLock
uid=1000}`). Offload's half was real, and ADR-0078/0079 fixed it. The next 4h20m on battery, with the
daemon stopped, had Offload holding the lock for 3 minutes and 16.6 mAh of 758 drained (mostly the app
on screen). adb held the lock for four hours. **A battery reading taken while wireless debugging is on
measures the walk's own connection too.** Read the per-uid lines, not the total.
