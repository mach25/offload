# The Android app: building it and putting it on your phone

The app is not on Google Play. You build it from this repository and install it yourself, once per
device, and again when you want a newer build. This takes about half an hour the first time,
mostly installing the Android tools.

What you get is described in the README's *The app* section. In short: the phone hosts a node of
its own and joins your fleet like any other device; from it you ask agents for things, run the
programs your devices offer, answer an agent's questions, and read what each run did.

## What you need

- **A Linux machine to build on.** The build script uses the NDK's `linux-x86_64` toolchain. (macOS
  would need the script's paths changed; nobody has done it.)
- **Rust**, through [rustup](https://rustup.rs). The script adds the two Android targets itself.
- **Android Studio**, for the parts of the Android SDK the build uses:
  - the SDK itself, at `~/Android/Sdk` (Android Studio's default), or wherever `ANDROID_HOME` says;
  - **NDK 27.0.12077973** (*SDK Manager → SDK Tools → NDK (Side by side)*), or another version
    with `ANDROID_NDK_HOME` pointing at it;
  - a **JDK 17** for Gradle. The script finds the JetBrains runtime Android Studio installs under
    `~/.jdks/jbr-17*`; otherwise set `JAVA_HOME_FOR_GRADLE` to any JDK 17.
- **A phone on Android 10 or newer.**
- **At least one other device running `offloadd` in a fleet**, to invite the phone. See the README's
  *Try it*: `offload init` founds a fleet on a laptop or desktop in one command.

Claude Code does not run on Android, so a phone never runs agent runs itself. It asks for them,
and a laptop or desktop in the fleet does the work. A phone can run *programs* (tasks) if its
owner grants it `host-runs`, which it does not have by default.

## Build it

From the repository root:

```bash
scripts/build-android-product.sh
```

It builds the daemon and the app's client library for both phone (`arm64-v8a`) and emulator
(`x86_64`) processors, generates the Kotlin bindings, and assembles a debug APK:

```
android-app/app/build/outputs/apk/debug/app-debug.apk
```

The first build compiles everything and takes a while; later ones take a minute or two.

## Put it on the phone

Either way works. The first is quicker if you will update often.

**With a cable or wireless debugging (adb).** Turn on *Developer options* (tap *Build number* in
*Settings → About phone* seven times), then *USB debugging*, or *Wireless debugging* and pair it
with `adb pair`. Then:

```bash
adb install -r android-app/app/build/outputs/apk/debug/app-debug.apk
```

`-r` replaces an installed copy and keeps its data, which is what you want for an update.
Wireless debugging holds a Wi-Fi lock while it is on and costs battery, so turn it off when you
are done.

**Without adb.** Copy the APK to the phone (a USB cable, a cloud drive, a message to yourself),
open it from the phone's file manager, and allow that app to *install unknown apps* when Android
asks. Android will warn that the app is not from the Play Store, which is true.

The APK is signed with your machine's debug key. An update must be signed by the same key, so
build updates on the same machine. Otherwise Android refuses the update, and uninstalling the old
copy first deletes the phone's node identity: it would have to be invited into the fleet again.

## Join your fleet

1. **Open the app.** Allow notifications when it asks: that is how an agent's question or a
   finished run reaches you.
2. **Copy the phone's node id**: *Settings* shows it, with a *Copy* button.
3. **Invite it from a device already in the fleet** (a laptop or desktop), naming the phone:

   ```bash
   offload invite <the phone's node id> --name phone
   ```

   It prints an `offload://join?token=…` link. The token names that one phone's key, so it is
   safe to send over any channel: nobody else can use it.
4. **Open the link on the phone**, for example by sending it to yourself in a message and tapping
   it, or paste it into *Settings → Join a fleet*. The app shows which fleet it is and what the
   phone would be allowed to do, and joins when you tap *Join*.
5. **Close the app and open it again.** A node that joined after it started only meets the fleet on
   its next start. Leave it for a couple of minutes, or force-stop it from Android's app settings,
   then open it. *Can take agent runs: …* above the text box then lists your fleet's computers.

The phone finds the rest of the fleet on the same Wi-Fi by itself (mDNS). Away from home it uses
the addresses the other devices shared while they were in touch. It reaches a computer that has a
routable address, such as IPv6 at home, but there is no relay yet for two devices behind NAT.

## Everyday use

- **Ask the agent**: type what you want, choose where it works (*Empty folder* or a repository)
  and, if you like, a model, then send. The line above the box says which devices can take it.
- **Run a program**: pick one of the programs your devices offer. Programs are set up by each
  device's owner in its `node.toml` (`[[tasks]]`), not in the app.
- **Tap a run** for its details: which device ran it, the model it used, what it cost, and its
  answer, with *Files* to browse what it wrote and a box to continue from it.
- **Questions**: when you ask for *Ask me first*, an agent that wants permission for something
  waits for you there.
- **In the background**: the app stops its node about a minute and a half after you leave it,
  unless it is running something. News that arrives meanwhile waits and shows when you open the
  app. *Settings → In the background → Keep running when the app is closed* keeps it in the fleet,
  at some cost in battery.

## Updating

Pull the repository, run `scripts/build-android-product.sh` again, and install with
`adb install -r` or by opening the new APK. The phone keeps its identity and its place in the
fleet. After a change to the wire protocol (a version bump, noted in `docs/HANDOFF.md`), update
every device in the fleet together: devices on different versions refuse to talk.
