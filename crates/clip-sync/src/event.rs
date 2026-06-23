//! Content-addressed clip events and conflict-free merge.
//!
//! Every change is an immutable event keyed by the clip's content hash. Devices
//! exchange events freely and converge by **last-writer-wins per clip**, ordered
//! by HLC then device id (a deterministic tiebreak). Because the key is the
//! content hash, the *entire* history merges consistently — that's what lets
//! Clip keep all of history in sync instead of one most-recent slot.

use std::collections::BTreeMap;

use clip_core::ContentKind;
use serde::{Deserialize, Serialize};

use crate::hlc::Hlc;

/// What happened to a clip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// Created or re-observed. Carries the metadata needed to render a row;
    /// the body/blob travels separately (see [`crate::chunk`]).
    Upsert {
        kind: ContentKind,
        preview: String,
        blob_ref: Option<String>,
        size: u64,
    },
    /// Pinned / unpinned.
    Pin(bool),
    /// Removed from history.
    Delete,
}

/// One immutable change to one clip, stamped for causal ordering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipEvent {
    /// Content hash of the clip this event concerns.
    pub clip_id: String,
    pub op: Op,
    pub hlc: Hlc,
    /// Originating device id; the deterministic tiebreak when HLCs are equal.
    pub node: String,
}

impl ClipEvent {
    /// Total order used to pick the winner: HLC first, device id as tiebreak.
    fn rank(&self) -> (Hlc, &str) {
        (self.hlc, self.node.as_str())
    }
}

/// Merge a stream of events into the current state per clip, deterministically.
///
/// For each `clip_id` the highest-ranked event wins. A winning [`Op::Delete`]
/// removes the clip entirely; otherwise the latest event is its current state.
/// Order-independent and idempotent — feeding the same events in any order on
/// any device yields the same result.
pub fn merge<I>(events: I) -> BTreeMap<String, ClipEvent>
where
    I: IntoIterator<Item = ClipEvent>,
{
    let mut latest: BTreeMap<String, ClipEvent> = BTreeMap::new();
    for ev in events {
        match latest.get(&ev.clip_id) {
            Some(cur) if cur.rank() >= ev.rank() => {}
            _ => {
                latest.insert(ev.clip_id.clone(), ev);
            }
        }
    }
    latest.retain(|_, ev| !matches!(ev.op, Op::Delete));
    latest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upsert(id: &str, hlc: Hlc, node: &str) -> ClipEvent {
        ClipEvent {
            clip_id: id.into(),
            op: Op::Upsert {
                kind: ContentKind::Text,
                preview: format!("{id}@{}", hlc.wall_ms),
                blob_ref: None,
                size: 1,
            },
            hlc,
            node: node.into(),
        }
    }

    #[test]
    fn latest_wins_by_hlc() {
        let events = vec![
            upsert("a", Hlc::new(1, 0), "n1"),
            upsert("a", Hlc::new(5, 0), "n1"),
            upsert("a", Hlc::new(3, 0), "n1"),
        ];
        let state = merge(events);
        if let Op::Upsert { preview, .. } = &state["a"].op {
            assert_eq!(preview, "a@5");
        } else {
            panic!()
        }
    }

    #[test]
    fn merge_is_order_independent() {
        let e1 = upsert("a", Hlc::new(1, 0), "n1");
        let e2 = upsert("a", Hlc::new(2, 0), "n2");
        let forward = merge(vec![e1.clone(), e2.clone()]);
        let backward = merge(vec![e2, e1]);
        assert_eq!(forward, backward);
    }

    #[test]
    fn equal_hlc_breaks_tie_by_node_deterministically() {
        let a = upsert("x", Hlc::new(7, 0), "nodeA");
        let b = upsert("x", Hlc::new(7, 0), "nodeB");
        // nodeB > nodeA, so b wins regardless of insertion order.
        assert_eq!(merge(vec![a.clone(), b.clone()])["x"].node, "nodeB");
        assert_eq!(merge(vec![b, a])["x"].node, "nodeB");
    }

    #[test]
    fn delete_wins_when_latest_and_removes_clip() {
        let events = vec![
            upsert("a", Hlc::new(1, 0), "n1"),
            ClipEvent {
                clip_id: "a".into(),
                op: Op::Delete,
                hlc: Hlc::new(2, 0),
                node: "n1".into(),
            },
        ];
        assert!(!merge(events).contains_key("a"));
    }

    #[test]
    fn older_delete_does_not_remove_newer_upsert() {
        let events = vec![
            ClipEvent {
                clip_id: "a".into(),
                op: Op::Delete,
                hlc: Hlc::new(1, 0),
                node: "n1".into(),
            },
            upsert("a", Hlc::new(2, 0), "n1"),
        ];
        assert!(merge(events).contains_key("a"));
    }
}
