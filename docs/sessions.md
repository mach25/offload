# Session history

Why the tree is the way it is, newest first. `docs/HANDOFF.md` is the live head — read this when
you need the reasoning behind something, not at the start of every session.

## Session ninety-four, a morning with the phone on the charger

It started by reading the phone's `batterystats`, as the list said to. What was on the phone was the
"before" night itself (reset 21:32, 10h55m), which session ninety-three had already read that morning.
The phone had been charging since. A dump that matches a documented figure to the second is the same
dump, not a confirmation, so check the reset time against the commit times before counting it as a
re-measure. The measure moves to tonight. Then the one owner decision on the list: should a
stopped phone host on charge? It can only host tasks, and Android 16 lets a job start the
foreground service only for an app exempt from battery optimisation. The owner declined. No code
changed.

Then the Mac, as a real host: a launchd LaunchAgent (`scripts/install-macos-service.sh`), and the
`host-runs` grant. The grant turned up a problem nobody had written down. The owner did not have
the passphrase. A session had founded the fleet on the laptop, and the passphrase existed only in
that `offload init`'s output under `/tmp`. They also asked why the tablet could not approve the
Mac. It was in a different fleet, and no approver can mint `host-runs` anyway. Three fleets had
been listed under "as left" as if they were deliberate, and two of them were test leftovers. At the
owner's word ("all we have ever been doing is testing") they were consolidated into `f1ee7001`: the
tablet's app was wiped and joined by link, and the test daemons were stopped. The lesson: when a
walk founds a fleet, give the passphrase to the owner, and retire the fleet when the walk ends.

The Mac's keep-awake failed on its first real test. The app left the Mac out of "Can take agent
runs", and it was right: the Mac had slept six seconds after it became a host, while holding
ADR-0077's assertion and while `pmset -g` said the assertion was preventing sleep.
`PreventUserIdleSystemSleep` does nothing for a machine that is already asleep and only dark-woken
by packets. It is `PreventSystemSleep` now, walked for five untouched minutes. Session ninety-two
had walked the assertion being *listed*. Nobody had walked the machine staying awake, and every ssh
used to look at it was waking it.

Then the model picker. The owner asked whether the app could list the models on offer, and then
designed it: read the list on each node at start, and let a person ask the fleet to read again when
a model is released. The probe's list had been a constant, and a stale one. Claude Code answers its
own `initialize` control request with the account's model list, offline and in two seconds, so that
became the source (ADR-0080, wire v37). The walk found two bugs in the request's path, both about
the counter that carries it. A device with no agent returned before passing the request on, and
that is the device a person presses Refresh on. The rule "the first count a node hears is old news"
also skipped a real request after a restart. Both are pitfalls now. The Mac did not take part: its
rebuilt binary lost macOS's Local Network permission, and that needs a click at its console.

Then the notification plane, because an open item said the Mac had given up on the stopped phone's
news in seconds, which contradicted ADR-0079. It had, and so had the laptop once, through a second
door. A send that got no answer was counted as a failed attempt, three of them in ten seconds, by a
node that had just restarted and still held the phone as alive. And a route missing from a
restarted node's view read as a route removed from the fleet. The fake fleet in the tests had
failed a *refusal* test with "did not answer", which is how the two cases had been one for so long.
Walked with the emulator standing in for a stopped phone: its overdue notice waited and arrived six
seconds after its app was opened.

The owner then tried the app for real, asking an agent from the phone to summarise their mail,
and asked for three things: who took the job and which model it used, more on the run's details,
and a way to keep the app running in the background. The first two were a list row that did not
carry the host or the model, and a detail sheet that showed only the log. The model the agent
actually used was already in the log's first line. The third is an owner's exception to ADR-0079,
off by default. Along the way: the SEEN column of `offload nodes` had been showing relayed news as
contact, and the mail run could not read mail because runs do not get the account's own connectors,
which is on purpose and is what ADR-0011's resources are for.

Then the cancelled run that tablet still showed as pending, which turned out to be pending on
every node but the one that cancelled it. The emulator had cancelled it before meeting anybody,
and a finished run was only gossiped for five minutes after it ended. Fixing that and walking it
found two more holes: a device that was away kept its stale copy for ever, and that copy could
reopen the run in another node's store, because "finished is finished" lived only in a view that
forgets finished runs. The walk caught the second one only because it checked the laptop's copy
as well as the tablet's.

## Session ninety-three, the phone left on the table

The phone lost half its battery overnight against the owner's usual 2%. `batterystats` put
Offload's own CPU at a tenth of it. The rest was the radio: a multicast lock held all night, and a
probe or answer about once a second waking the Wi-Fi chip. That is the right rate for a node
holding a run and the wrong one for a phone on a table holding nothing. The owner asked for
exactly that distinction, "a sort of hibernation mode", and pointed out that the whole system was
designed for nodes that go away for long periods. ADR-0078: a phone on battery, screen off, holding
nothing, is *quiet*. It is gossiped (wire v36), probed about once a minute, suspected only after
150 s, and releases the multicast lock. One trap came up before any walk: the host's facts go stale
*because* the phone is asleep, so the quiet decision reads them at any age. Whether a minute is
enough is the overnight re-measure's question. If it is not, the next step is not probing a quiet
node at all.

The owner then went further: they did not want the app running all the time, and reminded the
session that the system was built for nodes that go away. ADR-0079: the phone runs its daemon only
while the app is on screen or while it holds a run, and its notification now names the run. The
session also built a push "knock" through the owner's own ntfy on the Mac, as the owner had chosen
when asked. The owner then declined it at the step of installing ntfy on the phone, preferring no
notifications to a third-party app, and it was removed before it was walked. What survives is the
lesson that shaped it: read the delivery pass before designing a second path. News for an away
route already waited on its holder, so a stopped phone loses nothing, it only hears late.

## Session ninety-two, the host that fell asleep

The owner did not understand how a Mac could sleep a server away, and asked how other services
manage. They hold a power assertion, or are woken by the sleep proxy for TCP services they announce.
Offload did neither. The owner chose that Offload should handle it itself, then turned down Apple's
`caffeinate` as a helper process and then an outside crate. That left IOKit called directly, which
needs `unsafe`. At the owner's suggestion it went into a crate of its own, `offload-power`, so the
exception is one file in one crate, pinned by a test, and `offload-node` keeps `forbid`. The
assertion is held while the node may take work, by the same rule `accepting` reports. Walked on the
Mac with `pmset -g assertions`.

## Session ninety-two, overnight: why the Mac kept losing the laptop

The owner went to bed. The Mac and the laptop stayed apart for minutes after every laptop restart.
Measured in turn: my own detector changes from the evening made recovery slow; discovery threw away
the Mac's mDNS answer because the laptop already "knew" it, as dead; and addresses never travelled,
so the laptop could reach the Mac only over the Mac's own connection. All three are fixed, the last as
ADR-0076, wire v35, walked with the laptop reaching the Mac and the Mac reaching the phone at gossiped
addresses. Underneath was the Mac's own link, which drops: the kernel refusals had been in `offload
status` all along, and were read last, after four network theories each measured and dropped.

## Session ninety-two, looking inside a run

The owner wanted to see what an agent wrote without going to the machine that ran it. `offload
logs` already asked the node that ran a run for its log, so the files follow the same road: a new
request (wire v34), answered from the checkout while it exists and from the run's branch after. The
reading refuses anything outside the workspace, including a symlink the agent made. Found on the
way: the composer's new "who can do it" line reported "no device has an agent" for the second
before the node list arrived, which was unknown shown as none. It stays blank until it knows.

## Session ninety-two, why the phone kept dying

The laptop had marked the phone dead about 17,000 times. The first fix, not hanging up on a slow
answer, cured the emulator but not the phone. A trace of the real round trips did: the phone's radio
sleeps between packets, its answers ran to a second, and the 500 ms timeout threw away the slow
tenth of them. A comment in the code had recorded the same measurement months ago and quietened the
log. A timeout learned per peer took the phone to zero flaps. Dead nodes are now probed rarely and
never through helpers. That broke a storm property's idea of "at rest", which was widened with
the reason written beside it.

## Session ninety-two, the battery question

The owner asked how much battery Offload uses. Android's answer was the top of the phone's list. The
walk's trigger rule explained some of it: it tried to place a run nobody could take every 25
seconds, 1,433 times. But the daemon still used a quarter of a core idle once it was gone. The phone
cannot be profiled (Samsung's user build refuses perf events), so the laptop's node in the same fleet
stood in, and stack samples put the cost in probes serializing run progress. Every probe carried
every finished run the view had ever received, because the rule that replaces a node's own records
never drops one that nobody holds. The docs said only live and recent runs travel, and nothing made
it so. After the fix: 6%.

## Session ninety-two, where a person reads the work

The owner wanted run checkouts where they can read them, `~/offload`, and config in
`~/.config/offload`. State stays in `~/.offload`, because it is the node's identity. The trap was the
checkout sweep: it reclaims whatever in its directory no run of its own needs, so a *shared* default
would have had two daemons deleting each other's live checkouts, and the laptop runs three. The
directory now has an owner, and the second node refuses to start. The default is applied by
`offloadd` at start and never by `Config::default()`, or every test's checkouts would have landed in
the author's home.

## Session ninety-two, every device buzzing for every run

After ADR-0071 made each app a route, the owner saw every device notify on every finished run. The
laptop's `deliveries` table showed that agent runs were the cause, not the per-minute schedule,
which was already quiet under `Problems`. Asked, the owner chose: a finish goes to the device that
submitted the run, and problems go everywhere (ADR-0073). `Run::home` already said which device
that is. The rule sits beside `Audience` and `Notices` where the news is noticed, and the note
printed at submission now says where a finish goes as well.

## Session ninety-two, an agent run with no repository

The owner submitted a simple prompt from the phone with no repository. It was accepted and failed a
minute later on the laptop at `git clone ''`. Nothing checked that an agent run names a workspace,
and nothing offered one for work that needs none. ADR-0072 adds `scratch:`, an empty repository the
holder builds the way ADR-0061 builds an archive with no `.git`: content-addressed, so it migrates.
An empty repository is refused at submission. The app's field is optional, and it remembers what
was typed. The extraction of `mirror_from_tree` kept the archive's pinned commit, which the
existing test proved.

## Session ninety-two, re-approval through the tablet's key

The last unwalked piece of ADR-0069. A walk build shortened the approval year to 6 hours and the
member's retry to 20 seconds on both the laptop and the tablet, since the approver checks the window
too, and the source was reverted as soon as it was built. `reapprove` on the tablet recorded the
decision, the next ask filed a request, the owner confirmed, and the ask after that took the new
certificate: `reapproved=true`, issued by the tablet. Two things were found. The adb helper sent
`reapprove` to the socket, so it reported a member as outside any fleet. And `reapprove`'s sentences
restated the constants, which the walk build showed wrong; they now read the constants.

## Session ninety-two, the product app's key — approved on the tablet

The owner was at the tablet, so approving with the product app's own key came next. A fresh fleet
founded with `init --hardware-key` took the key the app made in the TEE. An invitation for a laptop
node was confirmed on the tablet's prompt, and the laptop joined on it and meshed. The first attempt
expired unnoticed, and there were two reasons. The approvals channel never vibrated, the same
omission the Questions channel had, and the tablet was locked. The harness app's daemon was also
running beside the product app's, from a launcher entry nobody could tell apart. The approvals
channel now buzzes, the harness is labelled "Offload harness", and `join --name` with `--token`,
which was silently ignored, is refused.

## Session ninety-two, the hardware key — approved with a touch on the tablet

The owner had the tablet beside them, so ADR-0069 §4 came next. A delegation may name a P-256 key,
and the tablet's app keeps one in its TEE, which needs a fingerprint or PIN for every use. The
daemon and the app meet over files in the state directory. The request carries only the unsigned
certificate. The app computes what to sign with the bundled `offload signing-bytes` and builds its
prompt from the same certificate, so there is no second implementation of the format.
`init --hardware-key` founded a walk fleet on the tablet. An invitation for a laptop node was
confirmed on the tablet, and the laptop meshed on it. **Wire v33, 1107 tests.** Also found: the
app never had a working button row (Android 15 edge-to-edge), and it crashed on its first prompt
(a missing `USE_BIOMETRIC`, with the exception uncaught). The owner asked mid-way for the real logo
as the app icon. It is now an adaptive icon built from `assets/app-icon.svg`.

## Session ninety-two, continued again — `offload reapprove`, and the cliff under it

The fleet upgrade came first. The laptop walk daemon was restarted on the current build, and the
tablet was updated and enrolled into the laptop's walk fleet, where it meshed. The phone is still on
an old build, off USB. On that build the laptop declared it dead about 30 times an hour while it
dozed unplugged, and it needs re-measuring once it is updated. `thermal none` now reads `none (not
throttling)`, because under `cpu not reported` it looked like a missing reading.

Then ADR-0069's step 3. `offload reapprove` records a person's decision on an approver, and a
member in its approval's last month asks over the existing `RenewMe` until an approver with a
decision re-approves it. So twenty machines take one command and nothing on any of them. No wire
bump. **1098 tests.** The node-level test crossed day 365 and failed on the founder's
*delegation*: it is issued only at `init`, for a year, and approver-issued certificates verify
against it as of now. So every fleet has a cliff a year after founding. A test states the current
rule on purpose, which makes it the owner's decision, and it is in HANDOFF. The walk found the
approver's view of its peers stuck at their handshake certificates, and a lapsed approver warned
about the wrong thing. Both are fixed.

## Session ninety-two, continued — an iOS app, and the Simulator found three bugs that were not iOS's

The owner asked for the iOS equivalent of the Android app. There is no iPhone, so it runs in the
Simulator on the Mac mini. An iOS app cannot spawn a program, so the daemon became a library
function, `offload_node::daemon::run(config, shutdown)`, and a new crate, `offload-ios`, exposes
`offload_start` and `offload_stop` to a UIKit app with no Xcode project (ADR-0070). The Mac's own
`offload` drives it over `/tmp/offload-ios.sock`. **ADR-0070 added; no wire or schema change;
1095 tests.**

Four defects, and only one of them was iOS's:

- **`offload_stop` returned before the daemon had gone**, so a quick background and foreground
  would have run two daemons on one state directory. Found writing the Swift caller. Stop now joins
  the thread.
- **A bid over the bidder's own connection was refused**, as "connection closed before its
  certificate could be checked". This was a regression from this session's own bidirectional
  sessions: `peer_certificate` looked only among dialled sessions. The rule for it was already in
  the detail file, written the day before by the change that broke it. The test meant to cover it
  passed for the wrong reason, and rewriting that test found a second hole: a closed dialled session
  was still returned.
- **A reinstall moved the app's container**, and the stale `state_dir` minted a new node identity.
  This one is iOS's. The app re-points the path on every launch.
- **Joining while the daemon ran needed a restart that nothing mentioned.** It is announced now.
  Doing it in place wants an ADR.

The Mac sync was first refused by the permission classifier, because it would have hard-reset the
Mac's checkout. The owner chose a fresh clone instead, and that is where iOS builds now.

## Session ninety-two — the first Linux↔macOS fleet, reached by measuring what an ADR said to measure first

The owner said *continue and do as many items as you can*. As in ninety-one, the pick-up list had
nothing one machine could close, so the session looked for what the list did not name. That turned
out to include the second machine. **ADR-0037 and ADR-0063 amended; no wire or schema change;
1073 tests; phase 10 done.**

**ADR-0037 §0 says to measure IPv6 before building any of the off-LAN plan. Nobody had.** The home
half took one command: this laptop holds a global `2001:db8:…/64` and goes out on it unchanged, so a
v6 phone needs no traversal to reach it. The same look found the fleet could not have used it.
`listen` defaulted to `0.0.0.0:7433`, and a throwaway test over the bind × dial matrix showed a v4
socket failing **both** ways on v6. quinn refuses the destination with `invalid remote address`
before a datagram leaves. The default is `[::]:7433` now, bound through `socket2` with
`IPV6_V6ONLY` set off rather than inherited, falling back to v4 with a warning. Walked on two local
daemons over `[::1]`, the global address, `127.0.0.1` and mDNS, with the old default as the control.

**Then the Mac, because v6 was a reason to look.** Session sixty-six had left it blocked on Local
Network access for ad-hoc-signed binaries. A freshly compiled ad-hoc sender now got 20 of 20
through on both families, and so did `offloadd`: a walk fleet met over the laptop's global v6
address from the Mac and over v4 from the laptop. Nobody recorded a grant, and re-measuring was
cheaper than the three sessions spent proving the block. The session then ran:

- an agent run on an archive workspace **submitted on macOS and run on Linux**, the bytes crossing
  the blob plane, followed to completion from the Mac;
- **ADR-0063 §2's weights walk**, open since session eighty-nine. There were six arms, and every one
  matched the arithmetic: a preference beats one reason (load, or a warm workspace) and yields to
  two, and the note printed `81 against 86: preferred -60, warm workspace +40, load +15, stability
  +10`. Two numbers were amended. A normal-demand node past 75 % load declines rather than bidding,
  so the load term tops out at −22, not 30. And floored memory doublings put a 15.5 GiB Linux laptop
  level with an 8 GiB Mac. The battery row stays unwalked, because the laptop was on mains
  throughout;
- a **task** submitted on Linux, placed on the Mac as the only node nominating it, with its output
  streamed back.

**What the second machine found, which one never could.** The Mac's `status` printed
`[::ffff:192.0.2.5]:7601`, because the dual-stack socket spells v4 peers as v6. That was this
session's own regression, fixed. And the laptop's log said `what this device is has changed` every
thirty seconds without saying what. Making the line name the fields said `disk_free_mb`: probed to
the megabyte, so every machine that writes anything bumped its incarnation and re-gossiped its
capabilities twice a minute, for ever. It is whole GiB now, rounded down, and measured under the state dir. It had been read under the
process's working directory, so a state dir on a 210 MB partition advertised the root's 266 GB. A partition also ran one run twice
(WARMUP finished on the Mac, the laptop had it `dead` and reassigned it). The fleet converged on one
record with one leg's 110 tokens, where 220 were spent. That is the handoff's old item about a finished leg that lost,
seen on real hardware, and it is left for an ADR.

**The network, which is what two machines add.** The Mac's `en0` drops for minutes at a time
(`status: inactive`, and Apple's `ping` says `Network is down`), and the laptop is dual-homed on one
subnet, so unpinned ssh fails. The mesh dropped 24 times in seventy minutes and healed every time.
ADR-0059's `sends … refused by this machine's kernel` line was right about the cause throughout.
An interleaved control showed the refusals come in bursts, not per process, which rules out the old
signing block.

**Then Android, on the SDK emulator**, after the owner asked whether one could stand in for the
phone. `scripts/build-android.sh` learned `ANDROID_ARCH=x86_64`. The headless emulator segfaulted
with every AVD and GPU mode, and the windowed one ran. `adb shell` needed `adb root` to bind the
control socket (SELinux, and not something Termux would meet). The node met the laptop across the
emulator's NAT, so **Linux↔Android has run**, though not on a phone. Two results came from it.
The battery row: with the console setting the charge, the term read −20 at 50 %, the table's
number. On a default phone it is always zero, since a phone accepts only while charging, and
charging is mains. And phase 8's whole sentence, which ran across the two with every report right:
a schedule, a trigger, a person's failing task escalated to an agent run placed on the laptop, and
the phone's sink carrying the news. One report defect was found on the way. quinn's GSO fall-back
on the virtual NIC was counted as the kernel refusing to send, for the life of the daemon.

**Then the owner's own phone, a Samsung phone on Android 16.** Under Termux first, which the owner
then retired as a product: *nobody is going to use Termux*. It found that an app is denied
`/sys/class/power_supply` (so a default phone refused all work while plugged in, and said it was not
charging, which was the wrong sentence and is now `ChargingUnknown`). It also found that Claude Code
publishes no Android build, that tasks need an environment under Termux, and that a backgrounded app
gets the little cores. Then **an Android app** (ADR-0066): the same `offloadd` from
`nativeLibraryDir` under a foreground service, platform facts from `BatteryManager` and
`ConnectivityManager` in `host-facts.json`, installed with adb and driven through `run-as`. Its
first run found that an app is denied `/proc/loadavg` and was reporting `cpu 0%`. On the phone,
**phase 5's demo ran in every clause but off-LAN**. And off-LAN answered ADR-0037's last
measurement: the carrier gives the phone IPv6 (`2a00:801:…`), so both ends are routable, and the
one thing between them was the home router's inbound IPv6 firewall. Back on wifi the phone
rejoined by itself within a minute. **The owner then added the rule**, one inbound UDP port to the
laptop from the carrier's prefix, and with wifi off the phone meshed from mobile data and stayed
`alive`. That is phase 5's demo, all of it, and ADR-0037 §2 walked with no relay and no NAT.

**Then phase 8's sentence over that network**: the phone on mobile data as the node with no agent,
with the schedule and a person's tasks from the laptop and the trigger, the escalation rule and the
sink on the phone. The escalated agent run was placed on the laptop, and one failing task gave one
escalation. Last, the owner mentioned that phone's plan is unmetered. Android calls every
cellular link metered, so `metered = "no"` in the phone's config was the owner overriding the
platform, as ADR-0045 intends, and a normal task then ran over mobile data. Three decisions the owner
took were built the same evening. A sleeping phone's suspicions went to `debug`, memory scores to
the nearest doubling, and ADR-0067 (a leg that lost still spent) was walked on two daemons with
`SIGSTOP` staging the fork.

**The morning after: the overnight flap was a peer, not Doze.** The phone had refuted its death
3 790 times. Bravo, left running by a `setsid` pid-file slip, could not dial the phone, and the laptop
answered bravo's indirect probe by dialling the phone rather than using the connection phone had
opened. So nobody but the node a phone dialled could ever vouch for it. A connection now carries
streams both ways, `session()` reuses a peer's inbound connection, and a certificate objection hangs
up both ways, a regression a new test caught first. Walked: 0 deaths in five minutes, with bravo,
the laptop and the phone on mobile data.

**Thermal state (ADR-0068)** closed phase 5's probing item for Android. The app writes
`getCurrentThermalStatus` beside battery and metered. It is observed pressure like load, so it goes
to the same gate, graduated by demand, and never into `Capabilities`. It was walked with Android's
own test hook, `cmd thermalservice override-status`, at `SEVERE` and `MODERATE`, from the phone and
from the laptop. iOS is blocked on Xcode, which needs the owner at the Mac.

**ADR-0069, from a proposal to two built steps in one afternoon of conversation.** The owner reshaped
it three times: no manual renewing, no device the fleet depends on, and suspicion of old
certificates (the stolen-shuttle code). The result is renewal as a routine every member may do, and
an approval that lasts a year. Step 1 kept every existing certificate's signed bytes identical
(versioned messages, pinned). Step 2's walk, with four-minute certificates, found that the second
renewal over any long-lived connection had always failed, because the renewer restated the
certificate the handshake had seen. That bug predates the ADR; `RenewMe` now carries the asker's
credentials. A Samsung tablet joined as a second Android device, and the probe learned from the app
that it is a tablet.

**Measurement besides.** After ADR-0067 and the memory rounding, the three hard property runs were
re-run overnight and all passed (20 000 / 20 000 / 600 cases), with the order-independence property
now generating per-leg entries. The ADR-0052 race test: 0 in 800. The three hard property runs all
passed, at 20 000, 20 000 and 600 cases. One `offload-agent` test failed once and not in 1 280
reruns, and its message was lost to my own output filter, so it is a residual with a labelled guess.
Two traps of my own went into the pitfalls. A background sweep's load made the laptop refuse the
round being measured. And `kill -STOP $(pgrep -f …)` stopped my own shell, which is the `pkill -f`
trap with a fourth verb.

## Session ninety-one — four reports, three of them on screens nobody had stood at

The owner said *continue and do as many items as you can*. The pick-up list had no defect and no
item a single machine could close, so the session took the cheap residuals and walked each one.
**No ADR, no wire or schema change; 1069 tests.**

**The daemon never said its grants changed when only the clock moved.** The handoff called it
"says nothing when it adopts a certificate". The line existed; it fired from `FleetChange::grants`,
a diff of `fleet.json` against memory, and neither a probation lifting (same bytes, later clock) nor
a certificate the daemon adopted itself (eaten by `adopt`'s own `reload`) changes the file. The
`ourselves` edge from ADR-0044, a second time. Walked with `PROBATION` at 20s: the fixed build logs
`grants=submit,deliver,host-runs` twenty seconds after the grant, and HEAD logs nothing across the
same 28 seconds. The unit test that had asserted the grant was in force passed only because
`reload` read the wall clock, years past the fixture's `NOW`.

**Three daemons for the two `explain` vantages session eighty-six left** — a run held by a third
node, and a node that has never met the run's home. Both screens were mostly right, and the walk
found three sentences that were not. A bystander said `charlie  no answer` about the healthy holder:
on loopback it had never had an address to dial, and `ask_one` folded the transport's reason into
silence (`Verdict::Unreachable`). The `due` line told a running run it was waiting for a node to take
it. And `ArbitratedBy(None)`, reached by restarting a bystander after the home was `kill -9`'d, is a
standing state and not the "first second of gossip" its comment said — measured healing the moment
that node met a peer that remembered the home, so the sentence now says how it ends. The staging
had its own trap, now in `docs/DEMO.md`: two joiners seeded only at the founder are partitioned the
moment it dies, and bravo orphaned charlie's run while charlie carried on.

**Then the three forwards session ninety had named and not measured.** On a dead holder, `offload
checkpoint` and `offload approve` each waited 2.0s and said the holder *is running this run*, beside
`ps` saying `orphaned`; `offload cancel` refused the same run in 0ms. One check now, read by all
three doors, and `continue`'s own view check is the same rule.

**Two pieces of paperwork.** Sixty-three detail entries backfilled across twelve pitfall files,
all fifteen paired by subject (three turned out complete): a read-only scout per file paired rules to
entries by subject and quoted a source for each gap, every quote was grepped before anything was
written, and the seven rules with no source beyond a commit diff are named rather than filled. One
scout noticed what the count had been missing — some detail entries are `## ` headings, not
bullets — which is why this session first thought session eighty-seven had used a different
method. It had not; this one had counted wrong. And pairing by subject showed the count overstates the gap:
`checkpoints-blobs-and-workspaces` read 48/30 and was missing five; one entry often covers several
rules. And `docs/HANDOFF.md` went from 98 KB to half that — the state
section had become one paragraph per session since about session sixty, all of it already here.

## Session ninety — phase 10's residuals, and the rows a fence writes

The owner said *continue and work as many items as you can*. Phase 10 stays at 13/14 — its one open
item needs two machines — so the session took the residuals and a carried walk. **ADR-0065**: every
spawn runs with `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, applied last in `child_env`. The lead ADR-0064
left as "empty directories only" was measured: Claude Code keys auto-memory by *repository*, so a
run in one worktree of a mirror saved a codeword and a fresh run in a second worktree answered it
in one turn with no tool call. Placement changed what a run knew — `--strict-mcp-config`'s argument,
about memory rather than reach. With memory off, the agent saved its note as a file in the
worktree, which the checkpoint carries. $0.093 on `~/.claude-alt`, folders removed.

**Wire v31: `offload continue` typed on any node.** The node it is typed at asks the one that ran
the parent's last leg for the *base* only, fetches the blobs it names, and makes the run itself —
so home and `--prefer here` are the person's machine, which forwarding the whole request would
have got wrong. The walk found two reports: the refusal read *"bravo answered: run … cannot be
continued: … no longer on this node"* at alpha's keyboard, and a parent whose node was killed took
30 s to time out while `offload nodes` said `dead`. On the way in, the same first-person shape in
the handshake: *"this node speaks v30, the peer speaks v31"*, printed on the v31 node — which
sent the walk looking for a stale binary on the wrong machine (it was a stale *alpha*).

**The audit rows only a fence writes, read on a screen for the first time** (carried item 16), by
freezing alpha with `SIGSTOP` until bravo was granted its run. `superseded` named bravo by id and
told a leg with no turns that "the turns after this one are not the run's"; and with the agent's
turn ending *inside* the freeze, alpha thawed, read its agent's result before the gossip, completed
the run at epoch 1, and was overwritten by bravo's epoch 2 with **nothing on alpha saying so** —
the rule that records a loss asked for a live agent. `Superseded { finished: true }` now. `refused`
is still unread: no staging made alpha attempt a write after it had learned, which is the fences
working. Also walked, never walked before: `--session`'s refusal for a parent with no transcript.

Then, asked to go on: two more report defects of the same shape. A pin refused with `is node
2a3b5d5d (this is 29be31be)` on a row headed `alpha`, and every clause printed Rust's names — `os
== MacOs (have Linux)`, and `have Battery { percent: 57, charging: false }` for `mains`; the bid
round has the view, so the node clause is named from it, and the capability enums have a `Display`
in the grammar's spelling. And `offload explain` on a parked run told the reader three different
things about what it waits for (HANDOFF item 17); the first fix asked supervision and only the
arbiter's screen changed, so "parked" is now `offload_core::parked`, a fact about the record. The
pitfall rules this session added got their detail entries.

## Session eighty-nine — phase 10, built and walked

The owner said *continue*, then *finish as many work items as you can*, and phase 10 went from
0/14 to 13/14 in one wire bump (v30). **ADR-0063**: `RunSpec::{prefer, hold_until}` and
`Constraint::Node`, which only a node that knows its own id can answer (`matches_on`); `here` and
`node=` parsed by the CLI and resolved by the daemon, a rule resolving them when it is written;
`--hold` as a duration because the CLI has no timezone; `ScoreTerms` so the three reports read the
numbers the round compared. **ADR-0064**: the closing message kept on `LogKind::Finished`, and
`offload continue` — the parent's worktree captured read-only into a checkpoint at turn zero, which
is how the start path tells it from the continuation's own; a handoff heading composed at spawn; the
parent's transcript as a blob the spec names, materialised at `.offload/parent/transcript.jsonl`
and kept out of git by the mirror's `info/exclude`.

The walks found four things the tests could not. A held-off node refused with two node ids and no
word of a hold (`NoBid::Held` now). The heading was baked into the prompt, so `offload ps` printed
it. **A peer's offer lost its score terms crossing the wire** — `ClusterMessage::Bid` had no field
for them — and the note called the winner outscored by itself; every unit test built its opinions
directly and so never crossed. And with Haiku, the continuation that needed the parent's transcript
could not `Read` it — one line was 38k tokens — and ran out of turns grepping; one sentence in the
heading saying *search it* took it to the right answer in three turns. The control continuation
never opened the file, which is the demo. $0.13 on the `~/.claude-alt` login; the agent project
folders it made were removed. Also corrected: `MembershipCert::probation`'s doc said an invite
skips probation, which it does not when the invite grants `host-runs`.

## Session eighty-eight — two ideas, and where work should run

The owner captured two ideas — an IMAP poller, and an agent that primes a job with project context
from designated folders — and they were explored against the tree into
`docs/use-cases/primed-work-from-a-mailbox.md`. No code changed and nothing was walked. The poller
is a trigger and fits once *already handled* lives in the mailbox (claim keyword, move on success),
because a program that marks mail seen on emit loses every message dropped by the one-in-flight
gate. The primer is ADR-0061's submitter run unattended, and the one gap worth an ADR is **a run
that submits a run**: recorded as `Origin::Operator`, which by reading walks around ADR-0057's
notice-loop guard and can double-submit across a resume. The owner then corrected the direction: the answers kept designing for work to *leave* the
machine, and a job may as well run where it was submitted, where it suits, or where they will be
later. That became **ADR-0063, proposed** — a preference is scored, never filtered — and the
use case was rewritten to start from local placement, with context routing (folder contexts as a
resource, git contexts cloned by the running node) and a dispatched session in place of the
primer. Their last point, that Remote Control needs a session already open, found the sharpest
gap: **a finished run cannot be resumed** (`run.rs:953`), so a dispatched session answers once. That became **ADR-0064, proposed**: a continuation is a
new run with a parent, and — the owner's refinement, mid-draft — it is handed a *summary* by
default rather than the whole session, because a prompt is portable across agent versions,
accounts and kinds and a transcript is not. The summary is the parent's own closing message,
moved verbatim; Offload writes none, which keeps it on the right side of "never manage
conversation state". And one step further: the parent's transcript is saved as a file in the
continuation's workspace to read on demand — fresh context, history one grep away — and naming
that file in the spec is what keeps it past the collector, for exactly the runs that were
continued. Then ADR-0064's four checks, run on git and a real Haiku agent for $0.12: three held, one
corrected the ADR (the closing message is dropped, not stored), and one was confounded on the first
try — the "resumed" leg had read the parent leg's auto-memory file by absolute path, which is now a
pitfall entry. Deleting it and forking again gave the clean result. The owner then accepted both ADRs, and they became phase 10 in the roadmap — one wire bump
for both, since each changes `RunSpec`.
Also noticed and not fixed: `HANDOFF.md`
says phase 9 is 0/7 while the roadmap and the CLI have §1–§3 built.

## Session eighty-seven — the third log, and a path nobody could have typed

**`offload audit`** was the subject: the per-node log of what this machine decided, which session
seventy-seven called *"the worst rendering in the CLI"* while fixing one thing in it during a sweep,
and which no session had walked. It is the pair to session eighty-six's `offload explain` — that one
answers *what is true now*, this one *what this machine did, then* — and it is the log that has to
work when the run's own log is gone. Four defects, one of them found only by the sweep the other
three prescribed.

**The timestamp was UTC and said nothing about it, under a doc comment saying it was local.** A run
submitted at 15:20:19 CEST appears in `offload audit` as `2026-09-12 13:20`. Two hours out, with no
`Z`, no offset, no marker, in the two commands whose entire job is placing an event in time after
the fact — the audit log, and `offload nodes --history`, which is ADR-0012 mitigation 4's durable
half. Anybody asking *did anything happen around three* was told no, silently. The comment is the
lesson rather than the arithmetic: it asserted the one thing the code did not do, which is precisely
what stops a reader checking. It says ` UTC` now, which also makes the column comparable with
`offloadd`'s own `Z`-suffixed lines. Converting to local was considered and declined — no timezone
in `std`, so it means a dependency this crate does not have or a TZif parser, and each has ways to
be wrong that a label cannot.

**The listing stopped at a hundred rows and the closing line was about somebody else.** Sixty
submissions, exactly 100 rows, the output simply ending — while the sentence underneath said *"What
this device did, not the fleet's — ask the others too"*, which is a real gap and not the one in
front of the reader. The `--help` promised the opposite outright: *"Omit for everything this node has
recorded"*, which the command has never done. It asks for one more than it shows now, so *there are
older rows* is a fact rather than a hedge; `offload nodes --history` had the same cap and matters
more, being what somebody opens to find out whether a device was enrolled without their knowledge.

**And the one that is worth the session on its own: the durable copy of a fact was the vague one.**
A rescue — a checkout from an earlier leg, kept because it holds uncommitted work nothing captured —
rendered as *"it is at `<state>/worktrees/<run>.superseded*`"*. Literal angle brackets, in the line
that says where the only copy of somebody's work is. The **run's own log** printed the real absolute
path for the same rescue, and that is the log which does not survive: `Rescued`'s own doc comment
says the rescuing leg is often not the node anybody can read the run's log from, and once the run is
deleted nobody can. The id was the sharp part. It is on the audit row already, abbreviated to twelve
by `id_width`, and the directory is named by the full thirty-two — so a reader who understood the
placeholder and substituted what was in front of them got a path that does not exist. Measured, with
`ls`, both ways. `run` is in the variant; the state directory is not and cannot be, so it is named in
words rather than spelled as a token.

**The fourth came from typing this file's own rule.** *Grep for every site that renders it before
writing the fix* — run over the whole CLI, it found `offload logs` printing `run released — resume
it with: offload resume <run>`, forty lines in the same `match` from the arm whose comment reads
*"The run, spelled, rather than `<run>`: this is the one line in the log that is an instruction."*
There were two, and the sentence asserting there was one is what stopped anybody looking. Session
seventy-four fixed the line that was in front of somebody; this is the other one, three sessions
later. So there is a lint now — `offload-cli/tests/no_placeholders.rs`, modelled on
`offload-core/tests/no_clock.rs` and for its reason: what these three share is a **shape** and not a
subject, a reviewer reading one arm will not find the next, and there is no type that says *this
string reaches a person*. Proved red by putting the line back.

The staging is the other thing worth keeping. `rescued` and `reclaimed` do not need the three-leg
ping-pong: `adopt` decides from a turn marker beside the worktree, so parking a run with `offload
checkpoint`, lowering that marker by hand and resuming produces exactly the shape a migration away
and back produces — with an untracked file for the rescue and without one for the reclamation, which
is its own control. Twenty seconds on one daemon, and it is in `docs/DEMO.md`.

1047 tests, clippy clean, wire v29, schema v12 — eighteen sessions and no bump. No ADR: every one of
these is a rendering, and the decisions behind them (per-node, never gossiped, the run id as the
whole address) are unchanged and were right. One new test file, which is a lint over source rather
than a test of behaviour, and says so in its own header.

## Session eighty-six — one run, two machines, and the screens that did not agree

**`offload explain`** was the subject, for `offload ps`'s reason one session earlier: it is read in
nearly every walk and had never been walked itself. The difference is that it does **not travel**.
`cancel`, `checkpoint`, `logs`, `deadline` and `priority` each go to the machine that can answer;
`explain` answers from the node you typed it on, and five of its lines — `held_back`, `waiting`,
`recovery`, `watchers`, `resume` — are computed only where the run is. So the walk was not a set of
states, it was states **times vantages**: two daemons, the second enrolled with `{submit, deliver}`
and never granted `host-runs`, so every run landed on the founder and the peer was a pure reader.
Running, `assigned` behind `max_concurrent_runs = 1`, cancelled, completed, failed, a task at each,
parked by `offload checkpoint`, blocked on an `--ask`, orphaned by `kill -9` — read from both
machines, whole screen each time. Most of it was sound, and the two that paid were the two where
the report knew something one line away from where it said the opposite.

**A failed run was told, on adjacent lines, that nothing was supervising it and that it would be
picked up in 29 seconds.** `supervise` answers `Bystanding::Terminal` for all three terminal
states, correctly — the drop-off loop is done with any run that has stopped — and `offload explain`
rendered one sentence for all three. `Failed` is the one ADR-0013 reopens, and the recovery tick
*is* supervising it; the tick's own answer was four lines down the same output. Measured twice,
once per tier, and watched through: the run was `running` again 29 seconds later. This file has
recorded the same rule twice before (`Drained`'s counts, `is_terminal` in the drain's settle pass)
and the tell is now worth stating plainly: **a single sentence hung on a predicate that covers
several states is right for at most one of them.** The `Failed` arm now says only what that loop
decided and then names **a machine** — *whether a failed run comes back is decided on the node it
failed on* — rather than pointing at the `recovery` line, which exists on that node alone.

**A failed task was invited to be resumed, by two reports, with a fleet grant named as the fix.**
`offload ps`'s footnote and `offload explain`'s `resume` line both gated on
`supervisor::resumable_state`, the **door's** predicate, which is deliberately tier-blind because
`restart_task` admits a run by the same states. ADR-0058 gave a failed task its own verb and its own
door: it is restarted from its spec, never resumed, and `offload resume` refuses every task on every
node for ever. So on the peer, a failed task read *"resume refused here right now: this node has not
been granted host-runs — run `offload grant host-runs`"*, and on a draining fleet of one whose only
failed run was a task, *"a run above says it is resumable, which is true of the run"* — which for a
task is false. The control is the sentence that matters: on the node that **had** the grant,
`offload resume` on that same run answered *"it has no checkpoint — there is no conversation to
continue"*. A node-level, temporary-sounding refusal standing in front of a run-level, permanent
one. `resumable_by_hand` is the reports' question — the door's states **and** the tier — and the
door keeps the tier-blind one it shares with `restart_task`.

**And the one that was a reasoning error rather than an oversight.** `offload explain` computed the
ADR-0017 `waiting` line on the holder and left it empty everywhere else, with the reason written
down beside it: *"for `held_back`'s reason carried one step further — a blocked process exists on
exactly one machine."* `held_back` is **arithmetic**, this machine's occupancy and its own account's
rate limit, which no peer could compute. A pending question is a **fact its holder can be asked
for**, and `offload asks` has canvassed the fleet for exactly that since ADR-0017 — whose own text
gives the justification as *"`offload explain`'s reason"*. The precedent it cited was the one report
not doing it. Measured: the peer's `offload explain` showed no sign of the block at all while
`offload asks` on the same socket printed the tool, the clock, `WHERE alpha` and the `offload
approve` line — which travels, so the person reading the silent screen could have unblocked the
agent from where they stood. One canvass, same window as the opinions round, skipped for a terminal
run. **When a report declines to answer, check the reason is the one that applies to it and not the
one that applied next door.**

Fixing that uncovered its other half: with the block visible off the holder, `attendance_now` put
*"it is running there"* one line above *"stopped for an answer"*. That arm has now over-claimed
three times — `Assigned`, `Orphaned`, and this — and the first two are readable from `run.state`
where this one is not, which is why it could only appear once the fact arrived.

All three walked against builds with the fixes reverted on the same state directories, with a
control beside each: the failed **agent** run keeping its resume line in the same listing where the
task lost it, and `Completed` keeping *"it is over"* where `Failed` stopped claiming it. Three
`docs/DEMO.md` notes came out of the staging and two of them are about instruments: a fake agent
that does not answer `--version` makes the probe run the whole script (a 48-second startup with no
socket, which reads as a daemon that failed to start), and `pgrep -x agent.sh` — the instrument
session eighty-five recommended — counts **zero** for a script with a `#!/usr/bin/env bash` shebang,
because the kernel runs it as `bash /path/agent.sh` and the process name is `bash`.

**And the fixture mistake this session paid for, recorded because the next one would repeat it.**
The fake agent's turn loop ends in `git add -A && git commit`, which is the shape every migration
recipe in `docs/DEMO.md` uses. The probe runs the configured agent binary with `--version` — in the
**daemon's own working directory**, which was the project repo — and the first version of the script
ignored the flag and ran its whole twelve-turn loop. Twelve commits landed on this tree's `main`,
between session eighty-five's commit and this one's, before there was a socket to notice anything
with. They are reverted in a commit of their own rather than rebased away: nothing here is pushed,
so either was available, and a history that records the mistake is worth more than a tidy one. The
guards are in `docs/DEMO.md` and the first is enough alone — answer `--version` and exit before
anything else. The second tell was there and unread: `TAG=$(basename "$CLAUDE_CONFIG_DIR")` came out
`.claude-alt`, this machine's own agent directory, because `[agent] config_dir` is applied when the
daemon spawns a *run* and not when the probe asks a binary what it is.

1044 tests, clippy clean, wire v29, schema v12 — seventeen sessions and no bump. No ADR: the verdict
and the tier were both already decided, and the canvass is what ADR-0017 §"`offload asks` canvasses
the fleet" already argues for. No field added and no shape changed on the wire; `Explanation` carries
the same fields and two of them are now populated in a case that used to leave them empty.

## Session eighty-five — three agents on an install the node itself says sustains two

The machine was free again, so this walk could place runs: **`offload ps` and `offload logs`**, the
two reports read in every other walk and never the subject of one. Both are sound — `-f` streams
and ends cleanly when the run does, a peer's log forwards, the columns line up, `TOKENS` and
`COST` fill in from the transcript and the `result` line. Two apparent defects in `ps` turned out
to be the fixture: a `TOKENS -` on a completed run was my fake agent writing transcript rows with
no `usage`, which `docs/DEMO.md` already warns about, and the checkpoint failures beside it had
the same cause. Measuring is what showed it.

**What the walk found was in a submission line.** A third run was told *"starting when one of the
2 runs ahead of it finishes"* and started immediately. That sentence comes from the winning bid's
`Availability`, and the bid was right: `offload status` said `runs 3/3 · claude-code sustains 2,
which is what binds`. Three runs were `running`, and `pgrep -c -x agent.sh` said **3**.

`WorkPolicy::admits` has four capacity clauses and one of them is the agent install's own ceiling —
`caps.agent_details(kind).max_concurrent`, which `deliver::set_agent_concurrency`'s own doc calls
*"a cap"*. `Room::for_one_more`, which every path that starts an agent goes through, has three
clauses and that is not one of them. And `admits` has exactly one caller: `bid::evaluate`. So the
ceiling shaped what a node **bid** and bound nothing, and `Refusal::AgentAtCapacity` was a refusal
that could be computed, was rendered in the gossiped summary as `agent is full`, and could never
stop a run.

The sharpest staging is the one with **no cluster at all**, which is the mode most of this
project's testing uses: there `admits` is never reached, so the ceiling was asked by nothing.
Three submissions, three agents, not a word.

**Four doors, which is the count that keeps being the lesson.** `Supervisor::agent_full` is asked
by `submit_built` (the tail of every submission and the whole of placement with no mesh), by
`start_refusal` — which `take_run` now calls instead of its own second copy of the capacity
arithmetic, and that second copy is exactly how a clause gets added to one start path and missed
by the other — and by `resume`, the door a person types at *and* the one the recovery tick uses
unasked, which is ADR-0047's argument met again one ceiling over. `restart_task` is the fourth
caller of that shape and needs nothing, because a task names no agent.

The **count** each door passes is its own: a submission asks what is committed, a start asks what
is going. That is the split `Room::for_one_more`'s callers already draw for the machine's capacity,
and passing it in is what keeps the ceiling from becoming a third opinion about which runs to
count. And the supervisor had to be told what the device *is* — the ceiling is a capability and it
held none; `Arc<deliver::Current>` is wired at startup, the same shape as `Supervisor::room`
overlaying the rate limit it alone observes.

Two runs on the fix, two agent processes, the third submission refused with *"agent claude-code is
at its per-node concurrency limit"* — and on two daemons, where a grant holds rather than refuses,
five submissions drained two at a time with nothing stranded. That last is the regression check
that mattered: a new refusal on a start path is exactly how a held run stops ever starting.

## Session eighty-four — an empty expression that matched everything, and a heading that claimed authorship

The machine was busy with its owner's own work — an Oracle container, a mockserver, a Spring Boot
suite — so every submission was refused `cpu at 100%`, which is the previous session's note doing
its job. That chooses the walk: **`offload probe`, `offload policy` and `offload match`** need no
daemon and no runs, they share `probed()`, and two of the three had never been the subject of one.
Walked as a family, which is session eighty-one's lesson.

**`offload match ""` said the device satisfies the constraint, and exited 0.** Its own help says to
compose it in scripts, so that is a gate that opens on an unset shell variable — answering the
permissive way, silently. `parse("")` split on commas, filtered the empty pieces, found nothing
left and returned `Constraint::Always`.

It was **tested**: `empty_input_matches_everything` asserted exactly that, so this is a decision
being changed rather than an oversight being fixed. What the decision missed is the caller count —
`constraint_expr::parse` has exactly one caller, whose `expr` is a required positional, so the only
way to reach the empty case is to type an empty string and there is nobody for whom that answer is
useful. And `ParseError::Empty` already existed and was **unreachable**, because `clause()` raised
it for an empty string while `parse` filtered the empties out first: the second time in three
sessions that a variant with no constructor was the fix that had not landed.

**The same parser required a value only where the value had to be a number.** `cores>=` was refused
by `number()`; `toolchain:rust>=` quietly meant *any* version and `tag=` meant a tag named nothing.
One rule applied to a third of the clauses, and it is in `split_op` now, where every clause passes.
And a bare word was told it is a clause that needs a value — *"clause `nonsense` needs a value like
`cores>=8`"* — which sends somebody to put a value on a clause that does not exist. The parser
cannot tell a known clause from an unknown one there without a second copy of the clause-name list,
so it says what is true of both and names the two that legitimately stand alone.

**`offload policy` claimed the owner had said three things when they had said one.**
`[policy.light]` is three independent overrides and `None` inherits — ADR-0019 §4 says so in as
many words — and a block naming `accept` alone still printed three lines under *"the owner has said
what this device does with it"*, with the **main** policy's battery floor among them. It changes
when that one changes, without the light block being touched. What the existing code gets right is
where the values come from, and the fix does not touch it: the heading was a claim about
**authorship**, and answering that needs `policy.light`'s `Option`s, which is a different fact from
what the value is.

**And it printed a spelling its own config rejects.** `accept work WhenCharging` via `{:?}`, while
`accept = "WhenCharging"` fails to parse. The config's refusal is a good one — it lists all three
spellings — so the cost was a detour rather than a dead end; it is still the command whose whole
job is saying what the policy in force *is*. The sweep behind it came out clean otherwise: every
other `{:?}` in operator output round-trips, because `offload match`'s parser is case-insensitive,
which was typed rather than assumed.

## Session eighty-three — a route that had given up was reported as never used

`offload sinks` is read in most walks here and had never been the subject of one, so this session
walked the **delivery plane** — the thing that wakes a person. Four `[[sinks]]` on one node and
none on the other: two that work, one whose script `exit 7`s, one whose program is not there. The
mechanism is sound throughout — news from a run on the node with no routes reached the other node's
phone and mailbox, the failing route was retried and then given up on, the missing one was listed
and never attempted, and `--test` distinguished the two. Five of the sentences around it were not.

**The one worth the walk.** After a single notice had gone round:

```
flaky   webhook   0   0   1   usable, never used — try `offload sinks --test`
        └─ last failure: gave up: /tmp/ow83/flaky.sh exited 7: no output
```

`DROPPED 1` in the column, `gave up` in the line underneath, `never used` in between. The state
matched on `(unusable, delivered, waiting)` — three counters, two of them matched, and the third
asked about only in a guard **below** the arm that swallows it. That guard's own comment names who
it is for: *"the route looks healthy and somebody was not told something"*, and it was unreachable
for exactly the route that has told nobody anything. A guard below an arm that already matches is a
guard for rows that never arrive.

It is a pure function of three numbers that lived inside a `println!` loop, so nothing could have
failed on it — session eighty-one's rule met again. `sink_state` is a function now, and its test
reproduces the old string against the old arm order.

**And the tables disagree with each other.** `offload sinks` prints this node's routes and then the
fleet's, in one function, twenty lines apart, and they ask the same three questions in different
orders: the local one *delivered anything?* before *gave up on anything?*, the fleet one the other
way round and correctly. Each reads perfectly well alone. Both on one screen, for routes in the
same state, is what showed it — and both said `1 were given up on earlier`.

**A peer was told about a credential.** The fleet table renders `!authenticated` as *"alpha says
its credential does not work"* — for a `[[sinks]]` entry whose program is simply not on that
device, while alpha's own table one command away said `not found on this device`. That bit has a
different meaning per role and more than one cause within a role. The **measured** reason was
already crossing the mesh: since ADR-0019's amendment the nominated kinds put label-then-reason in
`Capability::description`, `FleetRoute` carries the string, and the fleet table printed no
description line at all. So the honest sentence was being thrown away while the column beside it
guessed. `Undeliverable::Unauthenticated` made the same claim one crate down, in `offload-core`,
which holds the bit and not the machine.

**Two smaller ones.** `offload sinks --test` said it had tried *each usable route* while the
handler tries every one and lets the attempt decide — using `usable` for a different question from
the one the rows answer with the same word, which the `exit 7` route demonstrates by changing state
between the two commands. And a node with no routes of its own printed a column header with nothing
under it, directly beneath a line saying it had none.

**Two things about the walk itself.** The load-average trap has a second face: this machine's owner
was running an Oracle container, a mockserver and a Spring Boot suite, so every submission was
refused `cpu at 100%` with nothing about the fleet wrong and nothing to wait for — a delivery walk
can be driven by `offload grant` instead, since a fleet event goes to every route. And
`git checkout -- <file>`, used to undo a one-line `if false` after proving a test goes red,
discarded every edit made to that file in the session; the previous session's `cp` recipe is the
one that works and this one forgot it.

## Session eighty-two — two names for one machine, and an empty string that cancelled a run

The pick-up list's top item is still a click at the Mac's console, so this was the next unwalked
command: **`offload cancel`**, the most-typed mutating command nobody had walked as a subject. It
travels, it writes a terminal state, and it has three outcomes plus two kinds of target. The
mechanism turned out to be sound in every arm. Everything found was around it, which is the twelfth
walk in a row that has gone that way — and the first one was visible in the very first screen.

**Two nodes, and both of them were called `fedora`.** `offload explain` on the arbiter, for a run
placed on the peer, printed `holder fedora`, `arbiter fedora (this node)`, `attendance only fedora
can tell`, and a canvass listing `fedora` twice — once bidding, once already holding the run. Five
occurrences of one name for two machines, in the report whose whole job is saying which machine has
the run. `offload nodes` a minute earlier had said `bravo`.

**The staging was the default configuration, not a contrived one.** `offload init --name alpha`
names the *certificate*; `[node] name` is a separate optional field that defaults to the machine's
**hostname**. Three places in the tree say the certificate's name is what the fleet displays —
`main::report_membership` warns about it at startup in as many words, `Cluster::record_name` writes
it at the handshake, and ADR-0012 puts a name on the certificate *"for `offload nodes`"*. And then
`merge_node` copies a peer's `NodeView` wholesale above its own incarnation, name included, while
`set_capabilities` bumps the incarnation on every probe that finds the cpu load has moved. Measured
with a five-second poll: `bravo` for **25 seconds**, the hostname for ever after. A signed fact and
an unsigned one about one field, and the merge let the unsigned one win.

Corrected **on the way in** — `absorb` rewrites an incoming name from the certificate this node
holds, before the merge — rather than repaired afterwards: `merge_node` is `offload-core` and has no
certificates to consult, and nothing is briefly wrong. A peer learned by relay keeps its gossiped
name, which is the only one there is. And the node's **own** entry is the same fact from the
inside: it is what `Cluster::name_of(self.node())` answers, which is the `by` on a forwarded cancel
— so a run's log said `cancelled from fedora` on the machine that ran it. `fleet::display_name` is
the one answer now, read from `fleet.json` rather than cached, because `offload rekey` rewrites it
under a running daemon.

**No test could have caught it**, and that is the reusable part: every helper in
`offload-cluster/tests/mesh.rs` builds a node with one name and uses it for both the certificate and
the view entry. A fixture that cannot represent the disagreement cannot fail on it. `join_named`
can, and the new test goes red with `left: Some("fedora")` against the merge with the correction
forced off.

**Then `offload cancel ""` cancelled a run.** Typing the empty argument is an arm session
seventy-six already listed as worth trying, on a different command. `Store::resolve_run` refuses an
empty needle and a non-hex one, with the reasoning beside it — resolving rather than refusing is
*"the helpfulness that cancels the wrong job"*. `server::find_run` falls back to scanning the
cluster view when the store says no, deliberately, so a run this node has heard of but not stored is
still findable — and it re-implemented the prefix match with neither check. `"".starts_with("")` is
true of every run. Measured: `cancel`, `explain`, `checkpoint` and `audit` each acted on a real run;
`logs`, `rm` and `resume` refused. One argument, validated on three commands and not on four, and
the four included the two that stop work. An unset shell variable is how it arrives.
`offload_core::needle` is the one rule now, three-valued because the two refusals need different
sentences — and the first cut of *that* produced ``state store: no run matching `` (no run id was
given)``, three clauses fighting, which is why `StoreError::NoRunGiven` is its own variant carried
out transparently.

**Three more, all from reading the whole screen.** The attendance line said *"it is running there"*
for every state with a holder, including `assigned` — one line under a state line reading *accepted
but not started* — and `orphaned`, where `Run::holder` answers *who was holding it* and the fencing
file already records that as the wrong basis for a claim about now. The one refusal `offload cancel`
has that nothing else reaches, for a holder out of contact, named the node `21177c09` while
`offload explain` two lines later and `offload nodes` one command away both said `bravo`; the helper
it needed was four lines below in the same file. And `nobody was asked to take it: it cancelled` put
the run in the subject position of something it did not do.

**The walk itself cost a lesson.** `kill -9 $(pgrep -f "offloadd --config …/bravo.toml")` matches
the subshell's own command line, so it returned a pid that was not the daemon — bravo kept running,
the run went on to `completed`, and for a minute that read as the failure detector being broken.
That is the third shape of one trap in two sessions; the recipe that works is a loop over
`pgrep -x offloadd` reading each `/proc/<pid>/cmdline`, and it is in `docs/DEMO.md`.

## Session eighty-one — a node that claimed it could run a program it did not have

Session eighty's first lesson was *ask what kind of work a pass is holding*, and its closing line
was that the same question is worth putting to every pass that iterates runs. This session put a
narrower version of it to the cheap tier's **own** machinery: what does a node claim about a task,
and is the claim true?

The staging is two minutes and one daemon. Four `[[…]]` blocks pointing at programs that are not
there — a sink, a trigger, a resource and a task — with working ones beside them as the control,
and then every report typed. Three of the four say so plainly, one command away: `offload sinks`
prints `unusable: … not found on this device`, `offload triggers` prints `down, retrying: …`, and
`offload status` prints `resource … — NOT usable, its program was not found`. **The fourth appears
in no report at all.** That was the whole finding, and it was found by reading the other three: a
missing line is invisible on its own screen and obvious beside its siblings.

**And the task is the one of the four that is a run.** So it is the only one a bid round places
work on — and it placed:

```
$ offload run --task nightly
run 01a0942db7e8
$ offload ps --all
01a0942db7e8   failed   task   -  -  -   nightly
           └─ agent: the program for task `nightly` was not found: /tmp/ow81/nightly-report.sh
```

Accepted, placed here, failed a millisecond later, then restarted by the recovery tick every
backoff until the budget went. That is the sentence ADR-0014 exists to prevent, and it is the
sentence the *no-cluster* arm of this same door had already been fixed for **one cause over** — a
service nobody nominates at all, which the control arm was refused with correctly in the same
pass. On a fleet of more than one it is worse than a wasted run: the node that cannot do the work
bids against the node that can.

**The bit was already measured and nothing read it.** `task_capabilities` sets
`Capability::authenticated` from `which(&cfg.command)` on every probe, and ADR-0019 §1's own
argument is that *the one thing a general-purpose machine can honestly verify is that a program
exists*. But §1 then says `Constraint::HasService` asks the question that matters — and it asks
whether anybody **nominated** one. `Constraint::ServiceAuthenticated` is the clause that reads the
bit; it existed in the enum and nothing in the tree constructed it. The tell that this was an
oversight rather than a decision is one ADR back: `CanUse`, the **resource** tier's clause, is
`is_resource() && authenticated`. Two applications of one rule, and the newer one dropped the bit.
ADR-0019 is amended; no wire bump, because the variant is an old one.

**Then every other reader of the weaker fact,** which is where the rest of the session went.
`task_refusal` — the fleet-of-one door, which has no round to ask — now reads the capability rather
than the config and answers three ways (`task::Nomination`), because *nothing nominated* and
*nominated and unrunnable* are different sentences with different fixes. `nominates_task`, which
feeds both `offload when --task` and `offload every --task`, asks `authenticated` on this node and
on every peer: without it both commands accepted such a rule and such a schedule **in silence**,
refused at every firing for ever, while the control printed the whole paragraph. That is the exact
silence session seventy-three removed from `offload every`, arriving again through the same
predicate. And `offload status` grew a `task` line beside the `resource` line — which both new
refusals now point at, so the line had to exist before the sentence naming it did.

**Three smaller things, each its own kind of lesson.**

`SubmitError::Task` had **no constructor**. It is in the enum with a doc comment saying it exists
precisely because `SubmitError::Agent` *"prefixed the first walk's refusal with the word agent for
a run that has none"* — and `drive_task` mapped the spawn failure to `Agent` anyway, so the
sentence the variant was written to prevent was what `offload ps`, `offload logs` and `offload
explain` all printed. A variant and its call site are two edits; only the first feels like work.

`which` asked `is_file()`, so a program that is there and **not executable** was advertised as
usable, won its bid and failed the spawn with `Permission denied (os error 13)` — the same
over-claim one permission bit in, with nothing on screen to tell them apart. Fixing it found that
there were *two* resolvers, `resource::which` and `deliver::resolve`, each having learned something
the other had not; nobody would have found that by reading either one, only by having to write the
same fix twice. And the **first** fix returned an `Option`, so every report then said *its program
was not found* about a file sitting right there — six sentences, introduced by the fix, caught by
re-running the walk. `Program::{Runnable, NotFound, NotExecutable}` is the third two-valued answer
to a three-valued question this tree has had to unpick, after `can_obtain` and `steward_of(..) ==
Some(me)`, and the fourth was `task_for(..).is_some()` in this same session. The tell is always the
same: the fix gets written at the *decision*, and the decision only needs to know **whether**.

And `Capability::description` was one field holding two different facts: the owner's **label** for
the four nominated kinds, and a **reason** for the one arm that refuses an agent whose login does
not match its config — while `offload probe` prints it as a `└─` clause under `NOT authenticated`
that its own comment calls *why*. So `└─ posts the nightly report to slack` read as the explanation
for a missing program. `Capability::unusable` keeps both, in one order, in one place.

**The fixes produced false sentences of their own, three times, and the re-walk is what caught
every one** — session eighty's fifth lesson doing its job. The first wording of the new clause was
`ineligible: runs nightly, program present`, a requirement printed as though it were the finding,
directly after the word `ineligible`. The second, `with its program there`, asserted which of two
causes it was, which `offload-core` cannot know — it holds the bit and not the filesystem. And the
`Option` above put a wrong cause on six screens at once. The settled shape: core says `and can run
it` / `this node cannot run it`, and the cause is named by the node that measured it.

## Session eighty — `offload checkpoint`, and the kind of work nobody asked about

The pick-up list had nothing actionable at the top — phase 9 done, enrolment done, the two-machine
demos blocked on a click at the Mac's own console — so this was the next unwalked command, chosen
by a lead the previous session handed over: `offload rm` was wrong on a **task** because a task has
no workspace, and `offload checkpoint` had never been typed at all while a task has no turn
boundary either.

**It is the fourth agent-only path and the only one that was never given a door.**
`prepare_start`, `resume` and `drive` all ask for the agent half of a `Work` and refuse a task with
a reason — `Work::Task`'s own doc comment names all three. `Supervisor::request_checkpoint` guards
on `cancel.is_some()`, which asks *is something running here*, and `start_run` takes that channel
one branch **above** the split on `Work::Task`. So a task walks through a guard written for an
agent: `offload checkpoint <a running task>` answered *"it will be taken at the next turn
boundary"*, `offload ps` read `checkpointing`, and `offload explain` said *"finishing its turn
before checkpointing"* about a `/bin/sleep`. Nothing was ever captured, and the command had pointed
at `offload logs -f` to watch for it.

**The costly half was the drain.** It arms the same flag for every held started run and then waits
per run, up to `drain_deadline_secs` — **300 seconds by default**. Measured with the deadline cut
to 20s: the full 20s spent on one `/bin/sleep`, and then the run counted in `later`, whose sentence
promises a handover *at its next turn boundary*. After the partition: **0s**, with its own line
saying what is actually true — nothing is owed, because the node either stays up and the program
ends, or goes, and ADR-0043 re-runs it from its spec. The mixed case is the proof: one agent run
and one task held together waits for the agent alone.

The guard went in `Run::request_checkpoint` — `offload-core`, where the run knows its own kind —
for `Supervisor::register`'s reason: two callers guarding is two things to remember. And it is
`TransitionError::NoBoundary` rather than a `WrongState`, because the state was `Running`, which is
exactly when a checkpoint request is legal; a sentence naming the state would be true and would
send somebody to wait for a moment that is never coming.

**Two smaller things fell out.** The fleet-of-one drain arm counts from `held_count()` rather than
from the pass that partitions, so it needed splitting too — otherwise a running task is
`no_boundary` on one path and `left` on the other, one fact with two spellings, which is the trap
`finished`, `later` and `pooled` were each split out of `left` to avoid. Its sentence for `left`,
*"there is no fleet to hand it to, so it stays here with its checkpoint"*, is false for a task
twice over. And once nothing was being waited for, the drain announced *"waiting for 0 run(s) to
reach a turn boundary — up to 20.0s"* — a wait that is not happening, with a duration nobody will
spend.

`offload drain` has now been wrong about something in **five** separate sessions, and the file's
header says so. Every fix was correct. What kept being missed was a different question each time:
how many callers, then how many doors, and now **what kind of work it is holding** — a question
that became askable the day ADR-0019 added a second tier and that nothing in the drain had ever
asked.

The guard the last eight handoffs have carried earned its place again: the first cut of the new
refusal was written as a continued string literal and
`no_message_carries_a_mangled_line_continuation` failed with the file, the line and the offset.

## Session seventy-nine — `offload rm`, and the two halves of a true sentence

The command nothing had ever walked, picked because it is the one run-targeted command that
deliberately does **not** travel: a directory cannot be torn down over the network, so `offload rm`
acts only where it is typed. Two defects, one on each of its two answers.

**The teardown said only what survived.** `worktree removed (the run's branch and commits are
kept)`, unconditionally, whatever the checkout held. That is true and it is the reassuring half.
ADR-0055's rule — ask `holds_uncommitted` *before* the move, because the move is what makes the
question unanswerable — was applied on the sweep's path and on `supersede`'s, and never on the path
a person types. And that path meets the case most often: a `failed` run is terminal **and**
resumable, so its worktree is the one holding edits past the last turn boundary, and `offload rm`
is one keystroke from `offload resume`. Measured: a modified file and an untracked one went, with
`offload ps`, `offload logs` and `offload audit` between them saying nothing about either — and
nothing else anywhere had them, not the branch, not the checkpoint, not the bundle, with `git fsck`
finding nothing because none of it was ever an object. Not a refusal: whoever typed it is entitled
to the disk back. `Removal::Removed { discarded }`, asked before the removal, with `commits_ahead`
left out so the count is of what teardown destroys and never of what it keeps — the clean case
still prints the old sentence, because a line that cried wolf on every ordinary `rm` would be noise
on every teardown in the fleet.

**The refusal covered four causes and described one.** *"it was removed already, or the run finished
on another machine, where `offload rm` is what discards it"* was said to a **task** (no workspace on
any machine by construction), to a run **cancelled out of the queue** (nothing was ever made
anywhere), to a checkout **already torn down here**, and to a run that finished **on a peer** —
three false in both halves, and the fourth naming no machine while `RunProgress::by` sat in this
node's own row, the field `log_source` one function up already reads to route `offload logs`.
`Leg::{Never, Here, Peer}` crossed with the gossiped worktree note, in a pure function.

**And then the fix made the same mistake one arm over, which is the part worth carrying.** The
`Leg::Here` arm pointed at `offload audit <id>` and said it *"says what happened to it"*. Typed, on
a daemon, against a run whose last leg was still this node: `granted`, `accepted`, and not one word
about a directory. `AuditEvent::Reclaimed` is the row that would be there, the checkout sweep is
what writes it, and `note_reclaimed`'s **own doc comment** says the sweep runs on the leg that has
*lost* the run — which renders `Leg::Peer`. The arm that named the command is the one arm the row is
never in. So the row is read (`Supervisor::reclaimed_here`) and said in the audit log's own words
(`Reclamation::why`, lifted out of `AuditEvent::describe` so two renderings of one fact cannot
drift), and its absence is stated out loud. **A report that names a command is a promise that
command has an answer** — and the arm's own unit test had asserted `contains("offload audit")`,
which is a test of the wording agreeing with itself.

All six arms and both teardowns were then walked on two daemons, including the one only a second
machine shows: bravo tore its own checkout down, the `removed` note gossiped, and alpha stopped
sending anybody to a machine with nothing left to do.

Three things about the staging, all of them already in `docs/DEMO.md` and all of them met anyway:
`cd $W && nohup offloadd … &` records the **subshell's** pid in `$!`, so every restart silently
failed and a stale daemon answered the socket with the old sentence; `kill -9 $(pgrep -f "offloadd
--config …")` kills the shell that typed it; and a daemon told to stop drains for up to five
minutes, so `sleep 2` after a `kill` is not a restart. Each cost one confused measurement. The
general form: **when a walk's result looks like the code you just changed did not change, check
which binary is actually serving the socket** before believing anything about the code.

## Session seventy-eight — the two-step enrolment, and the grant it could take away

The handoff's second pick-up: walk `offload invite` and `offload join --token`, the half of
enrolment this project's own walks keep replacing with `--passphrase` because it scripts more
easily, and therefore the least-typed path in the least-walked subsystem. Refusals before the happy
path, which is what the last four walks did and what found most of what they found.

**Ten refusals, all sound.** A name instead of an id, and the twelve-character prefix `offload
nodes` prints (both refused with the id's length named and `offload id` pointed at); inviting from
a device that has not joined; a token pasted on the wrong device (*"certificate names 580993bf, but
this node is b6efe4c9"* — the non-transferability rule, working); a truncated token; one with no
prefix; `--token` with `--passphrase`, caught by clap; a token replayed; and `offload join` bare,
which names both built paths rather than silently falling back to the recovery one. The happy path
works, and `--grant host-runs` prompts for the passphrase with the reason stated first.

**The eleventh case was not a refusal.** `offload invite <an id that is already a member>` is
accepted — it has to be, because re-inviting with `--grant host-runs` is how a device is widened
without walking to it — and the certificate it issues under an approver's delegation carries
`default_grants()`. Offered to a device already holding `host-runs`, it expires later and grants
less, and `FleetState::adopt` took it: the improvement test was `expires_at` alone. Two commands,
no passphrase, and the grant the passphrase exists to protect was gone.

The screen is why it took a control arm to see. `grant_list` renders what is *in force*, and
probation renders `host-runs` on its own line, so the `grants` line printed by the command that
*took the grant away* is **character-for-character the line printed by the command that granted
it** — the only difference is a countdown disappearing, which reads as probation elapsing.
`offload fleet` beside it is what made the loss visible, which is seventy-seven's lesson reused:
the control arm is often a second command rather than a second node.

The founder form is the one to carry: `offload invite <its own id>` then `join --token` took alpha
from `submit, deliver, host-runs, approve` to `submit, deliver`, and the fleet's only approver
stopped being one. It lost its name too, because `invite` defaults `--name` to a short form of the
id. And the renewal loop in `mesh.rs` reaches the same function with whatever an approver peer
answers `RenewMe` with, so the same narrowing arrives over the network with nobody pasting
anything — which is what settled the fix. Reporting the loss loudly was the first draft and is
wrong on that path: a `warn` line is not consent.

ADR-0062: **a certificate is an improvement only if it takes nothing away.** It completes ADR-0012
mitigation 1, which bounded an approver from *minting* `host-runs` and said nothing about stripping
it — the same door from the other side. `Adopted::{Taken, NoBetter, Narrower { lost }}` replaces the
`bool`, which is session seventy-five's `RepoReach` fix in a second file and for the same reason:
the refusal had to word a sentence out of a measurement the `bool` had thrown away.

**Two things the fix got wrong first, both caught by writing the test.** The comparison must be on
the certificates' own grants, not `grants(now)` — probation suppresses `host-runs` without removing
it, so comparing what is in force would refuse every invitation that carries it. And the first cut
had no arm for a *wider* certificate, on the argument that every issuing path stamps `now`, so
anything carrying a new grant expires later. True of one machine, false of two: the issuer is a
different device, and clock skew behind this node makes a deliberately widened certificate expire
earlier and be refused as buying nothing. Both are in the ADR, the second because it is the
plausible kind of wrong.

**The report defect beside it** is the one this project keeps finding: `offload fleet`'s `approver`
line read `FleetState::approver`, the delegation, while the door, the note and `offload status` all
read the `Approve` grant. On a narrowed founder one screen said `approver this node may enrol
others` four lines above `note: this device is not an approver`, with the command refusing. Fixed
to read both halves in the door's order — and kept even though ADR-0062 makes the desync
unreachable, because holding two fields in agreement by arranging for nothing to separate them is
not the same as reading the one that decides.

**The house guard earned its keep.** `no_message_carries_a_mangled_line_continuation` caught a
literal this session's own tooling had mangled — a python heredoc ate a `\` continuation and left
the indentation embedded mid-sentence — and the failure message named the file, the offset and the
two acceptable fixes. Nine walks in a row have now found the thing under test sound and the screen
around it not; this is the first of the nine where the mechanism was wrong too.

### The second half: the two daemon-side claims, on two daemons

The same session's own pick-up: the enrolment walk ran entirely on state directories, which left
two things inherited rather than measured. Both are now measured, on two daemons on loopback with
`PROBATION` and `CERT_LIFETIME` shortened and put back.

**`join --token`'s promise is true, and much faster than it claims.** *"A daemon running on this
device picks it up within a second. No restart."* Staged so that only the grant could decide it:
alpha arbitrating with `accept = "never"`, bravo with no `host-runs`, one queued run that nobody
could take (*"bravo — not granted host-runs by the fleet; alpha — node is not accepting work"*).
Granting `host-runs` by invitation and taking the token up moved bravo's answer in **17ms**,
measured through `offload explain`, whose per-node block is recomputed live (*"asked just now"*) —
and the node whose answer changed is the **arbiter on the other machine**, because `permits_hosting`
asks at the moment of the question rather than at the handshake's. The run then landed on bravo 48
seconds later, which is the re-offer cadence and not the certificate; the first pass measured only
that 48s and nearly recorded it as the pickup being slow.

**The renewal path works and preserves grants**, which no walk had ever driven: bravo renewed from
alpha, serial moved, `submit, deliver, host-runs` intact. And **ADR-0062's new arm fires over the
network** — with `renew_for` patched to drop `HostRuns`, bravo logged `an approver offered a
certificate that takes grants away; not taken up … lost=host-runs` and kept the grant, serial
unchanged. That is the path with no screen, and it is the reason the ADR refuses rather than
reports.

**Three reports around them were wrong, and the walk found all three by reading the whole screen.**

The largest: **the fleet's only approver is told its fleet has no approver.** `renew_if_due` asks
peers and never itself — deliberately, since a node that could re-sign its own membership would
never lapse out of a fleet it had been removed from — so the sole approver is exactly the device
nothing can renew, which is the *default* two-device fleet. It warned `a fleet with no approver
needs offload grant approve on a device that has one` while its own `offload fleet` said `approver
this node may enrol others`. The mechanism is right and only the sentence was wrong; the advice was
right too, which is the more dangerous half, because advice with a false explanation gets argued
with.

Beside it, the same loop **measured a reason per peer and discarded all of them**. On the joiner,
two `WARN` lines 49µs apart contradicted each other: `an approver offered a certificate that takes
grants away … lost=host-runs`, then `no peer would renew it — a fleet with no approver`. The fleet
had one, it was reachable, it had answered, and the answer had been refused by the line above.
*Decisions carry reasons* is usually read as being about `Hold` and `NoBid::Refused`; it applies
just as much to the line reporting a decision **failing**, and that line is the one nobody writes a
type for.

Third: **`Millis`'s `Display` is a duration formatter and two absolute timestamps went through
it.** A lapsed certificate said `expired at 496980h44m, now 496980h44m` — the Unix epoch as an
elapsed span, twice, with the one thing that matters invisible at hour granularity. It is the
message for the moment the ADR-0012 backstop fires, the laptop shut for a month. `expired 25.1s
ago` now. A type holding milliseconds does not say whether it is an instant or a span, and the
formatter assumes span.

And one found by the walk's own staging: **`as_secs() / 60` renders the last minute of every
probation as `0 minutes`**, which reads as no wait at all. Five sites said it, including `offload
explain`'s per-node line. One helper, five callers, a test — the "grep every site" lesson met
before writing the fix rather than a session later.

### The third part: ADR-0061's open question, measured

The handoff's item 1 named a prerequisite nobody had taken: *"Whether acquisition is resumable.
Blobs are all-or-nothing today… worth measuring before building."* Staged as the ADR's own shape —
a fake agent that **commits** a 256 MiB file, so the bundle blob carries it — on two daemons.

The answer is worse than the question assumed, and the first fact settles it before resumability
is reached. **`MAX_BLOB_BYTES` is 512 MiB and the motivating workspace is 6.5 GB**, thirteen times
over, enforced on both the push and the fetch side. The cap is there for a good reason: a peer's
`size` field is attacker-controlled, so `BlobFound { size }` is an allocation request.

**Not resumable, and there is nothing to resume from.** The receiver was `kill -9`'d mid-transfer;
afterwards it held **363 bytes** — the previous turn's transcript blob — and nothing else.
`recv_bytes` fills one `Vec<u8>` and `blobs::put` writes temp-then-rename, so no partial file can
exist. Correct rather than sloppy, and a wire change to alter.

**A blob costs its whole size in memory at both ends**: peak RSS 308 MB sending and 298 MB
receiving, from a 33 MB baseline. The trait's doc comment had said so all along — *"If something
ever puts a repository in here, this is the signature to change first"* — which is the reusable
part: **a doc comment that names its own breaking point is a measurement waiting to be taken**, and
the ADR proposing to cross it should cite the number. Throughput, for scale: 6.36 s for 256 MiB
over loopback, ~40 MiB/s.

**One defect on the way**, and it is the session's third `bool`-shaped answer. With the cap lowered
under the bundle, a checkpoint that could never be replicated anywhere reported *"checkpoint is on
this node only; nobody would take a copy"* — which reads as *no peer was available*, the one cause
that gets better on its own. The peer was alive, meshed and willing. The real reason went to a
`tracing::debug!` inside `offload-cluster`, while the decline that arrives **after** a transfer —
the transient one — was at `warn`: backwards. `Pushed::{Stored, TooLarge, Declined}`, with
`TooLarge` decided **before dialling** because every node enforces the same cap, and
`Replicated::{To, NoPeer, Refused { why, permanent }}` to carry it out. `NoPeer` dropped to `debug`:
a fleet of one has nobody to ask and warning about it every turn is how a log gets ignored.

Three `bool`-shaped answers replaced in one session — `Adopted`, and this pair, after seventy-five's
`RepoReach`. **The shape to recognise is a function that decides something with a reason and
returns whether rather than why.** The sentence downstream then has nothing to say, and somebody
writes a plausible guess into it that is wrong in exactly the cases that matter.

### The fourth part: the detail files, and a piece of guidance that was wrong

The handoff's item 14 — *"a cheap, useful job for somebody"* — carried numbers from session
sixty-seven, and this tree's own rule is that an inherited number gets re-taken. Re-taken: **383
rules, 278 detail entries**, with only `agent-adapter` and `scheduling-and-attendance` complete.
Every subject the old entry named is now worse than it was, so writing the pair together — the
habit sixty-nine started and this session kept — slows the gap without closing it.

**The useful finding is not the count, it is that the advice beside it was wrong.** The entry said
the ordering "still aligns from the top — the missing ones are the newest — so opening a detail
entry for an older rule is safe and for a recent one finds nothing." It is not dependable: in
`capacity-policy-and-probing` the third detail entry is already about a different subject from the
third rule, and once one rule gains no detail entry every later position is offset. The failure
mode that advice produces is the bad kind — not finding nothing, but reading a confident,
well-written entry about **the wrong thing** and believing it is the detail for the rule in hand.
Corrected in `CLAUDE.md` and in the handoff: **find a detail entry by searching its opening text,
never by counting to the same position.**

**Two scripted attempts to measure alignment were both wrong, in both directions**, which is worth
recording because the instinct to automate this is strong. Comparing the first 90 characters said
`checkpoints-blobs-and-workspaces` diverged at entry 1; by hand, entries 1–4 correspond fine — the
two files deliberately word the same entry differently, because the rule is the compressed form and
the detail is the narrative. Comparing only the bolded titles moved the numbers and kept the same
error. Hand-checking is what settled it, and the automated number is not in the handoff as a result.
**A similarity score is not a measurement when the two things being compared were written to differ.**

And the caution that matters most for whoever picks the job up: **a detail entry backfilled from
the rule text alone would be fabrication.** Its sections are the mechanism, the measurement and how
it was found; for an old rule that evidence is in `docs/sessions.md` and the commit that added it,
or it is nowhere. Backfill what can be sourced and say what cannot — 105 is an upper bound on
missing narratives anyway, since a continuation rule often shares one detail entry with the rule
above it.

### The fifth part: `offload approve`, and the argument nobody mistypes in a walk

With both top pick-up items needing the owner, the gap worth filling was a command no session had
driven: **`offload approve`**. Session seventy walked its sibling `deny`, found the mechanism sound
and three reports around it wrong; `approve`'s own path had never been typed. Staged as
`docs/DEMO.md` says — a `[[sinks]]` entry so a question is actually *put*, `--ask --permission ask`
on the submission, and a fake agent that blocks by piping one line into `offloadd ask-hook`.

**The mechanism is sound**, and generously so. `offload asks` reports the tool, how long it has
waited, how long is left, where it is and what it wants; the instruction line beneath carries the
full run id and the `tool_use_id`; the sink received both the question and, later, the completion;
approving unblocked the agent, which finished its two turns. A **short** run id is accepted as
well, which makes session seventy-seven's widening of those lines belt-and-braces rather than
load-bearing — worth knowing, since that session treated it as closing a coupling.

**The finding is the refusal nobody meets in a walk.** `Asks::answer` filtered on the run *and* the
`tool_use_id` in one chain and answered an empty result with `NothingWaiting(run)`. So a mistyped or
stale id — the ordinary human error on a 20-character opaque token — got *"nothing on this node is
waiting for an answer about run X"* about a run whose question `offload asks` was printing on the
line above, with four minutes left on it and the agent blocked mid-tool-call. Two commands, one
screen, contradicting each other; and the false one sends somebody to look for an expired question
rather than at their own argument.

Asked in two steps now, with `AnswerError::NoSuchQuestion` **naming the ids that are waiting** —
`reports-and-cli`'s rule that an error telling somebody to try again must be satisfiable from the
screen it is printed on, met from the same side session seventy-six met it on `offload revoke`.
`offload deny` shares `Asks::answer` and was wrong in identical words, which is why seventy's walk
missed it: **a walk that supplies the correct argument never sees the refusal for a wrong one.**
That is the reusable part, and it generalises last session's sweep — for a command taking an opaque
identifier, try it with a *plausibly wrong* one, not only a missing one.

**And a staging trap that nearly produced a false negative.** The first before/after ran against the
**old binary**: `kill $(cat pidfile)` used a pid written by a relaunch that had already failed to
bind the socket the previous daemon still held, so the old daemon, its agent and its blocked
`ask-hook` were all still up and the fix appeared not to work. `pgrep -a offloadd` before believing
a before/after — the same family as the demo's existing note about `cd && nohup &` recording the
wrong pid.

### The sixth part: the sweep the fifth produced, which came back clean

`offload approve`'s finding generalised to a question, and this tree's habit is to turn a question
into a sweep: **for a command taking an opaque identifier, what happens when the operator supplies
a plausibly *wrong* one?** The sharpest form is an id of a different *kind*, and the shapes invite
it — a rule id and a schedule id are both sixteen hex characters, and only a run id (twelve) looks
different.

Six crossings on one daemon holding one of each. **All six are sound**: every one answers `no
<thing> matching <id>`, which is true and names the type it was looking for — and that naming is
itself the hint, because *no rule matching* tells somebody they are in the wrong command. Nothing
claims a wrong cause. A *"that is a schedule id, did you mean `offload unschedule`"* hint is
deliberately not built: it couples each command to the other subsystem's table for a convenience
the listing resolves in one command, and that is now recorded as a decision rather than left as an
omission. **A clean sweep is worth writing down** — seventy-seven's found a defect in ten minutes
and this one did not, and the difference between those two outcomes is not something a later
session should have to re-establish.

**The finding came from the staging instead**, which is the third time this session a walk's
scaffolding produced the defect rather than its subject. The `[[triggers]]` block was written
without a `service` line, and the daemon said `invalid config: : not a service this build can
offer` — a sentence opening with a colon, about a name nobody wrote. `#[serde(default)]` on a
required field turns *absent* into a value, so serde catches a misspelled field **name** and lets a
missing field through as `""`, and every message downstream then describes the value instead of the
absence. One `Display`, four config blocks (`[[sinks]]`, `[[triggers]]`, `[[resources]]`,
`[[tasks]]`).

### The seventh part: running the grep that retired the grep

The handoff had carried, as a job worth doing, "a `bool`-shaped answer throws away the measurement
the sentence needed — worth a grep as its own job." Four of this session's findings were that shape,
so the generalisation reads well. **Running it is what showed it does not work.**

Outside tests there are **83** `-> bool` functions. Stripping the plain predicates leaves
`heard_from`, `set_running`, `set_capabilities`, `refresh`, `absorb`, `probe`, `probe_indirect`,
`peer_has_blob` and a few more — and every one is *"did this change"* or internal control flow with
no operator-facing sentence hanging off it. `Probe::refresh` is the clearest case against the grep:
its doc comment already says the `bool` "is only worth a log line" and that whether to gossip the
change "is `Cluster::set_capabilities`' own question … and this must not become a second answer to
it." The grep flags it; the code is right. And the fourth finding of the shape, `Asks::answer`, has
no `bool` in it at all — it is a filter chain — so the grep would have missed the one it was
generalised from most directly.

So the shape is **not a return type**: it is *an operator-facing sentence downstream has to state a
cause, and the function that knew the cause answered whether instead of why*. The tractable test
runs the other way — start from a sentence a person reads and walk back asking whether what it
claims was measured or guessed. All four were found that way: three by walking a command and
reading the whole screen, one by reading the call site of a walk's finding. **None by reading a
signature**, which in hindsight was the evidence sitting in plain sight.

Kept beside seventy-seven's identifier sweep as the other outcome. That one was a `grep` that found
a defect in ten minutes; this one was worth running precisely because it came back empty and
retired an idea that reads well — and an idea that reads well and is wrong costs more, left in a
handoff, than one nobody wrote down.

### The eighth part: the owner resolved ADR-0061's fork, and it became phase 9

The measurement in part three left ADR-0061 with a fork rather than an answer, and the fork was the
owner's: the blob plane caps at 512 MiB, the motivating workspace is 6.5 GB, and either the archive
fits or the plane grows chunking and resumable transfer. **The owner chose the cap.**

**What that changes in the ADR is one paragraph, and it was wrong rather than incomplete.** §4 said
*"There is no size ceiling by default"* and argued it well — a limit invented for a case nobody has
hit is the knob-nobody-understands this project refuses to ship. The reasoning was sound and
pointed at the wrong ceiling: there already was one, it is the protocol's rather than the owner's,
and it exists because a peer's announced `size` is an allocation request. `max_archive_bytes`
survives as a **tightening** knob — `None` now means *the structural cap and no opinion beyond it*,
never *no limit* — and nothing may set one larger.

**The consequence worth being honest about is that a subset is worse than a snapshot.** The ADR's
Bad list already said the receiver gets a snapshot: stale, one-way, nothing syncs back. A subset is
missing things, chosen by an agent working to a budget, and its failure is a run dying for want of
a file that exists on the operator's disk. So the acquisition report naming what travelled stops
being a nicety and becomes the thing that makes such a failure diagnosable — which is why it is a
roadmap item rather than a line in Consequences.

**And the cap has to be sayable before the expensive step.** An agent that builds a 6.5 GB archive
and is refused afterwards has spent minutes and a disk to learn a constant. That is the first item
of the new phase and everything else leans on it.

§6 grew a fourth refusal for the same reason the other three exist: *larger than this node will
take* and *larger than any node will take* send somebody to opposite places — another node, versus
a smaller archive — and collapsing them would repeat exactly the defect session seventy-five had to
unpick in `NoBid::RepoUnavailable`. The structural one is also the only refusal here knowable
without asking a peer, which is what makes "say the cap first" implementable.

The ADR is **accepted** now, and `docs/ROADMAP.md` carries it as **phase 9**, seven items in
dependency order, none built. **Nothing was built this session, deliberately**: the first item is
worth nothing until there is an archive to budget for, and building it ahead of that is the
machinery-ahead-of-its-use the ADR itself argues against. Starting the phase is a phase-sized
commitment and wants the owner to say go, rather than a session deciding to begin because the
decision in front of it was interesting.

### The ninth part: building phase 9's §1 and §2

The owner said start, so the first two clauses of ADR-0061 are built and walked.

**What made it small is that nothing downstream learns about archives.**
`ensure_archive_mirror` unpacks the bytes and leaves an **ordinary bare mirror** under `repos/`;
from there `prepare`, the `base..HEAD` bundle, the patch, `restore`'s `merge-base` reasoning,
replication and migration are untouched. §2 claimed exactly that and it held — the test that proves
it calls `prepare` on an archive-derived mirror and reads the agent's files out of the worktree,
and the walk finished with a commit on an ordinary run branch.

`RepoSource::Archive`, spelled `archive:<digest>` inside the run spec's existing `repo` string. A
prefix rather than a new field, so every reader keeps working and a node too old to know the form
refuses it as an unobtainable path instead of misreading it as a directory. `Fleetwide`, which is
the headline: **a workspace no node could clone can now be placed anywhere.**

**Four things were not obvious until the code was written**, and all four are the same species — a
mechanism written for clones being asked a question about bytes:

- An archive's mirror is **immutable**, so `ensure_mirror`'s refresh must be skipped. Its origin
  was a scratch directory deleted at the end of the unpack and its name is the hash of its own
  bytes; a fetch fails on every start, survivably, logging *mirror refresh failed* about the one
  mirror in the system that cannot be stale.
- It is never **cloned** into existence either — `git clone archive:<hash>` would be a confusing
  failure at the far end of a start, so `ArchiveNotAcquired` is named instead. `Restorable`'s
  argument reused: the ordering mistake is a caller's, and carrying on quietly would build a
  workspace out of nothing.
- `tar czf x.tar dir` archives the **directory, not its contents**, and both spellings are things
  an agent will produce. The second makes a repository whose only tracked entry is a folder:
  everything downstream is happy and the agent finds its files one level deeper than it left them.
- The acquisition goes **before** the checkout lock. `drive` holds it for as long as the workspace
  is built — right for a worktree, wrong for a transfer that is seconds to minutes and would block
  the checkout sweep and `offload rm` throughout.

**The walk is the part worth keeping.** One command apart, on one daemon, against the motivating
case in miniature:

```
$ offload run --repo /tmp/o9/punch-out --queue "do the thing"
  alpha        /tmp/o9/punch-out is here, but it is not a git repository

$ tar -cf chosen.tar CLAUDE.md docs master-shop/conf
$ offload run --archive chosen.tar "do the thing"
archive 4f138a26b289  (10240 bytes)
```

The agent's own `find` inside its worktree returned exactly the three chosen paths and **not** the
2 MB backup sitting beside them — which is §4's subset, working, rather than asserted. The run
completed with one commit on its run branch, and the submitter's directory still had no `.git` in
it: §2's *never in the submitter's directory*, checked rather than assumed.

The over-cap refusal was walked with `MAX_BLOB_BYTES` lowered to 1 MiB and is refused at
`StoreArchive` — before the run exists, before any bytes are stored, naming both numbers and what
to do. That is §4's *say the cap before the expensive step* met at the first place that can, and it
needs no peer because every node enforces the same limit. Only **half** the item, and the roadmap
says so: the number is still not readable without building an archive first, which is what an agent
budgeting a 6.5 GB directory actually needs.

### The tenth part: §3, and the two defects only two daemons could find

§3's cadence half — *the archive moves once per node per run, and the per-turn checkpoint carries
only what changed* — is true by construction: a checkpoint is transcript, bundle and patch, and the
archive is none of them. So the work was the **walk**, and the walk is what earned the section.

**`replicated` was a false promise.** Alpha ran an archive workspace to turn 7 and `offload ps` said
`SAFE: replicated`, whose meaning is *this survives alpha going away*. Bravo held every checkpoint
blob and **not** the archive, because replication moves `checkpoint.blobs()`. Killing alpha, every
other part of the system behaved perfectly — detector, reassignment, epoch, fetch, and the refusal's
own wording — and the run failed at turn 12 with *"no peer could supply the blob"*. Twelve turns of
work, on a run the operator had been told was safe.

The fix is not to widen `Checkpoint::blobs`, which is what the checkpoint is made *of*: that would
make the method lie and re-offer the archive every turn as checkpoint content, which is the exact
thing §3 exists to prevent. Replication asks a different question — **what does a peer need to
materialise this run** — and for an archive workspace the answer includes the archive. Putting it
there makes `is_durable` honest for free, because `Mesh::replicate` is all-or-nothing per peer, so a
peer that could not take the archive is never recorded as a replica.

**Then the migration got one step further and failed differently**, which is the finding worth
keeping. §2 has the receiver `git init` an archive that is not already a repository — and a commit
hashes its tree, its parents, its message **and its two timestamps**. Two nodes unpacking
byte-identical archives therefore built *different* base commits, and bravo answered `fatal: invalid
reference <alpha's base>` over a checkpoint and a bundle that were both correct. The
content-addressed archive gave the same bytes and not the same history, which is the one thing
everything downstream assumes.

So the repository built from an archive is content-addressed the way the archive is: fixed identity,
`GIT_AUTHOR_DATE` and `GIT_COMMITTER_DATE` pinned, fixed message, and `--initial-branch` pinned —
that last because `init.defaultBranch` is somebody's global config and `main` versus `master` is the
same failure by a slower route. **The commit id is a wire format in all but name**: a node upgrading
mid-run must still find the base its peer's checkpoint names, so the test pins the literal digest
rather than only cross-node equality.

After both, the walk end to end: killed on alpha at turn 5, running on bravo at turn 13, one
unbroken branch reading `the archive this run started from` → turns 1–5 → turns 6–12, with the
archive's files and the agent's own beside them. *Close the laptop, the agent keeps working on the
desktop* — for a directory that is not a git repository and has no origin to clone, which is the
sentence ADR-0061 was written for.

**The reusable part is not about archives.** Anything a second machine has to reproduce must be a
function of its inputs alone, and the two things that leak in are **the clock** and **whatever is in
somebody's global config**. Both leaked here, in one six-line block, and neither is visible on one
daemon — which is the sharpest argument yet for the project's own rule that a walk needs the second
node.

### The eleventh part: finishing §4's cap item, and a number that would have drifted

The half left over from the ninth part: the cap was *refused against* but not *readable*. An agent
deciding which 12 MB of a 6.5 GB directory travels could only learn its budget by packing the wrong
thing and being turned away — which is precisely the expensive step §4 exists to put the number in
front of. `offload status` prints it now, and the walk that matters lowered the cap to 128 KiB to
check that **the number an agent reads and the number that refuses it are the same one**: both come
from `MAX_BLOB_BYTES`, not from two copies of a fact. *A report has to come from where the decision
reads*, applied before the decision rather than after it.

**Two things were wrong on the way there, and the first was mine from two hours earlier.** The
number was hardcoded in `offload run --archive`'s own help — *"the fleet moves at most 512 MiB in
one exchange"*. True when written, and one constant change from being confidently wrong in the one
place somebody reads **before** doing the expensive thing. It is the stale-number rule this tree
already applies to docs, met in argument help, where it is worse: a session re-measures a doc and
**nothing ever re-measures a `--help`**. The help now says the limit exists and where to read it.

The second is a shape worth naming. `max_archive_bytes` is a `#[serde(default)]` control-socket
field, so a daemon too old to send it leaves it **zero** — and `archives up to 0 bytes` is not a
missing sentence, it is a **wrong** one: it says the fleet will take no archive at all, the exact
opposite of what the field exists to say. Omitted at zero instead. The trap is specific to fields
whose zero is *meaningful*: a defaulted `None` reads as silence on its own, and a defaulted `0` on a
size reads as a policy. Six control-socket fields have been added across recent sessions and this is
the first where the default was not self-describing.

Phase 9 is 4/7.

### The twelfth part: `max_archive_bytes`, a ceiling that may only tighten

§4's knob, plus `WorkspaceSpec::archive_bytes` beside it so a node can bid *against* a size rather
than discover it by fetching (§5). Four decisions in it, and none is specific to archives.

**A larger value is refused, not clamped.** The structural limit is `MAX_BLOB_BYTES` and no config
can raise it, so a bigger number is a startup error naming both. Clamping is the tempting
alternative and is worse: an owner who wrote it believes their fleet moves that much, and a silent
clamp leaves them believing it until a run fails somewhere else entirely. The closing clause does as
much work as the numbers — *there is no setting here that would* stops somebody hunting for the one
that would.

**`None` means the existing limit, not the absence of one.** Worth restating because the ADR said
the opposite for four sessions, with an argument that was *sound* — a limit invented for a case
nobody has hit is the knob-nobody-understands this project keeps refusing to ship — and aimed at the
wrong ceiling. The reusable question before adding any `Option` ceiling: **what is already
enforcing one?**

**Two limits, one reported number.** `offload status` prints the effective one, because one number
is what an agent packs to; printing both and leaving the reader to work out which binds is the
report shape this tree keeps deleting.

**And the refusals stay distinct.** `NoBid::ArchiveTooLarge` ends *"another node may still take
it"*, because it is one owner's answer; the structural cap is refused before a run exists and says
the archive must be smaller. They send somebody to opposite places, and collapsing them would
repeat `NoBid::RepoUnavailable`'s own defect, which seventy-five had to unpick.

On the spec side: **a `None` size is not a zero size.** `archive_bytes` is absent for anything that
did not know to state it, and refusing on absence would turn a missing field into a policy decision.
The node finds out at acquisition — slower, and correct. The size is measured by the submitting node
from the blob already in its own store rather than taken from the caller, which is the same posture
as hashing the bytes instead of believing the digest.

**No wire bump**, and the reasoning is worth recording because both new fields *are* gossiped: the
`archive:` spelling is its own version gate. A node too old to know the form parses it as a path,
finds nothing of that name and declines to bid — so these fields only ever travel between nodes that
both understand them.

**One thing to own.** The walk's config had no `[agent] binary` override, so the archive that *fit*
was picked up by the **real** agent and spent 18.1k tokens before it was cancelled. Every other walk
this session used a fake agent; this one did not, because the config was written to exercise a
policy knob and the agent was not what was under test. A walk that submits a run needs the fake
agent whether or not the agent is the subject.

Phase 9 is 5.5/7.

### The thirteenth part: the acquisition report, and why it is load-bearing

The last substantial item of phase 9, and the one whose *necessity* is least obvious — a report is
usually a nicety, and §4's amendment promoted this one because the design's own failure mode
depends on it.

A cloned repository needs no line saying what is in it: the ref names a commit, the commit names a
tree, and anybody with the repo can look. An **archive** is a subset an agent chose, nobody else
holds a copy of that selection, and the failure this design has is *a run dying for want of a file
that is sitting on the operator's disk*. Without a record of what travelled, that is
indistinguishable from a workspace that never arrived — and the two want opposite responses.

Walked by staging exactly that: an agent that stops when `master-shop/conf/settings.ini` is absent,
and an archive that deliberately leaves `master-shop/` out. Two lines apart in the log:

```
archive     3a7f8fda
            2 file(s), 16 bytes — CLAUDE.md, docs/
…
cannot continue: master-shop/conf/settings.ini is missing
```

The selection was wrong, not the machinery, and a reader has that in a second.

**Three decisions.** It lives in the **run's** log rather than only the node's, because the
question is asked from anywhere and usually later — often on a different machine after a migration,
where `tracing` output is on a disk nobody has. It carries the **top-level entries and a count**,
never every path: an agent selecting 12 MB out of 6.5 GB may still bring thousands of files, a line
nobody reads to the end reports nothing, and the roots are what the agent actually *chose*, which is
the decision under review. And it is written **only when bytes moved** — `ensure_archive_mirror`
answers `Option<Acquired>` and the warm path answers `None`, so the caller *cannot* log an
acquisition that did not happen. Measured: a second run against the same archive has no `archive`
line at all, which is §3's once-per-node property read from the reporting side. Logging it every
time would put a second "what travelled" answer in the log of a run whose workspace had not changed,
and nothing would distinguish a re-acquisition from a repetition.

The counting is over the **unpacked tree** rather than the tar's own listing, because what a person
needs is what is *there*: a tar may carry entries that do not land, and the descent into a single
root changes what every path means.

Phase 9 is 6.5/7. What is left is §6's fourth refusal and it is deliberately unbuilt — the
structural cap is refused earlier, at `StoreArchive`, so a run carrying an over-cap archive never
reaches a bid round, which is better than having the variant.

### The fourteenth part: the gap in the agent line, and the reason it survived

The thing the thirteenth part saw and declined to chase. That was the wrong call by a small margin,
and the margin is instructive.

`offload logs` built its agent line as one format string over two fields taken straight from the
agent's own init line. `offload_agent::event` reads that **leniently**, on purpose, because it is
somebody else's format — so either field can be absent, and an agent reporting neither printed
`agent claude-code , model`. The failure is not that a fact is missing; it is that the **shape** of
the line is broken, so a reader concludes the *report* is faulty and goes to look at the renderer
rather than at the agent.

It is the exact string `LogKind::TaskStarted` was split into its own variant to avoid — *"a log line
claiming an agent for a run that has none, with two empty fields where the claim should have been"*
— reached from the agent side, where the run really does have an agent and only the details are
missing. Named now rather than omitted, because *which model did this run spend on* is a question
asked of that exact line, and silently dropping the answer is indistinguishable from nobody having
asked.

**Why it was dismissed:** it appeared under a fake agent, and a real `claude-code` reports both
fields. But the leniency in `event.rs` exists precisely because that is *not* a promise anybody
made — a future version, or an adapter for another agent, need not send them. "Only a fixture hits
it" is a claim about today, and the code one layer down already says otherwise.

**Why it survived a heavily-tested crate**, which is the part worth copying: the wording lived
inside `print_event`'s `println!`. That function writes to stdout and cannot be asserted on, so
**neither arm was checkable** — there was no test that could have failed. Moving the decision into
`agent_line`, a pure function returning a `String`, made both arms testable in four lines. When a
report's wording contains a decision, the decision wants to be a function that returns a string;
left inside a `println!` it is unreachable by everything except a walk.

## Session seventy-seven — the sweep the last handoff asked for, and what it found

One question, asked of every command that takes an identifier: **which report prints that
identifier, in the form the command demands?** It came out of seventy-six, where the largest of
three findings was a command whose argument nothing on screen could supply — a class of defect no
amount of walking finds, because a walk supplies the argument. Twelve commands take a run id, one
takes a rule id, one a schedule id, three a node id, two a `tool_use_id`, four a service.

**The sweep took about ten minutes and its answer was one line of `grep`.** `id_width` — session
seventy-four's function for "the shortest width at which this listing is distinct" — had **one
caller**, `offload ps`. Every other listing that prints a run id used the fixed twelve. Seventy-four
found its defect in `ps`, measured it there, fixed it there, and wrote an entry saying the width
"is now a fact about the listing", singular and true of the listing it was looking at.

**The reachable one is `offload audit`, and it is the worst rendering in the CLI.** Staged with
seventy-four's own collision — `every 1m` beside `every 2m` firing a task, one daemon, four
minutes — `ps` widened to fourteen while `audit` printed **four rows under one id**: `granted …
accepted … granted … accepted`, every one at epoch 1. That is not a listing that looks duplicated,
which is what `ps` showed before its fix. It is the signature of *an arbiter spending a token
twice* — the single thing the audit log exists to let somebody detect, and the reason anybody
opens it — rendered out of two ordinary runs. Typing the id back is refused as ambiguous, which is
seventy-four's other fix working, so it is not a dead end; but the refusal only arrives if
somebody distrusts what they just read. `id_width` over its own rows, and the merged block becomes
two runs with one grant and one accept each.

**The unreachable one is more interesting than the fix.** Four sites print `offload approve <run>
<tool_use_id>`; `offload logs` echoes what the operator typed and is right by construction, and
the other three abbreviated to twelve — `offload explain`'s doing it **four lines under a header
seventy-four had deliberately made print the id whole**. One screen, two widths. All three print
it whole now, because an instruction line is not a table and has nothing to line up with.

It cannot currently go wrong, and the reason is a coupling nothing in the tree states: derived ids
come only from `Schedule::occurrence_id`, rules use `Uuid::now_v7`, and `offload every` has no
`--ask` — clap refuses it, measured. **A run that can be blocked on a question can never have an
id that names a tick.** Two features that know nothing about each other, held apart by the absence
of one flag, checked by nothing. Cheaper to remove than to document.

**What the session is really about.** Seventy-four's fix was correct, tested, well-reasoned and
applied in one of the four places the fact was read. Nothing about reviewing it would have caught
that — the diff is right. What catches it is asking, once, *where else is this rendered*, and the
same question generalises: the sweep that found both of these is thirty seconds per command and
had never been run in seventy-six sessions.

## Session seventy-six — `offload revoke`, sound, and three reports around it wrong

The command no session had ever typed, and the eighth walk in a row to find the thing under test
**sound** and the screen around it not. The mechanism is right in every arm: alpha revoked bravo,
bravo learned it from `Refusal::Revoked` on its own next dial (ADR-0044) and stood down within a
second — `accepting no — this node has been revoked from its fleet` — and alpha's `offload nodes`
had it `dead`. Every refusal is right too: a wrong passphrase names both fleets, a self-revocation
is flagged in as many words, non-hex and empty are refused.

**The largest finding is not about the screen at all until you look for it: the command could not
be typed.** `offload revoke <node>` and `offload rekey --evict` both parsed 64 hex characters, and
`offload nodes` and `offload nodes --history` — the two commands that list peers — print twelve.
Measured across every report on the revoking node: the peer's id did not appear in full anywhere
until *after* it had been revoked, when it turned up in the `revoked` list. The two sources that
had it were `offload id` on that device and that device's `fleet.json`. So revoking a device meant
going to it, and the device you cannot go to is the one the command exists for. It resolves a
prefix now against what this machine witnessed, refusing no-match and ambiguity and naming the
candidates in full; 64 characters still bypass the lookup, so a device never met is still nameable.

**The second is ADR-0060's finding at the sibling command.** A device just revoked was told, on
`offload status`, `2 member(s) met, 1 approver(s) · this node's certificate lasts 30 more days`,
`revoked 1 device(s)` — that device being itself — and a note to grant `approve` to a second
device, which it may not do, naming the machine that had just evicted it. `offload fleet` one
command away said `cert UNUSABLE`. ADR-0060 fixed the rekey half and this half was never walked.
The difference between them is the evidence, and it is why one is a count and the other a `bool`:
a rekey's refusal is an uncheckable claim by a peer, and a revocation is signed by the fleet key.
`Supervisor` had been acting on it for four sessions, which is what made the `accepting` line
honest while everything under it was not.

**The third was found by the control arm and is a count that was wrong on one node only.** Four
`offload revoke` calls naming one device, two devices revoked in total: alpha said `revoked 5
device(s)` and bravo said `2`, and bravo — which only heard it by gossip — was right. One fact
with two dedup rules. `FleetState::file` deduplicated on `(member, serial)`, which is right for a
relayed copy and can never fire for a second local call because `revoke` mints the serial from the
clock; `NodeMembership::file`, one layer up on the gossip path, has always asked
`is_revoked(member)`. So the daemon's copy of the rule was correct and the CLI's was not, which is
why the duplicates never spread and why the node the command was typed on was the only one wrong.
`covers` ignores the serial, so nothing decided differently — the cost was a record gossiped every
round for ever and a count of *records* wearing the word `device(s)`.

**The guard that passed for the wrong reason has a name worth reading twice.**
`filing_the_same_revocation_twice_records_it_once` files one `Revocation` **object** twice. That is
the relay case, it is real, and the test is correct. Its *name* covers both cases and its body
covers one. When a dedup key has two fields, the test that exercises the case where both match
proves nothing about the case where one does.

**Two smaller ones, both about silence.** A second `offload revoke` reprinted the whole paragraph
about what had just been recorded and what the daemon was about to hang up on, having written and
sent nothing — seventy-four's `Removal` rule at the membership tier (`Revoked::{Now, Already}`).
And `offload fleet` printed `host-runs dormant for another 14 minutes` directly under `cert
UNUSABLE: … has been revoked`, a countdown to something that will never happen: the `if` was on
the statement after the one that decided, which is the removed schedule's `home` line again.

**One mistake of this session's own, and it is about the method rather than the code.** The
never-met warning was written to ask `name_of` *after* filing the revocation — and `witnessed`
reads the revocation list, so a stranger revoked a moment earlier had a name and the sentence
never printed. It was found by running the arm, in a change that had just been built and reviewed.
**Walking the arm is not a formality once the code looks right**; it is the only thing that found
this, and the fix was one line.

## Session seventy-five — a refusal that sent somebody to look for another machine

The one known defect the last handoff carried, fixed, plus its controls. Not a walk of a command
off the mention loop — item 1 on the pick-up list was cheap, specific and measured, and it is the
first thing anybody meeting ADR-0061's premise runs into.

**The defect.** `offload run --repo <a directory that is not a git repository>` was refused with
*"is a path on another machine"*, said by the node it was typed on about a path sitting right
there. Reproduced on one daemon in the default configuration (`[cluster] enabled = true`,
loopback, no mdns, no seeds — a real one-node bid round) in under a minute: `/tmp/ow/notarepo`
and `/tmp/ow/no-such-path-anywhere` produced character-for-character the same sentence.

**One field.** `WorkspaceManager::can_obtain` answered a `bool`, so `LocalFacts::repo_available`
carried a bool, so by the time `bid::evaluate` had to word the refusal the measurement was gone
and the only thing left to word it with was `RepoSource::parse(repo).portability()` — which is a
fact about the *string*, identical on every machine in the fleet by design, and answers
`NodeLocal` to both causes. The message was right about the class of repo and had no way to be
right about this node, which is the only thing the reader is standing in front of.

`RepoReach::{Obtainable, NotHere, NotARepo}`, measured in `WorkspaceManager::reach` — the one
place in the path that can look at a disk — and carried on `LocalFacts::repo_reach` into
`NoBid::RepoUnavailable { repo, reach }`. `NoBid` is rendered to a string before it travels and
nothing else reads the fact, so there is no wire or schema change and no ADR: this is a report
defect, not a decision.

**Walked with three controls**, because the whole question is whether the two cases now look
different. A path that exists on no machine still says *a path on another machine*; a plain
**file** says *here, but it is not a git repository* (right — there is nothing to clone); and a
real repository passes the gate and is refused for cpu pressure instead, which is the control
proving the check was not simply turned off. `offload explain` on the queued case says the same
sentence as `offload run`, because it re-canvasses through the same `evaluate` — the two cannot
disagree.

**What it does not do.** It closes nothing in ADR-0061. A directory that is not a repository is
still refused by every node; it is now refused with a sentence somebody can act on. The ADR's
two open questions are unchanged and are still the first thing to read about workspaces.

**One thing worth carrying.** `""` is what a *task* hands `reach`, since a task names no repo, and
it comes out `NotHere` rather than `NotARepo` because `Path::new("").exists()` is false. The guard
that stops a task being refused for a repository it never named is the `if let Some(work) = agent`
above and is unchanged — but a field that grew a third value is a field every comment about it had
to be re-read against, and that comment named the old `bool`'s answer in as many words.

## Session seventy-four — an id that named the tick, and a workspace that is not a repository

Two commands off the mention loop, the two *removal* commands the last session's handoff pointed
at: **`offload unwatch` is sound and `offload unschedule`'s mechanism is sound**, and three
reports around them were not — plus one finding that is not about either command and is the
largest of the four.

**Both resolvers are good, and that was the control arm.** `offload unwatch` and `offload
unschedule` were walked through every refusal before anything was removed: non-hex needles,
empty, a hex string matching nothing, a rule id handed to `unschedule` and a schedule id to
`unwatch`, an ambiguous single character (three rules starting `b`), `%` and `_` (the wildcards
both resolvers' doc comments warn about — blocked by the hex guard), leading and trailing
whitespace, and uppercase. Every one correct. Then the removals themselves: `unwatch` on a rule
firing every three seconds took the node from **115 records to 14** in one call, `offload rules`
dropped the row, and `offload triggers` flipped to `watching — but no rule is bound to it` with
`RULES 0` — which is the one sentence that report exists to say. `Prune::Finally` does what its
entry claims.

**The finding is one field over and is structural.** `offload ps --all` printed **two rows with
the same id**, four minutes into the walk, and then again two minutes later. Not a flake:
`Schedule::occurrence_id` derives an occurrence's `RunId` from the tick and the schedule, putting
the tick's milliseconds in bytes 0..6 — which is *exactly* the six bytes `RunId::short` displays —
and a digest of the schedule after them. So two schedules that share a boundary (`every 1m`
beside `every 2m`, every other minute; `15m` and `1h`; `1h` and `24h` at one offset) produce two
different runs whose displayed id is character-for-character identical, on every shared boundary,
for ever. Both resolvers then refuse them — correctly, no wrong run is ever acted on — with
*"ambiguous — use more characters"* and **no more characters anywhere on the screen to use**. A
report that prints an identifier every command rejects, and an error telling somebody to do
something the screen makes impossible.

**Session nineteen found this, in test clothes, and left the product half deliberately.** It was
right to: the flake was a test using `short()` for a lookup, and the judgement was "a papercut",
with the note that *"printing fourteen characters would reach one random byte and make it a
1-in-256 papercut instead, which is a worse kind of rare."* That argument rests on the tail being
random, and it was — for forty-eight sessions. ADR-0056 replaced the tail with a digest and
nobody re-took the measurement. Same shape as an inherited `[x]` on the roadmap, and `id.rs`'s own
comment names the lesson it was written to record: *"the one thing `RunId::short` had to learn: an
id that names two things."* The fix is not a longer fixed prefix — the width is now a fact about
the **listing** (`id_width`: the shortest whole number of bytes at which the rows on screen are
distinct, never below twelve, so an ordinary `ps` is unchanged), both spellings of the ambiguity
refusal **name the candidates in full** so an id that arrived from a notification or a colleague
is still actionable, and `offload explain` prints the id whole — it had been re-collapsing to
twelve the id somebody had just typed in fourteen.

**And two on the `unschedule` screen itself.** `offload unschedule` on a schedule already removed
printed `removed <id>` and the entire paragraph about what had just travelled, exit 0, having
written nothing and republished nothing: `remove_schedule` returned one `bool` for *there is such
a schedule* and so answered `true` to both a removal and a no-op. `Removal::{Done, Already,
Missing}` now, with the *first* tombstone's instant reported, because that is the one the fleet
agrees on. Still exit 0 — the schedule is gone, which is what was typed — but not described as an
act. `offload unwatch` had this right by accident, because a rule row is `DELETE`d. And the
`home` line of a removed schedule still said `fired from here right now`, directly above the
`removed` line saying it fires nothing: the steward is computed for a tombstone too and answers
`Here`. Session sixty-nine fixed this defect on this report one field over; it came back through
`removed` rather than through the steward. `steward_note` is extracted so the rule is a test.

**One asymmetry checked and deliberately left**: `unwatch` prunes immediately and `unschedule`
leaves it to the fifteen-minute retention pass, because a schedule's occurrences are still being
gossiped and a record deleted inside `GOSSIP_TAIL` comes back untagged and unprunable for ever.
Recorded in `docs/pitfalls/rules-and-retention.md` precisely because "make the two consistent" is
a plausible-looking change that would reintroduce it.

CLAUDE.md's pitfall-size sentence was re-measured and corrected for the third time:
`reports-and-cli` is ~9k tokens, not "around 6.5k".

**And then, at the owner's request, ADR-0061 — proposed, not built.** It went through three
shapes and the two that were deleted are the useful part of the record.

The gap is real and reachable in one command: `offload run --repo <any directory that is not a git
repo>` is refused by every node, and the refusal says *"is a path on another machine"* about a path
on the machine it was typed on. `can_obtain` conflates *not here* with *here and not a repository*.
That half is a plain report defect and is left on the pick-up list; the design question underneath
it is that **the ordinary workspace is not a git repository at all.** Measured on the workspace
this was asked for (`~/client-project`): root not a repo, 6.5 GB, one nested repo whose `.git` is
4.5 MB against a 3.6 GB working tree with 27,439 ignored entries, plus a 1.1 GB non-git site. So it
cannot be submitted — not migrated, not even started locally.

**Draft one invented a manifest of parts**, each declaring how it is obtained (`Git` / `Copied` /
`Provisioned { command }`), with per-part constraints in the bid round. **Draft two added a
peer-served `git upload-pack`** so a cold node could fetch a repository it had never seen. Both
were Offload trying to be clever about a directory only the agent understands, and both fell to one
sentence from the owner: *the AI agent will decide what works best and Offload provides the tools.*

That premise is what makes the design small, and the thing I had wrong was **where the deciding
agent is**. I objected that it would not be there for the ungraceful case — a closed laptop, which
ADR-0016 itself calls the single most likely event here — and costed an agent turn per migration.
Both objections dissolve once the submitter is the operator's *own session*: it is already running,
already has the context, and drives `offload` as a CLI. And the judgement is needed exactly **once**,
at the boundary where work leaves the machine — because if the receiving node `git init`s the
materialised copy in its own worktree, the run has a base commit and a branch from turn 1 and every
existing mechanism applies unchanged for every later move. No agent in the loop after the first hop.

The same realisation killed both drafts' machinery. If the agent hands over a curated archive,
there is no manifest to author; and if history matters, `.git` goes in the archive at a measured
4.5 MB, so there is no `upload-pack` to serve. One mechanism covers the non-repo directory, the
local-only repo, and the `Fleetwide` repo whose origin the receiving device cannot actually reach.
What survives from draft one is exactly one clause — **acquisition separated from checkpointing** —
because checkpoints are per turn and without the split "send the workspace" means sending it every
turn.

Two things I pushed back on and then wrote in as the owner decided: **size** (6.5 GB across their
own network is acceptable — so the size objection ADR-0016 rejected the mirror on is withdrawn, and
what survives is cadence, not volume), and **who caps the payload** (the agent chooses, the
receiving node's policy caps, because `allowlist.rs:363` already settled that content does not get
to grant itself — an agent-chosen payload is that same shape).

Worth carrying: **an objection can be right about the mechanism and wrong about the setting.**
"The agent will not be there to decide" was a sound objection to a design where Offload spawns the
deciding agent, and irrelevant to one where the operator is already talking to it.

## Session seventy-three — the front door that never learned its neighbour's warning

Two commands off the mention loop. **`offload id` is sound** and took four minutes to clear: the
id is stable across calls, survives `offload init` on the same directory — which is what makes an
invitation issued before enrolment still valid — and `offload id`, `offload fleet` and `offload
status` all print the same string. Nothing to fix, and it is off the list with evidence rather
than by assumption.

**`offload every`'s refusals are all good.** `every 30s` is refused with a reason worth reading,
`every banana` names the accepted units, `--at 20m` on a 15m period is refused as naming no
instant. What it did not do was warn.

`offload every 1m --task nosuch` was accepted **in silence**. One command over, `offload when
--task nosuch` has warned since ADR-0032, on exactly the same predicate — and `offload every`'s
own help promises the schedule does *not* die with the device you type it on, which makes the
argument for warning stronger rather than weaker. The consequence, measured: the bid round refuses
the occurrence at every tick for ever (`ineligible: has nosuch as execute`, a dozen attempts per
tick, INFO-level), and `offload schedules` prints `nothing fired from here yet` — the same
sentence it uses for a schedule created ten seconds ago. There is no state that separates them:
`schedules.last_tick_ms` is set after a *successful* placement.

The comment directly above `Request::Every` is about this class of omission on its neighbour. It
did not prompt anybody to check the arm underneath it, which is the argument for typing the
command rather than reading around it.

**The part of the fix that is a decision** is that the sentence is not shared. `task_note` ends
"`offload rules` counts those under DROPPED, with the reason", which is true of a rule and false
of a schedule — nothing counts a refused tick. Borrowing it would point somebody at a report that
will never mention their problem. So the *predicates* are shared, which is what must not diverge,
and the consequence is worded per command — the same split ADR-0032 already makes between
`offload run --task` (a refusal) and `offload when --task` (a note). The schedule's note says the
gap out loud: *"Nothing counts a refused tick, so `offload schedules` will go on saying nothing
has fired from here."* That is a promise, and it was measured a tick later.

An aside worth keeping, because it cost ten minutes and produced a false finding on the first
pass: `--task` takes a **service**, not the `[[tasks]]` block's `id`. Both are required and only
one is the name you schedule.

No ADR — the decision ADR-0032 already made, applied to the caller that was missing. One test, 999
total, clippy clean, no wire or schema change.

## Session seventy-two — the evicted device that was told its fleet was fine

`offload verify` came off the mention loop, third session running, and it is **sound**: six cases
— the right phrase, a wrong one, on a founder, on a device that was *invited* and never saw a
phrase, and on both sides of a rekey. Its wrong-phrase error is better than it needed to be, since
naming both fleet ids (`it derives 7ec07847, this node belongs to 7289c74e`) is what separates a
typo from holding the previous fleet's secret. Nothing to fix there. The finding came from staging
the rekey **with the daemons running**, which the record had never done — rekey was walked as a
CLI operation only.

**The mechanism is right and every report was wrong.** Three seconds after `offload rekey` on
alpha, the two daemons stopped talking in both directions with no message needing to arrive, which
is exactly what the convergent revocation promises. Then: `offload nodes` on the evicted device
said `alpha dead` — alpha was running and had *answered* — and `offload status` said
`2 member(s) met`, with its only note warning about *losing* the approver that had already gone.
The honest sentence existed the whole time, `that certificate is for fleet bf36a64a, this is fleet
433de62b`, reprinted every twenty seconds into a `WARN` line nothing reads.

**ADR-0060: it is reported, not believed.** ADR-0044 closed the sibling path, where
`Refusal::Revoked` carries a signed revocation the subject verifies and acts on.
`Refusal::WrongFleet` has nothing to carry — a re-founded fleet revokes nobody — so no credential
could make the claim checkable, and a device that stood down on an uncheckable claim is one any
peer could switch off. What is reportable is the half that is this node's own: it dialled, it
reached something, and it was turned away, which is a different fact from `no answer` and was
indistinguishable from it in every report. `Turnaways` mirrors ADR-0059's send counter exactly —
total, capped breakdown, the peer's words unreworded, no decision anywhere — and the ADR records
the three shapes that were refused, the worst being a second liveness opinion beside the detector.

**And the control arm found a defect in the fix.** The evicted device's report came out right
first time; the same command on the machine that had *done* the rekey told it to go and take up
the invitation it had itself issued. Both ends of a rekey are turned away by the other in
identical words — `WrongFleet` is symmetric because the situation is — so the refusal cannot say
which end you are on. The discriminator is `FleetState::last_rekeyed()`, this device's own
recorded action, which is the only evidence about itself a node here may reason from. Only the
evicted device's note carries "this node cannot check that claim and has not acted on it"; the
other is a node describing its own deliberate act, where there is no claim to disclaim.

Also written down, so nobody builds it: **a wrong passphrase is recorded nowhere and that is
right.** The key is derived locally, so there is no remote oracle — a failed attempt is somebody
with a shell on the machine, who already has everything the log would warn about and can delete
it. What a log of typos would buy is noise in the one place ADR-0012 needs read.

ADR-0060, five tests, 998 total, clippy clean. No wire or schema change: the count is an
observation about this process on this machine, so it is neither gossiped nor stored, and
`NodeStatus::turnaways` is the control socket with `#[serde(default)]`.

## Session seventy-one — two commands, one run, contradicting each other

`offload priority` came off the same loop `offload deny` came off one entry ago, and its doc
comment carries four claims worth measuring: priority decides who yields, a low-priority run on an
idle fleet starts immediately, a high-priority one preempts nothing, and a run that has waited long
enough outranks any priority by itself. **All four are true.** What the command *says* while they
are true was not.

**`pending?` is two arms where there are three states.** `edit_spec` answered every non-pending run
with "it is already under way" — but a run this node has accepted and not started is `Assigned`,
which `offload ps` renders *waiting for a slot*, and it is the only state in which an order exists
at all. So both editable fields, typed at the queue they order, reported that the run had begun.
Two commands one run apart saying opposite things, which is the second time in two sessions that
the cheapest defect to see was the one standing there.

**The deadline's understatement was the larger one.** One daemon capped at one slot, three runs a
second apart, control measured first: with no edit the older queued run goes next. Then
`offload deadline <older> 1h` — and the newer one started instead. An unstated deadline means *as
soon as you can*, so it sorts ahead of any time you can name, and putting a deadline on a queued
run usually sends it **backwards**. The note said the change affected "only how long the fleet
waits for it if something goes wrong". Priority has the mirror trap: slack leads and priority only
breaks its ties, and two runs submitted a second apart with no deadline never tie — measured, a run
set to priority 10 started after one set to −5, correctly, and nothing said the command had done
nothing.

The fix is a third arm whose position comes from `held_but_not_started()` read back after the edit
— the function that decides the order, not a second reading of `urgency_order` — so the note now
predicts the reordering, and the re-walk confirmed the prediction. The control is both commands
against a genuinely running run, where the original sentences print unchanged.

**And `priority` was readable in no report at all.** The command echoed the number once and nothing
would say it again — least of all `offload explain`, which already prints `due`, the slack that
leads the same ordering, and `held back`, which appears in exactly the state the pair is ordering.
Half the decision was visible and the half somebody had just set by hand was not. It is on
`explain` now, under `due`, omitted at its default.

No ADR: ADR-0013 is right and this changed nothing it decided. Two tests, 993 total, clippy clean,
no wire or schema change — `Explanation::priority` is the control socket and carries
`#[serde(default)]`.

## Session seventy — the command worked; the screen around it did not

`offload deny` had two mentions across `DEMO.md`, `HANDOFF.md` and `sessions.md` — near the top of
the loop that ranks commands by how often anything has typed them, and the method session
sixty-nine left behind. It got ADR-0017 end to end on two daemons: a fake agent taking one clean
turn, then piping a line into `offloadd ask-hook`, then `offload deny`.

**The mechanism is sound, including the half that is the whole point.** The hook received
`{"permissionDecision":"deny"}` and the agent carried on; the log recorded `-> denied (an
operator)`; the registry cleared; the error sentences for an unknown run, a wrong `tool_use_id` and
a question already answered are each right. And typed at **bravo** for a run held by **alpha** it
was forwarded and answered in one round trip, logged on both machines as `denied (an operator on
bravo)` — "answerable from any device" measured rather than asserted. That is the third walk in a
row where the mechanism under test passed and the screen around it did not, which is now less a
coincidence than a description of where this tree's defects live.

**`offload explain` closed a running run by pointing at a line it had not printed.** *"It is taken,
not unplaced — `held back` above is why it has not begun"* is the third of three trailers, and it
fires for any held run nobody is bidding for. But `held_back` is only ever computed for a run in
`assigned`, so on a `running` one the sentence referred to nothing and contradicted the `state`
line eight rows above it — a run that started nine seconds ago, said not to have begun. It is the
shape session sixty-nine named and one layer out: not a report built from `Option == Some`, but a
sentence written against the one state that was on screen at the time, rendered in every state that
reaches its branch. Now `taken_not_unplaced` picks from what was actually printed, tested, and all
three arms walked live.

**…and the state it was blind to is the one it is most typed for.** A run stopped mid-tool-call is
`running`, holds its lease, and does nothing for up to five minutes by design — the only way those
two things are both true — and explain had no field for it at all. `offload asks`, one command
away, had the question, the clock and the id. Explain now carries `waiting`, read from the same
registry on the holder so the two cannot disagree, and from the registry rather than the log
deliberately: the log says a question was *asked*, and whether it is still waiting is exactly what
the log does not know.

**Three commands offered an answer to that question and each offered a different half.** `offload
logs` printed a literal `<run>` in the only line of that log that is an instruction — while `logs`
had the run id as its own argument the whole time, because `print_event` took only a `&LogKind`.
`offload asks` printed the id and named only **approve**, in front of a `WANTS TO` column reading
`rm -rf /important`. The drain's `Blocked` step had both and was the only one. The divergence
between sites rendering one piece of advice is not usually about whether it is true; it is about
which half each author left out, looking at a different screen.

No ADR: nothing here is a decision. One test, 991 total, clippy clean, no wire or schema change —
`Explanation` is the control socket, and `waiting` carries `#[serde(default)]` so a daemon too old
to send it renders the fallback rather than a wrong sentence.

## Session sixty-nine — the machine that would not speak, and the schedule nobody was firing

Three things. The first was phase 6's last argued item; the other two came out of following this
file's own standing advice — that the ordinary path is where the defects are — first to the newest
mechanism nobody had walked, then to the *commands* nobody had typed. Written newest-first, because
the later two carry the rules that generalise.

### A schedule reported as somebody else's job when it was nobody's

ADR-0056's tombstone had never been walked — it is the newest gossiped fact in the tree and the
reason wire v29 exists — so it got two daemons and a one-minute schedule. **The tombstone is
sound**: a removal typed on the *peer* reached the home in eight seconds; a removal typed while
the peer was killed reached it when it came back, and the peer fired nothing in between. Two of
ADR-0056's own sentences turned out narrower than what was built, and both are amended at the foot
of it — any node may author a tombstone (which is what makes a retired home removable, and the
merge is a minimum so it converges anyway), and *"a schedule survives the device that created it"*
holds while that device is **remembered**, not once it is forgotten.

**The defect was the report, and it is this project's most-repeated shape.**
`ClusterView::steward_of` answers *me*, *that node*, or **nobody** — the third when the home is not
in this node's view at all, which any daemon reaches by restarting while the home is off. (The
middle answer, a home that is *gone but remembered*, was session sixty-seven's measurement and was
not re-taken here.)
`ScheduleReport::mine` was a `bool` over that, so `offload schedules` said **`fired elsewhere`**
about a schedule nothing anywhere was firing, and printed a countdown to the next tick underneath.
Measured across two whole tick periods with nothing firing. The tell was one line above the lie:
the `home` column had already fallen back to a bare node id, because that node is not in the view
either — the same absence told honestly, immediately above a line contradicting it.
`Steward::{Here, Elsewhere, Nobody}` now, the `Nobody` arm says what to do instead of printing
arithmetic, and both halves of what it promises were then measured — the home returned on the seed
backoff and the schedule resumed by itself. `steward_of` had **no tests at all**; it has three.

**Worth carrying: when a report is built from `Option<T> == Some(x)`, ask what the `None` means.**
If it means something of its own, the report has three states and a `bool` renders the third as
whichever of the other two the `else` happened to be.

### …and a trigger credited with a rule only a notice can fire

The method that found it is worth more than the defect. Cross-referencing every CLI subcommand
against `docs/DEMO.md`, `docs/HANDOFF.md` and `docs/sessions.md` turns up the commands nobody has
ever typed in a recorded walk — **`offload triggers` had zero mentions**, across the whole history
of this repository, and it is the report for a tier that grew a new firing source one session ago.

ADR-0057 let a rule be bound to a **notice**; both names live in `rules.service`, told apart by
`fired_by`. `fire_one` asks both questions and names the collision in a comment — `Service::Other`
accepts any string, so a trigger whose service is called `failed` is legal. **The hypothesis going
in was that the *firing* path was unfenced, and measuring showed it was wrong**: seven trigger
events, nothing fired, guard present and correct. What was unfenced was `report_triggers`, which
counted by name alone — so `offload triggers` said `RULES 1` / `watching` where a minute earlier it
had correctly said `0` / *"watching — but no rule is bound to it"*, the sentence its own source
calls the commonest reason a trigger does not work. The control is in the same walk: the
*trigger*-bound form of the identical name brings the count back and fires on the next line.

**Worth carrying: when a column starts holding two namespaces, every reader of that column is a
place the old assumption survives — and the reader that *decides* gets fixed first, because it is
the one somebody writes a test for.** The readers that only report are found later, by somebody
typing the command. Nobody had.

### A send this machine refused is counted, not decided on (ADR-0059)

**Phase 6's second item, and the argument it was waiting for came out the way the roadmap
guessed.** `quinn_udp::UdpSocketState::send` answers `Ok(())` to every send error but
`WouldBlock`, and its doc comment defends that correctly: UDP send errors are non-fatal, and a
transport that tore a connection down on one would be worse than the silence. Nothing in ADR-0059
disputes it. What the doc comment does not say is where the *symptom* goes — quinn believes the
datagram left, so the failure detector reports `no answer within 500ms` about a machine that was
never spoken to, and every report in the product then blames the peer. That is exactly what made
session sixty-six's macOS fault take three sessions: the one machine that knew said nothing, and
the only evidence that ever named the cause was an `EHOSTUNREACH` seen by hand under `strace`.

**So it is a count, and the change is the smallest one that separates two facts.**
`QuicTransport` binds its own socket and hands it to `Endpoint::new_with_abstract_socket`;
`sends::WatchedSocket` is a copy of quinn's private tokio socket with `try_send` where quinn calls
`send`, and it then answers what `send` would have, arm for arm — `WouldBlock` handed back,
`EMSGSIZE` ignored as an MTU probe, everything else counted against the destination and answered
`Ok(())`. Behaviour is unchanged by construction, which is what makes it not a decision. The two
shapes that *would* have been decisions were both refused and both for the same reason: acting on
a send error puts a second liveness opinion beside the detector, computed from a different input,
and a Prometheus counter has the phase-6 metrics item's problem — nobody scrapes their phone.

**The muzzle turned out to be rootless and one line long,** which is what made the walk cheap
enough to do properly. A UDP socket with `SO_BROADCAST` unset — quinn never sets it — is refused
`EACCES` for every datagram addressed to the broadcast address, and nothing leaves the host. So
`seeds = ["255.255.255.255:7433"]` on one daemon is a node whose every send is refused, on the
ordinary seed-dial path. Measured: `sends 9 refused by this machine's kernel`, sixteen twenty
seconds later, with the daemon's own INFO log saying **nothing whatsoever** and `offload nodes`
reporting a healthy fleet. The control is the same walk with a real peer — two daemons meshed on
loopback for fifteen seconds, neither printing the line, then bravo `kill -9`'d, three `no answer`
lines, `dead ~2`, and still no line. Two failures with one symptom, now told apart. The same
muzzle is a unit test, and with `send` restored in `try_send`'s place it fails on *the kernel
refused every datagram and nothing counted one*.

**The thing worth carrying is about attribution, not about UDP.** When a layer below you discards
an error, the report above you is not missing a detail — it is confidently blaming the wrong
machine, and it goes on doing so until somebody reaches for `strace`. The swallowing was right;
the silence about *who* was silent was not, and those are separable. Also worth noting: the
roadmap's own table was stale in the direction the handoff warns about — phase 8 read `not
started, 0/5` while its body said all seven items were done and its demo had been run.

## Session sixty-eight — phase 8's demo, run as one sentence, and the two reports it caught

**The whole demo, on two daemons.** Everything in it was built and each clause had been walked
alone; what had not happened is running them at once, from one node to another, with a person's
command at one end and a push at the other. The staging that made it possible on one machine —
and made the Mac unnecessary for this — is that **the tier decides where a run lands**: alpha with
`agent.binary` pointing at nothing plus both `[[tasks]]`, a `[[triggers]]` ticker and a
`[[sinks]]` push route; bravo with a fake agent and no tasks. A task submitted from bravo was
accepted by alpha, the schedule fired on the boundary, the ticker fired a task, a person's failing
task fired the escalation, each escalated agent run was placed on bravo, and the sink on alpha
carried each failure. Two machines would add the network, which sessions sixty-five and sixty-six
measured; the placement is what this proves.

**Both defects it found were reports, and both were invisible from the node that did the work.**
That is the whole argument for submitting from the other one, and it is the sharpest thing this
session has to say.

**One: `offload logs` on a task printed nothing at all.** Exit 0, no output, on the node the task
was submitted from — which reads as *that run produced none*. `server::log_source` asks
`RunProgress::by` which machine serves a finished run's log, on the sound grounds that the leg
which wrote the numbers wrote the log; a task writes no numbers, so nothing stamped the leg and
the answer fell through to the run's **arbiter**, which for a run submitted here is here. This is
the same silence session sixty-two removed for an agent run, arriving for the cheap tier through
the fact it turned on being an agent's side effect. The control was in the same walk: the
escalated *agent* run's log forwarded from the same node, over the same connection, a minute
later. `Supervisor::note_log_leg` writes the stamp where the log starts, with an empty closure,
because for a task the write *is* the signature.

The generalisation: **a fallback that covers two cases will cover a third one badly.** That
fall-through was documented as the pre-schema-v7 row — a real case, rare and harmless — and a
whole tier landed in it.

**Two: `offload explain` promised a retry the daemon could not make.** `decide_recovery` answers
`Recovery::Resume` for an unattended failed task, and the arm that acts on it called
`Supervisor::resume`, which refuses a task on its fourth line for want of a conversation. Both
halves right in isolation: one decision, two mechanisms, and only one of them existed. So the tick
asked every thirty seconds, was refused in the same millisecond, **counted the refusal as a spent
attempt**, and the program never ran again — while the report said *"picking it up again in 16.7s
(try 1)"* and the log told an operator a shell script had no conversation to continue. ADR-0058:
`Supervisor::restart_task` is the task tier's door, dispatched at the tick on `Work::kind()`, and
`offload resume` still refuses a task because that door is the one a person types at.

**The first fix for it was wrong, and four tests said so on the first run.** The attempt was an
escalation — *a program that already ran is not run again by itself* — with a real argument behind
it: a task that exits 2 may have sent half its mail. It contradicts a position the tree already
holds and walked one session earlier: ADR-0043 has a **departing** node hand a failed task to the
fleet precisely so another node re-runs it from its spec, and the test that names that walk failed
immediately. **Before deciding a mechanism is missing, find the place that already decided it was
not.** The argument itself is not lost — its home is a restartability field on `TaskConfig`, where
the owner who wrote the program is the one answering, and it is unbuilt because nobody has asked.

**A stated consequence, measured rather than predicted.** With the restart working, one failing
watcher produced **three agent runs on the peer and three identical pushes** in two and a half
minutes: each failure is a new `Notice::Failed` with a new sequence, so ADR-0057's escalation
fires once per failure. Before the fix it fired once, and that was a symptom rather than
restraint — there was only one failure because nothing retried. Bounded by `max_resumes`, so it is
three and not unbounded, and both levers that would reduce it are somewhere else and unbuilt.

**And a third thing, one line long and worth the same lesson twice.** `--on-notice`'s own help
text gave the example `offload when failed --on-notice --prompt "…"`. The prompt is positional, so
that line exits 2. Written one session earlier in the ADR's own vocabulary and never typed:
**`--help` is not a test and neither is an ADR.**

## Session sixty-seven — phase 8, all of it: the tier is reachable, the reports caught up, the owner got the demand axis, and the clock arrived

**The tier did not run anywhere it mattered.** Session sixty-six ended with `offload run --task`
working, walked and committed, and it *was* working — on the path it was walked on. With a
cluster, which is `enabled = true` by default and therefore every real node, `bid::evaluate`
refused every `Work::Task` at its first line: `walker  this node cannot host task work`. The
refusal was written when nothing could construct a `Work::Task`, said so in its own comment, and
was left behind by the three items that made one constructible. **The whole tier was behind one
door that nobody had thought to open**, and the previous session's walk missed it because the
config it walked had the cluster off — the mode `ClusterConfig`'s own doc comment calls "the mode
most of this project's testing happens in".

That is the second time in three sessions that an environment convenience decided what a walk
could see (the first was one Linux box with two ports standing in for two platforms). The
generalisation worth carrying: **walk the default configuration, and if the walk needs a
non-default one, walk it twice.**

**What `evaluate` now asks.** Three of its checks are about the agent half and are asked of it
alone — the repository, the account's rate limit, the checkpoint's agent version — and everything
else is about a *run*: the owner's standing answer, the demand budget, observed pressure, the
constraint tree, `AlreadyHolder`. Two of the three were not merely irrelevant but actively wrong
if hoisted: `LocalFacts::repo_available` is computed for a task as well (`can_obtain("")`) and
comes out `false`, so one `if` in the wrong place refuses every task on every node for a
repository nobody named; and `account_limited_until` is an *agent account's* rate limit, so a
node blocked until 09:00 would have reported `NotBefore` for a shell script it could run that
second — the cheap tier parked behind the expensive tier's bill.

**`WorkPolicy` gained a `Tier`, not an `Option`.** Three of its clauses are agent facts —
`allowed_agents`, the agent's own `max_concurrent`, an account's ceiling — so `permits` and
`admits` take `Tier::{Agent(&kind), Task}`, constructed from the run by `Work::tier`. An enum
rather than `Option<&AgentKind>` because a `None` that skips three of the owner's controls is one
an agent-run caller reaches by accident from a `RunSpec::agent()` that happened to be empty, and
`allowed_agents` has already been a control defeated by an ordering once. `AgentKind::Other`
makes this reachable rather than theoretical: a config naming some other agent would otherwise
have refused a shell script for not being that agent, on the one path — a fleet of one — with no
round to ask properly.

What is deliberately *not* skipped is the other direction. `accept`, the battery floor,
`allow_metered` and the demand budget are facts about the machine, so a phone told to work only
while charging does not run a shell script on battery either. A tier that skipped those would be
a way around the owner's own answer, which is exactly what ADR-0019 §4 refuses about letting
`--demand light` bypass a gate.

**A kind column is not one column.** The roadmap item was `offload ps` wants a *kind* column, and
it is one line of `println!`. Walking it found four more sentences written when an agent was the
only thing a run could be, and every one of them read as confident nonsense: `TURNS` printing
`0` for work with no turn boundary; `offload explain` labelling `Work::summary` as `prompt` over
a shell script's service; the closing log line and the **notification projected from it** saying
`finished after 0 turn(s), $0.0000`, which is a sentence that goes to somebody's phone; and
`offload policy` answering `would accept work: no — no agents installed` on a device that
nominates a task, which is **ADR-0019's own scenario** — the phone that cannot host an agent and
can watch an API — told the opposite of the truth. The rule the pitfall file already carried said
to run one and read the output. Doing it a second time is what found these; grep would not have.

The tier rides on the *event* (`LogKind::Finished::work`, a `#[serde(default)]` field rather than
a new variant, for `CaptureFailed::run_continues`'s reason) because `notify::notable` is a pure
function of one event and has no spec to look up. The default is `Agent` and is not a guess: a
row written before the task tier was written by a binary in which an agent was the only thing a
run could be.

**One more, found in the last five minutes and worth the shape of it.** The `kind` field started
as a `String` and was changed to a `WorkKind` so the CLI could compare a variant rather than
spell `"task"` in a second place — a strict improvement, and it broke the column: a `Display`
written with `write_str` silently ignores the width in `{:<5}`, where `f.pad` honours it. `task`
is one character shorter than `agent`, so every numeric column to its right sat one place left on
task rows. Invisible in a table with one row and obvious in one with a row of each tier, which is
the only way it was going to be seen. There is a width test now, because that part is
mechanically checkable.

**And a record was being written before the decision that record describes.** `build_task`
called `save_run`; `build`, the agent path it was copied from, never has. So a task submission
the round refused left a `pending` row on disk and in gossip, one line under `offload run`'s own
*"(use `--queue` to leave it pending anyway)"*. Measured on one daemon, and `offload explain` was
the honest report of the three: *"nobody holds it, and it was not queued — nothing offers it to
the fleet again on its own."* `place` records what it decides to keep, so a builder that persists
has made the arbiter's decision for it.

**What the walk proves now, on one machine.** A daemon with `agent.binary` pointing at nothing —
`agent=no agent at /nonexistent/claude` in its own startup line — won a bid for `--task webhook`
through the ordinary round, ran the program, and its output is the run's log; the same daemon
refused an agent run a minute later with `ineligible: has agent claude-code (not installed)`.
That is phase 8's demo minus the second machine and the escalation.

No ADR. Nothing here is a new decision: ADR-0019 says a task is a run and the round is the round,
and every fix above is that sentence being made true.

### §4, the owner's gates, in the same session

**The sentence phase 8 was written for is now sayable.** `[policy.light]` gives `accept`,
`min_battery_percent` and `allow_metered` a second answer for `Demand::Light`, so *take the light
watcher always, heavy work only while charging* is two lines of config. Every field is an
override whose `None` inherits, which is the whole shape of the feature: these are gates people
have already configured, and a light form that silently tightened or loosened one would be worse
than not having it at all. The class defaults state none — a phone still refuses light work on
battery until its owner says otherwise, because ADR-0019 §4 refuses making `Light` bypass a gate
and `Demand` is the *submitter's* word.

**The seam was already cut**, which is why this was the smaller half: `permits` had just gained a
`Tier`, and it gained a `Demand` beside it the same way. What took the thinking was not the gate
but its **callers**, and they split in two. Where a run is in hand its own demand is the question
— `submit`, `submit_task`, `resume`, `explain` — and where there is none the honest question is
about an ordinary run, which is now a named constant rather than a literal. Two had to be looked
up rather than guessed: the recovery tick was passing **one** `Hosting` for a whole pass over a
list that can hold a light watcher beside an expensive session, so it takes a closure and asks
per run; and `ready_elsewhere` was deliberately left alone, because its own docs say which way to
be wrong and a peer's gossiped battery is a second old.

**And the walk found the report, again.** A daemon configured `accept = "never"` with
`[policy.light] accept = "always"` ran `--task webhook --demand light`, refused the same task at
ordinary demand one command later — exactly right — and then `offload status` said `accepting no
— node is not accepting work` about a machine that had just done work. Both sentences were true
and the pair was the answer. That is the third instance this session of one rule: **a report
describes a machine, so it has to describe what the machine does**, and the way to find those is
to run one and read the output. `NodeStatus::light_refusal` travels beside the first answer and
is said only when it differs.

**Wire v28**, and the reasoning is v24's: a `WorkPolicy` rides inside a `NodeView`, a `NodeView`
is *relayed*, and the light form can tighten a door as well as loosen one — so a peer that
decoded the field away and won a merge could report a node as willing to do work its owner has
refused. Where the safe-sounding default is wrong in one of the two directions, the number moves.

**Two things about the instruments, both from this session's own mistakes.** The status test
first asserted `light work: yes`, which passed alone and failed inside the suite: `status` reads
the machine's real load average and `cargo test --workspace` pegs the CPU, so `admits` refused
under pressure. Correct behaviour, wrong assertion — the fix was to ask what the test is *about*
rather than to add a tolerance. And a workspace run that overlapped another reported `can't find
crate for offload_transport` in a doctest, which is impossible and was: two runs share
`target/`, and a concurrent build had replaced the `.rlib` rustdoc was handed by name. Serially,
clean. The second one also exposed a habit worth dropping — `grep -c FAILED` over a captured run
does not see a doctest failure at all, so **the exit code is the thing to read.**

### The escalation: a notice fires a rule, and ADR-0057

**Phase 8's demo ends with a sentence nothing could do** — *when the task exits non-zero, a rule
submits an agent run that the fleet places on a machine that does have an agent.* Nothing noticed
that a run had failed and started another. It does now: `offload when failed --on-notice --task
webhook`, or with a prompt for the agent tier.

**The decision that mattered was which existing mechanism it belongs to**, because all three
candidates work. A sink is a nominated program with the notice's summary appended to its argv, so
a two-line script calling `offload run` is an escalation *today* — and it has no loop guard, which
is fatal, and what it submits is invisible to `offload rules`. A trigger program could poll
`offload ps`, which makes this project's own output a machine interface and re-derives the notice
projection in `awk`. A `--escalate` field on the spec travels for free and is per-*run* where the
need is generic.

What decided it was reading the delivery tables: **the route key is a `TEXT` id**. So a rule can
*be* a route — `rule:<id>` — and the cursor, the `(sink, topic, seq)` dedup, the ordering by
`noticed_at_ms`, the per-route queue and the retries are the ones this plane already has, with **no
schema change and no wire bump**. Every one of those was a bug found by measurement in an earlier
session; none is obvious enough to get right twice.

**The loop guard is `Origin`.** A rule bound to `failed` fires a run; if that run fails it projects
the same notice; without a guard the rule fires again for ever. A notice about a run a *machine*
started is offered to sinks and never to rules — one comparison against a fact that is immutable
and gossiped. Walked, and it is the satisfying half of the walk: the escalation rule fired a task
that also exits 2, and nothing fired again. Two runs, not a chain.

**`Audience` is not asked of a rule route and `Notices` is.** An audience selects routes *to
people*, so reading `--notify nobody` as *do not recover this run* would be one axis doing the work
of two — ADR-0026's own mistake. The precedent was ten lines up in the pass: the fleet's own log is
offered to every route with no audience asked.

**And the delivery loop moved out of `main` into `serve`**, joining the trigger watchers and the
clock. Not tidying: firing a rule is a submission, submissions go through the checks in `server.rs`,
and those need the one `Ctx` — two of them would be two `Triggers` maps and two capability handles,
which is the divergence ADR-0048 was written about.

**The report was wrong twice, in the same way, and the walk found both.** With a second way to fire
a rule, `trigger_present` became the wrong question: `offload when` printed *"nothing on this node
watches `failed` — add a [[triggers]] entry"* about a rule the plane fires, and `offload rules`
printed the same sentence under an escalation rule that had **just fired**. Third instance of one
rule in a single session — a report describes a mechanism, so it has to know which one.

**One defect found and not fixed**, recorded with its evidence: on a fleet of one, a failed
*idempotent* run is re-offered at startup into a pool with no members, so it strands as `pending`
— and because `rule_run_in_flight` reads non-terminal as in-flight, a stranded occurrence **jams
its rule for ever**. `offload explain` is precise about it, so nothing is hidden; a task cannot be
resumed, so the way out is submitting it again. It predates the escalation and is easier to meet
with one, which is why it is now the top of the pick-up list rather than a footnote.

### …and then fixed, where its own name said it was

**No ADR — this is ADR-0043's rule met from a case it did not enumerate.** `fleet_could_start`
decides whether a node hands a failed run to the fleet or keeps it, and the name is the question:
*could anybody **else** start this.* For a `Resumable` run it asked `is_durable` — does another
node hold a copy of the conversation. For an `Idempotent` one it answered **yes unconditionally**,
because such work is re-runnable from its spec and there is nothing to fetch. True, and half the
question: somebody has to be there to re-run it. `Circumstances::alone` is the missing input, and
`ClusterView::alone` the one definition — asked by the recovery tick, by a departure and by
`offload explain`, because a fleet of one that two of those disagreed about would be worse than
the bug. Deliberately *any node this one has met*, not any node answering right now: a peer in a
bag comes back, and a `Pending` run waiting for it is what ADR-0043 wants.

**The fix is not only in the drain**, which is the part that needed looking up rather than
guessing: `LetGo` is reachable without departing, because a node whose owner will not let it host
(ADR-0049) releases a failed run the same way. `tend_own_runs` therefore takes the cluster for
**one** fact and its doc comment says which — the fleet's *size*, never its opinion, which is
what keeps that pass out of the gossip loop.

**And the causes are a 2×2, which the first version of the fix got wrong in the same way the
thing it was fixing did.** Leaving or refusing to host, crossed with no copy or no second device,
is four sentences; there were three, so an idempotent task — which has no conversation at all —
would have been told *"no other device holds a copy"*. That is `NodeIsDeparting`'s deleted mistake
(ADR-0034 §1) arriving from the other axis, and it matters because the two halves are fixed by
different actions: plug this one in, or add a second device.

**What the walk could and could not reach, said plainly.** The departing column is measured: three
failed tasks stay `Failed` across a drain and a restart, nothing is handed back, and `offload
rules` reads `last fired` instead of `running now` — the rule is no longer jammed. The refusing
column **is not walkable on one machine**, and that is a property of the gate: `permits` refuses
on `accept` and `allowed_agents`, which are config read once at startup, or on charging, the
battery floor and a metered link, which come off the probe. So the hosting gate can only disagree
with the placement gate that already let the run start after a *live* capability change —
ADR-0048's and ADR-0049's own premise, and not scriptable without root. The attempt (`always`, a
task that exits 2, a restart under `never`) showed something else worth having instead: the tick
iterates an **in-memory** map, so a restart considers no failed run at all and is not the cheap
way to re-drive this pass. The enforcement for that corner is the unit test, and the pitfall entry
says so rather than letting the walk be read as covering it.

### A rule fires either tier, which is the demo's first clause

**No ADR: this is ADR-0019 §2's split arriving at the one command that had not had it.**
`RuleSpec` stored a `SubmitRequest`, so `offload when` fired an agent run and a trigger could not
start a task — the first sentence of phase 8's own demo. It now stores a `RuleWork`, the control
protocol's own two-shaped choice, and `fire` has one dispatch point with the guards, the
recording, the tagging and the pruning unchanged either side of it, because all of those are
about a *rule* rather than about what it fires.

**Three things worth keeping from a small change.**

*The event stops at the tier boundary.* An agent rule appends the trigger's line to its prompt
under a heading that says it is data; a **task is not told the line at all**. That is ADR-0019 §1
rather than laziness: a trigger's output is text from outside, and handing it to a program the
owner nominated as `argv` is precisely the submitter-supplied command line that ADR refuses in
the strongest words it uses. If the payload is ever wanted, stdin or one named environment
variable is a decision with its own reasoning.

*`SubmitTaskRequest` had no `origin`.* The agent request has carried ADR-0024's fact since it
existed and the task one did not — invisible while nothing machine-started could be a task, and
wrong the moment a rule could fire one: the occurrence would have been recorded as an
**operator's**, so nothing would ever reclaim its record and `cleanup`'s premise that somebody
will read the worktree would have been applied to a run nobody typed. The compiler found it, which
is the good case.

*A precondition is a refusal on one command and a note on the other.* `offload run --task push`
is refused at the keyboard; `offload when --task push` is written with a warning, because a rule
fires for months and the machine that will nominate the program may not have enrolled yet
(ADR-0032, ADR-0036 — this file's own entry about `--use email` on a rule). Both read one
`nominates_task`, which asks this node's config and the fleet's advertised `Role::Execute`
capabilities. Walked including the promise the note makes: three firings refused, and `offload
rules` counted them under DROPPED with the round's own sentence.

And the same storage hazard as last session's, one type along: a rule is a whole `RuleSpec` as
JSON in `rules.request_json`, so the split changed what a daemon reads back from its own disk —
no migration to hang it on, no wire version to protect it, and a hand-written compatibility
fixture because a generated one changes shape in lockstep with the code it is meant to pin.

### §3, schedules — and ADR-0056, which is what §3 left open

**Phase 8's last item, and the one the graduated-cost argument actually needed.** Before this the
only thing that could submit a task was a person at a keyboard: `offload when` builds a
`SubmitRequest`, so even a trigger could not start one. `offload every 15m --task webhook` now
writes a standing instruction on a clock, and — the part that is not `cron` — **it is gossiped**,
so it outlives the device it was typed on.

**ADR-0019 §3 decided the hard half and four things were still open**, each of which can be got
wrong in a way that looks like it works. ADR-0056 settles them.

*How a schedule is spelled.* An **interval aligned to the unix epoch, with an optional UTC
offset** — `every 24h --at 3h` is 03:00Z. Integer arithmetic and nothing else, because that is the
property the derived occurrence id is built on: every node computes the same tick from the same
schedule with no message. Cron and local time are **refused**, and not for lack of a parser: a
timezone needs a database, and two nodes of one fleet that disagree about what "03:00" means
compute two different ticks and fire two occurrences, which is the failure the derived id exists
to prevent arriving through the front door. What that costs is stated rather than hidden —
"weekdays at 09:00 local" is not sayable, and the way to say it is still a trigger program.

*What it fires.* A whole **`RunSpec`**, so both tiers come free. Not a `SubmitRequest`, because a
submission becomes a spec by way of *the submitting node's* config — its model, its permission
mode, its baseline allowlist — and a peer that fires this may never have seen any of that. That is
not a new rule; it is what a submitted run already does. It is where a schedule differs from a
rule, which stores a request and builds at the firing because the machine that fires it is the
machine that stored it.

*Who fires it.* The home node while it is available, else the lowest-id available node — which is
`arbiter_for`'s rule, so it was **factored** rather than restated (`ClusterView::steward_of`). The
derived id is the net under a transiently doubled steward and never the mechanism.

*How one is removed.* A **tombstone**, because a gossiped set with removals needs one: a deleted
row comes straight back from the next peer, and the schedule then fires for ever on whichever node
was last to hear. `Revocation`'s shape and `Revocation`'s argument.

**Three things the build and the walk found, in that order.**

1. **The occurrence's record is not enough to say a tick fired, because the record is deleted.**
   The ADR's first draft said "the occurrence *is* the cursor" and argued it well — a cursor would
   be a mutable gossiped field describing something the fleet already records with an owner and a
   merge rule. Reading the retention pass killed it: a scheduled occurrence is machine-started, so
   `prune_spent_records` reclaims it about an hour after it finishes, which for a **daily**
   schedule is twenty-three hours before its tick ends. A nightly schedule would have fired
   hourly. The fix is a **node-local** high-water mark, which is `runs.rule`'s arrangement and
   needs no owner because no peer can state it.
2. **A schedule is not due for the tick it was born in.** Measured on a daemon, one command apart:
   `first tick in 36.9s` printed at the keyboard, and an occurrence in the log five seconds later.
   The tick containing the moment of creation started *before* it, so `every 15m` produced two
   occurrences eight minutes apart and the report was wrong about the one thing somebody had just
   asked. Compared against `created_at` — immutable and gossiped — so a peer that learns the
   schedule immediately declines the same occurrence the creator did.
3. **A refusal must not consume the tick, and the retry must be bounded by attempts.** Measured on
   two daemons: the home node killed, the successor correctly becoming steward and firing, and
   nobody able to host because the successor was still inside its fifteen-minute probation. The
   tick had been marked before the submission — the ordering that cannot double-fire — so a daily
   schedule would have lost the day for a refusal that lasted seconds. The mark moved after the
   placement, bounded by a dozen attempts. And **attempts, not elapsed time**: the first version
   of the bound was a window from the start of the tick, and the first test written against it
   fired nothing, because a daemon that starts mid-tick has made no attempts and should serve it.

**What the walk shows.** One daemon: a schedule created, the first tick firing at the boundary
rather than at creation, the occurrence's twelve-character prefix reading `01a08ad3ee20` — which
is `1789035540000` in hex, the tick itself — two ticks producing exactly two runs, and a removed
schedule firing nothing across the boundary after. Two daemons: the schedule gossiped to the peer
in seconds and reported there as `home nodeA · fired elsewhere`, exactly one node firing it,
killing the home making the survivor the steward, and a *learned* schedule surviving that node's
own restart, which is the whole reason it is written down rather than kept in gossip memory.

**Wire v29, schema v12.** The wire because a schedule is relayed and a v28 node would drop
tombstones; the schema because it is the first table here whose rows are gossiped — which is why
`home` is stored, why removal is an UPDATE, and why the tick mark sits outside the row's conflict
list.

## Session sixty-six (third half) — the cheap tier runs

**Three of phase 8's five items are done**, and the tier ADR-0019 was written for actually
executes: `[[tasks]]` in node config, an agent-agnostic supervisor, and `offload run --task`.
A node with no agent installed can now host work.

**The seam turned out to be smaller than the ADR feared, and in a different place.** The ADR
names the concrete `ClaudeCode` in `offload-node` and the `Agent` trait with two methods nothing
dispatches on, and reads as though the answer is to widen that trait. It is not. `drive` is ~370
lines of which almost all is generic run supervision — the workspace hold, the fence, the epoch,
`leftovers`, the `started` transition, the terminal write — and only a handful of lines are
agent-specific. So `start_run` gained **one** dispatch point and `drive_task` is a second body
beside `drive`, with no shared trait at all. `TaskHandle` is deliberately not behind one: an
agent handle carries a session, a transcript and a turn counter, and a trait wide enough for
both would be a type announcing that half its methods might not mean anything, which is the
`Option` mistake ADR-0019 §2 refuses, moved up a level. What the two genuinely share is how the
*supervisor* treats them, and that is expressed by both being driven from the same discipline
rather than by a common interface.

**The fencing is unchanged, and that was a decision rather than an oversight.** A task commits
nothing to a repository, which is the argument for relaxing it, and the argument is wrong: a
nominated program can send mail, restart a service or post to a webhook, and two of those at
once is the same harm double execution always was. So the epoch is read from the row immediately
before the spawn, the process group goes to `leftovers` before anything else can go wrong, and a
`started` transition that will not go stops the process.

**The walk found two defects the tests would not have, and both are report defects.** The first
run of a task printed `agent claude-code , model` — an `AgentStarted` line with two empty fields
where the claim should have been — and `offload cancel` answered *"its agent was stopped"* about
a shell script. Both sentences were written when an agent was the only thing a run could be, and
both became confident nonsense the moment that stopped being true. The general rule is now in
`docs/pitfalls/reports-and-cli.md`: **adding a kind of work means re-reading every sentence that
describes work**, and running one and reading the output finds them faster than grep does.

**The third was mine and it is the one worth remembering.** `offload run --task push`, on a node
nominating no such program, was *accepted* — then started and failed a millisecond later. With a
cluster the bid round refuses an unsatisfiable constraint; with no cluster there is nobody to
evaluate it against, so ADR-0014's promise has to be arranged on that path by hand. A run in the
log, a notification, and a person finding out at breakfast what they could have been told at the
keyboard.

**And an instrument lied for the fourth time in one session.** `pgrep -f slow.sh` matched its own
shell and made a correctly-cancelled task look like it had leaked its process group — reported
as a defect before it was checked. The `pkill -f` trap is recorded twice in `docs/DEMO.md`; the
read-only sibling is worse, because it does not kill the walk, it misinforms it.

## Session sixty-six (second half) — the `RunSpec` split, and phase 8 starts

**Phase 8's first item is built.** `RunSpec` is now shared fields plus
`Work::Agent(AgentWork)` beside `Work::Task(TaskWork)` — ADR-0019 §2, and everything else in the
phase waited on it. Each variant holds a named struct rather than the inline fields the ADR
spells: the same decision, with ergonomics the ADR did not need to specify, since the agent half
is eight fields that travel together everywhere below `offload-core` and `&AgentWork` is what
stops them becoming eight arguments.

**`Work::Task` is a type and nothing else, deliberately.** Nothing constructs one outside tests
— there is no `--task` and no `[[tasks]]` until later items — and every path that would have to
execute one refuses with a reason rather than defaulting: `NoBid::UnsupportedWork` at placement,
`SubmitError::UnsupportedWork` at the three doors that spawn, prepare or resume. A task that
could be submitted and placed but not run would be worse than one that cannot be submitted, and
those three refusal sites are also the map for the next item.

**The part that was nearly missed is a storage one.** A whole `Run` is one JSON blob in
`runs.run_json`, so changing the shape of `RunSpec` changes what a node reads back from *its own
disk* after an upgrade — with no migration to hang it on, because the schema never moved, and
with `MIN_VERSION` protecting only the wire. Without a compatibility deserializer, upgrading a
daemon turns every stored run and rule into `StoreError::Decode`. It has one, and it was checked
the way this project checks things: a pre-split binary wrote a real run row, the new binary read
it back intact, and both shapes now decode from one database. The hand-written fixture beside it
had the serde spellings right, which was luck — the walk is what made it evidence.

**One process note worth carrying.** A stray `git checkout` on an uncommitted file threw away an
afternoon's work on `offload-core/src/run.rs` and it had to be rebuilt from scratch. The lesson
is not "be careful"; it is that a large mechanical edit should be committed the moment it
compiles, before the tests are written, because a commit is the only thing that makes the work
recoverable.

## Session sixty-six — the cross-platform failure was two faults, and the instrument was the third

**Resolved.** Fifteen theories had died across two sessions and the record had settled on a
contradiction — *quinn's traffic fails four times in five where raw UDP on the same 4-tuple never
fails at all* — with instructions to derive the next theory from it. The contradiction was not
real. It rested on `udp_probe`, which called `UdpSocketState::send`; that call returns `Ok(())`
for **every** error but `WouldBlock`, so the probe had been counting calls rather than datagrams.
Switched to `try_send`, the same path measures 0 of 40 where it had measured 10 of 10. The
companion misreading was in the same family: "13 `sendmsg` errors in 2.5 hours" is a
`log_sendmsg_error` rate limit of one line a minute, not a count, and the socket had been failing
every send. **An instrument that cannot report failure will report success for ever, and two
sessions of reasoning were built on one.**

**The product defect was found by reading, not by walking, and reproduced in five seconds with no
Mac.** `QuicTransport::accept` ran every step of an inbound handshake inline in its own loop —
including `accept_stream`, an unbounded `accept_bi()` on a connection whose peer may open no
stream. Since quinn does not drive a handshake until the application takes the `Incoming`, one
dialer that completed TLS and then went quiet parked the loop, and every later dialer got neither
a refusal nor a close but silence. That is exactly the signature the record had been chasing —
kernel counting no drops, dialer's `quinn_proto` trace showing an Initial and PTO retransmits and
nothing back, acceptor logging nothing inbound — because the wedge sits above the syscall layer
and below anything either end reports. And it is self-sustaining, which is why the pair "meshed
and died eight times and then could no longer re-connect". Each handshake now runs on its own
task. A latent second fault on the same path went with it: `authenticated_peer` reported
`Closed`, which the serve loop reads as *the endpoint is gone*.

**The other half was never ours.** With the accept loop fixed, the pair meshes in both directions
and then dies within seconds on a single `EHOSTUNREACH` from the Mac's `sendmsg`. macOS 26
refuses to let an **ad-hoc-signed** binary send to the LAN and reports it as no-route-to-host;
Apple-signed `python3` sends happily from the same machine in the same millisecond. Interleaved
in one loop: python 30 of 30, an ad-hoc C sender 0 of 30, ad-hoc Rust 0 of 30. Socket options,
syscall family, ECN, socket lifetime and the message header were each eliminated with the failing
call interleaved as a positive control — which mattered, because ten socket-option variants first
"passed" 40/40 only because the condition had lifted between runs.

**Two of this session's own theories died the same way its predecessors' had**, and both are
recorded because the shape repeats. "A plain socket succeeds where quinn-udp's fails, so it is
the message header" was a real measurement — 354 disagreements out of 360, all one way — and
still wrong: `sendto` on the *same fd* immediately after each failure fails too. And "a fresh
code identity gets a grace period" predicted an early-successes-then-turn pattern and got 40 of
40 denied from the first datagram. **Two senders disagreeing is evidence about the senders, not
yet about what lies between them.**

**The environmental theory the record had left open was finally controlled, and it is dead.** The
laptop was dual-homed on the subnet the whole time and the file blamed that for `EHOSTUNREACH`.
Wi-Fi off, single-homed: it fails identically.

## Session sixty-five — the platform axis was untested, and the thesis was too narrow

Two halves, and they have nothing to do with each other except the day.

### Linux talks to Linux; Linux does not talk to macOS

**Nothing in this project had ever run on two operating systems.** Every multi-node test in its
history was two or three daemons on one Linux box with distinct ports, so the platform axis was
untested *by construction* — a test matrix with one row does not know which of its columns is
load-bearing. The first time a Mac mini joined, the mesh did not come up, and it is a **product**
defect rather than an environment one.

**The hunt is recorded in `docs/DEMO.md`, not here, because it is unresolved** — eleven theories
are recorded dead there, each one a thing not to re-test, and the rule for the next session is to
read that list before forming a theory. Two changes earned their place in the tree:
`mtu_discovery_config(None)`, which took macOS's `sendmsg` errors from many to zero, and log lines
on the inbound path, because **an accept path with no log line cannot be told from one that is
never entered**. GSO was reverted — an unproven behaviour change is worse than none.

**Where it stopped, stated as measured rather than theorised.** The handshake succeeds about **1
attempt in 5** and the liveness probe essentially never, while *every* raw-UDP test on the same
hosts, ports, sizes, bursts and ECN marking is **100%** with no kernel-counted drops — and
`examples/udp_probe.rs` clears `quinn-udp`'s own send path at 10 of 10. So quinn's traffic fails
four times in five where raw UDP on the same tuple never fails at all, nothing below `quinn-proto`
survives as an explanation, and that contradiction is the whole of what is known.

**The expensive lesson, paid three times in one day: one reading of an intermittent failure is not
a measurement.** Three claims were made and retracted, each an honest single observation that
reversed when run five times per arm — the last being the 4-tuple collision, which looked causal
on one observation each way and measured 1 success in 5 against 0 in 4, which is nothing. The rule
was already written in `docs/pitfalls/testing-and-sweeps.md` before the day began. Its sharper
cousin, which cost a retired hypothesis: **a control that does not change the thing it is
controlling for is worse than no control.** Moving the Mac's port was meant to break the tuple
symmetry and could not, because both daemons dial from their own listening port.

*(A trailing paragraph in `docs/DEMO.md` still asserted the retracted 4-tuple claim after the
correction had rewritten the section above it — found and fixed while writing this. A retraction
that lands in one place and not the other leaves the dead theory alive where somebody will read
it.)*

**Then the hunt got one more turn, and it produced the first lead that fits every instrument.**
The contradiction it stopped at was that quinn's traffic fails four times in five where raw UDP on
the same hosts, ports, sizes and ECN never fails, with the kernel counting no drops and
`quinn_proto=trace` silent — so every surviving theory had to be wrong about one of those three
readings. There is exactly one place where all three read clean at once: `quinn-udp` turns on
**UDP_GRO**, and the resulting split of a coalesced buffer happens in **userspace**, above the
kernel counters and below `quinn-proto`, and a python `sendto` test never sets the option.
`udp_probe` had been printing `gro=64` since it was written and only `gso` was ever recorded.

**Measured, and the honest half is the more useful one.** `examples/gro_probe.rs` says no
same-host path coalesces — loopback and a container-over-veth path both 16 of 16 datagrams with
**0** coalesced buffers — so since every multi-node test here has been one Linux box, quinn's
split loop **has never run in this repository**. That is the platform axis's own shape again:
untested by construction rather than merely unexamined. But it did **not** reproduce the failure,
so what exists is a coverage gap plus a hypothesis, and settling it needs a second *physical* host
where a NIC driver's GRO is in play. The next experiment is two commands and is written down, and
its control — `ethtool -K enp58s0u1 gro off` — actually moves the variable, unlike the port move
that once "disproved" the 4-tuple theory.

**Two theories died, and the second one was mine.** The trace was *not* compiled out of the
release binary (no `release_max_level_*` anywhere, and quinn's `log` feature adds a sink rather
than removing one), so the laptop's silence is real evidence rather than a sixth filter artifact.
And `gro_probe`'s own first run reported **0 of 16 datagrams over a path carrying all 16** — the
reported symptom exactly, on one machine, with no Mac — because `UdpSocketState::new` makes the
socket non-blocking as its first act, which silently voids the `set_read_timeout` the probe waited
on. It read once, got `WouldBlock` before the sender started, and exited having measured nothing.
For about ten minutes it looked like the find of the session. **The control that caught it was the
same probe over loopback, a path already known to work, which also reported 0** — and the
comparison had quietly drifted in a second way too, the quinn receiver and the python one never
once running on the same port. Fifth false zero in this one investigation, and the first from an
instrument built to end it: when a new tool and a real bug have the same signature, assume the
tool.

### An agent run is the most expensive thing the fleet can schedule

**No code changed, and the thesis got wider.** For most of this project's history the only kind of
work was an agent run, and `docs/ARCHITECTURE.md` opened by explaining why an agent run is an
unusually good migration candidate. That is still true and is still first, but it was being read
as the whole claim. It is not: **Offload schedules work of graduated cost.** Three tiers — a
**trigger** notices for free, a **task** evaluates for the price of a process, an **agent run**
costs tokens and a worktree and a transcript — and the escalation between them is the point, since
*a fleet that can only run agents costs money while it sleeps.* Stated so a future feature can be
tested against it: **keep the model out of the loop until it is the only thing that can help.**

**The reason it is architecture and not a roadmap item is that it was derived twice, from opposite
ends.** From first principles here, and from `docs/use-cases/operations-fleet.md`'s forty shops,
where the answer to *"does this need an agent at all?"* came out **"mostly no — it is a
comparison"** and the shape came out *"arithmetic that occasionally escalates"*. One derivation is
a preference; two from opposite ends is a property of the problem.

**So the cheap middle got scheduled: phase 8.** ADR-0019 has been accepted since 2026-08-24 and
sat in the phase-7 parking lot as a nice-to-have. It is not one — it is the tier the economics rest
on — and on the owner's call it is now a numbered phase with five items and a demo (a node with
**no agent installed** hosts a task, a trigger fires it, it reports through a sink, it spends no
tokens; and when it exits non-zero a rule submits an agent run that the fleet places on a machine
that *does* have an agent). It is numbered **8** rather than inserted, because six accepted ADRs
already say "phase 7" about it and a settled decision is not edited for bookkeeping. `phase 7` is
now documented as what it always was: a parking lot, not a position in time.

**What today's code makes impossible, stated where it will be found.** `RunSpec` is agent-shaped
throughout — `workspace` is a `WorkspaceSpec` and not an `Option` (`offload-core/src/run.rs:116`),
and its `repo` is a bare `String` (`run.rs:104`) — so **there is no run of any kind that does not
name a repository**, and every cheap poller would have to arrive dressed as an agent run against a
repo it never opens. The fix is ADR-0019 §2's tagged `Work` enum and **not** an `Option`, and the
architecture doc now says why at the moment somebody would reach for the cheap version: an
`Option` read as "not an agent run" is inference wearing a type, and it leaves the invalid
combinations expressible for something downstream to guess at.

**Three smaller things this shook out.**

- **The other direction of control needs nothing new.** The fleet drives the agent; an agent that
  *dispatches* to the fleet is ADR-0011's `Resource` projecting an `offload` MCP server, which is
  also how the phase-7 run-DAG item stops being a subsystem. Two rules bind there and are easy to
  skip because the caller looks trusted: an agent holding a submit tool **is a submitter** and
  every submitter rule binds it, and **the submitting conversation is not the audience** — a
  dispatched run outlives the turn that dispatched it, which is what ADR-0026's addressing by
  *service* already answers and nothing has exercised for agent-submitted work.
- **A clock is a program, except when it is a schedule.** The two mechanisms made opposite calls
  deliberately and confusing them is the likeliest way to design the wrong thing here: a trigger
  refuses the interval field (ADR-0020 §1) because the cadence belongs to the owner's program,
  while a schedule has ticks and pays for them with a `RunId` derived from `(schedule, tick)`
  (ADR-0019 §3). Worth carrying verbatim: *a duplicate record merges, and a duplicate agent does
  not.*
- **The reframing invented vocabulary, and the glossary caught it.** "Task" is now a tier with a
  prominent name, and `docs/GLOSSARY.md`'s **Run** entry had reserved that exact word *against*
  loose use. Both now point at each other, and **Task** is flagged in both vocabulary lists as
  the one word with no type behind it yet. A word that means two things is the same mistake as a
  field that does, one layer earlier.

**And a phase move fans out further than it looks.** `docs/HANDOFF.md`, `docs/ARCHITECTURE.md` and
`docs/use-cases/operations-fleet.md` each carried a stale "phase 7" about this item, found by
grepping for it rather than by remembering where it was written; the ADRs were deliberately left
saying it. One claim in the draft was simply false and was cut — that the item had been *filed for
a year*, when ADR-0019 is sixteen days old.

## Session sixty-four — a rescue that could not tell what it had rescued

ADR-0053's last residual, read as a description of what is true now rather than of what was
decided: *nothing tells the fleet, and nothing ever removes either.* Both halves turned out to be
about something upstream of what the sentence named.

**The rescue destroyed the evidence for its own necessity.** `supersede` renames a superseded
checkout instead of removing it, because it may hold the only copy of a mid-turn edit — and three
lines later deletes the copy's `.git` file, correctly, since it points at a registration
`worktree prune` is about to drop. Measured: `git status` in a rescued copy answers `fatal: not a
git repository`. So `holds_uncommitted` — the exact question `reclaim_occurrence` uses to tell
litter from treasure — was available *before* the rename and never again. Renaming
unconditionally did not defer that decision, it made it, in the direction of keeping everything;
and every kept copy then looked identical to the one that mattered. Three legs of one run left
**three full copies of the tree** and three refs, and nothing in the product removed one.

So `supersede` asks first (ADR-0055), and a checkout holding nothing goes through `remove` —
whose own doc comment already said the commits are on the run branch. `None` still keeps the copy:
unknown is not nothing, at the one door here whose wrong answer deletes somebody's only file.

**And "tell the fleet" was the wrong target.** A rescue is a fact about one disk with no second
opinion to merge against — `AuditEvent::Reclaimed`'s whole argument, and `RunProgress` could not
have carried it regardless, since its one writing leg is the leg that *kept* the run (session
seventeen). Both rescues are per-node audit rows now, with a third `Reclamation::Redundant` door
for the removal, and neither row needs a path because both names are derived from the run id.

**Walked, one daemon, both arms on a byte-identical state directory.** The staging is cheaper than
the handoff would have guessed: `Adoption::Superseded` does not need a two-daemon ping-pong, only
a **deleted turn marker** on a `pending` run, which is the *in flight across an upgrade* case the
marker exists for. The control kept `<run>.superseded` for ever and said *"the checkout here held
turn unknown and uncommitted work, so it is kept at …"* — **false about a clean checkout**, which
is what an unconditional rescue leaves a report no way to avoid saying. All six files in it were
checked against the run branch; all six were on it. The fixed build removed it and the run resumed
turn 5 → 8 and completed with its five earlier commits intact, identically to the control.

**Four things worth carrying.**

1. **Ask what a decision will still be answerable with afterwards**, not only whether it is right
   now. The `.git` deletion was in the code, in the ADR and in the doc comment, and none of the
   three noticed that it closed a question the line above it depended on staying open.
2. **A hand-written list of every variant goes stale in silence.** `Rescued` was added with no
   line in `every_kind_round_trips`, which is not a failing test but a variant nothing has ever
   encoded — and encoding is the only thing between the audit log and being unreadable. The guard
   counts `kind_name`'s arms from the enum's own source now, `no_clock.rs`'s trick in a smaller
   place, and it was verified by inventing a variant and watching it fail. What it does *not*
   reach — a new arm of `Reclamation`, `Rescue` or `Attempt` — is said in the test rather than
   implied.
3. **A residual can be stale about its own subject, not just its date.** Session sixty-one's entry
   said to read a residual as a description of what is true now; this one adds that the *thing it
   asks for* can be wrong. "Nothing tells the fleet" asked for gossip that would have been
   correctly dropped on the only node that had the fact.
4. **`cd X && nohup daemon … & echo $! > pidfile` records the subshell**, not the daemon, so the
   `kill -TERM` that follows signals nothing and the `until ! pgrep` after it waits for ever. The
   documented `pkill -f` trap in a new spelling — it is the *compound* that gets backgrounded.

918 tests, clippy clean, wire and schema unchanged (the audit log is JSON in one column).

## Session sixty-three — the flake was neither the machine nor the lock

Item 1, and it was item 1 because the previous session finally counted: the ADR-0052 race test
failed about **1 run in 5 at load 6.09** and **0 in 14 idle**, after three sessions had each
written it off as "the machine" from a single red run apiece. Two readings fitted and they needed
opposite fixes — the staging losing its own timing, or the lock not holding when the scheduler
stretches the window.

**It was a third thing, and instrumenting found it in one reproduction.** The rebuild task now
carries out which side of the guard it landed on, and the failure reads `rebuild adopted, teardown
Removed`. So the **rebuild** won `hold(run)`, `adopt` found the checkout present and current, it
returned and released the guard — and the teardown then took the lock and removed it. The two
never overlapped. The lock did exactly what ADR-0052 built it to do; the *test* then asserted
something the design does not promise.

What made that assertion look reasonable is that in the product it is true. `drive` holds the
guard until the run is in `live` with an agent handle, and every teardown door asks about that
handle — `cleanup` reloads the run and refuses a non-terminal one, `reclaim_departed_checkouts`
asks `reclaimable` twice. The staging has no handle: its teardown is `remove` with nothing in
front of it, so a rebuild that wins the lock and releases into that is asking to have its checkout
deleted, and the deletion is correct.

The check moved **inside** the rebuild task, where the hold is still held, which is the window the
guarantee is about. **0 failures in 60 runs at load up to 10.7**, against 2 in 10 before. It keeps
its teeth: without serialisation the removal overlaps the check rather than following it, which is
the 10-of-10-within-2ms case it was written for. It also gained an assertion that the teardown
removed *something*, since one that found nothing would have raced past what it was meant to tear
down. No product code changed.

Two rules out of it. **A race test's assertion has to live inside the window whose invariant it is
testing**, not after both racers have joined. And **when something is flaky, spend the one line
that makes the next failure say which case it was** — three sessions of theorising against one
reproduction that answered outright, which is the move ADR-0051 already made for `register`.

Residual: one unexplained failure in about eighty post-fix runs, whose message was not captured.
If it returns it now names its own case.

## Session sixty-two — "it is still in the reflog" is not a place you put something

ADR-0053's remaining residual. When a restore's `reset --hard` moves past commits this checkout
had and the bundle does not, the epoch has decided which leg is the run and the reset is right —
what was wrong was what became of the commits. The residual said they were "named in the log and
kept by the reflog", and both halves are weaker than they sound: an unreachable commit's reflog
entry expires (30 days by default, sooner if an automatic `gc` runs), and a hash in a log line
needs somebody to know to look and to still have the log.

They get a **ref** now — `refs/offload/left-behind/<run>/<short>` — which is the answer `supersede`
already gives a checkout it moves aside rather than deletes, for the reason written down beside it:
work nobody was told about is work nobody finds. The namespace is this project's own and safe from
the only thing that deletes refs here, since `ensure_mirror` is deliberately not a `--mirror` and
`fetch --prune` therefore touches only `refs/remotes/origin/*`. Writing it is best effort — the
run continuing matters more than the bookkeeping — and `LeftBehind { commit, kept_as }` carries
which happened, because "only the reflog has it" is worth stating rather than implying.

Tested by diverging two legs on one base and asserting the abandoned work is reachable **by name**
after the reset — `git show <ref>` still contains it — and confirmed red with the `update-ref`
removed. Still open, and now the whole of that residual: nothing tells the *fleet*. The ref is on
one disk, like the superseded checkout beside it, and nothing ever removes either.

**A measurement that changes something else.** `a_rebuild_never_adopts_a_checkout_a_teardown_is_
removing` has now failed in three sessions running, and each time it was written off as the machine
being busy — from one failure each time. It is **2 in 10 at load 6.09** and **0 in 14 idle**. That
rate is too high to keep waving through: the panic is `at delay=0ms the rebuild handed back a
checkout that is not there`, and two readings fit it — the staging losing its own timing, or the
guarantee not holding when the scheduler stretches the window. The second would be a defect in the
lock ADR-0052 exists for. It is the first pick-up item now, and the lesson is the cheaper half:
**a flake dismissed from one observation is a claim with no measurement behind it**, which is
exactly what this repository says not to do.

## Session sixty-one — ADR-0035's residual was half wrong about itself

Item 6, and the plan was to read it and leave it alone. ADR-0035 §4 rejects expiring a drained
node's pending questions — *"they are not unanswerable, they are unanswered"* — and the residual
says shortening the patience to the drain's remaining time is the weaker form of the same thing.
That argument holds and nothing here touches it.

**What the residual did not notice is that its own case had already broken §2.**
`DrainStep::Blocked` carried `within_ms` from `ask.left` — the question's own patience — and knew
nothing about the drain's deadline. So in exactly the situation the residual describes, a drain
printed:

```
  waiting for 1 run(s) to reach a turn boundary — up to 15.0s
  run 01a07310d9b4 is stopped waiting for an answer: Bash — rm -rf /tmp/nothing
    it reaches no turn boundary until that is decided, or 4m52s from now
```

The right number and the wrong one two lines apart, with the wrong one attached to the sentence
somebody acts on. Walked on one daemon with `drain_deadline_secs = 15`, a fake agent piping a real
hook into `offloadd ask-hook`, and a two-line shell sink so the question is actually put. The drain
then gave up and left the run mid-turn, four and a half minutes before the deadline it had just
quoted.

The fix is **more reporting, not an expiry** — §2 finished rather than §4 reopened. `Blocked`
carries `drain_ends_in_ms` beside `within_ms`, and the CLI names whichever is nearer *and the
consequence*, because the two deadlines end the wait differently: the question expiring decides it
and the run is handed over normally, while the drain expiring first decides nothing and leaves the
run mid-turn with the question still open and still answerable. No new ADR — ADR-0035's residual
is amended in place, standing for the mechanism and closed for the report.

The choice lives in `blocked_horizon`, a pure function over two `Millis` rather than three
`println!`s in a match arm, because it is the only line in the product that picks between two
deadlines and picking wrong is silent. Four branches, including *equal is not sooner*: at the same
instant the question decides it, which is the better outcome, so it is not the one to warn about.

**Worth carrying: a residual is a claim with a date on it, and the claim can be stale in a
direction nobody expected.** This one was written to say "we chose not to fix the mechanism", was
right about that, and was silently also saying "and the reporting is fine", which it was not. Read
a residual as a description of what is *true now*, not only of what was decided.

## Session sixty — a departing node no longer leaves with the only copy (ADR-0054)

The biggest actionable item on the list, open since ADR-0043 and sharpened by two measurements
since: replication is never forced on the way out.

`decide_recovery` asks `fleet_could_start` → `Checkpoint::is_durable` to choose between handing a
failed run to the fleet and stranding it — *"this node is leaving and its conversation exists
nowhere else"*. That sentence is honest and was decided by a fact `depart` had never tried to
improve. `Supervisor::replicate` has one caller, no retry, no background pass, so a capture taken
while no peer was reachable stays `here only` for ever; a failed run has no next capture, and
neither does a `Pending` one released by `offload checkpoint`. So *"there was no peer then"* and
*"there is a peer now"* were routinely both true at the moment a node left.

**Walked on two daemons, same staging both arms.** Alpha alone, run fails at turn 4 with its only
checkpoint here, **then** bravo starts, then `offload drain`. Before: `nothing to hand over`, run
stays `failed`/`here only` on the machine that is leaving. After: `copied 1 checkpoint(s) nobody
else had to a peer`, `1 failed run(s) given back to the fleet`, and the run is **running on bravo
at turn 5** with `replicated` beside it. The work went from dying with the laptop to continuing on
the desktop, which is the sentence on the front page of this repository.

ADR-0054 has the shape and the rejected alternatives. Three parts of it were decisions:
`push_last_copies` runs **before** `hand_back_failed_runs`, since that is the pass whose input it
changes; it covers **every** run with a `resumable_checkpoint` rather than only failed ones,
because a `Pending` run strands one door over; and every failure is **soft**, so the pass can
improve an outcome and never worsen one. It is bounded as a whole at 30 seconds, and `LastCopies`
reports four numbers because `refused` (the fleet would not) and `unattempted` (we ran out of time)
need opposite fixes.

**The bug it shipped with is worth more than the feature.** The first version called
`list_runs(false)` — *skip terminal rows* — and a `Failed` run is terminal, so the pass could not
see the runs it exists for. That is an entry already in
`docs/pitfalls/checkpoints-blobs-and-workspaces.md` (*"`is_terminal` is the wrong question in a
report for the same reason"*), re-derived three sessions after it was written down. Its own test
caught it on the first run, which is the actual lesson: a rule in the pitfalls file does not stop
you making the mistake; a test that stages the real case does.

Also this session, at the user's request: **CLAUDE.md now asks for a status at the end of every
completed task** — what changed, the phase, the ADR (or *none*, said explicitly), the test state,
and the one thing to pick up next. The handoff and the roadmap stay the authorities; the status is
the pointer into them.

## Session fifty-nine — closing ADR-0053's residual, and why the staging the handoff proposed does not work

The handoff called this the most concrete unwalked thing on the list: ADR-0053's `AlreadyHere`
arm — the branch already containing the checkpoint's commits — was covered by a unit test and not
by a walk, and the proposed staging was "a daemon killed after a checkpoint and several more
turns, then restarted".

**That staging does not reach `restore`, and the reason is worth keeping.** `adopt` reads the
worktree's **turn marker**, which is written at the last *capture*, not at the last turn. Kill the
daemon at turn 8 with `every_turns = 5` and the marker reads 5, the checkpoint reads 5,
`at >= through` holds, and the answer is `Adoption::Current` — **`adopted in place`**, with
`restore` never called. That is correct: the checkout on disk really is current. It just means a
same-node resume is the wrong place to look.

What reaches it is the case ADR-0053's own context names: **the branch outliving the worktree**.
Kill timed between captures so the branch is three commits past the checkpoint, then remove the
checkout. Measured against a build with the `--is-ancestor` check forced false, on the same state
directory: the guarded build says `the commits here already held this checkpoint; patch reapplied`
and keeps `work-L1-1..8`; the unguarded one says `rebuilt from bundle and patch; this branch was at
cef5d49, which this checkpoint does not contain` and keeps **five** — turns 6, 7 and 8 gone. So the
guard is load-bearing on real daemons and not only in a fixture, and ADR-0053's decision 4 earned
itself too: even the broken build put the abandoned tip's hash on the screen.

**Two things the walk cost before it worked**, both now in `docs/DEMO.md`. `offload ps` trails the
agent by seconds, so a pass that read `turn 7` and killed immediately caught the run at turn 10 —
on a capture boundary, where the branch equals the checkpoint and the walk proves nothing; time the
kill off a file the fake agent has just committed. And `rm -rf` on a worktree is not how the
product removes one: git keeps the registration and the next `prepare` dies with `'offload/run-…'
is already used by worktree at '…'`, which is a pitfall already in the file, met from the walk's
side. `supersede` and `remove` both run `git worktree prune`.

No code changed. The ADR's residual is struck through rather than deleted, which is what
`CONTRIBUTING.md` asks for: the commit that closes a gap is not the one that remembers to remove
the note.

## Session fifty-eight — the sweep the last handoff asked for, and an assertion that could not fail

Session fifty-seven's pick-up list named its own cheapest next item: one deliberate pass over the
checkpoint tests asking *what is set to `None` here, and what would `Some` do*. Two sessions had
now been caught by a fixture with the interesting half switched off, so the question was worth
asking systematically rather than once more by accident.

**The hit is ADR-0016's own test.**
`a_checkpoint_is_copied_off_this_machine_and_the_run_records_where` compares what replication was
asked to copy against `checkpoint.blobs()`, with the message *"every blob, not some of them"* —
over a `checkpoint_at` fixture whose bundle and patch are `None`. It read `[transcript] ==
[transcript]`. Measured: making `Supervisor::replicate` send `vec![checkpoint.transcript]` passed
`cargo test --workspace` at **909 tests, exit 0**. What would have been gone is the invariant
`mesh.rs` states in a comment and nothing enforced — *a peer holding two of three blobs cannot
materialise the run, so recording it as a replica would make `is_durable` a lie*. The fixture
carries all three now and the same revert fails it.

**The second is smaller and is the line these last three sessions have been reading.**
`Capture::summary` — `"1442 byte bundle + 146 byte patch (1 untracked file(s))"` — had no test in
either direction; nothing in the workspace asserted on `byte bundle`. Four shapes and its
agreement with `has_work` are pinned now.

**No behaviour changed.** Both paths were already correct; what was missing was anything that
would notice if they stopped being. That is the honest description of the session, and it is why
there is no ADR.

**Half the value was in what the sweep cleared**, and it is worth writing down because a sweep
that only reports hits is not a sweep. `Checkpoint::blobs()` is exercised with all three blobs in
`bid.rs`, so the root was solid and it was a consumer that was unchecked. The blob collector is
genuinely covered — reverting `referenced_blobs` to the transcript alone fails `gc.rs` — which was
the sweep's leading hypothesis and was **wrong**. And the `bundle: None` in the `mesh.rs`,
`explain.rs`, `view.rs` and `run.rs` fixtures is irrelevant to what those tests assert. One right,
one wrong, out of two inferences from reading: the revert is the only thing that told them apart,
and it costs two minutes.

## Session fifty-seven — the bundle a migration dropped, found in the log line of a passing walk

Session fifty-six's own pick-up list said its walk had not covered a **bundle**: every checkpoint
in it carried only a transcript. Covering that found a second defect, and a worse one.

`Supervisor::restore_workspace` decided whether to apply a checkpoint's bundle from
`WorkspaceManager::has_branch` — apply it only if the run branch is *absent* from this node's
mirror. The hazard it was guarding is real: `restore` ends in `git reset --hard`, so applying a
bundle over a branch that has moved *past* the capture walks a live run backwards, and a branch
ahead of the newest checkpoint is the ordinary state of a running agent. But `has_branch` answers
"has this node ever run a leg of this run", and nothing keeps that branch current — so a branch
merely **older** than the checkpoint suppressed the bundle just as thoroughly.

**Walked on two daemons, three legs, with nothing failing anywhere.** Alpha runs four turns and
commits four files; bravo resumes — `rebuilt from bundle and patch`, alpha's four arrive, four
more committed; alpha resumes again and reports **`re-checked out, patch reapplied`** with four of
the eight files. The bundle was on alpha, named by the checkpoint, and never opened. An earlier
six-turn pass had `git rev-list` on the two branches intersecting in exactly one hash — the base
commit. The other way in needs no migration: `prepare` creates the branch at base before `restore`
runs, so a leg that dies in that window poisons the node for every later resume.

ADR-0053: `restore_workspace` stops deciding, and `restore` asks git —
`merge-base --is-ancestor refs/offload/restored HEAD` — applying the bundle unless this checkout
already contains its tip. `BundleOutcome` carries the answer back as a reason rather than a bool,
because the run's log has to tell *left alone on purpose* from *nothing to apply*, and a reset that
moves past commits this checkout had names the hash it leaves behind. `has_branch` keeps its other
job, choosing the start ref.

**How it was found is the transferable part, and it was not by reading code.** It fell out of the
A/B for session fifty-six's fix: the control binary failed a resume midway, which left the branch
at base, and the fixed binary's next resume then said `re-checked out, patch reapplied` about a
checkpoint that plainly had a bundle. One word in a log line nobody had a reason to distrust.
Two rules came out of it — **read the account a resume gives even when it succeeds**, and **when a
fixture sets a field to `None`, ask what `Some` would have done**: the run-comes-back test stages
exactly this scenario and sets `bundle: None`, so it walked the patch and never the commits.

Also checked and **not** a finding: session fifty-six's handoff claimed auto-resume across nodes
was affected by the same hole. It is not. `recover()` marks an interrupted run failed with
attendance `None`, and `retryable` escalates `AttendanceUnknown` — so the tick never resumes it,
and a person types `offload resume`, which is the door that was already walked. The claim is
corrected rather than carried.

## Session fifty-six — the migration this project is named for had never been walked

Item 1 on the pick-up list: *a peer resuming a run whose blobs it lacks fails instead of fetching
them* — evidence, no mechanism, and the previous session had ruled the obvious explanation out.
It was the obvious explanation. `Supervisor::prepare_start` calls `ensure_blobs`, and the handoff
said it "is the only place `Start::Resume` is built"; `Supervisor::resume` builds one too, forty
lines further down, and calls `launch` directly. One `grep -n "Start::Resume"`. That is both doors
onto the only path that restarts an agent from a checkpoint, and only one of them fetched.

**What it cost, walked twice against the pre-fix binary on identical state.** Alpha alone runs to
turn 5, `offload checkpoint` leaves the run `pending` and `here only`; bravo starts, both nodes
`alive`, bravo holds the row and zero blob files. `offload resume` on bravo answers **`resumed
01a0708be323`** — an `Ok` to the operator — and 12ms later the log says `workspace ready`, then
`run failed … state store: blob bf5035b4… is not stored here`. **No fetch line anywhere**, because
nothing asked. And it is not a failure to start: it reopens the `Failed` run into `Assigned`,
spends an epoch, builds a worktree and fails the run again, so retrying buys a fresh worthless leg
each time. With the fix, the same command on the same state directory logs `fetching checkpoint
blob blob=bf5035b4` and the run goes 5 → 12 with SAFE reading `replicated`; with the peer killed
first it refuses at the door — `no peer could supply the blob: no route to 0705e430` — and leaves
the run `pending`, byte for byte as it was.

**The fix is a type rather than a call.** `restorable::Restorable` wraps the `Checkpoint` that
`Start::Resume` carries; its only constructor cannot be satisfied without an answer for every
blob, and `ensure_blobs` is its only caller — so the compiler asks, the way ADR-0052 made the
compiler ask about a checkout. In `resume` it sits **ahead of the claim**, unlike `start_run`'s:
everything it can go wrong with is a refusal this run would meet whoever asked, so no epoch is
spent and nothing has to be given back by hand. `ensure_blobs` now asks the store twice, the
second answer deciding — a `fetch` that reports success without leaving the bytes here is
otherwise found out by `restore_workspace`, with a worktree half built. No ADR: this restores
what ADR-0016 already claims.

**The larger finding is about the walks, not the code.** Migration to a node that has not already
been replicated to is the thing on the front page of this repository, and no walk had ever done
it — every migration staging started both daemons early enough for the checkpoints to replicate,
so `has_blob` was always true and `ensure_blobs` always a no-op. The staging that finds it is one
line different: **start bravo after the checkpoint, not before.**

**And the reason 900 tests did not have it either.** `Store::open_memory` put the connection in
memory and the blobs in one shared on-disk directory, so `has_blob` was not a question about *this*
node and "a node without this checkpoint's blobs" could not be staged at all. The tell was in the
suite: three tests resumed from a checkpoint naming a hash of nothing, stored nowhere, and passed.
The root is per-store now, those three say `checkpoint_here`, and two new tests say the opposite
on purpose. Ask what the harness makes unsayable — that is where the defect is.

## Session fifty-five — the measurement ADR-0043 asked for, and what it changed about the question

Item 7 on the pick-up list, and it was framed as a measurement before it was a change: *before
building forced replication on the way out, measure how often a checkpoint is not durable by the
time a node leaves*. The residual guessed the answer would be "only when there was never a peer".

**With a peer, durability is never a race.** Two daemons over loopback, `every_turns = 1`, three
runs: 1.3–6.7 ms for a ~1 KB checkpoint, 6.4 ms at ~1 MB, 47.3 ms at 11.5 MB — **30 of 30
replicated**, every one inside a turn gap of seconds, and the peer's blob store ended
byte-identical to the holder's (32 blobs, 97,613,840 bytes on both). Linear, about 244 MB/s, and
that last number is loopback's rather than a real link's, which is the one thing here worth
re-measuring on wifi.

**So the timing answer is "never", and the state answer is worse than the residual assumed.**
`Supervisor::replicate` has exactly one production caller — `checkpoint` — with no retry and no
background pass. A checkpoint taken while no peer was reachable stays `here only` *for ever*,
not until a peer turns up: four checkpoints taken alone, then a peer started and both nodes
confirming each other `alive`, and all four still `here only` — only the **next** checkpoint
replicated. For a run that has *failed* there is no next checkpoint. So the residual's "only when
there was never a peer" should read **"whenever no peer was reachable at the moment of the last
checkpoint"**, and a departing node is the last chance that copy will ever get. Whether to build
it is still open; what is settled is that the thing to weigh is not the size of the bytes.

The failure it leads to was walked end to end: alpha alone, run fails, `offload drain` →
*left for a person: this node is leaving and its conversation exists nowhere else, so nobody could
continue it*. Honest, and correct.

**A defect fell out of it.** Alpha escalated that failure, the run later moved to bravo and ran
there, and alpha's `offload explain` went on printing, in one output: *its holder is answering and
its lease is current*, *checkpoint turn 16, copied to fedora*, and *nobody could continue it*.
`recovery_note` matched on `state.decided` and printed the remembered escalation unconditionally —
while the branch beside it, the one with no decision to print, has always asked `decide_recovery`,
whose first act is to return `None` for a run that is not `Failed`. The same question, simply not
asked on the branch that had a memory. Gated on the run still being `Failed`; the memory stays,
because an escalation has to be answerable for, but it is no longer presented as what is true now.
Red-then-green on a unit test at that function, and it is durable wrongness rather than a window —
nothing re-examines `decided`, so it never self-corrects.

**Two things are worth carrying.**

1. **A residual that says "measure first" can be right about the method and wrong about the
   question.** The measurement it asked for — how fast do the bytes move — has a boring answer.
   The interesting one was next to it and cost one grep: how many callers does `replicate` have,
   and is there a retry. Nothing about replication *latency* was ever going to find that.
2. **`pgrep -f <pattern> | kill` is `pkill -f` wearing a different hat**, and the existing pitfall
   entry did not cover it, so it was hit twice in one session — once directly, once through
   `BPID=$(pgrep -f "offloadd --config …")`. The shell running the line has the pattern in its own
   command line, so the compound kills itself and everything after it silently does not run. It
   reads as the daemon failing to start. Also: a restart that fails to bind leaves the CLI talking
   happily to the **old** daemon, so a "verified after the fix" reading can come from the binary
   without it — caught here only because the answer looked wrong.

**Left open, with evidence and no mechanism.** A peer resuming a run whose blobs it lacks failed
with `state store: blob 345e28fe… is not stored here` instead of fetching them — with the holder
up, both nodes seeing each other `alive`, the holder demonstrably holding both the row and the
file, and **no blob-fetch line at all** under `offload_cluster=debug`. `prepare_start` does call
`ensure_blobs`, and `fetch_blob` errors honestly when nobody can supply a blob, so neither of the
two obvious explanations fits what was logged. Two occurrences got different distances — one past
`workspace ready`, one before it — so the path is not pinned. It is the next thing to pick up and
it is bigger than the residual that turned it up.

## Session fifty-four — a guard cannot go inside somebody else's program

The last residual ADR-0050 and ADR-0051 both named, and the one thing on the pick-up list that
was neither blocked on hardware nor a decision already argued: `remove` spans an await and no
check closes that.

**The two teardown doors were guarded correctly and both were still open.** `cleanup` loads the
run, refuses a non-terminal one, and calls `remove` as the next statement — two handoffs suspected
a gap there and it was not there. The sweep asks `reclaimable` **twice**, the second time with
nothing between it and the removal, which was worth 40 of 40 attempts when it was built. Neither
helps, because the effect is `git worktree remove --force`: a subprocess, a few milliseconds long,
deleting a directory tree. **There is no later place to put a read.** That is the rule this
session is really about — "a fence after the effect is not a fence" holds one level down too,
where the thing that cannot be split is somebody else's program rather than a statement.

**Measured before anything was built**, in a scratch repo with no daemon, the same shape session
fifty-three used for the `worktree add` race: `prepare` a worktree, write an uncommitted file into
it, start `remove`, then start a rebuild — `adopt`, falling through to `prepare` — a fixed number
of milliseconds later, sweeping the delay.

| rebuild starts | passes | outcome |
| --- | --- | --- |
| 0–1ms into the removal | 10 | **all ten adopted a checkout that was gone when `adopt` returned** |
| 2ms or later | 50 | all fifty fine — removal finished, `Absent`, rebuilt |

A ~2ms window that the rebuild loses **every** time it lands in, which is what a lock is for and a
check is not. What the loser gets is `Adoption::Current` for a directory that no longer exists,
"adopted in place" in the run's own log, an `install_transcript` that skips the install on the
strength of that word, and an agent spawned into a `cwd` that is gone.

ADR-0052 is the fix, and **its shape is the part worth carrying**: `WorkspaceManager::hold(run)`
returns a `CheckoutGuard`, and `prepare`, `adopt`, `supersede` and `remove` are methods **on the
guard**, taking the run id from it. So the compiler asks for the lock rather than a comment asking
the next session to remember it — and a guard for one run cannot be used to change another run's
checkout, which is the mismatch a `&CheckoutGuard` parameter beside a `run: RunId` would have let
through. `--strict-mcp-config` sat in the pitfall list for two phases as a warning about a hole
that was open the whole time, and the difference was never that somebody forgot to read.

**Three things are worth carrying.**

1. **The closing argument is what makes the lock narrow.** It is held across the *sequence* — a
   resume asks three of the four in a row — and dropped before the agent starts, not for the
   run's lifetime. That is sound because `register` claims the run in `live` **before** `launch`
   spawns `drive` (ADR-0051): a rebuild that has started *holds* the checkout, so the sweep's
   `try_hold` refuses; one that has not started has already *registered*, so `reclaimable`
   refuses. No third moment. Without that ordering the honest fix would have been much wider.
2. **`try_hold` for the sweep, and finding out why cost a hang.** Written first with `hold`, the
   node test did not fail — it *deadlocked*, the sweep waiting behind the checkout the test held.
   For a real resume that wait is a bundle fetch. A background tick blockable for the length of an
   unrelated operation is a worse bug than the one being fixed, and a checkout somebody is
   rebuilding is one the sweep must leave alone anyway, so "busy" is a complete answer.
3. **Taking a lock is an await, so `cleanup` takes the hold *before* the load.** The other order
   would have put its terminal check back on the far side of one — undoing, in the act of fixing a
   race, the exact property two previous handoffs had already cleared that function of. `Failed`
   is terminal *and* resumable, so `offload rm` and `offload resume` on one run are two commands
   one keystroke apart.

**Checked, both doors, both ways, and then walked.** The workspace crate's test walks the window;
the node's is deterministic. One line — making `mutex_for` hand out a fresh mutex each time —
fails both, and both were watched red before green. Then one daemon and a fake agent, because a
passing fixture is not evidence that a refactor of `drive` still starts anything: a fresh run
prepared its worktree and ran, `offload checkpoint` left it `pending`, `offload resume` reported
**adopted in place** and carried on from turn 9, `offload rm` refused with *still active; cancel it
first* while it ran and removed the checkout once it was terminal. No warning or error in the log
across the whole walk.

**Cost, and what it did not cost.** 904 tests from 901, wire and schema untouched, no gossiped
field and no new run state. The edit is wide and shallow — every caller of the four methods
changed shape — which is the price of making the rule a type instead of a paragraph.

## Session fifty-three — the door that had no lock, and the git message that proved it

Item one, and the only defect on the list: *one grant, two workspaces*. The previous session
recorded it as an observation with no mechanism — twice in eighty daemon passes, alpha built two
worktrees for one run 1ms apart, one `git worktree add` failing, writing `Failed`, and fencing out
the healthy leg — and said explicitly that nothing linked it to that session's change.

**The git message was the evidence, and it had been sitting in the log the whole time.**
`WorkspaceManager::prepare` passes `-B` and **no `--force`**, and without `--force` git's
`die_if_checked_out` fires first and says `'X' is already used by worktree at 'X'` — which is what
a *leftover* checkout produces. The logged sentence was `cannot force update the branch 'X' used
by worktree at 'X'`, which comes from `create_branch`, one check later. Reproduced outside this
codebase on git 2.55: 200 forced pairs of concurrent `worktree add` at one path gave 17 of the
second wording and 182 of the first. `git worktree add` is a read-decide-write too, and only the
loser of a *genuine race* gets through the checked-out test before the winner registers. So the
symptom was never a stale worktree and never anything upstream in placement: it was two `prepare`
calls at once, which is two `drive` tasks, which is two legs past `Supervisor::register`.

**And no epoch could ever have separated them, because both were entitled to it.** `Run::assign`
ran once. A second caller arriving afterwards finds a row saying *this node holds this run under a
lease of its own* — what the running leg wrote — so `hold_here` takes the already-ours branch,
correctly, and hands back the epoch it read. The one place that could refuse was `register`, and
it `insert`ed unconditionally. `BTreeMap::insert` **replaces**: the running leg's cancel sender and
event stream went on the floor while the leg carried on, so the displaced agent also became one
nothing could ever stop. ADR-0051 makes the claim exclusive under the `live` lock, in the writer
rather than in its four callers, with `SubmitError::AlreadyGoing` as its own variant.

**Three things are worth carrying.**

1. **A capacity check that excludes the run cannot also be the check that it is not already
   running.** `take_run` excludes the arriving run from every question it asks — deliberately, and
   correctly, because a run must not be counted against itself. That exclusion is silent about
   *"is it already going here"*, so a grant for a run this node was already running went straight
   to `start_run`. Two questions, one exclusion, second question never asked.
2. **A filter taken before a loop that awaits is not a check**, and neither is a check followed by
   two more steps. `start_held_runs` filters its snapshot on `live` and then works through it one
   blob fetch at a time; `resume` checks `live` and then does two more things. Both are the
   read-decide-write shape that put `hold_here` inside `Store::update_run` — one map further out,
   where nobody had looked.
3. **A failure message is a measurement.** The distinction between two git wordings, four minutes
   to establish in a scratch repo, is what turned an unexplained observation into a named door.
   Nothing in the daemon had to be run to learn it.

**Measured.** The unit reproduction is the one that decides, because it is deterministic: hold a
leg open inside `ensure_blobs` — the only `await` in `start_run` — and start the run again with the
row the first leg wrote. Without the refusal the second call registers over the first and walks
into the blob fetch, waiting on a gate only the first leg opens; the timeout is the assertion, and
it goes red in five seconds. The daemons were walked anyway, since a test cannot say whether a
migration still works with a refusal in the start path. Twenty-four passes of the drain-race
staging at `probe_interval_ms = 200`, two runs each: **48 traversals of `start_held_runs`, 48
clean migrations, and no anomaly of any kind** — no refusal, no worktree collision, no duplicate
grant, no leg reporting a lost run, no run ending `failed`. The honest reading of the zero
refusals is that twenty-four passes is not enough to expect one: the symptom was two in eighty, so
the walk says the fix costs a migration nothing, and says nothing at all about how often the door
was being opened.

The staging is also cheaper than the one it replaces. Both occurrences followed `there is room now;
starting the run held for it`, which session fifty-two reached only when the machine happened to be
busy — `max_concurrent_runs = 1` and a second run with `--queue` puts every pass through
`start_held_runs`, twice. That and two other demo traps are in `docs/DEMO.md`.

## Session fifty-two — the arbiter that raced itself, and the round that had no lock

Item one, which the previous session found by accident and deliberately did not fix. `Mesh::drain`
releases a run and calls `Cluster::place` for it; `Mesh::supervise`'s tick snapshots the view,
sees the same run `Pending { let_go_by }` and unheld, and calls `Cluster::place` too. On the
departing node those are **one node**, so both rounds read `run.epoch` from copies that say *N* and
both grant *N+1*. Nothing downstream can order them, because fencing protects a run from a *stale*
leg and neither of these is stale.

**The comment was the bug's hiding place.** ADR-0042 removed `Mesh::owed` so that exactly one node
offers an unheld run, and the loop says so: *this is the only offerer, which is the property that
matters*. That is a claim about **nodes**, read for three sessions as a claim about rounds. It is
the door-counting lesson again: the answer to "is it fixed" is not the answer to "how many are
there".

ADR-0050 puts a **per-run rendezvous inside `Cluster::place`**, held for the whole round. Three
things it decides, and each had an alternative that looks fine and is not:

* **In `place`, not in its six callers.** The two that raced were not the two anybody would have
  guessed, and this is the argument that already put `Supervisor::holds` in the writer rather than
  at each call site. The storm harness and the submission path are covered without being touched.
* **The loser waits rather than skipping.** The drain's pass cannot yield: under `Lifespan::Exits`
  `main` stops every agent the moment `depart` returns, so standing aside for "the tick, later"
  hands the run to a tick that never runs again — the stranding ADR-0042 exists to have ended.
* **…and then stands down rather than running a second round**, returning what the round it waited
  for decided. Running it after the wait is the same bug one lock later. The drain then counts a
  handover it did not itself perform, which is right: `offload drain` reports what became of the
  run, not which task did it.

**Measured.** The unit test is the reproduction that matters, because it is deterministic: two
`place` calls joined on one node, with the rendezvous bypassed, offer the winner the run twice at
`Epoch(1)`.

The daemons were walked anyway, because a test cannot say whether a drain still hands a run over
with a lock across the round. Thirty-two control passes on the unfixed build (sixteen at the usual
one-second tick, sixteen at `probe_interval_ms = 200`) produced **one hit** — and the hit is the
useful part, because it was **silent**: `handed over` and `an unheld run was placed` 1.78ms apart,
two `granted to 3e453757 at epoch 3` rows, and no error anywhere, because bravo *committed* rather
than starting so there was one clone and no colliding `git worktree add`. The git collision that
made session fifty-one notice this is incidental to it. Then thirty-two passes on the fixed build: **no double grant, and thirty clean migrations**.

**And the walk turned up something it could not close**, which is written down as an observation
rather than a finding. Twice on the fixed build — and never in forty-eight control passes — alpha
created **two workspaces for one run**, 1ms apart, off a single grant: one `git worktree add` failed
`cannot force update the branch … used by worktree at <the same path>`, wrote `Failed`, and fenced
out the healthy leg, which reported `this leg lost the run`. Same ending as the double grant and a
different door. Sixteen further passes on the fixed build did not reproduce it, thirteen of them
through the `there is room now` path both occurrences went through, and the window in `start_run`
between `hold_here` and `register` has **no await in it** — so there is no mechanism linking it to
this session's change and none proposed. Two in eighty passes, unexplained, and at the top of the
pick-up list with the log excerpt.

Two demo traps cost more of this session than the fix did, and both are in `docs/DEMO.md`.
Scraping the passphrase out of `offload init` with `grep -Eo '([a-z]+ ){5}[a-z]+'` picks up *"It is
shown once and stored"* three lines above the six words — and `join --passphrase` does not fail on
it, because the passphrase **is** the fleet, so the device enrols into a *different* one and the
mistake surfaces two steps later as `could not reach seed … that certificate is for fleet <other>`.
And `while pgrep -f batch.sh; do sleep 5; done` never exits, because the shell running that line
has `batch.sh` in its own command line — the documented `pkill -f` trap from the other side, and
quieter, since the loop looks like patience and every command after it in the compound never runs.

## Session fifty-one — the fact that was not a field, and the arbiter that raced itself

Item two from the list, and it had been parked twice for a reason that was correct and one step
short. `reclaim_departed_checkouts` removes a departed run's worktree and wrote a `tracing::info!`
on a machine nobody is logged into. The stated blocker: `cleanup` notes `workspace: "removed"`,
`RunProgress::workspace` is a single fleet-agreed value with one writing leg, and the sweep runs on
the leg that has *lost* the run — so `update_stats` drops the write, correctly, because a losing
leg describing a worktree is session seventeen's bug. And every other field on the record has the
same shape. Two handoffs read that as "so nothing can say it", and the previous one went looking
for a new kind of gossiped fact with attendance (ADR-0013) as the precedent.

**It did not need to be a field at all.** What happened to a directory is a fact about *one disk*:
no other node holds a copy, so there is nothing to merge and no owner to arbitrate. The per-node
audit log has existed for exactly that since phase 3 — `offload_core::audit`'s own header says
*sometimes the answer to "who owns this field" is that it should not be a field* — and it is
durable, it is subject-indexed by run, it survives the run's record being pruned, and it already
has a door in `offload audit <run>`. So: `AuditEvent::Reclaimed { run, why }`, no schema migration
(the table stores the variant as JSON beside a `kind` string) and no wire bump (the type is
control-socket and local-store only). `Reclamation::{MovedOn, NobodyWaiting}` because the sweep has
two doors and "your worktree is gone" without which one is a decision that discarded its reasoning;
`reclaimable` returns the door alongside the run, so the sentence comes from the function that
decided rather than from a second copy of the rule read at a third moment.

**Measured**, two daemons, with the sweep's fifteen-minute timer shortened to fifteen seconds:
alpha alone, submit, bravo late, `offload drain`. Handover at 18:21:03, `reclaimed the checkout of
a run this node is not running` at 18:21:18, and one line above it in `offload audit`:
`reclaimed this run's checkout here: another node has run it since, so the work went with it`. The
run's own record was untouched, which is the half that must not change. The old behaviour and the
new one are visible in the same pass — the tracing line is still there, and the row is beside it.

**And the walk found something the item was not about.** In the same logs, alpha placed one run
**twice**: `handed over` at .806206 and `an unheld run was placed` at .807632, two
`granted to <bravo> at epoch 3` rows in its own audit log, two `cloning repo mirror` lines on
bravo, and two concurrent `git worktree add` for one run. `Mesh::drain` releases the run and calls
`place` itself; `Mesh::supervise`'s tick snapshots the view, sees the same run `Pending {
let_go_by }` unheld, and calls `place` too — the same node, both computing the grant epoch from
their own copy, both granting *N+1*. The loop's comment says *this is the only offerer, which is
the property that matters*; it is true of nodes and the departing node runs two passes. Reproduced
three times in about sixteen, and **only** with the late join — sixteen passes with both daemons up
from the start, four of them at `probe_interval_ms = 200`, were all clean, so what widens the
window is still unidentified.

Two things about it are worth not re-deriving. **Git is what stopped it being two agents**, not a
fence — the second `worktree add` collided on the branch name — and the outcome is worse than a
coin flip, because the leg that collided wrote `Failed` first and fenced out the healthy leg that
had its workspace ready and its transcript restored. A clean migration ends as a run `failed` with
a raw git error on it. **And the obvious fix is not one**: a re-read before `place`, this
codebase's favourite idiom and the one that fixed this very sweep, narrows a window that two
overlapping rounds can still hit exactly. Exclusion per run per node is what is wanted, and the
drain's pass cannot simply yield, because under `Lifespan::Exits` `main` stops everything the
moment `depart` returns. That is an ADR, and writing it from one measurement at the end of a
session is how a fencing hot path gets a fix worse than the race. It is item 1 on the list,
with the staging and the measurement written down.

Left the tree at 899 tests, clippy clean, wire v26, schema v11, and both walk constants
(`PROBATION`, the sweep timer) restored.

## Session fifty — a record that names a command, and the door that answered it wrong

Item two from the list: `recover` writes `failed — resumable from turn N with 'offload resume
<id>'` onto an interrupted run at startup, and since ADR-0047 that is a command a drained, revoked
or policy-refusing node will not answer. **Measured** on one daemon, `kill -9` mid-run and a
restart under `[policy] accept = "never"`:

```
$ offload ps --all
01a053add754   failed    5   550   -   here only   clean   walk the parser
           └─ daemon restarted while this run was active — resumable from turn 5 with `offload resume 01a053add754`

$ offload resume 01a053add754
Error: node is not accepting work
```

The handoff had already ruled out the edit that suggests itself, and it was right to: rewording
the record puts a fact that changes by the hour into a permanent note about why a run stopped, on
a row that gossips. So the record keeps saying what it says, the node's standing answer travels
beside the listing, and the two are paired where somebody is reading the advice.

**Both halves come from the daemon**, which is the part worth carrying. The refusal is
`server::hosting_refusal` — the resume door's own function, not a second reading of the same three
questions — and the relevance gate is `supervisor::resumable_state`, split out of
`refuse_unresumable` so the report asks what the door asks. The gate is **per row**, not per
listing, because `offload ps` hides finished runs unless asked and a node-level gate would print
advice about work that is not on screen. `offload explain` carries the same line beside
`recovery`: what this node does *unasked* and what it does *when asked* are two answers, and a run
left for a person is precisely one where a person is being invited to do something the node may
decline.

**Then the pairing found the bug.** On the third door — a revoked fleet-of-one — the new footnote
read `not of this node right now: this node is draining`, one line under a run that had failed
with `this node was revoked from its fleet`. Two sentences, one daemon, one line apart, and the
report was the honest one. `stand_down` sets **both** latches (ADR-0044) and `hosting_refusal`
asked the drain first, so every door it guards told an evicted device's owner to restart
`offloadd` — which changes nothing, because the way back is `offload join`. `status` had the right
ordering and its reasoning written beside it the whole time; the door and the report were two
orderings of three questions and only one of them had the argument. Measured at the door, against
the *submit* door a second later, which reaches the fleet state through the mesh and named the
revocation correctly.

Three things this leaves.

1. **Both orderings refuse, which is why it survived.** Nothing was let through, no run was
   duplicated, and every test asserting a revoked node takes no work passed. What was wrong was
   the *cause* — ADR-0043 deleted `NodeIsDeparting` for this and ADR-0049 gave the handover a
   third sentence for it, and here it is a third time at the one door a person types at. The
   operator's next move differs: wait for the drain, or enrol the device again.
2. **The first fix was wrong in the safe-looking direction.** Guarding the drain clause with
   `!is_revoked()` let a revoked node whose `fleet.json` could not be read fall past every clause
   to *yes*. `revoked_refusal` is one place shared by the door and `status`, and its fallback is
   the constant rather than `None`. Caught by the unit test a minute after it was written.
3. **A tautological test is worse than none.** The first version of the resumable sweep compared
   `resumable_state` with `refuse_unresumable`, which *calls* it — so drifting the predicate
   drifted both sides and the test passed on any answer at all. Confirmed by drifting it
   deliberately. What it pins now is the content: which of the eight states a run can be found in
   are the two, written out rather than computed.

Not an ADR: nothing travels — both new fields are on the control socket — and the revocation half
is a bug fix restoring what ADR-0043 and ADR-0044 already claim.

## Session forty-nine — the last door, and it opens by itself

The one left on the list, and the one nobody types at: the recovery tick. ADR-0048 had made it
worth asking — the only clause of `permits` a tick could usefully consult is one that changes
under a running daemon, and until an hour earlier those were frozen at startup.

**Measured** with session forty-eight's fake `nmcli` and a fake agent that fails on its first leg:

```
16:21:45  run starts, link free
16:22:06  what this device is has changed          (metered)
16:22:33  run failed; noted who was watching       attendance=unattended
16:23:06  nobody was watching; resuming it         resume=1
16:23:06  spawning claude code
```

For that whole minute the node reported `accepting no — network is metered and policy disallows
it`, refused a submission at its own socket, and refused an `offload resume` of that very run.
Then it started the agent itself.

**ADR-0049.** `Circumstances::hosting`, a two-variant type rather than a fourth `bool` — the
struct exists because a boolean among small integers is unreadable, and a fourth would undo that.
It behaves like `departing` and not like `revoked`: a node that will not host is still one the
fleet can hear, so `Resume` and `Wait` become `LetGo` under ADR-0043's mechanism and its
`is_durable` guard. Where nobody else holds a copy it gets its own sentence —
`OwnerWillNotHost` — because "this node is leaving" about a laptop sitting there at 15% is the
wrong cause confidently stated, which is exactly what ADR-0043 deleted `NodeIsDeparting` for.
Where both are true, departing wins. `offload explain` asks the same question through the same
function, over the same live handle the `accepting` line reads.

Measured after, on the fleet of one that produced the symptom: the run stays `failed`, nothing
spawns, and `offload explain` says `left for a person: this node's owner does not allow it to host
runs, and its conversation exists nowhere else`.

**The branch that was written off, and then walked twenty minutes later.** The `LetGo` half — a
peer holds a replica, the run goes back to the fleet — was staged as a two-daemon walk and
blocked: an unrelated process pinned this machine's load average at ten, at which point the bid
refuses every submission for CPU pressure. That went into the ADR and the handoff as unwalked,
which was the right thing to write; then the machine went idle and it was walked:

```
16:48:49.536  handing the failed run back to the fleet
                reason="this node's owner does not allow it to host runs"
16:48:49.948  spawning claude code   session=fake-session-1   (on bravo)
```

412ms, same conversation, turn 12 to turn 16 on the other machine, and alpha never started an
agent. **And it found the third place the ADR's own sentence lives.** On the first pass that log
line read *"this node is leaving; handing the failed run back to the fleet"* — about a laptop
sitting right there on a metered link. §3 of the ADR is about exactly that mistake, and it had
fixed the two places somebody would look: the escalation and `offload explain`. The
`tracing::info!` a few lines from the transition was not one of them. **A sentence has as many
places as it has callers**, which is the door count one layer down, found the same way: by
running it.

**And the residual it left was measured rather than carried.** ADR-0049's first "what this leaves"
was the worry that a run handed back for a policy reason bounces: the fleet places it, this node
bids, its bid is refused for the same policy, round and round for as long as the laptop is on
battery. Staged with a peer that is *enrolled but not granted* `host-runs` — which holds a replica
and can never win a bid, so no `PROBATION` edit and no second grant — the answer is one round every
30.2 seconds, exactly `REASSIGN_RETRY`, nothing at INFO, and an epoch that does not move across six
minutes. 120 rounds an hour on a canvass of the live fleet. Nothing to build, and the shape of the
check is worth keeping: **count the rounds, then check the epoch** — a retry loop that is cheap in
messages and expensive in tokens looks identical in a log count.

**The lesson, and it is the third variation on it in three sessions.** Session forty-seven: count
the doors. Session forty-eight: ask how fresh the fact behind them is. This one: **the last door
to be found is the one that opens without being asked.** Seven doors on this gate now — the bid,
the grant, the no-cluster arm's drain clause, the tick's `departing`, the tick's `revoked`, the
policy on both doors a person types at, and the tick's own. Every fix was measured, correct and
written up. What kept being missed was the counting.

## Session forty-eight — and how fresh is the fact behind the door?

Picked up session forty-seven's own first item — the recovery tick not asking the owner's work
policy — and stopped one step short of it, because the fact the tick would ask about turned out to
be frozen.

**Which clauses can even change?** `permits` has four: `accept`, the battery floor, `metered`,
`allowed_agents`. Two come from config and cannot move without a restart, and a restart makes a
failed run `AttendanceUnknown`, which escalates rather than resumes — so the tick could never have
been reached through those. The two that *can* change under a running daemon are the ones the
probe supplies: the battery floor and metered. Which made the next question what they were being
read from.

`Ctx::capabilities` was an `Arc<Capabilities>` built once in `main` and never replaced. The
comment above it had been true and had stopped being so — *"phase 3 will re-probe on a schedule to
keep gossip honest. For a single node with no one to tell, once is enough"* — and phase 3 built
exactly that sentence: a re-probe that told `Cluster::set_capabilities` and nothing else, from
inside the loop a fleet of one never enters.

**Measured** with `nmcli` replaced on the daemon's `PATH` by a script that reads its answer from a
file, so the link becomes metered under a running process and nothing about the machine's real
network is touched. Thirteen seconds after the flip the daemon logged `capabilities changed;
gossiping them`. Then, seconds apart:

```
$ offload status                              accepting   yes
$ offload run  …                              Error: no node will take this run
                                                solo   network is metered and policy disallows it
$ offload resume 01a053607c0a                 resumed → running, turn 4
```

The bid round reads the cluster's copy and refuses. The `accepting` line and the door ADR-0047 had
just installed read the snapshot, say yes, and start an agent on the link the owner is paying for.
And on a fleet of one with no cluster there was no re-probe at all: on the old build, sixty
seconds after the flip, `offload probe` said `metered — bytes cost money here` while `offload
status` said `accepting yes` and the submission ran.

**ADR-0048.** One live handle, `deliver::Current`, with `now()` and `refresh()`; every local
reader goes through it, and a site that needs the device class *and* the capabilities takes one
`now()` for both, because a probe landing between two calls would answer with half of each.
One writer — the `reprobe` task — moved out of the gossip loop and spawned whether or not there
is a fleet, which is `tend_own_runs`' rule for the third time: **a node's own state needs tending
whether or not there is a fleet.** The fleet's copy is still set from the same probe in the same
statement, and the gossip decision stays `Cluster::set_capabilities`' own.

Measured after, on both stagings: `accepting no — network is metered and policy disallows it`, the
submission refused, the resume refused, the run left `pending` — including on the fleet of one,
where the loop that answers now exists.

**The lesson is one level under session forty-seven's.** That session's was to count the doors.
This one's is that counting them is not enough: **a snapshot is correct until somebody decides
with it**, and the change that turns a printed fact into a gate is the change that has to ask how
fresh the fact is. Two ADRs did that here, one after the other, in one session, and neither asked.

The tick is still not asking the owner's policy. It is now worth asking, which it would not have
been an hour earlier, and it is still an ADR with `decide_recovery`'s exhaustive sweep to re-run
rather than a clause to append.

## Session forty-seven — how many doors are there?

The handoff's first pick-up item was a **decision**, not a patch: a node reporting `accepting no`
still ran what you typed at its own socket. Session forty-six had measured it while walking
ADR-0045 and deliberately left it, because whether the owner's policy binds a submission typed at
this machine is a question about whose authority a keyboard carries.

**ADR-0046 decided it, in the direction `AcceptWork::Never`'s own doc comment had always stated**
— *control plane only… never host them*. `server::place`'s no-cluster arm asked two of the three
questions a bid asks; its own comment said "the two questions", and `Host::evaluate` asks three.
The third is the owner's policy, and the arm's comment history already recorded this exact symptom
for the *drain* clause, added after the same measurement: *"drained, `offload status` saying
`accepting no`, and the next submission accepted and started anyway"*. Same path, same words,
second clause. It also found `allowed_agents` being checked *after* capacity — so a node whose
owner forbade an agent, and which happened to be full, returned `AtCapacity`, which is the one
refusal that is not a no: a full node commits (ADR-0006). `WorkPolicy::permits` holds the four
clauses that do not empty by themselves, `admits` calls it first, and the general rule is written
down: **inside `admits`, order by whether the refusal can go away on its own.**

That much was committed. What it had not done was walk its own after-state or write any of it
down, and the ADR named two residuals. Both were taken up here, and one of them opened the
session's real finding.

**The `accept = "never"` half, walked both ways.** One daemon, no `[cluster]`, a fake agent: before
the fix, `accepting no — node is not accepting work` and the run completed; after, refused with
that same sentence. The A/B was `git checkout b5459b4^ -- crates/offload-node/src/server.rs`,
ninety seconds, and it also proved the change was one file.

**The `allowed_agents` residual cannot be walked at all**, which is worth more than the walk would
have been. ADR-0046 asked whether a forbidden agent really does get committed and started. It
cannot be staged: `AllowedAgents` refuses every name but `claude-code` at deserialize time and
refuses the empty list, and `Supervisor::build` writes `ClaudeCode` into every `RunSpec` — so the
only set a config can hold always contains the agent every run names. Measured, three configs:
`["claude-code"]` starts, `[]` and `["codex"]` are refused at startup. `Refusal::AgentNotAllowed`
is a sentence no daemon this build can start will say. That is not the ADR-0011 disease — nothing
reads as applied, because the two refusing shapes are refused loudly and pointed at `accept =
"never"` — but it does mean the clause becomes reachable and unnoticeable on the same day, when
the second adapter lands. So the ordering is pinned by a unit test that reproduces ADR-0046's own
measurement, `Err(AtCapacity { running: 1, max: 1 })`, and fails on the old order. There was no
test of any kind touching that refusal before; the ADR shipped an ordering fix and 890 tests, the
same 890 as the day before.

**Then the second door.** ADR-0046 had stated its own invariant — *"`permits` is asked on every
path that can start a run"* — about one door. `offload resume` starts an agent the same way a
submission does, and its handler asked **none** of the three: it resolved the id, built a `Room`,
and called `Supervisor::resume`, so the only gate on the path was the queue. Measured on one
daemon, three times:

```
accepting   no — node is not accepting work                       → resumed, turn 6
accepting   no — drained; restart offloadd to take work again     → resumed, turn 5
accepting   no — this node has been revoked from its fleet …      → resumed, turn 7
```

The third undoes ADR-0044. That node had been revoked a minute earlier, had halted its agent, had
written *this node was revoked from its fleet, so it stopped running it* onto the run, and refused
`offload run` with the fleet's own sentence — and then resumed the agent when asked by name. The
failed run's footnote, `resumable from turn 16 with 'offload resume …'`, was pointing at the hole.

**ADR-0047**: one `hosting_refusal(ctx)`, asked by both doors — draining, then the grant (where a
revocation turns a node away), then `permits` — with the resume door asking it after `resolve`
(a typo still earns `no such run`) and before `Supervisor::resume` (whose refusals are about the
particular run, and one of which would otherwise have to be reported without spending the retry
budget). `resume_run` beside `submit_run`, which is what made the handler testable; the arm had
grown to eighty lines with the decision inside it. Three tests, including one that asserts the two
doors produce the *same string*, because the failure mode of two lists is a drift nobody sees.

**The lesson is not the clause. It is the counting.** Five clause-shaped fixes have now been made
to the same gate: the drain on the submission arm, `departing` and then `revoked` on the recovery
tick, the owner's policy on the submission arm, and now all three on the resume door. Every one of
them was measured, correct, tested and written up. Not one asked *what else starts an agent* — and
the answer was a match arm in the same file, old enough that the drain and the revocation had both
been built around it. **When a clause is found missing, the next question is not "is it fixed" but
"how many doors are there".**

What is left, deliberately: the recovery tick still does not ask the owner's work policy, and the
obvious edit is the wrong one — it counts a refusal as a spent retry, so a battery dip would burn
the run's budget. It wants a standing fact in `Circumstances` beside `departing`, which means
re-running `decide_recovery`'s exhaustive sweep. Adoption at startup was checked and needs nothing:
a restarting daemon marks the interrupted run `failed` and spawns nothing, measured on the revoked
node.

## Session forty-six — a control that read as applied and could not fire

The handoff's first item is the phone demo, and it is blocked on a phone rather than on code:
phase 5's four open items each need hardware or a VPS this machine is not. Looking at what the
*probe* would need for the mobile half turned up something else.

```rust
// Assumed until proven otherwise; a phone's daemon should override this from the
// platform's connectivity API, which is the only reliable source.
caps.metered_network = false;
```

That was the only write to the field in the workspace outside a unit test. No config key, no
override, no platform call — a `bool` that was a constant, and four readers treating it as a fact:
`WorkPolicy::admits`, so `Refusal::MeteredNetwork` was a sentence no fleet could produce;
`accepts_replica`, so the metered half of ADR-0016's durability check never fired; `bid`'s
`metered_penalty`; and `Constraint::UnmeteredNetwork`, so `offload match "unmetered"` and a run's
`offload match "unmetered"` — a documented command whose whole job is that question — said yes
on **every device there has ever been**. (No submission can carry that constraint: `offload run`
has no `--require` flag, so that reader was a wrong answer to a person while the other three were
load-bearing.) `offload policy` printed `metered
network refused` on every device, guarding a state nothing could enter.

**This project had already diagnosed exactly this**, and the sentence is four lines further down
the same function that prints the line above: *"It was on `WorkPolicy` and enforced by `admits`
from the beginning, with nothing able to set it — so `Refusal::AgentNotAllowed` was a sentence no
fleet could produce. A policy rule that cannot be reached is worse than one that does not exist,
because it reads as a control that is being applied."* That is `allowed_agents`, ADR-0011's fix.
The field beside it had the same disease, and `offload policy` printed both.

**ADR-0045**, in four parts. The capability is three-valued — `Metered { Yes, No, Unknown }`, the
shape `PowerSource` four fields up already has, and for the same reason the probe states about it:
"`Unknown` says so and is not folded into either". The owner nominates with a top-level `metered =
"yes"`, which is this project's standing pattern for a fact only the owner holds. Where they said
nothing, NetworkManager is asked — and its `(guessed)` suffix is kept rather than cast away:
`no (guessed)` becomes **`Unknown`**, because NM guesses no for every wifi and ethernet link
including a tethered one, which is the first case the field's own doc comment names. Promoting
that guess would have reimplemented the bug with a nicer type.

**The part worth the ADR is what `Unknown` means, and the four readers disagree on purpose.** A
policy refusal claims the bytes cost money; a constraint claims they do not; `Unknown` proves
neither, so it supports neither — the burden of proof lies with whoever makes the claim. Refusing
work on `Unknown` would stop every machine without NetworkManager from hosting runs or holding
replicas, a certain cost paid to avoid a possible one, which is the same over-claim pointing the
other way. "Unknown is not good news" means unknown must not be quietly converted into whichever
answer the caller found convenient — not that it is always the pessimistic one.

**Measured, all three rows.** Before: `offload match "unmetered"` said *match: this device
satisfies the constraint*, `offload probe` printed no network line at all, and `offload policy`
said `metered network refused`. After, with the key unset, the run is still accepted — behaviour
on every existing deployment is unchanged — and `offload match "unmetered"` says *no match:
metered = unknown, and this run needs it known to be free*. With `metered = "yes"` and a one-node bid round:

```
Error: no node will take this run
  fedora       network is metered and policy disallows it
```

the first time the fleet has produced that refusal.

**And one thing the walk found that is not this ADR's.** A daemon reporting `accepting no —
network is metered and policy disallows it` accepted a submission at its own socket and ran it to
completion: `server::place`'s no-cluster arm goes straight to `Supervisor::submit`, which checks
`Room::for_one_more` and never `WorkPolicy::admits`. Pre-existing, and left alone — whether a
person at the keyboard is subject to the owner's own policy is a decision on the same axis
ADR-0037 §6 opens from the other end, and it is in the pick-up list with its measurement.

**Wire v26**, because `Capabilities` gossips inside `NodeView`. A v25 node's `false` never meant
"this link is free" — it meant a constant nothing could change — so it does not decode, on the
same argument v25 makes about a refusal missing its proof.

## Session forty-five — the queue path walked, and a report that knew more than the record

The handoff's second item: the `--queue` half of session forty-four's fix, "reasoned about and
tested, not walked". It is walked now, in both directions, and the walk found something the tests
could not — a sentence.

**The staging.** Three daemons and no probation. The founder hosts from the moment `init` returns;
two joiners with `submit, deliver` cost nothing. One submits (`accept = "never"`, so it arbitrates
and never bids), the other exists only to be frozen. `kill -STOP` the spare, submit with `--queue`,
`kill -STOP` the founder a second later: the founder bids, the frozen spare holds the canvass open
long enough to make the founder deaf, and the grant that follows is never confirmed. Four things
about that are in `docs/DEMO.md` and none of them are guessable — the canvass order is *node-id*
order, so which peer can hold the round open is decided by key bytes; a frozen peer stalls ~3s
rather than `bid_window_ms`, because its connection dies first; and `SIGSTOP` on a daemon is
undone by the kernel the moment the shell that started it exits, which is the orphaned-process-
group rule and reads exactly like the fix not working.

**The A/B, on the same pass with one line reverted.** With the fix, alpha records the run at
**epoch 2** — `assign` then `release`, the two bumps a refused grant costs — and `pending; last
assigned to host`. With `let queued = run.clone()` restored, **epoch 0**: the pre-round copy
written over the record `place` had just stored for exactly this reason. Then the harm, which is
the half that was never measured: thaw the fleet and let the retry round place it. On the reverted
build the founder is granted the run at **epoch 1** — the very token the swallowed round had
already issued to that same node, one arbiter spending one number twice, which is what `fence`
cannot order. On the fixed build the retry grants **epoch 3** and the founder runs it there.

**And the thing the walk was not looking for.** Reading `offload run` and `offload explain` about
one event, seconds apart: `host did not confirm: connection lost`, then `a728e97e let it go`.
`Run::release` has a third caller its doc comment did not know about — `Cluster::hand_over`'s
give-back is the *arbiter* taking its token back, not the holder changing its mind — so
`let_go_by` names a node that never held the run, and in the unconfirmed case may name the one
node that is running it. ADR-0006 treats silence as a decline in order to **act**; nobody observed
a decline. The field is right for the decision it exists for (offer it again, ADR-0042) and was
being printed as an intention. Both lines now state what the record can support — `last assigned
to` — which still separates a run a person parked from one nobody is coming back for, and that was
all ADR-0042 ever asked of it.

It had always been set that way. What changed is that session forty-four started *keeping* the
record a refused round leaves, so a claim that had been thrown away with the local copy reached the
store, the view and the operator. A fix that makes a thing durable makes everything it carries
durable too.

No ADR: nothing was decided that ADR-0042 had not already decided. Wire and schema unmoved.

## Session forty-four — the caller was still holding the copy it had before the round

The handoff's first item was the unfinished withdrawal audit's last two targets, each carried as a
note from *reading* rather than from walking: `Supervisor::cleanup`'s guard, and `Host::edit_spec`
publishing the store's row into the view. Both notes turned out to be pointing at the right places
for the wrong reasons, and the thing between them was worse than either.

**`Cluster::place` mutates the run and its caller does not know.** It takes `&Run`, works on a
clone, spends an epoch per grant attempt, hands the run over — and on the way out of a round nobody
confirmed, publishes the record at an epoch past everything it issued, with a comment saying why
that must not be lost. `server::place` then acted on `run`: the copy `Supervisor::build` produced
before any of it happened.

Both arms did it. On **acceptance by this node**, `confirm_record(&run)` publishes what it is given
straight into this node's own view, replacing the `Assigned`/epoch-1/holder-us record
`NodeHost::accept` had just published — deliberately, because its own comment says "`evaluate`
answers the next bid from the view, so a node whose view has not caught up with what it just
accepted bids as though it were idle". On **refusal with `--queue`**, `record_run(&run)` and
`confirm_record(&run)` wrote the pre-round copy to the store *and* back into the view, un-spending
the round's tokens from one line up the stack.

**Measured**, one daemon, `max_concurrent_runs = 1`, three submissions in the same second: before,
all three were told they could start **now**, and `offload ps` then showed two of them `waiting for
a slot`; after, the second says "when the run ahead of it finishes" and the third "when one of the 2
runs ahead of it finishes". That is exactly the distinction `Placement::Accepted`'s doc comment says
must not be blurred — "telling somebody the first when it is the second is how they close the laptop
expecting output by morning" — and on a fleet it is the "six submissions in two seconds all went to
the same node" symptom, restored one function above the fix for it.

**And "remembered" had to be made to mean written down.** The refused arm published its spent epoch
into the view and recorded nothing. A `ClusterView` is memory and is rebuilt from the store at
startup, so an arbiter that spent three tokens and was then restarted came back at the epoch it had
started from and handed the same numbers out again — which is the thing that block exists to
prevent, through the one door it did not cover.

**Three things are worth carrying.**

1. **A function that takes `&T` and changes the world has to hand the new `T` back.** `place`
   published its result and returned a verdict, so the caller had no way to use it except by
   knowing to go and look. `Placement::Refused { spent }` carries it now, `None` when the round
   handed nothing out, and the accepted branch reads the run back from the store — which is the
   truth for a run this node holds, and where `take_run` has just written it.
2. **Publishing is not remembering.** Anything a restart has to still believe goes in the store,
   and a comment that says "remembered" about a `publish_run` is describing an intention.
3. **Two notes made from reading were both wrong, and usefully.** `cleanup`'s guard is already
   adjacent to its effect — there is no `await` between them, so a second check would sit beside
   the first — and the difference from `reclaim_departed_checkouts` is that *that* one has a `git
   status` in the middle. It is the await that makes the gap, not the effect being dangerous. And
   `edit_spec`'s store-versus-view worry had exactly one non-racy instance, which was the spent
   epoch living in the view alone; with that recorded, what remains is a microsecond that heals at
   the holder's next gossip. Both are written down as checked so the next session finds the answer
   rather than the doubt.

## Session forty-three — the handler was on the path that cannot happen

Session forty-two closed the fleet's half of a revocation and left its own residual at the top of
the pick-up list: **a revoked node does not stop its own agents.** The fleet evicts it and moves the
run; the node, which can hear nobody, runs its old leg to the end. Some of that is the partition
ADR-0002 accepts and stays. The part that is not a partition is that this device could hear its
fleet perfectly well — on every dial it made, once a second, for as long as it ran.

**Why nothing acted on it.** `change.ourselves` existed and its handler logged and did nothing more,
and it was on the wrong path twice over. `ourselves` is set when `fleet.json` grows a revocation
naming this node, and the only way one arrives on a revoked device is by **gossip** — from the peer
that has just hung up on it in both directions and refuses every handshake it attempts from then
on. The revocation is precisely the thing that ends the channel that would have carried it.
Meanwhile `Refusal::Revoked` arrives on every dial and `TransportError::Refused` was matched
nowhere: on the reverted build it reads as `could not reach seed … refused by 32388d1a: member
64d1ca2e has been revoked`, at `WARN`, beside every other unreachable peer.

**Measured, before and after, on two daemons.** Before: revoked at 13:23:28.955, still `running` at
turn 38 forty-five seconds later with `runs 1/2 · accepting yes`, and **completed at turn 60** —
forty-eight turns of agent after the fleet threw the device out, while the peer held the run
`orphaned` and the node `dead`. After: the proof verified and filed at **+1.70s**, the agent stopped
at **+2.43s**, the run `failed` at turn 15 saying *"this node was revoked from its fleet, so it
stopped running it"*, `accepting no — this node has been revoked from its fleet…`, and one
`spawning claude code` in the whole log. ADR-0044, wire v25.

**Four things are worth carrying.**

1. **A refusal is a peer's claim about this node; the signature inside it is not.** That rule is not
   a technicality here — pointed at membership it is what stops a device talking itself back in, and
   a node acting on a bare `Refusal::Revoked` could be stopped by any peer that said so. So the
   refusal carries the signed `Revocation` and it is filed through `Members::revoked`, which is the
   same call, the same verify and the same file a gossiped one goes through. The field is
   **required** — a refusal that decoded with the proof missing is one the subject would have to act
   on without it, which is the failure the field exists to prevent — and this is the one message a
   node receives *after* being thrown out, so there is no later exchange to carry it instead.
2. **A fact that latches must not be reported as an edge.** Even filed, `ourselves` could not have
   reached the daemon's tick: `NodeMembership::file` reloads to refresh its own copy and **drops**
   the `FleetChange`, and `file` is exactly the path a revocation heard from anywhere else arrives
   on. The one announcement was eaten by the code that wrote the fact down. `revoked_here()` is a
   standing question now; revocation is monotonic, so ask rather than be told.
3. **A certificate that still verifies is not permission.** `FleetState::grants` read the
   certificate alone, and a revoked device's certificate lists its grants and verifies perfectly —
   that is the whole reason revocation is ADR-0012's exception. So `offload status` said `accepting
   yes`, the bid and the grant passed their `host-runs` check, and `offload run` submitted. One
   question, one answer, one place — and the two sentences above it were advice that cannot work:
   *run `offload grant`* to an evicted device, and *restart offloadd* to undo a drain the stand-down
   had set.
4. **A test double that models one end of a two-ended thing tests one end.** Session forty-two's own
   lesson, one layer down. `MemoryConnection::close` sets a flag — on the closing side only, so a
   peer that had been hung up on went on opening streams and gossiping. The revocation test written
   against that double **passed with the fix removed**: the subject's cached session survived, and
   the peer's reply gossip carried the revocation like any other. Revoking *before* the two ever
   speak is what isolates the path; the flag and its `Notify` are shared between the ends now.

**And the fifth path `stop_accepting` has been missing from.** Auto-resume asks neither the bid nor
the grant, so shutting those two doors on a revoked node would have left the one that spawns agents
open. `Circumstances::revoked` is checked above everything including `departing`, and unlike a drain
it does **not** become `Recovery::LetGo` — letting go is an offer to the fleet, and a revoked node
has none to offer to.

**A restart hides all of it**, which is why five sessions of walks never met it: a daemon that comes
back reads `fleet.json`, finds its own certificate unusable, and does not join the mesh at all. The
symptom exists only on a live process.

## Session forty-two — the half of the connection a revocation could not reach

The handoff's second item, the unfinished withdrawal audit, asked of three more operations what
session thirty-seven asked of `stop_recovering`: where is one piece of state written by two callers
with different intentions? The answer this time was not a piece of state but a piece of *the map* —
and the two callers were the two ends of a QUIC connection.

**What was wrong.** `Cluster::disconnect` removed a peer's entry from `connections` and closed it.
That map is keyed by node and holds the one session **this node dialled**; a session the *peer*
dialled is handed to `serve_session`, which owns it inside a spawned task and put it in no map at
all. So a hang-up reached exactly the half that did not matter — and the revoked device is by
definition the one still dialling. `Mesh::refresh_membership`'s own doc comment states the rule it
was breaking: *refusing the next connection to a device that is not going to make one is not a
revocation.*

**What that cost, measured on two daemons.** `offload revoke` typed on the arbiter while the other
node was eighteen turns into a run it was hosting. The revoked node kept its inbound connection and
went on gossiping over it — run records, lease renewals, checkpoints from turn 8 to turn 40 — and
finished the run and reported it home. `offload nodes` said `bravo alive ~48`: forty-eight absences
in ninety seconds and still `alive`, because the arbiter's own probe was correctly refused (`no
answer: suspecting`) while the peer's inbound traffic came straight back through `serve_stream`'s
"anything a peer sends is contact" (`answered: no longer suspect`). Once a second, for ever, so the
run was never orphaned and never moved. With the fix: orphaned **1.7 seconds** after the
revocation, marked dead at 6.2s, running on the arbiter at epoch 2, resumed from the checkpoint.

**Three things are worth carrying.**

1. **A test double that ignores a trait method cannot test the method.** `MemoryConnection::close`
   was a no-op — "dropping the sender is what the peer observes" — so the existing revocation test
   proved the session had been *forgotten*, not that anybody had hung up, and those two are the
   halves of a revocation. It could not have gone red. Making `close` real, including waking an
   `accept` already parked, is what made the new test possible at all.
2. **…and the test also asserted about the wrong node.** In
   `a_revoked_peer_stops_being_talked_to_rather_than_merely_being_marked` the desktop is the one
   dialling, so the only session is outbound and removing it from the map is enough. The bug lives
   entirely in the direction the test did not set up. When a rule is about two ends of something,
   write the test from both ends or it is a test about one.
3. **A restart hid it completely**, which is why five sessions of walks never met it: there is no
   inbound session to survive a restart, so a revoked peer is refused cleanly on both doors
   afterwards. Anything whose only symptom is on a *live* process has to be walked on one.

**The residual is that the fix produces two agents, and that is the right trade.** The fleet now
correctly evicts the revoked node and moves the run, while that node — which can hear nobody — runs
its old leg to the end. That is the partition ADR-0002 already accepts, made deliberate and
permanent, and leaving the device inside the mesh instead is strictly worse: it keeps gossip, lease
renewal, checkpoint push and blob access on every node it can dial, for as long as those nodes stay
up. What is new is that the revoked node is the one machine that *could* stop its own agent, and
does not: `Refusal::Revoked` reaches it on every dial it makes and `TransportError::Refused` is
matched nowhere, while `change.ourselves` — the path that *does* have a handler, and whose handler
is deliberately loud-and-nothing-more — needs the revocation to arrive by a gossip the hang-up has
just made impossible. The handler is on the path that cannot happen and the path that happens has
no handler.

## Session forty-one — the run that was left for a person nobody had told

One item, the handoff's first, and it had been the handoff's first for four sessions: **who picks
up a run that *failed* on a departing node**. ADR-0042 named it in its own closing paragraph and
predicted it would need a new home for the fact. It did not. It is ADR-0043, no wire bump and no
schema change, and it is the second session running whose fix is mostly a call to something that
already existed.

**What was wrong.** ADR-0034 found the fourth path `stop_accepting` had never reached —
auto-resume spawning agents on a drained laptop — and closed it by making `departing` the first
question `decide_recovery` asks and `Escalation::NodeIsDeparting` its whole answer. Right about the
agent, and it ended the question one step early: *this node will not start an agent* and *nobody
should* are two facts, and a drain is the one moment they come apart. The run is resumable,
unattended, ours, inside its budget, past its backoff — and the only objection is a machine that is
going away. What "left for a person" meant in practice was `Failed`, which `supervise` reads as
terminal, so no arbiter offers it and no node bids; the reason lived in memory and died with the
daemon. Measured: `offload drain` said *nothing to hand over*, the run explained itself by naming
the drain as the cause of something the drain cannot fix, both daemons were restarted, and it was
still failed with no reason on screen at all — its checkpoint replicated the whole time onto the
idle peer that would have taken it.

**What it is now.** `departing` **wraps** the run's own verdict instead of replacing it. `Resume`
and `Wait` become `Recovery::LetGo` — `Wait` too, because a backoff is a promise to try again in
thirty seconds and a leaving node has none to promise — and every other answer passes through
unchanged, since it is about the run and just as true of the next machine. The effect is a
*transition*, not a start: `Run::let_go` moves the run `Failed` → `Pending { let_go_by: me }`, and
from there every line of the placement is ADR-0042's, untouched. Gated on
`Checkpoint::is_durable`, because a run offered without its conversation is one the winner cannot
begin — `Escalation::NoCopyElsewhere` is the honest half, and it is what became of
`NodeIsDeparting`.

**Three things are worth carrying.**

1. **Ask what state the fact wants to be true in before inventing a place for it.** ADR-0042
   expected this decision to need its own home, since `let_go_by` lives inside `Pending` and a
   failed run is not pending. The premise was right and the conclusion was not: the run stops being
   failed. The same question that unlocked session forty — *what is the fact a property of* —
   asked one layer up.
2. **A guarantee written as a line order is a comment; written as a match arm it is a test.**
   ADR-0034's rule was "checked above everything", and reordering the function is exactly what this
   session did. `Resume` and `Wait` are now the only answers that spawn an agent, they leave one
   function, and the departing match is the one place that decides a leaving node may not hear
   them — swept by a property test over every shape of failed run against every `Circumstances`.
3. **The walk found two bugs the tests could not, and they are the same bug at two layers.** Both
   are "something that normally happens a moment later" on the one path where there is no later.
   `Mesh::drain` filters terminal runs out of `held`, so a node whose only run has failed returns on
   its first line and the recovery tick never fires — the sweep belongs to `depart`. And with the
   sweep in place, a `SIGTERM`ed node wrote `Pending` to its own store and exited while every peer
   went on holding the run as `Failed`: the stranding moved from the record to the network.
   `announce_departure` already carries `Gossip::runs`, so publishing into the view before `depart`
   returns is the whole fix — 265ms from the signal to the run placed on another machine.

**What it cost elsewhere.** `Drained` has a sixth number and `offload drain` a line for it, both
because the command said *nothing to hand over* about a pass that had just given a run to the
fleet — a failed run is not a *held* one, so every existing field was nought. `Recovered` replaces
the `Vec<Run>` that pass returned, since resuming a run here and giving one to somebody else are
two things to say and the caller says both.

**Then the second item: `GiveBack`, walked at last.** ADR-0006's other half — a node hands back a
commitment it can no longer honour in time — had been stamped by ADR-0042 and never once
exercised. Both branches now have timestamps.

The **happy path** had never been seen at all, and it is eleven milliseconds wide: `due and still
not started here; offering it to the fleet overdue_by=22.2s ready=1` at 09:21:15.675, `a late
commitment was handed over name=bravo starting=starting now` at .686, the agent spawning on bravo
at .716.

The **refused** path turns out to end well almost always, and the reason is worth knowing: the
take-back cannot fail for want of room, because a full node *commits* rather than declines
(ADR-0006). So `take_run` only fails when the row has moved on — in which case the run is
cancelled or held elsewhere, and not stranded. That is what "made rare rather than impossible by
the take-back path" means, concretely, and it leaves exactly one way in: **the daemon dying inside
the bid round.** Staged by pointing the round at a dead peer with `bid_window_ms = 10000` and
killing the daemon 65ms after it logged the offer. Restarted: `pending — waiting for a node since
15.3s ago; e3eda2fa let it go`, placed 56 seconds later with `offering=LetGo { by: alpha }`, and
running. With the stamp reverted to `None` on that one transition: `nobody holds it, and it was
not queued — nothing offers it to the fleet again on its own`, then two minutes on a node
reporting `runs 0/1 · accepting yes` with zero placements attempted.

**Two things the walk turned up that nothing was looking for.**

1. **A note about why this node has not started a run outlives the node's intention to.**
   `take_run` writes the refusal into `RunProgress::workspace`, which is the WORKSPACE column of
   `offload ps` and which **gossips**; nothing cleared it on release, so the stranded run sat
   `pending` in nobody's hands under `waiting for a slot`. Fixed in `release_unstarted`, the one
   place both callers pass through, and cleared rather than reworded — nothing ran, so there is no
   workspace to summarise. The test was checked by breaking the fix and watching it go red.
2. **`ready_elsewhere` read ability, and hosting needs permission.** Its three gates are
   liveness, capabilities and policy — every gate `NodeView` carries, and exactly the
   vocabulary's own split, with the fleet's `host-runs` grant a third thing that was not
   consulted. So the peer used to stage the refusal, which had joined and never been granted it,
   counted as `ready=1` for a run it could never take. Inside `review_commitment`'s stated
   tolerance, but that tolerance was written for a *stale* estimate that self-corrects, and this
   one was permanent: a release, a refused round and a take-back, two epoch bumps, every thirty
   seconds all night.

**And the correction that is the most useful thing in the session.** The finding above was first
written down as *not fixable from the reading end* — the view has no field for it, so the shape
would be a new gossiped fact needing an owner under ADR-0005. That was wrong, and what says so is
one function further on in the file that already enforces the rule: `Cluster::hosting_objection`
is ADR-0012's other half, and it works by reading the certificate **the peer presented on the
connection this node still holds**. Nothing is gossiped for that and nothing needs to be. The
estimate was looking at the view because its other three gates live there, and the fourth was
already local, twenty lines from the enforcement of it.

So `Cluster::peer_hosts_runs` sits beside `hosting_objection` with **one** `permits_hosting`
predicate under both — an estimate and an enforcement that cannot disagree, which is
`reports-and-cli`'s rule applied before the two had a chance to drift. No gossip, no wire bump,
no ADR. `None` is unknown and stays *counted*, which is the opposite of this codebase's usual
"unknown is not good news" and is right here because `review_commitment` states which way to be
wrong — so the change is strictly subtractive.

Measured: `keeping the commitment reason=no other node could start it either`, the run untouched
at epoch 1 where it used to flap every thirty seconds. And the regression that mattered more, with
the peer granted and past probation: offered at 08:03:18.825, `a late commitment was handed over`
at .831, spawning at .858 — six milliseconds, unchanged. A subtractive fix has one way to be
wrong, and it is to subtract too much; that is the pass worth walking. Probation falls out for
free: a node inside `PROBATION` has a certificate saying *granted but dormant*, so the estimate
now skips it instead of buying a doomed round every thirty seconds for fifteen minutes.

## Session forty — the fact the drain had nowhere to put

One item, the handoff's first, and it had been the handoff's first for three sessions: **`Pending`
plus a checkpoint is two facts with one spelling**, and the fix that needed building was a fact on
the run rather than a note in a process. It is ADR-0042, wire v24, and it retires ADR-0041's
mechanism while keeping everything ADR-0041 decided.

**What was wrong.** A run a person parks with `offload checkpoint` and a run a node lets go on its
way out reach the same state, with the same checkpoint beside them, and `offload_core::supervise`
read both as the first — correctly, for every way of reaching it except the one a drain produces.
Session thirty-nine fixed the half a live process could reach: a node-local set of runs the drain
still owed, re-offered on the backoff. It left three doors open and named them. A drained node
**restarted** before the fleet has room strands the run exactly as before, because the debt lives
beside the departure flag and a daemon that restarts has departed for real. `Lifespan::Exits` — a
`SIGTERM`, which is what closing a laptop looks like — recorded nothing at all, by construction: a
process on its way out cannot promise a later. And ADR-0041's own residual is the first of those
said from the other end.

**What it is now.** `RunState::Pending { since, let_go_by: Option<NodeId> }` — **inside the state**,
which is the whole of its merge story. The state is the holder's to write and rides on whichever
record `merge_run` settles on, so it inherits an owner and an arbiter rather than needing new ones;
and because every transition replaces the state, the fact cannot outlive what it is about. Two
handoffs had declined to build this on the reading that ADR-0005 meant a new field on `Run` plus a
new merge clause. The useful question turned out to be *what is the fact a property of* — and it is
a property of being pending, not of the run.

`GivenUp::{Parked, LetGo}` carries the answer from `Supervisor::request_checkpoint`, which is the
only moment that knows, to `Run::checkpointed`, a turn later. `LetGo` wins when both arrive, since
the node is leaving whatever else was typed at it — and making that reachable meant
`Run::request_checkpoint` becoming idempotent from `Checkpointing`, because it used to refuse the
second request and a drain reaching a run somebody had already parked was turned away at the door.

**Three things are worth carrying.**

1. **A fix can be finished by a deletion.** With the fact on the record, the run's *arbiter*
   offers it. Leaving `Mesh::owed` in place would have added a second offerer — the departing node
   — for the same run, which is the two-grants-at-one-epoch case `place`'s own comment says the
   epoch cannot settle. So the debt, `Owed`, `Unplaced`, `finish_owed_handovers` and the `owed`
   channel into `offload explain` all went, and nothing was lost: a draining node's `evaluate`
   already answers `draining`, which is the bid the owed pass passed by hand, and both offers were
   already on the same `may_retry` backoff. **When a new mechanism subsumes an old one, check
   whether keeping both is merely redundant or actively wrong.**
2. **The report was fixed by making the decision see, not by feeding the report.** Session
   thirty-nine handed `explain` the debt as an `Observed` field, which was right for that sentence
   and reached none of the others: the daemon logged `queued run placed` about a run nobody
   queued, and `Drained::later` covered two outcomes that had stopped waiting on the same thing.
   `Supervision::Place` now carries `Offering::{Queued, LetGo { by }}`, and the side channel could
   be deleted. `Drained` got its fifth number, `pooled`, which is the third time that struct has
   needed a split — a mid-turn run needs this process, a released one needs no daemon at all, and
   that is exactly what somebody about to close a laptop is asking.
3. **A defaulted field can still earn a wire bump, and the test is relaying rather than
   ignoring.** `Run::origin` did not need one: immutable, identical in every copy. `let_go_by` is
   mutable inside a state peers gossip, so an older build that drops it and wins an equal-epoch
   tiebreak re-states the run without it — silently restoring the exact bug. The safe-sounding
   default *is* the broken case here, so v24, `MIN_VERSION` with it.

**Measured**, one node, a fake agent, `drain_deadline_secs = 10`. Through the `SIGTERM` door on
the *prior build*, in a state dir of its own: released and refused 07:07:15, daemon exits 07:07:17,
restarted 07:07:17, still `pending` at 07:08:32 on an idle accepting node, with `offload explain`
saying it *waits for `offload resume`* about a run nobody parked. On the fix: released 07:05:11,
exits 07:05:13, restarted 07:05:18, `an unheld run was placed offering=LetGo { by: node-a }` at
07:05:19, resumed at turn 3. Through the `offload drain` door, with the decision alone reverted and
restored on the same daemon: reverted, restarted 07:03:45, still pending a minute later; fixed,
restarted 07:02:52, running at turn 5 one second later. `870 tests, clippy clean, wire v24, schema
v11`.

## Session thirty-nine — three runs that stopped and nothing said so

Three findings, three walks, one shape between them: **a piece of state that means two things, and
a reader that assumes the one it was written for.** None of them fails, errors, or logs a warning.
In all three the run simply stops, and every report about it says something reasonable.

**1. `live`'s entry answers three questions and the readers asked the map.** The handoff's first
item was the ADR-0005 question about `Supervisor::live`: who owns the fact that this node ran a leg
of this run, and when does it stop being true. The answer is that there are *three* facts in one
entry with three lifetimes — `cancel` is *an agent is going here*, dropped by `release` one step
before the terminal write; `live` is *this leg may still append*, dropped by `close_stream` after
the last line; `epoch` is *this node ran a leg numbered E*, and it stays until the run's row goes,
because `describes` has nothing else with which to tell a run that ended **here** from one that
ended elsewhere. It is node-local and gossiped nowhere, so there is nothing to arbitrate, and
losing it is normal: a restart empties the map and every reader already has the restart's answer.

Five readers asked the entry's *presence* instead, which answers all three at once and for as long
as the longest of them. The door in is `hold_here`, which does not touch `live`: a run granted back
to a node that had already run a leg of it — laptop, desktop, laptop — lands beside the old entry
and `awaits_a_slot` reads it as already started. Measured, same sequence both sides, one line
apart: fixed, the slot freed at 21:20:59 and the agent was running at 21:21:11, resumed at turn 17
from the checkpoint it left at 14; reverted, the slot freed at 21:32:53 and at 21:36:25 the run was
still `assigned` under a renewed lease on a node saying `runs 1/2 · accepting yes` — and the 1/2 is
the wedged run, holding the slot it will never use. `agent_here` for "has it started",
`stream_open` for "may a follower still get more".

**2. A refusal *now* was read as an answer about later, and the drain had already released the
run.** The handoff's second item said to ask of the other withdrawal-shaped operations what
session thirty-four asked of `stop_recovering`: where is one piece of state written by two callers
with different intentions? It is `Pending` plus a checkpoint. That is what `offload checkpoint`
leaves, and `offload_core::supervise` reads it as parked by a person and passes by (ADR-0014) — and
it is *also* what a drain leaves when it releases a run and nobody takes it. ADR-0041 had already
settled this collision for a run still mid-turn at the deadline, which becomes a debt and is
re-offered on the backoff; a run that reached its boundary **inside** the deadline was offered once
and dropped. The run that behaved better got the worse outcome. Measured with the peer down: drain
at 21:47:32, `1 run(s) still here`, `offload explain` describing the run as waiting for `offload
resume` — about one nobody had parked — and both daemons then restarted, idle, accepting, holding
the checkpoint, with the run still `pending` for as long as anybody watched. `unplaced` makes a
refusal the same debt a missed boundary is; handed over 23 seconds after the peer appeared.

**3. …and the report then sent somebody after the run the daemon was already handing over.** The
fix's own reporting half. `offload explain`'s verdict is `supervise`'s, deliberately, because a
second copy of a rule disagrees quietly on the day it matters — but the debt is in memory and
gossiped nowhere, so no pure function of the view can see it, and the sentence was left to infer
from the row. `Observed` is where a fact `supervise` cannot see is supposed to arrive, and
`departing` is the precedent one field back.

**Three things are worth carrying.**

1. **The unit test proved the fix; the walk proved the bug was reachable.** Finding 1 went red on a
   unit test in twenty minutes, and the claim that mattered — *this happens to people* — took an
   hour of two daemons and was worth every minute, because staging it turned out to need four
   things at once (a leg on A, a release, a second leg elsewhere, and a grant back while A is full)
   and two of the three attempts raced past the window. A bug that is hard to stage deliberately is
   not a bug that is rare; it is a bug nobody has been able to describe.
2. **Two daemons on one machine share an account and a device ledger**, so `max_concurrent_runs`
   is not the machine's limit and a peer's run makes this node refuse. Two passes were confusing
   before that was noticed. In `docs/DEMO.md` now.
3. **A fix that gives the daemon a new intention gives every report a new case.** Finding 3 exists
   only because finding 2 shipped, and it took ten minutes to find because the walk was still set
   up. Re-read the reports about a thing you have just taught the daemon to do; the arm that was
   right for two cases will not notice a third.

## Session thirty-eight — the entry that outlives the run, and the 37 KB it was holding

`Supervisor::release` does not release anything: it sets `entry.cancel = None` and leaves the
`live` entry, and nothing else in the crate removed one. Sessions thirty-six and thirty-seven each
closed a door into a stale entry *mattering*; what nobody had asked was whether the map should be
cleaned up at all.

### Why the entry is right to stay

Three readers keep it alive after the agent is gone, and all three are correct. `release` drops
only the handle, because `drive` calls it **before** the terminal write so a `cancel` or `resume`
arriving in between finds no agent — and the run's last log lines are appended after it.
`record_run` reads `cancel.is_some()` to tell a leg that is still driving from one that is not. And
`describes` asks whether a finished run ended *here*, which for a run with no holder is answerable
only from this leg's epoch, and that lives in the entry and nowhere else.

So the answer to "should it be cleaned up" is no. The answer to what it *holds* is different.

### What it was holding

`broadcast::channel(256)` allocates its ring when it is created. Measured in-process, with the
store deliberately kept out of the measurement and each case in its own process because allocator
reuse contaminates a shared one: **37,806 bytes per run**, identical whether or not anybody ever
followed it, against a control that removed the entry and kept nothing. Then measured on the
product, which is the number that matters — 100 one-turn runs at a fake agent, RSS read from the
daemon's own `statm`, the one line under test reverted with `sed` for the control: **9,836 KB and
9,868 KB** before, **7,420 KB and 7,060 KB** after. About **26 KB a run** reclaimed, reproducibly.

ADR-0020 is what turns that from a rounding error into a leak. A rule firing every three seconds is
1,200 runs an hour — session thirty-three measured exactly that cadence — and this daemon is meant
to run on a phone.

### The fix, in two halves

`close_stream` sets the channel to `None` at the end of `launch` rather than inside `drive`,
because it has to be after **every** way the leg can end, including the arm `drive` returns an
error to, where `fail` writes the run's last line. A follower still attached drains what is
buffered and then sees the close, which is what `offload logs -f` is waiting for. Two operator-
facing lines got more accurate as a side effect, both in `attendance_now`: a checkpointed run
released back to `Pending` said "unattended — nobody is streaming it" and now says "not running
here", and a superseded run held on another node said the same thing and now names the node.

`forget_legs` is the other half. A leg is a fact about a run, so it has no business outliving the
run's **row** — and `prune_spent_records`, ADR-0020's own pass on exactly the runs a node has
thousands of, deleted rows and dropped the list of what it deleted on the floor. It returns them
now. Safe by construction rather than by care: `describes` and `holds` both start from
`self.run(run_id)`, which is `None` for a row that is gone. `cancel_all` was tightened in the same
pass for the same reason read backwards — it iterated every key in the map, which is every run the
daemon has ever started.

### What the session is really about

The item was phrased as a naming question — should the map be cleaned up, or should the name stop
promising it. Both halves of that were wrong, and the way to find out was to read what each reader
of the map actually needs and then *measure* what the map costs. The entry is right; the ring is
not; and the answer was a number rather than an opinion.

## Session thirty-seven — the third refusal, and the one state where `holder()` lies

Session thirty-six's own residual, and it came with a warning attached: **measure before
enumerating**. That turned out to be the whole of the session.

### What the probe found

The item was one sentence — `hold_here`'s `assign` failure surfacing as `agent: cannot assign a run
that is running`, a `TransitionError` stringified into an operator's error message and logged under
`could not start it yet`. The warning was that getting it right needs the corners checked rather
than assumed. So the first thing written was not a fix but a probe: all four ways into that
decision, printed.

Three of the four were as expected. The fourth was not, and it was not an `assign` failure at all.
An **`Orphaned` row whose last holder was this node** never reaches `assign`, because `Run::holder`
answers `Some(last)` for that state — right for the question it is named after, wrong for the one
being asked. So it took the already-ours branch: `start_run` returned **`Ok`**, the row stayed
`orphaned`, the epoch went 1 → 1, and the run was registered as live. Everything downstream then
refuses it, because `fence` reads `state.lease()` and `Orphaned` has none — so `drive`'s pre-spawn
check reports a lost run, after a workspace prepare, to somebody who had not lost one. And the
`live` entry outlives it in the one case where that is not harmless: `started_excluding` gates on
`holder() == me`, and for this single state `holder()` says yes, so the slot stays spent and
`held_but_not_started` skips the run for ever. Last session's wedge, through the door `holder()`
opens.

The base is `state.lease()` now. An orphaned run then goes to `assign`, which is legal from
`Orphaned` and is the reclaim — the same thing this node does for a run *another* node last held,
because the last holder's identity is not authority. The guard drives both directions in one loop,
since they are one rule.

Two other rules already stood in front of this: `supervise` returns `HeldHere` rather than `Orphan`
for a run held locally, and `speaker_decides` refuses a peer's `Orphaned` about a run this node
holds a lease on. What is left is the epoch — a higher-epoch record is taken by `merge_run` before
either applies — so the reachable trace is a partition, not a keyboard. Worth stating rather than
overclaiming.

### …and the sentence turned out to be the wrong shape too

Fixing the wording by variant is the obvious move, and it is wrong the way taxonomies are wrong:
right about the cases somebody enumerated. `Ended` covered the terminal row last session; the probe
found two more, and there is no reason to think that is the last of them.

**What went wrong** is the error. **Whether the tick comes back** is the tick's own filter. Those
are two facts, and `Supervisor::awaits_a_slot` is now the three conditions `held_but_not_started`
selects on, written once and asked by both — `will_try_again` reports on exactly what the loop
selects on, so a failure nobody has thought of cannot be described wrongly by default. That is
sound only because of last session's fix: while a failed start could leave its `live` registration
behind, this predicate would have answered "nothing will try again" for *every* failure, which is
the wedge reporting itself as a decision. One fix made the other one possible.

`LostTheRun` is the sentence for a run that moved on. The pinned refusal gets a readable `Refused`
and no variant: `release` refuses to return a pinned run to the pool, `reassign` holds it with
`PinnedToHolder`, and a drain keeps it with `PinnedHere`, so nothing offers one to anybody — a
variant for a state the product cannot produce would be a taxonomy growing to cover a case rather
than a measurement.

### What was walked

The three guards, two of them checked red by breaking the fix. Then a short daemon pass — fake
agent, one slot, cluster on loopback — for the thing a unit test is worst at: that changing the
base of the branch **every start passes through** did not break the ordinary ones. Submission,
held at capacity (`accepted; holding it until it can start`), started by the tick (`there is room
now`), both completed clean. The three cases the fix is actually about need a partition or a
migration racing the tick, which is what the probe and the guards are for.

## Session thirty-six — two sentences that were guesses, and the wedge behind one of them

Picked up session thirty-five's own residual: `Supervisor::start_run`'s two remaining
operator-facing sentences, both seen in that session's walk and neither chased. They are small, and
one of them was standing in front of something that is not.

### The sentence that was a promise

`start_held_runs` matched every failure from `start_run` with one arm — `warn!(.., "could not start
it yet")`, under a comment saying the run is left `Assigned` deliberately because the next tick
tries again. For a run that ended while it waited, both halves are false: `held_but_not_started`
filters on `RunState::Assigned`, a terminal run holds no lease, and nothing ever looks at it again.
It is not a hypothetical arm either — it is exactly what session thirty-five's fix *produces*, since
making the cancel stick is what leaves the tick holding a snapshot of a run that is now over.

`SubmitError::Ended` is its own variant for the reason `LostTheRun`'s doc comment already argues:
the caller has to tell a refusal that will lift from one that never will, and no amount of reading a
formatted string does that. Two restraints in its shape. Its `Display` is character-for-character
the `Refused` it replaced, because nothing about a person's reading of the sentence changed — the
variant is for the caller, not the reader. And the level drops to `info`: a cancel that worked is
not a warning.

### …and the sentence that was left behind

A run held under ADR-0006 has a lease and an epoch and no worktree at all, so `take_run` writes the
*refusal* into the column that would otherwise describe a checkout. Nothing retracted it.
`Run::cancel` moves the state and touches no progress, so on a daemon `offload ps` showed
`cancelled` beside `waiting for a slot` for as long as the record existed — while `offload rm` on
that same run said there was no checkout at all. Two reports about one run disagreeing about whether
there had ever been a directory, and the column **gossips**, so on a real fleet that is everybody's
answer. `NEVER_STARTED` on the branch that already knew — `cancel_run`'s `assigned` arm, whose note
is "it had not started, so nothing was interrupted" — and deliberately not on the others.

### The wedge the first sentence was standing in front of

Asking *what makes "the next tick tries again" true* is what turned a wording fix into a defect.
`start_run` calls `register` before anything that can fail, and it has to: the registration is what
a concurrent `cancel_run` finds. What installs the cleanup is `launch`, at the end — so both early
returns between them leave the entry behind, and one of them is `ensure_blobs`, the ordinary way in
for a migration. Nothing removes a `live` entry anywhere else in the crate; `grep` finds two hits
and both are tests.

Measured on the cheapest reachable version — a held run with a checkpoint and no fleet to ask: the
row stayed `assigned`, the entry stayed in `live`, and `held_but_not_started` came back **empty**.
Two filters read that map and both bite. The tick never returns to the run, and
`started_excluding` counts live-or-`Running` for a run this node holds, so the slot it is not using
stays spent. A one-slot node that meets one unreachable transcript stops hosting for good.

The half that is worse is the half the fix has to be careful about. `halt_agent` takes the cancel
sender, sends, and `cancel_run` answers **"its agent was stopped" while writing nothing** —
deliberately, because the writer is `drive`. If the start then fails, `drive` never runs: hand the
registration back and the halt goes with it, the row stays `assigned`, the operator has been told
the run stopped, and the next tick starts it. `Supervisor::unregister` takes the entry and the
sender under one lock, which leaves two outcomes and no third — the sender is still here, so no
cancel can ever be accepted, or somebody already has it, so their message is on its way and the
wait for it is bounded by their `send`. `Halt::Superseded` is not written down: that run is somebody
else's and the record is theirs.

### What was walked, and what was not

Four guards, three of them red first and the fourth checked red by disabling one line of the fix.
Then a daemon with a fake agent, one slot, cluster enabled on loopback so a fleet of one holds a
real bid round — no model spend. A queued run held (`waiting for a slot`), cancelled while held
(`cancelled` / `never started`, no worktree directory on disk, and `offload rm` agreeing there is no
checkout), and then the control: a *running* run cancelled a minute later came out `cancelled` /
`clean`. Plus the ordinary arm intact — `there is room now; starting the run held for it` →
`completed`, `clean`.

What was **not** walked, deliberately: the wedge itself. Reaching it on daemons needs a migration
whose transcript cannot be fetched, and the transcript is replicated to the successor precisely
because migration needs it — so the daemon version would be an arranged fleet state, which is the
thing a trait stand-in does better and more precisely. That is session thirty-five's own second
lesson used rather than restated. And the `Ended` log line has no daemon reproduction at all: it
needs the snapshot race, which is what the gated `Peers::fetch` exists for.

## Session thirty-five — the choke point every start passes through, and a cancel that came back as a failure

One finding, which is the lead session thirty-four named and deliberately left:
`Supervisor::start_run`. It did `run.assign(..)` on the copy it was handed and then a whole-row
`save_run`.

### The half that was expected, and the half that reproduced

The expected half is `resume`'s bug verbatim — two callers loading before either saves both compute
`old+1`, and no fence can separate two legs born equal. It is real, and it is not what the
measurement found; the audit had already predicted a tripwire that does not trip, because
`start_run` registers into `live` before its first `await` and `held_but_not_started` filters on
`live`.

What reproduced is not about the epoch at all. **A whole-row write does not overrule what landed in
between — it erases it**, and the handed copy is as old as whatever its caller did on the way here.
`start_held_runs` snapshots the runs it will start and then works through them, so every copy after
the first is as old as the run ahead of it took to start. `start_run` has exactly one `await`:
`ensure_blobs`, a network fetch of a transcript, which is to say every migration.

Measured through the real loop with that fetch gated, a held run cancelled inside the window came
back **`Failed`** — not `cancelled`, and not "the cancel was refused". The row was restored to
`Assigned`, the agent was launched, and it failed on its own. `Failed` is the state auto-resume
picks up, so a run somebody deliberately stopped was left *resumable*. The `started()` fence one
function down cannot catch this: it compares the epoch against the row, and the row is the thing
that was clobbered back into agreement.

### The two things that made it not a copy of `resume`'s fix

Both were named in advance by the audit and both were real. A fresh submission has **no row** —
`submit` calls `build` and then `start_run` — so `Store::update_run`, which reads first and gives up
when there is nothing, cannot express the transition at all. And `save_run` writes the whole row, so
the grant path's semantics had to be preserved deliberately rather than inherited.

`Store::upsert_run` is the first: `update_run` plus the row that does not exist yet. It hands the
closure whatever is stored, `None` when nothing is, and writes what comes back; an error writes
nothing, which is why the closure receives the stored copy rather than a mutable handle on the
caller's.

`Supervisor::hold_here` is the second, and it carries the rule that is easy to get backwards: **the
row decides, except where the caller is carrying news.** The base is the stored row — the only copy
that has seen everything that happened while the caller held its own — *unless* the row is at a
**lower epoch**, which is exactly what a grant looks like: the arbiter's decision travelling against
this node's memory of the leg before it. Basing on the row unconditionally refuses migrations and
drops the checkpoint they carry, which is the whole of what makes a migration one. `take_run`'s
accept-without-starting goes through the same function, being the same whole-row write of a copy
that crossed a network.

### The tests, and what made them worth trusting

Two, and each is the other's control. The cancel test gates `Peers::fetch` — a trait method, so a
stand-in can signal it has been entered and then wait for the test to release it, which *arranges*
the race exactly on a current-thread runtime with the whole real loop still running. That is the
cheaper cousin of last session's widen-the-window trick, and it applies wherever the seam is a
trait rather than a subprocess.

The grant test is the one that catches the wrong half of the rule: it goes red with the lower-epoch
arm disabled, which was checked by disabling it. "The row decides" is half a rule and is wrong for a
grant, so the fix needed the control that proves the preference rather than only the one the bug was
about.

### Walked on a daemon, because it is the transition every start takes

A fake agent, no model spend, one node. All three arms of the base choice end to end: a submission
with no row; a `--queue`d run placed by the bid round, whose grant arrives one epoch ahead of the
local `Pending` row; and a run accepted at capacity, held at epoch 1, then started by
`start_held_runs` when the run ahead of it finished. Plus the operator's case — a held run cancelled
stays `cancelled`, gets no worktree, and is never started.

Two demo traps were paid for and written down: a fleet of one with the cluster **disabled** cannot
demonstrate a held run at all, because accept-without-starting is reached only through a grant; and
a machine busy with `cargo test` refuses every run on cpu load, which reads exactly like a broken
fix.

### What the search learned

The await-audit's question has a second half. *Where is there an `await` between a decision and its
effect* found session thirty-four's three. This one needed **and how old is the copy the caller
handed in** — the `await` that opened the window was in a different iteration of the *caller's*
loop, not between this function's own read and write. A snapshot passed down is a read, and it is as
old as everything done since.

## Session thirty-four — three seams, and a guard on the wrong side of an await each time

Three findings. The first came from the search the handoff carried; the second and third came from
widening it by one word — not *what does a person and a machine both call*, but **what do they both
write**. All three are the same defect: a guard read on one side of an `await` and acted on from the
other, where the gap is a git subprocess with a floor of milliseconds.

### The checkout sweep asked git, then deleted what it had decided about 3ms ago

The unwalked candidate the list named was `offload rm` against the collector, and the shared state
turned out to be the *checkout* rather than the blob: `offload rm` reaches `Workspaces::remove`
through `Supervisor::cleanup`, and the fifteen-minute disk tick reaches it through
`reclaim_departed_checkouts`.

The gap is not in the guards, which are good and which ADR-0023 enumerates five of. It is in
*where* they are. The four about the run were read at the top of the loop body; then
`holds_uncommitted` was awaited — `git status`, a real subprocess, **3.0ms median warm and 24ms with
20k untracked files** — and only then was the directory removed. `Supervisor::resume` makes a run
held-and-launching with two **synchronous** calls, `update_run` then `register`, so that transition
is atomic with respect to the sweep and fits entirely inside its window.

**40 of 40.** Four worker threads, the racer one millisecond into the `git status`, and the sweep
deleted the checkout every single time. The interesting contrast is with last session's fix to
`resume` itself, which was structural against a tripwire that never tripped in 200 attempts: same
class of defect, opposite ends of the scale. CLAUDE.md's rule — *when a guard and the thing it
guards are separated by an `await`, the guard is on the wrong side of it* — turns out to have a
case where the wrong side is the only side.

The blast radius is wider than the failed run the test races. The first door is `moved_on`, and a
run parked **`Pending`** here after a migration satisfies it — no lease, and its last position came
from the other leg. That is the ordinary state of every run this node has handed on, so the racer is
not only the recovery tick on its 5s–5min backoff against a 15-minute sweep. It is a person typing
`offload resume`, which has no schedule at all. `Supervisor::reclaimable` is asked twice now, and
the second answer, with nothing between it and `remove`, is the one that decides.

### …and `offload rm` was signing its notes with the other machine's name

Widening the search to *what do they both write* pointed straight at `RunProgress::workspace`, whose
writers are a turn-boundary refresh, two notes before a run starts, and `cleanup` — which is
`offload rm`. Session seventeen already fixed the loud half of this: a node with **no** checkout
saying "removed" about another machine's disk. The fix was to note only where `remove` actually
returned `Removed`.

It does not reach a node that ran an *earlier* leg. Nothing removes a worktree when a run leaves —
ADR-0023's whole premise — so the checkout is really there, the removal really happens, and the note
really is this node's to make. What it is not is the *winning leg's* to make, and that is what it
was making it as. `Supervisor::writing_leg` answers `None` when this node has no `live` entry and
does not `describe` the run, and `None` was documented as leaving whatever stamp the row carried —
correct for the case it names, a run that ended here whose `live` entry a restart took. After a
restart `live` is empty for runs that ended **elsewhere** too, and the stamp on those rows is the
peer's, absorbed by gossip. **One return value, two situations, opposite meanings** — which is the
previous session's lesson (one flag with two owners) one level down, at a value rather than a flag.

The write was not inert. `update_stats` set `stats.at = now()` unconditionally, `at` is the second
component of `RunProgress::position`, and `superseded_by` at equal epoch and equal author falls
through to comparing positions. Touching the row was enough to beat the peer's own copy of it.
Measured in a fixture that hands the row to the peer the way gossip does: the peer's `3 modified`
became `removed`, about a worktree still sitting on its disk.

### …and a rule fired a second occurrence while the first was coming back to life

The third came from the same widening, pointed at `rules.last_run`. `fire` asks
`rule_run_in_flight` — ADR-0020 §3, *is this rule's last occurrence still going?* — and then awaits
`reclaim_occurrence(previous)` before submitting the next one: `git status` at 3.0ms plus, on a
clean checkout, `git worktree remove --force` and a `prune` at 4.1ms. So the guard was taken **7ms
or more** before the submission it authorises.

Which racer survives is the interesting half. ADR-0027 put `stop_recovering(previous)` immediately
above the reclaim so the *machine* cannot revive the occurrence, and that ordering is right and
load-bearing — once the recovery entry is gone the tick cannot decide to resume. It does nothing
about the other caller. A person typing `offload resume <previous>` has no schedule to fence and no
entry to withdraw, and `Supervisor::resume` takes a `Failed` run by name. `rule_run_in_flight` reads
the `terminal` column, and one `reopen` inside the window is the whole of it.

Measured with `tokio::join!`, so the revival lands where the firing actually yields rather than on
thread timing: the firing submitted a second occurrence **and made it the rule's `last_run`** —
worse than a stray run, because the rule's own pointer then names the new occurrence and the revived
one falls outside the rule's bookkeeping entirely. `in_flight` is asked twice now, with the
drop-recording inside it so the two calls cannot disagree about what the operator is told.

The bigger claim this nearly became is worth recording as a non-finding: an occurrence placed on a
**peer** looks like a deterministic version of the same bug, since recovery is node-local and never
gossiped, so the rule's node cannot call `stop_recovering` on it at all. That is already decided and
built — `standing_to_retry` answers `MachineStartedElsewhere` for an occurrence with no local rule
tag, and `decide_recovery` escalates it as `NotOursToRetry` above every other question (ADR-0030).
Checking it before claiming it was the difference between a finding and an embarrassment.

**Three things are worth carrying.**

1. **A search is worth re-pointing before it is worth replacing.** *What does a person and a machine
   both call?* has now paid three times; changing one word to *write* paid twice more in one
   session, and found defects no amount of call-graph reading would have — in both cases the two
   writers share no function at all.
2. **Ask what sits between the guard and the effect, and put a stopwatch on it.** Every guard in all
   three findings was correct, documented and individually well-reasoned; the defect each time was
   the *distance* between reading it and acting on it, and each time that distance was a subprocess
   — a floor rather than a distribution. Timing `git status` and `git worktree remove` before
   writing any test is what made a 1ms racer the right fixture instead of a hopeful one. **Two of
   the three gaps are the same two git calls**, which suggests the search worth running next is not
   about callers at all: find every `.await` between a decision and its effect, and ask what its
   floor is.
3. **All three fixes needed a control, and for the same reason.** A `reclaimable` that answered
   `None` for any unrelated reason would pass forty raced attempts by never removing anything; a
   rule about whose stamp is on a row is one line from silently discarding every note the system
   makes; a firing that never submits anything passes every assertion about not submitting twice.
   Each test asserts the *positive* case beside the negative one.
4. **Check the bigger version of the claim before making it.** The peer-hosted occurrence looked
   like a deterministic form of the third bug and was settled by ADR-0030 two years of decisions
   ago. The narrow finding survived; the sweeping one would not have.

### The guard test took three goes, and the two failures are the useful part

The sweep's guard test was committed flaky and had to be fixed, which is worth recording in full
because both wrong versions looked right.

The racing form that *found* the defect — a 1ms pickup against the 3ms `git status` of a two-file
fixture, forty attempts — measured 40/40 before the fix and passed 10/10 alone and 24/24 under
synthetic CPU load afterwards. It then went red **2 times in 8 full-suite runs**: under real
parallelism the sleep can outlast the subprocess, and the sweep removes the checkout *correctly*.
Failing for the right reason does not make it less flaky, and flaky guards get deleted. Isolated
repetition and synthetic load both said it was fine; only running the whole suite repeatedly found
it.

Replacing the race with a purely deterministic test was **worse, and it passed** — asserting
`reclaimable` before the pickup and after it stays green *with the fix removed*, because the defect
is that state changes inside the loop and a test that changes it beforehand only ever exercises the
first call. That was caught by deleting the fix and re-running, which is two minutes and is the only
thing that separates a guard from a decoration.

The answer was to widen the window rather than abandon the race. `git status` re-hashes a file whose
mtime has moved and still reports the worktree **clean**, so rewriting a 16 MB file with identical
bytes before each attempt buys **83ms** against the 1ms pickup instead of 3ms. Red with the fix
removed, green across nine full-suite runs, three under full CPU load.

### The await-audit, run at the end, and what it says

The three findings all being the same defect made the next search obvious, so it was run before the
session ended rather than left as advice. Every named candidate came back **clean**, and three of
them are clean because an earlier session already applied exactly this fix: `replicate` re-reads the
row inside `update_run` after its network await and checks the checkpoint is still current; the
agent-start path is re-fenced by `run.started(node, epoch)` inside `update_run`, whose failure branch
exists to stop a second agent; `collect_garbage` is covered by an hour of grace that both `put` and
`get` refresh, which kills the plausible-sounding theory that a dedup hit leaves a stale timestamp;
and the delivery pass guards on `is_available()`, which is `Alive` only, re-read every tick.

That is a useful result on its own — five things the next session should not re-walk — and it left
one live lead, which is recorded rather than half-fixed: **`Supervisor::start_run` is session
thirty-three's `resume` bug one function over**, doing `assign` on the copy it was handed followed by
a whole-row `save_run`. It is the choke point every started run passes through, and it is delicate in
two ways `resume` was not: for a fresh submission the row does not exist yet, so `update_run` alone
cannot express the transition, and `save_run` writes the whole row, so the grant path's semantics
have to be preserved deliberately. Fixing it hastily at the end of a session, with no daemon walk
available, would have been worse than naming it precisely.

Three things were checked rather than assumed, and all came back clean. `reclaim_occurrence`, the
trigger path to the checkout removal, goes through `cleanup`, which reloads the run and refuses a
non-terminal one — that reload *is* the re-check, so it never had the first bug. Every writer of
*spend* — cost, denials, asks, tokens, turns — runs while an agent is live here, so `writing_leg`
answers `Some` for all of them and the second fix cannot touch them. And ADR-0030 already covers the
peer-hosted occurrence the third finding nearly over-claimed.

## Session thirty-three — the seam the last session named, and what it was hiding

Session thirty-two ended by naming its own successor: look for two things built close together
whose *assumptions* touch, and walk the pair. Its strongest candidate was **a checkpoint request
and a drain arriving at the same run** — `checkpoint_requested` read-without-clearing (session
thirty-one) beside `mesh::depart` waiting on a boundary with its own deadline (ADR-0035). Both
correct alone, and nobody had looked between them.

The seam turned out not to need two commands at all. **A drain arms the same flag `offload
checkpoint` does**, and that flag is a standing instruction until honoured — so the drain's own
request outlives the drain's own deadline. A run still mid-turn when the deadline expires is
reported as still here, and then, twenty seconds later, finishes its turn, captures, and *releases
itself* into the pool. Nothing offers it. `offload_core::supervise` reads a checkpointed `Pending`
run as one a person parked with `offload checkpoint` — and says so in its own comment — which is
true of every way a run reaches that state except this one.

The measurement is the whole argument, and it is one `offload explain`:

```
state       pending — waiting for a node since 1m19s ago
            nobody holds it, so it is waiting for a bid round rather than for a hold-down
checkpoint  turn 1, copied to node-b
  → node-b       bid 57, starting now
```

The checkpoint was already replicated to the peer. The peer was bidding to take it. The run says
it is waiting for a bid round. The round is never held, and a drained laptop is left holding a
stopped run that every other machine in the fleet was willing to continue — which is the exact
opposite of what the person typed `offload drain` to achieve.

ADR-0041 fixes it by deciding the thing neither ADR-0034 nor ADR-0035 had cause to ask: **a
drain's deadline bounds how long somebody waits, not what the node intends.** The drain keeps what
it could not finish (`Mesh::owed`) and settles it at the boundary it could not wait for. Three
passes out of three afterwards, handed over nineteen seconds after the drain returned.

**Three things are worth carrying.**

1. **The seam was one flag with two owners, not two components.** The last session's advice was to
   look for two *things* whose assumptions touch; what this one found was one piece of state
   written by two callers with different intentions, where the reader could not tell them apart.
   A machine's request and a person's request look identical at the point of use, and only one of
   them has somebody who will come back for it. That is a cheaper thing to search for than a pair
   of components, and there are more of them.
2. **Only one of `depart`'s two callers can promise anything about later**, and the tempting way to
   tell them apart was already sitting there: the shutdown path passes `Report::silent()`. Using
   that would have worked today and been wrong — "nobody is listening" and "this process is about
   to stop existing" are two facts that coincide, and inferring one from the other is precisely how
   ADR-0035 §3 records the departure flag landing in the wrong place twice. `Lifespan` is a
   parameter for that reason and no other.
3. **The guard caught the fix's own bug, because the decision was a pure function.** `owed`'s
   first version asked `run.holder().is_some()` for "has somebody else taken this" — which answers
   *still ours, mid-turn* identically, and that is the state the debt is recorded in. It went red
   on the first run. It could only be a unit test because the decision takes a run record and
   nothing else; the pass that acts on it needs a `Mesh`, a `Cluster` and a peer to talk to, and
   the same mistake folded into that pass would have been findable only by walking.

**A second finding, from walking the first one to its reports.** `offload explain`'s `Unheld`
verdict said *"waiting for a bid round"* — which is what `Supervision::Place` does, on the arm that
is the `else` of that test. It reads the right function, which is the rule this project keeps, and
then described the wrong branch of it; the sentence was inherited from `Bystanding::Unheld`'s own
doc comment in `offload-core`, true when it was written and given away to the sibling branch by
ADR-0014's queued runs. Its guard asserted the wrong string, so the test held the mistake in place
rather than catching it. Both the CLI string and the core doc comment are fixed, and the test now
asserts the claim its own comment always described.

Also: `left` was two outcomes wearing one number, which is ADR-0035 §3's finding one bucket further
on. `later` is now beside it, and the drain says what will happen instead of pointing at two
commands that cannot answer.

**Then the fix was walked, and it found two more things — one of them mine, an hour old.** The
handoff had named `offload cancel` arriving at an owed run as a candidate; it took four minutes and
the report was wrong. `Owed::Done` tested `is_terminal()` and then said what had happened, and
`is_terminal()` is three outcomes — so a run somebody had just **cancelled** was logged
`why="it finished here first"`, and so was one whose agent died. That line is the only record of
what became of a run the drain *promised* to hand over, which makes it the worst place available
to guess. Same rule as `Boundary`'s four answers and `Drained`'s fourth count, arrived at from a
third direction, in code written by somebody who had just written both of those sentences down.

**The second was not mine and was worse.** Walking the same seam with a run that *fails* on the
drained node: `stop_accepting` is checked at the bid and at the grant, and auto-resume asks
neither. Measured — drain returned 11:42:50, the run failed 11:43:23, and at 11:43:54 the same
node logged `nobody was watching; resuming it` and `spawning claude code`, while its own `offload
status` said `accepting no — drained` and a peer holding a replica of the checkpoint sat idle. A
laptop somebody closed, starting a fresh agent ninety seconds later. The seventh lie this command
has told, and the first that was never true rather than broken by the fix to the last one.
`Circumstances.departing` is checked above every other answer, because the rest ask whether a
retry would work and this asks whether this is the machine to try it on.

**Two things about walking, from the second one.** The first attempt proved nothing and looked
like it had: the run died in turn 1, before any boundary, so recovery refused it for an unrelated
reason — "it has no checkpointed conversation to continue" — and the node's restraint was an
accident of the fixture rather than the behaviour under test. A walk that produces the answer you
wanted, for a reason you did not choose, is not a walk. And the signature growing to eight
positional arguments — four of them numbers with a new `bool` among them — was caught by clippy
rather than by judgement; `#[allow]` has precedent here and would have been the wrong trade on the
one pure decision function the simulation tests drive.

**The session's last item found nothing, which took a walk to establish.** `--max-turns` under a
migration had been the handoff's strongest candidate for two sessions. Reading the code predicts a
serious defect — the agent's parser counts each process from one, `record_event` *assigns*
`stats.turns = turn`, and `absorb` takes the winning leg's count outright rather than a `max` — so
the limit should hand a run its whole budget again on every migration. It does not: `drive`
computes a `turn_offset` from the checkpoint and `accumulate_turns` shifts every turn number
*before* the pump hands the event to anything, so the writer is assigning an already-cumulative
number. Walked to be sure: a run capped at 3, drained mid-turn-2, was stopped by node-b on its
first boundary — `turn=3 limit=3`, twenty-two milliseconds after `spawning claude code` — captured
at the limit and replicated it, showed `3/3` in `ps`, and refused `offload resume` from both
nodes.

Written down as a pitfall pointing the *other* way, because the failure available here is somebody
finding it by reading and reaching for a `max` in `absorb` — the one change that would break the
leg-ownership rule ADR-0005 sets. Three places touch this number and the one that makes it
cumulative is not either of the two you would check.

**Then the search from the top of this entry was run on purpose, and it found one.** "One piece of
state written by two callers with different intentions" generalises to "what does a person and a
machine both *call*", and the answer is `Supervisor::resume`: an operator typing `offload resume`,
and the recovery tick picking an unattended failure back up. It loaded the run, checked its state,
spent two full `list_runs` scans on the capacity check, then assigned and saved. `Run::assign`
bumps the epoch it read — so two callers landing in that window both compute `old+1`, both save,
and both launch an agent into the same worktree.

That last part is why it matters more than an ordinary lost update: it defeats the mechanism built
to catch it. Fencing separates a *stale* leg from a current one, and these two legs are **equal** —
every later `holds`, `describes` and epoch check passes for both. `Store::update_run` exists for
exactly this and its doc comment says so; `resume` was the load-modify-save it warns about, in the
one place whose failure is two agents committing to one repo.

**The tripwire did not trip.** Two hundred aligned attempts on a barrier, four worker threads,
before the fix: not one lost update — the loser's `load_run` landed after the winner's `save_run`
every time and was refused with "it is assigned, so somebody is already holding it". A photo finish
this machine happens to win. So the fix is structural (read, decide and write under one lock)
rather than an argument about timing, and the test is kept as an invariant check with a comment
saying plainly that it passed before the change too. A guard that cannot fail today is worth
keeping and worth labelling; it is not evidence, and writing it up as though it were would be the
same mistake as trusting a doc comment.

853 tests, clippy clean, wire and schema unmoved.

## Session thirty-two — the seam between two things built in one session

Session thirty-one built two things hours apart and walked each of them. It did not walk them
**against each other**, and that is where the defect was. The guard that refuses a capture whose
transcript holds no conversation is safe because "a failed capture is not a failed run — the next
boundary tries again". Every caller but one. ADR-0039's turn limit captures at the boundary it
*stops* on, so there is no next boundary, and the refusal that is harmless everywhere else is
permanent exactly where the work matters most — on the run somebody deliberately capped, which §4
captures unconditionally on the grounds that it is "precisely the run they want to look at".

Measured with a real agent at `--max-turns 1`: **two of three runs ended with no checkpoint at
all.** The evidence that says the whole thing is one `offload ps` row — `TOKENS 30` beside
`SAFE -`. The same transcript, read twice at two different moments, disagreeing about whether the
conversation was in it: `note_tokens` reads it after the agent is down and got the numbers, the
capture read it while the agent was alive and got nothing. Session thirty-one built that late read
*for the reporting column* and left the checkpoint on the early one — and wrote the sentence that
makes the consequence certain into `note_tokens`' own doc comment, where it has sat since: "the
run it is emptiest for is the short one, which is exactly the run somebody caps."

**The first fix was wrong, and measuring is the only reason that is known.** It stopped the agent
and then captured, borrowing `note_tokens`' premise that the file is final once the process is
gone. True, and not sufficient: a `SIGTERM` landing before the agent has flushed the turn it just
spoke loses that message *outright* rather than writing it on the way out — the failing transcripts
are still exactly 24,188 bytes with zero assistant rows now, long after their processes exited.
Final, and empty. That version measured 3/4, which is the shape of improvement that reads as a fix
and is not one. The order has to be **wait, then stop, then capture**: after the stop, nothing more
is coming. 8/8 afterwards, with the three-second timeout never firing and the flush landing
sub-second every time. This is the third session running in which a guess written into a doc
comment was contradicted by the first measurement taken against it.

The failure is also **binary rather than gradual**, which the earlier sessions' framing of the lag
as "about a turn behind" does not suggest: every transcript that had not caught up was *exactly*
24,188 bytes with no assistant rows, every one that had was ~30KB with two or three.

**The report half was a separate fact and shipped separately.** `LogKind::CaptureFailed` carries
the sentence "the run continues; the next turn boundary tries again", printed unconditionally
because there was one caller when it was written. ADR-0039 added the second, so a capped run
printed a promise of a retry one line above the line saying it had ended. The fact now sits on the
variant (`run_continues`) rather than in the renderer, because the caller is the only thing that
knows it — and it is `#[serde(default)]` to `true`, so no wire bump, which is ADR-0040's shape and
not a new argument.

Two habits are worth carrying out of this. **Walk the last session's two changes against each
other, not only each against itself** — the previous corollary was "walk the last session's fix,
not only the thing it fixed", and this is the same lesson one step further out: the seam between
two correct halves is where nobody is looking, because each half has already been walked. And
**a fix measured once is not measured**: 3/4 and 8/8 are different claims, and only running it
several times told them apart.

## Session thirty-one — the turn limit, and two things found by walking it

**`RunSpec::max_turns` is enforced (ADR-0039)**, seven phases after phase 1 recorded it as a known
gap and one session after session thirty found it still inert — declared, initialised to `None` in
eight fixtures, gossiped on every record, and read by no production path. The third field of that
shape after `WorkPolicy::allowed_agents` and `bid_delay`, and the roadmap's instruction was
enforce-or-delete. Re-measured first, because the phase-1 note turned on it: **there is still no
`--max-turns` in claude 2.1.251**, so the supervisor counts boundaries itself.

What made it more than a boundary check is where else the limit had to be asked. Reaching it writes
the run `Failed` — and `Failed` is exactly the input `decide_recovery` turns *back* into a running
agent, so a cap enforced only where it is reached is one the auto-resume undoes a backoff later,
unattended, for as many resumes as the policy allows. So `Escalation::TurnLimitReached` sits
**above attendance**, for `NotOursToRetry`'s reason: `Unattended` is the branch that resumes, and a
run that quietly spent its budget overnight is unattended by construction. The third point is
`resume` itself, because a failed run's own sentence offers `offload resume` by name. Three
enforcement points, one predicate returning the *limit* rather than a `bool` so the three sentences
cannot disagree about the number. The state is `Failed` and deliberately not `Pending` — a released
checkpoint hands the run to the next bidder, who carries on past the limit, which is the cap
applied one machine at a time and never to the run — nor `Completed`, which reads as success. Zero
is refused in the *type* (`Option<NonZeroU32>`), which is `allowed_agents`' empty list again: a
rule stored months ago is fired by a build that never saw the CLI's parser.

**Then the walk, and both of the session's other findings were in it rather than in the code.** The
first was in the change itself and cost nothing but reading `--help`: the new flag's doc comment
had been inserted *inside* `--use`'s, so `--max-turns` printed both flags' help and `--use` printed
none — the kind of thing a passing test suite says nothing about. The other two were real and
pre-existing. **A refused resume cleared the run's recovery entry**, because `stop_recovering` ran
before `Supervisor::resume` rather than after it, and it removes the entry outright — so the run
left the recovery bookkeeping permanently: no `recovery` line in `offload explain`, and the tick
never revisited it because the tick iterates that map. Found because a turn-limited run is the one
run that can *never* be resumed, so every attempt hit the refusal; but the same erasure was already
reachable through "its agent is still shutting down here; try again in a moment", which asks the
operator to repeat the request that does the damage. And **`explain`'s attendance line asserted a
cause that was no longer the cause**: `attended when it failed — which is what decided`, printed one
line above `left for a person: it reached its turn limit of 1`. True for as long as attendance was
the only thing that *could* decide, and this ADR added the first reason that outranks it. It now
lists the escalations that are about attendance, so a reason added later has to claim that
deliberately rather than inherit it by being unlisted.

**The residual, stated rather than papered over:** a capped run reports **no cost**. `cost_micro_usd`
comes only from the agent's final `result` event and a stopped agent never emits one — so `offload
ps` shows `-` in the column the operator capped the run to watch. Pre-existing and wider than this
(a released checkpoint, a drain and a cancel all end without a result), but a turn limit makes it
certain rather than incidental, and it is now written into phase 7's cost-accounting item as the
symptom that item never had. The second residual is that `max_turns` cannot be raised: it is fixed
at submission, so a spent run's only way forward is a new one, and a third `SpecEdit` field is an
argument to make on its own rather than because the field just built wants it.

Walked on one daemon with **Haiku 4.5** on the `~/.claude-alt` account, three runs, a few cents.
`843 tests, clippy clean, wire v23, schema v10` — no wire or schema change, because `max_turns` has
been on the gossiped `RunSpec` since phase 1 and only ever held `None`.

**Then the residual was taken as far as a decision, and the decision's most useful output was
finding this session's own note wrong.** The pick-up list had said the fix for a capped run's
missing cost was "per-turn accounting from each assistant message's `usage`" — reasoned, written
down with confidence, and **measurably false in two independent ways**. Measured against claude
2.1.251 on two legs of one real conversation: the **streamed** assistant `usage` is a partial
snapshot with `stop_reason: null`, reporting `output_tokens` of **1, 2, 4** where the finals were
**236, 85, 42** — a fifty-fold undercount, in the direction that looks harmless; and one assistant
message is emitted **once per content block** sharing a single `message.id` and a single `usage`,
so summing rows gives **726** against a true **363**. The next session would have built both bugs
from a note this session wrote. That is the whole argument for measuring, made against its own
work, four hours later.

What the measurement gave in exchange is ADR-0040 and one piece of reassurance about shipped code.
The **transcript** carries the final numbers, and deduplicated by `message.id` it reconciles
**exactly** with the sum of every leg's `result` event on all four counters — input 44, output 517,
cache-creation 9,474, cache-read 104,639 — across a fresh leg and a `--resume`. And each `result`'s
`total_cost_usd` is **that leg's alone** ($0.026998, then $0.006026 on the resume), which is why
`Supervisor`'s `saturating_add` per leg is correct: a load-bearing assumption about somebody else's
wire format that had never been checked, and that would have silently doubled every migrated run's
cost had it gone the other way. So the two sources have **opposite merge rules** — a result's cost
adds, a transcript total replaces — which is ADR-0005's one-field-two-facts shape again and the
reason ADR-0040 gives tokens their own field. The decision it reaches: **record tokens, never
compute dollars.** A price table is four models times four rates plus multipliers plus per-account
plans, stale the week a price changes and wrong with nothing to say so — and the agent's own output
says `costBasis: "list"`, so it would be a third number disagreeing with two others. Then **built**, and building it
produced the session's fourth finding — the one that matters most and is the least finished.

**The agent writes its transcript behind its own event stream, by an amount it does not promise.**
The first version of the column was blank for exactly the run it exists for: a capped run showed
`-`. The parser was right and the data was not there — the checkpoint taken at the turn-1 boundary
captured **16,999 bytes** of transcript whose rows were `queue-operation`, `user`, `attachment`,
`atis-latch` and `ai-title`, with **no assistant rows at all**, while the same file held three once
the run was over. Nothing errored and the blob was not empty, which is exactly why it had never
been noticed. So the transcript is read **twice**: at the checkpoint, on bytes already in hand,
which keeps a long run's number moving while somebody watches; and once the leg has ended and the
process is gone, which is the only moment the file is known to be final — inside the `describes`
branch, because these numbers gossip and a leg that lost the run may not state them. Re-walked:
**21.8k** on a run capped at one turn, matching the transcript's own totals (10 + 217 + 3,740 +
17,838 = 21,805) exactly.

**And the part that is not finished, now at the top of the pick-up list.** The same lag applies to
what a *checkpoint stores*. A run checkpointed at an early boundary carries a transcript missing
its most recent turns, and at turn 1 one with no assistant messages in it — so a migration from
there resumes into a conversation that does not contain what the agent already did. The plausible
outcome is that it redoes a turn rather than anything worse, and plausible is not measured. Session
23's migration walk succeeded, which says the case is not universal and says nothing about this
one. It is a question about the product's central claim, so it is written down as a walk rather
than as a paragraph.

**Then that walk was run, and the answer was worse than the guess in the note.** Resumed onto a
rebuilt worktree — the migration path, `transcript restored from checkpoint` in the log, the stored
blob written over the agent's own current file — the agent replied **"No response requested."**,
made no tool call, wrote nothing, and the run reported **`Completed`**. An abandoned run that reads
as a success, arrived at from a new direction. The control, whose capture had caught up, resumed
and wrote the nanosecond timestamp that had existed only in the conversation, so the mechanism is
right and it was the bytes that were not there. The lag is a **race**, not a fixed offset: two runs
captured at the same turn 1 stored 22,953 bytes with the conversation and 17,087 with none of it.

The fix needed no ADR, because `checkpoint` already stated the rule — *better to fail the
checkpoint loudly than to record one that cannot do its job* — and was asking the weaker question
of whether the **file** existed rather than whether there was a conversation in it. It now refuses
a capture with no assistant messages, and the predicate it uses is the token parser built an hour
earlier for reporting. Walking *that* produced two more findings, which is the session's lesson
three times over. **A `messages < turn` warning I added fired at every boundary of a healthy
five-turn run** — turn 2 with 1 message, turn 3 with 1, turn 4 with 2, turn 5 with 3 — so being a
turn behind is the normal state and the warning was noise; removed. And **a refused capture ate the
operator's checkpoint request**: the flag was read *and cleared* before the attempt, so
`offload checkpoint` said "it will be taken at the next turn boundary" and then the run finished
normally, never released, with nothing said. Reachable before by any capture failure and routine
the moment lagging transcripts started being refused. Read without clearing; cleared by the arm
that honours it. Re-walked: turn 1 refused and said so in the run's own log, turn 2 captured with
`release=true`, released, resumed onto a rebuilt worktree, and `answer.txt` came out holding
`1787985667801634502` — the value that had existed only in the conversation.

**And then the two-node migration, re-walked because the guard changes what a drain waits for —
and the central claim holds.** Two daemons, a real fleet, fifteen minutes of probation. A run
submitted on alpha was placed on **beta**; `offload drain` on beta handed it to alpha at turn 4;
alpha finished it and wrote `result.txt` holding `1787988284525120168` — a nanosecond timestamp
`date` had printed **on beta at 09:24:44**, two seconds before the drain — with **zero Bash calls
after the resume**. The prompt forbade re-running the command and nothing had written the number to
a file, so the only way the receiving node could know it is the conversation the checkpoint
carried. Session 23's proof shape, re-run against a system that now refuses captures.

Three more things came out of the same forty seconds. **A refused capture does not slow a drain**:
beta refused three during that run and the handover still took **0.77 seconds**, which was the
open worry and is now answered. **Tokens survive a migration** — 1.1M after the move, which is
`RunProgress`'s position merge working across two real nodes rather than in a property test. And
**cost is summed across legs** ($0.178, beta's plus alpha's), which is ADR-0040 §4 verified on a
real migration instead of on one machine. No `Refused` audit rows on the drain path, so session
twenty-nine's fix still holds.

Two walk notes went to `docs/DEMO.md`, both costing an attempt each. **A daemon that started before
its node was a member has no mesh until restarted** — `offload grant` says "no restart" and that is
true of the certificate, not of the mesh. And **Haiku finishes a twenty-step dependent chain in
under a minute** (29, 37 and 62 turns across three runs), so the window for draining mid-run is
seconds: submit without `--follow`, poll `explain` for the holder, drain *that* node — and do not
assume the run landed where it was submitted, because in all three attempts it did not.

Schema **v11** (five token columns); the **wire did not move**, and `offload-proto` records why
beside `VERSION` rather than leaving it to inference — `RunProgress::tokens` is the third
`#[serde(default)]` field of exactly that shape, and a relay cannot erase it because it moves only
inside `absorb`'s position branch. **847 tests, clippy clean.** Four Haiku walks on the
`~/.claude-alt` account across the session, well under a dollar.

## Session thirty, in one paragraph

No product change — **the documentation itself was measured and found not to work**, which is the
same failure this project writes down everywhere else: a rule is either enforced by something or it
is a hope, and "read this before starting anything" in front of 4,635 lines is a hope. `CLAUDE.md`
was 1,865 lines and `docs/HANDOFF.md` 4,635, and both had gone stale in the way that proves nobody
could reach the bottom of them: `CLAUDE.md`'s crate layout still listed **`offload-sched`**, never
created — the bid round is `offload-cluster::place`, a deviation recorded only at line 3,611 of
HANDOFF — and HANDOFF's own head still said "ADRs 0001–0035" with 0036, 0037 and 0038 accepted.
`CLAUDE.md` is now 238 lines and its pitfall index is keyed on **the path you are about to edit**
rather than a topic you have to recognise, with every path verified to resolve against `crates/`.
The 177 entries moved verbatim into `docs/pitfalls/`, by subject, and were then split again into two
tiers once the context cost was measured: opening the three files a `supervisor.rs` edit needs cost
**9,500 tokens** before writing a line. What made them expensive is that each entry carries the
rule, the mechanism *and* the measurement — and the measurement is evidence for believing the rule,
needed once, where the rule is needed every time. `docs/pitfalls/*.md` is the rules and
`docs/pitfalls/detail/*.md` the entries verbatim in the same order; that edit now costs **3,300**.
Both moves were checked rather than trusted — every entry compared line-for-line against the
original, and every rule verified to share vocabulary with the entry at its own index, which caught
two drain entries reordered and one entry folded into a heading, losing the case where `offload
fleet --socket` answers from `$HOME/.offload`. Session history moved to `docs/sessions.md` (this
file) and the demo runbook to `docs/DEMO.md`, so HANDOFF is a 120-line live head with a rule that
the next session **replaces** its pick-up list rather than appending. HANDOFF's open-questions list
was dropped in favour of ROADMAP's, which had the same items and was current — two lists of open
questions is the same failure as two copies of a rule. ADR-0001 gained an amendment for the crate
that was never created. **The general shape, and the reason this counts as work:** the docs are
read into context every session, so length, staleness and duplication are functional defects and
not matters of taste — and the file nobody can reach the bottom of is the file whose top nobody
corrects. `docs/ROADMAP.md` got the same treatment last: 1,373 lines of which the live content was
**five unchecked items out of 114**, so it is now 150 lines — a phase table, what is not built, and
the four open questions — with `docs/phases.md` holding every phase section verbatim. Reading it
rather than moving it found two things. **`RunSpec::max_turns` is declared, initialised to `None`
in eight fixtures and read in no production path**: phase 1 recorded it as a known gap and nothing
changed, which makes it `WorkPolicy::allowed_agents` a third time — a field that reads as a control
being applied — so it is stated in the live roadmap rather than archived, because enforce-or-delete
is a decision. And **"shutdown cancels rather than drains" was still listed as open** though
ADR-0034/0035 closed it and `main.rs` calls `mesh::depart`. The extractor also truncated four
phase-7 entries at an internal blank line, caught only because every kept item was compared
byte-for-byte with its original — which is the same lesson as the pitfall compression: a move is
cheap to verify mechanically, and the verification is what finds the mistake.

## The rolling state-of-the-tree narrative, as it stood at session twenty-nine

**Phases 0–3 and ADRs 0001–0035 are on `main`**, and **every accepted ADR is built except
ADR-0019**, which is accepted as intent and unbuilt — design ahead of code, the way ADR-0010,
ADR-0011 and ADR-0013 were when they were written; its *smaller half* is now built, see below.
New in session twenty-nine, five findings across two walks. From the second — the last pair on the
previous list, **a rule using a proxied `--use` resource on a peer** — the mechanism came up
**clean** (11 firings, 0 dropped, every occurrence projecting the *proxy* rather than the holder's
command) and the precondition did not: `--use` is a **refusal** on `offload run` and was silent on
`offload when`, so a rule with it fires and has every occurrence refused — 13 firings, 0 runs,
thirty seconds, under `Nothing else to do: the next event fires it` (**ADR-0036**). Beside it, from
running two daemons: **a node gossiped every run it had heard of as one it was running**, so one run
read as `RUNS 1` on both rows on both machines, including on a node not granted `host-runs` at all.
And from the first walk, the item ADR-0034 left at the top of the pick-up list plus two more from
walking it: **`offload drain` streams** (ADR-0035), because it was a blocking request in front of a
five-minute pass and printed *nothing at all* while waiting behind a question the person who typed
it could have answered in one second; **a run that finished while the drain waited** was counted and
logged as one that could not be handed over; and — the one nobody was looking for — **ADR-0034's own
fix left the `SIGTERM` path accepting work**, because it moved the departure flag into one of
`Mesh::drain`'s two callers.
New in session twenty-six, two findings from asking what the last session's own numbers meant, and
neither touched the schema or the wire. **A checkpoint stops being one when the run stopped by
decision** (ADR-0022 §5): `resume` refuses `Completed` and `Cancelled` by name and nothing else
fetches a checkpoint, so the tick was collecting the five cheap superseded transcripts and keeping
the biggest one for ever — **472 KB a run**, which session twenty-five measured and wrote down as
success ("the store settling at exactly one blob per completed run"). It reaches a **peer** with no
new mechanism, which is most of what the top of the pick-up list was reaching for: 4.9 MB to zero
on a node hosting nothing. And the same predicate fixed two reports that asked `is_terminal` and
were therefore silent about the run that *reopens* — a failed run whose checkpoint is on one
machine is the only row `here only` was ever written for, and `offload ps` suppressed it. Second:
**`offload drain` said `nothing to hand over` about a run it had just stopped**, which is the
observation the last session left unconfirmed and is not the race it guessed at — `left` came from
`held_count()`, and a released run has no holder by design. Third, the combination three sessions
of trigger walks had never tried — a rule **with a sink configured**, which turned up a watcher
firing every three seconds putting **11 notifications on a phone in 40 seconds** and an escape
hatch that threw away **0 of 11 failures**: `Audience` picks routes, and nothing picked *kinds*
(**ADR-0026**). And then the item that had sat at the
top of two pick-up lists, answered by **ADR-0025** as a decision *not* to build a retention policy:
a bystander's stub record — 0 events, 0 blobs, 0 outbox rows — is the **index** `offload logs`
resolves an id in before forwarding, and it answers byte-identically to the machine that ran the
work, so deleting it would not narrow what a device can say but end it. What was missing was the
*number*, on the machine where it piles up and where neither `offload rules` nor `offload sinks`
has anything to print.
What is left in phase 5 is all platform work:
relay/NAT (a deferred library choice, ADR-0015), the mobile host process, mobile probing, and
approvers in secure hardware. **Phase 6 is done**, with
metrics left checked-open on purpose. New in session twenty-five, the half ADR-0020 named and
left: **a triggered run's record is pruned** (ADR-0021, schema **v8** → **v9**, no wire change).
A rule reclaimed its last occurrence's *checkout* and kept every occurrence's *row*, and
`Store::delete_run` had carried the reason since it was written — it cascades to outbox rows, and
an outbox row is the whole of what at-least-once is made of. The answer is that a record is
**spent** once the delivery plane has finished with it, which turned out to be two questions and
not one: nothing pending in the outbox, *and* every live route's scan past the run's last event,
which is the half with no row to point at. The finding that shaped the build is that a prune which
looks at each record **once** is a prune that mostly does not happen — every bound is a temporary
no, and the gossip-quiet one is never satisfied at the next firing of a fast rule. So a firing
asks about every occurrence of its rule, which is what `runs.rule` is for. Walked on a real daemon
for eleven minutes: **193 firings, 101 kept, flat** where it used to grow without limit; every one
of 190 *failures* kept, which is the point of the outcome clause. The walk then found two things
worth having. `offload unwatch` stranded **101 records** inside the quiet window with nothing left
to re-ask — fixed, because a quiet period buys the ability to re-ask and not safety
(`Prune::{AtAFiring, Finally}`). And on two daemons, **alpha settles at 101 while beta grows 81
→ 181 in five minutes**: the prune is node-local, gossip is not, and nothing has ever removed a
peer's copy of a finished run. Named rather than fixed — see "what to pick up". And reading the
code for what a deletion is *exposed* to found the thing worth the session: every clause guarding
a prune is about a peer that is present, so a laptop that saw an occurrence running and then
closed for an hour could teach the pruned record back to the node that deleted it — as a run held
by itself, with no agent, renewing its own lease for ever. ADR-0021 §7: **a node does not learn of
its own run from somebody else.** And asking what that prune was supposed to have *freed* found
the bigger leak beside it: **the blob collector was never called at all**, so every run on every
node has been leaving its superseded checkpoints on disk for ever — 600 KB from six turns,
quadratic in the conversation. ADR-0022 puts it on a tick. And the same question about
**checkouts** (ADR-0023): nothing has ever removed a worktree when a run *leaves*, which cost
**62 worktrees and 145 MB on a peer in five minutes** from one rule. And what all three were
stopping at, which was one bit: **ADR-0024** puts `origin` on the run, so a node that has never
heard of a rule can tell an occurrence from somebody's work — after which a peer's records and
checkouts both go flat instead of growing without limit. New in session twenty-four, `Role::Trigger` **stops
describing nothing**: ADR-0011 declared it eight phases ago and ADR-0019 named it as the half to
build first if only one got built, so ADR-0020 settles what it actually is and this session built
it. A trigger is a program the node's owner nominated whose stdout is an event stream — no
`interval` field anywhere, because a daemon that polled on a schedule would be a scheduler inside
an orchestrator — and a **rule** binds one of its events to an *ordinary* run, so bidding, leases,
epochs, migration, `explain`, the audit log and the delivery plane all apply with nothing added
and `RunSpec` is untouched. Walking it found the thing worth the session: **a rule that fires all
night leaves a checkout per firing and nobody is going to remove one.** `cleanup` is deliberately
never automatic on the sound premise that somebody will read the output, and a triggered run has
nobody by construction — eight worktrees in forty-five seconds, measured, from a watcher ticking
every three seconds. Plus a smaller one from reading the output: `offload rules` **replayed a
stored reason as current state**, reporting `still running <id>` about a run that had failed an
hour earlier, one line under `last fired <id>`. New in session twenty-three, **the claim on the front of
the README still holds, re-read with a real agent** — proven this time by a word that existed only
in the conversation: a run picked one in turn 1 on the laptop, wrote it into no file, was drained
at turn 7, and wrote it out on the desktop twenty-eight turns later, with the two daemons' agent
state directories kept separate so the transcript genuinely had to travel. The walk that proved it found five things, the last of them the one that matters: an
**agent outlives the daemon that spawned it** and finished a whole task after its `offloadd` was
SIGKILLed, unreachable by `cancel` and beyond the reach of fencing, which refuses writes to a
record a leftover process never makes. Plus a **graceful drain reporting a fence** in the audit log
of the machine that left, **`offload explain` counting a finished run down** towards a deadline it
had already met, the **resume nudge asking a migrated agent to continue rather than to finish** —
one run stopped at six steps of thirty-one, recorded `Completed` — and every row of **`offload ps`
sitting four columns right of its own heading**. New in session twenty-two, the rest of
that walk —
**`offload fleet` told a laptop its fleet had no approver** from a command that had met nobody,
**probation did not follow the grant** on the invited path so `offload grant host-runs` took
effect with no window at all, and **`offload rekey` re-invited the device just revoked**. Two
paths came out clean: the cross-node **resource proxy** (a run on one machine reaching a mailbox
nominated on another) and, in session twenty-one, ADR-0017's cross-device answer. New in session
twenty-one, three things a node said about
itself, found by two daemons and a walk: **the probe asked `claude` on `PATH` while the daemon
spawns `agent.binary`**, so a node either advertised a program it would never run or advertised
none at all while its configured agent worked perfectly; **a per-node concurrency cap of 2 that no
config could raise** sat under an owner's `max_concurrent_runs` of 4 on every desktop; and
**`--ask` on a fleet with no route to a person was accepted without a word**, then behaved exactly
as if it had not been passed. ADR-0017's cross-device claim was walked first and holds: a question
raised on the desktop, delivered by the phone, answered on the phone, `-> denied (an operator on
beta)` in the run's log. New in session twenty, the follow-up session nineteen named
— **a leg that loses a run writes down that it lost it**, in the audit log rather than the run's,
because a run's log is served from whoever *holds* it — plus phase 5's policy half: **`offload
policy` was answering about the class default** rather than the policy its daemon uses, so an
owner who configured their phone was told the opposite of what they had written, and
**`WorkPolicy::allowed_agents` was enforced by `admits` and settable by nothing**, which is the
one owner decision a device class cannot express. And a deletion: **the bid delay ADR-0006
replaced before it shipped** left a function and two weights behind, describing the superseded
protocol in the present tense. New in session nineteen,
its last open decision is answered: **`openraft` is not the answer** (ADR-0018), argued in
`storm.rs` rather than from unease, because there are two paths to two live legs of one run and
quorum removes only one of them — the other is a grant whose acknowledgement was lost, which needs
no partition and which no consensus protocol reaches. Arguing it meant modelling what a leg *does*,
which found the session's bug: **the leg that lost a run set the run's reported position for the
rest of its life.** `RunProgress` was merged forward-only on the stated grounds that its numbers
have a single author by construction, and a second grant is exactly what breaks that construction —
so a run on turn 3 reported turn 19, with the losing machine's worktree summary beside it, and the
surviving leg could never correct either because everything it said was smaller and refused. Two
kinds of number, two rules now (ADR-0005's second amendment, schema **v7**): position follows the
leg the record settled on, spend stays cumulative from every leg. Plus, from the same property and
needing no fork at all, an ordering that was forward but not **total**: two legs writing different
worktree summaries in one millisecond left a fully connected fleet permanently disagreeing about a
string. New in session eighteen, the last operator command that acted on the machine it was typed at
now **travels**: `offload cancel` reaches the node the run is on — or the node that owns the
record when nobody holds it — and it turned up a run being **recorded as cancelled by the node
that had just lost it**, unfenced and at the new holder's epoch, so the fleet said cancelled while
the agent worked on. Plus, found by running the demo rather than reading the code, **`offload
drain` did nothing at all**: it handed its runs over and never stopped accepting, which its own
doc comment has claimed since it was written, so a laptop about to be closed went on winning
rounds. And, found by reading the output, **a run id shown in a daemon's own refusal named every
run of the preceding minute** — the twelve-character rule lived in the CLI and not in the type.
Then phase 6's last unchecked item, a **deterministic simulation over the real cluster**
(`storm.rs`), which found a **refused bid round handing its fencing tokens out again** on the next
round, to nodes already running under them — and, once it grew a blob store, a **checkpoint on a
merely-suspected node that could not be fetched at all**, which is the one thing ADR-0016 exists
to guarantee. New in session seventeen, the sweep reached `offload-store`'s own
writers, `offload-node::server`'s forwarding and `offload-probe`, and found five more — two
**writing runs back as they used to be**, two commands acting on the machine they were typed on
rather than the machine the run is on (`offload rm` reporting a removal it had not performed, and
telling the whole fleet; `offload cancel` calling a running run stopped), and — measured on the
laptop this was written on — **a probe reporting the touchscreen's battery as the machine's**, 0%
while `BAT0` sat at 98%. The first two: a lease
renewal built from a listing could resurrect a run that had finished mid-loop and then keep it
alive by renewing it, and — the more damaging one, and new in the last session's fix — an
operator moving a deadline could undo the checkpoint the run had just taken. New in session sixteen, the phase-6 sweep found five more, four of them work
or news the fleet threw away: a **run that came back to a node it had already left resumed from
the checkout it left behind**, nineteen turns of somebody else's progress still in the blob store
and never applied; the **untracked-file policy dropped an agent's new file for the name of the
directory it was in**, which is `build/` in one of the repos on this laptop; a **prompt beginning
with a dash never reached the agent at all**; the **node running a run never heard the operator
move its deadline**; and a **route whose holder said its credential had stopped working had
everything queued for it thrown away**, while `offload sinks` went on listing it. New in session fifteen, and the first phase-6 work: **property tests** over
`offload-core` — one run and one view, then a whole fleet of them, then the allowlist, then
membership — which found six bugs, including **two agents able to run on one repository**
(through the mechanism ADR-0002 names as the thing that prevents it), a **healthy run unable to
report that it had finished** because its holder believed a peer about its own absence, a
**repository able to grant itself a shell** through a pattern the code called scoped, and an
**approver able to mint the one grant ADR-0012 keeps behind the passphrase**, and a **sleeping
phone that stopped the whole fleet's news**, and a **blob collector that deleted the checkpoints
of live runs**. New in session fourteen: a device is **enrolled by one that already belongs** (`offload
invite`), a **revocation travels** and a **certificate renews itself**, an **enrolment is
announced** on the delivery plane, **`offload rekey`** evicts a device without having to reach it,
and a run **reaches a mailbox on another machine**. Phase 4 has the thing on the front of
the README: a run migrates mid-conversation — now including when the machine it was
*submitted from* is one of the ones that died — and **notifications leave the fleet**: a run
finishes on a machine with no way to reach anybody, and a device that hosts nothing tells you.
New in session ten, a run **says how it wants to be told** (`--notify push` reaches the phone and
leaves the desktop's own two routes quiet) and — the direction ADR-0010 left open — a run **stops
and asks**: an agent blocked on a command it has no permission for puts the question to a person
and waits for the answer. New in session thirteen, three claims the code did not honour: a run **stops and asks about
edits too** (so `--permission ask` is usable, bounded by a budget), a run **reaches no MCP server
the fleet did not grant it** — it used to inherit the owner's own — and it **can be granted one
deliberately** (`--use email`). Plus: two daemons on one state directory no longer both start.
New in session twelve, **the last two capacity holes are closed**: concurrency
caps are real per node *and* per account — on a fingerprint two machines on one login actually
agree about, which is what made the per-account half impossible before — and a **device with two
fleets on it stops over-committing itself**, because both `offloadd` instances now consult one
ledger that belongs to the machine rather than to either fleet. 815 tests passing, clippy clean.
The wire is **v23** and the schema **v10**: session twenty-nine changed neither — `DrainStep` and
`Watching::resources` are both control-socket traffic, and the control protocol is deliberately unversioned because both binaries
ship together. Session twenty-seven bumped the wire for
`Availability::NotBefore` (ADR-0029) and left the schema alone. Session twenty-six changed neither, and
session twenty-five changed the schema
and not the wire — `runs.rule` is a node-local column, and everything that reads it is
control-socket traffic about one machine. Session twenty changed neither. Session nineteen changed the schema and deliberately
not the wire — the gossip body is JSON and `offload-proto`'s list already says why a field on
`Gossip::progress` does not earn a bump. Session sixteen changed neither at all, and most of
session fifteen changed neither — the epoch
collision was a *rule* missing from the merge and from one comparison operator, not a field
missing from the wire — and the one thing that did needs saying loudly: the membership fix adds a
field to a certificate, so **every device has to re-join**. There is no migration for a
signature.

Session twenty-seven, straight onto `main` (twelve commits): the failure a watcher's own filter discarded
(`17a0f7c`), ADR-0027 and the occurrence auto-resume ran beside its successor (`b4b3205`), the
`CONTRIBUTING.md` that was asked for (`6bec581`), ADR-0028 and the account a shell was choosing
(`aa5994e`), the guard that named no clause (`2e9f9c4`), ADR-0029 and the limit nothing remembered
(`7100636`), the nominated directory that answered from `$HOME` (`7e27e6d`), the docs
(`712568c`), the handoff (`ed1004a`), and then — from walking ADR-0029 on two daemons — the held run
`offload explain` could not account for (`d629ac1`) and the idle machine the whole fleet called
`waiting for a slot` (`d2bbd21`), those two written up (`f4c455b`), a `cargo fmt` sweep over code
committed without it (`994daa5`), ADR-0030 and the peer that retried a rule's occurrences 23 times
(`b57efda`), and the mangled-message check widened to the crates it had been claiming not to need
(`6d2f352`), those written up (`a61fbd0`), the failed run `offload explain` accounted for
backwards (`d7acf3e`), that written up (`34e4c6a`), the second start gate (`33c5a0d`), three more
dead claims from the same sweep (`402e489`), ADR-0031 (`0ec38c1`), those written up (`04cc7f7`), and the two states a layer
can never be in (`385c2be`). Eight commits, then sixteen more: three findings from one walk, two decisions with their builds, two
follow-on findings from walking those, and the docs. ADR-0029 and the fingerprint fallback were
committed together at first and then split, because the second belongs to ADR-0028 — nothing is
pushed, so the honest fix was to split rather than note it.

Session twenty-six, straight onto `main`: the checkpoint nothing could resume from (`3f609e1`),
its docs (`2a6e2f5`), the drain that reported an idle laptop (`fcfd4e3`), those two written up
(`b89b5ae`), ADR-0025 with the status line it needed (`340cf4c`), and the stray checkout plus the
four copies of a stale "does not reach" note (`8c5cd47`), those written up (`75f160f`), and
ADR-0026 with the axis a watcher was missing (`5296fd4`). Seven commits: two bug findings, two
decisions, and one sweep of doc drift that a walk rather than a reading turned up — each ADR is its
own commit because it decides something, and the drift is its own because it touches four files and
changes no behaviour.

Session twenty-four, straight onto `main`: ADR-0020 (`208d36e`), the trigger plane
(`4795f8d`, schema v8), and the checkout a rule left behind every time it fired (`7552aa3`).
Three commits: the decision, the build, and the finding the walk added — the third is a
separate fact from the second, it touches `offload-workspace` and `Supervisor` rather than the
plane, and its reasoning is about a *premise* (`cleanup` assumes a reader) rather than about
triggers.

**Everything is on `main`** and there are no branches at all: sessions fourteen to sixteen were
committed straight onto it, as sessions six through eleven were.

```
main
  313331e  Phase 0: domain model, capability probing, CLI
  …
  2871481  Merge phases 1 and 2: the single-node agent runner
  7030a62  ADR-0010 … 5bd8013 ADR-0014     the five design ADRs of session three
  f90903e  Merge phase 3: the fleet exists
```

Phase 3, in the order it was built: identity as a keypair (`405dacd`), the passphrase and
enrolment (`f111ec5`), ADR-0015 (`87e4c21`, accepted `5113862`), `offload-proto` (`9faff59`),
`offload-transport` (`41eb264`), `offload-cluster` (`d8fa195`), the daemon joining
(`9c01847`), and discovery plus workload gossip (`41b68b0`).

Phase 4 so far, straight onto `main`: capability instances (`73f41a7`), local-path
eligibility and grants with teeth (`a5bc9a1`), ADR-0016 (`d862021`), blob transfer
(`a91f129`), replication on capture (`81cc6a1`), the bid round (`6de3488`), run gossip
(`259be31`), drain and migration (`68b167e`), the ungraceful path (`023ecb0`), the merge fix
that made checkpoints travel (`0dedec6`), and reclaim (`9385098`).

**Work goes straight onto `main`, and there are no other branches.** `phase-1-agent-runner` and
`phase-3-mesh` are gone; so is session nineteen's, which was the one exception and the reason
this is written down. The repo is greenfield and has **no remote** — nothing is pushed until the
project is done — so a branch buys none of what branching is for and only adds a merge step.

**The design is a long way ahead of the code, deliberately but worth knowing.** Everything
from ADR-0010 onward describes a fleet that does not exist yet, and several of those
decisions are load-bearing in ways only building will test — capacity budgets and the device
broker, deadline arithmetic, accept-without-starting, and the whole delivery plane. Read the
ADRs as intent, not as description.

Session six, straight onto `main`: `offload explain` (`4f5cc7a`), the cold-repo clone race it
turned up (`392b4b5`), and run progress travelling with the run (`d70f36d`, schema v2).

Session seven, straight onto `main`: the deadline (`8e27622`), ordering and the deadlock it
found (`71af202`), and `offload deadline` (`a8ec605`, wire v4).

Session eight, straight onto `main`: capacity as a budget (`03ed96b`, wire v5), giving back a
commitment that will not be kept in time (`eff5f05`), attendance deciding autonomy in
failure (`a6e3c3b`), the dead-agent bug its demo found (`b36382b`), `offload priority`
(`15cd2b7`, wire v6), and following a run that is somewhere else (`85ba18b`, wire v7).

Session nine, straight onto `main`: a run that cannot make its deadline says so (wire v8), then
the **delivery plane** (ADR-0010, wire v9, schema v3) — first through this node's own routes, then
through a peer's. **ADR-0013 is built end to end** — all three axes, the budget, and both of the
reporting rows its table left to a delivery plane that did not exist — and **ADR-0010's forward
direction is built**, including the demo it was written for: a phone that hosts nothing is the
node that tells you. Both ADRs carry amendment sections recording where the implementation
corrected them.

Session eleven, straight onto `main`: grant enforcement at the bid exchange (`fe1653c`) — no
wire change, since the check reads what the handshake already carried.

Session twelve, on `session-twelve-capacity`: an account fingerprint two machines on one login
agree about (`26911b0`), then the caps that needed it and the device ledger together (`62aa626`,
wire v13). Two commits rather than three because the caps and the broker interleave in the same
functions — `Room::for_one_more`, `LocalFacts`, and the four supervisor call sites where the
device's occupancy wraps the lines the caps had just introduced — so splitting them would have
meant re-editing code rather than staging hunks. `26911b0` was built and tested alone in a
throwaway worktree (532 tests) rather than assumed to stand on its own.

Session nineteen, straight onto `main`: the position and spend split with the simulation that
found it (`a047880`, schema v7), the flake it turned up on the way (`a1338cc`), the `openraft`
answer (`f5e38d4`, ADR-0018) and the structural half (`d0434e6`). Four commits rather than three
because the flake belongs to neither the rule nor the decision — a test resolving a run by
`RunId::short()`, which that method's own doc comment forbids — and folding it into either would
have made a commit that did two things.

Session twenty-three, straight onto `main`: the false fence a drain recorded (`d9f051c`), the
finished run counted down towards its deadline (`d589028`), the resume nudge that did not ask for
the task to be finished (`0ed197f`), the misaligned `ps` header (`fbd4445`), the note that a demo
run should be pinned to the cheap model (`0a16a19`), and — from re-walking the ungraceful half
once Haiku made it cost pennies — the agent that outlived its daemon (`ee1c5bb`). One commit per
finding; they share nothing but the walk that turned them up.

Session twenty-two, straight onto `main`: `offload fleet` reporting only what it could know
(`239c4cb`), probation following the grant on the invited path (`9a52b5b`), and a rekey not
re-inviting a revoked device (`0c75f17`). The first two are both in `fleet.rs` and were committed
by splitting the working tree in half rather than as one commit that did two things — they answer
different questions and one is a security window.

Session twenty-one, straight onto `main`: the probe asking about the binary the daemon spawns
(`3f62e82`), the concurrency cap the owner could not set (`7839fa7`), and `--ask` saying what it
will amount to on this fleet (`42e25fb`). Three commits for three findings that share a family and
touch none of the same lines — the first is a wiring seam, the second a config field, the third a
submission note.

## Session twenty-nine, in one paragraph

Took the item ADR-0034 called "the best-shaped thing on this list" and walked it, on the harness the
demo notes describe and for nothing: a **fake agent that poses a real permission question** by
piping a `PreToolUse` payload into `offloadd ask-hook`, one `[[sinks]]` route that appends to a file,
one daemon with a mesh. No model spend, and the whole of ADR-0017 and ADR-0033 came up working on
the way past — `offload asks` naming the question, the phone told `answer within 5m0s`.

**The residual is worse than it reads.** `offload drain` with a run blocked mid-tool-call:

```
17:53:13
exit=124 at 17:53:53          ← `timeout 40 offload drain`; nothing above this line is its output
```

Forty seconds of a blank terminal and it would have been five minutes — `DEFAULT_ASK_PATIENCE` and
`drain_deadline_secs` are both 300 seconds, and a blocked run reaches no turn boundary until one of
them expires. Two windows away, `offload asks` printed the question *and the command that answers
it*, and `offload status` said `waiting 1 run(s) stopped for an answer`. So two commands could say
what the drain was waiting behind and the drain — the one somebody types with their hand on the lid
— could not, about a wait that was **avoidable**. **ADR-0035** streams it: `Response::Draining`
before `Response::Drained`, over the one-request-many-responses shape `logs --follow` already uses,
so no wire bump. Three steps and not a running log — `StoppedAccepting` the moment it is true (the
only thing a drain does on a fleet of one, and it was printed at the *end* of the pass), one
`Waiting` for the wait, and `Blocked`, which carries the `tool_use_id` because that is the difference
between information and the way out. Measured after the change, four seconds in, then answered as
the line says to:

```
this node has stopped accepting work — restart offloadd to take work again
  waiting for 1 run(s) to reach a turn boundary — up to 5m0s
  run 01a0440bbca7 is stopped waiting for an answer: Bash — rm -rf /tmp/nothing
    it reaches no turn boundary until that is decided, or 4m59s from now
    answer it: offload approve 01a0440bbca7 toolu_walk_1   (or `offload deny`)
  nothing to hand over — 1 run(s) finished while it waited
[the command returned at 18:27:08]
```

Reporting rather than **expiring** the question, which was ADR-0034's other candidate: the walk is
the argument against it — the answer arrived seven seconds after the line asking for it, so those
questions are unanswered rather than unanswerable, and deciding somebody's tool call to save the
drain a wait is the same trade as snapshotting mid-turn.

**Then the two the walk added, both in the same report.** Answering the question is what exposed the
first: `wait_for_checkpoint` returned `Option<Run>` and answered `None` for **three** reasons —
deadline, record gone, and *the run finished by itself*, which its own comment calls "the nicest
possible outcome" — and the caller read all three as `still mid-turn at the drain deadline`, counting
them in `left`, whose entire meaning is "could not be handed over". A run that had completed 150 ms
earlier therefore produced the sentence that keeps a lid open. `Boundary::{Reached, Finished,
StillMidTurn, Gone}` and a third count on `Drained`; the tell to remember is a doc comment
enumerating outcomes ("exactly one of three ways") beside a return type that cannot say which.

And the one nobody was looking for: **ADR-0034's own fix stopped the `SIGTERM` path stopping
accepting.** It moved `stop_accepting()` out of `Mesh::drain` and into `Request::Drain` on the sound
reasoning that the departure is a fact about the node and should have one owner rather than a
call-site check — and then made it a call-site check at one of that function's two call sites. The
other is how a laptop actually leaves. Measured: `SIGTERM` at 15:55:41, `draining runs=1` a
millisecond later, `spawning claude code` at 15:55:44 for a run submitted three seconds into the
shutdown, `offload status` saying `accepting yes`, and the operator handed a run id at the keyboard
for work that daemon was about to kill. `mesh::depart` is the third home for the flag and the only
way in — `Mesh::drain` is **private** now, so two callers agreeing is a compiler question. After the
fix, three seconds after the signal: `accepting no — drained`, and a submission refused with `alpha
draining` while still being *allowed to submit*, which is ADR-0034 §2's deliberate distinction.

**Then the last pair on the previous list, and the reason to keep that list.** A rule using a
proxied `--use` resource on a peer: two daemons, the mailbox nominated on the node that cannot host
at all, the rule on the node that can. **The mechanism holds** — 11 firings, 0 dropped, and every
occurrence spawned with `{"mcpServers":{"email":{"command":"…/offloadd","args":["use-resource",…,"--service","email"]}}}`,
which is the *proxy*, named by service, with no `env` and nothing about beta's `/bin/cat`, for a run
nobody submitted on a node where nobody is watching. Worth recording as a clean result. What it
produced instead was the **precondition**, and it is ADR-0032's shape one flag over: `--use` is the
one of the three whose keyboard answer is a *refusal*, so nobody thought of it as a note.
`unreachable_resource` sits inside `submit_run` — rightly the one copy both callers use — so a rule
with `--use email` on a fleet with no mailbox is written in silence and then has **every occurrence
refused**: 13 firings, 0 runs, thirty seconds, nothing on the phone (a refused firing is not a run,
so the plane has nothing to project), one `WARN` per firing where nobody is logged in, and at the
keyboard `Nothing else to do: the next event fires it`. **ADR-0036** warns rather than refuses,
because a run is submitted now and a rule fires for months into a fleet that changes — the mailbox
is on the phone and the phone may not have enrolled yet — which is exactly the call the trigger
question one line below already makes.

**And, from having two daemons up: a node gossiped every run it had heard of as one it was
running.** `active_runs` fills `NodeView::running` — "what this node says it is running", the ground
truth a registry could be rebuilt from — as `list_runs(false)` with no holder filter, while the
store deliberately keeps runs placed elsewhere *and*, since ADR-0025, work this node neither ran nor
submitted. One run held by alpha printed `RUNS 1` for both rows on both machines, about a node whose
own `offload status` said `runs 0/2`. The predicate was written two functions below in `held`'s doc
comment — "**Held, not merely known**" — the same bug, fixed for capacity in an earlier session and
left live on the gossiped copy. It survived because nothing *decides* on it. Worth carrying as a
habit: when a bug is fixed for the value a decision reads, go and look at the copy a report reads,
and at the one that travels.

**Then a use case from the owner, which turned into two ADRs and two more findings.** The ask: a
phone, away from the laptop that holds every credential, asking it to sweep some shops or check a
server's logs. **ADR-0037** settles the order — measure IPv6, then one reachable end (the laptop
forwarded, the phone always dialling out), then a relay, then iroh — from the observation that
Napster, Gnutella, eDonkey and BitTorrent all needed only *one* connectable end, and that of the
three things this use case needs, two are phone-initiated and the third leaves through a **sink**,
outbound, so nothing ever needs the phone to be reachable. **ADR-0038** answers "what should the
always-on box be": an ordinary member that does no work, adding **introduction** as a third
`Resolver` source — because a member with a public address is *not* dialable, ADR-0015 §1 keeping
every address inside the transport. It is cheap because an address is routing and not authority: a
hostile introduction wastes a dial and can never impersonate. Its cost is the run plane (a member
reads every prompt the fleet gossips), so `offload invite --introducer` is the first certificate that
carries **less** than the door grants — and §7 records the setup order, which inverts the obvious
reading: the reachable node comes **first in time and last in authority**, because `Terms::founding`
grants `{HostRuns, Approve}` and `offload init` derives the fleet key on whatever machine runs it.

Then ADR-0037 §2's two build items, and the walk paid twice. `seeds` was parsed as a `SocketAddr`, so
a **name was refused outright** and dynamic DNS was unconfigurable; and the list was dialled **once**
from startup, while the LAN loop "runs until the process ends" — self-healing for multicast and not
for the internet. Measured with the old binary: one attempt, given up after 30 seconds, and forty
seconds after the peer came up the two daemons still knew nothing about each other. Fixed, and then
the re-dial **exposed the sharper bug underneath it**: the fleet still did not recover. `probeable`
excludes `Draining | Departed` while a `Dead` node is re-probed for ever, so **an impolite death
healed and a polite departure was permanent** — and `introduce` could not revise it either, building
a `NodeView` at incarnation 0 that `merge_node` ignores. Measured: a peer re-met **40 times over four
minutes**, still `draining`, last heard `7m`, with the other side's `offload nodes` containing only
itself. First-hand contact now goes through `alive`, the observation path, and recovery takes
thirteen seconds. Newly urgent because *this session* made `SIGTERM` announce departure properly.

**And then a third, from walking ADR-0036's own warning: a restarted node's facts were ignored by
every peer that knew its last life.** Nominate a mailbox on beta, restart it, and alpha keeps
refusing `--use email` while beta's own `offload status` lists it — **37 seconds**, measured, and it
cleared only because this laptop's cpu load kept changing and each change bumps an incarnation by
one. On a phone, which is the device most likely to restart and least likely to churn, it never
clears. An incarnation lives in memory, so a restart begins at 0, and `merge_node` copies a peer's
facts only when the incoming number is *strictly greater* — equal means nothing at all is copied.
The refutation that existed answers a different question (`absorb` refuted a peer that said we were
*not Alive*; a peer holding a previous life's facts says `Alive` while being wrong), and a `+1`
cannot escape a number somebody remembers, so `refute_above` takes a floor. Re-walked: 8 seconds,
driven by the first gossip exchange. The churn simulation's hand-rolled refutation is deleted, which
is what proves the production path is real — 20 000 cases green without it.

**What to pick up.** 838 tests, clippy clean, wire **v23**, schema **v10**, nothing half-done. Four
walk notes are in the demo section — two about `pkill`/`pgrep` matching the shell, and one about a
one-shot dial's 30-second grace period hiding the bug it was meant to expose. What pays is unchanged and has
now paid for eight sessions: **walk two features whose justifications both name a person, together,
on a node where there is no person** — and this session adds a corollary, because two of its three
findings were in the *fix* from the previous session and in the report beside it: **walk the last
session's fix, not only the thing it fixed** — and the fifth was found simply by having two daemons
up for an unrelated reason. ADR-0034's stated residual was on the list; the
regression its move introduced and the misreported outcome were not, and both were in the same
forty seconds of output. The original family's list is now **empty** — the last pair is walked and
produced ADR-0036 — so the next one has to be *chosen*: the shape is two features whose
justifications name a person, and the candidates left are thinner (a resource proxy under a
migration; `--ask` while a rate limit holds the run).

ADR-0035's residual is stated and small: the drain's deadline does not bound the
question's patience, both being 300 seconds, so a question raised *after* a drain begins outlives it
and the run is left mid-turn — shortening the patience is the weaker form of the expiry §4 rejects,
and lengthening the drain is a number to argue about. The phase-7 list is otherwise unchanged: run
DAGs, more agent adapters, ADR-0019's `Work` enum and the agent-agnostic supervisor, cost
accounting, sandboxing, a web dashboard.

## Session twenty-eight, in one paragraph

Started with the third question in the sweep family — the two existing ones ask *what does nothing
call* and *what can this fleet never say*, and this one asks **is there a path in from an owner and
a path out to a decision**, which is the shape of two prior findings (`allowed_agents` was enforced
and unsettable; `AgentDetails::max_concurrent` was a cap no config could express), both found by
hand. Run mechanically it came up **clean in both directions**, which is worth recording rather
than hiding: every config field with no reader outside `config.rs` folds one hop into a domain type
that reaches a real decision (`untracked_policy` → `capture`, `ClusterConfig::detector` →
`mesh.rs:1696`), and the fields whose only assignment is a `default()` are `BidWeights`,
`ReassignPolicy` and `RecoveryPolicy` — all three the answer open question 7 already gives, which
is that nothing can set them so nothing can disagree, and the day to decide is the day somebody
adds a setter. The same pass over the project's own stated rule — **every gossiped field needs an
owner** — also came up clean: `merge_node` names every non-skipped field on `NodeView` and
`last_heard` is maintained by `alive()`, which `serve_stream` calls on *any* inbound contact rather
than only on our own probe rotation; `merge_run` takes the winning record wholesale so one rule
settles every field, with `spec_rev` split out into `settle_spec` and `RunProgress` living beside
the record. Three clean sweeps is a real answer to "is there anything cheap left", and the answer
is no — which is why the session then went back to the method that does pay.

**The finding, and it is one shape found four ways.** The combination never walked: `--ask`
(ADR-0017) and rules (ADR-0020), whose justifications *both* name a person — which is the shape
this project has now been burned by five times. Two flags make a promise the fleet may not be able
to keep, and both notes rode on `Response::Submitted` and nothing else. Measured on one node with
no routes, two commands in a row with identical flags:

```
$ offload run  --ask --notify push -- "check the build"
  no push route in this fleet, so nothing will tell you — `offload sinks` lists what there is
  nothing in this fleet can reach a person, so --ask will stop nothing …

$ offload when nightly --ask --notify push -- "check the build"
  Quiet while it works: you will hear about a failure, a missed deadline or a question, and
  not about a firing that went fine.
```

Zero warnings on the rule, and a **reassurance** in their place naming three notifications that
fleet could deliver none of — and with `--ask` unreachable there would never be a question at all.
Backwards: a run is answered for by somebody sitting in front of it, a rule is written once and
fires unattended for months. The handler already held the reasoning in a comment directly above the
wrong sentence ("a rule is written and then fires unattended for months"), applied to `--notify-on`
and to neither flag beside it. Three more cases from the same walk. The **plainest possible
invocation** — `offload when nightly -- "…"`, no flags — is one of them, because `audience_note`
returns `None` for `Audience::Everyone` on the stated grounds that it "needs no words": true in
front of a keyboard, false for a watcher, and it is the line everybody types first. `--notify
nobody --notify-on everything` printed `Every firing will be reported`, wrong on **any** fleet,
because ADR-0026 made those two independent axes and the sentence about *kinds* never consulted the
one that zeroes it out. And `offload run --notify nobody` gets it right, one command away.

**ADR-0032** is the fix. `Response::Watching` carries `audience` and `ask` from the *same two
functions* `Submitted` uses, so a rule and a run cannot say different things about one fleet — the
rule `can_reach_a_person` already enforces between the submission and the blocked agent, extended
to a third caller. No wire bump: the control protocol is deliberately unversioned and both binaries
ship together. The new type is `deliver::Reach::{Somebody, NobodyWanted, NoRoute}`, three answers
and not a bool for `Removal::{Removed, NothingHere}`'s reason — a silence the author **asked** for
is not a silence the fleet imposed, and printing "add a route" under `--notify nobody` is advice
about a setting somebody chose on purpose. It is computed through `Audience::admits`, asked per
route over the same local-and-peer set the delivery pass walks, so the report cannot drift from the
plane it describes; a configured route counts before it is known to work, for
`can_reach_a_person`'s reason. The `--notify-on` line still says which way the switch went — it
travels with every occurrence and this fleet may gain a route tomorrow — but in the **future tense
only under `Somebody`**, and the `NoRoute` wording carries the clause that matters: *including when
it fails*. A watcher that fires and succeeds is silent by design (ADR-0026 §3), so "no
notification" and "no failure" look identical, and the one thing a watcher exists to say is exactly
what a missing route eats. Re-walked all five audiences after the change; the test was watched red
on the no-flags assertion first.

**Then the pair at the top of that list, walked immediately: `--ask` and a rule.** Both justified
by a person, and the walk paid twice, in one notice. `Notice::NeedsDecision`'s doc comment says it
carries "how long there is to give one, because `approve this` with no clock is a promise this plane
cannot keep" — three fields, no clock. The patience was in scope at the `LogKind::Asked` call site
the whole time and went to `tracing::info!`, on the machine nobody is logged into, while
`Notice::Overdue` one arm above renders exactly such a duration. Not cosmetic: `offload approve`
addresses the agent's own `tool_use_id`, which exists only while that process is blocked, so
somebody picking the phone up twenty minutes later approves nothing and was never told there was a
window. And second, the larger half — `notable` excluded the whole `Answered` kind, right for
`Allowed`/`Denied` ("whoever approved it knows") and resting for the third answer on "what a run did
without permission is what its **result** reports", which is a claim about the result being
*delivered*. `Notices::Problems` discards exactly the result, and that is a rule's default. Measured
on a rule with a working push route, a trigger every three seconds, `--permission ask --ask=3`:

```
t=180s   1 firing    94 dropped   ask: 16.2s left    1 notification
t=210s   2 firings  103 dropped   ask: 4m46s left    2 notifications
kinds delivered:  2 × "asked"   ← and nothing else, ever
```

The phone says a decision is needed; five minutes later the wait runs out, the agent applies its own
rules, the run completes, and nothing is said again — and the question stops existing in `offload
asks` at the same moment (correctly: a pending question lives as long as the blocked process), so
neither end has it and the only record is a line in the run's own log. `Notices::Problems`' doc
comment had already stated the principle — "an unanswered question blocks the run mid-turn until its
patience runs out, which is the last thing to be quiet about" — and half of it was built.
**ADR-0033**: `within_ms` on `LogKind::Asked` and `within` on the notice; `Notice::Undecided`
projected from `Answered { Unanswered }` only; `answered` joins `NOTABLE_KINDS` as a scan **bound**
and not a decision, three outcomes of which one is news being the identical shape to `finished`
naming two. `Problems` needed no edit, which is ADR-0026's design paying off: `is_good_news` is a
predicate over one variant rather than a list to remember to extend. Re-walked: `answer within
59.9s`, then `nobody answered within 59.9s, so Bash was left to the agent's own rules`.

**Then the last pair on the list — a rule and a drain — and it found the fourth lie of `offload
drain`, in the branch a fleet of one takes.** `Request::Drain` matches on cluster *and* mesh, and
`stop_accepting()` was the first line of `Mesh::drain`: the arm a node with `[cluster] enabled =
false` never reaches. Measured — `offload drain` printed `nothing to hand over`, `offload status`
said `accepting yes`, and a run submitted a second later was accepted and started. The comment on
that very line records the *previous* session finding the identical failure one level lower and
fixing it for the early return, in words that describe exactly what was left: "the version that set
no flag at all meant `offload drain` on an idle laptop did *nothing whatsoever*". Two faults, both
live, so fixing either alone still gives a broken drain — the flag is set by the **handler** now,
before the branch, and the refusal it turns on had to reach the fleet-of-one path, where
`submit_run`'s no-cluster arm had taken the *grant* check down to itself and left the drain one
behind, ten lines from an `evaluate` that checks drain "first of all" and says why. Deliberately
**not** hoisted above the branch: a draining node may still *submit*, since the door grants
`{Submit, Deliver}` and hosting is separate, and a laptop being closed asking the desktop to work is
what this product is for. Plus `Response::Drained { no_fleet }`, because the report was telling a
fleet of one that its run was "refused by every other node — `offload nodes` says which" about a
machine whose fleet is itself. **ADR-0034.** The rule half needed no change and that is worth
knowing: `fire`'s refusal arm already counts a refused submission as a drop and keeps the reason, so
a drained node's rule stops producing work and `offload rules` shows why.

**What to pick up.** 830 tests, clippy clean, wire **v23**, schema **v10** — no wire or schema
change this session, and none needed: `LogKind`'s new fields are `#[serde(default)]` and the control
protocol is unversioned. Nothing is half-done. The three sweeps are a set of three questions and all
three are currently clean, so do not expect them to pay again immediately; what pays is what has
paid for seven sessions now — **walk two features whose justifications both name a person, together,
on a node where there is no person.** Still unwalked that way: a rule using a proxied `--use` resource on a peer
— the last pair from the previous list, a rule and a **drain**, is now walked and produced
ADR-0034. And ADR-0034 leaves one finding *unfixed on purpose*, which is the best-shaped thing on
this list: **a drain waits behind a question nobody is going to answer.** `DEFAULT_ASK_PATIENCE` and
`drain_deadline_secs` are both **300 seconds**, a run blocked mid-turn on `--ask` reaches no turn
boundary until its patience expires, and `Request::Drain` is a *blocking* request that prints
nothing until the pass returns — so `offload drain` hangs silently for up to five minutes while the
one person who could release it instantly is the person who just typed it. The two candidate fixes
are in the ADR; the better one needs the command to stream rather than block, which is a shape
change to the one command that cannot currently report progress. Two residuals are argued in ADR-0033
rather than deferred, and both are worth reading before reopening: a blocked occurrence **throttles
its own rule** (122 dropped across 2 firings — that is ADR-0020 §3 working, and the alternative is
the queueing it refuses), and **`--ask=N` is per occurrence rather than per rule**, left alone
because patience already bounds the rate and a per-rule counter would have to survive ADR-0021's
prune. The thing that would make that second one matter is a long `--deadline` on a fast rule, which
is what stops patience being the limiting factor. ADR-0032's residual: the reach answer is a
snapshot against a gossip-stale view, so a peer whose sink capability has not been learned yet reads
as `NoRoute` — over-warning, harmless, settles in a tick. The phase-7 list is otherwise unchanged:
run DAGs, more agent adapters, ADR-0019's `Work` enum and the agent-agnostic supervisor, cost
accounting, sandboxing, a web dashboard.

## Session twenty-seven, in one paragraph

**Four walks, a sweep, fourteen findings, and three of the user's own asks — the first of which
turned out to be the hole in the axis the last session built.** 827 tests, clippy clean, wire
**v23**, schema
**v10**.

**The walk was one combination: a rule whose occurrences fail *after* checkpointing.** Session
twenty-six had walked a rule with a sink and a rule with a failing agent; what neither had was an
agent that gets far enough to leave a resumable checkpoint and *then* fails, which is the ordinary
overnight failure. Two things fell out of it.

**`Notices::Problems` discarded every failure the agent reports about itself.** ADR-0026 is one
build old and its escape hatch had the same shape as the problem it fixed: `admits` was asked about
the event log's denormalised `kind` column, on the stated grounds that the scan "never decodes the
payload" — and `success` is *in* the payload. `LogKind::Finished { success: false }` has kind name
`finished` and projects to `Notice::Failed`; the projection says so in as many words ("the same news
arriving by two routes"). The two namespaces overlap in three of four kinds and disagree about the
one the filter turns on. Measured, one daemon, a three-second rule with a working sink: **69
firings, 0 notifications**, on a node whose `offload when` had printed "you will hear about a
failure". After: 12 firings, 17 notifications. `admits` takes the projected `Notice` now, so the
wrong string cannot be passed, and the column is left doing what `NOTABLE_KINDS` was always for —
bounding the scan. **Why it shipped**: the guard took the other route. ADR-0026's test drove failure
through `LogKind::Failed { reason: "the agent stopped without reporting a result" }`, which is what
`fail_if_unfinished` writes when no result event arrives, and its walk used an agent that produced
the same thing. An agent that runs and *reports* failure was in neither.

**And the 17-for-12 in that number is the second finding.** A rule's failed occurrence is picked
back up **behind the rule's back** (ADR-0027). ADR-0020 §3 promises one occurrence at a time and
`fire`'s own comment says what that prevents — "running it beside the first is two agents on one
repository by a new route". Auto-resume is a new route: `recover_failed_runs` walks a per-node watch
list that knows nothing about rules, and `rule_run_in_flight` reads `rules.last_run`, which by the
time the backoff has elapsed names the *newer* occurrence. **49 firings → 105 resumes → 150
notifications**, peak two agents, each resumed leg three turns into an event half a minute old. The
mild reading is two agents in one repository; the sharper one is that resuming *is* queueing, with
the queue hidden in another subsystem and the staleness unbounded, which is exactly what §3 refuses
to do. A firing withdraws the previous occurrence's recovery watch — one line above the
`reclaim_occurrence` that was already deleting its checkout on the same premise, so the reclaim's
"nothing is coming back for this" stops being merely usually harmless. A nightly rule still resumes
(nothing withdraws it); a fast one does not. After: **13 firings, 0 resumes, 12 withdrawals, 12
notifications.** Fourth time a mechanism whose justification names a *person* has been re-read on
the path where there is none, and ADR-0020's write-up predicted this one by name.

**Then three asks, and they are one subject: what account this device runs as.** ADR-0028 —
every fact about the agent only its owner knows is nominated in `node.toml` (`binary`,
`max_concurrent`) except the one that decides whose money is spent. The account is selected by the
agent's state directory, read from the *environment*, in two independent copies, with no config
field able to say otherwise: `offloadd` from a terminal ran on one login and from a service manager
on another, and every report said `agent claude-code 9.9.9, authenticated` either way. `[agent]
config_dir` nominates it and reaches all three places that ask about that directory — the probe
(authenticated, and as whom), the capture (where the transcript is), and the spawn, which is now
**told** rather than left to inherit. That last one is the load-bearing half:
`transcript::config_dir` argued the opposite in as many words, and it was true only while nothing
else could set the path. Its test drives a real spawn, and watched red it reported
`/home/owner/.claude-alt` — the environment of the shell running the suite, leaking in, which is
the bug demonstrating itself. `[agent] account` is the guard, reported as *not authenticated* rather
than as a missing agent. Walked against two real logins on this laptop: `acct:0a0a0a0a…` for
`.claude-alt` and `acct:0b0b0b0b…` for the default, which was not previously askable.

**Two more from that walk.** The guard fired and printed **no clause** — `NOT authenticated` with
the sentence naming both accounts set on the capability and rendered nowhere, and `offloadd`'s own
startup line saying `no authenticated agent` about a machine that is authenticated. And
`account_fingerprint` falls back to `~/.claude.json`, which is a sound hedge for a path the crate
*guessed* at and a wrong answer for one somebody wrote down: a daemon pointed at a fresh
`config_dir` reported the personal login from a file two levels up, and would have gossiped and
rate-limit-accounted itself as the account its owner deliberately did not choose.

**And the third ask, which is the one with teeth: ADR-0029, the limit no operator can raise.**
`AgentEvent::RateLimit`'s doc comment says it "feeds per-account bid scoring; this is the real signal
the probe's account fingerprint cannot provide" — and it fed nothing. `judge_rate_limit` used it to
decide whether *that run* would miss its deadline and dropped it, so the next run was started into
the same wall: an agent spawned to stall, holding a session slot, on a node idle in every report.
That is the "dead code that asserts a mechanism" entry with the polarity reversed. A node now
remembers a *blocking* status with a *stated* reset, latest wins per kind (the agent restates the
same limit every turn), and a block with no reset is not remembered at all — a fact with no expiry
is the one that gets stuck. It gates **starting**, which "load gates accepting, never starting"
appears to forbid and does not: that rule is about a number nobody controls, and this one lifts at
an instant the agent stated. Being full, with a clock instead of a count. The bid says *when*
(`Availability::NotBefore`, wire **v23** — a new enum *variant* on the bid exchange, so a v22 node
cannot decode the offer at all, which is a bid lost in silence rather than a dash in `ps`), commits
like any full node, and outranks a queue count when both hold.

**The walk found two things in it that reading did not.** The gate was first passed into `Room` by
each caller, and the fleet-of-one submit path hands a bare `Capacity` in — `From<Capacity>` fills
the field with `None`, so on the path most of this project's testing uses **the gate did not
exist**: a node rate-limited for another 76 seconds started the next run immediately.
`Supervisor::room` is the funnel, and the general shape is worth more than the bug — *a `From` impl
that defaults a decision is a forgotten call site with no compiler error.* And `offload status`
printed the reset as **`496117104h5m`**, because `Millis`'s `Display` renders a *duration*: a crate
with no clock cannot format an absolute instant, so the sentence there carries none and the readable
one is built where there is a clock. Walked end to end: limit at turn 1, second submission refused
with "holding new runs for another 39.0s", one agent launched not two, the line vanishing from
`offload status` with no tick involved, and the next run starting normally.

**Plus a `CONTRIBUTING.md`**, asked for and overdue: when an ADR is required and what shape it
takes, what each kind of test is for and why they are not interchangeable, one commit per fact, and
the four things a commit message has to answer — including *which guard was missing*, which is the
one that gets skipped. It decides nothing new; it states the existing rules once, in the file
somebody arriving looks for.

**And then the walk that verified it, which is where two more findings came from.** ADR-0029 was
written with its *fleet* claims unverified — the fleet-of-one path refuses rather than commits, so
the commitment and the steering were reachable only with peers. Two daemons, two accounts
(`acct:0d0d…` on alpha, `acct:0e0e…` on beta, which ADR-0028 is what made arrangeable at all),
PROBATION shortened for the walk and put back. All three claims hold: `run …, starting when the
account's rate limit lifts` with one agent launched not two; `there is room now; starting the run
held for it` when the limit lifted; and, with both nodes up, `run 01a0420ce70c — accepted by beta`
with beta's agent log confirming it. Nothing in the comparison — `is_now()` is false for
`NotBefore` and `winner()`'s existing precedence did the rest.

**What the walk found is that neither report could say why a run was waiting.** `offload explain` —
the command whose entire job is "why is my run not running" — had no answer for `assigned`: the
lease's remaining time, a canvass line reading `already holds this run` (true, and not the question,
because a canvass asks who would *take* it), and a footer saying **`No node would take it right
now`** about a run alpha had accepted thirty seconds earlier. The gap is older than ADR-0029 and was
invisible while every reason was seconds long. `Supervisor::start_refusal` is the answer and is the
**same call** `start_held_runs` makes, so the report cannot disagree with the gate and a new reason
arrives without being enumerated. And the gossiped worktree summary said **`waiting for a slot`
about a machine with four free slots** — in the column somebody reads at 07:00 to find out whether
there is uncommitted work, on every node in the fleet. That fix needed the refusal rendered
*twice*, deliberately: the operator's sentence carries a fresh duration, and the stored one carries
none (a number written down is a claim about now that was true about then) and must fit the
**eighteen characters** of that column — the first attempt said `account rate-limited`, which is
twenty, and pushed every following column off the end of the table.

One thing the walk settled by *not* being a problem: a commitment held by a rate limit is not handed
to an idle peer, because `review_commitment` acts only on a **stated** deadline. That is the rule
that stops every undeadlined run bouncing from queue to queue, it is what a full node's commitment
does too, and changing it is an ADR rather than a patch.

**And then the residual ADR-0027 had named, measured rather than believed.** ADR-0027 wrote down
what it did not reach — an occurrence placed on a **peer** and failed there is watched by that peer,
with the rule on another machine — and *guessed the bound*: "what is left is a resume of a stale
event, three times, and not two legs of a rule racing." Walked instead, a rule on alpha with `accept
= "never"` so every occurrence lands on beta and fails there:

```
alpha  29 firings  ·  0 events dropped  ·  0 withdrawals
beta   50 agent launches (29 fresh + 23 resumes)  ·  bursts of 5 in one second
```

Understated in three ways. ADR-0027's withdrawal is a **no-op** there — the watch is on beta and
the rule is on alpha — so nothing bounded it at all. Alpha reports **perfect health**, because each
occurrence finishes before the next tick, so the 1.7× spend is invisible from the machine somebody
logs into. And it arrives in bursts, which reads as a fault in the peer. Underneath it, a
contradiction needing no rule and no gossip: `nobody_waiting` deletes a terminal occurrence's
*checkout* precisely because nobody will come back for it, while `recover_failed_runs` intends to
start an agent in that checkout.

**ADR-0030: recovery of a machine-started run belongs to the machine that started it**, and the
discriminator needed nothing new — which is the part worth remembering. `origin` **travels**
(ADR-0024), so a peer can tell an occurrence from somebody's work; `runs.rule` deliberately **does
not survive a merge**, so "tagged here" means "a rule on this machine fired it". No `RuleId` leaves
the machine that owns it, which is the trade ADR-0021 and ADR-0024 had both already declined and
which turned out not to be needed. Checked **before** attendance, and the test asserts the order
rather than assuming it: an occurrence is unattended by construction, so a check after that branch
is waved through every time. Walked after: **137 firings → 137 agent launches, 0 resumes, 135
escalations**, sustained over nine minutes.

**And the check that should have caught what I wrote.** `Escalation::NotOursToRetry`'s sentence
shipped into the walk with **thirty spaces in the middle of it** — the mangled-continuation defect
`offload-node/tests/messages.rs` exists for, made by exactly the tool-and-Python route its module
docs describe. The check did not see it because it was scoped to one crate on the strength of a
claim about where mangled text matters ("the daemon's refusals are the longest sentences in the
workspace"), and `Escalation`'s reasons live in `offload-core` and render straight into the daemon's
log. It walks `crates/*/src` now and immediately found a second hit, in `offload-agent`, also
written this session. A check scoped by a claim about where a mistake matters is scoped by a claim
with a date on it — and it now asserts it found at least eight crates, because a walk looking in
the wrong place passes by finding nothing.

**One process miss, written down rather than tidied away.** Three commits this session ran the full
suite and clippy and **not `cargo fmt --all`**, which is listed one line below them in
CONTRIBUTING.md. Clippy says nothing about formatting, so the drift was invisible until the next
`fmt` swept three files at once; it is its own commit (`994daa5`) so that somebody else's
whitespace is not in a diff about recovery.

**And the same command one state over, which is where the session ended.** `offload explain` was
fixed for a run that is `assigned`; a run that is `failed` turned out to be worse than silent.
Measured, one daemon, one run, one second apart:

```
daemon:   leaving it failed for a person to look at
          reason=somebody was watching it when it failed
explain:  attendance  unattended — nobody is streaming it
```

ADR-0013's module docs predicted it in as many words — attendance is sampled at the moment a run
fails *because* a failure ends the stream, so "sampling attendance a tick after the fact would find
nobody watching every single time" — and that line sampled it a tick after the fact. For a terminal
run the remembered observation wins now and says which it is (`attended when it failed — which is
what decided`); where the node has forgotten, it says that instead, which is
`Escalation::AttendanceUnknown`'s answer and its reason.

The other half of the same gap: nothing said whether a failed run was coming back, and `failed`
looks identical whether a retry is thirty seconds away, the retries are spent, or the run was
deliberately left for the person who was watching — which is precisely what ADR-0013's third axis
decides between. There is a `recovery` line now, and both halves needed the same thing: **the
escalate branch records its decision rather than deleting the entry.** Deleting it was right about
saying the reason once and was also the only record of it. Walked: `picking it up again in 53.0s
(try 2) (picked up once already)`, `left for a person: somebody was watching it when it failed`, and
both lines disappearing correctly once a person resumed it.

That also moved the "fresh budget" rule somewhere it means something. It was a side effect of that
deletion, so it applied only to a run already given up on — a person resuming one still in backoff
inherited its retry count. It is `stop_recovering` on the human path now, which gives that method
its second caller: a firing superseded the occurrence, or somebody took the run over.

**And then a sweep, because three findings in one report family suggested a systematic one was
overdue.** Mechanical: list every `pub fn` in a crate, count references outside its own declaration
and test modules, read what is left. **7 of 334** in `offload-core`, **19 of 384** elsewhere. It
needs reading rather than acting on — `serialize` was a serde `with =` helper reached by the derive
macro — and three of the survivors mattered.

**`WorkPolicy::admits_start`: a second start gate with five tests and no callers.** Its doc comment
asserted the mechanism in the present tense ("The account cap *does* gate starting, where observed
pressure deliberately does not… Skipping it here would also make the cap decorative"), and the live
gate is `Room::for_one_more`. The danger is specific: it has the right name, the right doc comment
and the right shape, and it does **not** know about the account's rate limit — so the next person
wiring a start gate reaches for it and silently reopens ADR-0029's hole. Deleted, its reasoning moved
to the live gate, and its five tests — which protect real rules, including the deadlock a real daemon
found — now test the code that runs.

**And a whole module nothing called (ADR-0031).** `offload-store::observations` opens by describing
the restart bug it fixed: "In phase 1 that history lived in memory, so it reset on every daemon
restart — which is precisely when a node has just been absent." No production reference at all, so
every sentence was still true in the present. That is the third time this session a *file arguing
for itself* turned out not to be wired, and the general shape is now in CLAUDE.md: a doc comment in
the past tense is not evidence that anything calls the code.

Reading it turned up the half nobody had asked. **The same history was a field on the gossiped
`NodeView`** with no owner in ADR-0005's table, and it gave two answers: a peer learned *indirectly*
inherited the relayer's history of it wholesale, and one learned *directly* started at zero, because
a node's report about itself always carries zeros. So the only values that crossed the wire were
third-party hearsay, and which one a node got depended on the order it met the fleet in. It does not
travel now and is durable instead — and `Store::record_return` went too, a second copy of
`NodeView::set_status`'s average whose own doc comment asked somebody to keep two copies in step.

Walked on two daemons: `beta` killed and returned, then **`alpha` killed and returned**, with
`offload nodes` still showing `beta alive ~1` afterwards — which can only have come from the store,
since gossip now carries zero. And the ownership half measured rather than argued: alpha's store
holds one row about beta at 1505 ms, beta's holds one about alpha at 2722 ms. Each the observer's
own, different numbers, each surviving its own restart.

**And the sweep has a second axis, which asks the opposite question: what can this fleet never
*say*?** Same mechanics over enum variants — count references in construction position rather than
pattern position, exclude thiserror's `#[from]` — 16 of 512, mostly serde or clap. It is the mirror
of `Refusal::AgentNotAllowed`, which was enforced and unsettable; these are sentences nothing can
produce. Two named states their layer can never be in. `handshake::Refusal::Draining` is the one
worth remembering: **a draining node needs its connections**, because handing its runs over is
`place()` dialling peers and the fleet only learns it is leaving because `NodeStatus::Draining`
gossips out of it — so a refusal at the door would break the feature it is named after, and it is
exactly the kind of variant somebody wires up. `TransportError::NotAMember` described a state a
transport cannot be in, with two real answers already at the two layers that can ask.

And the third was a **feature claim in a doc comment**: the `--constraint` grammar ANDs a flat list
and pointed elsewhere for the rest — "anything genuinely tree-shaped goes in a run spec file", of
which there is none, that sentence being its only mention anywhere in the CLI. So `Constraint::Not`
is handled correctly by `matches`, `explain` and `failures` and producible by nobody. Left
unreachable deliberately and the sentence fixed: the domain model is a boolean tree, so what is
missing is a *surface*, and a syntax people will have to live with is a decision rather than a
patch.

**What to pick up.** 827 tests, clippy clean, wire **v23**, schema **v10**. Nothing from this
session is left half-done. **Both sweeps are worth repeating after any phase of work** — it is cheap,
it is mechanical, and it found in one pass three things that reading the same files repeatedly had
not. The phase-7 list is what remains: run DAGs, more agent adapters,
ADR-0019's `Work` enum and the agent-agnostic supervisor, cost accounting, sandboxing, a web
dashboard. Two residuals were argued rather than deferred and are worth reading before reopening
them: **several accounts on one daemon** (ADR-0028 — a placement dimension, not a config field, and
a run naming an account would put a login into a spec that travels) and **gossiping an account's
rate-limit position** (ADR-0029 — a moving value, so ADR-0013's rule about attendance applies; the
half that would be safe to travel is exactly the half with a stated reset, and that is a decision
for whoever has two rate-limited daemons in front of them). ADR-0027's one residual is **closed** — ADR-0030 —
and its stated bound turned out to be wrong, which is the argument for measuring a "does not reach"
note rather than filing it. What is left in its place is small and stated: an occurrence placed on a
peer that fails for a *transient* reason is retried nowhere, is reported (a rule's default is
`Problems`), and the next firing is the recovery.

## Session twenty-six, in one paragraph

**Two findings from asking what the last session's own numbers meant, the item that had sat at the
top of the pick-up list for two sessions — which turned out to be a decision *not* to build
anything (ADR-0025) — and then the combination no walk had ever tried (ADR-0026).** No schema change,
no wire change — 815 tests, clippy clean, wire **v22**, schema **v10**.

**The first was written down last session as a success.** ADR-0022 put the blob collector on a
tick and walked it, and the walk's own conclusion was "the store settling at exactly one blob per
completed run". The right number is **zero**. `Supervisor::resume` refuses `Completed` and
`Cancelled` **by name** — they are decisions, and re-opening one would restart work somebody
deliberately stopped — and nothing else fetches a checkpoint at all, so past one of those two
states the blobs the record names are reachable by no path there is. And they are the *largest*
checkpoint the run ever took, because `record_checkpoint` replaces and a transcript is the whole
conversation so far: the tick was reclaiming the five cheap superseded transcripts and keeping the
expensive one. ADR-0022 §3 contains the whole argument already — recovery fetches the blobs the
*record* names, so anything the record has moved past is unreachable — and nobody drew the half
that applies to the checkpoint the record **still** names.

Measured on a daemon with a fake agent whose transcript grows the way a conversation does: **472
KB a run, flat and growing one run at a time**, 1.4 MB across three. After: the standing 1.4 MB
goes to zero on the next pass, a run that checkpoints *through* two passes is untouched and
finishes normally at six turns, and the steady state is nothing.

**And it reaches a peer with no new mechanism, which is most of what the item at the top of the
list was reaching for.** Two daemons, three operator runs placed on alpha, beta enrolled without
`host-runs` so it hosted nothing: **beta held 18 blobs and 4.9 MB** of replicated checkpoints for
runs it never ran. Both nodes went to zero on the next pass, because a bystander's copy of the
record says `completed` and the predicate is asked of the record. What is left on a bystander is
the ~1.5 KB row, at whatever rate a person submits — so the retention policy ADR-0021 deferred is
still deferred and is now a decision about kilobytes rather than megabytes.

**`Failed` is the exception, and it is why the rule is not `is_terminal`.** It is the one terminal
state that reopens (ADR-0013), so its checkpoint is the one whose loss cannot be undone — and the
walk went further than the test: the failed run's checkpoint survived three passes, auto-resume
picked it up, and it carried on at turn 5. A narrower predicate would have destroyed exactly that.
`Run::resumable_checkpoint` is the predicate, over `RunState::may_resume_later` whose arms are
**written out**: a new terminal state has to make this decision rather than inherit it, because
the two wrong answers are not symmetrical and the compiler is the only thing that will ask.

**The same predicate corrected two reports that had been asking `is_terminal`, about the same
run.** `offload ps` suppressed the `here only` warning for every terminal run — so the one row it
was written for, a failed run whose checkpoint exists on one machine, never showed it — and
`offload explain` printed a bare turn number in the same case. Extending both needed
`Checkpoint::is_durable(Option<NodeId>)`, because a run with **no holder** has nothing to discount
and `replicas` never contains the node that took the checkpoint: the question is simply whether
anybody else has it. Both callers had invented a holder — one substituted the local node, which
answers about a peer's copy on a peer, and one read no holder as no copy, reporting three replicas
as `on one machine only`. Nothing failed when the fix went in, which is what the five new tests
are for; each was watched red against the code it replaces.

**The second finding is the observation the last session declined to believe.** `offload drain`
printed `nothing to hand over` on a node with a run going, and the guess was that the run had
completed between the drain returning and `held_count()` being read. It is not a race, and it
reproduces deliberately: alpha holding a run, beta enrolled without `host-runs`, so nobody can
take it.

```
running turn 3  →  offload drain  →  "nothing to hand over"
                     ps: pending, turn 7
```

`left` came from `Supervisor::held_count`, which answers a **capacity** question — how many runs
is this node holding — and a checkpoint captured with `release = true` hands the run back to the
pool, so it has *no holder by design*. The one outcome worth reporting was the one outcome
invisible. The agent had been stopped, the run released, and the person about to close the lid was
told there was nothing to do; the daemon's own log said `nobody would take it; it stays here`.
Same shape as `offload cancel` reading the process table: a question that *looks* like the right
one, asked of the wrong thing at a different instant, and the third time this command has said
something untrue about what it did.

Both numbers come from inside the pass now (`mesh::Drained`), which is the only thing that knows
them: every run ends a drain in exactly one of three ways — handed over, still mid-turn at the
deadline, or refused by every peer. All three arms walked. Refused prints `nothing could be handed
over; 1 run(s) still here`, its own sentence because "handed 0 run(s) over" is the least useful
way to say the most alarming outcome. Mid-turn, with a one-second deadline against three-second
turns, prints the same and the run keeps its turn. Handed over — beta granted `host-runs`, with
`PROBATION` shortened for the walk — prints `handed 1 run(s) to other nodes`, and the run resumed
on beta at turn 4.

**What guards it, said plainly.** The blob fix has four tests and each was watched red. The drain
fix has none at the drain: `Mesh::drain` needs a QUIC transport and a peer willing to refuse, so
what is pinned instead is the *premise* — a run a drain released is still here and `held_count()`
counts none of it — which is what the next person reaching for that number will find. The guard
for the behaviour is the walk.

**And then the item itself, which turned out to be a "no" (ADR-0025).** It sat at the top of two
sessions' pick-up lists as a retention policy waiting to be designed. ADR-0021 deferred it on the
premise that a peer's copy is accumulation, and *guessed* that deleting it "would change what
`offload ps --all` and `offload logs` can answer from a machine that was not the one running the
work". Measured instead, on two daemons with a rule firing every two seconds and an agent that
fails every time: beta, enrolled without `host-runs` so it gossips and never bids, held **127
records with 0 events, 0 blobs and 0 outbox rows** — one kilobyte of row apiece, ~1022 bytes of
`run_json`. And then `offload logs <run>` **on beta**, for a run beta never touched, came back
byte-identical to alpha's answer, failure reason and all.

The stub is not residue, it is the **index**. `logs` resolves a run id in the local store and
forwards to whichever leg last wrote the run's position — session twenty-three's own fix — so with
no local row there is nothing to resolve and nowhere to forward. Deleting it would not *change*
what a device can answer; it would end it, for that run, on that device. It is also the only answer
ADR-0021 §7's standing allows: a node refuses a peer's copy of *its own* run because such a record
was made here and deleted here, and a node pruning **somebody else's** has no such standing — so
that prune deletes and re-learns for ever.

**What bounds it, stated rather than assumed.** Operator runs are human-paced (1 KB a record; fifty
a day is 18 MB a year). Completed occurrences are pruned on every node by ADR-0024. Failed
occurrences are **unbounded — and not by anything ADR-0025 introduces**: that is ADR-0021 §2
keeping every failure because only the log says why, holding *exactly* on both machines, 127 and
127, because the bystander is where somebody reads it from. `offload unwatch` does not help and
should not — `spent_occurrences` requires `completed`, so a failing rule's records outlive the rule.

**So what was missing was never a sweep. It was that nothing said so on the machine where it piles
up.** `offload rules` prints what a rule is keeping and `offload sinks` prints the queue behind a
silence; a peer that hosts nothing has neither a rule nor a route, and it is the machine these
arrive on fastest and the one nobody logs into. `offload status` prints it now — and the same
sentence one layer down, for ADR-0023's own last stated residual: a **checkout with no run record**
is kept on purpose, may hold the only copy of an agent's uncommitted work, and had nothing at all
pointing at it. Both are counts and not sizes, because the cheapest command in the CLI must not be
the one that walks the most disk.

```
beta   records     332  ·  331 for work this node neither ran nor submitted
                   └─ 331 of those failed. Nothing prunes a failed run's record …
alpha  records     332
```

**And a gap that had already closed, repeated in four places.** `reclaim_departed_checkouts`' doc
comment said "what this deliberately does **not** reach: an occurrence that finished on a peer" —
four lines above `nobody_waiting`, which is the code that reaches it. ADR-0024 closed it two
commits after ADR-0023 stated it, and the sentence survived in that comment, in ROADMAP, in
CLAUDE.md and in ADR-0023's own consequence list. Settled by walking rather than reading, because
reading is what produced four copies of it: a rule on alpha firing every three seconds, alpha
configured `accept = "never"` so every occurrence is placed on beta and finishes there —
**160 firings, 76 records kept and flat, blobs 1–10, beta's worktrees 0–5, disk flat at ~5 MB on
both nodes**, where before ADR-0024 each of those grew one per firing without limit. A "does not
reach" note is a claim with a date on it, and the commit that closes a gap is not the commit that
will remember to delete the note.

**And then the combination no trigger walk had ever used: a rule with a sink configured
(ADR-0026).** Every walk of the trigger plane, in three sessions, ran on a node whose own log said
`no delivery sinks and no fleet; nothing will be notified from this node` — so ADR-0010 and
ADR-0020 had never been in the same room. With one sink present, a rule firing every three seconds:

```
13 firings  →  11 notifications
06:08:42 | finished after 2 turn(s), $0.0005
06:08:47 | finished after 2 turn(s), $0.0005
06:08:52 | finished after 2 turn(s), $0.0005
```

A phone buzzing every three seconds all night to say nothing happened — CLAUDE.md's own sentence
about a sink's cursor ("how somebody learns to turn notifications off") arriving through another
door. **And the escape hatch is worse than the problem**: `--notify nobody` on the same rule
failing every firing delivered **0 of 11 failures**, measured, which is the one thing a watcher
exists to say. A control run confirmed the failures do project — the same rule on the default
delivered 11 of 11.

So a standing instruction had two settings and both were wrong, and no third could exist:
`Audience` selects **routes** and is asked per capability for exactly that reason (ADR-0010's own
amendment). What a watcher needs is a selection over **kinds**, and one axis was doing the work of
two. `RunSpec::notices` is the other one — `Problems` is everything except the run finishing, and
deliberately not "only failures", because a missed deadline and a question are requests for
attention too and an unanswered question blocks the run mid-turn until its patience runs out.

**`offload when` defaults to `problems` and `offload run` does not**, which is what a watcher *is*.
Both say which way it went and `offload rules` prints `reports problems only`, because a rule
written months ago is exactly where "no notification" and "no failure" look identical.

**The cheap fix was available and is the wrong one.** The delivery plane could notice that a run
was started by a rule and be quiet about its successes — which ADR-0024 forbids in as many words,
and the prohibition turned out to be right here rather than merely binding: some watchers want the
heartbeat, and "don't buzz me when it works, only if it breaks" is a reasonable thing to say about
a run *you typed*, which a fix keyed on origin could not express. So the author decides and it
travels as an ordinary field, and the plane learns nothing about where the run came from.

No wire bump — `#[serde(default)]` reads `Everything`, today's behaviour — and that was checked
against a rule written by the previous build rather than assumed: it kept notifying every firing,
and `offload rules` said so by *not* printing the quiet line. After, one rule, same trigger, same
sink, only the agent changed: **37 succeeding firings → 0 notifications, 14 failing firings → 14.**

**What to pick up.** 815 tests, clippy clean, wire **v22**, schema **v10**. The phase-7 list, which
is now what is actually left: run DAGs, more agent adapters, ADR-0019's `Work` enum, cost
accounting, sandboxing, a web dashboard. Phase 5's four items are all platform work (relay/NAT,
the mobile host process, mobile probing, approvers in hardware) and phase 6's one open item is
metrics, checked open on purpose. Nothing in the prune/reclaim family is outstanding: ADR-0025
answered the last question in it, and the three residuals that remain are each stated in an ADR
with the reason they are residuals — a failing rule's records, a route that is away for ever, and a
checkout holding uncommitted work that nothing will reclaim. All three are now visible from
`offload status` or `offload rules`, which is the whole of what was missing.

## Session twenty-five, in one paragraph

**The one item ADR-0020 named and did not build, which is a decision before it is a chore.**
Schema **v9**, no wire change — the tag is node-local, like the rule that writes it.

**ADR-0021 first, because `Store::delete_run` had spent two phases carrying the question rather
than the answer.** Its doc comment already said what a caller would take with it: the foreign keys
cascade to a run's events *and* to its outbox rows, and an outbox row is the whole of
at-least-once. Four questions, and three of them have a silent wrong answer. *What may be deleted*
— a **completed** occurrence, never a failed one, because the notification said it failed and only
the log says why; the same shape as the checkout's uncommitted-work bound, and the same trade
(a rule that fails every firing keeps every failure, which is exactly what somebody wants in front
of them). *What is still owed* — **two** things, and asking only the first is the trap: an outbox
row that is still pending, and an event **no route has scanned yet**, which has no row to point at
because the outbox is filled by a scan. `deliver::scanned_to` is that half, asked of the routes
that exist *right now*: a cursor left behind by a peer that has gone never advances again and
would pin every record for ever, while a route with no cursor row holds nothing back at all,
because a new cursor initialises to the end of the log. *When it is safe* — only once the record
has been **quiet** for as long as a finished run is gossiped, because the copy a peer teaches back
comes through a merge path that knows nothing about rules and would be *untagged*, and therefore
unprunable for ever. *How many chances* — the question that shaped everything else.

**A prune that gets one chance per record is a prune that mostly does not happen.** Every bound
above is a *temporary* no, and the quiet one is never satisfied at the next firing of a rule
ticking every three seconds — which is the rule ADR-0020 §6 was written about. So a firing asks
about **every** occurrence of its rule, which is what `runs.rule` buys and why a single `last_run`
pointer was not enough. The tag survives a gossip merge only because `write_run`'s `ON CONFLICT DO
UPDATE` names the columns it overwrites and that is not one of them; there is a test whose whole
job is to fail if somebody adds it.

**The walk, eleven minutes on a real daemon, and it earned its keep twice.** A watcher ticking
every three seconds and a fake agent: **193 firings, 101 kept, flat**, one worktree, a 520K
database — where the same walk in session twenty-four grew a record per firing for ever. Then the
same rule with a failing agent: **190 fired, 190 kept**, every failure still readable by `offload
logs`, which is the clause whose wrong answer destroys the thing somebody came back for. Then
`offload unwatch`, which left **101 records** behind — every one inside the quiet window, and with
the rule gone nothing would ever look at them again. That is the fix worth the session: **a quiet
period buys the ability to re-ask, not safety.** Where there is no next firing the clause protects
nothing and costs everything, so `Prune::{AtAFiring, Finally}` says which it is, and unwatching a
three-second rule now leaves nothing.

**And the second thing, from putting a peer next to it.** Two daemons, beta joined by invitation
and hosting nothing: alpha's rule settles at **101** records while **beta goes from 81 to 181 in
five minutes**, linearly. The prune is node-local and gossip is not, so the accumulation simply
moved to the machine doing no work. Not a regression — nothing has ever deleted a peer's copy of
a finished run, for any run — but ADR-0020 is what made the supply automatic, so this is where it
starts to matter. Deliberately **not** fixed: the fix is a retention policy for a finished run a
node neither hosted nor submitted, which is a decision about *every* run and which would change
what `offload ps --all` and `offload logs` can answer from a machine that was not the one running
the work. That is the exact shape session twenty-three had to fix, so it gets its own ADR rather
than a follow-my-leader.

**And then the question that mattered more than either, asked of the code rather than of a
daemon: what is a deletion exposed to?** Every clause guarding a prune is about a peer that is
**present** — the outbox, the scan watermark, the quiet period — and none of them can see one
that is *away*. The sequence needs no partition, only a lid. A laptop learns an occurrence while
it is `Running` and closes; the home node finishes it and, five quiet minutes later — quiet
partly *because* the laptop is away — prunes it; the laptop opens and gossips the copy it still
holds. `merge_run` had never seen that run, so it inserted it: a run held by **this node**, at an
epoch nothing can contradict, with no agent behind it. `heartbeat` renews the lease for ever,
`offload ps` reports last night's finished work as running, and a restart offers it to somebody
as resumable — the "a run left `Running` with no agent behind it, its lease renewed by the very
loop that erased it" shape, reached from outside the machine. ADR-0021 §7 is the answer and it is
one condition: **a node does not learn of its own run from somebody else.** A record naming us as
`home` for a run we do not have was made here and deleted here, so a peer's copy is a memory
rather than news — the sibling of a rule `merge_run` has held since it was written, that a node
must not believe a peer about its own absence. Safe because the ordering is not a race:
`hand_over` records a grant before it publishes one and `take_run` saves before the arbiter
confirms, so our own copy always exists first. **Seven fixtures broke, and every one of them was
introducing the home node's own run by gossip** — a step the daemon has no code path for, which
is `storm.rs`'s lesson one layer down and is the reason they were fixtures rather than findings.

**And then the bigger half of the same sentence, found by asking what a pruned record was
supposed to have freed.** `blobs::collect_garbage` has existed since phase 2, is careful, has two
property tests — one of which says it is checking "the property a person actually relies on when
they put this on a timer" — and is called by **nothing**, because it is "deliberately conservative
and deliberately manual". That is the third statement of one premise in three ADRs: `cleanup` was
never automatic because somebody would read the worktree, `delete_run` was never called because
somebody had to decide about news still owed, and **manual means somebody runs it**, on a machine
nobody logs into. What it costs, measured on a daemon with a fake agent whose transcript grows the
way a conversation does: one run, six turn boundaries, **six blobs totalling 851 KB of which the
surviving checkpoint referenced one, at 243 KB.** `Run::record_checkpoint` *replaces* the
checkpoint and a transcript is the whole conversation so far, so the waste is quadratic in the
conversation's size, it is **every run on every node**, and it dwarfs the thing ADR-0021 was
about — 600 KB a run against 1.5 KB a record. ADR-0021's own "the disk stops being a function of
how long the daemon has been up" was corrected the same day; rows were the smaller half. ADR-0022
puts the collector on a tick beside `tend_own_runs`, every fifteen minutes with an hour of grace,
the grace because a blob is written *before* the row that references it — in `capture`, and again
on the receiving side of a replication. The hazard that looks fatal and is not: a **superseded
replica** goes, and keeping it would protect nothing, because recovery fetches the blobs the
*record* names and the record has moved past it. Walked with the tick and grace temporarily
shortened, which is the only way to walk a timer: standing garbage collected 10 of 12 blobs
(1.7M → 480K), and a run that ran *through* two passes finished normally at six turns with the
store settling at exactly one blob per completed run. The shipped numbers are the long ones.

**And a fourth time, which is where the session stopped: the checkouts.** `WorkspaceManager::
remove` has one production caller and it is `cleanup`, manual and terminal-only — so **nothing
removes a worktree when a run leaves**, a sentence CLAUDE.md has carried since session sixteen as
a note about `adopt` and nobody had read as a leak. Two daemons, a rule ticking every five
seconds, alpha configured not to accept work: **62 firings, 62 worktrees, 145 MB on beta in five
minutes**, from a 3.9 MB repository. ADR-0023 sweeps them on ADR-0022's tick, and the guard worth
remembering is the one that says a run has *moved on*: **"not the holder" is true of every
finished run**, because the lease goes with the terminal transition — a sweep built on it deletes
the checkout `cleanup` is manual for, which is the whole premise this chain of ADRs has been
careful about. `Supervisor::describes` answers correctly and only for *this incarnation* (it reads
the in-memory `live` map, so a restart says no about everything on disk); `RunProgress::by` is the
durable form, and unstamped is unknown rather than somebody else. Verified live: a run that
finished on alpha kept its checkout across two sweeps, zero reclaims.

**What that walk did *not* cover, and one thing it left unexplained.** The **migration** case — a
run that leaves — is covered by a real-worktree test and not by the walk: the daemons were started
without `CLAUDE_CONFIG_DIR`, so every checkpoint failed with "no transcript found", the drain had
nothing it could hand over, and the agent ran to completion where it was. Both nodes need that
variable, which is ADR-0022's demo-friction note met from the other side. And one observation
left **unconfirmed rather than written up as a finding**: `offload drain` printed `nothing to hand
over` on a node that had a run going, with `still mid-turn at the drain deadline; leaving it to
the lease` in the log — `left` comes from `held_count()`, which is read *after* `drain` returns,
so the run may simply have completed in between and `(0, 0)` may have been true at that instant.
Worth reproducing deliberately (a run that is genuinely still mid-turn when the reply is built)
before believing either reading; `offload drain` has told this lie once before.

**And then the thing all three had been stopping at, which turned out to be one bit.** Every one
of those sweeps is justified by "nobody will read this", and the only node that can tell an
occurrence from somebody's run is the one holding the **rule** — which is node-local by ADR-0020
§2, on purpose. So the reclaiming was right and its *reach* was wrong, twice over and both
measured: a peer kept **a record per firing** (81 → 181 in five minutes) and **a checkout per
firing** (62 worktrees, 145 MB). The answer is not a fourth sweep. **ADR-0024** puts `origin` on
the run — `Operator` or `Rule`, set once at creation and changed by nothing, which makes it the
cleanest kind of gossiped field: identical in every copy, so there is no owner to name and no
merge rule to get wrong. What travels is deliberately **not** the rule id, which is one machine's
name for one of its own things and useless to a peer for the same reason an `Audience` names
services and never route ids; what travels is that a machine started it. `runs.rule` stays local
for `except` and the `KEPT` column, and the two columns sit in one `INSERT` with **opposite**
rules about `ON CONFLICT DO UPDATE` — the local one must survive a merge, the travelling one must
be overwritten by it. **No wire bump**, by the rule in `offload-proto`: a build that drops the
field reads `Operator`, keeps everything, and behaves exactly as it does today, so a mixed fleet
reclaims unevenly and correctly. Schema **v10**.

**And the walk earned its keep, twice.** Two daemons, alpha holding the rule and refusing to
host, beta taking every occurrence. The field travels — **beta's records all carried
`machine_started = 1`**, learned purely by gossip from a node that has never been told a rule
exists. And then the checkout sweep reclaimed **nothing at all**, on the one machine it was
written for. ADR-0023's "no agent of ours is live on it" was asked as `live.contains_key(&run_id)`,
and **`live` is not a map of running agents**: nothing ever removes an entry, so a key means "this
incarnation started that run at some point", which is true of every occurrence a peer has hosted.
`cancel` is the process handle and `release` clears it — the same field `record_run` reads to tell
a live leg from a remembered one. **Every test passed**, and the reason is the lesson: a fixture's
runs are never actually run, so `live` is empty in all of them and the guard was vacuously true.
Fixed, and then measured: **42 checkouts reclaimed in one pass, 22 MB → 2.2 MB**, and over 148
firings both numbers flat — 2 checkouts, and 62 records on beta against alpha's 61, which is the
same five-minute quiet window on both machines. Before ADR-0024 each of those grew without limit.

**What was *not* walked, said plainly.** Still the two-node *placement* of a triggered run — beta
was enrolled without `host-runs` on purpose, so it gossiped and never bid. What that leaves
unverified is unchanged from last session and now has one more line to it: an occurrence placed on
a peer is reclaimed and pruned by neither node, since the checkout is not here and the tag is not
there. Covered by a test on the reclaim side
(`reclaiming_an_occurrence_that_is_not_here_touches_nothing`) and by nothing on the prune side.

**What to pick up.** 806 tests, clippy clean, wire **v22**, schema **v10**. The phase-7 list, and
and at the top of it what ADR-0024 deliberately did **not** answer: a peer still keeps the record
and the checkout of a run *a person* submitted, for ever, and `ps --all` on any device still
lists them. Much smaller than it was — the unbounded automatic supply was never the operator's
runs — and a different question, because deleting those narrows what a device that was not there
can say about work it did not do, which is session twenty-three's bug from the other side. Read §7 before touching it: a node pruning *somebody else's* record has none of
the standing that makes §7 safe, which is a constraint on whatever shape that fix takes.

## Session twenty-four, in one paragraph

**The first thing built from the phase-7 list, because the unwalked list was empty and this is
what ADR-0019 named to do next.** Schema **v8**, no wire change — everything here is
control-socket traffic about one machine.

**ADR-0020 first, because ADR-0019's one sentence about `Role::Trigger` is a decision about
*which* rather than about *what*.** Four questions were open and each has a wrong answer paid for
elsewhere. *How does an event arrive* — the cadence is the program's, one line of stdout is one
event, and there is deliberately no `interval` field, because that is a scheduler inside an
orchestrator and it hands the daemon an opinion about missed ticks it otherwise never needs.
*Who owns the binding* — the node whose trigger fires it, node-local, never gossiped, which is
the entire reason this is the small half: a trigger belongs to one machine's owner, so it fires
on one machine, so ADR-0019 §3's derived-`RunId` machinery is not needed. *What stops a runaway
watcher* — one occurrence in flight per rule, with anything arriving meanwhile dropped and
counted, which is ADR-0019's no-catch-up rule reached from the other direction and needs no
clock, no queue and no interval knob. *What the event becomes* — appended under a fixed heading,
capped at 4 KiB, no templating (the sink rule verbatim), and said plainly to be untrusted text
reaching an agent's prompt.

**Then the build, and the one thing worth insisting on: `submit_run` has one copy.** A trigger's
run goes through the same function `offload run` does. The checks in front of it are not
formalities a second caller may skip — `Grant::Submit` can be revoked while a watcher goes on
watching, and a service nothing in the fleet offers is still a refusal somebody can act on — and
a second copy that forgot one is the exact shape this project keeps finding. One other thing the
type system would not have caught: **a rule holds its deadline as a *duration***, because
`SubmitRequest::deadline` is an instant resolved where the command was typed, and an instant
stored on a standing instruction is in the past by the second firing with every run after that
reported overdue before it started.

**The walk found the session's real finding, and it is a premise rather than a bug.** A watcher
ticking every three seconds, a rule bound to it: eight finished runs, eight worktrees, forty-five
seconds, and nothing in the product would ever have removed one. `Supervisor::cleanup` is
*deliberately* never automatic and its stated reason is right — when an agent finishes its
worktree holds the work, and reclaiming the disk the moment the process exits throws it away
before anybody reads it. What ADR-0020 breaks is the sentence underneath: **there is somebody who
will read it.** A triggered run has none by construction; that is what unattended means. So
`reclaim_occurrence`, bounded three ways and every bound borrowed from somewhere else: only when
the checkout holds nothing **uncommitted** (committed work is on the branch and survives
teardown, so the fragile part is exactly what `git status` reports); only where the checkout
**is** (an occurrence may have been placed on a peer, and session seventeen's `offload rm` bug is
worse here because nobody is typing it, so "not here" is a third answer rather than a kind of
"no"); and asked of **git** rather than of the worktree summary beside the run, which is a turn
boundary stale by construction. Verified both directions on a real daemon: a fake agent that
leaves nothing settles at one checkout, the same agent writing an untracked file keeps every one.

**And one from reading the output rather than the code.** `offload rules` printed `still running
01a03ad8ef23` one line under `last fired 01a03ad8ef23` — one run named twice, saying two
different things, about a run that had *failed* an hour before. It was replaying the stored
reason the last event was dropped, which is a true sentence about a moment and reads as a claim
about now. Same family as session twenty-three's `offload explain` counting a finished run down
towards a deadline it had met, and the same two fixes: ask the question at the instant it is
answered (`rule_run_in_flight`, live), and word what is written down in the past tense because it
will be read back later.

**What was *not* walked, said plainly.** The two-node case — a rule on one device firing a run
that the fleet places on another — was not exercised, because `host-runs` on a second node costs
fifteen minutes of probation and the placement path is structurally the same function. What that
leaves unverified is only the *placement*; the part that could have gone wrong across nodes is
the reclaim reporting a removal it did not perform, and that is covered by a test
(`reclaiming_an_occurrence_that_is_not_here_touches_nothing`) rather than by the walk. It is the
first thing to do next time two daemons are up for something else.

**What to pick up.** 794 tests, clippy clean, wire **v22**, schema **v8**. The phase-7 list is
what is left, plus the half ADR-0020 named and did not build: **pruning a triggered run's
record**, which is a decision rather than a chore, because `delete_run` cascades to outbox rows
and an outbox row is the whole of what at-least-once is made of. *(Built in session twenty-five,
ADR-0021.)*

## Session twenty-three, in one paragraph

**The last unwalked path: a migration re-read end to end with a real agent — the thing on the
front of the README, built and verified in phase 4 and not read since, and the one path a fake
agent cannot exercise, because it has no transcript and therefore never resumes. It holds. Four
findings on the way.** No wire change, no schema change.

**It holds, and this time with a proof rather than a plausible transcript.** Two daemons on one
laptop with *separate* `$CLAUDE_CONFIG_DIR`s, so beta could not read alpha's transcript off the
shared disk and the conversation genuinely had to travel. The prompt asked the agent to pick an
unusual noun in turn 1, say it out loud, and **write it into no file** until the final step. Alpha
picked `ferrule`, ran six turns, was drained at turn 7; beta restored the transcript into its own
config directory under its own worktree path, resumed the same session id with `--fork-session`,
and twenty-eight turns later wrote `ferrule` into `notes/secret.txt`. The word existed only in the
conversation. The agent narrated the move itself — "steps 1–6 on `.../o23a`, then resumed on
`.../o23b`". The ungraceful half was walked too and got **part** of the way: `kill -9` on alpha's
daemon, 36 seconds of hold-down (`AwaitingReturn`, then `HolderDead`), reassigned to beta, which
started the run **fresh** — correctly, because alpha had not reached a turn boundary and there was
therefore no checkpoint to resume from. Say plainly what that did and did not show: the hold-down
and the reassignment were exercised, the ungraceful *resume* was not, and it is the demo traps
below that cost the two attempts at it. The ungraceful resume itself is verified and written up
under phase 4's demo 3 in `ROADMAP.md` — three nodes, resumed from turn 5 in the same conversation
— just not re-read here.

**A graceful drain reported a fence on the machine that left.** Found by reading `offload audit` on
alpha afterwards: every migration left `refused: this node held epoch 1 and tried to describe it`,
one line under the `superseded` row that records the same event correctly. `Refused` rows are
documented as "the rows worth having … every one of them is a moment when two agents on one
repository was prevented rather than merely unlikely" — which is a claim about how *rare* they are,
and closing a laptop produced one per run. The departing leg is superseded a moment after handing
its run over, so `describes` goes false before the leg's last act. `Halt::Superseded` already says
"nothing here is written down" and the match arm two lines above honours it; the end-of-leg
describe did not. It is the same false row that `describes` was invented to prevent — its doc
comment says so, about a *finished* run — one case over, on the migrating one.

**`offload explain` counted a finished run down towards a deadline it had already met.** `state
completed — finished 1m25s ago` on one line, `due overdue by 31.2s` on the next, about a run that
finished thirty seconds *inside* the minute it was given, with the number growing for as long as
the record is kept. `ps` has been right all along from the same daemon and the same field, because
`RunSummary.due` filters on `!state.is_terminal()`. Slack is a countdown and a finished run has
nothing to count; what a person wants then is whether it was met, which is `Run::prospect_at` asked
at the instant it ended.

**A migrated run was told to continue, not to finish — and one stopped at six steps of
thirty-one.** Under `--print` a run ends at the first assistant turn with no tool call, so
`CONTINUE_PROMPT` is what decides whether a resumed run carries on. "Continue from where you left
off" is a sentence a model can satisfy with one step and a summary, and one of the two migrations
did exactly that: resumed, did step 6, wrote a line about which machine it was on, and the record
said `Completed`. Nothing in the daemon was wrong — the agent reported success. What makes it worth
changing is the silence: `Completed` is terminal, so nothing retried it and nothing anywhere said
twenty-five steps had been abandoned, and a failed run is looked after where an abandoned one looks
like a success. Observed **once in two**, which is the honest strength of it; the mechanism does not
depend on the toss-up.

**And every row of `offload ps` sat four columns right of its own heading**, since `short_id` went
from eight characters to twelve and only the row was widened. One `const` shared by both lines now.

**Then the ungraceful half was re-walked properly — and it turned up the worst thing in the
session.** The first two attempts had killed alpha before it reached a turn boundary, so beta
correctly started fresh and proved nothing; pacing the run with a *dependent* chain (each file's
number read out of the one before, so the model cannot batch) fixed that. The resume itself is
fine: `HolderDead` after 15s, transcript restored, resumed **from turn 3 in the same session**,
`collywobbles` — chosen on alpha before the kill and written into no file — written out on beta at
the end. But alpha's worktree, on the node whose daemon was SIGKILLed at turn 4 with two files in
it, turned out to hold **all twenty-one**. Its agent had gone on working for another minute:
`03.txt` two seconds after the daemon died, `secret.txt` sixty-two seconds after. Both legs ran the
whole task to completion.

**An agent outlives the daemon that spawned it, and nothing in the product could stop it.**
`offload cancel` resolves a run to its *holder*, which by then is the machine that took it over,
and the machine the agent is actually on has no daemon listening. Fencing does not reach it: an
epoch check refuses a **write to the record**, and a leftover agent writes to no record — it
spends money and uses whatever tool grants, allowlists and resources the run was given. The record
was never in danger and the work was still done twice. And it is the *ordinary* case: a daemon
dying while the machine stays up is a crash, an OOM kill or an upgrade, and when it comes back
`Supervisor::recover` walks **past** that run, because somebody else holds it by then and
recovering a peer's run is exactly what that function must not do — so the leftover is invisible
for ever. The agent already ran in its own process group; what was missing was anywhere durable to
write the number down. `crate::leftovers` is that, and a starting daemon sweeps what the last one
left. Verified against the failure it was written for: daemon killed mid-run, agent confirmed
alive, daemon restarted, `an agent outlived the daemon that spawned it; stopping it pid=468379`,
process gone, note cleaned up, and a clean run still leaves no note.

**Then following it through turned the finding from "work done twice" into something worse.** The
daemon's own recovery message says `failed — resumable from turn N with offload resume <id>`, and
`resume` **adopts the worktree in place**; the guard against putting a second agent into one
checkout reads `self.live`, which is in memory and therefore empty in a daemon that has just
started. So it cannot see what the last incarnation left. Measured, with the sweep disabled: kill
at turn 4, restart, `offload resume`, and **two `claude` processes with the same worktree as their
cwd, both working**. That is the double execution CLAUDE.md calls the failure mode that matters —
on one machine, in one checkout, needing no fleet and no partition, reached by doing exactly what
the daemon told the operator to do. With the sweep in place the same sequence gives one agent, and
the single-node crash path now round-trips properly: `phantasmagoria`, chosen before the kill and
written into no file, came out of `notes/secret.txt` after the resume. Worth remembering as a
shape — *the recovery path a fix funnels people into is part of the fix*, and it had never been
walked.

**And the sweep's own safety argument is now checked rather than hoped.** It rests on "a note
still here at startup is stale", which holds only while one daemon can own the state directory —
run it unlocked and a second `offloadd` would kill the *first* one's agents on its way to failing
the lock. `main` had the order right, but an order survives until somebody moves a line, so
`sweep` takes the `StateDirLock` and calling it unlocked no longer compiles. Confirmed first that
the ordering did hold: a live agent, a second daemon refused with `another offloadd is already
using …`, and the first one's agent untouched.

**Four things about running this demo that cost time.** A step that says "make no tool call this
turn" **ends a `--print` run** — that is what a final answer *is*, so a paced prompt has to give
every step a tool call. A nested agent driven from inside a Claude Code session **inherits the
outer session's tool list**: one run called `ScheduleWakeup`, ended its turn intending to come
back, and the run was over at step 1 — the same family as the `sleep` warning below, one layer up,
and it makes paced demo prompts unreliable in a way that looks like the daemon's fault. And an
agent **outlives the daemon that spawned it**: `kill -9` on `offloadd` left `claude` running and
writing into the worktree. The guess that it would die on its next write to the closed pipe was
wrong, and chasing that down became the session's fifth finding, above. And **Haiku batches its
tool calls**, so it is wonderfully cheap and a poor instrument for catching a run mid-flight — 26
files in 3 turn boundaries. Pace it the way this file has always said: make each turn *depend* on
the last, so the model has to read before it can write.

**What to pick up (as of session twenty-three).** 778 tests, clippy clean, wire **v22**, schema
**v7**. The unwalked list is **empty**: everything left needs a phone or a decision. And the one standing reason to hesitate
before a real-agent walk is gone — pin the run to Haiku (see the demo notes), which is a fortieth
of the cost and tests exactly the same machinery. That is the state phase 6 was aiming at.

## Session twenty-two, in one paragraph

**The rest of the walk session twenty-one left: the membership commands and the resource proxy.
Three findings, all in membership, and two paths that turned out to be clean.** No wire change,
no schema change.

**The proxy holds.** ADR-0011's cross-node resource, which nothing had read since session
fourteen: beta nominates a `[[resources]]` mailbox and hosts nothing, alpha runs the agent with
`--use email`, and the generated `--mcp-config` points the agent at `offloadd use-resource`. A
line written by the run on alpha reached the program on beta and its answer came back — `opening a
resource for a peer's run` on one daemon, `carrying a peer's run's calls to a resource` on the
other, and the mailbox's own log showing it saw the call. Worth recording how the first attempt
failed, because it will happen again: a stand-in that pipes one line and closes stdin gets nothing
back, since the proxy treats EOF as the agent being finished. A real MCP client holds the pipe
open, and so must a fake one.

**`offload fleet` told a laptop its fleet had no approver.** `health` takes what handshakes
proved; the CLI passed an empty slice, and the notes read that as "met nobody, therefore none
about". So beta — gossiping with an approver every second — was told to run `offload grant
approve`, which needs the passphrase, which is the posture the whole invite path exists to avoid,
recommended by the command whose job is to describe membership. `offload status` said `1
approver(s)` one line of scrollback away. The caveat that would have softened it existed and was
printed under `state.approver.is_some()` — so the only node ever shown it was one that is itself
an approver, and the node being misled is precisely the one that is not. `Met::{Handshakes,
NotAsked}` now tells the two callers apart, because the difference is in *who is asking* rather
than in the data: a daemon's empty list is a fact worth acting on, a CLI's is an absence of
evidence.

**Probation did not follow the grant on the path people use.** ADR-0012's own amendment says it
does, with no exemption for the passphrase, because somebody holding the passphrase is the threat
it was written for. `add_grant` inherited the `probation` flag instead, and an *invited*
certificate carries `false` — for the door's reason, that a member deliberately enrolled the
device. So `offload grant host-runs` on an invited laptop took effect the same second: measured,
`grants submit, deliver, host-runs`, immediately, with no window at all on the enrolment path this
ADR exists to make the normal one. Those fifteen minutes are what mitigation 4's alarm is bought —
the announcement reaches somebody and a revocation lands before anything runs with their
credentials on their repositories. The CLI already had the sentence for it, unreachable by that
route, which is the shape `Refusal::AgentNotAllowed` had. The flag is derived from the rule now
rather than carried: a certificate probates exactly when it puts `HostRuns` in play that is not in
force already.

**And `offload rekey` handed fresh papers to the device it had just thrown out.** `offload revoke
<gamma>`, then `offload rekey`, and the rekey printed gamma an invitation into the new fleet —
name, grants, a token to paste. `known_members` reads the enrolment log, which is what this
machine witnessed and has no opinion about what happened next, and nothing else in the path had
one either. The new fleet's revocation list starts empty, so the eviction went with the old fleet.
The rule was already written one command over: `renew_for` refuses a revoked member, and its test
says "the day it becomes reachable is the day a background loop nobody is watching re-papers an
evicted device" — this was the same act with somebody watching, which is worse, because the token
is printed for them.

**Two shapes worth carrying.** The first is that all three of these are *one* mistake with three
faces: **a command said something it was not in a position to say**. The fleet note claimed a
fleet-wide fact from a process that had met nobody; the grant claimed a window it had not started;
the rekey claimed a member list that was one revocation out of date. Session twenty-one's three
were the same sentence about a probe. The second is where the checks went: `Met` and
`split_revoked` are both *seams* rather than assertions bolted to the old shape, because in each
case the path around them needs a terminal, a passphrase or a live mesh — the same reason
`log_source` and `ask_note` were extracted in the two sessions before.

**What to pick up.** 771 tests, clippy clean, wire **v22**, schema **v7**. The unwalked list is
down to one: a **migration read end to end with a real agent** — the front-page claim, and the one
thing a fake agent cannot exercise, because it has no transcript and therefore never resumes. It
costs model time on somebody's account, which is why it is worth deciding rather than starting.
Everything else left needs a phone or a decision, unchanged from session twenty.

## Session twenty-one, in one paragraph

**Three findings, all in what a node *says about itself*, and the first one was also about what
it does.** No wire change, no schema change; the control socket grew two fields, which is local
between a CLI and a daemon built together. The method was the one this file has recommended since
session eighteen — two daemons and a walk down a path nobody had walked — plus the cheap one from
session twenty, *ask what can set this field*, which paid twice in ten minutes.

**The path walked first was ADR-0017's cross-device claim, and it works.** A run submitted from
**beta** (no `host-runs`, one `[[sinks]]` script) and hosted on **alpha**: the agent's question
reached the phone, `offload asks` listed it on both nodes — `WHERE: alpha` on one, `here` on the
other — `offload deny` typed on **beta** was forwarded to alpha, the hook returned `deny`, and the
run's log says `-> denied (an operator on beta)`. Nothing wrong found, which is worth recording
too: the walk is not only for bugs.

**Then the node advertised an agent it was never going to run.** `offload-probe` asked `claude` on
`PATH`; `offloadd` spawns `agent.binary`. So the setting whose entire job is "where the agent is"
decided what ran and nothing about what the device *claimed* — and the two disagree exactly when
somebody uses it, which is why the default (`claude`, both places) hid it for six phases. Both
directions were measured on daemons here. With `claude` on `PATH` and `agent.binary` at a script,
the node advertised `claude-code 2.1.245, authenticated` — a version and an authentication
belonging to a program it would never spawn, which is the over-claim `offload-probe`'s own module
doc exists to prevent: it wins bids and then fails every run at spawn, and a typo in the field is
the same thing. With the identical working install one directory off `PATH`, the node advertised
**no agent at all**, never bid, and refused every submission with `ineligible: has agent
claude-code (not installed)` while `offload status` said `none installed`. The binary is an
argument now, and `deliver::capabilities` is the one place the daemon joins probed facts to
nominated ones — the config is a fact only `offload-node` holds, which is exactly why the join
belongs there and not in the probe. Then the reports, which had the same wrong source: `offload
probe` and `offload match` take `--config` as `offload policy` did last session, each says which
program it asked about, and `offload status` names the binary where it used to say "none" —
the resource line's rule, since "none installed" about a machine that has an agent somewhere else
is a sentence somebody argues with. `offload match "agent=claude-code"` had been answering "this
device satisfies the constraint" a second after the daemon refused the run for want of one.

**And next door, a per-node cap the owner could not set, overriding the one they could.**
`AgentDetails::max_concurrent` is a probe guess of 2, enforced by `admits` and handled by the bid.
On a desktop, server or VM the owner's `max_concurrent_runs` defaults to **4** — so the third run
was refused with "agent claude-code is at its per-node concurrency limit", a per-node limit no
config could express, while `offload status` printed `runs 2/4` and raising the owner's number did
nothing above 2. The constant's own doc says it "is about the account, not the machine", which was
the honest stand-in it was until session twelve built the real thing
(`policy.max_concurrent_account`). It stays a **capability** rather than becoming a policy field,
and the split is the point: how many sessions an install sustains is a fact about a plan and a rate
limit, `max_concurrent_runs` is how much of the machine the owner will give it,
`max_concurrent_account` is what the account may run fleet-wide, and the lowest of the three
happens. Nothing on the machine states the first, so the owner nominates it — `[agent]
max_concurrent`, the sink and resource rule applied to what an agent *is*, validated at
deserialize for `allowed_agents`' reason and refusing 0. And `runs 0/4  ·  claude-code sustains 2,
which is what binds`, said only when the agent's ceiling is the lower one.

**Then `--ask` on a fleet that can reach nobody, which was accepted without a word.** The flag
says "stop and ask me rather than be denied". With no route to a person the question is never put
— correctly; a run stalled five minutes and then answered by the clock is worse than the denial it
replaced — and the run behaves exactly as if the flag had not been passed. Nothing said so: not at
submission with the operator still standing there (ADR-0014), not in the run's log, not in `offload
asks`, only a `tracing::debug!` on a holder nobody is logged into. `--notify push` has answered
this since session nine and `--ask` needed it more, because an audience decides who *hears* and
this decides what the run *does* when it is blocked. Both ends read one predicate now
(`can_reach_a_person`), so a submission and a blocked agent cannot say different things about one
fleet, and a configured route counts before it is known to work — broken or asleep is a route, and
"nobody to ask" is a different sentence. Found by accident, which is the argument for the walk: the
demo was set up to exercise the `--ask=N` budget and the budget looked broken, because with no
sink configured *neither* question was put. With a route it behaves exactly as documented —
question one answered from `offload approve`, question two logged `that was question 1 of 1; the
rest is up to the agent's own rules`, the hook printing nothing rather than a denial.

**Each earned a check that could have found it**, and the shapes are worth copying. The probe one
is asserted over `deliver::capabilities` rather than over the probe alone, because the *wiring* was
the bug, and its fake agent is deliberately **not** named `claude`: reverted, the test reports this
laptop's 2.1.245 and goes red, which was watched rather than assumed. The `--ask` note is a pure
`deliver::ask_note`, extracted for the reason `log_source` was last session — the only path that
reaches it is a submission over a live socket, so a test that could catch its absence has to have
something to call.

**One thing about running the demo.** The fake-agent trick now needs one more line: the node
probes the binary it will spawn, so a stand-in script has to answer `--version` with something
version-shaped. Before this session it worked by accident of the two disagreeing — the script ran
the runs while a real `claude` on `PATH` supplied the capability.

**What to pick up.** 768 tests, clippy clean, wire **v22**, schema **v7**. The list below is
unchanged: everything left needs either a phone or a decision. What this session says about that
is that the *walk* keeps paying at three findings a session, and the unwalked paths left are the
resource proxy (`--use` across nodes, built in session fourteen and not read since), a migration
read end to end with a real agent, and the membership commands' output.

## Session twenty, in one paragraph

**One follow-up, one phase-5 item, a deletion, an ADR, and the demo that found the best bug of the session.** No wire change, no schema change.

**The follow-up, and it went to a different log than the note guessed.** Session nineteen made a
run's position follow the leg its record settled on, which means the reported turn count can now
go *down*: after a fork heals, a run somebody watched reach turn 19 on the losing leg drops back
to the survivor's turn 3. Correct, and indistinguishable from a fault. The note said the
explanation belonged in a typed line in the **run's** log — wrong log, and the reason is in
`offload_core::audit`'s own module doc two paragraphs from where the note was written: a run's log
is served from whoever *holds* the run, so a line written by the leg that just lost it is a line
nobody will ever read. That is why the audit log exists. So `AuditEvent::Superseded`, written
where a leg actually learns it lost (`record_run` seeing a record that names another holder at or
above its own epoch), carrying the turn it had reached — the number that was on screen — beside
who has the run now. Until now that moment was a `tracing::warn!` on a machine nobody is logged
into, which is the exact complaint the audit log was built to answer. Not a `Refused`: nothing was
attempted, this node was told.

**Phase 5's "phone as a worker, gated by owner policy rather than by class", as far as it goes
without a phone.** The item is one line with no elaboration, so the first job was to make it a
checkable claim: is there anywhere a device's *class* decides eligibility rather than its owner's
policy? No — `WorkPolicy::for_class` is defaults and every field is overridable, the capacity
rules and `arbiter_for` are class-blind, and `Stability` (which does come from the class) is a
preference in `score` and in `replica_for`, which falls back to any candidate rather than
excluding one, exactly as CLAUDE.md claims. Two holes, both of them the owner not being heard:

*`offload policy` answered about the wrong policy.* The command whose entire job is "would this
device take work right now?" computed `WorkPolicy::for_class(class)` and reported that — so the
moment an owner wrote a `[policy]` block it described a policy their daemon does not use. A phone
configured `accept = "always"` was told it accepts work only while charging; a battery floor of 80
was reported as 40. The natural conclusion is that the fleet is ignoring you. It takes `--config`
now, the same path `offloadd` takes, through `Config::work_policy` rather than a second copy of
the layering — and it says *which* policy it is answering about, because a class default presented
as the node's is the whole bug in one line. Measured here: `battery floor 80%` with the config
against `20%` without.

*`WorkPolicy::allowed_agents` was enforced and settable by nothing.* Checked by `admits`, gossiped
in the struct, reachable from no config field and no flag — so `Refusal::AgentNotAllowed` was a
sentence no fleet could produce while the field read as a control being applied. It is the one
owner decision a device class cannot express, which is what makes it the field this item is
*about*: "this machine is for light work, not an expensive agent session" is a preference, not a
missing capability. Validated at **deserialize** time, which is the design rather than a detail —
`work_policy` is called from the gossip tick and must not be fallible, and a fallible version's
only safe fallback would be "the owner said nothing", which is *less* restrictive than what they
wrote. An unknown agent name is refused for the reason `Service::from_str` gives in the capability
direction, and an empty list is refused because it reads as a restriction while meaning "refuse
everything" — which is `accept = "never"` said in a way nothing else understands. With one adapter
every run still names `claude-code`, so the knob's useful settings arrive with the second; the
enforcement path is real either way, since `bid::evaluate` passes `run.spec.agent`.

**And a deletion, found by asking whether open question #7 is reachable.** It is not:
`BidWeights::default()` is the only constructor in the workspace, so two nodes cannot bid in
different currencies and there is nothing to make comparable yet. Next door, though,
`bid_delay` and the two weights that fed it implemented ADR-0006 step 3 — bids broadcast after a
score-proportional delay — which that ADR's own implementation amendment replaced **before it
shipped**. Called by nothing but its own test, and its doc comment described the delay in the
present tense, so a reader learned something false about the protocol; ADR-0006's consequences
still named the delay as what mitigates a bid storm, a mitigation by a mechanism that is not
there. Deleted, with the reasoning kept where the fields were, because the next person to want a
delay should have to make the decision rather than find the field.

**And then the demo, which is why this section is longer than it was.** Two sessions had changed
fleet-visible numbers without anybody starting a daemon, so: two nodes on one machine, beta
enrolled by invitation from alpha and *without* `host-runs` so alpha has to host, a fake agent
that writes three files. The numbers travelled correctly — both nodes agreed `3  $0.042  3 new`,
which is session nineteen's position-and-spend split working over the real wire and the real store.
Then `offload audit` said this:

```
2026-08-24 19:25  01a0353c5c78  granted to 9dcd2436 at epoch 1
2026-08-24 19:25  01a0353c5c78  granted to 9dcd2436 at epoch 1
2026-08-24 19:25  01a0353c5c78  granted to 9dcd2436 at epoch 1
```

Three rows, one run, one epoch, nothing wrong with the run. `offload_core::audit`'s own doc says a
pair of these at one number *is* one arbiter having spent a fencing token twice — the failure the
whole epoch scheme exists to prevent. So the first thing this log ever showed was a false report of
the worst thing that can happen here.

The row was written from `Host::record`, whose doc comment describes the arbiter's post-grant
persistence — and `Cluster::learn` calls that same hook for **every** record a gossip merge moved.
So the count grew with gossip: two rows for the first run, three for the second. And the same
hook's other caller is why the row was *missing* in the commonest case — a grant to *this* node
returns before `record` is reached, so alpha submitting its own run recorded `accepted here at
epoch 1` and no grant at all. **On a fleet of one, the audit log had never recorded a grant.**
Over-reported where it must be exact and absent where it matters most, from one hook carrying two
facts, which is the same shape as `Halt::{Cancelled, Superseded}` and `LogKind::CaptureFailed`.
`Host::granted` is the decision now and `Host::record` is the record. Verified both ways round on
the daemons afterwards: one row per grant, and a grant-to-self recorded beside its acceptance.

The storm property that guards it (`an_arbiter_reports_each_grant_once`) needs a four-step script
in order — a grant, a record change, and two ticks to deliver it — so it passes at 24 cases and
shrinks at 300 to `[Place(0), Turn(3), Tick(3), Tick(0)]`, which is exactly the shape the daemons
showed. Worth saying plainly: **the test did not find this and was not going to.** Nine properties
over four real clusters have been run against this code all session and every one passed, because
none of them reads what a machine wrote down about itself.

**Then the rest of the command list, because the first pass paid.** `status`, `explain`, `logs`,
`logs -f`, `deadline`, `priority`, `checkpoint`, `cancel`, `rm`, `sinks`, `drain`, and a refused
submission, on two daemons, from the node holding the run and the node that submitted it. Session
eighteen's work verified in passing: a cancel and a checkpoint both travel and name the machine
("cancelled … on alpha: its agent was stopped"), a drained node reports `drained` in `status` and
a new submission is refused to the operator's face naming every node's reason. Two more bugs:

**A finished run's log was invisible from every node but the one that ran it.** `logs` resolves to
the holder and falls back to the arbiter — and a run with no holder is *two* states. For a pending
run the arbiter is right and the fallback exists for it. For a finished one the arbiter is
precisely the machine that never had the log, and when the arbiter is the asking node the answer
was **silence**: exit 0, no output, which reads as "this run produced none". `logs` had worked on
that run minutes earlier, while it was live and had a holder to forward to. What knows the answer
is `RunProgress::by` — added last session for the merge's ranking — because the leg that wrote the
run's position is the leg that wrote its log.

**And three operator-facing refusals printed with gaps in the middle of the sentence**, including
the one this file singles out as the model of a good refusal. A `\` line continuation in a Rust
literal drops the newline and the indentation; a tool that rewrites source through Python keeps the
indentation, so the spaces end up in the literal. The diff shows one long line and the mangling
reads as formatting — nothing is wrong with the code, and the text is only wrong when printed. Six
of them, one written this session by the same route. Checked now
(`offload-node/tests/messages.rs`), and the heuristic separates mangling from deliberate column
padding cleanly because padding sits near the front of a literal, behind a short label.

**And the property found a bug in the fix it was written for, inside the hour.** Splitting
`Host::granted` from `Host::record` meant reporting on both branches of `offer`; the local branch
reported *before* asking, and `accept` can refuse (ADR-0006 step 6), so a node that declined its
own grant wrote a row saying it had given the run away. `Granted` is the offer the arbiter believes
went through and `Accepted` is the taker taking it — that pair is what tells "took it, answer lost"
from "never got it", and a row for a refused offer breaks the distinction the rows exist for.

**Then the delivery plane, which had never been exercised this session.** A `[[sinks]]` command on
**beta** — the device that cannot host runs at all — and a run submitted from beta, hosted on
alpha, which has no route to anybody. ADR-0010's headline demo, end to end: alpha says "no delivery
routes on this node — it uses the fleet's, below", the phone's script logs `finished after 3
turn(s), $0.0421`, and alpha's `sinks` shows `beta/phone-push … 1 delivered ok` — the sender
keeping the outbox, the peer told what to say and never how. `sinks --test` fires, and audience
selection is real: `--notify none` delivered nothing, `--notify push` delivered.

Three more output bugs from the same pass, all of the same family — **a report that does not say
what the decision says**:

* **`offload probe` printed `power  Battery { percent: 96, charging: true }`**, a Rust debug string
  in a report whose every other line is prose, in two places (the daemon logs the same summary at
  startup). It made the reader do the translation `WorkPolicy` does for them: charging *is* on
  mains for `on_mains` and every battery floor, so `charging: true` beside a percentage invites
  somebody to think the floor applies when it does not. `power_line` now says what the gate would,
  with `Unknown` explicitly *not* folded into mains, and the test asserts each line against
  `on_mains()` so the two cannot drift.
* **`offload fleet --socket <path>` answered about a node nobody asked about** — "This node has not
  joined a fleet", exit 0, one command after `nodes --history` had listed the founding of it.
  `--socket` picks a daemon and about a third of these commands never talk to one, membership being
  answered from the state directory (ADR-0012); the flag was accepted, ignored, and the answer came
  from `$HOME/.offload`. Reachable straight from the recommended setup, since `OFFLOAD_STATE_DIR`
  is how you run two nodes and `--socket` is how you address them. A note on stderr names
  `--state-dir`; `addresses_a_daemon` is exhaustive so a new subcommand must be classified.
* **And a reason written down rather than a change**: `sinks --test` announces a *failure*, which
  looks careless and is not. Every `Notice` is an event about a run and none is "this is a test",
  so whichever is used lies; of the two, "something went wrong" is the safe lie, because a test
  announcing a run had *finished* is the one somebody might believe and then not go and look.

**What to pick up.** 762 tests at the end of session twenty (768 now), clippy clean, wire **v22**,
schema **v7**. Everything left needs either a phone or a decision:

* **Phase 5's remainder is all platform work** — relay/NAT (ADR-0015 defers the library choice and
  the verification needs a real NAT), the Android/iOS host process, mobile capability probing from
  platform APIs, and approvers in secure hardware. None of it is checkable on a laptop, which is
  worth saying plainly rather than starting and discovering.
* ~~**Open question #8, non-agent workloads, needs an ADR before code.**~~ Written, at the end of
  the session: **ADR-0019**, accepted as *intent* and unbuilt — design ahead of code, as ADR-0010,
  ADR-0011 and ADR-0013 were when they were accepted. The shape, the four things it settles and the
  one it refuses are in the open-questions entry above; the thing worth carrying forward is that it
  names **its own smaller half** — `Role::Trigger`, which changes no existing type — as what to
  build if only one of the two gets built. It is a phase 7 item, so nothing is waiting on it.
* **Open questions #6 and #7 are answered** — #6 by ADR-0018, #7 as "not reachable, and answerable
  the day somebody can set them".

Two methods paid this session. The first is the one this file has said pays best since session
eighteen, and it did three times: **run the thing and read the output.** All three bugs it found
were beyond the tests by construction — nine properties over four real clusters pass against the
audit-log bug, because a property checks what the fleet *decided* and that bug was in what a
machine *wrote down about itself*; no test resolves a log across two stores; and nothing at all
reads the daemon's own prose. Two daemons and a walk down the command list found them in an hour.

The corollary is worth keeping: **each one then earned a check that could have found it.** A storm
property over the arbiter's own reports, a pure `log_source` extracted from a path otherwise
reachable only through a live mesh, and a scan of the crate's string literals. The second of those
immediately found a bug in the fix it was written for, which is the argument for writing them at
all rather than trusting the fix that was just measured working.

The second is the cheap one: **ask what can set this field.**
`allowed_agents` was enforced and unsettable; `BidWeights` is settable by nothing and asserts a
protocol; and the same question about `offload policy` is what showed it was answering from a
default rather than from the config. A grep for a field's writers is a five-second question with a
good hit rate on a codebase whose design ran ahead of its code on purpose.

## Session nineteen, in one paragraph

**Phase 6's last open decision, and the bug that answering it turned up.** The item was
"reconsider negotiated arbitration vs `openraft` over `Stable` nodes", left over from ADR-0002,
and the last session named where it should be argued: `storm.rs`, because it can now *produce* a
fork rather than describe one. The answer is **keep negotiated arbitration** (ADR-0018), and the
argument is one ADR-0002's own amendment set up without noticing. There are **two** paths to two
live legs of one run, and quorum removes only one of them. Two arbiters, yes. A grant whose
acknowledgement was lost — ADR-0006's silence-is-a-decline, which makes it the *ordinary* case —
no: the decision is replicated and the side effect is on one machine, and no consensus protocol
reaches across that. Proptest shrank that path to four steps on a fully connected four-node fleet,
`[Swallow(3), Place(0), Turn(2), Turn(3)]`. Meanwhile the price of quorum is a floor that is
either one node — ADR-0002's "fixed coordinator by config", rejected outright — or a fleet that
stops placing work when a laptop shuts. It would also be *additional* machinery rather than
replacement machinery, since every fence stays exactly as it is.

**What the item was actually worth is the bug.** To argue about a fork you have to model what a
leg *does*, so `storm.rs` grew turn boundaries: the capture onto the record, the turn count, and
the worktree summary beside it, published by whichever leg its own view says holds the run — which
during a fork is two of them, each correctly. Then one property: **the position the fleet reports
is the surviving leg's**. It went red, and not on the partition it was written for.

`RunProgress` was merged **forward only** — larger wins, ties on the author's clock — and the doc
comment said why that was enough: the numbers "have a single author by construction: only the node
running the turns produces them, everybody else relays them verbatim". A second grant is exactly
what breaks that construction. Two legs publish two positions; the lost leg started first, so it
gets further, so *its* number is the larger one — and the surviving leg can never correct it,
because for the next sixteen turns everything it says is smaller and refused. A run on turn 3
reporting turn 19, with the losing machine's worktree summary beside it, in the column somebody
reads at 07:00 to find out whether there is uncommitted work. Nothing ever fixes it.

**The fix is that there were two facts in one field.** ADR-0005's table never had a row for a run's
numbers, and filling it in means admitting there are two rows. **Position** — the turn, the
worktree summary — says where the surviving branch *is*, which is a fact about one leg, so it is
settled by the arithmetic that settles the record beside it: the epoch, and at equal epoch the
lowest author id, which has to stay *identical* to `merge_run`'s or a run's numbers describe one
leg while its record names another. **Spend** — money, denials, questions put to a person — is
owned by nobody and folded in field-wise from either leg, because the money left the account and
the person was interrupted whichever leg went on to win. A max rather than a sum: a relay repeats
what it heard, so adding would count the same dollar every gossip tick. That understates a forked
run's bill and is the tightest bound an idempotent merge can give.

So a merged record can now be a **combination** of two legs, which is a shape nothing else in the
view has, and it has a consequence: everything that writes one down has to write the record *as
settled* rather than as it arrived. `Cluster::absorb` was handing the store the incoming copy —
the same mistake it used to make about run records, and the same fix.

**The `holds` check the last session declined was never the fix.** That session found that a lost
leg's turn count can travel and left it alone, on the grounds that the window is short, the damage
self-corrects, and a guard there is a store read on the daemon's one hot path. Two of the three
were wrong once a *fork* rather than a race is the case: the window is the fork, and the damage
does not self-correct. And the guard would not have helped anyway — during a fork **both** legs
pass it, because each one's view says the run is its own, and each one is right. What was missing
was not a guard but a **stamp**: which leg wrote these numbers, so one position can be *ranked*
against another instead of merely compared for size. That is cheaper than the guard it stands in
for, too — the leg's epoch is in memory beside the agent handle.

**And a second bug from the same property, which needs no fork at all.** The comparison was not
**total**. Two legs writing different worktree summaries in one millisecond tie on every field
that was compared, so the merge kept whichever gossip arrived *first* — per node. A fleet at rest,
fully connected, permanently disagreeing about a string, with `offload ps` giving two answers
depending on which machine you ask. The tiebreaks run out somewhere, and wherever that is has to
be something every node orders the same way.

**The harness nearly passed for the wrong reason twice, and that is the part worth keeping.**
First: it published its records **unstamped**, so the merge fell back to the rule for rows written
by an older build, and the property was testing the fix out of existence — green at 600 cases with
the fix fully reverted. `storm.rs` bypasses `Supervisor::update_stats`, which is where the stamp is
written, so faithfulness here meant stamping in the harness too. Second, and subtler: the
*totality* fix alone was enough to make the checked-in counterexample green, because the worktree
summary in the harness names the node that wrote it, so ordering strings happened to order legs.
Reverting the two halves **separately** is what caught both. The generated property is the net; the
case worth reading is arranged by hand beside it (`a_lost_leg_that_got_further_does_not_become_the
_run_s_position`), for `mesh.rs`'s reason — needing "swallow, place, turn on the loser three times"
in that order makes it a deep search for something that reads in six lines. Reverted, it says:
`left: (3, "3 modified on ca93ac17"), right: (1, "1 modified on ed4928c6")`.

**Schema v7, wire unchanged, and the second one is a decision rather than an oversight.**
`RunProgress::{by, epoch}` are two new columns (`progress_by`, `progress_epoch`), nullable and
defaulted rather than backfilled — a row written before this cannot be attributed, and the merge
reads that as exactly what it is, a record whose leg is unknown, which loses to any stamped one and
falls back to the old forward-only comparison against another unstamped one. That branch is
load-bearing for the **store** rather than for the wire. The wire is still **v22**: the body is
JSON and `offload-proto`'s list already has a paragraph saying why `Gossip::progress` does not earn
a bump, so this applies the rule rather than reciting it.

**One more thing the change forced.** `Store::update_stats` is new, and is `update_run`'s sibling
for `update_run`'s reason arriving later: these numbers used to be *replaced*, so a
load-decide-save was writing a decision, and they are **folded** now, so the result depends on what
the row holds at the moment of the write. A turn boundary and an arriving gossip tick are two tasks
doing exactly that, and one of the two contributions would go with no error anywhere.

**What to pick up.** **Phase 6 is done.** Every item is checked except metrics, which is checked
*open* on purpose — the roadmap entry says a counter reporting three rejected epochs today answers
none of the questions this project's users ask, and the audit log was the half of that item with a
stated purpose. Somebody who wants the other half should write down the question first. The three
open questions left are #6 (now answered — ADR-0018 — with only its sub-question deliberately
untaken), #7 (bid weights: cluster config or node preference, and nothing forces it while every
node ships the defaults) and #8 (non-agent workloads). So the next session is either phase 5's
remainder — relay/NAT, a deferred library choice under ADR-0015, and the mobile platform half — or
open question #8, which has a sketch at the end of this file and needs an ADR before code.

Two methods paid this session, and the second is new:

1. **To argue about a failure mode, make the simulation produce it.** The openraft question had
   been "revisit at phase 6" for four months and was answerable in an afternoon once `storm.rs`
   could fork a run — and the answer turned out to be *in ADR-0002 already*, one sentence in its
   own alternatives section ("a Raft leader can still be partitioned from a worker that keeps
   running"). Modelling it is what made that sentence load-bearing rather than a caveat.
2. **Revert each half of a fix separately.** Two halves went in — the leg stamp and the ordering
   being total — and *each one alone* made the checked-in counterexample green, for different and
   equally accidental reasons. Reverting the pair together would have proved nothing; reverting
   both individually is what showed the property was measuring one fix and not the other.
3. **A harness that skips the writer skips the rule.** `storm.rs` publishes progress directly, so
   it never went through `Supervisor::update_stats` — the one place the stamp is written — and its
   records were therefore the *unstamped* kind, which the merge deliberately handles by falling
   back to the old rule. The property passed at 600 cases with the fix entirely reverted. Same
   family as the three "invariant violations" that turned out to be the harness in session
   eighteen, and the tell is the same: the counterexample needs a state the daemon cannot produce
   — here, numbers with no author.

**And one flake, which is a product fact wearing a test's clothes.**
`a_command_about_a_run_elsewhere_names_the_machine_it_is_on` failed roughly whenever its two
`Uuid::now_v7()` calls landed in one millisecond, with a message that points at the id rather than
at what is under test: *"no run matching `01a035052f04` (ambiguous — use more characters)"*. Twelve
hex characters is exactly the 48-bit millisecond clock at the front of a UUIDv7 — that is *why*
twelve was chosen last session — so a displayed prefix names a millisecond and not a run, and two
runs submitted in the same one print the same twelve. `RunId::short`'s own doc comment says "never
use for equality or lookup" and the test was using it for a lookup. Fixed there. The product half
is left alone deliberately: `resolve_run` refusing both is the correct answer, and the residual is
a papercut — the daemon can print an id that a person cannot then paste back. Printing fourteen
characters would reach one random byte and make it a 1-in-256 papercut instead, which is a worse
kind of rare.

Smaller things noticed and not taken:

- **`Cluster::absorb` still skips a progress report for a run this node holds.** Written when a
  peer's copy could only be stale zeros, and now redundant: a previous leg's numbers are at a lower
  epoch and lose on their own, and a *concurrent* leg's at a higher one should win, which the skip
  prevents until the record merge tells us we have lost the run — a tick later. Harmless, one gossip
  tick of our own numbers, and it is a filter by holder status of exactly the shape this project has
  been burned by twice (`fetch_blob`'s `Alive` sweep, `cancel`'s process table). Worth removing the
  day something makes it matter.
- **A forked run's bill is understated.** Spend is the highest any single leg reached, not the sum,
  because a relay repeats what it heard and adding would count the same dollar every tick. The
  honest number needs per-leg accounting, which is a field per leg on the wire to fix a report
  nobody has yet complained about.
- ~~**Nothing displays which leg a run's numbers came from, and the number can now go down.**~~
  Taken, in session twenty, and it went to a different log than this entry guessed. The symptom is
  the change working: after a fork heals, a run reported at turn 19 by the leg that lost drops to
  the surviving leg's turn 3, which is the truth and looks like a fault. The guess here was a
  typed line in the **run's** log — and a run's log is served from whoever *holds* the run, so a
  line written by the leg that just lost it is a line nobody will ever read. That sentence is in
  `offload_core::audit`'s own module doc, two paragraphs from where this was written, and it is
  the reason the audit log exists. So: `AuditEvent::Superseded`, per-node, carrying the turn the
  leg had reached — the number that was on screen — plus who has the run now and under what
  epoch. No schema change; a new `kind` value in a column that already exists.

## Session eighteen, in one paragraph

**The follow-up the last session named, and a bug it turned up.** `offload cancel` acted on the
machine it was typed at. Session seventeen made it *say so* — "run ab12… is running on desktop,
not here" — and left the forwarding, because unlike `offload approve` and `offload deadline` it
needed a cluster message that did not exist. It exists now: wire **v21**.

**The routing has one thing `answer`'s does not.** An answer goes to the holder, because a
blocked process is on one machine and a run nobody holds has nothing waiting. A cancel has two
kinds of target: a run somebody is **holding** is on that node, agent or no agent, and a run
nobody holds is a **record**, which belongs to the node that arbitrates it — `arbiter_for`, the
same rule a `SpecEdit` uses and for the same reason. So `--queue`'s pending run is cancellable
too, which it never was.

**And the process table was never the question.** It cannot tell a run that is *elsewhere* from a
run that is *here and has not started*, and it answered both "not running" — so a commitment this
very node was holding, and would have begun the moment a slot freed (ADR-0006), read as a run
that did not exist. Three outcomes now, and the note says which: an agent stopped, a commitment
given up, a record closed. A person who cancels a mid-turn agent has spent money and a person who
cancels a commitment has not, and "cancelled" alone does not tell them apart.

A run whose holder has gone quiet is **refused rather than forwarded into a timeout**. `Orphaned`
is an observation and not a decision (ADR-0007), an agent may well still be running there, and
cancelling the record from here would leave it running with the fleet convinced otherwise. The
refusal says what will happen instead: the hold-down reclaims it or reassigns it, and the cancel
reaches whoever answers next. Same trade `arbiter_for` makes when it prefers a stalled run to a
duplicated one.

**The bug on the way, and it is the more interesting half.** Making the channel carry *who asked*
exposed that it was carrying a decision it had no business making. Stopping an agent here and
ending a run were one signal: the cancel channel was the only way to take an agent down, so
whatever came down it was recorded as `Cancelled`. A node that had just **lost** a run — the
equal-epoch grant the churn properties found in session fifteen — stopped its agent, which is
right, and then wrote a terminal state into the record `record_run` had a moment earlier
overwritten with the new holder's copy. Unfenced, because an operator's cancel always wins; at
*their* epoch; and terminal, so it beat the live record everywhere `absorb` does not refuse a
peer's word about a run it holds. The fleet said cancelled, the agent worked on, and the operator
was told their run had stopped. `Halt::{Cancelled, Superseded}` separates the two, and the same
rule decides the log: `LogKind::Cancelled` is terminal to every follower, so a leg that merely
lost its run writes nothing at all.

That one is reproduced rather than asserted. The existing equal-epoch test only registers a run
and reads the channel, so it can check *which* halt was sent and never what was written — the
store assertion beside it is a guard that cannot go red, which is worth saying rather than
tidying away. So there is a second test that actually drives a run: a git repo, an agent script
that prints two events and sleeps, and the merge arriving while the agent is up. Reverted to
watch it: `left: "cancelled", right: "running"`.

**`LogKind::Cancelled` grew a `by`.** The wire already carried the name of the node the command
was typed at — `ClusterMessage::Answer`'s reason, so the desktop's log says the run was approved
from the phone — and the cancel was receiving it and dropping it on the floor. A field carried and
discarded is its own small lie. `offload logs` says `cancelled from beta`.

**Two things about the version number, found while bumping it.** v20 has no entry in
`offload-proto`'s list: the commit that made the change (a field on a certificate, so every device
re-joins) moved `VERSION` and wrote no paragraph. And it did not move `MIN_VERSION`, whose own doc
comment says the two are equal and that widening the range is a deliberate edit — so the range
quietly started admitting a v19 peer whose certificates this build cannot verify, refused a moment
later for a membership reason, which is a confusing message rather than a fault. Both are the same
lesson: that list and that constant are the whole of what the number means, and a bump that
touches only the number has not been made. There is a test now that fails if they part company.

Verified on two real daemons with a fake agent: submitted from `beta`, accepted by `alpha`,
`offload cancel` typed on `beta` → `cancelled … on alpha: its agent was stopped`, `ps` on alpha
says `cancelled`, its log says `cancelled from beta`, and no agent process survives. Plus the
three refusals: already cancelled, no such run, and the second run on alpha for good measure.

**Then phase 6's last unchecked item, and the bug it found.** `offload-cluster/tests/storm.rs`:
a generated script — ticks, partitions, isolations, bid rounds, declines, lost acceptances —
against four **real** `Cluster`s on the in-memory transport. `churn.rs` names the limit this
closes in its own header: *"a node holding a run means its own view says so."* Here the nodes
encode gossip and decode it, handshake, run the real SWIM detector, and place runs through the
real bid round.

Not `turmoil`, and the reason is worth recording: every seam it would provide already exists.
`probe_round(now)` takes the clock as an argument, `MemoryNetwork` cuts and heals at an exact
instant, and the transport is behind a trait. What `turmoil` adds is a simulated *socket*, which
is `quinn`'s problem rather than this crate's. Clock skew is the one axis left un-varied, on
purpose: it exercises `Millis` arithmetic the core's own properties already drive, and would make
every failure here ambiguous between two layers.

**What it found: a refused round handed its tokens out again.** "A grant spends a token whether or
not it is confirmed" was fixed *within* a round in session fifteen — one record threaded through
the attempts — and nothing carried it across the boundary. `place` works on a local copy and
publishes only a **confirmed** grant, so a round in which nobody confirmed left the arbiter's view
at the epoch it started from, and the next round counted from there and re-issued the same
numbers. Silence is a decline (ADR-0006), so the nodes that had taken those grants and lost the
answer were running under them. Two grants at one epoch from one arbiter is precisely what the
epoch exists to prevent: `fence` cannot order them, neither agent is ever told it lost, and
`merge_run`'s equal-epoch tiebreak is left holding a decision it was only ever the backstop for.
A refused round now publishes its record at the epoch it reached.

**Three of its first five "violations" were the harness, and that is the more transferable
lesson.** In order: a stub host that published runs it had only been told to *record* (the real
`Mesh::record` writes to the store and lets gossip move it, under the epoch rules); a `place` that
conjured a fresh run when the arbiter had not heard of one, minting two records under one
`RunId`; and — the instructive one — letting **any** node place a run. That last found a genuine
sequence: a node granted the run at epoch 1 while another already held it at epoch 3, its own view
walking backwards to take it. Real, and reachable only by breaking the rule the simulation exists
to model, because an epoch is monotonic *per arbiter* and two arbiters are two counters. The tell
in every case: the counterexample needs a step the daemon has no code path for.

**And the fixpoint was declared a rotation too early.** SWIM probes one peer per period, so a
node's opinion of any given peer is revisited once every `peers` rounds — two identical passes are
the ordinary state of a fleet mid-recovery, not evidence of convergence. The eager version called
a fleet settled at round 1 while a node it had marked `Dead` came back at round 4. It waits a whole
rotation of quiet now. That is `churn.rs`'s own lesson arriving from the other end: its first
version stopped when the run record stopped moving and reported a fleet still arguing about
liveness as converged.

Each fix was reverted to watch its property go red, and the epoch one has a named regression test
beside the within-a-round case it extends (`a_round_that_failed_does_not_hand_its_tokens_out_again`):
`the arbiter went back to a record at epoch Epoch(0) after spending [Epoch(1)]`.

**Then a sixth property, and a second bug.** The first version of `storm.rs` moved no blobs, which
left ADR-0016 — the reason any of this matters, since a checkpoint is the work — outside the
generated coverage. With a blob store, a `Store`/`Push` pair of events and the claim *a checkpoint
any reachable node holds can be fetched*: `fetch_blob` swept only peers it marked `Alive`.

Its own doc comment is the argument against that filter: availability is not gossiped **because**
an advertisement would be stale by the time it was used, and a node without the bytes answers in a
millisecond. A node's liveness is exactly as stale — and the moment a checkpoint is needed is the
moment somebody has gone quiet, so the filter skipped precisely the peers most likely to hold it.
`Suspect` is not unreachable; the state exists to be argued with. So the fleet's only copy could
sit on a node that answered every dial and never be asked, and the migration failed with `no peer
could supply the blob`. The code had already conceded the point for the `from` hint, which is asked
whatever its status; it was the rest of the sweep that filtered. Ordered now — hint, then live
peers because they are likelier, then everyone else.

Worth knowing about that property: it needs a **deeper search than the default**. The window is
cut, advance, tick until suspicion, heal, ask — five specific steps in order — so 24 cases pass
with the fix reverted and 600 shrink it to `[Isolate(1), Store(1), Tick(3), Heal(3, 1)]`. That seed
is checked in, so it now fails at the default depth: the deep run has to happen once. `mesh.rs` has
the same case arranged by hand, which is what makes the failure legible.

**The sweep's own find: `offload checkpoint` was the last command acting only where it was
typed.** Its refusal was not false the way cancel's was — "it is not running here, so there is no
turn boundary coming" — which is exactly what made it survive the session-seventeen pass: it names
*this* machine and stays silent about the one the run is on, so the operator is told the truth and
still has nowhere to go. Wire **v22**.

One kind of target rather than the cancel's two. A cancel has a record half because a run nobody
holds is still a record somebody owns; a checkpoint has none, because what is being asked for is a
pause in a *process* at a moment only that process reaches (ADR-0004). A run nobody holds has no
boundary coming and is already back in the pool — which is where a checkpoint would have put it —
so that is the sentence, rather than a forward into nowhere.

Verified live in both directions: `offload checkpoint` typed on `b-node` for a run on `a-node`
answers `checkpoint requested on a-node — it will be taken at that run's next turn boundary`, and
`ps` on `a-node` shows the run `checkpointing`; the same command after the run is cancelled answers
`nobody is running … — it is cancelled, so there is no turn boundary coming and nothing to hand
back`.

**And a third finding, from running the demo rather than reading the code.** `offload drain` did
nothing at all. `Mesh::drain`'s own doc comment has said "and stop accepting new ones" since it
was written, and nothing implemented it: it handed its held runs over and returned. So the drained
laptop went on bidding and winning — measured, one second after `offload drain` said "nothing to
hand over" — and the run it had just moved to the desktop could come straight back to the machine
that was about to be closed, which is the whole of what a drain exists to prevent.

Refused at the **bid** and at the **grant**, not just the bid: a grant can arrive after the bid
that earned it — a round answered a moment before the drain began, or a peer acting on what it
last heard — and refusing only one of the two makes the bid's promise true merely usually. Both
checks come first, before the cluster is even consulted, which is what the test can rely on. And
it is reported: `offload status` says `accepting no — drained; restart offloadd to take work
again`, because a node that silently takes nothing is the same symptom as a broken one. One-way
and in memory on purpose — a drain is a departure, so the way back is starting the daemon again.

Verified the way it was found: drain `a-node`, submit from `b-node`, and instead of "accepted by
alpha" the answer is `no node will take this run / fedora draining / b-node not granted
host-runs`.

**A fourth, from walking the command list against a live fleet.** `offload ps` reported a
running run's worktree as `preparing`. `RunProgress::workspace` is documented as "the holder's
summary of the run's worktree — 2 modified, 1 new", and it was written exactly twice in a run's
life: `preparing` when the leg starts, and the truth once the leg has *ended*. Everything in
between — which is the whole of a run — said the checkout was still being built. It sits beside
the `SAFE` column, so the pair somebody reads at 07:00 was "half-built, not replicated" about a
worktree full of finished work, and it **gossips**, so every node in the fleet said the same.

Refreshed at every turn boundary, and at the agent's start so the first turn is not misreported.
Deliberately not at every *checkpoint*: the cadence is configurable, so that would be two turns
stale under `every_turns = 3` and permanently wrong under `every_turns = 0` — the bug unchanged
for anybody who turned automatic captures off. One `git status` per turn, against minutes of model
time. Reverting it prints `left: "preparing", right: "preparing"`.

The stale comment above `cancel_all` in `main.rs` went with it: it said runs "are cancelled rather
than migrated. Phase 4 replaces this with a drain that checkpoints at the next turn boundary",
while the ten lines directly above it had been doing exactly that for four sessions — and after
this session's `Halt::Superseded` change the first half is wrong too.

**And a fifth, which the fourth walked straight into.** With the demo daemon still up, `cargo
test` failed four capacity tests — and the first instinct was that the workspace change had broken
something. It had not. The device reservation ledger is per-user and per-machine *by design*
(ADR-0013's one laptop, two fleets, one set of slots), so `Supervisor::new` opens the real one and
every capacity test in `offload-node` was reading whatever `offloadd` happened to be running on
the machine executing the suite. `two_commitments_on_a_one_slot_node_do_not_block_each_other`
reported `left: 0, right: 1` about a slot an unrelated agent was holding.

It runs the other way as well, and that half is worse: a test that *drives* a run reserves against
that same ledger, so `cargo test` could tell a live daemon its machine was full. Anybody
developing this project runs both on one laptop, which is the whole reason `OFFLOAD_STATE_DIR`
exists — and the ledger is the one piece of state that deliberately ignores it.

`Supervisor::with_private_ledger`, the sibling of `with_home`, whose comment already said the rule
in as many words: "a test's temporary directory must not be quietly replaced by whatever
`$CLAUDE_CONFIG_DIR` says on the machine running the suite." Verified by running the suite with a
daemon up and a run in flight: 196 passing where four failed.

**And a last small one, found while looking at whether fence rejections are observable at all**
(which is the audit-log item's substance). `finish_cancelled` discarded the result of the
transition it was recording. `Run::cancel` is unfenced but still refuses a terminal run, and the
window where that happens is the one somebody is most likely to be typing in: the agent reports its
result, the run goes `Completed`, the stream has not closed, and the pump's *biased* select picks up
a cancel that arrived a moment ago. Refused, correctly — and the log line was written anyway, so a
run that had just succeeded had `cancelled` as the last word in its own log. The neighbouring
`Finished` path already got this right and shouts on a fencing rejection; this one had no check at
all.

**And the audit-log item's first real find, which is why it was worth starting there.** The
question — is a fence rejection observable at all — turned into: what does the *caller* do with
one? `TransitionError` already distinguishes a fencing refusal (`StaleEpoch`, `NotHolder`) from a
benign state race, and `drive`'s two guards are correct: neither spawns an agent for a run this
node has lost. `launch` then treated that refusal like every other error from `drive` and called
`fail`, which uses `abandon` — unfenced on purpose, because a run whose workspace could not be
built has no holder to fence against.

For this one error it is exactly backwards. The row by then is the **new holder's**, at the new
holder's epoch, so `Failed` is terminal, beats the live record everywhere, and `note_failure` makes
the run a candidate for auto-resume. Somebody else is running it, the fleet says it failed, and
recovery is entitled to start a second agent: double execution, reached through the fence firing.
`SubmitError::LostTheRun` is its own variant so the caller cannot confuse the two.

It survived four phases for a reason worth repeating: the existing test
(`a_run_that_moved_on_while_its_workspace_was_being_built_never_gets_an_agent`) calls `drive`
*directly*. It proves the guard works and never goes near the function that calls it — so the whole
of what happens to the refusal was untested. **Testing the unit under test is not the same as
testing the path that runs it.**

**And a third door into the same room, which is what makes it a pattern rather than a bug.**
`fail_if_unfinished` exists because an agent that dies without a result leaves a run `Running` for
ever — real, and session eight's fix for it asked the wrong question. `still_ours` tested the run's
*state*, `Running` or `Checkpointing`, which is about what an agent would be doing and not about
whose run it is. A reassigned run is `Running` too, on somebody else's machine. So a leg whose
agent crashed at the moment it lost the run wrote `Failed` over the new holder's record: unfenced,
at their epoch, terminal, winning everywhere while their agent worked on. Reachable by ordinary
luck rather than an arranged race — `record_run` asks the agent here to stop, and an agent that
dies of its own accord first makes the pump report `Finished` rather than `Halted`.

Four doors, one room, all found in one session: `finish_cancelled` writing `Cancelled` for a
superseded leg, `launch` writing `Failed` for a refused one, `fail_if_unfinished` failing a run
that had moved, and — on the *numbers* rather than the states — `refresh_workspace_summary`
publishing a lost leg's own checkout as the run's, which wins the fleet because progress ties break
on the author's clock. The shape they share is worth more than any of them — **the guard is right
and the caller is wrong** — and so is the reason they hid: each guard has a test that drives *it*,
and none of them go near what happens next. `Supervisor::holds` (holder **and** epoch) is the
question all four were getting wrong in different words — and it moved into the **writer**, so the
next path somebody adds cannot forget it: `fail` refuses a run this node does not hold. Proved
structural by deleting `launch`'s own guard and watching the test still pass; the guard was put
back, because it does two things `fail` cannot — logs at the right level ("run failed" is the wrong
sentence for a leg that lost a race) and lets go of the process handle.

**And then the item itself: the audit log** (`offload_core::audit`, schema **v6**, `offload audit
[<run>]`). A third log, and the reason it is not one of the other two is worth keeping: a run's log
is the run's *output*, served from whoever holds the run — so a line written by a leg that has just
been fenced **out** is a line nobody will ever read, because that node is by definition not the
holder. And this is not about the fleet. It is what *this machine* did: per-node, never gossiped,
for `fleet_events`' reason (two nodes recording one grant are not disagreeing, they are each
describing what they saw, and merging them would need an owner for an event neither owns).

Three kinds — `Granted`, `Accepted`, `Refused` — and the epoch is on all three, because two grants
at one number from one arbiter is the failure the whole scheme exists to prevent and nothing
durable recorded it. It outlives the runs it describes (no foreign key on `run_id`), since the rows
worth reading are about a run this node has stopped holding.

Two things it taught while being built. **The acceptance was in the wrong place**: recorded at
`take_run`, which a node with no mesh never reaches — so the commonest configuration in this
project recorded nothing at all, and `offload audit` said "nothing recorded" for a run that was
happily going. It is at `start_run` now, the one choke point every started run passes through, with
the commitment branch of `take_run` recording separately because a commitment is an assignment too
(ADR-0006). And **collapsing two questions produced a false row**: a *completed* run reported as a
refused write, because a finished run has no holder and the guard read that as a stranger.
`holds` ("may I conclude it") and `describes` ("may I say what its worktree holds") are separate
now — the epoch tells "ended here" from "moved away" where the holder cannot, since the terminal
transitions are fenced. That also restored the final worktree summary, which the guard had quietly
stopped writing.

**Metrics and the Prometheus endpoint are left open on purpose**, and the roadmap says why rather
than leaving it looking unfinished: nobody scrapes their phone, and a counter saying three epochs
were rejected today answers none of the questions people actually ask here. The audit log was the
half of that item with a stated purpose.

**What to pick up.** Thirteen fixes — twelve product bugs and one piece of test hygiene — and the
methods are worth more than any of them. Three of them, in the order they paid:

1. **Take the follow-up, run the thing, read the output.** Two came from forwarding a command that
   had never travelled, one from a demo that answered "nothing to hand over" and meant it
   literally, one from two adjacent lines of output disagreeing about the same run id, one from a
   column that had said `preparing` for four phases.
2. **Simulate the real code, and revert each fix to watch its property go red.** Two bugs, and the
   red-watching is what caught a property passing for the wrong reason.
3. **Ask what the *caller* does with a refusal.** Four, all in one room. Two came from forwarding a command that had never
travelled, one from a demo that answered "nothing to hand over" and meant it literally, one from
reading two adjacent lines of output that disagreed about the same run id, one from a column that
had said `preparing` for four phases, and one from the suite failing for a reason that was not the
change under test. None needed a new tool.

The rest of the command list is exercised and behaves: `probe`, `policy`, `match`, `fleet`, `id`,
`invite`, `join`, `nodes`, `status`, `run` (including refusals under load and on probation), `ps`,
`logs` and `logs -f` across nodes, `explain` from a node that does not hold the run, `deadline` and
`priority` forwarded to the owner, `cancel`, `checkpoint`, `rm`, `resume`, `asks`, `approve`,
`sinks`, `drain`. Worth repeating after any change to routing, and worth doing on a machine that is
not also compiling: this laptop's load average refused every `normal` run for several minutes and
`--demand light` was the way through, which is the pressure rule working exactly as ADR-0013 says
and a surprise the first time.

What is left in phase 6 is **metrics and an audit log**, and the `openraft` reconsideration (open
question #6). The simulation item is done, by the route described above rather than with
`turmoil`; `storm.rs` is also where open question #6 should be argued, because it can now
demonstrate the two-arbiter case rather than describe it. Two smaller things noticed, one taken:

- ~~**`RunId::short()` is 8 hex characters, and `CLAUDE.md` says displayed prefixes are 12.**~~
  Taken, and it was one number. UUIDv7 bytes 0..6 are a 48-bit millisecond clock, so bytes 0..4
  are the top 32 bits of it and hold still for 65_536 ms — "run 01a02606 cannot be cancelled"
  named every run of the preceding minute, beside a CLI that had printed twelve characters of the
  same id on the line above. The rule lived in the CLI's own `short_id` and not in the type, so
  the daemon's refusals disagreed with everything around them. `byte_id!(RunId, 16, 6)`, which is
  exactly the whole clock; `NodeId` and `BlobHash` stay at four, because an ed25519 key and a
  BLAKE3 digest have uniform leading bytes. **Nothing in the tree checked the length**, which is
  why it drifted: 730 tests passed before and after the change. There is a test now, written
  against the layout the length depends on rather than against a minted id (generation lives
  outside `offload-core` by decision), and reverting it prints `left: "01a02605", right:
  "01a02605"`.
- **A lost leg's turn count can still travel, and is not guarded.** `gossipable_progress`
  publishes stats for every run this node has numbers for, holder or not — deliberately, since a
  node that only submitted a run has the record and none of the numbers — and `record_event`
  writes `stats.turns` on every turn boundary with no `holds` check. So an agent that emits one
  more boundary in the window between losing its run and being halted publishes a turn count for
  somebody else's run, and the forward-only merge then refuses the surviving leg's smaller number
  until it catches up. Left alone on purpose: the window is short (the halt is a biased select
  away), the damage is self-correcting within a turn or two, and a `holds` check on that path is a
  store read *per agent event* — the one hot path in the daemon. The cheap fix would be an epoch
  on `RunProgress`, which is a wire version and a decision.
- **`offload drain` cannot be undone.** Deliberate, and written down where the flag lives, but the
  first person to drain a laptop and then decide not to close it will want `offload undrain` — or
  will restart the daemon and lose nothing, which is the argument for leaving it alone.
- **A `SIGTERM`'d daemon holds its state directory for up to `drain_deadline_secs`** — five
  minutes by default — because shutdown drains first, and a run mid-turn is waited for. Correct,
  and it means `pkill offloadd; offloadd` fails with "another offloadd is already using …" and the
  *old* daemon answers the next command. Cost twenty minutes here, and belongs beside the
  `pkill -f` note in the demo section rather than in the code.

## Session seventeen, in one paragraph

**`offload-store`'s writers, which the last session named and left unanswered.** One commit, no
wire or schema change. The question it was left with — "`save_run` is last-write-wins over a
whole row, the supervisor writes it from several tasks, do any two of those windows overlap
across an `await`" — has a cleaner answer than expected: **none of them do, and two of them
never needed to.** An `await` is not what makes the window; holding a copy is. Two tokio tasks
and one store are enough, which is exactly why "the load and the save are three lines apart"
reads as a defence and is not one.

`Supervisor::replicate` is the one writer that had it right, and it says so in a comment: "turns
keep happening while bytes are in flight, and writing back a stale checkpoint would undo one".
It re-read the row before writing it. Nothing else did, and nothing in the tree could tell the
two apart.

**The heartbeat renews leases from a listing taken before its loop begins.** A run that finishes
while the loop is working through the rows ahead of it is written back as `Running`, from the
copy the listing handed over — and then *kept* that way, because the same loop goes on renewing
the lease of the run it has just resurrected, and nothing orphans a holder that is heartbeating.
A run in the store for ever, with no agent behind it and no fault recorded anywhere. The other
loser is a checkpoint: the blobs go unreferenced, and the collector is entitled to take them.

**And the gossip path, which was the last session's own fix.** `absorb` refuses a peer's record
for a run this node holds and takes the two fields on it the home node owns — right, and honoured
in the view. What it then handed *down* to be written was the whole record, and the view's copy
of a run this node is running is whatever the last gossip tick published. So `offload deadline`,
arriving a second after a turn boundary, wrote the run back as it stood before the checkpoint:
nineteen turns of somebody's night, undone by moving a deadline, with the log saying the edit had
been applied. The split is complete now — `merge_run` for the record, `merge_spec_edit` for the
edit, `Host::record_spec_edit` for the edit **travelling as an edit**, and
`Supervisor::apply_spec_edit` applying it to the row rather than carrying it in on a copy of one.

`Store::update_run` is the mechanism: read, change, write, under the store's own lock, with the
window between the read and the write no longer existing. Ten writers moved onto it. Two stayed
on `save_run` deliberately and the reasons are worth keeping: a record arriving from the fleet is
*meant* to be written whole (`record_run`, `take_run`, `start_run` — that is what a grant is),
and `resume` validates a dormant run against a room check that would deadlock under the store
lock, on a row no agent is writing. Nothing inside an `update_run` closure may touch the store,
for the same reason; it is holding the lock.

**What is tested, and what is not.** The store's own claim is a property over real threads
(`offload-store/tests/writers.rs`): concurrent read-modify-writes lose nothing. Beside it, the
same interleave written out deterministically in both shapes — a `save_run` that restores a
moment, and an `update_run` that does not — because asserting the *bug* would be asserting a
race. The gossip half has a regression at the seam that had it (`apply_spec_edit` keeps the
checkpoint and takes the edit), reverted to watch it go red: nineteen turns, gone. The
heartbeat's own interleave is not directly tested and cannot easily be: it is a race between two
tokio tasks, and what is checked is the mechanism it now uses rather than the timing it no longer
depends on.

**Then the second surface, `offload-node::server`'s forwarding.** The question was whether any
command is applied locally *and* forwarded. Two of them are neither. `offload approve` and
`offload deadline` route properly — apply-here XOR forward, each with the reasoning written down
— and `offload cancel` and `offload rm` were left behind when they were built.

Cancel reads this node's *process table*, so for a run on another machine it said "run ab12… is
not running": a false sentence about a run that is running, spending money, with nothing
forwarded and nothing to tell the operator where to go. It names the machine now. Forwarding it
is a **new cluster message and therefore a wire version**, which is why it is named as a
follow-up rather than done here — unlike approve and deadline, which ride messages that already
existed.

`offload rm` was the worse one, because it **succeeded**. `WorkspaceManager::remove` returns `Ok`
for a path that is not there — idempotent teardown, from phase 1, and right — so `cleanup` went
on to write `workspace: "removed"` into this node's numbers for a checkout on another machine.
Those numbers gossip: `gossipable_progress` sends every run this node has stats for, holder or
not, and `accept_progress` compares (turns, cost, denials, asks, `at`) with the clock last — so a
copy that ties on the counters and carries a later stamp is *ahead* and wins everywhere. The
fleet then reports a worktree as removed while it sits untouched on the desktop, and the operator
is told it worked. A finished run **names no node** (the lease goes with the terminal
transition), so "is it here" is a question only this node's own disk can answer:
`Removal::{Removed, NothingHere}` is that question being asked, and the note is written only
where the disk changed. Reverted to watch it go red: `left: "removed", right: "3 modified, 1
new"`.

**And the third surface, `offload-probe`, which was wrong on the machine this was written on.**
The question was what it reports when a file it reads is present but says something else. The
answer: `power::detect` was reporting **the touchscreen's battery as the laptop's**. Four entries
under `/sys/class/power_supply` here — `AC0`, `BAT0` at 98%, an ELAN touchscreen with a phantom
battery at 0% and `present=0`, and a Logitech receiver whose `capacity` file is empty — three of
them `type=Battery`, and the loop assigned to one slot per battery it saw, so whichever name
`read_dir` yielded last won. `offload probe` printed `Battery { percent: 0, charging: true }`.
It prints 98% now.

The consequence is not cosmetic in either direction. Unplugged, that node reports 0% and every
battery floor in the fleet refuses it while it has 98% charge — the same shape as the plugged-in
laptop that refused every checkpoint replica, which is already in `CLAUDE.md`. And the reverse —
system battery at 15%, a freshly charged mouse at 90% — accepts work and dies mid-run, which is
the over-claim the whole crate exists to prevent. Worst of all it is **not deterministic**:
`read_dir` is filesystem order, so one machine can answer differently across two boots.

The kernel has the discriminator and nothing asked it: `scope=Device` on a peripheral, absent or
`System` on the machine's own (`BAT0` here has no `scope` file at all, which is why absent has to
read as the machine's), and `present=0` for an empty bay. `settle` is now a pure function of the
parsed directory, which is what makes the order-dependence testable at all — no test against a
real `/sys` can arrange it. Several system batteries report the **lowest**: wrong arithmetic (an
energy-weighted average is the right one) and the right direction. And a system battery whose
capacity will not parse is `Unknown` rather than `Ac`, because `Ac` claims there is nothing to
run out of — the module's own header said that all along and the one code path that mattered did
not honour it.

Three of the seven tests go red on reverting the scope filter, the first of them reproducing the
live symptom exactly: `left: Some(Battery { percent: 0, charging: true })`. The order-independence
one does *not* go red, and that is worth knowing rather than papering over — the new shape has
that property by construction, so it guards the future rather than reproducing the past.

The rest of the crate was asked the same question and answers it honestly:
`account_uuid_in` returns `None` on unparseable settings (and the node then claims auth with no
account, which is the documented, stated cost), `cpu_load_percent` returns `None` rather than a
plausible zero, `chassis_type` that will not parse falls through to the battery hint, and the
disk, memory and core counts all fail *downward*. The one remaining assumption is
`metered_network = false`, which is declared rather than probed and belongs to phase 5's mobile
work.

**What to pick up.** The surface list is empty again — that is twelve findings from the sweep, and
nineteen counting sessions fifteen and sixteen. What is left in phase 6 is the part that is not a
sweep: `turmoil`, metrics and
an audit log, and the `openraft` reconsideration (open question #6). And the follow-up this
session named rather than took: **cancelling a run that is somewhere else**, which is one cluster
message and a wire version. *(Taken in session eighteen.)*

## Session sixteen, in one paragraph

**The two surfaces the last session left named, and both were losing work.** Three commits, no
wire or schema change in any of them. The recipe was session fifteen's, unchanged: pick a
surface, write down in one sentence what the tree claims about it, make that sentence a
property — and read the code *around* it, which is again where the worst one turned up.

**A run that came back was resumed from the leg before it left.** `WorkspaceManager::adopt` had
this in as many words: "if it is here, it is by definition at least as current as the last
checkpoint — the agent stopped, the files did not move". True of the run that never left. False
of the one this project exists for, because **nothing removes a worktree when a run leaves a
node** — `remove` is called only from `cleanup`, which is deliberately manual and only for
terminal runs, so the earlier leg's checkout sits there indefinitely. So: checkpoint here at
turn 1, hand over, nineteen turns elsewhere, come home. `adopt` returns the turn-1 checkout, the
bundle and patch are never applied, and `install_transcript` skips for the same reason at the
same path — a transcript's path is derived from the worktree path, so leg one's copy is sitting
exactly where this leg looks. The agent resumes turn 1's conversation on turn 1's files. The log
says "adopted in place". Every one of those nineteen turns is still in the blob store, and
nothing will ever apply them.

Turn counts are the one thing that orders two legs of state with **no clock and no message** —
they continue across a migration because they belong to the run rather than to a process, which
is a rule this tree already had (`accumulate_turns`, and `RunProgress` beside the record rather
than in it). So a worktree records the turn it holds, written after a capture *and* after a
restore — the two moments where what is on this disk changes what a later resume may adopt — and
`adopt` answers `Current | Superseded | Absent` rather than an `Option`. Equal turns is current.
**Unknown is not current**, which is the third time that sentence has been the right answer in
two sessions (the collector's undecodable row, `AttendanceUnknown` refusing to auto-resume): a
run in flight across this upgrade has no marker, and the confident reading is the unsafe one. It
costs one rebuild.

And a superseded checkout is **moved aside, not deleted** — it may hold the only copy of a
mid-turn edit, and this is the module where uncommitted work is the valuable part. The rename is
also what frees the run's branch, since git refuses two worktrees on one branch; `prune` drops
the stale registration; and the path is named in the run's own log through `Resumed.workspace`,
which is exactly what that field was for ("how it came back is what matters when a resumed run
turns out to be missing something").

The marker goes *beside* the worktree rather than in it. A file inside would be untracked, and
would therefore turn up in `git status`, in the checkpoint's own untracked selection, and in
every patch the run ever produced.

**Then the untracked-file policy, which is where the properties were pointed in the first
place.** Six of them, each a sentence the module already claims: everything accounted for
exactly once, the limits are limits, git's listing order decides nothing, a full budget
sacrifices the biggest, and — as a comparison rather than an adjective — a more generous policy
never carries less. That last one is worth keeping: the selection is a greedy prefix over a
sorted list, and a bigger per-file limit puts *new* files into that list, so monotonicity in the
thing being spent is not obvious. It holds.

They found the reporting bug immediately: `summary` counted build output and then called
everything else "skipped as too large", so a file left behind because the *checkpoint's total*
budget was full was reported as one that was too big. Two settings, and the one line the
operator gets named the wrong one. Same arithmetic as the delivery plane's delivered/waiting/
dropped, on the other plane.

**And then the bigger one, which closes open question #3 by measuring rather than waiting.**
`build_output_dir` matched twenty directory names against the path text. A name is a guess about
somebody else's repository — so it was measured against the repositories on this laptop: an
ingress chart keeps four hand-written files in `build/`, WordPress ships 132 tracked files under
`wp-includes/js/dist/`, and two more commit `vendor/` and `node_modules/` outright. An agent
asked to add a stage to `build/Dockerfile` writes `build/Dockerfile.debug`, and the checkpoint
called it build output and left it behind. A name loses to what the repository *says* now: a
directory git tracks content in is content, asked of each directory in turn so
`build/target/debug/x.o` in a repo that tracks `build/` is still caught. One `git ls-files -z`
per capture, deduplicated to directories. The same check makes `NEVER_SOURCE`'s own claim about
`src/target/mod.rs` true for the first time, and for the right reason — it is a module when the
repo keeps source there. Residual, stated rather than left to be found: a directory the agent
creates from scratch whose name is on the list is still excluded, because there is nothing in
the repo to ask.

**The third surface was `build_argv`, and its own note named the way in.** One unit test per
flag, and the question those cannot answer is whether a *combination* loses one. Three
properties over generated requests. The prompt is positional and sat in the middle of the line,
so `offload run -- "--version should be documented"` — which the CLI accepts; checked — reached
the agent as an option: `error: unknown option`, exit at spawn, on `claude 2.1.238`. Asking an
agent about a CLI flag is an ordinary thing to ask, and the failure is a run that never starts
for a reason nothing in the fleet can explain, retried by an unattended run's own recovery to
exactly the same end. The prompt goes last, behind `--`.

Both halves of that were measured rather than reasoned about, because the parser is the agent's
and not ours: `--` terminates the variadic `--allowedTools` instead of becoming another pattern,
and the grants before it still apply — a run allowed `Bash(cargo build:*)` with a dash-leading
prompt after `--` ran its command under `--permission-mode default` with zero denials. Then end
to end: the smoke example spawns with a dash-leading prompt now, which is the only place this
can be proved at all, and it replies, hits a turn boundary and writes a findable transcript.

**Every fix was reverted to watch its test go red**, including the one that is easy to skip: the
adopt-in-place test fails without the marker, because *always* rebuilding loses the work done
after the last checkpoint, which is the reason adoption exists. A guard is two-sided or it is a
coin toss.

**Then the two surfaces this session had just finished naming, both of which had one.** The first
was aimed at `offload-store`'s run registry under concurrent writers — `save_run` is
last-write-wins over a whole row — and the answer turned out to be upstream of the store, in the
line that decides what gets written at all. `absorb` skips a peer's whole record for any run this
node holds: "our own store is newer than anything a peer can say about it, and merging their copy
would let a stale record undo a turn". True of the state, the epoch, the lease and the checkpoint.
False of the two fields a holder does not own — the deadline and the priority belong to the run's
*home* node, `offload deadline` is forwarded there precisely so two nodes cannot each write
revision 1, and gossip is the only way the edit travels back. So a run submitted on the laptop and
running on the desktop kept the deadline it was submitted with, for ever, while the CLI said the
edit had been applied. Not cosmetic on the holder: that number orders the runs waiting for a slot
(`start_held_runs`) and decides whether a late commitment is handed back (`review_commitment`).

The rule splits into its two halves rather than being weakened — `merge_run` for the record,
`ClusterView::merge_spec_edit` for the edit, with the settling rule now written once
(`settle_spec`) instead of twice. `merge_run`'s coarse protection is untouched: a holder still
refuses a peer's record and still cannot be talked out of its own liveness. And one thing found
next to it: what `absorb` hands the store is now the record as **settled** rather than as it
arrived. They differ whenever a winning record carried an older spec revision — the merge patches
it before adopting it — and the arrival is what was being written down, so the store sat a
revision behind the view with nothing to heal it, since a losing copy never wins a later merge.

**The second was the delivery plane's retry accounting, and it had a state missing.** `Route` knew
working and gone; a peer whose credential stops verifying is neither. It was *filtered out* of the
fleet's routes, so the pass found nothing for the rows already queued against it and abandoned
them — reason recorded as "this route no longer exists in the fleet", about a route `offload
sinks` was listing on the next line. `Route::unusable` is the third state and behaves like
`reachable`, because it is the same distinction: listed, so the news waits; nothing attempted, so
nothing spent. Our own broken script stays the asymmetric case — attempted, failed, given up on
loudly — which is the probe's rule about believing a peer over ourselves. The pass and the report
now derive the fleet's routes from one walk over the view, which is the reason they had drifted:
`fleet_routes`'s doc said the pass "skips both", true of an unreachable node and false of an
unauthenticated route.

**What to pick up.** The list of surfaces is empty again, and the pattern has now paid for itself
eleven findings running. What remains in phase 6 is the part that is not a sweep: `turmoil` (still
the one to be sceptical about — what it adds over `churn.rs` is network timing, not more churn),
metrics and an audit log, and the `openraft` reconsideration, which is the one with a concrete
argument behind it (open question #6). Of the open questions, bid weights (#7) is still the one
that will be found late and needs an ADR.

Three surfaces nobody has aimed one at yet, for whoever wants the twelfth. **`offload-store`'s
own writers**, which this session went looking for and left unanswered: `save_run` is
last-write-wins over a whole row, the supervisor writes it from several tasks, and the question is
whether any two of those windows overlap across an `await` — the ones read here did not, but that
is four call sites of a dozen. **`offload-node::server`'s forwarding**, where a command typed on
one node acts on another (`logs`, `approve`, `deadline`, `cancel`) and the question is whether any
of them is applied locally *and* forwarded, which is the shape ADR-0013 warns about for spec
edits. And **`offload-probe`**, the one crate whose whole job is not over-claiming, where nothing
has ever asked what it reports when a file it reads is present but unparseable.

## Session fifteen, in one paragraph

**The property tests, and the double execution they found.** Two commits. `offload-core` was
built clock-free and synchronous for exactly this — ADR-0001's "that is what the simulation
tests exercise" — and nothing had ever driven it that way: the phase-0 item *property tests: no
run lost across transitions, epochs monotonic under churn* sat unticked through six phases.
`proptest` as a dev-dependency with `default-features = false`, because these are pure
functions: a failing case is a panic, never a hang, and the fork/timeout machinery buys nothing.

Ten properties. The run state machine — epoch, `attempts` and `spec_rev` only grow; a live lease
always carries the run's current epoch (two fields that `fence` treats as one question, and only
while they agree); `Orphaned` never returns a lease; a refused transition leaves no mark; and a
non-terminal run can still be *taken* by somebody, where `cancel` and `abandon` deliberately do
not count, because a run that needs a person is precisely a lost run. The merge — gossip never
walks a run backwards, an unentitled speaker cannot undo an arbiter's orphan, and two grants at
one epoch never leave the fleet disagreeing. The numbers — `RunProgress` settles in the same
place whatever order it arrived in, which matters because it is the one thing about a run that
cannot be re-derived. The bid round — the winner does not depend on arrival order, which is what
lets a grant cost one message instead of a consensus protocol. And `explain` — a refusal always
names a reason, and names the same ones however the clauses were written down.

**Three failures, and every one was a rule written down in one place and honoured only there.**

*A pinned run could be put back in a pool that cannot place it.* `Pending` has three entrances —
a released checkpoint, a handed-back commitment, an operator reopening a failure — and one exit,
`assign`, which refuses a pinned run a second holder. So a pinned run that reached the pool by
any of the three was stuck for ever: not failed, not cancelled, and invisible to the hold-down,
which only ever looks at `Orphaned`. `ps` said `pending`, the arbiter re-offered it, every grant
was refused, and nothing anywhere reported a problem. The rule was already on
`KeepReason::PinnedHere` in as many words — releasing one "would not re-place it, it would make
it unplaceable" — and enforced in exactly that one function. The minimal counterexample was two
acts long: assign, then checkpoint.

*A failing clause went unexplained depending on where it sat in the list.* `failures()` descends
only through unsatisfied branches, which is right, and a branch whose children all *held* — a
`Not` — has no failing leaf, so it reports itself. The check for that asked whether **anything**
had been collected rather than anything of its own, so a sibling that failed earlier silenced
it. Nobody chooses that order: the list comes from a repo config, a `--constraint` string, or
`agent_ready`.

*And the one that matters: an epoch is monotonic **per arbiter**, which is a weaker thing than a
total order.* Two arbiters both compute `next()` from the same record and issue the same number
to different nodes; `fence` refuses a caller that is *behind*, and neither of these is. So both
writers were admitted, and the merge had nothing to break the tie with — at equal epoch each
record is spoken for by its own holder, so `from_holder` was true on both sides and `merge_run`
believed whoever arrived most recently, flipping again on the next tick. Neither agent was ever
told it had lost. **Two agents on one repository, both committing** — the failure this project
cares about most, reached through the mechanism ADR-0002 names as its safety mechanism. That ADR
says fencing makes a split-brain grant a *rejected* writer and, two paragraphs later, calls the
reconciliation "last-writer-wins on epoch". Both sentences were in the file; only one was true.

And it needed **no partition**, which is how it survived being written down as a partition-time
compromise. A node that concludes the home node dead grants the run and *then* hears the
refutation, so a few missed probes on a healthy LAN do it. Worse, a *single* arbiter did it:
`place` re-cloned the original record for each grant attempt in a round, so every node in one
round was offered the run at one epoch — and ADR-0006's "silence is a decline" makes "took the
grant, started the agent, answer lost" the ordinary case rather than an exotic one.

Three changes. A grant **spends a token whether or not it is confirmed**: `hand_over` threads one
`Run` through the round and gives the token back with `Run::release`, whose existing sentence
about ending a holder's right "at the moment it stops intending to" is exactly this seen from the
arbiter's end. Two live grants at one epoch are ordered by **lowest holder id** — the bid round's
own tiebreak, computable from the record alone, offline, with no message. Which grant wins is
arbitrary; that every node picks the *same* one is the point, because that is what makes the
loser's record name somebody else and its own `fence` refuse it. And a holder whose record names
somebody else **at its own epoch** stops its agent: `record_run` read `<`, which is the one
comparison that excludes this case. The merge rule is restricted to two *live* leases, so the
orphan rules above it are untouched — an orphan is an observation, not a grant.

What stays true, in the form the code now implements: under a partition that *persists*, two
agents can run, and no fencing changes that without quorum. What fencing buys is that the moment
the two records meet, exactly one leg survives and the other's effects are refused. A better
tiebreak is written into the amendment and deliberately not built — the granting arbiter on the
`Lease`, so the run's *home* node's grant beats a successor's, since a successor only ever acted
on a belief that turned out to be wrong. It costs a field, a wire version and a schema default,
to choose between two grants either of which is safe.

Each of the three fixes has a test that fails without it, checked by reverting the fix rather
than by assuming it would. No wire or schema change.

**Then the multi-node half**, in a third commit: `tests/churn.rs`, a fleet of nodes that each
hold their own `ClusterView`, gossip to each other, run their own supervision pass, and crash
and come back. None of it needs a network, which is ADR-0001's arrangement collected on a second
time — `supervise` is a pure function of (view, run, now) and `merge_run` of the two records plus
who spoke, so a partition is a fact about which deliveries the loop performs.

Five properties: at most one node thinks it holds the run once the gossip stops; a settled fleet
holds one answer about *who* holds it and at what epoch; two arbiters that both grant do not
leave two holders; which grant survives does not depend on delivery order; and a run outlives
the machine it was running on. Plus two invariants checked after every single event — no view
walks an epoch back, and nobody who is up believes *it* is the run's absent holder.

**And the fourth bug, which is the one this half was worth building for.** A holder is running an
agent. Its arbiter misses two probes — a Wi-Fi handover — suspects it and gossips `Orphaned`. The
holder *accepted that about itself*. `Orphaned` returns no lease by design, so afterwards it
cannot renew, and when its agent finishes `complete` is **fenced out and the work is recorded
nowhere**. Nothing was wrong with the run or the machine. `merge_node` has had the rule since it
was written — "our own entry is ours… never by believing it about ourselves" — and `merge_run`
had no equivalent at the one place it decides whether work counts. Narrow fix: a node still
believes its arbiter about a run it is *not* holding. And the refutation needed nothing new,
because keeping `Running` and gossiping it *is* the reclaim path seen from the other end.

**Written down rather than fixed**, because it is the design working: an `Orphaned` record is its
arbiter's *observation*, and nobody else may relay one — that rule is what stops a third node's
stale copy undoing a decision. So an observation half-delivered at the moment its author crashed
stays half-delivered: some nodes say `orphaned`, the rest `running`, about one holder at one
epoch, and no gossip moves either. ADR-0007's own distinction, doing its job. It costs nothing —
an orphan grants no authority to anybody, and any *action* bumps the epoch, which wins outright
everywhere. So the property asserts the two fields that are *decisions* unconditionally and the
observation only while its author is present to state it.

**Two of the first failures were the test's own, and both are worth knowing.** `settle` compared
run records and stopped as soon as they stopped moving — but the equal-epoch rules key on
entitlement and `arbiter_for` derives entitlement from *liveness*, so a fleet still arguing about
who is dead is a fleet still arguing about who is entitled: it reported a standoff as settled.
The fixpoint is over the whole view now. And the aimed two-arbiter property passed for two
rounds while never once constructing two arbiters — the scenario needs the *holder* to look gone
as well as the home node, or the hold-down simply waits. A property that cannot reach its own
premise is worse than no property, so it asserts the premise now (`granted` has two distinct
holders at one epoch) rather than trusting it.

660 tests, clippy clean, and no wire or schema change in the whole session.

**And a third target, which turned out to be the sharpest.** `offload-core::allowlist` was on
the list of untouched surfaces at the end of the second commit, with the question already
phrased: does a pattern that `risk` calls safe ever admit a shell? It does — four ways.
`ToolPattern::risk` compared the *text* somebody wrote against a list of program names, so a
path (`/bin/sh`, `./sh`), a version suffix (`python3.11`, which is how python is spelled on a
real machine), a quote (`"sh"`, and `/bin/"sh"`) and an environment assignment (`FOO=1 sh`) all
read as `Scoped`. `Scoped` is what `require_scoped` accepts, and `require_scoped` is applied to
the allowlist a **repository** ships in its `.offload.toml` — the check whose entire job is
stopping content from granting itself a shell.

**Measured on `claude 2.1.238`, because this is a claim about the agent and not about us.** A run
allowed `Bash(/bin/sh:*)` and nothing else executed `/bin/sh -c "cargo build --version"`; the
same run, asked for `cargo build --version` directly, was refused with "This command requires
approval". So the pattern was a working escape from its own label, not merely a mislabelling.

**And the measurement settled a bigger question this module had asserted since phase 1 and never
checked: the agent decomposes a compound command and checks each part.** Under
`Bash(cargo test:*)`, `cargo test --version && cargo build --version` is refused with "This Bash
command contains multiple operations. The following part requires approval: cargo build
--version". Worth knowing precisely, because if it had been a plain prefix match then *every*
scoped grant in the design was a shell via `;` and the allowlist was decorative. It is not — and
that is why an interpreter in first position is the only leak of this shape, and why `risk` is
rightly a list of programs rather than a shell parser.

Two side notes from that probing, both worth remembering. `--allowedTools` is variadic and
swallows everything after it, which is why the adapter puts it last and the prompt earlier — a
probe that ignored that got "Input must be provided either through stdin or as a prompt
argument". (Session sixteen amended the second half: the prompt goes *after* the patterns now,
behind `--`, which terminates the variadic list rather than being taken as another pattern.) And `echo` and `touch` are auto-approved by the agent under `acceptEdits`, so a
control that uses them proves nothing: the first probe of the compound question was worthless
until it was rerun with `cargo`, which is genuinely gated.

**And the fourth target, which was the last one on the list and the sharpest of all.**
`offload-core::fleet`, with its question already written down: does a certificate chain that
verifies ever grant more than it was issued? It does. ADR-0012's own delegation sketch reads
`Delegation { fleet, approver, may_issue, expires_at, serial }`, and **`may_issue` never reached
the code** — so a delegation bounded *who* may issue and nothing about *what*. An approver, which
is a delegation plus a node key, could sign a certificate granting `HostRuns` to any device with
probation cleared, and every peer verified it and admitted it. Mitigation 1 says that grant
"turns membership into runs agents on your repositories with your credentials" and stays behind
the root; one compromised approver was one certificate away from a device that receives and
executes the fleet's work.

**A flat bound would have been the wrong fix, and this is the part worth remembering.**
Certificates are renewed on contact by an approver, and a renewal *restates* the grants it
renews — so an approver forbidden from ever signing `HostRuns` cannot renew a host node either,
and every host needs the passphrase every thirty days. That is precisely the failure session
fourteen built renewal to prevent, walked back in through the other door. What a verifier needs
is not a bound on grants but the difference between **minting** and **restating**, checkable with
the fleet public key alone, offline, at first contact. So a renewal carries the fleet-signed
certificate it descends from (`MembershipCert::authority`) and may grant no more than that
certificate did — the chain's *root* rather than the previous link, so a host renewed monthly for
two years carries one certificate; and with the authority's expiry deliberately unchecked, since
the grant was made once and the renewal in hand is what keeps it current.

`may_issue` stays a constant rather than becoming a field, by this ADR's own argument against
mitigation 3: no caller would set it to anything else, and a gossiped per-approver policy record
every fleet ships identically is machinery guarding a personal fleet. Where it goes when a fleet
needs it is written into the amendment.

**Two things to carry forward.** First: **every device re-joins**, because adding a field changes
the bytes a signature covers — `offload join --passphrase` once per device, or `offload invite`
from one that has already re-joined. Second, and the reason that is stated here rather than
discovered in six months: **nothing in the tree noticed the format change.**
`fleet::passphrase` has had a pinned known-answer test since it was written, for a reason that
applies to the signed message word for word — the value is compared across machines, so changing
it does not fail loudly, it silently re-founds every fleet. The certificate's own signing bytes
had no such test, so this change broke every existing credential with nothing going red. There
is one now over all four credential types, including the case where an authority is present.

**And a sixth, off the end of the original list: the blob collector.** Flagged the commit before
as "closest in shape to the bugs found so far — a conservative sweep whose conservatism is
asserted nowhere". It was worse than that. `collect_garbage` found referenced blobs by looking for
`lower(hex(blobs.hash))` inside `run_json`, and `BlobHash` derives serde over `[u8; 32]`, so a
stored run spells its transcript `[163,188,121,…]`. The match could never hit. **Every blob in the
store looked unreferenced**, so the collector deleted the transcript, bundle and patch of runs
that were still running — measured on a two-blob store with one blob live, and it removed both.
Its own comment said a substring match "cannot miss one. It can only be over-cautious, which is
the safe direction."

**This one is the sharpest lesson of the session, because the test was the reason nobody looked.**
The fixture inserted a row whose JSON was `{"transcript":"<hex>"}` — a shape `save_run` has never
produced — so "referenced blob survives" passed while nothing on the real path worked. Code and
test were mutually consistent and both fictional. Build a fixture through the writer the daemon
actually uses, or a test checks your idea of the schema against your idea of the query.

The hazard was already known *in the same crate*: `fleet_events.subject` is denormalised out of
the JSON specifically so a lookup is not "a `LIKE` over a blob of JSON that happens to spell node
ids as arrays of integers". Four hundred lines from the query that did it anyway.

References come from `Checkpoint::blobs` now — the same accessor a receiving node uses to know
what it must fetch, so a new blob field cannot be added to a checkpoint and forgotten here — and
an **undecodable run row fails the whole collection** rather than being skipped, because a run
whose references are unknown is not a run with none. Two properties: exactly the unreferenced
blobs are collected, in both directions, and collecting is idempotent.

**What to pick up.** Nothing in the properties is blocking. The open questions in order: bid
weights bidding in different currencies (#7) is the one that will be found late and needs an ADR;
the untracked-file denylist has still never met a large real repo (#3); and phase 6 still has
three unticked items — the `turmoil` simulation, metrics and an audit log, and the
`openraft` reconsideration. Of those, **`turmoil` is the one to be sceptical about now**: what
it adds over `churn.rs` is real network timing and task scheduling, not more churn, and
`churn.rs` reaches the interesting disagreements without a dependency. The `openraft` question
has a concrete argument behind it for the first time (open question #6 below), which is the
better reason to spend a session. The list of surfaces to point properties at is empty, and
every one paid for itself: two live security holes (`allowlist`, `fleet`), two correctness bugs in
the fleet's own loop, one silent delivery outage, and a collector that deleted work. **Five of the
six were a rule stated in prose and enforced only where an honest caller passes.** The pattern is
reliable enough to plan around: pick a surface, write down in one sentence what the tree claims
about it, make that sentence a property — and then read the code *around* it, which is where two
of these actually turned up.

Surfaces that have not had this treatment, for whoever wants more: `offload-workspace` (does a
checkpoint's untracked-file policy ever drop something a resume needs? — the closest remaining
thing to the blob collector, since it is the other place work is silently not kept) and
`offload-agent`'s argv construction (`build_argv` has a unit test per flag; is there a combination
that loses one? `--allowedTools` is variadic, which is one way to find out).

**The sweep is done, and it found one more.** Every place a hash or an id is matched, filtered or
joined as text: `knows_member` compares `fleet_events.subject` against `NodeId::to_string()` and
the writer uses the same function, so that one is sound. `resolve_run` was not — it matched
`lower(hex(id)) LIKE needle || '%'`, and `LIKE` reads `%` and `_` as wildcards, so
`offload cancel %` resolved to the only run in the store and `ab_d` reached a run whose id has no
underscore. It resolves on a single match rather than refusing, which is the "helpfulness that
cancels the wrong job" its own comment warns about. A needle that is not hex is refused by name
now. Worth repeating that sweep after any change to how an id is stored: three of the seven
findings were a value compared in the wrong representation, and none of them failed loudly.

## Session fourteen, in one paragraph

**The front door, finished — and the last unbuilt ADR.** Nine commits, eight of them on ADR-0012,
and the pattern from session thirteen repeated exactly: *four of the first six were claims the
code did not honour*, and the way to find them was to read the ADR as a specification rather than
as a description.

*A grant or a revocation reaches the daemon that is already running.* `NodeMembership`'s own doc
comment said certificates are renewed and revocations arrive, and nothing ever wrote to the lock
it said that about. Every membership command works with no daemon — that is this ADR's central
claim in operational form — so `fleet.json` is written by a process that is not the daemon, and
the daemon never looked again. `offload grant host-runs` re-issued a certificate the daemon kept
not presenting, so session eleven's "a grant needs no restart" was true of the checking side and
false of the granted node's own; and `offload revoke` wrote a fact the daemon never read, which
made "revocation is immediate and local" mean "immediate, once you remember to restart" — on every
machine, because the daemon is always up.

*A revocation travels* (wire v16). The gossiping needed one sentence the ADR had not made
explicit, and it is worth keeping: **membership is the one thing here that does not need to
travel** — a certificate verifies against the fleet public key at first contact — and revocation
is the exception because it is the *absence* of a signature, which no certificate can carry. The
half that is easy to leave out is the hang-up: membership is checked at the handshake and a live
QUIC session never handshakes again, so a node that files a revocation and keeps the socket open
has recorded a fact and changed nothing. The test drives it all the way to `Dead`.

*Certificates are renewed on contact* (wire v17). This ADR leans on expiry twice — it is what
makes an unheard revocation eventually bite, and what makes a device out of contact for a month
fall out by itself — and nothing renewed anything. So the real behaviour at thirty days was not
decay towards safety; it was **every device dropping out at the same moment**, with the passphrase
as the only way back on each of them. Founding a fleet quietly set a date. Asked rather than
offered, because the node whose certificate is lapsing is the one that knows and an approver
volunteering would renew nothing for the laptop that has been shut for three weeks. The request
carries *nothing*: what is re-issued is the certificate the handshake authenticated, which is
`admit`'s separation of the claimed from the proved applied to the other place a certificate is
minted. Probation is the one field renewal cannot copy verbatim, because it is measured from
`issued_at` and renewal moves it.

*`offload invite`, and the online issuer.* `offload id` there, `offload invite <id>` here,
`offload join --token` back there. **The token is not a bearer credential**, and that made the
design simpler rather than harder: what is carried is a certificate naming the joining device's
key, so it needs no expiry of its own and can be pasted anywhere — the same sentence as "a
membership certificate is not a bearer token", arrived at from the other end. Which produced a
correction to mitigation 2: **probation follows the grant, not the path.** Probation only ever
suppresses `host-runs`, so "an invite skips probation" is a statement about the door, where the
grant is not in play; the mitigation is about the grant and has no exemption for the passphrase,
so `offload invite --grant host-runs` — which is this ADR's `offload grant <node>` — probates like
the local command. Without that it would have been the documented way around mitigation 2.

*Fleet health.* The nag the ADR asked for existed and was wrong: `offload fleet` printed "this
node is the only approver it knows of" whenever this node was *an* approver, which is a sentence
about the fleet derived from a fact about one device, and it said the same thing on a fleet with
four. Counting approvers means counting certificates, and a certificate is exactly what gossip
cannot be trusted to carry — so the cluster remembers what each handshake *proved*, which bounds
the answer to peers this node has met and says so rather than smoothing it over. Two nags needed
narrowing to be worth printing: an invited device has never seen the passphrase, so telling it to
run `offload verify` sends somebody to use the fleet's root secret on the wrong machine.

*An enrolment is announced* (schema v5, wire v18), which is the control the rest of the posture
leans on: with no per-join approval step, probation's fifteen minutes were being bought for an
alarm that did not exist. It needed the delivery plane to address something other than a run, so a
notification carries a `Subject` and the store has a second log — two logs rather than a nullable
column, joined by `topic`, because **both logs number from one** and a dedup identity without it
would have the first fleet event mark the first run's notification sent. Two producers, because
neither is enough alone: the CLI (these commands run with no daemon, so an alarm needing one would
be missing on the machine somebody just walked up to) and every *other* node when it meets a member
it has no record of (the inviting machine may hold no route to a person, and may be the machine an
attacker used). Which makes it a log of what a node **witnessed** rather than the fleet's memory —
the ADR's "written durably to every node's store" would be a gossiped fact, and a gossiped fact
needs an owner (ADR-0005) that independent observations do not have. `offload nodes --history`
says so and tells you to ask another device.

*`offload rekey`.* The convergent revocation: ordinary revocation has to reach every node, and
this does not have to reach anybody — the evicted device is simply not in the new fleet. The first
version printed an invitation per member and every one was refused, because a device will not let
an unfamiliar fleet replace the one it belongs to, and that is exactly what an attacker would
send. So a rekey carries a **succession**: the new fleet's identity signed by the key being
replaced, which every member already holds. Only somebody with the old passphrase can move a
device, which is the same authority that could already do anything to it. Both ends of the
statement are checked, because half of it is the dangerous half.

*And the ninth: a run reaches a mailbox on another machine* — ADR-0011's proxied call, the last
unbuilt piece of any accepted ADR. The agent gets an MCP server whose program is `offloadd` itself
and each line of the agent's own protocol is carried to the holder and back, opaque at every hop
(wire v19). A proxied server has **no `env`**, which is the whole feature, so it is named by
*service* rather than by the holder's own id — which this node does not know and has no business
learning, and which as a side effect keeps the tool prefix the same on every machine, so a
migrated run's transcript stays readable. It retires `Constraint::CanUse` from what `--use`
builds, and the ADR's own words are what retire it: the grant constrained placement "until a
resource can be reached across the mesh". Keeping it would have pinned the run to the one device
that cannot host it. The refusal moves to submission instead (ADR-0014's argument in the place it
belongs). Verified with a real agent and a real stdio MCP server on two daemons: the run ran on
the desktop, called `mcp__email__read_inbox`, and wrote what the phone's mailbox said; ungranted,
the same prompt found no `mcp__` tools at all.

**Deliberately not built, and written into the ADR as such:** mitigation 3, "once an approver
exists, it is required". Its own escape hatch two paragraphs later defeats the threat it names —
`offload join --passphrase` has to stay unguarded, and an attacker with the passphrase takes the
unguarded path. What is left after that exemption is a bar against carelessness, bought with a new
fleet-signed, gossiped, monotonic policy record. The alarm is what actually covers that threat,
and the alarm is built. Also open: the *online* `offload join`, where an approver is asked over
the network and the owner confirms on a device they are holding. The offline artifact makes that
a convenience rather than the missing half.

639 tests, clippy clean. Wire v16 (a revocation travels), v17 (renewal), v18 (a notification's
subject), v19 (a proxied resource call). Schema v5 (the fleet log, and `topic` in the outbox).

## Session thirteen, in one paragraph

**Four things a run can and cannot reach.** Phase 4 is complete, so this session worked phase 5's
unblocked items — and three of the four turned out to be a claim the code did not honour.

*The approval channel now covers the mode it is running in.* ADR-0017 left `PermissionMode::Ask`
refused at submit and said honestly why: the hook matched `Bash` and `WebFetch`, and under `Ask`
the agent gates every edit, so lifting the refusal would have produced a run that can be asked
about a command and is silently denied every `Write`. The fix was not the refusal but the list.
`ask_tools(mode)`, because the mode *is* what decides whether the agent gates a call — a constant
is wrong in one direction or the other and this one was wrong in both. Measured first, on
`claude 2.1.238`, as this ADR requires: under `manual` a `Write` is denied headless; with a
`Write` matcher the hook fires with the call's own `tool_use_id` and an `allow` writes the file.
Widening forced the question the ADR declined to answer on the way past — how many questions will
a person answer for one run — and the answer is a budget (`--ask=N`, default 20) counted on
`RunProgress` so a migration continues it. Running out is **not a denial**: the calls after it are
decided by the agent's own rules, which is what nobody-answering already does, so the failure mode
of running out is the behaviour we shipped. Verified on a real daemon under
`--permission ask --ask=2`.

*A run reaches nothing the fleet did not grant it.* `--strict-mcp-config` was never passed.
CLAUDE.md has warned about this since phase 1 and the warning was describing the tree it was in:
measured on this laptop, a run spawned by `offloadd` inherited **five** MCP servers — one from a
`.mcp.json` in the repo and four of the owner's own, three connected, mail and calendar among
them. So "what a run can reach depends on which machine won the bid" was literally true, and a
repo could grant itself a service by committing a file. The flag is unconditional now and the
test asserts it for resumed and `Full` requests too.

*And then the granting side*, which only means something once the fence exists. ADR-0011's
`Role::Resource`, local half: `[[resources]]` in node config (an MCP server the owner nominated —
the sink rule applied to the other direction), `offload run --use email`, a new
`Constraint::CanUse` so a granted run is placed where the resource is or refused to the operator's
face, and a projection at spawn that hands over the config **and permission to call it**. That
last clause is the demo earning its keep again: the first end-to-end run projected the config
alone, and the agent saw `mcp__mail__read_inbox`, was denied it, and reported a declined
permission request. A grant that grants nothing is worse than no grant. Verified against a real
stdio MCP server, all three ways: granted, ungranted, and a service nothing offers.

*One daemon per state directory.* Session twelve flagged `server.rs`'s comment claiming a lock
that did not exist; the consequence was worse than the confusing error the comment was excusing.
Two daemons on one directory both started, the second unlinked the first's socket, and the first
kept its runs and its leases while every command reached the second. A SQLite exclusive
transaction on the directory — the broker's mechanism, for the broker's reasons — plus a
connect-probe before `bind` removes a leftover, because a socket shared by two *different*
directories is the case a directory lock cannot see.

And a sixth, which came out of writing the fifth down: auditing this list for claims nothing
enforces found the biggest one unenforced. "Every side-effecting path checks the epoch" — and
`drive` spawned the agent *before* the check and discarded its result. A slow workspace
preparation is exactly what gets a run reassigned underneath its holder, so the window was real
and the outcome was two agents on one repo. Checked before the spawn now, and a failure after it
kills the agent.

Then the audit kept going, and three smaller things came out of it. A resource's `env` is where
the token for the service it reaches lives, and the generated MCP configuration was a
**command-line argument** — readable by any local user through `/proc/<pid>/cmdline`, measured on
this machine. It is a `0600` file now, removed when the leg ends. A route's gossiped description
defaulted to `runs <command>`, so "a peer learns this device *has* a push route, never how it
works" was false in the fleet listing of `offload sinks`; the command stays local now and is still
shown where the owner is reading about their own machine. And `Constraint::CanUse` — added earlier
the same day — fell into `observed`'s `_ => None`, so a refusal repeated the requirement and never
said what the node had. That catch-all is written out now, so the next variant fails to compile.

The fencing change touches the path a **migration** resumes through, so that was re-verified
rather than assumed: a real run checkpointed and released at turn 4, `offload resume` picked it up
in the same conversation, adopted the worktree in place, and finished all six sequential steps —
the agent noting for itself that only the first two files existed and carrying on from step three.

**The pattern across all six of the audit's findings is one sentence:** a rule that is written
down and not mechanised reads exactly like a rule that is enforced, and the ones that had been
reasoned about most carefully were not safer for it. Two of these were introduced by this session,
hours after writing the lesson — which is the honest version of it. Auditing the list is worth
repeating, and the parts of it still unmechanised are the behavioural ones (mid-turn checkpoints,
attendance, "don't over-claim in the probe").

595 tests, clippy clean. The wire is **v15** (v14: the ask budget; v15: what a run may use).

## Session twelve, in one paragraph

**Capacity finally means the machine.** Two roadmap boxes, and each turned out to be blocked on
something that was not the feature. *Per-node caps* looked done and were enforced against nothing:
every test in `policy.rs` built capabilities with no agent in them, so the per-agent branch was
unreachable while looking covered, and `offload status` fed it `held.runs` — every agent conflated
— so the number it printed disagreed with the one `bid::evaluate` computes from the view. *Per-account
caps* were blocked on the fingerprint, which derived from `$USER` and `$HOME` and was candid in its
own doc comment about being per-machine: `bid::evaluate`'s account pressure was live code that
could never match anything. It now hashes the account uuid the agent records in its own settings
file — the uuid and not the email beside it, because a digest of a guessable string is reversible
in practice however opaque it looks — with a versioned salt and a pinned known-answer test, because
a value compared across nodes is a wire format. Verified against this machine's two real accounts,
which used to fingerprint identically and now do not. The API-key path was its own small bug:
it returned the literal string `env:ANTHROPIC_API_KEY`, so every key-authenticated node in the
world was one account sharing one cap.

Then the caps themselves, where **the demo earned its keep twice**. A one-run account ceiling with
two submissions a second apart started both, because the node asked the *view* about its own runs
and gossip was a tick behind — so `AccountUse::elsewhere` is peers-only now and the local count
comes from the store, which knows. Worse: run one completed and run two sat `assigned` for ever,
because a held run is `Assigned`, therefore visible, therefore counted against the ceiling it was
waiting on. That is the same self-blocking commitment deadlock this project hit in session eight,
in a new dimension, and the fix is the same distinction — the start side counts started agents
excluding this one. Both are tests now, and the fixed version was verified on a real daemon: run
two held while run one ran, then started by itself and finished all eight turns.

And **the device broker**, ADR-0013's one piece of shared mutable state, built last and with the
least drama. Its own SQLite database under a per-user runtime path — chosen because SQLite's
locking is genuinely multi-process, this workspace forbids `unsafe` so `flock` was never available,
and `state.db` is per-state-dir, which *is* the problem. Reconciled from the heartbeat rather than
maintained on the lifecycle paths, because a reservation is a lease and because the ledger and the
store drift for ordinary reasons a reconcile handles without anybody enumerating them. The one
thing the demo corrected: consulted only where runs are accepted, a node bid "starting now", won,
and held the run on arrival — right outcome, broken promise — so the number reaches
`bid::evaluate` through `LocalFacts::device_committed`. Verified with two independently founded
fleets on one laptop, one run permitted each: one agent ran, the other fleet's was held, both rows
sat side by side in the ledger, and the held one started when the first finished. One wording change
fell out of both features at once — `Availability` says "the run ahead of it" rather than "its
current run", because since these two the work a run waits behind is often on another machine, or
in another fleet, and "its" sent somebody to the wrong place.

Left deliberately undone: two *users* on one machine can still over-commit it, which is now a
stated limit rather than an oversight (a device-wide ledger would have to be world-writable). The
per-agent per-node ceiling is still the probe's hard-coded 2 with no config path — `[policy]
max_concurrent_runs` is the configurable per-node cap, and a second knob wants a reason to exist.
And `server.rs`'s claim that "a live daemon already holds the lock on the state dir" was still
false: nothing locked a state dir, and two daemons pointed at one would unlink each other's
socket. *(Closed in session thirteen, using the broker's own mechanism — which this paragraph
correctly identified as the first thing in the tree that could make the comment true.)*

## Session eleven, in one paragraph

**Peers check each other's grants now.** The gap the last session's notes flagged: a node
enforced `HostRuns` only on itself, so the signed certificate — the thing that exists so a phone
cannot promote itself by editing its own TOML — was decorative against a node that simply bid
anyway. The refusal lives at the **bid exchange**, not the handshake, because a node without
`HostRuns` is a full member (it submits, it delivers) and must keep being admitted; what it may
not do is host, so what peers refuse is its bid. An honest node never bids without the grant —
its own bid path checks its own certificate — so the arbiter-side objection fires only for a
node that is misconfigured, out of date, or lying, and it answers like any other refusal:
`offload run` and `explain` print "bid, but its certificate does not grant host-runs" (or "…is
dormant for another 12m of probation") rather than placing the run or shrugging.

Two staleness problems decided the shape, pulling in opposite directions. `handshake::Peer` used
to carry the certificate's *effect* — an effective grant set computed at admission — which bakes
in a moment: probation ends while a connection stays up, and a check against admission-time
grants would refuse a now-legitimate host until something happened to redial. So `Peer` now
carries the **certificate itself** and `Peer::may(grant, now)` asks it fresh; probation lifts on
a live connection with nothing redialled and nothing gossiped. The other direction: `offload
grant host-runs` mints a *new* certificate, and a live connection still carries the old one — so
an **objection also drops the connection**, and the next round re-handshakes and believes the
renewed papers within the arbiter's own retry. Without that, "a grant needs no restart" (session
five) would have quietly become "a grant needs the link to happen to break". The same close
handles a certificate that expired mid-connection: the re-handshake is refused at admission and
the node fades to silence rather than bidding for ever on lapsed papers. All three are tests in
`offload-cluster/tests/mesh.rs`; no wire change, since the check reads what the handshake
already carried.

Left deliberately unchecked, and written into ADR-0012's amendment so it is not mistaken for
done: the **sender's** authority — any admitted member can act as an arbiter and send a grant,
which arbiter failover requires, so tightening that needs an answer to "who may arbitrate this
run", not a grant lookup. And revocation mid-connection still relies on the probe path noticing;
the bid check sees a revoked node only after its connection re-handshakes.

## Session ten, in one paragraph

**A run says how it wants to be told.** ADR-0010's last open piece on the forward direction:
`offload run --notify push|email|chat:team|all|none`, defaulting to every route there is. Three
decisions did the work. It is **not a `Constraint`**, which is what the ADR itself said and which
pointed the wrong way — a constraint selects a *node*, and a phone holding a push route and a
mailbox satisfies `HasService { push }`, after which the obvious implementation tells that person
twice, once by each route. So `Audience::admits` is asked about one *capability* at a time. It
names a **service and never a route id**, because an id is a node's own name for one of its sinks
(`toast`, on the phone) and naming one would pin a run's news to a device — the exact thing this
plane exists to stop mattering. And it lives on the **spec** (wire v10) for `queue`'s reason: the
node that delivers is usually not the node that took the request, so an audience only the
submitting process remembered would be one a migration silently widened. Deliberately not a third
`SpecEdit` — two editable fields share one counter on purpose.

Two smaller things that are the difference between a feature and a trap. The filter runs where
news is **noticed**, not where it is sent: an outbox row is a promise to deliver, so a route the
run never asked for is never owed one — while the *cursor* still advances, because a cursor bounds
the scan rather than recording what was sent. And a **silence is chosen or it is explained at the
keyboard** (ADR-0014's argument on this plane): a run asking for a service no device in the fleet
offers is told so at submission — "no chat:team route in this fleet, so nothing will tell you" —
rather than discovering it at breakfast, and `--notify none` is said back for the deadline's
reason, that it changes what happens with no later event to point at. Neither is a refusal; this
project does not cancel a run over reporting.

Verified on two daemons with three routes — push and email on the desktop, `chat:team` on a phone
that hosts nothing and was never granted `host-runs`. A default run reached all three.
`--notify push` reached the toast alone. `--notify none` reached nobody and said so.
`--notify chat:team` was carried by the *phone* while the desktop's own routes stayed quiet, and
the same flag against a fleet without that route was answered at submission instead. One thing
refused rather than explained: `--notify agent:claude-code` parses (`Service` is one enum for both
planes) and could never match a route, so the CLI says "claude-code is an agent, not a way of
reaching anybody".

**And then the reply direction, which is ADR-0017.** A run submitted with `--ask` stops mid-tool-call
instead of being denied: the question goes into the run's log, the delivery plane fans it out
(`Notice::NeedsDecision`), `offload asks` lists what is waiting with a clock on it, and
`offload approve|deny <run> [tool_use_id]` answers — **from any device in the fleet**. The agent's half is a `PreToolUse` hook whose
program is **`offloadd` itself** (`offloadd ask-hook`) — the agent's own protocol, and `current_exe`
means there is nothing to discover, configure, or point at the wrong version.

Everything load-bearing about it was *measured* on `claude 2.1.237`, and one measurement rewrote the
design: **the hook fires for every matching tool call and the agent never says whether permission
was actually needed.** So the harness has to decide what is worth asking about, and both halves of
that are traps — asking about everything means asking a person about reads the agent would have
allowed by itself. Hence a short tool list (`Bash`, `WebFetch`), and hence a run's own grant
suppressing the question, which is safe only because a match means *let the agent apply its own
rules* rather than *allow*. The same insight decides the timeout: **nobody answering prints
nothing**, not `deny`, so the channel's failure mode is byte-for-byte today's behaviour. An explicit
denial would have refused calls the agent would have allowed — a regression dressed as a safety
measure.

Two other measurements worth keeping. A hook killed at its timeout leaves the call **denied**
(`outcome: cancelled`, exit 1, no output) — fail-closed, verified rather than hoped for, and the
property the whole design rests on. And `echo hello` runs under every permission mode with or
without a hook, because trivial commands are allowed by the agent's own judgment: a permission
probe built on `echo` measures nothing, which cost this session two inconclusive runs. `dd
if=/dev/zero of=x bs=1 count=1` is a command that is genuinely gated.

Verified with a real agent, five paths: **approved** (the file was written, the log says "allowed
(an operator)"), **denied** (the agent quoted "denied by an operator" back and wrote nothing, and
the run recorded a denial), **unanswered** (patience came to 54.9s under a stated one-minute
deadline, then the pre-existing "This command requires approval"), **already granted**
(`--allow "Bash(dd:*)" --ask` asked nobody and ran), and **nobody to ask** (no sink, nobody
watching → declined instantly with a reason instead of stalling five minutes to reach the same
answer by clock). Each question also arrived as a push through the delivery plane, unchanged.

**And the answer travels, which is what made the rest worth having.** Without it the question
arrived on a phone that could not act on it — the delivery plane's own failure mode with an extra
step. `ClusterMessage::Answer` goes to the holder (wire v12), the mirror of `EditSpec` going to a
field's owner and stricter, because a blocked *process* exists on exactly one machine. It carries
**no epoch**, and that is not an omission: a question's identity is the agent's own `tool_use_id`,
which exists only while that process is blocked on that call, so an answer for a leg that has ended
matches nothing and is refused by having nowhere to go — fencing by construction rather than by a
check somebody has to remember. `offload asks` canvasses the fleet rather than reading gossip, for
`explain`'s reason, and the clocks come from the holder because a laptop subtracting timestamps
across two machines would report clock skew as urgency.

Verified on two daemons: the phone joined with `{submit, deliver}` and never got `host-runs`, a run
on the desktop blocked twice, both questions arrived as pushes on the phone, `offload asks` **on
the phone** showed them with `WHERE: desktop`, and `offload approve` **on the phone** unblocked the
desktop's agent both times. The desktop's log records `allowed (an operator on phone)`, so the
record says which machine said yes.

**`PermissionMode::Ask` is still refused at submit**, and that is the honest part: the hook covers
commands and fetches, `Ask` also gates every edit, and lifting ADR-0008's refusal today would hand
somebody a run that can be asked about `Bash` and is silently denied every `Write`. The valuable
half needs none of it — under the product default, a command a run has no grant for used to come
back as a denial in the morning and can now be approved from a phone.

**Left undone on purpose:** `offload explain <run>` does not say where a run's news will go. It
would want the sentence `deliver::audience_note` already builds, and `explain` is a pure function
already at clippy's argument limit — so the honest options are a parameter bundle or nothing, and
nothing is cheaper until somebody asks the question from a second machine. On the approval side,
two rough edges: `offload ps` shows a blocked run as `running`, which is true of the state machine
and useless to somebody wondering why nothing is happening (`offload status` counts them and
`offload asks` lists them).

**And a real bug the demos found, which the first diagnosis got wrong.** Every checkpoint in the
approval demos failed with "no transcript found for session …", and it was written off as an
artefact of this machine. It is not: `$CLAUDE_CONFIG_DIR` is a supported way to move the agent's
state, this box has it set to `~/.claude-alt`, and both `offload-agent` (transcripts) and
`offload-probe` (`.credentials.json`) hard-coded `~/.claude`. Two failures, in opposite directions
and both silent: a node whose owner moved that directory **captures nothing all night** — loud in
the run's log, invisible everywhere else, and the run is unmigratable without anybody being told —
or reports itself **unauthenticated** and refuses every run, which is the honest-probe rule
producing a dishonest answer because it was looking in the wrong place.

Fixed in both, and the environment is now read in exactly one place per crate: `ClaudeCode`
resolves it once at construction and everything downstream asks the adapter (`config_dir()`)
instead of deriving a path from `$HOME`. That also made the path functions pure — they take a
config directory rather than a home — which matters more than it sounds: a test that resolved the
variable would have written into somebody's real agent state. There are deliberately **two** copies
of the three-line rule, in `offload-agent` and `offload-probe`, because a probe that pulled in the
whole agent adapter to resolve one path is the dependency doing more harm than the duplication;
both are tested and both say so. Verified after the fix: `checkpoint turn 1: 124 byte patch (1
untracked file(s))` on the machine where every capture had been failing.

## Session nine, in one paragraph

ADR-0013's last two rows, the ones its own table left open: **a run that cannot make its
deadline says so**. They turned out to be one question asked about two waits, so there is one
core function — `Run::prospect_at(at)`, *where will this run stand at the instant this delay
ends* — and two callers. A rate limit says when it lifts, so the answer arrives an hour before
the deadline does: at 07:00 a run due at 08:00 whose account frees at 10:00 has already been
decided, and waiting to find out wastes the hour somebody could have used. A fleet that refused
the run says nothing about when it will change its mind, so the instant is `now` and the only
thing establishable is whether the deadline has already gone — the honest reading rather than a
degenerate one, because the alternative is a forecast and nothing here knows how long an agent
turn takes.

The trap appeared for the fourth time and this was the place it would have been loudest: an
unspecified deadline means the moment of submission, so a rule that skipped the check would have
announced *every ordinary run in the fleet*. It is a `Prospect::NoStatedDeadline` variant now
rather than a comment, so a caller matching only what it cares about cannot lose it. And the
other half of getting it right is that the alarm **changes nothing**: `note_overdue` writes a
typed `LogKind::Overdue` into the run's own log and the run keeps being offered, keeps its
checkpoint, keeps its lease, and still says `pending` in `ps`. Said once per stated deadline,
latched in memory — so a moved deadline earns a fresh answer, which is the one intervention that
makes the old announcement wrong rather than repetitive — and written to the log rather than only
to `tracing`, because `WARN` on a machine nobody is logged into is not telling anybody, and the
log is what ADR-0010's delivery plane will fan out from unchanged.

Verified live on one node with a fake agent — a shell script emitting a plausible turn, a
`rate_limit_event` and a result, which is the fake the last session said this needed. Due in 5m
with the limit lifting in 2h gives "1h54m past it, and its account's five_hour rate limit lifts
after the deadline"; due in 3h gives the rate-limit line alone; no deadline at all gives nothing,
which is the trap not firing. Then the placement half on a fleet of one: a queued run against a
repository that does not exist, due in 20s, announced once at 11.2s past — once across four
refused rounds — and again after `offload deadline <run> 15s`, with `ps` still showing it pending
and `explain` saying "…offers it to the fleet again until somebody takes it, and it is 33.3s past
the deadline it was given, which changes nothing about that".

One thing fixed on the way, because it was a hole in the feature rather than a separate one:
`logs` resolved a run to its *holder*, and a pending run has none — so the arbiter's
announcement was written somewhere nobody could read it from another machine. It falls back to
`arbiter_for` now, which is safe to follow because a node that is not the holder reports its leg
as ended, so the poll stops instead of waiting on a node that will never produce a turn.

Then, in the same session, the thing that made the previous paragraph worth having: **the
delivery plane** (ADR-0010). `LogKind::Overdue` was written to be fanned out, and until this it
reached whoever read the log. Now a notification is projected from the log, queued in a per-route
outbox, and carried either by a command the node's owner nominated or by a *peer* that holds the
credential — which is the demo the ADR was written for: a device that hosts nothing tells you your
run is done. Details in the next two sections, including the three things the demos found: a
failed *checkpoint* logged as a failed *run* (which hung up every follower mid-run and notified a
failure that had not happened), `offload sinks` counting a given-up notification as a delivered
one, and — the one that mattered — a queued notification for a phone that was merely *asleep*
being thrown away after fifteen seconds.

## The delivery plane, local half — what exists now

ADR-0010's forward direction: a notification derived from the event log and delivered by the node
that logged it. Eight things to know before touching it.

- **The projection is the whole of what counts as news.** `offload_core::notify::notable` maps one
  log entry to at most one `Notification`, and the subset is three variants: finished, failed,
  will-miss-its-deadline. Everything else in the log is deliberately absent, and the interesting
  absences are the ones that look terminal — a released checkpoint (a run being *handed on*), a
  cancellation (they just did it themselves), and a per-turn rate-limit report.
- **`NOTABLE_KINDS` lives next to the projection**, because the store scans on the denormalised
  `kind` column and a filter list that drifts from the projection silently stops finding a kind
  somebody added. A test asserts every projecting kind is in it.
- **Fan-out is an outbox, two mechanisms on purpose.** A cursor per sink bounds the scan; a row
  per `(sink, seq)` is ADR-0010's dedup identity and carries the outcome. Advance a cursor only on
  success and one broken sink swallows everything behind it — the exact failure the plane exists
  to prevent, arriving through the door marked "simpler".
- **A new sink starts at the end of the log.** Not in the ADR, and the plane is unusable without
  it: a cursor at zero hands a route configured this evening every notable event the node ever
  logged. `Store::sink_cursor` initialises it; `existing_sink_cursor` is the read-only twin, so a
  status command cannot start a route by looking at it.
- **The content is never copied into the outbox.** A row holds the identity and the notification
  is re-derived from the log every time. Two versions of one fact eventually disagree.
- **The sink is a command the owner nominated**, and the *service* is a separate declaration from
  the *transport*. That looked redundant and is what makes "reach me by push" expressible as a
  `Constraint` later. `authenticated` is the one thing the machine can check — the program
  exists — and it cannot check that the script reaches a human.
- **No templating.** The notification arrives as JSON on stdin (a `Payload` struct that exists
  because it is an *interface*: `RunId` encodes as an array of bytes on the wire, which is
  unusable in `jq`), as `OFFLOAD_*` variables, and as one final argument holding the summary. A
  template language would be a quoting bug with a syntax.
- **Delivery is its own tick, beside `tend_own_runs`.** Never something a turn boundary or a
  checkpoint waits on, and never in the mesh tick — the mistake that has now cost this project
  three bugs, and a fleet of one is exactly where an agent falling over at 02:00 still needs an
  answer.

**Verified live on one node**, with a fake agent and a two-line shell script as the sink: a
finished run delivers once ("finished after 1 turn(s), $0.0123"), a run that cannot start delivers
its reason once, a *capture* failure delivers nothing, a route pointing at a missing program
accumulates and then gives up loudly without affecting the working one, and a sink added to the
config after four runs had finished hears nothing about them and everything about the next one.

**And the bug the demo found, which is not about delivery at all.** A failed checkpoint was
logged as `LogKind::Failed` — the *run's* terminal state — with a comment right above it saying a
failed capture is not a failed run. Everything downstream believed the log rather than the
comment: `is_terminal` ended every follower's stream mid-run, and with it the attendance that
decides whether a failure resumes itself; and the new projection told somebody their run had
failed a minute before telling them it had finished. `CaptureFailed` is its own variant now
(wire v9), rendered mid-stream by `offload logs`. The lesson is bigger than the fix: **a log kind
named after a state must not be borrowed for an operation inside it**, because a log is read by
things that cannot ask what was meant.

**A smaller one, found by reading the output**: `offload sinks` reported four successful
deliveries for a route that had never worked, because delivered and abandoned are both resolved
rows. Three numbers now — delivered, waiting, dropped — and a route that is usable *today* says
how many it dropped earlier.

## …and the fleet half, which is the demo the ADR was written for

A run finishes on a machine with no route to a human, and a device that hosts nothing tells you.
Five things to know.

- **`Deliverer` is its own registration beside `Host`**, and that is deliberate rather than tidy.
  It would have been cheaper to hang `deliver` off the trait the cluster already uses to reach the
  daemon — that trait has absorbed non-hosting methods before — but the device this plane exists
  for answers `NoHost` to every run and implements this one. A plane that is "kept apart" in prose
  while sharing an interface in code is apart only until somebody is in a hurry.
- **The sender keeps the outbox.** The node that logged the event is the only node that knows what
  it has already said, so it is the only one that can be at-least-once about it. A peer receives an
  ask, runs its route, replies, and forgets. Being asked twice is the contract working.
- **Nothing about a route travels.** `Deliver { sink, note }` carries a notification and the id of
  a capability the peer advertises. The command behind it, its arguments and any credential stay
  where they are, and the routing decision is made from gossiped capabilities alone.
- **Away is not gone**, and getting that wrong broke the promise. The first version treated an
  unreachable route as a removed one: the queued notification was retried three times in fifteen
  seconds and abandoned — which is exactly backwards, since the promise is *you will be told when
  you pick your phone up* and a phone is asleep for hours. A route on a device the fleet still
  knows is listed, queued for and waited on **indefinitely**, with nothing attempted and nothing
  spent from its retry budget; only a route the fleet no longer lists at all is given up on. Same
  distinction as ADR-0007's *unreachable is not dead*, on the other plane. The cost is stated
  rather than hidden: rows accumulate for a device that never returns, at tens of bytes each, and
  the answer for a device gone for good is to revoke it.
- **`offload sinks` explains a silence.** It shows the fleet's routes as well as this node's,
  including the ones that cannot be used — unauthenticated, or on a device that is not answering —
  because "there is a phone in this fleet and nothing reaches it" is the question being asked, and
  omitting the row answers "there is no phone".

**Verified on two daemons.** The phone joined with `{submit, deliver}` and was never granted
`host-runs`; the desktop has no route of its own and says "no delivery routes on this node — it
uses the fleet's". A run finished on the desktop and the phone said so, logging "carried a
notification for a peer". Then the phone was `kill -9`'d, two more runs finished, and the desktop
reported "not answering — 2 waiting for it to come back" with the attempt count untouched; the
phone came back thirty seconds later and was told about both, in order.

**Next, on this plane:** the reply path (ADR-0008's `PermissionMode::Ask`), which needs the
fencing rather than the transport. The audience — which route a given run *asks* for — was the
next thing on this plane and is the section above this one now.

One smaller thing worth knowing: `sink_id_of` recovers a route's id from the `sink:<id>`
capability convention — one place, by design, because two copies of a convention is one copy that
will be wrong. (The other note that used to sit here, about a peer's name, was a misdiagnosis; the
corrected version is in the demo-friction list below.)

## Session eight, in one paragraph

ADR-0013's other axis — **capacity as a budget** — and the commitment review that needed a
deadline before it could exist. A run declares a `Demand` (light, normal, heavy), a device has
a budget of shares beside its run count, and the count stays the owner's hard ceiling because
"never more than two agents on my laptop" is a different statement from "at most half this
machine". Saying nothing costs nothing: the budget defaults to `Normal × max_concurrent_runs`,
so a fleet that never mentions demand behaves exactly as it did. `offload-probe` measures the
one-minute load average per core now — the first time this project has read it at all — and it
is an `Option`, because a plausible zero on a platform that will not answer wins bids the
machine should lose.

Two rules came out of the writing rather than the ADR. **A lone run always fits**: a heavy run
wants more shares than a phone's whole budget, and refusing it there turns a concurrency number
into an eligibility rule, so the run is unplaceable across a fleet of small devices rather than
slow on one. **Pressure gates accepting, never starting**: a committed run has nowhere else to
be, so refusing to start it until the load average improves holds it hostage to a number nobody
controls. That second one is what gave `NoBid::Busy` its real meaning, which is not the one the
ADR wrote down — being *full* is this node's own queue, known and self-emptying, so a full node
still commits (ADR-0006); being under *pressure* is the owner's build on the same machine, and
that is the only case that defers, judged against the run's slack.

Then the way out of a commitment: `review_commitment` gives back a run this node cannot start
before it is due, **offering it before letting go** so the run is never left `Pending` in
nobody's hands, and only for a deadline somebody *stated* — the same trap as `grace_for`, in a
new place, because with an unspecified deadline every held run is overdue within a second and
the rule would bounce every commitment in the fleet from one queue to another for ever. It does
not anticipate, either: giving a commitment back early needs an ETA for this node's own slot,
and `WhenFree` carries a count precisely because nothing here has learned to guess.

Then ADR-0013's third axis, which finishes it: **attendance**, observed rather than declared,
deciding what happens when a run breaks — unattended resumes itself, attended stays broken in
front of the person sitting there. The part that would have been wrong the obvious way is
*when* it is observed: a client following a run stops following when the stream ends, and a
failure ends the stream, so sampling at decision time finds nobody watching every single time
and the attended branch is dead code that looks correct. It is sampled when the run is written
`Failed`, held in memory by the node that failed it, and gossiped nowhere — a `Failed` run is
terminal to `supervise`, so this is the holder's decision and the holder has the checkpoint, the
worktree and the stream. A daemon that restarted therefore knows *nothing*, which is a third
answer rather than "unattended": those runs still wait for `offload resume`, because the
shortcut hands a crash-looping daemon every run on the machine on every start.

## Session seven, in one paragraph

ADR-0013's deadline, which is the prerequisite the rest of phase 4's open boxes were waiting
on: a busy node cannot judge a deferral, a hold-down cannot know how much flakiness a run can
afford, and nothing can decide what to start first, until a run can say when it is due. A run
carries `deadline: Option<Millis>` and **nothing carries an urgency level** — slack is
`f(deadline, now)`, so a run gets more urgent as the morning approaches with no edit, no
message and no revision counter, and every node computes the same number from state it
already has. An unspecified deadline means the *moment of submission*, which makes slack
negative age: aging for free, starvation impossible without an anti-starvation mechanism.

That last property is also the trap, and finding it is most of what this session was. It
makes every ordinary run overdue within a second, so a rule that let slack shorten the
hold-down would move every run in the fleet fifteen seconds after a Wi-Fi handover — the
thrash ADR-0007 exists to prevent, arrived at through the door marked "scheduling hint". The
split that came out of it: **ordering and pickiness use slack whatever its origin; patience is
shortened only by a deadline somebody stated**, and never below `min_grace`. `grace_for`
returns what it waited *and* which of the two numbers decided, because "it moved after twenty
seconds when the policy says forty-five" is a bug report unless the answer says which.

Then the only ordering this system has — the runs a single node holds — became least-slack
first with priority as the tiebreak, and writing the test for that turned up **a deadlock
accept-without-starting had all along**: `start_held_runs` asked how many runs were *held*
rather than how many agents were *running*, so three runs granted to a one-slot node leaves
two commitments each reading the other as the reason it cannot begin. Nothing fails; the
leases renew and `ps` says `assigned` for ever. Two submissions cannot show it, and two is
what the commitment demo had.

Finally `offload deadline <run> 45m|none`, forwarded to the node that owns the field — the
run's arbiter — and arbitrated by `deadline_rev`, because applying it locally and letting
gossip sort it out is two nodes each writing revision 1 and undoing each other. The reply says
what the change *does*: pending re-opens placement, running changes nothing anybody can see
until something goes wrong, finished is refused. ADR-0013 calls overclaiming there the most
tempting lie in the design, so the CLI prints the caveat every time.

## Session six, in one paragraph

`offload explain <run>` — the last unticked box in phase 4's placement work, and mostly a
matter of printing what was already there. Every refusal, hold and deferral in this system was
kept as a structured value for exactly this day; nothing had ever surfaced them per run, so
the two forms of the same question — "why is my run still pending" and "why did nobody move
it" — were answered by reading a daemon log. It has two halves: `offload_core::supervise`
asked one extra time for a human (so the explanation cannot describe one thing while the loop
does another), and `Cluster::canvass` — the same bounded round a submission runs, granting
nothing, keeping every answer including the silences. `place` is now built on `canvass`, which
is the whole of the change to placement. **It re-asks rather than replaying**: a bid describes
one second, and showing the round that placed a run as if it were current is a confident wrong
answer. Then the demo, which as usual is where the real bug was: three runs submitted for a
repo the node had never seen, and two of them died before starting because `git clone` into
the mirror's final path is not atomic — leaving a directory with no `HEAD` that every later
run on that repo would clone over and fail on identically.

Then the third item on the list: **run stats travel**. `offload ps` on the submitting node had
the state right and said "0 turns, $0", and after a migration neither machine had all the
numbers. They gossip now as `RunProgress`, beside the run records rather than inside them, with
the owner and the arbitration rule written down as CLAUDE.md demands — owned by whoever is
running the turns, arbitrated **forward only**, ties settled by the author's own timestamp. The
rule started as "defer to the run's holder" and the demo broke it within the hour: a finished
run has no holder, so the moment a run's numbers are final is the moment nobody is entitled to
state them — and a holder's last act is to write the summary of the worktree it leaves behind,
which moves no counter. Two smaller things had to be true first: starting a run stopped wiping
its numbers (`start_run` wrote `RunStats::default()`, which is how a migrated run arrived
claiming it had done nothing), and cost and denials became cumulative like turns, so a run
resumed at turn 20 no longer reports the price of its last leg as the price of the run.

## Session five, in one paragraph

Two of phase 4's open boxes, both built and then verified on real daemons: arbiter failover,
and a busy node committing to work instead of declining it.

Arbiter failover was the last unticked migration box. The supervision loop's *decision* is
now `offload_core::supervise`, a pure function of (view, run, now) that says who arbitrates,
whether the holder counts as gone, and what the hold-down makes of it; `mesh::supervise` is
left with the two effects.
Writing those cases down changed two rules: a **suspected** home node keeps arbitrating,
because failing over on a guess it can refute means two arbiters granting one run twice, and
a home node this view has **never met** yields no arbiter at all rather than failing over,
since "not met yet" is every node's first second of gossip. Then the demo, which is where the
real bugs were: `kill -9` the submitter *and* the holder at once, and the third node picked
up arbitration, orphaned the run, held it down 15 seconds, reassigned it to itself, and
resumed the same conversation at turn 8 of 31 — but only after fixing a plugged-in laptop
that had been refusing every checkpoint replica because its battery reads 0% while charging.
Nothing about that failed loudly; the checkpoints just quietly stayed on one machine.

Then the other half of the overnight case: a node at its concurrency cap now **commits**
rather than declining — it bids with `Availability::WhenFree { behind }`, takes the grant into
`Assigned`, and starts the agent when a slot frees, so `offload run` answers "starting when
its current run finishes" instead of "no node will take this run". One slot and two
submissions on a real daemon: the second was held for 23 seconds and then started by itself.
That work turned up the session's second silent bug — **nothing had ever renewed a lease**,
which a merge rule had been quietly papering over.

## Session four, in one paragraph

Phase 3 start to finish, merged, and then most of phase 4 — which means the sentence on the
front of the README is now true of the code. ADR-0012's passphrase became argon2id over a
diceware phrase; ADR-0015 settled the transport by deciding its *address* is a node key,
which made quinn-now-iroh-later a swap rather than a fork; `offload-proto`,
`offload-transport` and `offload-cluster` followed, each with the pure part separated from
the I/O so partitions could be arranged rather than provoked. Then ADR-0016 on blob transfer
and durability, the bid round, run gossip, and migration in both directions: `offload drain`
hands work over mid-conversation, a `kill -9` is noticed and held down before the run moves,
and a suspended node reclaims its run for free. **Almost every bug worth remembering came
from running real daemons rather than from a test** — a probe timeout that covered the answer
but not the connect, a 64-character node id in a 63-byte DNS label, a transcript slug that
made a migrated agent start a fresh conversation while reporting it had resumed one, and two
merge rules that were right about which state looked further along and wrong about who was
allowed to say so.

## Session three, in one paragraph

Phase 2 was finished and merged; five ADRs were written, all of them settled by argument
rather than by default. The through-line: reporting is a capability, not a subsystem
(0010); capabilities are instances with roles and an identity, and agent-facing prose is a
per-run projection rather than a gossiped field (0011); the fleet is a passphrase with no
privileged device, approvers hold delegated issuing authority, and losing the secret means
re-founding (0012); capacity is a budget with a deadline rather than a count with an urgency
level, and urgency is derived from the clock rather than stored (0013); a submission is
accepted by a node or refused to your face, because anything else is babysitting (0014).
Then phase 3 started with the identity work 0012 made a prerequisite.

## Phase 3, done — what exists now

**The demo works with nothing configured.** Two daemons on one machine, real QUIC, no seed
list: they find each other over mDNS, each lists the other with cores, memory and agent
version, `kill -9` produces `suspect` then `dead`, a restart shows the absence on the record,
and `SIGTERM` produces `draining` with no detection timeout. A real agent run submitted on
one node shows up as a run in the *other* node's `offload nodes`, and shows up finishing.

The pieces, and the one thing about each that is easy to lose:

- **`offload_core::fleet`** — certificates, delegations, revocations, canonical signing bytes
  with per-type domain separation. `passphrase` derives the fleet key with argon2id over a
  six-word diceware phrase (EFF wordlist, vendored whole). *Salt, parameters, wordlist and
  normalisation are one wire format: change any of them and every certificate in every fleet
  stops verifying.*
- **`offload_core::view`** — merge by fact ownership. *Two rules in one struct: self-reported
  facts move on a higher incarnation, liveness also moves on stronger evidence at equal
  incarnation, or a `Suspect` could never spread.*
- **`offload-proto`** — framing, version ranges, and `admit`. *It takes what the peer claimed
  and what the transport authenticated as separate arguments; pass the claim for both and
  every check becomes decorative while the tests stay green.*
- **`offload-transport`** — the trait, an in-memory implementation with `partition`/`heal`,
  and QUIC with RFC 7250 raw public keys. *Nothing above this crate knows an address.*
- **`offload-cluster`** — a pure, clock-injected detector and the loop that drives it.
  *`probe_round` takes the time, which is why the partition tests are readable.*
- **`offload-node::mesh`** — assembles them, plus mDNS and the polling that keeps this node's
  own entry honest. *A node with no fleet joins nothing, and that is not a degraded mode.*

What phase 3 does **not** have: blob availability gossip and peer fetch. Nothing needs it
until a run migrates, which is phase 4.

## Phase 4, most of the way — what exists now

**Placement.** `offload run` offers the submission to the fleet and returns the node that
took it, or every reason nobody would (ADR-0014). One bounded round: the arbiter asks each
peer directly rather than nodes broadcasting bids after a delay — recorded as an amendment in
ADR-0006, since a transport that addresses peers makes the delay unnecessary. A grant is an
offer; a node that bid and then went busy declines, and the next-best bid gets it.

**Durability.** ADR-0016 (accepted 2026-08-07, after building and running it): blobs are fetched
by asking, integrity comes from
the hash so any member may be asked, and a checkpoint is not durable until a second node holds
it. Capture pushes to the node most likely to take the run over, so the common migration
transfers nothing. `offload ps` says `replicated` or `here only` per run.

**Migration, both halves, verified against real daemons and a real agent.**

- `offload drain` (and SIGTERM): each run is checkpointed at its next turn boundary — never
  mid-turn — and offered to the fleet. A run started on A resumed on B *in the same
  conversation* and finished. A run still mid-turn at the deadline keeps its turn and stays.
- `kill -9` the holder: the arbiter marks the run `Orphaned` within seconds, holds it down per
  ADR-0007, then reassigns; the new node resumed from turn 5 of the same conversation.
- `SIGSTOP` then `SIGCONT`: `Orphaned` and straight back to `running` on the same node at the
  same epoch. No migration, no lost turn.
- `kill -9` the **submitter and the holder at once**: arbitration is per run and lives with
  the run's home node, so this is the case where the arbiter itself dies. Three daemons,
  charlie submits, alpha runs it, both are killed mid-run; bravo — neither home nor holder —
  concludes both are dead, marks the run `Orphaned` ten seconds later, reassigns on
  `HolderDead { absent: 15.2s }`, restores the transcript from the replica it already held,
  and resumes **at turn 8 of the same conversation**, finishing all 25 files. Bringing the
  two dead nodes back shows the run `completed` on both, by the ordinary epoch-ordered merge.

**Fencing acts.** Learning that a run has moved to a higher epoch elsewhere stops the agent
here — two agents on one repository, both committing, is what the epoch exists to prevent.

**The three prerequisites that landed first**: capability instances with roles (ADR-0011,
protocol version 2), `Portability::NodeLocal` in eligibility, and `HostRuns` enforced at bid
time rather than at submit — a node may submit work it is not allowed to run itself, which is
the whole point of ADR-0012 granting `{Submit, Deliver}` at the door.

**Where the code deviates from the layout in `CLAUDE.md`:** the bid round lives in
`offload-cluster::place`, not in `offload-sched`. It needs the view, the connections and the
serve loop, all of which the cluster already has. `offload-sched` is worth creating when
migration *policy* grows past what `offload-core` already decides, and not before.

**Arbitration, now that it has been exercised.** `arbiter_for` is the home node until that
node is *gone*, then the lowest-id available node. Two rules earned the hard way, both in its
doc comment: a `Suspect` home node **keeps** arbitrating (it can refute, and meanwhile it
still thinks it arbitrates — two arbiters mean one run granted twice), and a home node this
view has **never met** returns no arbiter rather than failing over. The decision that follows
is `offload_core::supervise(view, run, now, policy)`; `mesh::supervise` performs it.

**Commitment, since a full node now takes work rather than declining it.** Being at capacity
was the one refusal that was really a "later", so `evaluate` returns a bid carrying
`Availability::WhenFree { behind }` instead of `NoBid::Refused(AtCapacity)`; `winner()` puts
anybody who can start now ahead of anybody who cannot, outright; and the granted node parks
the run in `Assigned` until `start_held_runs` finds it a slot. Three things to know:

- **`behind` is a count, not an ETA.** A node knows how many runs are ahead of it and does not
  know how long an agent turn takes. ADR-0013's budgets can make this a real estimate; until
  then nothing here has learned to guess.
- **Leases are renewed now** (`Supervisor::heartbeat`, every 5 ticks). Nothing ever renewed
  one before, which was survivable only because the merge rule reclaimed the run — see the
  hard-won list below.
- **A commitment does not leave with the node that made it.** `Run::release` hands an
  unstarted run back to the pool at a fresh epoch, and `drain` does that before it starts
  waiting for turn boundaries.

**Explaining a run, now that it exists.** `offload explain <run>` prints who arbitrates the
run and what this node's supervision pass makes of it, then what every node says about taking
it. Four things to know before changing it:

- **It re-asks; it never replays.** `Cluster::canvass` runs the same round `place` does and
  grants nothing. Storing the bids that placed a run and showing them later would be showing
  somebody what the fleet thought at 09:00 to explain what it is doing at midnight.
- **The top half is `supervise` itself**, not a second implementation of it. An explanation
  computed by different code from the decision it describes will disagree with it eventually,
  quietly, on the day somebody is relying on it.
- **Silence is its own verdict.** `Verdict::Silent` is not a refusal — a node mid-restart has
  said nothing, and `place` still ignores it, because a non-answer is not a reason nobody took
  the run.
- **It works from any node.** A run known only through gossip explains identically and names
  its arbiter rather than pretending to be one; `find_run` looks in the store and then in the
  view, and both halves are needed.

Verified on two daemons and a real agent: a live run marking the peer that would win now while
its holder answers "already holds this run"; the same run explained from the node that has
only heard about it ("alpha arbitrates it; every other node watches"); a peer refused for the
one reason that is neither capacity nor capability ("host-runs granted but on probation for
another 11 minutes"); and `kill -9` on the holder, where `explain` names the arbiter moving
from the dead node to the survivor. The hold-down sentence — "holding it for up to 33s more:
its holder has been out of contact 12.0s, and this run waits 45.0s before moving" — is covered
by a unit test rather than by the demo: the agent finished every prompt in two to five turns,
so the run was over before the detector had concluded anything.

**Run progress, now that it travels.** Turns, cost, denials and the worktree summary gossip
as `RunProgress` beside the run records. Four things to keep in mind:

- **One author, forward only.** Only the node running the turns produces these numbers;
  everybody else relays them verbatim, and a relay that has fallen behind is behind rather
  than wrong. So the merge rule needs no "who is speaking" test — unlike `merge_run` right
  next to it, which is nothing but that test.
- **The tiebreak is the author's clock, and `save_stats` does not restamp it.** Two records
  can describe the same turns and the same cost and still differ, because the worktree summary
  moves on its own. Restamping on arrival would let a node relaying a stale copy make it look
  newer than the record it overwrites.
- **Cumulative for the run, not the leg.** `start_run` used to write `RunStats::default()`
  while preparing the workspace, so a migrated run arrived reporting nothing; cost and denials
  used to be assigned from each agent process rather than added, so a resumed run reported its
  last leg's bill as the whole. Both fixed, and the drain demo shows the arithmetic: 19 turns,
  $0.321 and 3 denials on *both* nodes after a mid-conversation handover.
- **`ps` turns and checkpoint turns are two different counters.** `ps` shows the agent's own
  `num_turns`, checkpoints and `explain` show our turn-boundary count, and after a migration
  they diverge visibly — 19 against `checkpoint turn 6` in the demo above. Both are honest and
  neither is wrong; nothing has decided which one a human should be shown.

**Capacity, after the demo that broke it.** Three fixes that are one idea — a decision taken
from a picture that was out of date, or that was never true:

- **One lease** (`offload_core::LEASE`, sixty seconds). It was an hour for a locally submitted
  run and a minute for a granted one; a run does not hold differently depending on how it
  arrived.
- **The heartbeat runs whether or not there is a fleet.** It lived in the mesh's gossip tick,
  so a node with no fleet renewed nothing — invisible while its leases lasted an hour, and a
  countdown the moment they did not.
- **A node that submitted work elsewhere is not busy.** `running_count` counted every
  unfinished run in the store, including the ones it had placed on peers, so a node reported
  `runs 6/2, at capacity` while completely idle.
- **Capacity is written down the instant it changes**, by the arbiter that granted the run and
  by the node that accepted it. Six submissions in two seconds used to land on one machine
  while the other sat empty, because every round after the first read a view taken before the
  previous grant.

**A submission that survives the laptop.** `offload run` used to report success as soon as a
node accepted the run — and when that node was *this* one, the run existed on exactly one
machine, the one about to be shut. It now waits for a peer to write the record down first, and
says so when there is nobody: "no other node has a copy — if this machine goes away, so does
the run". No new message was needed; a probe carries the whole view and a peer persists what it
learns before it answers, so the `Ack` is the receipt.

**`--queue`, and the loop that makes it mean something.** ADR-0014's opt-in: a submission
nobody will take is refused to your face *unless* you asked for it to be left pending, in which
case it is written down, gossiped, and offered again by its arbiter until somebody takes it —
`Supervision::Place`, rate-limited by the same backoff a reassignment uses. Two rules worth
knowing before touching it: a run a human parked with `offload checkpoint` is `Pending` too and
must **not** be picked up on their behalf (the checkpoint is what distinguishes them), and
`queue` lives on the spec because the node that eventually places a run is not always the one
that took the request. Verified by queueing a run against a repository that did not exist,
creating it, and watching the run start on its own at epoch 1.

**Deadlines, now that a run has one.** ADR-0013's first axis, and five things to keep straight:

- **Urgency is not a field.** `Run::slack(now)` is the whole of it. Nothing gossips urgency,
  nothing bumps it, and two nodes cannot disagree about it — which is the argument for a
  deadline over a stored level, not a side effect of one.
- **`None` means the moment of submission**, so slack is negative age and a waiting run gets
  more urgent on its own. That is the anti-starvation property and it costs nothing.
- **…which is why derived urgency must not cut the hold-down.** Every run with no deadline is
  overdue within a second. `grace_for` shortens the wait only for a *stated* deadline, floors
  it at `min_grace`, and reports `DeadlinePressing` rather than `GraceExpired` when the
  deadline was what decided — the two are indistinguishable in a log otherwise, and the first
  question about an early migration is which number caused it.
- **The deadline is the only mutable field in a spec**, so it is the only one with an owner and
  a counter: the home node (its arbiter successor once it is gone), `deadline_rev`, settled in
  `merge_run` *before* the record is. The holder is believed about a run it is running — that
  is what makes turns travel — so without this it re-asserts the deadline it started with once
  a second and the edit silently reverts.
- **`offload deadline` on a running run changes nothing visible**, and says so. There is no
  preemption and no making an agent think faster; what it buys is different behaviour when the
  run breaks. A CLI that let somebody believe otherwise would be the one lie this design is
  most tempted by.

**Capacity as a budget, and the two rules the ADR did not have.** ADR-0013's other axis, with
an amendment recording the four places building it changed the design. Six things to know:

- **Saying nothing costs nothing.** `budget_shares` defaults to `Normal × max_concurrent_runs`,
  so a config that never mentions a budget admits exactly what it used to. `Demand::Normal` is
  four shares rather than one for the same reason: a scale with no room below the default cannot
  say that four light runs fit in the space of one agent run.
- **A lone run always fits**, whatever it costs. A heavy run wants more shares than a phone's
  entire budget, and refusing it there turns a concurrency number into an eligibility rule —
  the run is then unplaceable across a fleet of small devices rather than slow on one. The
  budget limits what runs *beside* something; a `Constraint` is what refuses a run outright.
- **Pressure gates accepting, never starting.** `admits` and `admits_start` are two calls over
  one rule (`Capacity::room_for`) taking the two counts CLAUDE.md warns about — held runs for
  whether to take more, running agents for what to begin. The second deliberately asks nothing
  about load: a committed run has nowhere else to be, so refusing to start it until the machine
  calms down holds it hostage to a number nobody controls.
- **`NoBid::Busy` is for pressure only**, and the line is which queue the node is behind. Full —
  by count or by budget — is this node's own queue, so it commits (ADR-0006). Under pressure is
  the owner's build on the same machine: never promised, undrainable from here, so it defers,
  and only when the run's slack can afford `PRESSURE_RETRY`. An overdue run is told to look
  elsewhere *now*, because being picky is how a late run gets later.
- **The load average is measured for the first time**, per core, one-minute, and it is an
  `Option`: a platform that will not answer produces `None` rather than a plausible zero, which
  would win bids this machine should lose. It also makes `BidWeights::load_penalty` real, having
  been multiplied by a hard-coded zero since it was written.
- **`retry_after` is a constant tied to its input.** A one-minute load average cannot see a
  spike shorter than a minute, so promising to be free in fifteen seconds would be a claim about
  something the node cannot observe. Turn-duration history would make it an estimate; nothing
  measures that yet.

Verified live rather than only in tests: fourteen spinners on an eight-core laptop, and
`offload policy` refuses all three demand levels with "cpu at 100%: 100% of the budget is free,
the machine is not" — the budget and the machine disagreeing, which is the whole point of
printing both numbers.

**Giving a commitment back.** `review_commitment(run, ready_elsewhere, now)` on the holder,
run from the daemon tick beside `start_held_runs`. Four things to keep:

- **It offers before it lets go.** Release, run a round, and if nobody wants it, take the
  commitment back — a run is never left `Pending` in nobody's hands, which is ADR-0014's whole
  rule applied to a decision this node took on its own.
- **Only a stated deadline moves it**, for `grace_for`'s reason in a new place. Every held run
  with an unspecified deadline is overdue within a second.
- **A pinned run is kept however late it is**: `assign` refuses a second attempt, so releasing
  one makes it unplaceable rather than re-placeable.
- **`ready_elsewhere` asks for room, not eligibility.** Handing a late run from this queue to
  another queue costs a bid round, an epoch and an attempt and buys nothing. It reads gossiped
  capacity, so it is a tick stale rather than fictional — and being wrong costs one round, which
  is the trade `may_retry`'s thirty seconds bounds.

**Attendance, and the recovery it decides.** `offload_core::recovery` — five things to keep:

- **Observed at the failure, not at the decision.** The whole reason the module has a section on
  it: a failure ends the stream a follower was reading, so "who is watching now" is always
  nobody, and the attended branch would look right and never run.
- **In memory on the node that failed the run**, because a `Failed` run is terminal to
  `supervise` and recovery is therefore the holder's business — nothing to gossip, nothing to
  own, nothing to arbitrate. A restart forgets, and forgetting escalates.
- **Unknown is not unattended.** The startup recovery path notes `None` deliberately, so an
  interrupted run keeps today's behaviour and says why. The shortcut — nobody can be watching
  after a restart — gives a crash-looping daemon every run on the machine, on every start.
- **The deadline never decides whether to resume, only how long to wait first**, and only when
  somebody stated one. Third appearance of the same trap.
- **A resume that could not even start still counts.** Otherwise a run that cannot resume here
  for a reason of its own retries at tick rate; counting it puts the run in front of a person
  after three tries instead.

**Verified on a real daemon, and the demo is worth keeping.** Point `agent.binary` at
`/bin/false` — the cheapest possible dead agent — and both branches show up on one node with no
fleet and no model spend: `offload run` gives `attendance=unattended`, and `offload run --follow`
gives `attendance=attended` and then "leaving it failed: somebody was watching it when it
failed". It found two bugs before it found what it was looking for:

- **A run whose agent exits without a result stayed `Running` for ever.** No `Result` event
  means no terminal transition, and a live holder renewing its lease means nothing orphans it
  either — so `ps` said `running` indefinitely and recovery had nothing to recover.
  `fail_if_unfinished` covers it. The general lesson is in CLAUDE.md: a state machine driven by
  events needs an answer for the stream simply *stopping*.
- **Recovery was in the mesh tick**, which is spawned only for a node in a fleet — the same
  mistake the lease heartbeat made, invisible for the same reason. Both are in `tend_own_runs`
  now, which is about a node's own runs and needs nothing from the mesh.

What the demo cannot show without a real agent turn is the *resume* branch: a run whose agent
died with a checkpoint behind it. `/bin/false` never gets a turn in, so it escalates on
`NothingToResumeFrom`, which is correct and is not the interesting case. Unit-tested both ways;
a real one wants a fake agent that emits a plausible turn and then dies.

**Priority, and the mechanism it shares.** `offload priority <run> 10|+10|-5`, and four things
to know:

- **One counter, one owner, one message.** `deadline_rev` → `spec_rev`, `SetDeadline` →
  `EditSpec { edit: SpecEdit }`. The two editable fields have the same owner by design, so a
  record at revision N carries the spec after N edits whichever field moved. A second counter
  would be a second merge rule to get wrong.
- **Absolute, not a delta**, because a delta has to be added to a value that may be a gossip
  tick old — `+10` twice would sometimes mean 20 and sometimes 10. `+10` parses as 10, and
  negative values need `allow_negative_numbers` on the subcommand or clap reads `-5` as a flag.
- **Struct variants, not newtypes.** `Deadline(Option<Millis>)` compiles and fails at *runtime*:
  serde's internally-tagged representation cannot encode a newtype variant wrapping an `Option`.
  Same family as the sequence case already in CLAUDE.md, found the same way — a round trip over
  a real exchange.
- **"Unchanged" is computed from the value before the edit, and only where the edit was
  applied.** From the amended record every priority change looks like a no-op; from a forwarded
  edit there is nothing local worth claiming.

**And attendance is deliberately not an input to the hold-down** — recorded as an amendment,
because the ADR's table asks for it. Attendance is observable only where the stream is served,
the hold-down is the arbiter's decision about a holder that has vanished, and the one node that
could answer is the one that is gone. Gossiping it would hand the arbiter a value from before
the silence and present it as current. The prerequisite is a feature rather than a field:
following a run placed elsewhere, proxied through the fleet.

**Following a run that is somewhere else.** `offload logs -f` and `run --follow` against a run
on a peer, which also closed the attendance gap the last session left. Five things:

- **A poll, not a stream.** One `FetchEvents { after, limit }` per second per follower, forwarded
  by the node in front of the operator. No long-lived stream to keep alive across a migration or
  a daemon restart, and nothing that can miss an event.
- **`done` is not `page.is_empty()`.** A page that filled means more *right now*; `done` means
  that holder's log has ended. A run between turns is quiet for minutes, and conflating the two
  is how a follower either gives up early or waits for ever.
- **`LogEvent` lives in `offload-core` now**, because the run's output crosses the wire and
  `offload-proto` cannot depend on `offload-node`. The clock stayed behind: `LogEvent::at(now,
  kind)`.
- **Being asked is the attendance observation.** A peer polling for a log is a follower
  elsewhere, counted for `WATCHER_GRACE` (10s) after its last poll — a poll is what it has
  instead of a connection. Without this a run followed from another machine looked unattended and
  would have resumed itself behind the person watching.
- **A departed follower has to be noticed.** The demo found that it was not: the loop only
  discovers a closed client when a write fails, and a quiet run writes nothing, so an abandoned
  `logs -f` polled for ever *and* kept the run attended. Both follow paths now watch the client's
  read half for EOF.

Known limit, written into the code: sequence numbers are per node, so a migrated run's log is
this node's leg followed by the holder's — a concatenation, not a merge. A run that bounced
A → B → A interleaves in a way nothing sorts, and sorting two machines' log lines by their own
timestamps would be a bigger lie than the concatenation.

**A run that cannot make its deadline says so.** ADR-0013's last two rows, and five things to
keep:

- **One function, two waits.** `Run::prospect_at(at)` asks where the run stands at the instant a
  delay ends: a rate-limit reset, or `now` for a wait with no end in sight. Passing `now` is the
  honest case rather than a degenerate one — with no end to wait for, the only establishable fact
  is whether the deadline has already gone, and anything more would be a forecast.
- **Only a stated deadline is announced.** The trap's fourth appearance, in the place it would
  have been loudest: unspecified means the moment of submission, so the rule without this check
  announces every ordinary run in the fleet. It is a variant (`Prospect::NoStatedDeadline`), not
  a comment.
- **It changes nothing.** `note_overdue` writes a `LogKind::Overdue` and stops. The run keeps
  being offered, keeps its checkpoint and lease, and `ps` still says `pending` — because the
  moment "it cannot make the deadline" starts cancelling work, a scheduling hint has become the
  flag that throws away a night of agent turns.
- **Once per stated deadline, in the log.** Latched in memory on the node that established it, so
  a moved deadline earns a fresh answer and a restart repeats itself in the safe direction. In
  the *log* rather than only in `tracing`, because a `WARN` on a machine nobody is logged into is
  not telling anybody, and the log is what ADR-0010's delivery plane fans out from.
- **The placement half fires after a refused round, not from the run merely being late.** It is a
  claim that the fleet was *asked*; between two rounds a perfectly placeable run is pending too.

Known limit, and it is the one already written down about logs: sequence numbers are per node, so
an announcement made by an arbiter that is not the run's home node lands in that node's leg.
`logs` follows the arbiter when there is no holder, so it is readable — but a run whose
arbitration has moved has its log split the same way a migrated run's is.

**Next, in this order:**

ADR-0010 is built in both directions now — the audience (session ten) closed "which route a run
asks for", and ADR-0017 is the reply path. Grant enforcement at the bid exchange (session eleven)
closed the certificate's other half.

Still open: **per-account** concurrency caps (the probe's account fingerprint is
per-machine, open question #5) and the **device-local broker** so two fleets cannot over-commit
one laptop — the one piece of ADR-0013 that introduces shared state, and worth building when a
second fleet actually exists on a machine. Two smaller rough edges from session ten:
`offload ps` shows a blocked-on-question run as `running`, and `offload explain` does not say
where a run's news will go.

Smaller things noticed and left: `Run::reclaim` in core is unused (the merge rule covers the
live case), a reassignment retries at most every 30 seconds — a number picked to stop a log flood, not measured — and
`offload explain` costs one bid window plus a message per peer every time it is run, which is
fine for a person typing it and would not be for anything polling it. `--deadline` takes
durations only (`45m`, `2h`, `1h30m`): a wall-clock `08:00` needs a timezone, which is a
dependency decision nobody has made, and guessing it silently is worse than refusing it.
`priority` is still only a tiebreak in one place.

New this session, and left on purpose: **nothing pushes a re-bid when capacity frees.** A node
that frees a slot arbitrates nothing and cannot grant itself work, so the re-bid would be
somebody else's pass anyway — the arbiter's own loop re-offers a pending run within its
thirty-second backoff, and a push buys up to thirty seconds at the price of the thundering herd
ADR-0013 warns about. **`offload ps` does not show demand** (`explain` does), because a column
that says `normal` on every row is noise; the moment a fleet actually mixes demands, it earns
its place. And the **pressure thresholds are three numbers in `Demand::needs_idle_percent`** —
10/25/50 — chosen to read as a sentence rather than measured, and deliberately not derived from
shares as a fraction of the budget, which would make a small device absurdly picky.

~~Also worth knowing: **peers do not check each other's grants yet.**~~ *Closed in session
eleven*: the arbiter refuses a bid the bidder's certificate cannot back, checked at decision
time and self-healing across re-issued certificates — see the session paragraph above and
ADR-0012's amendment. What remains unchecked is the *sender's* authority (who may arbitrate a
run), which is a different question and is named there.

## Things learned the hard way

Each of these cost real debugging; the short forms are in `CLAUDE.md`.

- **A property test is for a *sentence*; a unit test is for a case.** Session fifteen's three
  findings were all claims about "every" — every transition, every pair of grants, every way of
  writing a constraint down — and none of them was ever going to be checked by arranging a case,
  because arranging a case means choosing the one you already thought of. Two of the three were
  rules stated in the tree's own prose, in exactly one place, and honoured in exactly that place;
  the third was stated in an ADR and contradicted two paragraphs later in the same file. So the
  productive way to write one of these is not "what could break" but **"what does the tree claim
  about everything"** — then say it in code and let the generator argue.
- **Check that a new test fails without its fix.** All four were confirmed by reverting the fix
  and watching the test go red, and two of them needed it: the equal-epoch supersede test passes
  trivially if the arrangement is slightly off, because the assertion is about something *not*
  happening within a timeout; and the aimed two-arbiter property passed for two rounds while
  never once constructing two arbiters. A test for an absence needs proof it can see a presence,
  and a test for a hazard needs to assert its own premise.
- **A fixture built by hand can agree with the code about a world neither lives in.** The blob
  collector's test inserted `{"transcript":"<hex>"}` and passed for two phases while the real path
  deleted live runs' checkpoints, because `save_run` writes `[163,188,…]` and the query looked for
  hex. Both halves were self-consistent and both were fiction, and the *test* is why nobody
  looked. Build fixtures through the writer the daemon uses — `save_run`, never an `INSERT` — or
  what is being checked is your idea of the schema against your idea of the query.
- **A guard written for the fields it was thinking of covers the ones it was not.** Twice this
  session, in two crates. `adopt` inferred currency from existence, which was right until runs
  came back; `absorb` refused a peer's whole record about a run this node holds, which is right
  about the state and the epoch and wrong about the two fields a holder does not own. Both read
  as one obvious rule, and both were a rule about *some* of what they were applied to. The
  question that finds these: not "is this rule true" but **"true of which fields, and who owns the
  rest"** — which is ADR-0005's table asked at a call site rather than at a struct.
- **Two functions answering one question will answer it differently.** The delivery pass and
  `offload sinks` both derived the fleet's routes from the view, with different filters, and the
  disagreement was invisible until it destroyed something: the pass called a route gone that the
  report was listing. Same shape as `sink_id_of` being one function because a convention with two
  copies is one copy that will be wrong. When a public function and a private one walk the same
  data, the fix is one walker with two thin callers.
- **"It is still here" is not "it is still current", and the difference only exists once things
  move.** `adopt`'s reasoning was sound when it was written and became false when runs started
  coming *back* to a node — nothing in it changed, and nothing failed. Worth generalising: a
  cache, a checkout, a transcript, an installed file, all of them answer "is this mine and is it
  the latest" with one existence check, and a system whose whole point is that work moves
  between machines has to answer the second half separately. What made it answerable here was
  that the domain already had a monotonic number nobody had thought to write down (turns), and
  that recording it needed no clock, no message and no agreement.
- **A guard is two-sided or it is a coin toss.** Reverting a fix to watch its test go red is the
  habit; the version of it that nearly got skipped here is the *other* direction. Adoption is a
  comparison, so there are two ways to get it wrong, and always-rebuilding loses the work done
  since the last checkpoint just as silently as always-adopting loses the work done elsewhere.
  Both cases need a test, and the second one is the case the feature exists for, which is
  exactly why it feels like it does not need checking.
- **The bug is often next to the property, not under it.** Two of this session's six were found
  while reading the code *around* the thing being asserted rather than by an assertion failing:
  the delivery starvation turned up in the query next to the one whose invariant had just come up
  clean, and the certificate signing-bytes hazard turned up because adding a field made nothing
  go red. Budget the reading, not only the writing.
- **A rule stated in prose and enforced at the honest caller is not enforced.** Twice in one
  session, both security-critical: `require_scoped` was the only thing standing between a
  repository's `.offload.toml` and a shell, and it judged the text of a pattern rather than the
  program it named; and `FleetState::invite` politely used `Terms::joining` while
  `MembershipCert::verify` never looked at what an approver had put in a certificate. Both read
  as correct at the call site somebody wrote. The question that finds them is not "is this
  right" but **"where is this checked when the caller is hostile"** — which for a credential is
  always the verifier, and for a config is always the thing that loads it.
- **A property that spends its budget on rejects is searching a fraction of what it claims to.**
  Four of them aborted with "too many global rejects" the first time they were run at 20,000
  cases and passed silently at the default 256 — each generated a shape and then threw most of it
  away with `prop_assume!`. Construct what the property needs instead: top the list up to two
  distinct holders, generate a holder that is not the local node, build the command word rather
  than filtering out spellings nobody writes. **Run every new property once at a high case count
  for exactly this reason** — the abort is the only signal that a passing property is barely
  searching.
- **A property that has to be weakened is telling you something; read it before weakening it.**
  The churn sim's "every settled node agrees" failed three times for three different reasons — an
  impatient fixpoint (the test's bug), a real bug (a holder believing its own orphan), and a
  genuine characteristic of the design (an arbiter's observation cannot be relayed). Only the
  third deserved the weakening, and the weakened form is *sharper* than the original: decisions
  agree unconditionally, observations agree while their author is there. Weakening first would
  have hidden the other two.

- **The most important guard in the design was on the wrong side of an `await`.** Found by
  auditing this list rather than by a failure, which is the only way it was ever going to be
  found: `drive` spawned the agent and *then* called `Run::started` — the fenced transition —
  and discarded its result with `if ….is_ok()`. So a node whose workspace preparation ran long
  (a cold clone is minutes; restoring one fetches blobs from peers) could have its run
  reassigned underneath it, finish preparing, spawn an agent, fail the epoch check, and keep
  driving that agent anyway. Two agents, one repo, both committing, and nothing in either log
  looking wrong. The gossip-driven kill would eventually notice, which makes it worse rather
  than better: the window is "until the next gossip round" precisely on a node whose gossip is
  the thing that failed. Now checked before the spawn, against the store rather than against
  what the task remembered, and a failure after the spawn stops the agent. The test arranges
  the state rather than racing for it, and was verified against the old code.
- **A warning in `CLAUDE.md` is not a fence.** "An MCP config without `--strict-mcp-config` is
  ambient authority" had been written down since phase 1, in a document read at the start of
  every session, describing a hole that was open the whole time. Nothing passed the flag. The
  measurement is what turned it from a note into a bug: a run spawned by the daemon on an
  ordinary laptop reported five MCP servers, four of them the owner's own and three connected.
  The lesson is not "read the warnings" — they were read, several times. It is that a bullet
  describing a failure mode reads identically whether it is a rule the code enforces or a rule
  somebody intends to enforce, and only running the thing tells them apart. Worth doing to the
  rest of that list.
- **A constant that should be a function of the mode is wrong in both directions at once.**
  `ASK_TOOLS` was `["Bash", "WebFetch"]`, which is right under `AcceptEdits` and silently wrong
  under `Ask` — and it had been reasoned about carefully, at length, in an ADR that arrived at
  the correct short list for the mode it was thinking about. The tell was the *refusal it forced*:
  ADR-0008's ban on `Ask` had survived two phases with an explanation that kept getting longer.
  A rule that needs a growing apology is usually a missing parameter.
- **Granting a tool is not granting permission to call it.** Projecting a `Role::Resource` into
  `--mcp-config` and stopping there gave the agent a tool it could see and was then denied, so
  `--use email` produced a visible, unusable mailbox and a run that reported a declined
  permission request. Found on the very first end-to-end run, and it would have survived any
  amount of unit testing of the projection, because the projection was correct. Two subsystems
  each behaving perfectly is exactly where this kind of gap lives.
- **The comment that asserts a safety property is the one to check.** `server::bind` said "a live
  daemon already holds the lock on the state dir" and used it to justify unlinking a socket
  file. Nothing held any lock. The justification had been sitting next to the code it justified
  for two phases, and the failure it enabled was silent: two daemons both start, one becomes
  unreachable while continuing to hold runs and renew leases. A comment explaining why something
  unsafe-looking is safe is a claim about *other* code, and nothing type-checks it.

- **A commitment that counts as a full slot blocks the commitment beside it.** ADR-0006 says a
  full node accepts anyway and starts when a slot frees; `start_held_runs` then asked how many
  runs this node *held*, which includes the very commitments waiting to start. Three runs on a
  one-slot node: the first runs, the other two are held, and when the first finishes each of
  the remaining two sees the other occupying the slot. Neither ever starts, the leases renew
  for ever, `ps` says `assigned`, and nothing in the log looks wrong. Capacity gates how many
  agents *run*; holding is the right count for whether to accept *more*. Two lessons: a
  commitment is a promise about a slot and not an occupant of one, and a demo with two of
  something cannot exercise the rule that only bites at three.
- **Deriving urgency from the submission time makes every run overdue, including yours.** It is
  the right default — it is what "as soon as you can" means, and it makes aging free — and it
  means slack cannot be handed to anything that spends patience without asking where the
  deadline came from. Caught while writing the hold-down test rather than by a failure, because
  the symptom would have been a fleet that migrated everything at the slightest blip and looked
  merely flaky.

- **Nothing renewed a lease, and a self-correcting loop was hiding it.** `Run::renew` existed
  and had no callers anywhere, so a run granted to a peer kept the arbiter's 60-second lease
  for its whole life; `supervise` orphans on `lease_expired`, so a minute after granting, the
  arbiter would call a perfectly healthy run `Orphaned` — and the holder's next gossip would
  take it straight back, because a holder's own claim outranks an arbiter's suspicion. Found
  by reading the code while building accepting-without-starting, not by anything failing: the
  flap costs nothing visible, and the hold-down never fires while the holder is `Alive`. After
  the fix, a peer-arbitrated run held for two and a half minutes produced zero orphan lines.
  Two lessons: a mechanism with no callers is not "unused" if something reads its output — it
  is wrong; and a loop that corrects itself is exactly how a bug survives every demo.

- **An owner that does not exist at the moment the fact is final is not an owner.**
  `RunProgress` was first arbitrated by deferring to the run's *holder*, which reads perfectly
  and fails at exactly one moment: a finished run has no holder, and a holder's last act is to
  write the summary of the worktree it leaves behind — which moves no counter, so it needs the
  tiebreak it can no longer win. The symptom was a peer showing `preparing` for a run that had
  completed minutes ago. The rule is forward-only now, ties on the author's own timestamp, and
  it is *simpler* than the one it replaced: a fact with a single author needs no test of who
  is speaking.
- **Three runs on a cold repo raced each other to clone it, and the loser poisoned the well.**
  `git clone` into the mirror's final path is not atomic, so submitting three runs for a repo
  the node has never seen fails two of them on `destination path already exists` — before
  either had done anything. The lasting damage is the leftover: a half-written directory has
  no `HEAD`, so `is_warm` says cold and every later run on that repo clones over it and fails
  the same way, while `can_obtain` — which asks whether the *path* exists — happily keeps
  bidding. The mirror is built under a scratch name and renamed in now, so it exists whole or
  not at all, and losing the race is not an error. Two lessons: a cache that can be half-built
  is a cache that will be, and the test for this has to start three runs at once — every
  sequential test passed throughout.
- **Twelve characters of a run id are still not an identity.** Two runs submitted in the same
  millisecond share all twelve, because that prefix is exactly UUIDv7's millisecond clock —
  `offload ps` printed two different runs under one id during the concurrent-submission test.
  `resolve` reports the ambiguity rather than picking one, which is the important half; the
  display is still capable of showing somebody two rows that look like the same run.
- **A battery floor that forgets to ask whether the device is plugged in refuses everything.**
  This machine's probe reports `battery 0%, charging`, and `StoreBlobs::accepts_push` applied
  the laptop policy's 20% floor without checking `on_mains()` — which
  `WorkPolicy::min_battery_percent` documents as ignored and `WorkPolicy::admits` honours. So
  every checkpoint push was declined, every checkpoint stayed `here only`, and ADR-0016's
  durability was simply not happening. Nothing failed: the only symptom was a `WARN` that
  reads like a fleet with nowhere to put a copy, and the bill comes due at the one moment the
  copy is needed. Two lessons, not one — duplicate a policy rule and the copies drift, and a
  degradation that logs at `WARN` and then carries on is the kind you find in a demo.
- **Failing over on a suspicion produces two arbiters.** `Suspect` is a guess the subject can
  refute, and while it is arguing it still believes it arbitrates its own runs. If the
  successor believes that too, both run a bid round and the same run is granted twice — the
  epochs fence one of the agents, but only after both have started. Arbitration therefore
  moves on `is_gone()`, not on "not currently available", which is the opposite of what every
  other placement question in the codebase wants.
- **"I have never met this node" is not "this node died".** `arbiter_for` used to fail over
  whenever the home node was not available *including when it was simply unknown*, which is
  every node's state in its first second of gossip. It now returns no arbiter: prefer a run
  stalled to a run arbitrated twice.
- **`publish_runs` replaces this node's whole contribution to the view.** Right for the tick
  that republishes the store, wrong for the single record an arbiter writes about somebody
  else's run — that call dropped every run this node was *running* out of its own view until
  the next tick, and `running_count` reads that view to answer whether we are at capacity.

- **The agent's transcript path is derived from the cwd.** A migrating run must have its
  transcript *rewritten* to the receiving node's worktree path — same session id, different
  location. `offload_agent::transcript::install` is that counterpart, and the resume path
  calls it only when no transcript is already there: one on disk is never staler than the
  one in the checkpoint.
- **A resumed agent numbers its turns from one**, because it is a new process. Unshifted,
  the checkpoint taken after a resume claims less work than the one it replaces, and `ps`
  shows a run going backwards. `accumulate_turns` fixes it in one place, on the way in.
- **A released checkpoint is terminal for the log stream but not for the run.** And the
  test is whether the *last* backlog event is terminal, not whether any is: a resumed run's
  history contains the checkpoint that released it, and `--follow` used to exit on it.
- **A worktree that survived beats one rebuilt from blobs — while the run never left.**
  Restore adopts in place, then falls back to the run branch in the mirror, and only unpacks
  the bundle if neither is there. Rebuilding unconditionally would replace live state with a
  staler copy; adopting unconditionally resumes a returning run from the leg before it left
  (session sixteen), so adoption compares the turn the checkout holds against the
  checkpoint's.
- **`prepare` reports the ref it checked out as the base commit**, which is wrong for a
  resumed run — the base is where the *run* started, and every capture bundles
  `base..HEAD` against it. Restore overwrites it with the checkpoint's value; without that,
  the next checkpoint silently bundles nothing.
- **Unix socket paths have a hard length limit** (`SUN_LEN`, ~108 bytes). A deep
  `OFFLOAD_STATE_DIR` makes `offloadd` fail to bind with an error that does not suggest the
  fix. Worth catching at startup with a better message, or defaulting the socket elsewhere.
- **Run ids are UUIDv7, so their leading bytes are a clock.** Every run submitted within
  ~65 seconds shares its first 8 hex characters. Branch names use the full id; displayed
  prefixes are 12 chars.
- **Don't `clone --mirror` a repo cache.** Its `+refs/*:refs/*` refspec means the next
  `fetch --prune` deletes every `offload/run-*` branch, destroying the agent's commits.
- **Serde's internally-tagged enums can't encode a newtype variant wrapping a sequence.**
  `Response::Runs(Vec<_>)` compiled and failed at runtime.
- **An allowlist entry can quietly be a shell.** `Bash(sh:*)` looks scoped and isn't; nor
  are `python`, `xargs`, `env`, `find`.
- **A signing format must be hand-rolled, not serialised.** A serde representation that
  gains a field, reorders a map or changes how it encodes an enum would silently invalidate
  every credential in the fleet. Fields are length-prefixed so `("ab","c")` cannot sign the
  same bytes as `("a","bc")`, and each credential type carries a domain-separation context so
  a signature cannot be replayed across types. Both have tests, because neither failure would
  be visible until it mattered.
- **"A holder's own claim beats a peer's suspicion" means *its own*.** `merge_run` ordered
  records by `(epoch, progress)`, so a third node relaying its remembered `Running` undid the
  arbiter's `Orphaned` every second — pinning a run to a machine that no longer existed.
  Merge now asks who is speaking: only the holder may say it is still there, only the arbiter
  may say it has gone.
- **Almost everything a run does happens inside `Running`.** A merge that requires the state
  *rank* to advance therefore drops every checkpoint, and the first ungraceful migration hands
  the next node a run with nothing to resume — three seconds after a checkpoint reached it.
  The record's owners are believed even when the state has not moved.
- **A newly enrolled node cannot host for fifteen minutes**, by design (ADR-0012's probation),
  which makes multi-node testing slower than expected. Plan for it: grant `host-runs` to a
  test node before you need it.
- **`find` is for reading a transcript, never for deciding whether one is here.** It falls
  back to scanning every project directory for the session id, which is right when the slug
  rule may have changed and catastrophic on a migration between two nodes sharing a home
  directory: the receiving node finds the *sending* node's copy, skips the install, and the
  agent — computing its own path from its own cwd — starts a fresh conversation while
  reporting that it resumed one. `expected_path` is the check that belongs there.
- **A terminal run has no holder**, so "gossip what I hold" goes quiet exactly when the news
  is worth spreading. The submitting node sat at `running` for ever. What travels is what is
  live plus what finished recently, from whoever knows it.
- **`recover()` belongs to this node's own runs.** It fails anything the store thinks is
  active, which was right when the store only held local work; with a mesh it also holds runs
  placed elsewhere, so a restart told the fleet that a node happily running an agent had lost
  the run.
- **A `str.replace` with no `assert` is a silent no-op.** Two tests described in a commit
  message were never inserted, because a `cargo fmt` run had reformatted the anchor they were
  keyed to — and the test count happened to match what it had been before. Every scripted edit
  asserts its anchor now.
- **"Accepted" is not "has it".** A push that returns once the peer agreed to receive reports
  a checkpoint durable while it is still in flight. The receiver confirms after storing, and
  a test caught the difference the first time it ran.
- **A bundle is `base..HEAD`, so it needs the base commit to apply.** That is why a
  local-path repo is an eligibility fact rather than a scoring one: a cold node has neither
  the origin nor the history, and no amount of blob fetching fixes it.
- **A DNS label is 63 bytes, and a node id is 64 characters of hex.** The mDNS advertisement
  published happily and nothing on the LAN could resolve it. The id travels as a TXT property
  now, with a test that every label is legal.
- **A probe's timeout has to cover connecting, not just answering.** Dialling a machine that
  has vanished blocks on the kernel's own timeout, which overruns the whole SWIM period — and
  because suspicions expire at the *top* of a period, they then never expire at all. The
  symptom was a node stuck on `suspect` for ever while `dead` never arrived.
- **A node ages its own entry unless something says otherwise.** `offload nodes` reported the
  local machine as last heard from thirty seconds ago, because nothing was touching our own
  `last_heard`.
- **Dropping a quinn endpoint discards what it has not sent.** A refusal written into a
  connection that is then dropped never arrives, and the peer sees a bare connection error —
  so it retries for ever. `QuicTransport::accept` waits (bounded) for the peer to close after
  a refusal, and the tests hold their endpoints open with a oneshot rather than letting the
  task end.
- **A memory-hard KDF in a debug build is hard on you and nobody else.** Unoptimised,
  argon2id at 128 MiB × 3 took ten seconds, which made `offload init` look broken and the
  test suite unusable. `[profile.dev.package.argon2]` and `blake2` at `opt-level = 3` bring
  it to 280 ms while leaving the rest of the tree debuggable.
- **Nothing warns you when you re-found every fleet in the world.** Salt, argon2 parameters,
  the wordlist and the passphrase normalisation all feed the derived key, so tuning any of
  them invalidates every certificate that exists — and nothing fails at the point of the
  edit. The pinned known-answer test in `fleet::passphrase` exists solely to be the thing
  that breaks.
- **A derived `Debug` on anything holding a key leaks it.** `NodeIdentity` writes its own and
  prints `<redacted>`; that is the difference between a secret and a secret that has been in
  a log line.
- **There is no `--max-turns`** in claude 2.1.220, so `RunSpec::max_turns` is unenforced —
  the supervisor has to count boundaries itself. Still open, and the turn-offset work has
  now put a cumulative count in exactly the right place to enforce it.

## Open questions, in the order they'll bite

1. ~~**Permission prompts have nowhere to go**~~ — answered (ADR-0017, built, and closed out in
   session thirteen). A run with `--ask` stops, the question leaves through the delivery plane,
   and `offload approve` unblocks it — for edits as well as commands now, because the hook's tool
   list follows the permission mode. `PermissionMode::Ask` is accepted for a run that can answer
   for it and still refused for one that cannot. What the widening cost is a **budget**
   (`--ask=N`): the open sub-question was "how many questions will a person answer for one run",
   and 20 is the answer with running-out defined as the pre-existing behaviour rather than a
   denial.
2. ~~Checkpoint cadence.~~ Decided: one per turn boundary, with `every_turns` to override.
   The argument is written out on `CheckpointConfig`.
3. ~~**Untracked-file policy in the wild.**~~ Answered in session sixteen, by measuring the
   repos on one laptop rather than waiting for the gap to arrive: `build/` holds hand-written
   files in an ingress chart, WordPress tracks 132 files under `wp-includes/js/dist/`, and two
   more commit `vendor/` and `node_modules/`. A name is a guess about somebody else's
   repository, so it now loses to what that repository tracks. The residual is the directory the
   agent creates from scratch whose name is on the list — excluded, because there is nothing in
   the repo to ask, and reported.
4. ~~**`Portability::NodeLocal` isn't wired into eligibility.**~~ Closed early in phase 4:
   `LocalFacts::repo_available` refuses the bid, answered by `WorkspaceManager::can_obtain`, so
   a run whose repo is a path on the closed laptop is refused at placement rather than at 03:00.
5. ~~**Per-account rate limits.**~~ Closed in session twelve. The blocker was the fingerprint,
   which derived from `$USER` and `$HOME` and could never match across machines; it hashes the
   account uuid the agent records in its own settings now, with a versioned salt and a pinned
   known-answer test. `WorkPolicy::max_concurrent_account` (wire v13) is the ceiling it made
   possible, folded to the lowest any node on the account claims.

   **And its other half, which nobody had noticed was missing, is closed in session
   twenty-seven** (ADR-0029): a *concurrency* ceiling is not a *usage* limit, and the agent has
   been reporting the second one every turn since phase 1 into a field nothing read.
   `judge_rate_limit` used it for one run's deadline and dropped it, so the next run was started
   into the same wall. A node now holds new runs until the limit lifts and bids
   `Availability::NotBefore`. What stays open is whether that position should *travel* — argued in
   the ADR rather than deferred, because an account's position is a moving value and ADR-0013's
   rule about attendance applies to it.
6. ~~**A partition that persists still runs two agents.**~~ Answered as far as it can be
   (ADR-0018, session nineteen). Fencing closed the safety half in session fifteen: once the two
   records meet, exactly one leg survives and the loser's effects are refused. Quorum was the
   candidate for closing the rest, and the simulation says it would not — there are two paths to
   two live legs and quorum removes only the two-arbiter one, while the other (a grant whose
   acknowledgement was lost, ADR-0006's ordinary case) needs no partition and is beyond any
   consensus protocol's reach, because the decision is replicated and the side effect is on one
   machine. So the residual risk stands and is written down rather than mitigated: a persistent
   partition runs two agents, the losing leg's work is lost including the uncommitted part, and
   the fleet now *reports that correctly* instead of freezing the run at the loser's position.
   The sub-question — whether the losing leg should be *chosen* rather than settled by node id,
   via the granting arbiter on the `Lease` — is still deliberately untaken, and the reason is
   unchanged: both grants are safe, the choice is arbitrary but agreed, and making the numbers
   follow that choice did not make the choice itself matter more.
7. **Bid weights: cluster config or node preference?** Two nodes running different `BidWeights`
   bid in different currencies, so scores stop being comparable and the winner is decided by
   whoever was configured most generously. Leaning cluster-wide and gossiped, with a version.
   Checked in session twenty: it is **not reachable today**. `BidWeights::default()` is the only
   constructor in the workspace — no config field, no flag — so two nodes cannot disagree about
   the currency, and the question becomes answerable the day somebody adds a way to set them.
   That is also the day to answer it, rather than now: the answer is a gossiped struct with a
   version, and building one for a knob nobody can turn is machinery ahead of its use. What the
   check *did* find was the opposite problem next door — `bid_delay` and the two weights that fed
   it, describing a superseded protocol in the present tense (see session twenty).
8. ~~**Non-agent workloads.**~~ Answered in session twenty by **ADR-0019**, accepted as intent
   and unbuilt: read it as a decision about shape rather than a description of code, the way
   ADR-0010, ADR-0011 and ADR-0013 read when they were new. The shape: non-agent work is **a program the node's owner nominated**,
   addressed by service exactly as a sink and a resource are, which is that rule's third
   application and makes the sketch's "outbound HTTP needs an allowlist of its own" question
   disappear rather than need answering — nominating the program *is* the grant, and the submitter
   never says where to connect. Recurring work is a **schedule that fires idempotent runs**, owned
   by its home node with arbitration's own successor rule, and an occurrence's `RunId` is
   **derived from `(schedule, tick)`** so two nodes firing one tick produce one record that
   `merge_run` settles rather than two runs racing. That works here and could never work for
   placement, for the reason worth remembering: a duplicate *record* merges and a duplicate
   *agent* does not.

   Two of the sketch's four gaps had already closed by the time it was read: `Demand` exists and
   capacity *and* pressure honour it. Two remain — `RunSpec` assuming a prompt and a workspace,
   and the owner's gates (`accept`, `min_battery_percent`, `allow_metered`) not consulting demand
   the way `room_for` and `needs_idle_percent` do. And one the sketch did not name: the
   **supervisor** is concretely `ClaudeCode`, so "the run machinery is agent-agnostic" is true of
   `offload-core` and false of `offload-node`, which is the difference between a small change and
   a new seam.

   The ADR names its own smaller half, and it is the thing to build if only one gets built:
   `Role::Trigger` — declared by ADR-0011, used by nothing — is "something arrives, work starts",
   changes no existing type, and needs none of the `RunSpec` split. **Built in session
   twenty-four** (ADR-0020), which leaves this question as the `Work` enum and the agent-agnostic
   supervisor. What building it added to the answer: the *third* application of the nominated-
   program rule works exactly as predicted, and the gap the sketch never named is that a rule
   fires with **nobody watching**, so every mechanism whose justification quietly assumes an
   operator has to be re-read on that path — `Supervisor::cleanup` was the first and will not be
   the last.

9. ~~**What removes a peer's copy of a finished run?**~~ **Answered in session twenty-six by
   ADR-0025, and the answer is "nothing should."** The premise below — that a peer's stub is
   accumulation — was a guess, and measuring it inverted the conclusion: a 0-event, 0-blob,
   0-outbox-row record is the **index** `offload logs` resolves an id in before forwarding to the
   leg that ran the work, and it answers *byte-identically* to that machine. Deleting it would not
   narrow what a device can say; it would end it, for that run, on that device. What was missing
   was the *number*, which `offload status` now prints. The entry is kept rather than deleted
   because the three shapes it weighs are still the right three, and the middle one is precisely
   what got rejected.

   The original entry, as written in session twenty-five: the one
   thing that session measured and declined to fix. Nothing has ever deleted a run record on a
   node that neither hosted nor submitted the run — which was invisible while every run had a
   person behind it, and is not any more: on two daemons, alpha's three-second rule settles at
   **101** records while **beta grows 81 → 181 in five minutes**, on a machine hosting nothing.
   ADR-0021's prune is node-local because its tag is, so the accumulation moves rather than stops.

   The reason it is a question rather than a patch: the fix is a retention policy for *every*
   finished run and not for occurrences, and deleting a peer's copy changes what `offload ps
   --all` and `offload logs` can answer from a machine that was not the one running the work —
   which is precisely the bug session twenty-three had to fix from the other side (a run finished
   on the desktop was invisible from the laptop it was submitted on). Three shapes worth weighing:
   gossip the occurrence tag and let any node prune (a gossiped field needs an owner, and this one
   has an obvious one); a general "a node forgets a finished run it neither hosted nor submitted
   once the fleet has stopped gossiping it" (simple, and it silently narrows two commands); or
   keep them and say the size out loud, which is what today does badly.

## A sketch, superseded by ADR-0019 — kept because the ADR argues against parts of it

Read [ADR-0019](adr/0019-non-agent-work.md) first; this is what it was written from, and two of
the four gaps below had closed before anybody re-read it. Left in place rather than deleted
because the ADR's reasoning is partly a disagreement with it — the outbound-HTTP allowlist it
predicts is a mechanism that turns out not to be needed, and the long-lived sleeping run it offers
as one of two options is refused with a reason.

## The sketch itself: workloads that are not agent runs

The scenario: a phone joins the fleet, has no agent, but can watch an API and report when a
response changes. It should take that work always, and heavier work only while charging.

What already fits:

- **Capabilities vs policy.** The split is exactly right — the phone *lacks* the capability
  `agent=claude-code` and that is a fact, while "only when charging" is its owner's policy.
- **`Restartability::Idempotent`** describes a watcher perfectly: no transcript, no
  workspace, re-runnable from its spec. Migration becomes a no-op — reschedule it.
- **Bidding, leases, epochs, the state machine** never mention agents.

What does not fit yet:

- `RunSpec` requires a prompt and a `WorkspaceSpec`. A watcher has neither.
- `WorkPolicy::accept` is one gate for the device, so "charging" is all-or-nothing. The
  missing axis is *what a run demands* — light (a scheduled HTTP check) vs heavy (an agent
  run) — declared by the run and admitted per class by the policy.
- Recurring work has no home. Either a long-lived run that sleeps between polls, or a
  schedule that fires short `Idempotent` runs. The second reuses everything and lets a
  sleeping phone simply not bid.
- Outbound HTTP from a fleet node is a real capability grant, and needs an allowlist story
  of its own — the tool-allowlist reasoning applies almost unchanged.

**ADR-0011** then reshapes capabilities themselves: instances rather than a map keyed by
kind (two mailboxes are two entries), each with a role — `Sink`, `Trigger`, `Resource` — an
identity that is comparable across nodes, and a verified auth flag. Only typed fields are
matchable; the agent-facing description of a capability is *not* a field on it, but a
per-run projection into the agent's own tool protocol (`--mcp-config` plus
`--strict-mcp-config`). **This is a breaking change to `Capabilities`, which is why it is
written down now** — before phase 3 gossips the struct and it becomes a protocol version.

The reporting half is no longer a sketch: **ADR-0010** settles it. Work and reporting are
two planes, and a delivery route (terminal, push, email, chat, webhook) is a *capability* —
probed, never over-claimed, matched by `Constraint`, gated by `WorkPolicy`, with credentials
that do not migrate. That means one scheduler, not two, and it means a device that cannot
host anything is still worth enrolling if it can reach you.
