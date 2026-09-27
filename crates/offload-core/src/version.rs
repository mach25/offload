//! Minimal version comparison for capability matching.
//!
//! Not a semver implementation. We need to answer one question — "is the agent/toolchain on
//! this node at least version X" — across strings emitted by whatever `--version` prints.
//! Pre-release and build metadata are stripped rather than ordered, because for eligibility
//! `2.1.0-beta` should count as `2.1.0`; if a run genuinely cannot tolerate a pre-release,
//! that is a tag, not a version comparison.

use std::cmp::Ordering;

/// Compare two dotted version strings numerically component by component.
///
/// Missing trailing components are treated as zero, so `1.2` == `1.2.0`. Non-numeric
/// components fall back to lexical comparison.
#[must_use]
pub fn compare(a: &str, b: &str) -> Ordering {
    let mut left = core_part(a).split('.');
    let mut right = core_part(b).split('.');

    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (Some(x), Some(y)) => {
                let ord = match (x.trim().parse::<u64>(), y.trim().parse::<u64>()) {
                    (Ok(xn), Ok(yn)) => xn.cmp(&yn),
                    _ => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            // One side ran out: it is only equal if the remainder is all zeroes.
            (None, Some(y)) => {
                if y.trim().parse::<u64>() != Ok(0) {
                    return Ordering::Less;
                }
            }
            (Some(x), None) => {
                if x.trim().parse::<u64>() != Ok(0) {
                    return Ordering::Greater;
                }
            }
        }
    }
}

/// `true` if `have` is at least `want`.
#[must_use]
pub fn at_least(have: &str, want: &str) -> bool {
    compare(have, want) != Ordering::Less
}

/// Strip a leading `v` and anything from the first `-` or `+`, and keep only the leading
/// dotted-numeric run. Turns `claude-code v2.1.4-beta.3 (build 91)` into `2.1.4` once the
/// caller has isolated the token; tolerant of the common decorations.
fn core_part(s: &str) -> &str {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let end = s.find(['-', '+', ' ']).unwrap_or(s.len());
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numerically_not_lexically() {
        // The bug this exists to prevent: "2.10.0" < "2.9.0" under string comparison.
        assert_eq!(compare("2.10.0", "2.9.0"), Ordering::Greater);
        assert!(at_least("2.10.0", "2.9.0"));
    }

    #[test]
    fn missing_components_are_zero() {
        assert_eq!(compare("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare("1.2.0.0", "1.2"), Ordering::Equal);
        assert_eq!(compare("1.2", "1.2.1"), Ordering::Less);
    }

    #[test]
    fn decorations_are_stripped() {
        assert_eq!(compare("v2.1.4", "2.1.4"), Ordering::Equal);
        assert_eq!(compare("2.1.4-beta.3", "2.1.4"), Ordering::Equal);
    }

    #[test]
    fn at_least_is_inclusive() {
        assert!(at_least("2.1.4", "2.1.4"));
        assert!(!at_least("2.1.3", "2.1.4"));
    }
}
