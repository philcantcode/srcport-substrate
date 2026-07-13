//! Framework modes: converge, stream (loop on inject), selective, start_after / from_node cut+seed.

use std::sync::atomic::{AtomicU64, Ordering};

use srcport_framework::{
    seed_input_name, seeds_from_run, DriveAfter, FrameworkError, FrameworkPolicy, Host,
    ModulePlugin, PortBody, PresentationStatus, StepContext, StepOutput, StepStage,
};
use srcport_substrate::{
    artifact_with_trait, Assembly, AssemblyNode, Binding, Capability, Firing, MemoryKernel,
    ModuleManifest, NamedArtifact, NodeOutput, Port, RequestContext, RunState,
};

struct Echo {
    hits: AtomicU64,
}

impl ModulePlugin for Echo {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "echo".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "echo.run".into(),
                inputs: vec![Port {
                    name: "in".into(),
                    traits: vec!["demo.v1.In".into()],
                    ..Default::default()
                }],
                outputs: vec![Port {
                    name: "out".into(),
                    traits: vec!["demo.v1.Out".into()],
                    ..Default::default()
                }],
                // Capability default is ONCE; stream policy overrides to ALWAYS.
                firing: Firing::Once as i32,
            }],
            ..Default::default()
        }
    }

    fn execute(&self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        let body = step
            .inputs
            .get("in")
            .and_then(|a| a.traits.values().next().map(|f| f.body.clone()))
            .unwrap_or_default();
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("out", "demo.v1.Out", body)],
        })
    }
}

struct Extractor;
struct Retriever;
struct Writer {
    /// When true, writer requires sources port (full diamond).
    with_sources: bool,
}

impl ModulePlugin for Extractor {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "extractor".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "facts.extract".into(),
                inputs: vec![port("question", "demo.v1.Question")],
                outputs: vec![port("facts", "demo.v1.Facts")],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn execute(&self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        let q = step.inputs.get("question").and_then(|a| a.traits.values().next().map(|f| f.body.as_slice())).unwrap_or(b"");
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait(
                "facts",
                "demo.v1.Facts",
                format!("facts:{}", String::from_utf8_lossy(q)).into_bytes(),
            )],
        })
    }
}

impl ModulePlugin for Retriever {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "retriever".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "sources.retrieve".into(),
                inputs: vec![port("question", "demo.v1.Question")],
                outputs: vec![port("sources", "demo.v1.Sources")],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn execute(&self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        let q = step
            .inputs
            .get("question")
            .and_then(|a| a.traits.values().next().map(|f| f.body.as_slice()))
            .unwrap_or(b"");
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait(
                "sources",
                "demo.v1.Sources",
                format!("src:{}", String::from_utf8_lossy(q)).into_bytes(),
            )],
        })
    }
}

impl ModulePlugin for Writer {
    fn manifest(&self) -> ModuleManifest {
        let mut inputs = vec![
            port("question", "demo.v1.Question"),
            port("facts", "demo.v1.Facts"),
        ];
        if self.with_sources {
            inputs.push(port("sources", "demo.v1.Sources"));
        }
        ModuleManifest {
            name: "writer".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "answer.write".into(),
                inputs,
                outputs: vec![port("answer", "demo.v1.Answer")],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn execute(&self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        let mut body = b"answer:".to_vec();
        if let Some(f) = step.inputs.get("facts") {
            if let Some(tr) = f.traits.values().next() {
                body.extend_from_slice(&tr.body);
            }
        }
        if let Some(s) = step.inputs.get("sources") {
            if let Some(tr) = s.traits.values().next() {
                body.push(b'+');
                body.extend_from_slice(&tr.body);
            }
        }
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("answer", "demo.v1.Answer", body)],
        })
    }
}

fn port(name: &str, contract: &str) -> Port {
    Port {
        name: name.into(),
        traits: vec![contract.into()],
        ..Default::default()
    }
}

fn put_in(host: &Host<MemoryKernel>, body: &[u8]) -> srcport_substrate::ArtifactRef {
    host.kernel()
        .put_artifact({ let mut __a = artifact_with_trait("demo.v1.In", body.to_vec()); __a.produced_by = "op".into(); __a })
        .unwrap()
}

