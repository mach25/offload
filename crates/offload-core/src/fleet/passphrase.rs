//! The fleet's root secret: a generated passphrase, and the key derived from it.
//!
//! ADR-0012 makes the passphrase the anchor of the whole trust model — no device holds
//! signing authority at rest, so the fleet key exists only for the seconds it takes to sign
//! something and is reconstructed from a phrase a human typed. Two consequences shape
//! everything here:
//!
//! * **The passphrase is generated, never chosen.** The fleet public key is on every device
//!   and travels in every certificate, so anyone holding either can grind candidates
//!   offline. Entropy and a slow KDF are the only defences, and a human-chosen password
//!   defeats both.
//! * **The derivation is a wire format, not a tuning knob.** Salt, algorithm, parameters and
//!   normalisation all feed the same 32 bytes; change any of them and every existing fleet's
//!   identity changes, which means every certificate stops verifying and every device has to
//!   re-join. New values need a new version tag in [`KDF_SALT`], not an edit.
//!
//! Still pure, so it stays inside `offload-core`'s no-I/O rule (ADR-0001): entropy is
//! supplied by the caller and derivation is a function of its input. What it is *not* is
//! cheap — a derivation costs about a second by design, which is the point of it.

use super::FleetId;
use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

/// Words in a generated passphrase. Six of them from a 7776-word list is ~77 bits, which is
/// what ADR-0012's "at least six words" buys against an offline attack at ~1s a guess.
pub const WORDS: usize = 6;

/// Entropy [`Passphrase::generate`] consumes: four bytes per word, so that reducing a random
/// `u32` into the list costs no meaningful bias.
pub const ENTROPY_BYTES: usize = WORDS * 4;

/// Salt and format version in one. Bump the suffix to change anything about the derivation;
/// editing the parameters below without it silently re-founds every fleet in the world.
const KDF_SALT: &[u8] = b"offload-fleet-v1";

/// Memory cost in KiB. Argon2id's memory hardness is what makes GPU grinding expensive, and
/// it is the parameter worth spending on: 128 MiB is a fraction of a second on a desktop and
/// roughly the second ADR-0012 asks for on a phone, while staying well inside what a phone
/// can actually allocate.
const KDF_MEMORY_KIB: u32 = 128 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_PARALLELISM: u32 = 1;

/// The wordlist, parsed at first use. `#` lines carry its provenance.
const WORDLIST_SOURCE: &str = include_str!("wordlist.txt");

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PassphraseError {
    #[error("need {ENTROPY_BYTES} bytes of entropy for a {WORDS}-word passphrase, got {got}")]
    NotEnoughEntropy { got: usize },
    #[error("a fleet passphrase cannot be empty")]
    Empty,
    #[error("key derivation failed: {reason}")]
    Kdf { reason: String },
}

/// A fleet passphrase in memory.
///
/// Zeroized on drop and printed as `<redacted>`, because the one thing that must never
/// happen to it is ending up in a log line. Reading it back is deliberately called
/// [`Passphrase::expose`] rather than `as_str`, so every use of it is greppable.
pub struct Passphrase(Zeroizing<String>);

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrase(<redacted>)")
    }
}

impl Passphrase {
    /// Turn caller-supplied randomness into a diceware phrase.
    ///
    /// The caller owns the entropy because this crate has no access to any (ADR-0001); pass
    /// [`ENTROPY_BYTES`] from a CSPRNG. Four bytes select each word, so the modulo reduction
    /// is biased by about one part in half a million — far below the rounding in "77 bits".
    pub fn generate(entropy: &[u8]) -> Result<Passphrase, PassphraseError> {
        if entropy.len() < ENTROPY_BYTES {
            return Err(PassphraseError::NotEnoughEntropy { got: entropy.len() });
        }
        let list = wordlist();
        let words: Vec<&str> = entropy
            .chunks_exact(4)
            .take(WORDS)
            .map(|chunk| {
                let mut buf = [0u8; 4];
                buf.copy_from_slice(chunk);
                let index = u32::from_le_bytes(buf) as usize % list.len();
                list[index]
            })
            .collect();
        Ok(Passphrase(Zeroizing::new(words.join(" "))))
    }

    /// Adopt a phrase a human typed, normalised the same way [`FleetKey::derive`] will.
    pub fn typed(input: &str) -> Result<Passphrase, PassphraseError> {
        let normalized = normalize(input);
        if normalized.is_empty() {
            return Err(PassphraseError::Empty);
        }
        Ok(Passphrase(Zeroizing::new(normalized)))
    }

    /// The phrase itself. Print it once, write it down, do not store it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn word_count(&self) -> usize {
        self.0.split_whitespace().count()
    }

    /// Derive the fleet key this phrase stands for.
    pub fn derive(&self) -> Result<FleetKey, PassphraseError> {
        FleetKey::derive(self.expose())
    }
}

