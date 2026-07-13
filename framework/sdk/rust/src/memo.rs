//! Cross-run work memoisation (optional framework mode).
//!
//! When enabled, the host treats each ready work unit as a pure function of
//! `(module, module_digest, capability, input artifact ids)`. A hit reuses
//! prior output artifact refs via a real kernel `Commit` (no `execute`).
//! A miss runs the plugin and records a new memo entry.
//!
//! Invalidation is content-based: change the module digest or any input
//! artifact id and the key misses; new output ids cascade to downstream nodes.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use srcport_substrate::{ArtifactRef, NamedArtifact, WorkItem};

use crate::FrameworkError;

/// One cached production path (ids only — bodies live in the kernel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoRecord {
    /// Content key ([`memo_key`]).
    pub key: String,
    /// Module name.
    pub module: String,
    /// Module version from the assembly / work item.
    pub module_version: String,
    /// Plugin content digest that participated in the key.
    pub module_digest: String,
    /// Capability name.
    pub capability: String,
    /// Assembly node id that produced this entry (observational).
    pub node_id: String,
    /// Input port → artifact id (sorted on write).
    pub inputs: BTreeMap<String, String>,
    /// Output port → artifact id.
    pub outputs: BTreeMap<String, String>,
    /// Run that first stored this entry.
    pub source_run_id: String,
    /// Work id that first stored this entry.
    pub source_work_id: String,
}

/// Which assembly nodes may use the memo store.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MemoNodes {
    /// Every node with a resolvable module digest (subject to [`MemoPlan::require_digest`]).
    #[default]
    All,
    /// Only these assembly node ids.
    Only(Vec<String>),
    /// All except these assembly node ids.
    Except(Vec<String>),
}

impl MemoNodes {
    /// Whether `node_id` is eligible for memo lookup / store.
    pub fn allows(&self, node_id: &str) -> bool {
        match self {
            MemoNodes::All => true,
            MemoNodes::Only(ids) => ids.iter().any(|i| i == node_id),
            MemoNodes::Except(ids) => !ids.iter().any(|i| i == node_id),
        }
    }
}

/// Host-only memo policy (never enters the kernel).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoPlan {
    /// When false, host never looks up or stores memo entries.
    pub enabled: bool,
    /// If true, nodes without [`crate::ModulePlugin::module_digest`] always miss
    /// (and never store). If false, empty digest still means uncacheable miss.
    pub require_digest: bool,
    /// Node filter.
    pub nodes: MemoNodes,
}

impl MemoPlan {
    /// Memoisation off (default).
    pub fn off() -> Self {
        Self::default()
    }

    /// Enable memo with safe defaults: require a non-empty module digest.
    pub fn on() -> Self {
        Self {
            enabled: true,
            require_digest: true,
            nodes: MemoNodes::All,
        }
    }

    /// Enable but allow missing digests to be treated as uncacheable without error.
    pub fn on_optional_digest() -> Self {
        Self {
            enabled: true,
            require_digest: false,
            nodes: MemoNodes::All,
        }
    }

    /// Restrict which nodes participate.
    pub fn with_nodes(mut self, nodes: MemoNodes) -> Self {
        self.nodes = nodes;
        self
    }

    /// Require / not require non-empty digests for caching.
    pub fn with_require_digest(mut self, require: bool) -> Self {
        self.require_digest = require;
        self
    }
}

/// Durable (or in-memory) index from memo key → record.
///
/// Implementations must be safe for concurrent host use if shared across
/// threads; [`MemoryMemo`] uses an internal mutex.
pub trait MemoStore: Send {
    /// Lookup by key. `None` = miss.
    fn get(&self, key: &str) -> Result<Option<MemoRecord>, FrameworkError>;

    /// Insert or replace a record under its key.
    fn put(&mut self, record: MemoRecord) -> Result<(), FrameworkError>;

    /// Number of stored entries (tests / metrics).
    fn len(&self) -> usize;

    /// Whether the store is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove all entries (tests).
    fn clear(&mut self);
}

/// Process-local memo store (shared across runs on the same host).
#[derive(Debug, Default, Clone)]
pub struct MemoryMemo {
    inner: Arc<Mutex<HashMap<String, MemoRecord>>>,
}

impl MemoryMemo {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Shared handle (cheap clone) for the same map.
    pub fn shared(self) -> Self {
        self
    }
}

impl MemoStore for MemoryMemo {
    fn get(&self, key: &str) -> Result<Option<MemoRecord>, FrameworkError> {
        let g = self
            .inner
            .lock()
            .map_err(|_| FrameworkError::Invalid("memo store lock poisoned".into()))?;
        Ok(g.get(key).cloned())
    }