#[test]
fn stream_mode_loops_on_inject() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Echo { hits: AtomicU64::new(0) }).unwrap();

    let first = put_in(&host, b"one");
    host.start_pipeline(
        "stream-1",
        Assembly {
            id: "echo@1".into(),
            nodes: vec![AssemblyNode {
                id: "echo".into(),
                module: "echo".into(),
                module_version: "1.0.0".into(),
                capability: "echo.run".into(),
            }],
            bindings: vec![Binding {
                to_node: "echo".into(),
                to_port: "in".into(),
                input: "in".into(),
                ..Default::default()
            }],
            terminal: Some(NodeOutput {
                node: "echo".into(),
                port: "out".into(),
            }),
        },
        vec![NamedArtifact {
            name: "in".into(),
            artifact: Some(first),
        }],
        FrameworkPolicy::stream().with_max_steps(100),
    )
    .unwrap();

    let run = host.drive("stream-1").unwrap();
    assert_eq!(run.state(), RunState::Running, "OPEN stream stays running");
    assert!(run.answer.is_some());
    assert_eq!(run.steps, 1);

    let second = put_in(&host, b"two");
    let run = host
        .inject(
            "stream-1",
            NamedArtifact {
                name: "in".into(),
                artifact: Some(second),
            },
            DriveAfter::UntilIdle,
        )
        .unwrap();
    assert_eq!(run.state(), RunState::Running);
    assert_eq!(run.steps, 2, "ALWAYS re-fires after inject");

    let run = host.cancel("stream-1").unwrap();
    assert_eq!(run.state(), RunState::Cancelled);
    assert!(host.policy("stream-1").is_none());
}

#[test]
fn selective_mode_runs_subset_assembly() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Extractor).unwrap();
    host.register_plugin(Writer {
        with_sources: false,
    })
    .unwrap();
    // retriever deliberately not registered / not in include_nodes

    let question = host
        .kernel()
        .put_artifact({ let mut __a = artifact_with_trait("demo.v1.Question", b"q".to_vec()); __a.produced_by = "op".into(); __a })
        .unwrap();

    // Full diamond-shaped assembly textually includes a retrieve node we drop via selective.
    let assembly = Assembly {
        id: "sel@1".into(),
        nodes: vec![
            AssemblyNode {
                id: "extract".into(),
                module: "extractor".into(),
                module_version: "1.0.0".into(),
                capability: "facts.extract".into(),
            },
            AssemblyNode {
                id: "retrieve".into(),
                module: "retriever".into(),
                module_version: "1.0.0".into(),
                capability: "sources.retrieve".into(),
            },
            AssemblyNode {
                id: "write".into(),
                module: "writer".into(),
                module_version: "1.0.0".into(),
                capability: "answer.write".into(),
            },
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
            // retrieve → write omitted from selective subset; writer does not need sources
        ],
        terminal: Some(NodeOutput {
            node: "write".into(),
            port: "answer".into(),
        }),
    };

    host.start_pipeline(
        "sel-1",
        assembly,
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(question),
        }],
        FrameworkPolicy::selective(["extract", "write"]),
    )
    .unwrap();

    let events = host.take_step_events();
    assert!(
        events.iter().any(|e| {
            e.stage == StepStage::Skipped && e.presentation.node_id == "retrieve"
        }),
        "dropped retrieve should emit Skipped: {events:?}"
    );

    let run = host.drive("sel-1").unwrap();
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(run.steps, 2);
    assert!(run.answer.is_some());
}

fn full_diamond() -> Assembly {
    Assembly {
        id: "diamond@1".into(),
        nodes: vec![
            AssemblyNode {
                id: "extract".into(),
                module: "extractor".into(),
                module_version: "1.0.0".into(),
                capability: "facts.extract".into(),
            },
            AssemblyNode {
                id: "retrieve".into(),
                module: "retriever".into(),
                module_version: "1.0.0".into(),
                capability: "sources.retrieve".into(),
            },
            AssemblyNode {
                id: "write".into(),
                module: "writer".into(),
                module_version: "1.0.0".into(),
                capability: "answer.write".into(),
            },
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

fn put_question(host: &Host<MemoryKernel>, body: &[u8]) -> srcport_substrate::ArtifactRef {
    host.kernel()
        .put_artifact({
            let mut a = artifact_with_trait("demo.v1.Question", body.to_vec());
            a.produced_by = "op".into();
            a
        })
        .unwrap()
}

#[test]
fn start_after_requires_seed_and_runs_rest() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Extractor).unwrap();
    host.register_plugin(Retriever).unwrap();
    host.register_plugin(Writer {
        with_sources: true,
    })
    .unwrap();

    let question = put_question(&host, b"q");
    let facts = host
        .kernel()
        .put_artifact({
            let mut a = artifact_with_trait("demo.v1.Facts", b"facts:hand".to_vec());
            a.produced_by = "fixture".into();
            a
        })
        .unwrap();

    // Missing seed → fail closed.
    let err = host
        .start_pipeline(
            "after-missing",
            full_diamond(),
            vec![NamedArtifact {
                name: "question".into(),
                artifact: Some(question.clone()),
            }],
            FrameworkPolicy::start_after("extract"),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("__seed/extract/facts"),
        "expected seed error, got {err}"
    );

    host.start_pipeline(
        "after-1",
        full_diamond(),
        vec![
            NamedArtifact {
                name: "question".into(),
                artifact: Some(question),
            },
            NamedArtifact {
                name: seed_input_name("extract", "facts"),
                artifact: Some(facts),
            },
        ],
        FrameworkPolicy::start_after("extract"),
    )
    .unwrap();

    let events = host.take_step_events();
    let skipped: Vec<_> = events
        .iter()
        .filter(|e| e.stage == StepStage::Skipped)
        .collect();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].presentation.node_id, "extract");
    assert_eq!(
        skipped[0].presentation.status,
        PresentationStatus::Empty
    );

    let run = host.drive("after-1").unwrap();
    assert_eq!(run.state(), RunState::Completed);
    // retrieve + write only
    assert_eq!(run.steps, 2);
    assert!(run.answer.is_some());

    let answer = host
        .kernel()
        .get_artifact(run.answer.as_ref().unwrap())
        .unwrap();
    let body = answer.traits.values().next().unwrap().body.clone();
    assert_eq!(
        String::from_utf8_lossy(&body),
        "answer:facts:hand+src:q"
    );
}

