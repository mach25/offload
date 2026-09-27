# ADR-0079: A phone runs its daemon only while needed

**Status:** accepted · 2026-09-27 · session ninety-three · amends ADR-0071 §3 (the app hosts its
node). A push "knock" was built under this number and withdrawn the same session; see the last
section before proposing one again.

## Context

ADR-0078 made a set-down phone quiet, but its daemon still ran all night. The owner did not want
that: "i sort of don't like that the app runs all the time". They added: "the entire system is
designed with nodes going away for extended periods of time", and "if a run is hosted it can be
active and show what runs it is running instead of Offload is running".

Two facts already in the tree made this small:

- **News for an away node waits.** Only the node holding a run notices its news and owes outbox
  rows. A row for a route whose node is not answering is deferred indefinitely, with nothing
  attempted and nothing spent ("a route that is away is not a route that is gone",
  `delivery-and-notifications`).
- **Absence is ordinary.** A node holding nothing loses nothing by being gone: there is no lease to
  fence, and it rejoins at its next start.

## Decision

1. **The Android app runs its daemon only while the app is on screen, or while a run is held
   here.** A 90 s grace after the app leaves the screen covers switching apps and news already on
   its way. Otherwise the service stops itself, SIGTERM drains the daemon, and the fleet lists the
   phone as `draining`, which is what a node that left gracefully looks like. The service is no
   longer sticky.
2. **ADR-0071 §3's "opening the app starts the daemon" holds on every return to the screen**
   (`onStart`), not only at `onCreate`.
3. **The ongoing notification says what the daemon is doing:** "Running: <the run's work>" while
   it holds one, from `NodeStatus::holding` (the supervisor's own held rule, `held_work`), and
   otherwise "Connected to your fleet". Never "offloadd running".
4. **A daemon that does not answer is not idle.** The service keeps it for up to 5 minutes with
   nobody looking, then stops anyway, so a broken daemon cannot keep a phone awake all night.
5. **No push.** A stopped phone hears its news when the app is next opened: the daemon starts, the
   holder's next pass finds the route reachable, and the waiting rows arrive over the app route as
   they always did.

## Consequences

- A set-down phone with nothing held runs no daemon at all. Measured on the phone: the service
  stopped itself about 70 s after the screen went off, and the fleet listed it `draining`.
- **The owner accepted the cost:** a question, a failure, or a finished run meant for this phone
  waits until the app is opened. A question still reaches any *other* device with a route. The
  owner called this "an ok trade-off".
- **Residual: a stopped phone does not host.** It cannot bid while its daemon is down, so it takes
  work only while the app is open, or while already holding something. Starting the daemon on
  charge (a JobScheduler job that requires charging) is the obvious next step, if the owner wants
  the phone as a host. **Asked in session ninety-four: the owner declined.** The phone hosts only
  while the app is open or a run is held. It could only ever host tasks: Claude Code has no Android
  build. If this is re-opened, know the constraint: on Android 16 a job may start the `specialUse`
  foreground service from the background only if the app is exempt from battery optimisation. The
  the phone's app is not (`dumpsys deviceidle whitelist`, 2026-09-27). Without the exemption the daemon
  would have to live inside the job, which the OS caps at about ten minutes, so the phone would
  keep joining and leaving the mesh.
- ADR-0078 now matters mostly inside the 90 s grace and for the multicast lock. It is kept: it is
  also what a phone that holds nothing but keeps the app open with the screen off needs.

## Withdrawn: the knock

Built and committed, deployed, and removed before it was walked. The owner declined it at the step
of installing the distributor app: "I don't want to install a third party on my phone I rather live
without notifications".

What it was: the phone gossiped a UnifiedPush endpoint (`NodeView::wake`). A holder whose delivery
pass deferred rows for an away node POSTed "wake" to it, through the owner's own ntfy server on the
Mac, backing off from 5 minutes to 4 hours. The ntfy app on the phone then woke Offload, which
started its daemon and received the waiting news over its own route. It was a knock rather than a
delivery, so ntfy never saw the news and a duplicate cost only a wake-up. HTTP and UnifiedPush were
both written by hand, with no new libraries.

Any push to an Android app needs a distributor app (ntfy) or Google's FCM. So **"notify a stopped
phone" is not available** under the owner's standing constraint of no third-party apps on the
phone. Do not propose it again without a way that needs neither. If one is found, the knock design
above is still the right shape: `git show 5656f34` has it whole.

## Amendment (session ninety-four): the owner may keep it running

The owner asked for "a setting to keep the app active even when backgrounding". Settings has *In
the background → Keep running when the app is closed*, off by default. On, the service's
keep-or-stop check (every 10 s) keeps the daemon for that reason alone, and the ongoing notification
says "Connected to your fleet". ADR-0078's quiet mode still applies with the screen off. The service
stays `START_NOT_STICKY`: if Android kills it, opening the app starts it again, and the setting says
so. Walked on the emulator: on, the daemon was still running 152 s after leaving the app; off, it
stopped after 96 s, as before. The choice is the owner's per device (a preference in the app, not
node config), so it neither gossips nor needs an owner beyond the device.
