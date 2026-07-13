//! Rebuild the dataflow graph from a ledger chain — and nothing else.
//!
//! Every field below comes from *decoding* [`LedgerEntry::detail`] back into the
//! `substrate.proto` message named for its `kind` (see SPEC.md "Ledger detail"):
//!
//! - `artifact.put`        → [`Artifact`] (body cleared; `type` + id survive)
//! - `derivation.committed`→ [`Derivation`] (a node's typed inputs and outputs)
//! - `run.*`               → [`Run`] (final state and terminal answer)
//!
//! From the derivations we recover which node produced which artifact, and thus
//! every edge; from the artifact entries we recover each edge's contract type.
//! That the picture reconstructs at all is the invariant this example exists to
//! demonstrate.

use std::collections::BTreeMap;

use srcport_substrate::*;

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const MAGENTA: &str = "\x1b[35m";
const RESET: &str = "\x1b[0m";

fn short(id: &str) -> String {
    let body = id.strip_prefix("sha256:").unwrap_or(id);
    format!("sha256:{}…", &body[..body.len().min(8)])
}

/// One typed port instance on a reconstructed node: the port name, the artifact
/// that flowed through it, and that artifact's contract type.
pub struct Slot {
    pub port: String,
    pub artifact: String,
    pub contract: String,
}

/// One executed node, recovered from its committed [`Derivation`].
pub struct Node {
    pub key: String,
    pub module: String,
    pub version: String,
    pub capability: String,
    pub inputs: Vec<Slot>,
    pub outputs: Vec<Slot>,
    /// Layout column: 0 = external inputs, nodes start at 1.
    pub layer: usize,
}

/// One artifact flowing into a node. `from` is the producing node, or `None`
/// when the artifact is an external run input.
pub struct Edge {
    pub from: Option<String>,
    pub to: String,
    pub artifact: String,
    pub contract: String,
}

/// The whole flow, rebuilt from the chain.
pub struct Graph {
    pub run_id: String,
    pub run_state: String,
    pub answer: Option<String>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Distinct external input artifacts: (contract, artifact id).
    pub externals: Vec<(String, String)>,
    pub max_layer: usize,
}

impl Graph {
    /// Decode a ledger chain into a dataflow graph. Reads only the chain.
    pub fn from_ledger(chain: &[LedgerEntry]) -> Graph {
        // artifact id → contract type, from every `artifact.put` entry.
        let mut types: BTreeMap<String, String> = BTreeMap::new();
        // node key → decoded derivation.
        let mut derivations: Vec<Derivation> = Vec::new();
        let mut run_id = String::new();
        let mut run_state = "RUNNING".to_string();
        let mut answer = None;

        for e in chain {
            match e.kind.as_str() {
                "artifact.put" => {
                    if let Ok(a) = Artifact::decode(&e.detail[..]) {
                        types.insert(e.subject.clone(), a.r#type);
                    }
                }
                "derivation.committed" => {
                    if let Ok(d) = Derivation::decode(&e.detail[..]) {
                        derivations.push(d);
                    }
                }
                k if k.starts_with("run.") => {
                    if let Ok(r) = Run::decode(&e.detail[..]) {
                        run_state = run_state_word(r.state()).to_string();
                        run_id = r.id;
                        answer = r.answer.map(|a| a.id);
                    }
                }
                _ => {}
            }
        }

        let contract_of = |id: &str| types.get(id).cloned().unwrap_or_else(|| "?".into());

        // producer: which node output each artifact id.
        let mut producer: BTreeMap<String, String> = BTreeMap::new();
        for d in &derivations {
            for o in &d.outputs {
                if let Some(r) = &o.artifact {
                    producer.insert(r.id.clone(), d.node_id.clone());
                }
            }
        }

        let mut nodes: Vec<Node> = derivations
            .iter()
            .map(|d| Node {
                key: d.node_id.clone(),
                module: d.module.clone(),
                version: d.module_version.clone(),
                capability: d.capability.clone(),
                inputs: d.inputs.iter().map(|i| slot(i, &contract_of)).collect(),
                outputs: d.outputs.iter().map(|o| slot(o, &contract_of)).collect(),
                layer: 0,
            })
            .collect();

        // edges, and the distinct external inputs.
        let mut edges = Vec::new();
        let mut externals: Vec<(String, String)> = Vec::new();
        for d in &derivations {
            for i in &d.inputs {
                if let Some(r) = &i.artifact {
                    let from = producer.get(&r.id).cloned();
                    if from.is_none() && !externals.iter().any(|(_, id)| id == &r.id) {
                        externals.push((contract_of(&r.id), r.id.clone()));
                    }
                    edges.push(Edge {
                        from,
                        to: d.node_id.clone(),
                        artifact: r.id.clone(),
                        contract: contract_of(&r.id),
                    });
                }
            }
        }

        // Layer assignment: external inputs sit at 0; a node sits one past the
        // deepest node feeding it. Iterate to a fixpoint (the graph is a DAG, so
        // this converges in ≤ nodes.len() passes).
        let key_index: BTreeMap<String, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.key.clone(), i)).collect();
        for _ in 0..nodes.len() {
            for i in 0..nodes.len() {
                let mut layer = 1;
                for e in edges.iter().filter(|e| e.to == nodes[i].key) {
                    if let Some(src) = &e.from {
                        if let Some(&j) = key_index.get(src) {
                            layer = layer.max(nodes[j].layer + 1);
                        }
                    }
                }
                nodes[i].layer = layer;
            }
        }
        nodes.sort_by(|a, b| a.layer.cmp(&b.layer).then(a.key.cmp(&b.key)));
        let max_layer = nodes.iter().map(|n| n.layer).max().unwrap_or(0);

