//! # flow — a real Run, visualised from the ledger alone
//!
//! This example does three things, in order:
//!
//! 1. **Builds a tiny domain on the kernel.** Three Modules with typed
//!    capability ports form a diamond: a `question` fans out to an `extractor`
//!    and a `retriever`, and a `writer` fans *in* from both plus the original
//!    question to produce the terminal `answer`. The kernel knows none of these
//!    words — they are just Modules coupling through contract refs.
//!
//! 2. **Drives a real convergent Run.** We `claim_ready → put_artifact →
//!    commit`, printing a live trace. The writer is provably *not* claimable
//!    until every one of its typed inputs exists — that gate is the whole point
//!    of feed-forward convergence, and you can watch it hold.
//!
//! 3. **Reconstructs the dataflow from the tamper-evident chain — nothing
//!    else.** We throw away every in-memory handle, verify `k.ledger()`, and
//!    rebuild the entire graph by *decoding the ledger entries*. The picture you
//!    get out — terminal and `flow.html` — is proof of the spec's central
//!    claim: artifact refs are the data plane, and the chain records exactly
//!    what happened, reconstructably and tamper-evidently.
//!
//! Run it: `cargo run -p flow-example` (writes `flow.html` beside you).

use srcport_substrate::*;

mod html;
mod reconstruct;

use reconstruct::Graph;

// ── styling helpers for the terminal trace ──────────────────────────────────
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const MAGENTA: &str = "\x1b[35m";
const RESET: &str = "\x1b[0m";

/// Truncate a `sha256:…` id to something a human can scan.
fn short(id: &str) -> String {
    let body = id.strip_prefix("sha256:").unwrap_or(id);
    format!("sha256:{}…", &body[..body.len().min(8)])
}

fn rule(title: &str) {
    println!("\n{BOLD}{CYAN}── {title} {}{RESET}", "─".repeat(60usize.saturating_sub(title.len())));
}

