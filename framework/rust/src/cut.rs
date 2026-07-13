//! Assembly cut + seed helpers for "start after / from" without a hard-coded DAG.
//!
//! Readiness is pure dataflow: a node is claimable when bound input artifacts
//! exist. Skipping means dropping producers and **seeding** the cut frontier
//! with artifact refs (fixtures or prior-run outputs).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use srcport_substrate::{
    Assembly, AssemblyNode, Binding, Derivation, KernelApi, NamedArtifact, RequestContext, RunRef,
};

use crate::policy::NodePlan;
use crate::FrameworkError;

/// Prefix for synthetic run-input names created at a cut edge.
pub const SEED_INPUT_PREFIX: &str = "__seed/";

/// Stable synthetic input name for a dropped producer's output port.
///
/// Format: `__seed/{from_node}/{from_port}`.
pub fn seed_input_name(from_node: &str, from_port: &str) -> String {
    format!("{SEED_INPUT_PREFIX}{from_node}/{from_port}")
}

/// Whether a run-input name was invented by a cut (not a human-authored input).
pub fn is_seed_input_name(name: &str) -> bool {
    name.starts_with(SEED_INPUT_PREFIX)
}

/// One binding rewritten from a dropped node → kept node into a run input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedSpec {
    /// Synthetic input name (`__seed/{node}/{port}`).
    pub input_name: String,
    /// Dropped assembly node that would have produced the value.
    pub from_node: String,
    /// Producer output port.
    pub from_port: String,
    /// Kept consumer node (one of possibly many).
    pub to_node: String,
    /// Consumer input port.
    pub to_port: String,
}

/// A node excluded from the materialised assembly for this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedNode {
    /// Assembly node id.
    pub node_id: String,
    /// Module name.
    pub module: String,
    /// Capability name.
    pub capability: String,
    /// Module version pin from the assembly.
    pub module_version: String,
}

/// Result of applying a [`NodePlan`] cut to a full assembly.
#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyCut {
    /// Assembly with only kept nodes and rewritten seed bindings.
    pub assembly: Assembly,
    /// Nodes that will not execute (dropped by the cut).
    pub skipped: Vec<SkippedNode>,
    /// Unique seed inputs required (one per `(from_node, from_port)`).
    pub required_seeds: Vec<SeedSpec>,
    /// Kept assembly node ids (stable order matching original assembly).
    pub kept_node_ids: Vec<String>,
}

/// Resolve which node ids participate under `plan` (before rebinding).
pub fn resolve_kept_nodes(
    assembly: &Assembly,
    plan: &NodePlan,
) -> Result<BTreeSet<String>, FrameworkError> {
    let known: HashSet<&str> = assembly.nodes.iter().map(|n| n.id.as_str()).collect();
    if known.is_empty() {
        return Err(FrameworkError::Invalid(
            "assembly has no nodes".into(),
        ));
    }

    let terminal = assembly
        .terminal
        .as_ref()
        .ok_or_else(|| FrameworkError::Invalid("assembly terminal is required".into()))?;
    if !known.contains(terminal.node.as_str()) {
        return Err(FrameworkError::Invalid(format!(
            "terminal node {} is not in the assembly",
            terminal.node
        )));
    }

    match plan {
        NodePlan::All => Ok(assembly.nodes.iter().map(|n| n.id.clone()).collect()),
        NodePlan::Only(ids) => {
            if ids.is_empty() {
                return Err(FrameworkError::Invalid(
                    "NodePlan::Only requires at least one node id".into(),
                ));
            }
            let mut kept = BTreeSet::new();
            let mut seen = HashSet::new();
            for id in ids {
                if !seen.insert(id.as_str()) {
                    return Err(FrameworkError::Invalid(format!(
                        "NodePlan::Only contains duplicate node id {id}"
                    )));
                }
                if !known.contains(id.as_str()) {
                    return Err(FrameworkError::Invalid(format!(
                        "NodePlan::Only references unknown node {id}"
                    )));
                }
                kept.insert(id.clone());
            }
            if !kept.contains(&terminal.node) {
                return Err(FrameworkError::Invalid(
                    "node plan must retain the terminal node".into(),
                ));
            }
            Ok(kept)
        }
        NodePlan::After(node_id) => {
            if !known.contains(node_id.as_str()) {
                return Err(FrameworkError::Invalid(format!(
                    "NodePlan::After references unknown node {node_id}"
                )));
            }
            let preds = transitive_predecessors(assembly, node_id);
            let mut dropped: HashSet<&str> = preds.iter().map(String::as_str).collect();
            dropped.insert(node_id.as_str());
            if dropped.contains(terminal.node.as_str()) {
                return Err(FrameworkError::Invalid(format!(
                    "NodePlan::After({node_id}) would drop the terminal node"
                )));
            }
            let kept: BTreeSet<String> = assembly
                .nodes
                .iter()
                .filter(|n| !dropped.contains(n.id.as_str()))
                .map(|n| n.id.clone())
                .collect();
            if kept.is_empty() {
                return Err(FrameworkError::Invalid(
                    "NodePlan::After left no nodes to run".into(),
                ));
            }
            Ok(kept)
        }
        NodePlan::From(node_id) => {
            if !known.contains(node_id.as_str()) {
                return Err(FrameworkError::Invalid(format!(
                    "NodePlan::From references unknown node {node_id}"
                )));
            }
            let reachable = reachable_from(assembly, node_id);
            if !reachable.contains(terminal.node.as_str()) {
                return Err(FrameworkError::Invalid(format!(
                    "NodePlan::From({node_id}): terminal {} is not reachable from that node",
                    terminal.node
                )));
            }
            Ok(reachable.into_iter().map(str::to_string).collect())
        }
    }
}