        Graph { run_id, run_state, answer, nodes, edges, externals, max_layer }
    }

    /// Print the reconstructed dataflow as a readable, topologically-ordered
    /// listing — each node, its typed inputs (and where each came from) and its
    /// typed outputs.
    pub fn print_terminal(&self) {
        println!("  {BOLD}run {} — {}{RESET}\n", self.run_id, self.run_state);
        for (contract, id) in &self.externals {
            println!("  {DIM}○ external input{RESET}  {contract}  {MAGENTA}{}{RESET}", short(id));
        }
        for n in &self.nodes {
            println!(
                "\n  {BOLD}┌ {}{RESET}  {DIM}{}@{} · {}{RESET}",
                n.key, n.module, n.version, n.capability
            );
            for s in &n.inputs {
                let src = self
                    .edges
                    .iter()
                    .find(|e| e.to == n.key && e.artifact == s.artifact)
                    .and_then(|e| e.from.clone());
                let origin = match src {
                    Some(node) => format!("◀ from {CYAN}{node}{RESET}"),
                    None => format!("◀ {DIM}external{RESET}"),
                };
                println!(
                    "  │   {origin}  {DIM}{}{RESET} {}  {MAGENTA}{}{RESET}",
                    s.contract, s.port, short(&s.artifact)
                );
            }
            for s in &n.outputs {
                let terminal = self.answer.as_deref() == Some(s.artifact.as_str());
                let mark = if terminal { "▶▶ terminal" } else { "▶ produces " };
                println!(
                    "  └   {mark}  {DIM}{}{RESET} {}  {MAGENTA}{}{RESET}",
                    s.contract, s.port, short(&s.artifact)
                );
            }
        }
    }

    /// Print the tamper-evident chain itself: every entry, hash-linked.
    pub fn print_ledger(&self, chain: &[LedgerEntry]) {
        println!("\n  {BOLD}the ledger the picture was rebuilt from:{RESET}");
        for e in chain {
            let subject = if e.subject.starts_with("sha256:") {
                short(&e.subject)
            } else {
                e.subject.clone()
            };
            println!(
                "  {DIM}#{:<2}{RESET} {CYAN}{:<22}{RESET} {:<34} {DIM}{}{RESET}",
                e.seq,
                e.kind,
                subject,
                short_hash(&e.hash)
            );
        }
    }
}

fn slot<F: Fn(&str) -> String>(na: &NamedArtifact, contract_of: &F) -> Slot {
    let artifact = na.artifact.as_ref().map(|r| r.id.clone()).unwrap_or_default();
    Slot {
        port: na.name.clone(),
        contract: contract_of(&artifact),
        artifact,
    }
}

fn short_hash(h: &str) -> String {
    format!("{}…", &h[..h.len().min(12)])
}

fn run_state_word(s: RunState) -> &'static str {
    match s {
        RunState::Completed => "COMPLETED",
        RunState::Stalled => "STALLED",
        RunState::Failed => "FAILED",
        RunState::Cancelled => "CANCELLED",
        _ => "RUNNING",
    }
}
