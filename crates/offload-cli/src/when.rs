//! Saying when something is due, on a command line.
//!
//! Deliberately relative — `45m`, `2h`, `1h30m`, `9h` — and deliberately **not** wall-clock
//! (`08:00`). ADR-0013 writes deadlines as absolute instants and that is what travels; what is
//! missing here is a timezone. Turning `08:00` into an instant needs the local offset, which
//! is a dependency this crate does not have and a guess it must not make: a deadline quietly
//! interpreted an hour out is exactly the kind of confidently wrong answer the rest of this
//! project spends its effort avoiding. Everything else about time in this CLI is already
//! relative — `seen 4s ago`, `overdue by 3m` — so this is the house style rather than a
//! concession.
//!
//! The result is an absolute unix instant, computed here rather than sent as a duration for
//! the daemon to add to its own clock: a deadline is a point in time, and one of the two
//! machines has to pin it down.

use anyhow::{bail, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Parse a duration from now into an absolute unix-millisecond deadline.
pub fn parse_deadline(input: &str) -> Result<u64> {
    let delta = parse_duration(input)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_millis();
    let now = u64::try_from(now).unwrap_or(u64::MAX);
    Ok(now.saturating_add(delta))
}

/// Parse what `offload deadline <run> <when>` was given: a duration, or `none`.
///
/// `none` is a real answer rather than a way of clearing a field: a run with no deadline is due
/// *as soon as somebody can take it*, which is what it meant before anybody said otherwise.
pub fn parse_change(input: &str) -> Result<Option<u64>> {
    match input.trim().to_ascii_lowercase().as_str() {
        "none" | "asap" | "-" => Ok(None),
        _ => parse_deadline(input).map(Some),
    }
}

/// Now, in unix milliseconds: the one reading of the wall clock the CLI's reports share.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// How long there is until an absolute unix-millisecond instant, for saying it back.
pub fn until(deadline_ms: u64) -> String {
    offload_node::render::until(deadline_ms, now_ms())
}

/// `90s`, `45m`, `2h`, `1h30m`, `3d`. Milliseconds.
pub fn parse_duration(input: &str) -> Result<u64> {
    let text = input.trim().to_ascii_lowercase();
    if text.is_empty() {
        bail!("expected a duration like 45m, 2h or 1h30m");
    }

    let mut total: u64 = 0;
    let mut digits = String::new();
    let mut saw_unit = false;

    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let unit_ms = match ch {
            's' => 1_000,
            'm' => 60_000,
            'h' => 3_600_000,
            'd' => 86_400_000,
            _ => bail!("{input}: unknown unit {ch:?} — use s, m, h or d"),
        };
        if digits.is_empty() {
            bail!("{input}: {ch:?} needs a number in front of it");
        }
        let value: u64 = digits
            .parse()
            .with_context(|| format!("{input}: {digits} is not a number"))?;
        total = total.saturating_add(value.saturating_mul(unit_ms));
        digits.clear();
        saw_unit = true;
    }

    if !digits.is_empty() {
        // A bare number is ambiguous in the way that matters — 30 seconds and 30 minutes are
        // different answers to "when is this due" — so it is refused rather than assumed.
        bail!("{input}: needs a unit, e.g. {digits}m for minutes or {digits}h for hours");
    }
    if !saw_unit {
        bail!("expected a duration like 45m, 2h or 1h30m");
    }
    Ok(total)
}

/// The same duration, in seconds, for the one place a *duration* is what is stored.
///
/// `parse_deadline` resolves against the clock because a submitted run's deadline is an instant.
/// A standing instruction's is not: it applies to a run that does not exist yet, and resolving it
/// here would put every firing after the first one past a deadline set this morning (ADR-0020).
pub fn parse_duration_secs(input: &str) -> Result<u64> {
    Ok(parse_duration(input)? / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_add_up() {
        assert_eq!(parse_duration("90s").unwrap(), 90_000);
        assert_eq!(parse_duration("45m").unwrap(), 2_700_000);
        assert_eq!(parse_duration("2h").unwrap(), 7_200_000);
        assert_eq!(parse_duration("1h30m").unwrap(), 5_400_000);
        assert_eq!(parse_duration(" 3D ").unwrap(), 259_200_000);
    }

    #[test]
    fn ambiguity_is_refused_rather_than_guessed() {
        // "30" is thirty of something, and being wrong about which one is worse than asking.
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("m").is_err());
        assert!(parse_duration("08:00").is_err(), "no timezone, no guessing");
        assert!(parse_duration("2w").is_err());
    }

    #[test]
    fn a_deadline_is_in_the_future() {
        let then = parse_deadline("1h").unwrap();
        let now = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        assert!(then > now && then <= now + 3_600_000);
    }
}