fn main() {
    // MemoryKernel is one in-process implementation of KernelApi — durability
    // lives in Modules (or other backends), not the core.
    let k = MemoryKernel::new();

    // RequestContext rides as call metadata (not ledger detail). Demonstrate
    // the ABI surface once; remaining calls use the ctx-free inherent methods
    // so call sites stay zero-churn.
    let ctx = RequestContext {
        caller: "flow-example".into(),
        correlation_id: "demo-flow".into(),
        ..Default::default()
    };

    // ── 1. THE DOMAIN — three modules, typed ports, coupled only by contract ─
    rule("1. register the domain (Modules + typed Capability ports)");

    // extractor:  question ─▶ facts  (via KernelApi — shows RequestContext)
    KernelApi::register(
        &k,
        ModuleManifest {
            name: "extractor".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "facts.extract".into(),
                contract: "demo.v1.Extract".into(),
                inputs: vec![port("question", "demo.v1.Question")],
                outputs: vec![port("facts", "demo.v1.Facts")],
            }],
            ..Default::default()
        },
        &ctx,
    );
    // retriever:  question ─▶ sources
    k.register(ModuleManifest {
        name: "retriever".into(),
        version: "1.0.0".into(),
        provides: vec![Capability {
            name: "sources.retrieve".into(),
            contract: "demo.v1.Retrieve".into(),
            inputs: vec![port("question", "demo.v1.Question")],
            outputs: vec![port("sources", "demo.v1.Sources")],
        }],
        ..Default::default()
    });
    // writer:  (question, facts, sources) ─▶ answer   ← the fan-in
    k.register(ModuleManifest {
        name: "writer".into(),
        version: "2.0.0".into(),
        provides: vec![Capability {
            name: "answer.write".into(),
            contract: "demo.v1.Write".into(),
            inputs: vec![
                port("question", "demo.v1.Question"),
                port("facts", "demo.v1.Facts"),
                port("sources", "demo.v1.Sources"),
            ],
            outputs: vec![port("answer", "demo.v1.Answer")],
        }],
        ..Default::default()
    });

    for m in k.snapshot().modules {
        let caps: Vec<_> = m.provides.iter().map(|c| c.name.clone()).collect();
        println!("  {GREEN}✓{RESET} {BOLD}{}@{}{RESET}  provides {}", m.name, m.version, caps.join(", "));
    }

    // ── the Assembly — a human pins versions, binds typed ports, names one out ─
    rule("2. pin the Assembly (feed-forward graph, one terminal output)");
    let assembly = Assembly {
        id: "answer-pipeline@1".into(),
        nodes: vec![
            node("extract", "extractor", "1.0.0", "facts.extract"),
            node("retrieve", "retriever", "1.0.0", "sources.retrieve"),
            node("write", "writer", "2.0.0", "answer.write"),
        ],
        bindings: vec![
            // the question fans out to all three nodes …
            from_input("extract", "question", "question"),
            from_input("retrieve", "question", "question"),
            from_input("write", "question", "question"),
            // … and the writer fans in from the two producers
            from_node("write", "facts", "extract", "facts"),
            from_node("write", "sources", "retrieve", "sources"),
        ],
        terminal: Some(NodeOutput { node: "write".into(), port: "answer".into() }),
    };
    println!("  {DIM}nodes:{RESET} extract, retrieve, write   {DIM}terminal:{RESET} write.answer");
    println!("  {DIM}the kernel will reject cycles, unbound inputs, and contract mismatches at start_run{RESET}");

    // ── 3. THE RUN — freeze the assembly over an immutable input artifact ────
    rule("3. start the Run (freeze the assembly over an input Artifact)");
    let question = k.put_artifact(Artifact {
        r#type: "demo.v1.Question".into(),
        body: b"What makes this substrate reusable?".to_vec(),
        produced_by: "operator".into(),
        ..Default::default()
    }).unwrap();
    println!("  {DIM}input{RESET} question = {MAGENTA}{}{RESET}  {DIM}\"What makes this substrate reusable?\"{RESET}", short(&question.id));

    k.start_run(RunRequest {
        id: "run-1".into(),
        assembly: Some(assembly),
        inputs: vec![named("question", &question)],
        ..Default::default()
    })
    .expect("assembly is valid");
    println!("  {GREEN}✓{RESET} run-1 {BOLD}RUNNING{RESET}");

    // ── drive it, narrating the dataflow as artifacts appear ────────────────
    rule("4. execute — watch typed artifacts feed forward");

    // The writer cannot be claimed yet: its `facts` and `sources` inputs do not
    // exist. This is convergence, observable.
    let blocked = k
        .claim_ready(ClaimRequest { run_id: "run-1".into(), module: "writer".into() })
        .unwrap();
    assert!(blocked.id.is_empty(), "writer must be blocked on its unmet inputs");
    println!("  {YELLOW}⏳ writer is NOT ready{RESET} {DIM}— fan-in still waiting on facts + sources{RESET}");

    run_node(&k, "extractor", "facts", "demo.v1.Facts", b"one canonical contract, many conforming implementations");
    run_node(&k, "retriever", "sources", "demo.v1.Sources", b"SPEC.md#the-eight-primitives");

    // Now all three of the writer's inputs exist; it becomes claimable and closes the run.
    let write = k
        .claim_ready(ClaimRequest { run_id: "run-1".into(), module: "writer".into() })
        .unwrap();
    println!(
        "  {GREEN}▶ writer READY{RESET} {DIM}— fan-in supplied {} typed inputs{RESET}",
        write.inputs.len()
    );
    let answer = k.put_artifact(Artifact {
        r#type: "demo.v1.Answer".into(),
        body: b"One versioned contract; every project is just Modules on top.".to_vec(),
        produced_by: "writer".into(),
        derived_from: write.inputs.iter().filter_map(|i| i.artifact.as_ref().map(|a| a.id.clone())).collect(),
        ..Default::default()
    }).unwrap();
    let run = k
        .commit(Derivation {
            run_id: "run-1".into(),
            work_id: write.id,
            node_id: write.node_id,
            outputs: vec![named("answer", &answer)],
            ..Default::default()
        })
        .unwrap();
    println!(
        "  {GREEN}✓ run-1 {}{RESET}  terminal answer = {MAGENTA}{}{RESET}",
        run_state_word(run.state()),
        short(&run.answer.as_ref().unwrap().id)
    );

    // ── 5. RECONSTRUCT FROM THE CHAIN ALONE — the point of the whole exercise ─
    rule("5. reconstruct the flow from the ledger — no in-memory handles");
    let chain = k.ledger();
    let verified = verify_chain(&chain);
    println!(
        "  {} chain verifies: {BOLD}{}{RESET} {DIM}({} entries, tamper-evident){RESET}",
        if verified { format!("{GREEN}✓{RESET}") } else { format!("\x1b[31m✗{RESET}") },
        verified,
        chain.len()
    );
    println!("  {DIM}decoding every entry back into its substrate.proto message …{RESET}\n");

    // Everything below is rebuilt SOLELY by decoding `chain`. We never touch the
    // live kernel state again — this is what an auditor or a cold agent sees.
    let graph = Graph::from_ledger(&chain);
    graph.print_terminal();
    graph.print_ledger(&chain);

    // ── 6. EMIT THE VISUAL ──────────────────────────────────────────────────
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("flow.html");
    let doc = html::render(&graph, &chain, verified);
    std::fs::write(&out, doc).expect("write flow.html");
    rule("6. wrote a self-contained visualisation");
    println!("  {GREEN}✓{RESET} {}", out.display());
    println!("  {DIM}open it in a browser — the diagram is reconstructed from the chain above{RESET}\n");
}

