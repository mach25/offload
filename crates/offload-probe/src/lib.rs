//! Local capability detection.
//!
//! Everything here is a best-effort observation of the machine we are running on. Where a
//! fact cannot be determined, the honest answer is the conservative one — an unknown power
//! source is not "on mains", and an agent whose auth we cannot verify is not "authenticated".
//! Over-claiming here produces runs that get placed and then fail, which is worse than a
//! run that stays pending with a legible reason.
//!
//! Kept out of `offload-core` on purpose: this crate shells out, reads `/sys`, and touches
//! the filesystem. See ADR-0001.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use offload_core::capability::{
    AgentKind, Arch, Capabilities, DeviceClass, Metered, Os, PowerSource, Stability,
};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

mod agents;
mod load;
pub mod network;
mod power;

pub use agents::{detect_agents, probe_agent, DEFAULT_CLAUDE_BINARY};
pub use load::cpu_load_percent;

/// Toolchain probes: `(capability name, binary, version args)`.
///
/// Deliberately a fixed list rather than a scan of `$PATH` — a constraint can only ask for
/// what it can name, so the useful set is the one runs actually reference.
const TOOLCHAINS: &[(&str, &str, &[&str])] = &[
    ("rust", "rustc", &["--version"]),
    ("cargo", "cargo", &["--version"]),
    ("node", "node", &["--version"]),
    ("python", "python3", &["--version"]),
    ("go", "go", &["version"]),
    ("java", "java", &["-version"]),
    ("git", "git", &["--version"]),
    ("docker", "docker", &["--version"]),
    ("gh", "gh", &["--version"]),
];

/// Probe this device, asking `claude` on `PATH` about the agent.
///
/// The right answer for `offload probe` on a machine with an ordinary install, and the wrong one
/// for anything that knows better: a daemon spawns the binary its owner configured. Use
/// [`probe_with_agent`] wherever the caller can say which program it will run.
#[must_use]
pub fn probe() -> Capabilities {
    probe_with_agent(Path::new(DEFAULT_CLAUDE_BINARY), None)
}

/// …and the same probe, told which agent binary the caller will actually spawn, and where that
/// agent keeps its state.
///
/// The capability this returns is what decides whether the node bids at all, so it has to be
/// about the program that would then run — and about the **account** it would run as, which is
/// what the state directory selects (ADR-0028). `agent_state` of `None` means ask the
/// environment, which is what every caller did before an owner could nominate one. Nothing else
/// about a device changes with either argument.
#[must_use]
pub fn probe_with_agent(agent: &Path, agent_state: Option<&Path>) -> Capabilities {
    probe_with(agent, agent_state, None)
}

/// …and the same probe again, told what the owner has *nominated* about this device.
///
/// `metered` of `None` means "ask the platform", which is what `network::detect` does. `Some`
/// ends the question: whoever tethered the laptop knows, and NetworkManager cannot see through a
/// tether (ADR-0045 §2). The standing pattern for a fact only the owner holds — the same reason
/// `agent.max_concurrent`, the account, sinks, resources and `allowed_agents` are nominated
/// rather than detected.
#[must_use]
pub fn probe_with(
    agent: &Path,
    agent_state: Option<&Path>,
    metered: Option<Metered>,
) -> Capabilities {
    let os = detect_os();
    let arch = detect_arch();
    // **Before the class, because the class depends on it.** Where there is no DMI to read, what
    // powers the machine is the only thing left that separates a portable from a desktop — and
    // `power::detect` is the one function that can answer on more than one platform. It used to
    // be probed after, so the fallback reached for `power::has_battery`, which reads
    // `/sys/class/power_supply` and therefore answered `false` on *every* Mac.
    let power = power::detect();
    let device_class = detect_device_class(&os, &power);

    let mut caps = Capabilities::empty(os, arch, device_class);

    caps.cpu_cores = std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .unwrap_or(1);

    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    caps.memory_mb = sys.total_memory() / (1024 * 1024);
    // The working directory, because that is the only path this function is told about. The
    // daemon knows better — its worktrees are under the state dir — and asks `free_disk_under`.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    caps.disk_free_mb = free_disk_under(&cwd);

    caps.power = power;
    // Asked rather than assumed (ADR-0045). This was `false`, unconditionally, under a comment
    // saying it was an assumption — and four readers took it as a fact, so the fleet could not
    // produce `Refusal::MeteredNetwork` and `--require unmetered` matched everything. What
    // `network::detect` cannot establish comes back `Unknown`, which is now a thing the type can
    // hold; the owner's nomination overrides it in `probe_with`, because a tether is the case
    // nothing on the machine can see.
    caps.metered_network = network::detect();

    for capability in agents::detect_agents(agent, agent_state) {
        caps.add(capability);
    }
    caps.toolchains = detect_toolchains();
    caps.tags = derive_tags(&caps);

    // The owner's word beats the platform's, and beats its silence. Applied after the probe
    // rather than instead of it so that `offload probe` on a nominating node still shows what the
    // platform would have said in the line below.
    if let Some(said) = metered {
        caps.metered_network = said;
    }

    // Refine the class-based guess where we can actually tell.
    caps.stability = detect_stability(device_class, &caps.power);

    caps
}

