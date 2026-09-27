//! Power source detection.
//!
//! Matters more than it looks: it gates whether a phone accepts work at all, and it is a
//! large term in bid scoring. `Unknown` is a real answer and must not be conflated with
//! "on mains" — a policy of `WhenCharging` should refuse when we cannot tell.
//!
//! The hard part is not reading the files. It is that **not every battery on a machine powers
//! the machine**: a wireless mouse, a keyboard, a touchscreen and a game controller each
//! register under `/sys/class/power_supply` with `type=Battery` and a `capacity`, and they are
//! indistinguishable from the system battery unless you ask. The kernel provides the answer —
//! `scope=Device` on a peripheral, absent or `System` on the real one — and this did not ask
//! it, so whichever entry `read_dir` happened to yield last became this machine's battery
//! level. Measured on the laptop this was written on: `BAT0` at 98%, a touchscreen's phantom
//! battery at 0%, and `offload probe` reporting **0%**.

use offload_core::capability::PowerSource;
use std::fs;
use std::path::Path;

const POWER_SUPPLY: &str = "/sys/class/power_supply";

#[must_use]
pub fn detect() -> PowerSource {
    if let Some(source) = detect_linux() {
        return source;
    }
    if let Some(source) = detect_termux() {
        return source;
    }
    if let Some(source) = detect_macos() {
        return source;
    }
    PowerSource::Unknown
}

/// One entry under `/sys/class/power_supply`, as far as this decision cares.
///
/// Kept as data so that [`settle`] is a pure function of what the directory said: the bug this
/// replaces was reachable only through `read_dir` order, which no test that touches a real
/// `/sys` can arrange.
#[derive(Debug, Clone, Default)]
struct Supply {
    kind: String,
    /// `Device` for a peripheral's own battery, `System` or absent for the machine's.
    ///
    /// Absent is deliberately read as the machine's: the attribute post-dates the class, real
    /// system batteries omit it (`BAT0` on this laptop has no `scope` file at all), and a
    /// peripheral driver that reports a battery without scoping it is rarer than a laptop.
    scope: Option<String>,
    /// `present=0` is a battery *bay* with nothing in it — a docking station, a removable
    /// pack that has been taken out — and its capacity reads as 0.
    present: Option<bool>,
    capacity: Option<u8>,
    status: Option<String>,
    online: bool,
}

impl Supply {
    fn is_system_battery(&self) -> bool {
        self.kind == "Battery"
            && self.scope.as_deref() != Some("Device")
            && self.present != Some(false)
    }

    fn is_charging(&self) -> bool {
        matches!(self.status.as_deref(), Some("Charging" | "Full"))
    }
}

fn read_supplies() -> Vec<Supply> {
    let Ok(entries) = fs::read_dir(POWER_SUPPLY) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            let read = |name: &str| {
                fs::read_to_string(path.join(name))
                    .ok()
                    .map(|s| s.trim().to_string())
            };
            Supply {
                kind: read("type").unwrap_or_default(),
                scope: read("scope"),
                present: read("present").and_then(|s| match s.as_str() {
                    "0" => Some(false),
                    "1" => Some(true),
                    _ => None,
                }),
                capacity: read("capacity").and_then(|s| s.parse::<u8>().ok()),
                status: read("status"),
                online: read("online").as_deref() == Some("1"),
            }
        })
        .collect()
}

fn detect_linux() -> Option<PowerSource> {
    if !Path::new(POWER_SUPPLY).exists() {
        return None;
    }
    settle(&read_supplies())
}

/// What a directory full of power supplies adds up to.
///
/// Order-independent by construction, which the version this replaces was not: it assigned to
/// one slot per battery it saw, so the answer depended on which name `read_dir` yielded last.
/// The same machine could answer differently across boots.
///
/// Where a machine has more than one system battery — dual-battery laptops are real — the
/// **lowest** is reported. It is not the right arithmetic (that is an energy-weighted average
/// over `energy_now`/`energy_full`, which is a bigger read for a number nothing here is precise
/// about) and it is the right *direction*: this crate's whole job is not to over-claim, and a
/// floor that trips early costs an idle node while one that trips late costs a run.
fn settle(supplies: &[Supply]) -> Option<PowerSource> {
    let mains_online = supplies
        .iter()
        .any(|supply| supply.kind == "Mains" && supply.online);

    let batteries: Vec<&Supply> = supplies
        .iter()
        .filter(|supply| supply.is_system_battery())
        .collect();
    let charging = batteries.iter().any(|supply| supply.is_charging()) || mains_online;

    match batteries.iter().filter_map(|s| s.capacity).min() {
        Some(percent) => Some(PowerSource::Battery {
            percent: percent.min(100),
            charging,
        }),
        // A battery this machine runs on, whose level will not read. Not `Ac`, which claims
        // there is nothing to run out of, and not a guess at a number: the one thing known is
        // that the answer is unknown, and `WhenCharging` refusing on that is the module's own
        // rule rather than a degradation of it.
        None if !batteries.is_empty() => Some(PowerSource::Unknown),
        // Mains present with no battery: a desktop or server.
        None if mains_online => Some(PowerSource::Ac),
        None => None,
    }
}

