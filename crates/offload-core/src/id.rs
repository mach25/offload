//! Identities. All fixed-size byte arrays with hex representations.
//!
//! Generation lives outside this crate — `NodeId` will derive from an ed25519 public key
//! (phase 3) and `RunId` from a UUIDv7 minted by the daemon. Core only carries them, so it
//! stays free of randomness and clock access.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable identity of a node, persisted in its state dir.
///
/// Currently opaque bytes. From phase 3 this is the ed25519 public key, so that a node
/// cannot claim another's identity simply by asserting a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId([u8; 32]);

/// Identity of a single agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId([u8; 16]);

/// BLAKE3 hash of a blob: transcript, git bundle, dirty patch, run output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlobHash([u8; 32]);

/// Identity of one standing instruction: when this trigger fires, submit this run (ADR-0020).
///
/// Node-local and never gossiped, which is why it is minted from randomness rather than being
/// made comparable across nodes the way a `NodeId` or a `BlobHash` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RuleId([u8; 8]);

/// Identity of one standing instruction on a clock (ADR-0019 §3, ADR-0056).
///
/// Unlike a [`RuleId`] this **is** gossiped — a schedule outlives the device that created it,
/// which is the whole difference between it and `cron` — so it is compared across nodes. Random
/// rather than derived for the same reason a `RuleId` is: there is nothing about a schedule that
/// two nodes would need to compute the same name for. What they do compute the same name for is
/// each *occurrence*, and that is a `RunId` (`Schedule::occurrence_id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScheduleId([u8; 8]);

macro_rules! byte_id {
    ($ty:ty, $len:expr, $short:expr) => {
        impl $ty {
            #[must_use]
            pub const fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            /// Truncated form for logs. Never use for equality or lookup.
            #[must_use]
            pub fn short(&self) -> String {
                hex_encode(&self.0[..$short])
            }

            pub fn parse_hex(s: &str) -> Result<Self, IdParseError> {
                let bytes = hex_decode(s)?;
                let arr: [u8; $len] = bytes
                    .try_into()
                    .map_err(|_| IdParseError::WrongLength { expected: $len })?;
                Ok(Self(arr))
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&hex_encode(&self.0))
            }
        }

        impl std::str::FromStr for $ty {
            type Err = IdParseError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse_hex(s)
            }
        }
    };
}

byte_id!(NodeId, 32, 4);
// Six bytes for a run, four for the others, and the difference is not a style choice. A `NodeId`
// is an ed25519 public key and a `BlobHash` is a BLAKE3 digest, so their leading bytes are
// uniform and eight hex characters tell a personal fleet's devices apart. A `RunId` is a UUIDv7,
// whose first six bytes are a 48-bit millisecond clock — so the first *four* are the top 32 bits
// of it, constant for 65_536 ms, and every run submitted within about a minute has the same
// eight-character prefix. That is `CLAUDE.md`'s "never use an abbreviated run id as an identity"
// arriving in the one place that made it unavoidable: a refusal naming `01a02606` named two runs.
// Twelve characters is exactly the whole clock, which is why the CLI had already picked it.
byte_id!(RunId, 16, 6);
byte_id!(BlobHash, 32, 4);
// Eight bytes, all eight shown, and both halves are deliberate. A `RuleId` (ADR-0020) is minted
// from randomness rather than from a clock, so it has no leading-bytes problem to abbreviate
// around — and there are a handful of rules on a node rather than thousands of runs, so the whole
// id is short enough to type and abbreviating it would buy nothing while costing the one thing
// `RunId::short` had to learn: an id that names two things.
byte_id!(RuleId, 8, 8);
// Eight bytes and all eight shown, for `RuleId`'s reasons exactly: a `ScheduleId` is random
// rather than a clock, and a fleet holds a handful of schedules rather than thousands of runs.
// It is *not* an occurrence's id — that is a `RunId` derived from this and the tick, which is
// where the clock lives (`Schedule::occurrence_id`).
byte_id!(ScheduleId, 8, 8);