fn detect_os() -> Os {
    match std::env::consts::OS {
        "linux" => {
            // Android reports as "linux"; the property store is the giveaway.
            if Path::new("/system/build.prop").exists()
                || std::env::var_os("ANDROID_ROOT").is_some()
            {
                Os::Android
            } else {
                Os::Linux
            }
        }
        "macos" => Os::MacOs,
        "windows" => Os::Windows,
        "android" => Os::Android,
        "ios" => Os::Ios,
        other => Os::Other(other.to_string()),
    }
}

fn detect_arch() -> Arch {
    match std::env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => Arch::Other(other.to_string()),
    }
}

fn detect_device_class(os: &Os, power: &PowerSource) -> DeviceClass {
    match os {
        Os::Android | Os::Ios => return DeviceClass::Phone,
        _ => {}
    }

    if let Some(virt) = detect_virtualisation() {
        if virt {
            return DeviceClass::Vm;
        }
    }

    // DMI chassis type is the most reliable signal on real hardware.
    // 3=desktop, 4=low profile desktop, 6=mini tower, 7=tower, 8..=11 portable/laptop/
    // notebook/sub-notebook, 13=all-in-one, 17=main server chassis, 30=tablet, 31=convertible.
    if let Ok(chassis) = std::fs::read_to_string("/sys/class/dmi/id/chassis_type") {
        if let Ok(code) = chassis.trim().parse::<u32>() {
            return match code {
                8..=11 | 14 | 31 => DeviceClass::Laptop,
                30 | 32 => DeviceClass::Tablet,
                17 | 23 | 28 => DeviceClass::Server,
                3..=7 | 13 | 15 | 16 => DeviceClass::Desktop,
                _ => DeviceClass::Unknown,
            };
        }
    }

    class_from_power(os, power)
}

/// The class where there is no DMI to read — containers, macOS, BSD — decided by what powers
/// the machine.
///
/// Pure, and separate from [`detect_device_class`] for that reason: the DMI read is a file this
/// cannot be given, and every interesting case here is a platform this machine is not.
///
/// This used to ask `power::has_battery`, which reads `/sys/class/power_supply` — so on macOS it
/// answered `false` unconditionally and **every** Mac came out `Unknown`, a MacBook included.
/// The comment above it named macOS explicitly as a case it handled. `PowerSource` is the signal
/// that travels: `settle`'s own arms establish that `Ac` means *mains present and no battery*,
/// and `detect_macos` gets there through `pmset`.
///
/// **`Desktop` is claimed only on macOS**, and the asymmetry is the point rather than an
/// oversight. `Ac` there is a positive answer from `pmset` about a lineup where no battery means
/// a Mini, a Studio, a Pro or an iMac — all desktops. On Linux, reaching this function at all
/// means there was no DMI, which is a container or an odd board rather than a desktop, and
/// `Stability::Stable` is exactly the claim this crate must not make on a guess: it moves bid
/// scores, and a node that over-claims wins bids and then fails the runs it took.
fn class_from_power(os: &Os, power: &PowerSource) -> DeviceClass {
    match power {
        // Something this machine runs on that can run out. True of a MacBook via `pmset`, and
        // of anything on Linux whose battery is not a peripheral.
        PowerSource::Battery { .. } => DeviceClass::Laptop,
        PowerSource::Ac if matches!(os, Os::MacOs) => DeviceClass::Desktop,
        // Includes `PowerSource::Unknown`, which is a battery whose level would not read — the
        // one case where knowing there *is* one is not enough to say the machine is portable,
        // and where the safe class is the one that claims least.
        _ => DeviceClass::Unknown,
    }
}