/// Claim → produce → commit one single-output node, narrating the flow.
fn run_node(k: &MemoryKernel, module: &str, out_port: &str, contract: &str, body: &[u8]) {
    let work = k
        .claim_ready(ClaimRequest { run_id: "run-1".into(), module: module.into() })
        .unwrap();
    assert!(!work.id.is_empty(), "{module} should have a ready node");
    let inputs: Vec<_> = work.inputs.iter().map(|i| i.name.clone()).collect();
    let artifact = k.put_artifact(Artifact {
        r#type: contract.into(),
        body: body.to_vec(),
        produced_by: module.into(),
        derived_from: work.inputs.iter().filter_map(|i| i.artifact.as_ref().map(|a| a.id.clone())).collect(),
        ..Default::default()
    }).unwrap();
    k.commit(Derivation {
        run_id: "run-1".into(),
        work_id: work.id,
        node_id: work.node_id.clone(),
        outputs: vec![named(out_port, &artifact)],
        ..Default::default()
    })
    .unwrap();
    println!(
        "  {GREEN}✓ [{}]{RESET} {CYAN}{}{RESET}  in({}) ─▶ {out_port} = {MAGENTA}{}{RESET}",
        work.node_id,
        module,
        inputs.join(", "),
        short(&artifact.id)
    );
}

// ── tiny constructors so the domain above reads like a diagram ───────────────

fn port(name: &str, contract: &str) -> Port {
    Port { name: name.into(), contract: contract.into(), ..Default::default() }
}

fn node(id: &str, module: &str, version: &str, capability: &str) -> AssemblyNode {
    AssemblyNode {
        id: id.into(),
        module: module.into(),
        module_version: version.into(),
        capability: capability.into(),
    }
}

fn from_input(to_node: &str, to_port: &str, input: &str) -> Binding {
    Binding {
        to_node: to_node.into(),
        to_port: to_port.into(),
        input: input.into(),
        ..Default::default()
    }
}

fn from_node(to_node: &str, to_port: &str, from_node: &str, from_port: &str) -> Binding {
    Binding {
        to_node: to_node.into(),
        to_port: to_port.into(),
        from_node: from_node.into(),
        from_port: from_port.into(),
        ..Default::default()
    }
}

fn named(name: &str, r: &ArtifactRef) -> NamedArtifact {
    NamedArtifact { name: name.into(), artifact: Some(r.clone()) }
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