    fn put(&mut self, record: MemoRecord) -> Result<(), FrameworkError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| FrameworkError::Invalid("memo store lock poisoned".into()))?;
        g.insert(record.key.clone(), record);
        Ok(())
    }

    fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|g| g.len())
            .unwrap_or(0)
    }

    fn clear(&mut self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
    }
}

/// Build the stable memo key for a work unit.
///
/// ```text
/// key = "sha256:" + hex(sha256(
///   module ‖ 0x00 ‖ module_version ‖ 0x00 ‖ module_digest ‖ 0x00 ‖
///   capability ‖ 0x00 ‖
///   for each input port in UTF-8 order:
///     port ‖ 0x00 ‖ artifact_id ‖ 0x00
/// ))
/// ```
pub fn memo_key(
    module: &str,
    module_version: &str,
    module_digest: &str,
    capability: &str,
    inputs: &BTreeMap<String, String>,
) -> String {
    let mut h = Sha256::new();
    h.update(module.as_bytes());
    h.update([0u8]);
    h.update(module_version.as_bytes());
    h.update([0u8]);
    h.update(module_digest.as_bytes());
    h.update([0u8]);
    h.update(capability.as_bytes());
    h.update([0u8]);
    for (port, art_id) in inputs {
        h.update(port.as_bytes());
        h.update([0u8]);
        h.update(art_id.as_bytes());
        h.update([0u8]);
    }
    format!("sha256:{}", hex_encode(&h.finalize()))
}

/// Extract sorted port → artifact id map from a claimed work item.
pub fn input_fingerprint_map(work: &WorkItem) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for na in &work.inputs {
        if let Some(r) = na.artifact.as_ref() {
            if !na.name.is_empty() && !r.id.is_empty() {
                m.insert(na.name.clone(), r.id.clone());
            }
        }
    }
    m
}

/// Convert a memo record's outputs into commit-ready [`NamedArtifact`]s.
pub fn record_to_named_outputs(record: &MemoRecord) -> Vec<NamedArtifact> {
    record
        .outputs
        .iter()
        .map(|(port, id)| NamedArtifact {
            name: port.clone(),
            artifact: Some(ArtifactRef {
                id: id.clone(),
            }),
        })
        .collect()
}

/// Build a [`MemoRecord`] after a successful execute/commit.
pub fn build_record(
    key: String,
    work: &WorkItem,
    module_digest: &str,
    outputs: &[NamedArtifact],
    run_id: &str,
) -> MemoRecord {
    let mut out = BTreeMap::new();
    for na in outputs {
        if let Some(r) = na.artifact.as_ref() {
            if !na.name.is_empty() && !r.id.is_empty() {
                out.insert(na.name.clone(), r.id.clone());
            }
        }
    }
    MemoRecord {
        key,
        module: work.module.clone(),
        module_version: work.module_version.clone(),
        module_digest: module_digest.into(),
        capability: work.capability.clone(),
        node_id: work.node_id.clone(),
        inputs: input_fingerprint_map(work),
        outputs: out,
        source_run_id: run_id.into(),
        source_work_id: work.id.clone(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn key_stable_and_order_independent_of_map_insert_order() {
        let mut a = BTreeMap::new();
        a.insert("b".into(), "id2".into());
        a.insert("a".into(), "id1".into());
        let mut b = BTreeMap::new();
        b.insert("a".into(), "id1".into());
        b.insert("b".into(), "id2".into());
        assert_eq!(
            memo_key("m", "1", "d", "cap", &a),
            memo_key("m", "1", "d", "cap", &b)
        );
    }

    #[test]
    fn key_changes_with_digest_or_input() {
        let mut inputs = BTreeMap::new();
        inputs.insert("in".into(), "art1".into());
        let k1 = memo_key("m", "1", "d1", "cap", &inputs);
        let k2 = memo_key("m", "1", "d2", "cap", &inputs);
        inputs.insert("in".into(), "art2".into());
        let k3 = memo_key("m", "1", "d1", "cap", &inputs);
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn memory_memo_roundtrip() {
        let mut store = MemoryMemo::new();
        let rec = MemoRecord {
            key: "k".into(),
            module: "m".into(),
            module_version: "1".into(),
            module_digest: "d".into(),
            capability: "c".into(),
            node_id: "n".into(),
            inputs: BTreeMap::new(),
            outputs: BTreeMap::from([("out".into(), "aid".into())]),
            source_run_id: "r".into(),
            source_work_id: "w".into(),
        };
        store.put(rec.clone()).unwrap();
        assert_eq!(store.get("k").unwrap().unwrap(), rec);
        assert_eq!(store.len(), 1);
        store.clear();
        assert!(store.is_empty());
    }
}