fn detect_virtualisation() -> Option<bool> {
    // `systemd-detect-virt` exits 0 when virtualised, 1 when not. Absent on plenty of
    // systems, hence the Option — "don't know" is not "bare metal".
    let out = Command::new("systemd-detect-virt")
        .arg("--quiet")
        .output()
        .ok()?;
    Some(out.status.success())
}

/// Refine the class default with what we can observe.
///
/// Only ever revises *downward*. Claiming a machine is more stable than its class suggests
/// needs evidence from observed uptime, which is the cluster's job, not the probe's.
fn detect_stability(class: DeviceClass, power: &PowerSource) -> Stability {
    let base = class.default_stability();
    match power {
        // A "desktop" running on battery is a laptop wearing a disguise.
        PowerSource::Battery { .. } if base == Stability::Stable => Stability::Transient,
        _ => base,
    }
}

fn detect_toolchains() -> std::collections::BTreeMap<String, String> {
    let mut found = std::collections::BTreeMap::new();
    for (name, bin, args) in TOOLCHAINS {
        if let Some(version) = probe_version(bin, args) {
            found.insert((*name).to_string(), version);
        }
    }
    found
}

/// Run `bin args` and pull the first version-looking token out of the output.
pub(crate) fn probe_version(bin: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    // `java -version` writes to stderr; several others write to stdout. Try both.
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).to_string()
    } else {
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    extract_version(&text)
}

/// First whitespace-separated token that starts with a digit (optionally after a `v`) and
/// contains a dot. Handles `rustc 1.82.0 (f6e511eec 2024-10-15)`, `v22.3.0`,
/// `git version 2.45.2`, `go version go1.22.5 linux/amd64`.
pub(crate) fn extract_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|tok| tok.trim_start_matches(['v', 'V']).trim_matches('"'))
        .map(|tok| tok.strip_prefix("go").unwrap_or(tok))
        .find(|tok| tok.contains('.') && tok.starts_with(|c: char| c.is_ascii_digit()))
        .map(|tok| {
            tok.trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
}

/// Free disk, rounded **down** to a whole GiB and still counted in MB.
///
/// To the megabyte, this number moves on every probe of a machine that writes anything — a log
/// line, a build — and any change to `Capabilities` bumps the node's incarnation and re-gossips
/// all of it. Measured in session ninety-two: `what this device is has changed
/// changed=disk_free_mb` every thirty seconds on an idle laptop, 55 incarnations in forty minutes.
/// Down rather than to the nearest, because the only reader is `MinDiskFreeMb`, and a node that
/// claims space it lacks takes a run it then fails; under-claiming by less than a GiB costs at
/// most a bid.
fn whole_gib_mb(mb: u64) -> u64 {
    mb - mb % 1024
}

/// Free space, in whole GiB counted in MB, on the filesystem holding `path` — or on the largest
/// one when no mount point contains it.
///
/// Ask it about the directory runs will be written under. `probe` used to ask about the process's
/// working directory, beside a comment saying worktrees live under the state dir, so a daemon
/// started from `/` by a service manager advertised the root filesystem's space for work that
/// lands on `/home`: the "ask about the thing that will actually be used" rule the agent binary
/// already follows. Measured: a state dir on a 210 MB `/boot` advertised 266 240 MB, the root
/// filesystem's. One blind spot is left and is sysinfo's: it lists no tmpfs, so a state dir on
/// one is answered with whichever real mount contains the path, usually `/`.
#[must_use]
pub fn free_disk_under(path: &Path) -> u64 {
    whole_gib_mb(free_disk_mb(path))
}

fn free_disk_mb(path: &Path) -> u64 {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    // A relative or symlinked path would match no mount point and fall through to the largest
    // disk, which is a guess; resolved first where it can be.
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let best_match = disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len());

    match best_match {
        Some(disk) => disk.available_space() / (1024 * 1024),
        None => {
            disks
                .list()
                .iter()
                .map(|d| d.available_space())
                .max()
                .unwrap_or(0)
                / (1024 * 1024)
        }
    }
}