/// What an operator typed where an identifier was expected — three answers, not two.
///
/// Every command here takes an *abbreviation*: `offload cancel 01a09473` is the whole point of
/// `RunId::short`. So something has to decide what may be one, and the answer is not a `bool`
/// because the two refusals need different sentences — *you left the argument out* and *that is
/// not an abbreviation of an id* send somebody to different places.
///
/// **It exists because there were two of them.** `Store::resolve_run` had this rule, with the
/// reasoning beside it, and `server::find_run` — reached exactly when the store says no, to look
/// in the cluster view for a run this node has heard of but not stored — re-implemented the
/// prefix match without it. `"".starts_with("")` is true of every run, so on a node with one run
/// in view an **empty** argument resolved to it. Measured: `offload cancel ""`, `offload explain
/// ""`, `offload checkpoint ""` and `offload audit ""` each acted on a real run, while `offload
/// logs ""`, `offload rm ""` and `offload resume ""` refused — the same argument, validated on
/// three commands and not on four, and the four included the two that stop work. An unset shell
/// variable is how an empty argument arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Needle {
    /// Nothing was typed. Not an abbreviation of everything.
    Empty,
    /// Typed, and not hex — so it is not an abbreviation of any identifier here.
    NotHex,
    /// Usable as a prefix: trimmed and lowercased, which is the form to match with.
    Prefix(String),
}

/// Normalise and judge what was typed where an identifier was expected. See [`Needle`].
#[must_use]
pub fn needle(raw: &str) -> Needle {
    let needle = raw.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Needle::Empty;
    }
    if !needle.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Needle::NotHex;
    }
    Needle::Prefix(needle)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdParseError {
    #[error("identifier contains a non-hex character")]
    NotHex,
    #[error("identifier has an odd number of hex digits")]
    OddLength,
    #[error("identifier has the wrong length, expected {expected} bytes")]
    WrongLength { expected: usize },
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(s: &str) -> Result<Vec<u8>, IdParseError> {
    if s.len() % 2 != 0 {
        return Err(IdParseError::OddLength);
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(c: u8) -> Result<u8, IdParseError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(IdParseError::NotHex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_hex() {
        let id = NodeId::from_bytes([0xab; 32]);
        let parsed: NodeId = id.to_string().parse().expect("round trip");
        assert_eq!(id, parsed);
        assert_eq!(id.short(), "abababab");
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(RunId::parse_hex("zz"), Err(IdParseError::NotHex));
        assert_eq!(RunId::parse_hex("abc"), Err(IdParseError::OddLength));
        assert_eq!(
            RunId::parse_hex("abcd"),
            Err(IdParseError::WrongLength { expected: 16 })
        );
    }

    #[test]
    fn two_runs_a_second_apart_have_different_short_forms() {
        // The length of a `RunId::short` is a fact about UUIDv7's layout, not a taste in
        // formatting, and nothing in the tree checked it. Bytes 0..6 are a 48-bit millisecond
        // clock, so bytes 0..4 — the old eight characters — are the top 32 bits of it and hold
        // still for 65_536 ms. Every run submitted within about a minute printed the same
        // fragment, and a refusal saying "run 01a02606 cannot be cancelled" named two of them.
        //
        // Written out by hand rather than minted, because generation lives outside this crate by
        // decision (see the module header). What is asserted is the layout the length depends on.
        let at = |ms: u64| {
            let mut bytes = [0x11; 16];
            bytes[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
            // Version and variant nibbles, so this is a UUIDv7 and not just sixteen bytes.
            bytes[6] = 0x70 | (bytes[6] & 0x0f);
            bytes[8] = 0x80 | (bytes[8] & 0x3f);
            RunId::from_bytes(bytes)
        };
        let now = 1_787_344_311_459;
        let a = at(now);
        let b = at(now + 1_000);
        assert_ne!(a, b);
        assert_ne!(
            a.short(),
            b.short(),
            "a displayed run id has to tell two runs a second apart apart"
        );
        // And the boundary the length was chosen for: the same millisecond is the one tie left,
        // which `Store::resolve_run` reports as ambiguous rather than guessing between.
        assert_eq!(a.short(), at(now).short());
        assert_eq!(a.short().len(), 12);
    }

    #[test]
    fn ordering_is_by_bytes_so_election_is_deterministic() {
        let a = NodeId::from_bytes([0x01; 32]);
        let b = NodeId::from_bytes([0x02; 32]);
        assert!(a < b);
    }
}
