//! What the mesh needs from a blob store, and nothing more.
//!
//! `offload-cluster` does not depend on `offload-store` — the mesh should not know that
//! anything is stored in SQLite, and the tests should not need a database to move bytes
//! between two nodes. This trait is the whole surface: does this node have a hash, hand it
//! over, take one.
//!
//! **Integrity is the caller's, and it is free.** Blobs are content-addressed (ADR-0003), so
//! a fetch verifies what arrived by hashing it. That is why `store` returns the hash it
//! computed rather than accepting the one it was promised: a peer that serves the wrong bytes
//! achieves a retry, not a poisoned checkpoint (ADR-0016).

use async_trait::async_trait;
use offload_core::BlobHash;

/// Async because `offload-store` is synchronous and the node's implementation has to get off
/// the reactor to use it (`spawn_blocking`, per the workspace's own rule). A megabyte written
/// inline is a small stall in the failure detector, which is precisely the thing that must not
/// stall.
#[async_trait]
pub trait Blobs: Send + Sync + 'static {
    async fn has(&self, hash: BlobHash) -> bool;

    /// The bytes, or `None` if this node does not hold them.
    ///
    /// Whole-blob rather than streaming, because a checkpoint is megabytes: a transcript, a
    /// `base..HEAD` bundle, a patch. If something ever puts a repository in here, this is the
    /// signature to change first.
    async fn get(&self, hash: BlobHash) -> Option<Vec<u8>>;

    /// Store bytes and return the hash they actually have.
    async fn store(&self, bytes: Vec<u8>) -> Result<BlobHash, String>;

    /// Whether this node is willing to take a pushed blob of `size` right now.
    ///
    /// Separate from having room: a phone on mobile data may be perfectly able to hold a
    /// checkpoint and quite right to refuse it (ADR-0016 gates replication on the receiver's
    /// policy, like any other work).
    fn accepts_push(&self, _hash: BlobHash, _size: u64) -> bool {
        true
    }
}

/// A store that holds nothing and accepts nothing.
///
/// For tests that are about liveness rather than blobs, and for a node with no store at all —
/// which is a legitimate member: a phone that can only deliver notifications has no runs to
/// checkpoint.
#[derive(Debug, Default)]
pub struct NoBlobs;

#[async_trait]
impl Blobs for NoBlobs {
    async fn has(&self, _hash: BlobHash) -> bool {
        false
    }

    async fn get(&self, _hash: BlobHash) -> Option<Vec<u8>> {
        None
    }

    async fn store(&self, _bytes: Vec<u8>) -> Result<BlobHash, String> {
        Err("this node does not store blobs".into())
    }

    fn accepts_push(&self, _hash: BlobHash, _size: u64) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// An in-memory store, which is all the mesh's tests need.
    #[derive(Debug, Default)]
    pub struct MemoryBlobs {
        held: Mutex<HashMap<BlobHash, Vec<u8>>>,
    }

    #[async_trait]
    impl Blobs for MemoryBlobs {
        async fn has(&self, hash: BlobHash) -> bool {
            self.held.lock().expect("lock").contains_key(&hash)
        }
        async fn get(&self, hash: BlobHash) -> Option<Vec<u8>> {
            self.held.lock().expect("lock").get(&hash).cloned()
        }
        async fn store(&self, bytes: Vec<u8>) -> Result<BlobHash, String> {
            let hash = BlobHash::from_bytes(*blake3::hash(&bytes).as_bytes());
            self.held.lock().expect("lock").insert(hash, bytes);
            Ok(hash)
        }
    }

    #[tokio::test]
    async fn a_node_with_no_store_refuses_rather_than_pretending() {
        // `NoBlobs` is not a stub: a phone that only delivers notifications is a member with
        // nowhere to put a checkpoint, and it has to say so rather than accept and drop it.
        let nothing = NoBlobs;
        assert!(!nothing.has(BlobHash::from_bytes([1; 32])).await);
        assert!(nothing.store(b"anything".to_vec()).await.is_err());
        assert!(!nothing.accepts_push(BlobHash::from_bytes([1; 32]), 10));
    }

    #[tokio::test]
    async fn storing_returns_the_hash_the_bytes_actually_have() {
        // The property the fetch path leans on: the hash is computed, never taken on trust.
        let store = MemoryBlobs::default();
        let hash = store.store(b"a transcript".to_vec()).await.expect("store");
        assert!(store.has(hash).await);
        assert_eq!(store.get(hash).await.as_deref(), Some(&b"a transcript"[..]));
        assert_ne!(
            hash,
            store
                .store(b"a different transcript".to_vec())
                .await
                .expect("store")
        );
    }
}
