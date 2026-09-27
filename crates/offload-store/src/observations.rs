//! Per-node absence history.
//!
//! ADR-0007's hold-down policy waits longer for a node whose absences are historically
//! short. In phase 1 that history lived in memory, so it reset on every daemon restart —
//! which is precisely when a node has just been absent. Every restart threw away the
//! evidence of the event that caused it.
//!
//! Small table, disproportionate value: it is the difference between a policy that adapts
//! to each device and one that applies the same guess to a phone and a rack server.
//!
//! **That paragraph was written in the past tense and was true in the present until ADR-0031.**
//! This module — the table, its writer, its reader — had **no reference in production code at
//! all**, so the restart case it describes as fixed was not. Found by sweeping for public
//! functions nothing calls, in the same pass that found a second start gate and a second copy of
//! the averaging that used to live below. The lesson is the module's own: a file that says it
//! fixed something is not evidence that anything calls it.

use crate::{now_ms, Store, StoreError};
use offload_core::{Millis, NodeId, NodeObservation, NodeStatus};
use std::collections::BTreeMap;

/// What the store keeps about one node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObservation {
    pub observation: NodeObservation,
    /// When the node went away, if it is away now.
    pub absent_since: Option<Millis>,
    pub last_seen: Millis,
}

impl Store {
    pub fn save_observation(
        &self,
        node: NodeId,
        stored: &StoredObservation,
    ) -> Result<(), StoreError> {
        let status = serde_json::to_string(&stored.observation.status)
            .map_err(|e| StoreError::Encode(e.to_string()))?;

        self.conn().execute(
            "INSERT INTO node_observations
                 (node_id, status, absences, typical_absence_ms, absent_since_ms, last_seen_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(node_id) DO UPDATE SET
                 status             = excluded.status,
                 absences           = excluded.absences,
                 typical_absence_ms = excluded.typical_absence_ms,
                 absent_since_ms    = excluded.absent_since_ms,
                 last_seen_ms       = excluded.last_seen_ms",
            rusqlite::params![
                node.as_bytes().as_slice(),
                status,
                i64::from(stored.observation.observed_absences),
                stored
                    .observation
                    .typical_absence
                    .map(|m| i64::try_from(m.0).unwrap_or(i64::MAX)),
                stored
                    .absent_since
                    .map(|m| i64::try_from(m.0).unwrap_or(i64::MAX)),
                i64::try_from(stored.last_seen.0).unwrap_or(now_ms()),
            ],
        )?;
        Ok(())
    }

    pub fn load_observation(&self, node: NodeId) -> Result<Option<StoredObservation>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT status, absences, typical_absence_ms, absent_since_ms, last_seen_ms
             FROM node_observations WHERE node_id = ?1",
        )?;
        let mut rows = stmt.query([node.as_bytes().as_slice()])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        decode_row(row).map(Some)
    }

    pub fn all_observations(&self) -> Result<BTreeMap<NodeId, StoredObservation>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT node_id, status, absences, typical_absence_ms, absent_since_ms, last_seen_ms
             FROM node_observations",
        )?;
        let mut rows = stmt.query([])?;

        let mut out = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let raw: Vec<u8> = row.get(0)?;
            let Ok(bytes): Result<[u8; 32], _> = raw.try_into() else {
                continue;
            };
            // Column 0 is the id; the decoder expects the remaining columns to start at 0,
            // so read them by name-equivalent offsets here.
            let stored = StoredObservation {
                observation: NodeObservation {
                    status: decode_status(&row.get::<_, String>(1)?),
                    typical_absence: row.get::<_, Option<i64>>(3)?.map(as_millis),
                    observed_absences: row.get::<_, i64>(2)?.try_into().unwrap_or(u32::MAX),
                },
                absent_since: row.get::<_, Option<i64>>(4)?.map(as_millis),
                last_seen: as_millis(row.get::<_, i64>(5)?),
            };
            out.insert(NodeId::from_bytes(bytes), stored);
        }
        Ok(out)
    }

    // There was a `record_return` here that folded an absence into the average itself — a second
    // copy of `NodeView::set_status`'s arithmetic, with a doc comment naming the hazard ("both
    // places must agree, or a restart would visibly change how patient the policy is") and no
    // caller on either side. One learner is the fix rather than two that are asked to agree: the
    // view computes the average and this table persists what it produced (ADR-0031).
}

fn decode_row(row: &rusqlite::Row<'_>) -> Result<StoredObservation, StoreError> {
    Ok(StoredObservation {
        observation: NodeObservation {
            status: decode_status(&row.get::<_, String>(0)?),
            typical_absence: row.get::<_, Option<i64>>(2)?.map(as_millis),
            observed_absences: row.get::<_, i64>(1)?.try_into().unwrap_or(u32::MAX),
        },
        absent_since: row.get::<_, Option<i64>>(3)?.map(as_millis),
        last_seen: as_millis(row.get::<_, i64>(4)?),
    })
}

/// An unreadable status defaults to `Suspect` rather than failing the load.
///
/// `Suspect` is the honest answer to "we do not know": it is neither an assertion that the
/// node is fine nor that it is gone, and it is the state the failure detector would put an
/// unknown node in anyway.
fn decode_status(raw: &str) -> NodeStatus {
    serde_json::from_str(raw).unwrap_or(NodeStatus::Suspect)
}

fn as_millis(value: i64) -> Millis {
    Millis(u64::try_from(value).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    /// A learned observation, as `NodeView::observation()` produces one.
    fn learned(absences: u32, typical: Millis, now: Millis) -> StoredObservation {
        StoredObservation {
            observation: NodeObservation {
                status: NodeStatus::Alive,
                typical_absence: Some(typical),
                observed_absences: absences,
            },
            absent_since: None,
            last_seen: now,
        }
    }

    #[test]
    fn absence_history_survives_a_restart() {
        // The whole point. A restart is exactly when a node has been absent, so losing the
        // history at that moment discarded the evidence of the event that caused it — and until
        // ADR-0031 nothing in production called any of this, so it still did.
        //
        // Through `save_observation`, which is the writer the daemon uses. It used to go through
        // a `record_return` that computed the average itself, so the test exercised a second copy
        // of the arithmetic instead of the path that runs.
        let dir = std::env::temp_dir().join(format!("offload-obs-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let store = Store::open(&dir).expect("open");
        store
            .save_observation(node(1), &learned(1, Millis::from_secs(90), Millis(10_000)))
            .expect("save");
        drop(store);

        let reopened = Store::open(&dir).expect("reopen");
        let stored = reopened
            .load_observation(node(1))
            .expect("load")
            .expect("present");

        assert_eq!(stored.observation.observed_absences, 1);
        assert_eq!(
            stored.observation.typical_absence,
            Some(Millis::from_secs(90))
        );

        // …and a later save replaces it rather than accumulating rows, which is what makes a
        // per-tick write cheap enough to do at all.
        let reopened2 = reopened;
        reopened2
            .save_observation(node(1), &learned(2, Millis::from_secs(75), Millis(20_000)))
            .expect("save");
        assert_eq!(
            reopened2.all_observations().expect("all").len(),
            1,
            "one row per node"
        );
        assert_eq!(
            reopened2
                .load_observation(node(1))
                .expect("load")
                .expect("present")
                .observation
                .observed_absences,
            2
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn all_observations_returns_every_node() {
        let store = Store::open_memory().expect("open");
        for b in 1..=3 {
            store
                .save_observation(node(b), &learned(1, Millis::from_secs(10), Millis(0)))
                .expect("save");
        }
        assert_eq!(store.all_observations().expect("all").len(), 3);
    }
}