/// Canonical form of a typed passphrase.
///
/// Part of the derivation format, and forgiving on purpose: the phrase is generated from a
/// fixed lowercase wordlist, so case, run-together spacing and the hyphens in words like
/// `yo-yo` carry no information worth preserving. What that buys is the person retyping it
/// from a piece of paper a year from now, who has no way to tell a normalisation failure
/// from a wrong phrase — both just produce a different fleet.
#[must_use]
pub fn normalize(input: &str) -> String {
    input
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The fleet's signing key, alive only as long as the operation that needs it.
///
/// There is no way to serialise one, and that is the design: ADR-0012's central claim is
/// that a stolen device yields no ability to enrol anything, which holds exactly as long as
/// nothing writes this to disk. `SigningKey` zeroizes itself on drop.
pub struct FleetKey {
    signing: SigningKey,
    id: FleetId,
}

impl std::fmt::Debug for FleetKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FleetKey")
            .field("id", &self.id.short())
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl FleetKey {
    /// argon2id over the normalised passphrase, seeding an ed25519 keypair (ADR-0012).
    ///
    /// Costs a fraction of a second on a laptop and about a second on a phone, deliberately:
    /// it is the only thing standing between a leaked fleet public key and an offline search
    /// of the phrase space.
    pub fn derive(passphrase: &str) -> Result<FleetKey, PassphraseError> {
        let normalized = Zeroizing::new(normalize(passphrase));
        if normalized.is_empty() {
            return Err(PassphraseError::Empty);
        }

        let params = argon2::Params::new(KDF_MEMORY_KIB, KDF_ITERATIONS, KDF_PARALLELISM, Some(32))
            .map_err(|e| PassphraseError::Kdf {
                reason: e.to_string(),
            })?;
        let argon =
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        let mut seed = Zeroizing::new([0u8; 32]);
        argon
            .hash_password_into(normalized.as_bytes(), KDF_SALT, seed.as_mut())
            .map_err(|e| PassphraseError::Kdf {
                reason: e.to_string(),
            })?;

        let signing = SigningKey::from_bytes(&seed);
        let id = FleetId::from_bytes(signing.verifying_key().to_bytes());
        Ok(FleetKey { signing, id })
    }

    #[must_use]
    pub fn id(&self) -> FleetId {
        self.id
    }

    /// For signing certificates, delegations and revocations — nothing else needs it.
    #[must_use]
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    /// Is this the key that fleet expects? What `offload verify` asks, and what every
    /// passphrase-taking command checks before it signs anything.
    #[must_use]
    pub fn is(&self, fleet: FleetId) -> bool {
        self.id == fleet
    }
}

/// The wordlist as a slice, comments stripped.
///
/// Parsed on every call rather than cached: this runs once per `offload init`, and a
/// `OnceLock` here would be state in a crate that has none.
fn wordlist() -> Vec<&'static str> {
    WORDLIST_SOURCE
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entropy that is not random, so the words it picks are checkable.
    fn entropy(values: [u32; WORDS]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn the_wordlist_is_the_one_the_entropy_claim_assumes() {
        // 7776 = 6^5 is what makes a word worth 12.92 bits and six words worth ~77. A
        // pruned list would quietly weaken every passphrase ever generated.
        let list = wordlist();
        assert_eq!(list.len(), 7776);
        assert!(list.iter().all(|w| !w.is_empty()));

        let mut sorted = list.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), list.len(), "duplicates would cost entropy");
    }

    #[test]
    fn a_generated_passphrase_has_the_words_its_entropy_selected() {
        let list = wordlist();
        let phrase = Passphrase::generate(&entropy([0, 1, 2, 3, 4, 5])).expect("generate");
        assert_eq!(
            phrase.expose(),
            format!(
                "{} {} {} {} {} {}",
                list[0], list[1], list[2], list[3], list[4], list[5]
            )
        );
        assert_eq!(phrase.word_count(), WORDS);
    }

    #[test]
    fn generation_refuses_to_stretch_short_entropy() {
        // Silently generating a four-word phrase from sixteen bytes would look identical
        // and be a thousand times weaker.
        assert!(matches!(
            Passphrase::generate(&[0u8; 8]),
            Err(PassphraseError::NotEnoughEntropy { got: 8 })
        ));
    }

    #[test]
    fn retyping_forgives_case_spacing_and_hyphens() {
        // The person retyping this from paper cannot tell a normalisation failure from a
        // wrong phrase; both produce a fleet that isn't theirs.
        assert_eq!(normalize("  Yo-Yo   ABACUS\tzoom\n"), "yo yo abacus zoom");
        assert_eq!(normalize("yo yo abacus zoom"), "yo yo abacus zoom");
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn an_empty_passphrase_is_refused_rather_than_derived() {
        assert!(matches!(
            FleetKey::derive("   "),
            Err(PassphraseError::Empty)
        ));
        assert!(matches!(Passphrase::typed(""), Err(PassphraseError::Empty)));
    }

    #[test]
    fn the_derivation_is_pinned_to_a_known_answer() {
        // This is a format test, not a maths test. Salt, algorithm, parameters and
        // normalisation all feed these 32 bytes, so any edit to them breaks this — which is
        // the only warning anyone gets that they have just invalidated every certificate in
        // every fleet. Change it deliberately, with a new version in KDF_SALT.
        let key = FleetKey::derive("abacus zoom yo-yo abdomen zebra puppy").expect("derive");
        assert_eq!(
            key.id().to_string(),
            "4f6822fe2802654136002a9ff5298cef01111072de16d4c00b60b6d4af1f4799"
        );
    }

    #[test]
    fn the_same_phrase_typed_differently_reaches_the_same_fleet() {
        let a = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let b = FleetKey::derive("  ABACUS  zoom   yo yo ").expect("derive");
        assert_eq!(a.id(), b.id());
        assert!(a.is(b.id()));
    }

    #[test]
    fn a_different_phrase_is_a_different_fleet() {
        // Not a property of argon2 worth testing so much as the reason `offload verify`
        // exists: nothing about a wrong phrase fails loudly, it just derives another fleet.
        let a = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let b = FleetKey::derive("abacus zoom yoyo").expect("derive");
        assert_ne!(a.id(), b.id());
    }
}