/// Android's own answer, through Termux:API, for the phone that may not read `/sys`.
///
/// Measured on a Samsung phone (Android 16) under Termux: `/sys/class/power_supply` is `Permission
/// denied` to an app, so `detect_linux` finds nothing and the phone reports `Unknown`, and a
/// default phone (`WhenCharging`) then refuses all work, plugged in or not. `termux-battery-status`
/// asks `BatteryManager` through the Termux:API app, which is the platform API the roadmap's
/// "battery … from platform APIs" item names.
///
/// Bounded, because without the Termux:API *app* installed the command waits for a reply that
/// never comes: it is killed after [`TERMUX_API_PATIENCE`] and the answer stays unknown.
fn detect_termux() -> Option<PowerSource> {
    let prefix = std::env::var_os("PREFIX")?;
    let command = Path::new(&prefix).join("bin/termux-battery-status");
    if !command.is_file() {
        return None;
    }
    let mut child = std::process::Command::new(&command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TERMUX_API_PATIENCE => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    parse_termux_battery(&out)
}

/// How long `termux-battery-status` may take before it is taken as not answering.
const TERMUX_API_PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// `termux-battery-status`'s JSON, as far as this decision cares: the level, and whether anything
/// is feeding it. `plugged` is the cable and `status` the charger's view; either one saying power is
/// coming in is on mains, because a full battery on its charger reads `FULL` and `NOT_CHARGING`
/// while it is plugged in and has nothing to run out of.
fn parse_termux_battery(json: &str) -> Option<PowerSource> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let percent = value.get("percentage")?.as_u64()?;
    let plugged = value
        .get("plugged")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|p| p != "UNPLUGGED");
    let charging = plugged
        || matches!(
            value.get("status").and_then(serde_json::Value::as_str),
            Some("CHARGING" | "FULL")
        );
    Some(PowerSource::Battery {
        percent: u8::try_from(percent.min(100)).unwrap_or(100),
        charging,
    })
}