fn derive_tags(caps: &Capabilities) -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    if caps.toolchains.contains_key("docker") {
        tags.insert("docker".to_string());
    }
    if caps.toolchains.contains_key("gh") {
        tags.insert("gh".to_string());
    }
    if Path::new("/proc/driver/nvidia/version").exists() {
        tags.insert("gpu:cuda".to_string());
    }
    if matches!(caps.os, Os::MacOs) && matches!(caps.arch, Arch::Aarch64) {
        tags.insert("gpu:metal".to_string());
    }
    tags
}

/// How this device is powered, in the words the *decision* uses.
///
/// It printed `PowerSource`'s `Debug` — `Battery { percent: 96, charging: true }` — in a report
/// whose every other line is prose, and in two places, since the daemon logs this at startup too.
/// Worse than untidy: it makes the reader do the translation that `WorkPolicy` does for them.
/// Charging **is** on mains as far as `on_mains` and every battery floor are concerned, so a line
/// that says `charging: true` beside a percentage invites somebody to think the floor applies when
/// it does not — the same gap between what a report says and what a gate reads that `offload
/// policy` had this morning.
///
/// `Unknown` says so and is not folded into either. A device whose battery would not parse is not
/// a device on mains: `Ac` claims there is nothing to run out of, which is the over-claim this
/// crate exists to avoid.
///
/// Not a `Display` on `PowerSource`. `Service`'s exists because something parses it back, and a
/// type with one direction only eventually grows a hand-written parser somewhere else; nobody
/// parses a power source, so the prose belongs where the audience is.
#[must_use]
pub fn power_line(power: &offload_core::PowerSource) -> String {
    match power {
        offload_core::PowerSource::Ac => "on mains".to_string(),
        offload_core::PowerSource::Battery {
            percent,
            charging: true,
        } => format!("on mains, battery {percent}%"),
        offload_core::PowerSource::Battery {
            percent,
            charging: false,
        } => format!("on battery, {percent}%"),
        offload_core::PowerSource::Unknown => "unknown — treated as not on mains".to_string(),
    }
}

/// Summary for `offload probe`, kept out of the CLI so both the CLI and the daemon's
/// startup log render capabilities the same way.
#[must_use]
pub fn summarise(caps: &Capabilities) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "device      {:?} / {:?} / {:?}  ({:?})\n",
        caps.device_class, caps.os, caps.arch, caps.stability
    ));
    out.push_str(&format!(
        "resources   {} cores, {} MB RAM, {} MB free disk\n",
        caps.cpu_cores, caps.memory_mb, caps.disk_free_mb
    ));
    out.push_str(&format!("power       {}\n", power_line(&caps.power)));
    // Its own line, and never blank. This printed nothing at all when the field was `false`, so
    // silence meant both "this link is free" and "nobody asked" — and it was always the second
    // (ADR-0045 §5). `Unknown` says where the answer would have come from, because the next
    // question is always "why does it not know".
    out.push_str(&format!(
        "network     {}\n",
        match caps.metered_network {
            Metered::Yes => "metered — bytes cost money here".to_string(),
            Metered::No => "unmetered".to_string(),
            // Not "no answer from NetworkManager": on this laptop NM *did* answer, with
            // `no (guessed)`, which is a guess it makes about every wifi and ethernet link and
            // therefore about a tethered one. Saying NM was silent would be this line making
            // the same kind of claim the field itself stopped making.
            Metered::Unknown => "metered unknown — nothing here can tell a tether from \
                                 plain wifi; say `metered = \"yes\"` in the config if this \
                                 link costs money"
                .to_string(),
        }
    ));

    let capabilities: Vec<&offload_core::Capability> = caps.all().collect();
    if capabilities.is_empty() {
        out.push_str("services    none detected\n");
    } else {
        for capability in capabilities {
            let detail = match capability.details.agent() {
                Some(agent) => format!(", max {} concurrent", agent.max_concurrent),
                None => String::new(),
            };
            out.push_str(&format!(
                "service     {} — {}{}{}\n",
                capability.id,
                if capability.authenticated {
                    "authenticated"
                } else {
                    "NOT authenticated"
                },
                detail,
                match &capability.identity {
                    Some(account) => format!(", as {}", account.0),
                    None => String::new(),
                }
            ));
            // …and *why*, for anything this device says it cannot use. A `NOT authenticated`
            // with no clause beside it is the shape CLAUDE.md keeps finding: found by walking
            // this, where `agent.account` naming the wrong login produced exactly that line and
            // the sentence explaining it — which the guard sets — was printed nowhere at all.
            // Only for the unusable ones: an authenticated route's description is its owner's
            // label and belongs on the line above.
            if !capability.authenticated && !capability.description.is_empty() {
                out.push_str(&format!("            └─ {}\n", capability.description));
            }
        }
    }

    let tools: Vec<String> = caps
        .toolchains
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
    out.push_str(&format!(
        "toolchains  {}\n",
        if tools.is_empty() {
            "none detected".to_string()
        } else {
            tools.join(", ")
        }
    ));
    out.push_str(&format!(
        "tags        {}\n",
        if caps.tags.is_empty() {
            "none".to_string()
        } else {
            caps.tags.iter().cloned().collect::<Vec<_>>().join(", ")
        }
    ));
    out
}