/// Materialise a cut: drop nodes, rebind crossing edges to `__seed/…` inputs.
pub fn materialize_cut(
    assembly: &Assembly,
    plan: &NodePlan,
) -> Result<AssemblyCut, FrameworkError> {
    let kept = resolve_kept_nodes(assembly, plan)?;
    let kept_set: HashSet<&str> = kept.iter().map(String::as_str).collect();

    if matches!(plan, NodePlan::All) || kept.len() == assembly.nodes.len() {
        return Ok(AssemblyCut {
            assembly: assembly.clone(),
            skipped: Vec::new(),
            required_seeds: Vec::new(),
            kept_node_ids: assembly.nodes.iter().map(|n| n.id.clone()).collect(),
        });
    }

    let by_id: HashMap<&str, &AssemblyNode> =
        assembly.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let skipped: Vec<SkippedNode> = assembly
        .nodes
        .iter()
        .filter(|n| !kept_set.contains(n.id.as_str()))
        .map(|n| SkippedNode {
            node_id: n.id.clone(),
            module: n.module.clone(),
            capability: n.capability.clone(),
            module_version: n.module_version.clone(),
        })
        .collect();

    let mut seed_by_key: BTreeMap<(String, String), SeedSpec> = BTreeMap::new();
    let mut bindings: Vec<Binding> = Vec::new();

    for b in &assembly.bindings {
        if !kept_set.contains(b.to_node.as_str()) {
            continue;
        }
        if !b.input.is_empty() {
            // Run-input binding: keep as-is when the consumer is kept.
            bindings.push(b.clone());
            continue;
        }
        if b.from_node.is_empty() {
            return Err(FrameworkError::Invalid(format!(
                "binding to {}.{} has neither input nor from_node",
                b.to_node, b.to_port
            )));
        }
        if kept_set.contains(b.from_node.as_str()) {
            bindings.push(b.clone());
            continue;
        }
        // Crossing edge: dropped producer → kept consumer → synthetic seed input.
        let input_name = seed_input_name(&b.from_node, &b.from_port);
        let key = (b.from_node.clone(), b.from_port.clone());
        seed_by_key.entry(key).or_insert_with(|| SeedSpec {
            input_name: input_name.clone(),
            from_node: b.from_node.clone(),
            from_port: b.from_port.clone(),
            to_node: b.to_node.clone(),
            to_port: b.to_port.clone(),
        });
        bindings.push(Binding {
            to_node: b.to_node.clone(),
            to_port: b.to_port.clone(),
            from_node: String::new(),
            from_port: String::new(),
            input: input_name,
        });
    }

    let nodes: Vec<AssemblyNode> = assembly
        .nodes
        .iter()
        .filter(|n| kept_set.contains(n.id.as_str()))
        .cloned()
        .collect();

    // Sanity: every kept node still exists in by_id (always true).
    let _ = by_id;

    let kept_node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
    let required_seeds: Vec<SeedSpec> = seed_by_key.into_values().collect();

    Ok(AssemblyCut {
        assembly: Assembly {
            id: assembly.id.clone(),
            nodes,
            bindings,
            terminal: assembly.terminal.clone(),
        },
        skipped,
        required_seeds,
        kept_node_ids,
    })
}

