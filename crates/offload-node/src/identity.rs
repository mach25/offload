//! This node's stable identity: an ed25519 keypair.
//!
//! The public half *is* the `NodeId` (ADR-0012), which is what stops a node from claiming
//! another's identity by asserting it, and what lets the QUIC transport authenticate a peer
//! by pinning rather than by trusting a name. It is also why an approver needs no key
//! material beyond its membership: a `NodeId` is already a verifying key.
//!
//! Persisted on first start and never regenerated. A node that changes id on restart looks
//! to the fleet like a new device every time, so its absence history — the input the
//! drop-off policy depends on — never accumulates, and its membership certificate stops
//! matching it.

use offload_core::NodeId;
use rand::rngs::OsRng;
use rand::RngCore;
use std::path::Path;

/// The secret key file. Separate from the old `node-id` (which held a public value) because
/// the file's contents and its permissions now mean something different.
pub const KEY_FILE: &str = "node-key";
/// Pre-phase-3 identity: 32 random bytes with no key behind them.
pub const LEGACY_FILE: &str = "node-id";

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("could not read identity at {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("could not write identity to {path}: {reason}")]
    Write { path: String, reason: String },
    #[error("identity file {path} is corrupt: {reason}")]
    Corrupt { path: String, reason: String },
}

/// This node's keypair.
///
/// `Debug` is implemented by hand and prints only the public id: the secret must never reach
/// a log line, and a derived `Debug` is how that happens by accident.
pub struct NodeIdentity {
    signing: ed25519_dalek::SigningKey,
    id: NodeId,
}

impl std::fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeIdentity")
            .field("id", &self.id.short())
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl NodeIdentity {
    #[must_use]
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// The signing key, for issuing credentials this node is entitled to issue — an
    /// approver's membership certificates, and its own transport certificate.
    #[must_use]
    pub fn signing_key(&self) -> &ed25519_dalek::SigningKey {
        &self.signing
    }

    fn from_seed(seed: [u8; 32]) -> Self {
        let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
        let id = NodeId::from_bytes(signing.verifying_key().to_bytes());
        NodeIdentity { signing, id }
    }
}

/// Load this node's keypair, creating one on first start.
///
/// A pre-phase-3 `node-id` file is reported rather than silently upgraded: its 32 bytes were
/// never a public key, so there is no keypair to recover and the node's identity necessarily
/// changes. Saying so is the difference between "your node has a new id" and a fleet quietly
/// treating it as a stranger.
pub fn load_or_create(state_dir: &Path) -> Result<NodeIdentity, IdentityError> {
    let path = state_dir.join(KEY_FILE);

    if path.is_file() {
        let text = std::fs::read_to_string(&path).map_err(|e| IdentityError::Read {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        let bytes = hex::decode(text.trim()).map_err(|e| IdentityError::Corrupt {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        let seed: [u8; 32] = bytes.try_into().map_err(|_| IdentityError::Corrupt {
            path: path.display().to_string(),
            reason: "expected 32 bytes of key material".into(),
        })?;
        return Ok(NodeIdentity::from_seed(seed));
    }

    let legacy = state_dir.join(LEGACY_FILE);
    if legacy.is_file() {
        tracing::warn!(
            path = %legacy.display(),
            "found a pre-keypair identity; this node is getting a new id, and will have to \
             re-join its fleet"
        );
    }

    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let identity = NodeIdentity::from_seed(seed);

    std::fs::create_dir_all(state_dir).map_err(|e| IdentityError::Write {
        path: state_dir.display().to_string(),
        reason: e.to_string(),
    })?;
    write_secret(&path, &hex::encode(seed))?;

    tracing::info!(node_id = %identity.id, "minted node identity");
    Ok(identity)
}

/// Write a secret with an owner-only mode, and never widen an existing one.
fn write_secret(path: &Path, contents: &str) -> Result<(), IdentityError> {
    let write = |path: &Path| -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(contents.as_bytes())?;
            file.write_all(b"\n")
        }
        #[cfg(not(unix))]
        std::fs::write(path, format!("{contents}\n"))
    };

    write(path).map_err(|e| IdentityError::Write {
        path: path.display().to_string(),
        reason: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "offload-id-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).expect("create scratch");
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn identity_survives_restart() {
        // Restarting must not look like a new device: the absence history the drop-off
        // policy relies on would reset, and the node's membership certificate names the id.
        let scratch = Scratch::new("restart");
        let first = load_or_create(&scratch.0).expect("create");
        let second = load_or_create(&scratch.0).expect("load");
        assert_eq!(first.id(), second.id());
        assert_eq!(
            first.signing_key().to_bytes(),
            second.signing_key().to_bytes(),
            "the key came back, not just the id"
        );
    }

    #[test]
    fn the_node_id_is_the_public_key() {
        // What ADR-0012 rests on: a peer can verify a signature from this node using only
        // its id, so an approver needs no key material beyond its membership.
        let scratch = Scratch::new("pubkey");
        let identity = load_or_create(&scratch.0).expect("create");

        let verifying =
            ed25519_dalek::VerifyingKey::from_bytes(identity.id().as_bytes()).expect("valid key");
        assert_eq!(verifying, identity.signing_key().verifying_key());

        use ed25519_dalek::{Signer, Verifier};
        let signature = identity.signing_key().sign(b"a message");
        assert!(verifying.verify(b"a message", &signature).is_ok());
    }

    #[test]
    fn separate_state_dirs_get_separate_identities() {
        // Two nodes on one machine is how a device joins two fleets (ADR-0012), and they
        // must be unlinkable rather than merely different.
        let a = Scratch::new("two-a");
        let b = Scratch::new("two-b");
        assert_ne!(
            load_or_create(&a.0).expect("a").id(),
            load_or_create(&b.0).expect("b").id()
        );
    }

    #[test]
    fn the_secret_is_not_world_readable() {
        let scratch = Scratch::new("mode");
        load_or_create(&scratch.0).expect("create");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(scratch.0.join(KEY_FILE))
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "group and other must have nothing");
        }
    }

    #[test]
    fn a_corrupt_key_file_is_reported_not_silently_replaced() {
        // Minting a new key would orphan every run recorded under the old id and quietly
        // eject the node from its fleet.
        let scratch = Scratch::new("corrupt");
        std::fs::write(scratch.0.join(KEY_FILE), "not a key").expect("write");

        assert!(matches!(
            load_or_create(&scratch.0),
            Err(IdentityError::Corrupt { .. })
        ));
    }

    #[test]
    fn a_key_of_the_wrong_length_is_corrupt_rather_than_padded() {
        let scratch = Scratch::new("short");
        std::fs::write(scratch.0.join(KEY_FILE), "abcd").expect("write");
        assert!(matches!(
            load_or_create(&scratch.0),
            Err(IdentityError::Corrupt { .. })
        ));
    }

    #[test]
    fn a_legacy_identity_is_replaced_rather_than_reused() {
        // Its 32 bytes were never a public key, so there is no keypair to recover. The node
        // necessarily gets a new id; the warning is what makes that visible instead of
        // leaving the fleet to treat it as a stranger.
        let scratch = Scratch::new("legacy");
        let old = "aa".repeat(32);
        std::fs::write(scratch.0.join(LEGACY_FILE), &old).expect("write");

        let identity = load_or_create(&scratch.0).expect("create");
        assert_ne!(identity.id().to_string(), old);
        assert!(scratch.0.join(KEY_FILE).is_file());
    }
}