/// Placeholder so the type is used consistently until agent adapters land in phase 1.
#[must_use]
pub fn known_agent_kinds() -> Vec<AgentKind> {
    vec![AgentKind::ClaudeCode]
}

#[cfg(test)]
mod tests {
    /// Free disk moves by the megabyte on any working machine, and a moving capability is an
    /// incarnation every probe. Whole GiB, rounded down so it never claims space that is not there.
    #[test]
    fn free_disk_is_whole_gib_and_never_more_than_there_is() {
        use super::whole_gib_mb;
        assert_eq!(whole_gib_mb(266_794), 266_240);
        assert_eq!(whole_gib_mb(266_240), 266_240, "a whole GiB is left alone");
        assert_eq!(whole_gib_mb(1023), 0, "less than a GiB is none, not one");
        // The measured case: two probes a few megabytes apart are one fact.
        assert_eq!(whole_gib_mb(266_794), whole_gib_mb(266_800));
    }

    /// The line an owner reads has to agree with the gate that reads the same fact.
    #[test]
    fn the_power_line_says_what_the_policy_would_do() {
        use offload_core::PowerSource;

        // Charging *is* on mains for `on_mains` and therefore for every battery floor, so the
        // line has to say so rather than leave somebody to infer it from `charging: true`.
        let charging = PowerSource::Battery {
            percent: 96,
            charging: true,
        };
        assert!(charging.on_mains());
        assert_eq!(power_line(&charging), "on mains, battery 96%");

        let draining = PowerSource::Battery {
            percent: 40,
            charging: false,
        };
        assert!(!draining.on_mains());
        assert_eq!(power_line(&draining), "on battery, 40%");

        assert_eq!(power_line(&PowerSource::Ac), "on mains");

        // And the one that must not read as mains: `Ac` claims there is nothing to run out of,
        // and a battery that would not parse is not that.
        assert!(!PowerSource::Unknown.on_mains());
        assert!(
            power_line(&PowerSource::Unknown).contains("not on mains"),
            "{}",
            power_line(&PowerSource::Unknown)
        );
    }

    use super::*;

    #[test]
    fn extracts_versions_from_real_world_output() {
        assert_eq!(
            extract_version("rustc 1.82.0 (f6e511eec 2024-10-15)").as_deref(),
            Some("1.82.0")
        );
        assert_eq!(extract_version("v22.3.0\n").as_deref(), Some("22.3.0"));
        assert_eq!(
            extract_version("git version 2.45.2").as_deref(),
            Some("2.45.2")
        );
        assert_eq!(
            extract_version("go version go1.22.5 linux/amd64").as_deref(),
            Some("1.22.5")
        );
        assert_eq!(
            extract_version("Docker version 27.1.1, build 6312585").as_deref(),
            Some("27.1.1")
        );
        assert_eq!(extract_version("no version here").as_deref(), None);
    }