/// Collect seed [`NamedArtifact`]s from a prior run's latest derivations.
///
/// For each node in `cut_nodes`, takes the **latest** committed derivation and
/// emits one named artifact per output port using [`seed_input_name`].
pub fn seeds_from_run(
    kernel: &impl KernelApi,
    run_id: &str,
    cut_nodes: impl IntoIterator<Item = impl AsRef<str>>,
    ctx: &RequestContext,
) -> Result<Vec<NamedArtifact>, FrameworkError> {
    let list = kernel.list_derivations(
        &RunRef {
            id: run_id.into(),
        },
        ctx,
    )?;

    let want: HashSet<String> = cut_nodes
        .into_iter()
        .map(|s| s.as_ref().to_string())
        .collect();
    if want.is_empty() {
        return Ok(Vec::new());
    }

    // Latest derivation per node_id (list order is commit order).
    let mut latest: HashMap<String, &Derivation> = HashMap::new();
    for d in &list.derivations {
        if want.contains(&d.node_id) {
            latest.insert(d.node_id.clone(), d);
        }
    }

    let mut out: Vec<NamedArtifact> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    // Stable order: sorted cut nodes then output ports.
    let mut nodes: Vec<_> = want.into_iter().collect();
    nodes.sort();
    for node_id in nodes {
        let Some(d) = latest.get(&node_id) else {
            return Err(FrameworkError::Invalid(format!(
                "seeds_from_run: no derivation for node {node_id} on run {run_id}"
            )));
        };
        for o in &d.outputs {
            if o.name.is_empty() {
                continue;
            }
            let name = seed_input_name(&node_id, &o.name);
            if !seen_names.insert(name.clone()) {
                continue;
            }
            let Some(art) = o.artifact.clone() else {
                return Err(FrameworkError::Invalid(format!(
                    "seeds_from_run: output {}.{} has no artifact ref",
                    node_id, o.name
                )));
            };
            out.push(NamedArtifact {
                name,
                artifact: Some(art),
            });
        }
    }
    Ok(out)
}

/// Merge caller inputs with seeds; seeds win on name collision (explicit seed).
pub fn merge_inputs(
    base: Vec<NamedArtifact>,
    seeds: Vec<NamedArtifact>,
) -> Vec<NamedArtifact> {
    let mut by_name: BTreeMap<String, NamedArtifact> = BTreeMap::new();
    for na in base {
        by_name.insert(na.name.clone(), na);
    }
    for na in seeds {
        by_name.insert(na.name.clone(), na);
    }
    by_name.into_values().collect()
}