fn detect_macos() -> Option<PowerSource> {
    if std::env::consts::OS != "macos" {
        return None;
    }
    let out = std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);

    // Format: "Now drawing from 'AC Power'\n -InternalBattery-0 (id=...)\t100%; charged; ..."
    let on_ac = text.contains("'AC Power'");
    let percent = text
        .split_whitespace()
        .find(|tok| tok.ends_with("%;") || tok.ends_with('%'))
        .and_then(|tok| tok.trim_end_matches([';', '%']).parse::<u8>().ok());

    match percent {
        Some(percent) => Some(PowerSource::Battery {
            percent: percent.min(100),
            charging: on_ac,
        }),
        None if on_ac => Some(PowerSource::Ac),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Termux:API's answer, in the shapes Android gives it. Plugged is mains even when the
    /// battery is full and "not charging"; unplugged is a battery; anything unparseable is no
    /// answer at all, so the probe falls through to `Unknown` rather than guessing.
    #[test]
    fn termux_battery_status_is_read_as_the_phone_says() {
        let on_cable = r#"{"health":"GOOD","percentage":84,"plugged":"PLUGGED_AC","status":"CHARGING","temperature":29.1}"#;
        assert_eq!(
            parse_termux_battery(on_cable),
            Some(PowerSource::Battery {
                percent: 84,
                charging: true
            })
        );
        let full = r#"{"percentage":100,"plugged":"PLUGGED_USB","status":"NOT_CHARGING"}"#;
        assert_eq!(
            parse_termux_battery(full),
            Some(PowerSource::Battery {
                percent: 100,
                charging: true
            })
        );
        let unplugged = r#"{"percentage":57,"plugged":"UNPLUGGED","status":"DISCHARGING"}"#;
        assert_eq!(
            parse_termux_battery(unplugged),
            Some(PowerSource::Battery {
                percent: 57,
                charging: false
            })
        );
        assert_eq!(parse_termux_battery(""), None);
        assert_eq!(parse_termux_battery(r#"{"plugged":"PLUGGED_AC"}"#), None);
    }

    fn mains(online: bool) -> Supply {
        Supply {
            kind: "Mains".into(),
            online,
            ..Supply::default()
        }
    }

    fn battery(capacity: Option<u8>, status: &str) -> Supply {
        Supply {
            kind: "Battery".into(),
            capacity,
            status: Some(status.into()),
            present: Some(true),
            ..Supply::default()
        }
    }

    /// A peripheral's battery: a mouse, a keyboard, a touchscreen, a controller.
    fn peripheral(capacity: Option<u8>) -> Supply {
        Supply {
            kind: "Battery".into(),
            scope: Some("Device".into()),
            capacity,
            ..Supply::default()
        }
    }

    /// The machine this was written on, entry for entry — and the answer it used to give.
    ///
    /// `BAT0` at 98% with no `scope` file, an ELAN touchscreen reporting a phantom battery at
    /// 0% with `scope=Device` and `present=0`, a Logitech receiver whose `capacity` is empty,
    /// and mains online. `offload probe` printed `Battery { percent: 0, charging: true }`:
    /// unplug that machine and every battery floor in the fleet refuses it, at 98% charge.
    #[test]
    fn a_touchscreens_phantom_battery_is_not_this_machines_battery() {
        let supplies = vec![
            mains(true),
            battery(Some(98), "Not charging"),
            Supply {
                kind: "Battery".into(),
                scope: Some("Device".into()),
                present: Some(false),
                capacity: Some(0),
                status: Some("Unknown".into()),
                online: true,
            },
            Supply {
                kind: "Battery".into(),
                scope: Some("Device".into()),
                capacity: None,
                ..Supply::default()
            },
        ];
        assert_eq!(
            settle(&supplies),
            Some(PowerSource::Battery {
                percent: 98,
                charging: true
            })
        );
    }

    #[test]
    fn the_answer_does_not_depend_on_what_the_directory_yielded_last() {
        // The property the bug lived in. `read_dir` order is filesystem order, so the version
        // this replaces could answer 98% or 5% on the same machine across two boots.
        let laptop = battery(Some(98), "Not charging");
        let mouse = peripheral(Some(5));
        assert_eq!(
            settle(&[mains(false), laptop.clone(), mouse.clone()]),
            settle(&[mouse, mains(false), laptop]),
        );
    }

    #[test]
    fn a_desktop_with_a_wireless_mouse_is_still_a_desktop() {
        // This answer feeds the device class where DMI is absent — containers, macOS, BSD — and
        // a desktop misread as a laptop gets a laptop's default stability and policy. It used to
        // feed it through `has_battery`, a `/sys`-only reader that could not answer on macOS at
        // all; `PowerSource` is what travels now (`probe::class_from_power`), and `Ac` is the
        // arm that says *mains, and nothing to run out of*.
        assert_eq!(
            settle(&[mains(true), peripheral(Some(70))]),
            Some(PowerSource::Ac)
        );
        assert!(!peripheral(Some(70)).is_system_battery());
    }

    #[test]
    fn a_battery_that_will_not_say_is_unknown_rather_than_mains() {
        // The module's own rule, at the one place it was not honoured: a system battery whose
        // capacity does not parse used to fall through to "mains present, so `Ac`" — which
        // says there is nothing to run out of, on a machine that has a battery.
        assert_eq!(
            settle(&[mains(true), battery(None, "Discharging")]),
            Some(PowerSource::Unknown)
        );
        // And with no battery at all, `Ac` is the truth.
        assert_eq!(settle(&[mains(true)]), Some(PowerSource::Ac));
        // Nothing readable at all is nothing claimed.
        assert_eq!(settle(&[]), None);
    }

    #[test]
    fn two_batteries_report_the_one_that_runs_out_first() {
        assert_eq!(
            settle(&[
                battery(Some(80), "Discharging"),
                battery(Some(20), "Discharging")
            ]),
            Some(PowerSource::Battery {
                percent: 20,
                charging: false
            })
        );
    }

    #[test]
    fn an_empty_bay_is_not_a_flat_battery() {
        // `present=0` with `capacity=0` is a dock or a removed pack. Reading it as the machine's
        // level is a node at 0% that is not going anywhere.
        let empty_bay = Supply {
            kind: "Battery".into(),
            present: Some(false),
            capacity: Some(0),
            ..Supply::default()
        };
        assert_eq!(
            settle(&[mains(false), battery(Some(64), "Discharging"), empty_bay]),
            Some(PowerSource::Battery {
                percent: 64,
                charging: false
            })
        );
    }

    #[test]
    fn charging_is_true_when_the_machine_is_plugged_in_however_the_battery_words_it() {
        // `status` is driver prose — "Not charging" is what a laptop at its charge threshold
        // says while plugged in — so mains being online settles it either way.
        assert_eq!(
            settle(&[mains(true), battery(Some(98), "Not charging")]),
            Some(PowerSource::Battery {
                percent: 98,
                charging: true
            })
        );
        assert_eq!(
            settle(&[mains(false), battery(Some(98), "Charging")]),
            Some(PowerSource::Battery {
                percent: 98,
                charging: true
            })
        );
    }
}
