//! Time as a value, never as an ambient effect.
//!
//! Nothing in this crate reads the clock. Callers pass `now` in. Simulation tests drive it
//! manually, which is the only way partition and churn scenarios stay reproducible.

use serde::{Deserialize, Serialize};
use std::ops::{Add, Sub};

/// Milliseconds. Used for both instants (since unix epoch) and durations — the distinction
/// is carried by the parameter name, not the type. Kept deliberately simple; if that ever
/// causes a real bug, split it into `Instant`/`Duration` newtypes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Millis(pub u64);

impl Millis {
    pub const ZERO: Millis = Millis(0);

    #[must_use]
    pub const fn from_secs(secs: u64) -> Self {
        Millis(secs.saturating_mul(1_000))
    }

    #[must_use]
    pub const fn from_mins(mins: u64) -> Self {
        Millis::from_secs(mins.saturating_mul(60))
    }

    #[must_use]
    pub const fn as_secs(self) -> u64 {
        self.0 / 1_000
    }

    #[must_use]
    pub const fn saturating_sub(self, other: Millis) -> Millis {
        Millis(self.0.saturating_sub(other.0))
    }

    #[must_use]
    pub const fn saturating_add(self, other: Millis) -> Millis {
        Millis(self.0.saturating_add(other.0))
    }

    #[must_use]
    pub const fn saturating_mul(self, factor: u64) -> Millis {
        Millis(self.0.saturating_mul(factor))
    }

    /// Scale by a percentage. Used by backoff and grace-period maths, which want
    /// "150% of the usual return time" without dragging floats into a type that gets
    /// compared for equality in tests.
    #[must_use]
    pub const fn scaled_percent(self, percent: u64) -> Millis {
        Millis(self.0.saturating_mul(percent) / 100)
    }

    #[must_use]
    pub fn clamp_range(self, lo: Millis, hi: Millis) -> Millis {
        if self < lo {
            lo
        } else if self > hi {
            hi
        } else {
            self
        }
    }
}

impl Add for Millis {
    type Output = Millis;
    fn add(self, rhs: Millis) -> Millis {
        self.saturating_add(rhs)
    }
}

impl Sub for Millis {
    type Output = Millis;
    fn sub(self, rhs: Millis) -> Millis {
        self.saturating_sub(rhs)
    }
}

impl std::fmt::Display for Millis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 < 1_000 {
            write!(f, "{}ms", self.0)
        } else if self.0 < 60_000 {
            write!(f, "{:.1}s", self.0 as f64 / 1_000.0)
        } else if self.0 < 3_600_000 {
            write!(f, "{}m{}s", self.0 / 60_000, (self.0 % 60_000) / 1_000)
        } else {
            // Deadlines are the reason this branch exists: "540m0s" is a correct way to say
            // nine hours and not a way anybody reads one.
            write!(
                f,
                "{}h{}m",
                self.0 / 3_600_000,
                (self.0 % 3_600_000) / 60_000
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturates_instead_of_panicking() {
        assert_eq!(Millis(5) - Millis(10), Millis::ZERO);
        assert_eq!(Millis(u64::MAX) + Millis(10), Millis(u64::MAX));
    }

    #[test]
    fn scaling_and_clamping() {
        assert_eq!(Millis(1000).scaled_percent(150), Millis(1500));
        assert_eq!(
            Millis(50).clamp_range(Millis(100), Millis(200)),
            Millis(100)
        );
        assert_eq!(
            Millis(500).clamp_range(Millis(100), Millis(200)),
            Millis(200)
        );
    }
}