#[test]
fn from_node_write_seeds_both_producers() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Writer {
        with_sources: true,
    })
    .unwrap();

    let question = put_question(&host, b"q");
    let facts = host
        .kernel()
        .put_artifact({
            let mut a = artifact_with_trait("demo.v1.Facts", b"F".to_vec());
            a.produced_by = "f".into();
            a
        })
        .unwrap();
    let sources = host
        .kernel()
        .put_artifact({
            let mut a = artifact_with_trait("demo.v1.Sources", b"S".to_vec());
            a.produced_by = "s".into();
            a
        })
        .unwrap();

    host.start_pipeline(
        "from-1",
        full_diamond(),
        vec![
            NamedArtifact {
                name: "question".into(),
                artifact: Some(question),
            },
            NamedArtifact {
                name: seed_input_name("extract", "facts"),
                artifact: Some(facts),
            },
            NamedArtifact {
                name: seed_input_name("retrieve", "sources"),
                artifact: Some(sources),
            },
        ],
        FrameworkPolicy::from_node("write"),
    )
    .unwrap();

    let events = host.take_step_events();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.stage == StepStage::Skipped)
            .count(),
        2
    );

    let run = host.drive("from-1").unwrap();
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(run.steps, 1);
    let answer = host
        .kernel()
        .get_artifact(run.answer.as_ref().unwrap())
        .unwrap();
    let body = answer.traits.values().next().unwrap().body.clone();
    assert_eq!(String::from_utf8_lossy(&body), "answer:F+S");
}

#[test]
fn resume_after_seeds_from_prior_run() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Extractor).unwrap();
    host.register_plugin(Retriever).unwrap();
    host.register_plugin(Writer {
        with_sources: true,
    })
    .unwrap();

    let question = put_question(&host, b"hello");
    let assembly = full_diamond();

    host.start_pipeline(
        "full-1",
        assembly.clone(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(question),
        }],
        FrameworkPolicy::converge(),
    )
    .unwrap();
    let full = host.drive("full-1").unwrap();
    assert_eq!(full.state(), RunState::Completed);
    assert_eq!(full.steps, 3);

    // Simulate "re-run from after extract": only retrieve + write, facts from prior.
    host.resume_after(
        "resume-1",
        "full-1",
        "extract",
        FrameworkPolicy::start_after("extract"),
    )
    .unwrap();

    let skipped = host
        .take_step_events()
        .into_iter()
        .filter(|e| e.stage == StepStage::Skipped)
        .count();
    assert_eq!(skipped, 1);

    let run = host.drive("resume-1").unwrap();
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(run.steps, 2);

    let answer = host
        .kernel()
        .get_artifact(run.answer.as_ref().unwrap())
        .unwrap();
    let body = answer.traits.values().next().unwrap().body.clone();
    assert_eq!(
        String::from_utf8_lossy(&body),
        "answer:facts:hello+src:hello"
    );

    // seeds_from_run helper is what resume uses under the hood.
    let seeds = seeds_from_run(
        host.kernel(),
        "full-1",
        ["extract"],
        &RequestContext::default(),
    )
    .unwrap();
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].name, seed_input_name("extract", "facts"));
}

#[test]
fn policy_compiles_stream_to_open_always() {
    let assembly = Assembly {
        id: "a".into(),
        nodes: vec![AssemblyNode {
            id: "echo".into(),
            module: "echo".into(),
            module_version: "1".into(),
            capability: "echo.run".into(),
        }],
        ..Default::default()
    };
    let p = FrameworkPolicy::stream();
    let ep = p.execution_policy_for(Some(&assembly));
    assert_eq!(ep.closure(), srcport_substrate::Closure::Open);
    assert_eq!(ep.default(), Firing::Always);
    assert_eq!(ep.by_node.get("echo").copied(), Some(Firing::Always as i32));

    let c = FrameworkPolicy::converge().execution_policy();
    assert_eq!(c.closure(), srcport_substrate::Closure::FirstTerminal);
    assert_eq!(c.default(), Firing::Unspecified);

    let d = FrameworkPolicy::stream_dedupe().execution_policy();
    assert_eq!(d.default(), Firing::OncePerKey);
}