/// Fail closed if any required seed input is missing or lacks an artifact ref.
pub fn validate_seeds_present(
    cut: &AssemblyCut,
    inputs: &[NamedArtifact],
) -> Result<(), FrameworkError> {
    if cut.required_seeds.is_empty() {
        return Ok(());
    }
    let have: HashMap<&str, &NamedArtifact> =
        inputs.iter().map(|i| (i.name.as_str(), i)).collect();
    let mut missing = Vec::new();
    for s in &cut.required_seeds {
        match have.get(s.input_name.as_str()) {
            None => missing.push(format!(
                "{} (from {}.{} → {}.{})",
                s.input_name, s.from_node, s.from_port, s.to_node, s.to_port
            )),
            Some(na) if na.artifact.is_none() => missing.push(format!(
                "{} (present but artifact ref is empty)",
                s.input_name
            )),
            Some(_) => {}
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(FrameworkError::Invalid(format!(
            "cut requires seed inputs that were not provided: {}",
            missing.join("; ")
        )))
    }
}

fn transitive_predecessors(assembly: &Assembly, node_id: &str) -> HashSet<String> {
    // Edge: from_node → to_node (producer → consumer).
    let mut incoming: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &assembly.bindings {
        if b.from_node.is_empty() || b.to_node.is_empty() {
            continue;
        }
        incoming
            .entry(b.to_node.as_str())
            .or_default()
            .push(b.from_node.as_str());
    }
    let mut out = HashSet::new();
    let mut q = VecDeque::new();
    q.push_back(node_id);
    let mut visited = HashSet::new();
    visited.insert(node_id);
    while let Some(cur) = q.pop_front() {
        let Some(preds) = incoming.get(cur) else {
            continue;
        };
        for p in preds {
            if out.insert((*p).to_string()) && visited.insert(*p) {
                q.push_back(*p);
            }
        }
    }
    out
}

fn reachable_from<'a>(assembly: &'a Assembly, node_id: &'a str) -> HashSet<&'a str> {
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &assembly.bindings {
        if b.from_node.is_empty() || b.to_node.is_empty() {
            continue;
        }
        outgoing
            .entry(b.from_node.as_str())
            .or_default()
            .push(b.to_node.as_str());
    }
    let mut out = HashSet::new();
    let mut q = VecDeque::new();
    q.push_back(node_id);
    out.insert(node_id);
    while let Some(cur) = q.pop_front() {
        let Some(nexts) = outgoing.get(cur) else {
            continue;
        };
        for n in nexts {
            if out.insert(*n) {
                q.push_back(*n);
            }
        }
    }
    out
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use srcport_substrate::{AssemblyNode, NodeOutput};

    fn diamond() -> Assembly {
        Assembly {
            id: "d@1".into(),
            nodes: vec![
                node("extract", "extractor", "facts.extract"),
                node("retrieve", "retriever", "sources.retrieve"),
                node("write", "writer", "answer.write"),
            ],
            bindings: vec![
                Binding {
                    to_node: "extract".into(),
                    to_port: "question".into(),
                    input: "question".into(),
                    ..Default::default()
                },
                Binding {
                    to_node: "retrieve".into(),
                    to_port: "question".into(),
                    input: "question".into(),
                    ..Default::default()
                },
                Binding {
                    to_node: "write".into(),
                    to_port: "question".into(),
                    input: "question".into(),
                    ..Default::default()
                },
                Binding {
                    to_node: "write".into(),
                    to_port: "facts".into(),
                    from_node: "extract".into(),
                    from_port: "facts".into(),
                    ..Default::default()
                },
                Binding {
                    to_node: "write".into(),
                    to_port: "sources".into(),
                    from_node: "retrieve".into(),
                    from_port: "sources".into(),
                    ..Default::default()
                },
            ],
            terminal: Some(NodeOutput {
                node: "write".into(),
                port: "answer".into(),
            }),
        }
    }

    fn node(id: &str, module: &str, cap: &str) -> AssemblyNode {
        AssemblyNode {
            id: id.into(),
            module: module.into(),
            module_version: "1.0.0".into(),
            capability: cap.into(),
        }
    }

    #[test]
    fn after_extract_keeps_parallel_branch_and_seeds_facts() {
        let a = diamond();
        let cut = materialize_cut(&a, &NodePlan::After("extract".into())).unwrap();
        assert_eq!(cut.kept_node_ids, vec!["retrieve", "write"]);
        assert_eq!(cut.skipped.len(), 1);
        assert_eq!(cut.skipped[0].node_id, "extract");
        assert_eq!(cut.required_seeds.len(), 1);
        assert_eq!(cut.required_seeds[0].input_name, "__seed/extract/facts");
        assert!(cut
            .assembly
            .bindings
            .iter()
            .any(|b| b.to_port == "facts" && b.input == "__seed/extract/facts"));
    }

    #[test]
    fn from_write_seeds_both_producers() {
        let a = diamond();
        let cut = materialize_cut(&a, &NodePlan::From("write".into())).unwrap();
        assert_eq!(cut.kept_node_ids, vec!["write"]);
        assert_eq!(cut.skipped.len(), 2);
        let names: BTreeSet<_> = cut
            .required_seeds
            .iter()
            .map(|s| s.input_name.as_str())
            .collect();
        assert!(names.contains("__seed/extract/facts"));
        assert!(names.contains("__seed/retrieve/sources"));
    }

    #[test]
    fn after_terminal_is_rejected() {
        let a = diamond();
        let err = materialize_cut(&a, &NodePlan::After("write".into())).unwrap_err();
        assert!(err.to_string().contains("terminal"));
    }
}