    #[test]
    fn a_desktop_on_battery_is_not_treated_as_stable() {
        let on_battery = PowerSource::Battery {
            percent: 80,
            charging: false,
        };
        assert_eq!(
            detect_stability(DeviceClass::Desktop, &on_battery),
            Stability::Transient
        );
        assert_eq!(
            detect_stability(DeviceClass::Desktop, &PowerSource::Ac),
            Stability::Stable
        );
    }

    #[test]
    fn probing_this_machine_produces_something_coherent() {
        let caps = probe();
        assert!(caps.cpu_cores >= 1);
        assert!(!summarise(&caps).is_empty());
    }

    /// **Every Mac used to come out `Unknown`, a MacBook included.**
    ///
    /// The fallback for a machine with no DMI asked `power::has_battery`, which reads
    /// `/sys/class/power_supply` — absent on macOS, so the answer was `false` unconditionally
    /// and the comment above it named macOS as a case it handled. `Unknown` maps to
    /// `Stability::Transient`, which is why the MacBook half was invisible: it is the answer a
    /// laptop should get, arrived at for the wrong reason. The Mac mini half is not invisible —
    /// a mains-only desktop scored as `Transient` bids below what it is.
    #[test]
    fn a_mac_is_classified_by_what_powers_it_rather_than_by_a_file_it_does_not_have() {
        // A Mac mini, Studio, Pro or iMac: `pmset` says AC and reports no battery line, which
        // `settle`'s own arms define as *mains present and nothing to run out of*.
        assert_eq!(
            class_from_power(&Os::MacOs, &PowerSource::Ac),
            DeviceClass::Desktop
        );
        // …and it is `Stable`, which is the whole consequence — the number that reaches a bid.
        assert_eq!(
            detect_stability(DeviceClass::Desktop, &PowerSource::Ac),
            Stability::Stable
        );

        // A MacBook: `pmset` reports a percentage whether or not it is plugged in.
        for power in [
            PowerSource::Battery {
                percent: 96,
                charging: true,
            },
            PowerSource::Battery {
                percent: 40,
                charging: false,
            },
        ] {
            assert_eq!(
                class_from_power(&Os::MacOs, &power),
                DeviceClass::Laptop,
                "a machine with something to run out of is portable: {power:?}"
            );
        }
    }

    /// The asymmetry in `class_from_power`, asserted rather than left to the comment.
    ///
    /// `Desktop` carries `Stability::Stable`, which moves bid scores — so it is claimed only
    /// where the platform gave a positive answer about a lineup that supports it. Reaching this
    /// function on Linux means there was no DMI to read, which is a container or an odd board.
    #[test]
    fn stability_is_not_claimed_for_a_machine_that_merely_failed_to_describe_itself() {
        assert_eq!(
            class_from_power(&Os::Linux, &PowerSource::Ac),
            DeviceClass::Unknown,
            "no DMI on Linux is a container, not a desktop"
        );
        assert_eq!(
            class_from_power(&Os::MacOs, &PowerSource::Unknown),
            DeviceClass::Unknown,
            "a battery whose level will not read: there is one, and that is not enough to say"
        );
        assert_eq!(
            class_from_power(&Os::Linux, &PowerSource::Unknown),
            DeviceClass::Unknown
        );
        // The one that matters: nothing here reaches `Stable` by accident.
        for (os, power) in [
            (Os::Linux, PowerSource::Ac),
            (Os::MacOs, PowerSource::Unknown),
            (Os::Linux, PowerSource::Unknown),
        ] {
            let class = class_from_power(&os, &power);
            assert_ne!(
                detect_stability(class, &power),
                Stability::Stable,
                "{os:?} on {power:?} was scored as stable on no evidence"
            );
        }
    }

    /// A phone is decided by its OS and never reaches the power fallback, which is what keeps
    /// a charging phone from being read as a laptop.
    #[test]
    fn a_charging_phone_is_still_a_phone() {
        assert_eq!(
            detect_device_class(
                &Os::Android,
                &PowerSource::Battery {
                    percent: 100,
                    charging: true,
                }
            ),
            DeviceClass::Phone
        );
        assert_eq!(
            DeviceClass::Phone.default_stability(),
            Stability::Ephemeral,
            "and it keeps the stability that makes the fleet expect it to vanish"
        );
    }
}
