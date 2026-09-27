//! How busy this machine is right now.
//!
//! The one capability-adjacent fact that is *not* a capability: load is not what a device
//! *is*, it is what it happens to be doing this minute, so it never gets gossiped and never
//! reaches a `Capabilities`. It goes into `LocalFacts` at bid time, where ADR-0013 wants it —
//! declared demand gates the burst, and this is what catches the liars.
//!
//! `None` rather than a plausible zero where the platform will not say. A node reporting 0%
//! because nobody asked the kernel is claiming to be idle, which is the over-claiming the
//! probe exists not to do: it would win a bid it should lose and then serve the run badly.

use offload_core::Os;

/// This machine's load as a percentage of its cores, or `None` if the platform won't say.
///
/// The one-minute load average, deliberately: five and fifteen are too slow to notice the
/// build that started thirty seconds ago, and there is nothing faster available portably.
/// Being a *lagging* indicator is a known and accepted property (ADR-0013) — a node can
/// accept two runs before either registers, which is why the declared budget gates the burst
/// and this only has to catch the machine that is already busy.
///
/// Scaled by cores, so it reads the same on a phone and a workstation: 100% means "as many
/// runnable tasks as this machine has cores", not "one core busy". Clamped there rather than
/// reported as 400%, because everything downstream is a percentage of a budget.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn cpu_load_percent(os: &Os, cores: u32) -> Option<u8> {
    // Windows has no load average, and sysinfo answers 0.0 there rather than failing —
    // which is exactly the plausible zero this returns `None` for instead.
    if os == &Os::Windows {
        return None;
    }
    // …and the same plausible zero where the file exists and this process may not read it. An
    // Android *app* is denied `/proc/loadavg` by SELinux (measured on the SDK emulator, ADR-0066:
    // `avc: denied { read } for name="loadavg"`), and sysinfo answers a failed read with 0.0 — so
    // the app reported `cpu 0%`, idle for ever, which is the over-claim this function exists to
    // refuse. Asked of the file itself, since that is the thing that can be denied.
    if matches!(os, Os::Linux | Os::Android) && std::fs::read_to_string("/proc/loadavg").is_err() {
        return None;
    }
    let one_minute = sysinfo::System::load_average().one;
    if !one_minute.is_finite() || one_minute < 0.0 {
        return None;
    }
    // Clamped before the cast, so the truncation the lint is about cannot happen: the value
    // is in 0..=100 and rounded by the time it becomes an integer.
    let per_core = (one_minute * 100.0 / f64::from(cores.max(1)))
        .round()
        .clamp(0.0, 100.0);
    Some(per_core as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_says_nothing_rather_than_claiming_to_be_idle() {
        assert_eq!(cpu_load_percent(&Os::Windows, 8), None);
    }

    #[test]
    fn load_is_a_percentage_of_this_machine_and_never_exceeds_it() {
        // Whatever this machine is doing while the test runs, the answer has to be in range —
        // a `u8` that saturated or a percentage over 100 would both mislead the budget
        // arithmetic downstream rather than fail visibly.
        let Some(percent) = cpu_load_percent(&Os::Linux, 1) else {
            return;
        };
        assert!(percent <= 100, "{percent}% is not a percentage");
    }
}
