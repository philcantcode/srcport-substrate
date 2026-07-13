//! End-to-end: three plugins, converge policy, step lifecycle presentation.

use srcport_framework::{
    FrameworkError, FrameworkPolicy, Host, ModulePlugin, PortBody, Presentation, PresentationStatus,
    ProcessingStatus, ProcessingView, ResultStatus, ResultView, StepContext, StepEvent, StepOutput,
    StepResult, StepStage, UiPersist, CONTRACT_STEP_FINAL, CONTRACT_STEP_INIT,
};
use srcport_substrate::{
    artifact_with_trait, has_traits, Assembly, AssemblyNode, Binding, Capability, MemoryKernel,
    ModuleManifest, NamedArtifact, NodeOutput, Port, RunState, WorkItem,
};

// ── plugins ─────────────────────────────────────────────────────────────────

struct Extractor;

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

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        let q_body = step
            .inputs
            .get("question")
            .ok_or_else(|| FrameworkError::Invalid("missing question".into()))?
            .traits
            .values()
            .next()
            .map(|f| f.body.clone())
            .unwrap_or_default();
        step.emit_progress(
            Presentation::progress("Extracting facts", Some(0.5)).with_detail("Reading question…"),
        );
        let facts = format!("facts-from:{}", String::from_utf8_lossy(&q_body));
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("facts", "demo.v1.Facts", facts.into_bytes())],
        })
    }

    fn on_init(&self, _step: &StepContext) -> Option<Presentation> {
        Some(Presentation::init("Extracting facts").with_detail("Starting…"))
    }

    fn on_final(&self, _step: &StepContext, _result: &StepResult) -> Option<Presentation> {
        Some(Presentation::final_ok("Facts ready").with_detail("Extracted facts"))
    }
}

struct Retriever;

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

    fn execute(&mut self, _step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("sources", "demo.v1.Sources", b"SPEC.md".to_vec())],
        })
    }
}

struct Writer;

impl ModulePlugin for Writer {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "writer".into(),
            version: "2.0.0".into(),
            provides: vec![Capability {
                name: "answer.write".into(),
                inputs: vec![
                    port("question", "demo.v1.Question"),
                    port("facts", "demo.v1.Facts"),
                    port("sources", "demo.v1.Sources"),
                ],
                outputs: vec![port("answer", "demo.v1.Answer")],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        let facts = step.inputs.get("facts").and_then(|a| a.traits.values().next().map(|f| f.body.clone())).unwrap_or_default();
        let sources = step.inputs.get("sources").and_then(|a| a.traits.values().next().map(|f| f.body.clone())).unwrap_or_default();
        let mut body = b"answer:".to_vec();
        body.extend_from_slice(&facts);
        body.push(b'+');
        body.extend_from_slice(&sources);
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("answer", "demo.v1.Answer", body)],
        })
    }

    // Legacy hooks still work via default on_init / on_final adapters.
    fn processing_ui(&self, _work: &WorkItem) -> Option<ProcessingView> {
        Some(ProcessingView {
            title: "Writing answer".into(),
            status: ProcessingStatus::Running,
            ..Default::default()
        })
    }

    fn result_ui(&self, _work: &WorkItem, _outputs: &[NamedArtifact]) -> Option<ResultView> {
        Some(ResultView {
            title: "Answer ready".into(),
            status: ResultStatus::Ok,
            ..Default::default()
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

#[test]
fn host_drives_diamond_with_step_lifecycle() {
    let mut host = Host::new(MemoryKernel::new()).with_ui_persist(UiPersist::Artifacts);

    host.register_plugin(Box::new(Extractor)).unwrap();
    host.register_plugin(Box::new(Retriever)).unwrap();
    host.register_plugin(Box::new(Writer)).unwrap();

    let question = host
        .kernel()
        .put_artifact({ let mut __a = artifact_with_trait("demo.v1.Question", b"What is substrate?".to_vec()); __a.produced_by = "operator".into(); __a })
        .unwrap();

    let assembly = Assembly {
        id: "answer-pipeline@1".into(),
        nodes: vec![
            node("extract", "extractor", "1.0.0", "facts.extract"),
            node("retrieve", "retriever", "1.0.0", "sources.retrieve"),
            node("write", "writer", "2.0.0", "answer.write"),
        ],
        bindings: vec![
            from_input("extract", "question", "question"),
            from_input("retrieve", "question", "question"),
            from_input("write", "question", "question"),
            from_node("write", "facts", "extract", "facts"),
            from_node("write", "sources", "retrieve", "sources"),
        ],
        terminal: Some(NodeOutput {
            node: "write".into(),
            port: "answer".into(),
        }),
    };

    host.start_pipeline(
        "run-1",
        assembly,
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(question),
        }],
        FrameworkPolicy::converge(),
    )
    .unwrap();

    let run = host.drive("run-1").unwrap();
    assert_eq!(run.state(), RunState::Completed);
    assert!(run.answer.is_some());

    let events = host.take_step_events();
    // extractor: init + progress + final; writer: init (legacy) + final (legacy); retriever: none
    assert!(
        events.len() >= 5,
        "expected lifecycle events, got {events:?}"
    );

    let extract: Vec<&StepEvent> = events
        .iter()
        .filter(|e| e.presentation.module == "extractor")
        .collect();
    assert_eq!(extract[0].stage, StepStage::Init);
    assert_eq!(extract[1].stage, StepStage::Progress);
    assert_eq!(extract[2].stage, StepStage::Final);
    assert_eq!(extract[2].presentation.status, PresentationStatus::Ok);
    assert!(!extract[0].artifact_id.is_empty());

    let art = host
        .kernel()
        .get_artifact(&srcport_substrate::ArtifactRef {
            id: extract[0].artifact_id.clone(),
        })
        .unwrap();
    assert!(has_traits(&art, &[CONTRACT_STEP_INIT]));

    let final_art = host
        .kernel()
        .get_artifact(&srcport_substrate::ArtifactRef {
            id: extract[2].artifact_id.clone(),
        })
        .unwrap();
    assert!(has_traits(&final_art, &[CONTRACT_STEP_FINAL]));
}

#[test]
fn headless_plugins_need_no_presentation() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Box::new(Retriever)).unwrap();

    let question = host
        .kernel()
        .put_artifact({ let mut __a = artifact_with_trait("demo.v1.Question", b"q".to_vec()); __a.produced_by = "op".into(); __a })
        .unwrap();

    host.start_pipeline(
        "run-h",
        Assembly {
            id: "only-retrieve".into(),
            nodes: vec![node("retrieve", "retriever", "1.0.0", "sources.retrieve")],
            bindings: vec![from_input("retrieve", "question", "question")],
            terminal: Some(NodeOutput {
                node: "retrieve".into(),
                port: "sources".into(),
            }),
        },
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(question),
        }],
        FrameworkPolicy::converge(),
    )
    .unwrap();

    let run = host.drive("run-h").unwrap();
    assert_eq!(run.state(), RunState::Completed);
    assert!(host.step_events().is_empty());
}
