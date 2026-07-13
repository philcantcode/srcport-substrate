//! End-to-end: three plugins, manual assembly, host drive, optional UI.

use srcport_framework::{
    FrameworkError, FrameworkPolicy, Host, ModulePlugin, PortBody, ProcessingStatus, ProcessingView,
    ResultStatus, ResultView, StepContext, StepOutput, UiEvent, UiPersist, CONTRACT_PROCESSING_VIEW,
    CONTRACT_RESULT_VIEW,
};
use srcport_substrate::{
    Artifact, Assembly, AssemblyNode, Binding, Capability, MemoryKernel, ModuleManifest,
    NamedArtifact, NodeOutput, Port, RunState, WorkItem,
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

    fn execute(&mut self, step: &StepContext) -> Result<StepOutput, FrameworkError> {
        let q = step
            .inputs
            .get("question")
            .ok_or_else(|| FrameworkError::Invalid("missing question".into()))?;
        let facts = format!("facts-from:{}", String::from_utf8_lossy(&q.body));
        Ok(StepOutput {
            outputs: vec![PortBody {
                port: "facts".into(),
                contract: "demo.v1.Facts".into(),
                body: facts.into_bytes(),
            }],
        })
    }

    fn processing_ui(&self, _work: &WorkItem) -> Option<ProcessingView> {
        Some(ProcessingView {
            title: "Extracting facts".into(),
            status: ProcessingStatus::Running,
            detail: Some("Reading question…".into()),
            progress: Some(0.5),
            ..Default::default()
        })
    }

    fn result_ui(&self, _work: &WorkItem, _outputs: &[NamedArtifact]) -> Option<ResultView> {
        Some(ResultView {
            title: "Facts ready".into(),
            status: ResultStatus::Ok,
            summary: Some("Extracted facts".into()),
            ..Default::default()
        })
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

    fn execute(&mut self, _step: &StepContext) -> Result<StepOutput, FrameworkError> {
        Ok(StepOutput {
            outputs: vec![PortBody {
                port: "sources".into(),
                contract: "demo.v1.Sources".into(),
                body: b"SPEC.md".to_vec(),
            }],
        })
    }
    // headless — no UI hooks
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

    fn execute(&mut self, step: &StepContext) -> Result<StepOutput, FrameworkError> {
        let facts = step.inputs.get("facts").map(|a| a.body.clone()).unwrap_or_default();
        let sources = step.inputs.get("sources").map(|a| a.body.clone()).unwrap_or_default();
        let mut body = b"answer:".to_vec();
        body.extend_from_slice(&facts);
        body.push(b'+');
        body.extend_from_slice(&sources);
        Ok(StepOutput {
            outputs: vec![PortBody {
                port: "answer".into(),
                contract: "demo.v1.Answer".into(),
                body,
            }],
        })
    }

    fn processing_ui(&self, _work: &WorkItem) -> Option<ProcessingView> {
        Some(ProcessingView {
            title: "Writing answer".into(),
            status: ProcessingStatus::Running,
            ..Default::default()
        })
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn port(name: &str, contract: &str) -> Port {
    Port {
        name: name.into(),
        contract: contract.into(),
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

// ── tests ───────────────────────────────────────────────────────────────────

#[test]
fn host_drives_diamond_with_optional_ui() {
    let mut host = Host::new(MemoryKernel::new()).with_ui_persist(UiPersist::Artifacts);

    host.register_plugin(Box::new(Extractor)).unwrap();
    host.register_plugin(Box::new(Retriever)).unwrap();
    host.register_plugin(Box::new(Writer)).unwrap();

    let question = host
        .kernel()
        .put_artifact(Artifact {
            r#type: "demo.v1.Question".into(),
            body: b"What is substrate?".to_vec(),
            produced_by: "operator".into(),
            ..Default::default()
        })
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
    assert!(matches!(
        host.policy("run-1").map(|p| &p.mode),
        Some(srcport_framework::RunMode::Converge)
    ));

    let events = host.take_ui_events();
    // extractor: processing + result; writer: processing; retriever: none
    assert!(
        events.len() >= 3,
        "expected UI from extractor and writer, got {events:?}"
    );

    let mut saw_extract_processing = false;
    let mut saw_extract_result = false;
    for ev in &events {
        match ev {
            UiEvent::Processing { view, artifact_id } => {
                assert!(!artifact_id.is_empty(), "UiPersist::Artifacts must put views");
                if view.module == "extractor" {
                    saw_extract_processing = true;
                    assert_eq!(view.title, "Extracting facts");
                    let art = host
                        .kernel()
                        .get_artifact(
                            &srcport_substrate::ArtifactRef {
                                id: artifact_id.clone(),
                            },
                        )
                        .unwrap();
                    assert_eq!(art.r#type, CONTRACT_PROCESSING_VIEW);
                }
            }
            UiEvent::Result { view, artifact_id } => {
                assert!(!artifact_id.is_empty());
                if view.module == "extractor" {
                    saw_extract_result = true;
                    assert_eq!(view.status, ResultStatus::Ok);
                    let art = host
                        .kernel()
                        .get_artifact(
                            &srcport_substrate::ArtifactRef {
                                id: artifact_id.clone(),
                            },
                        )
                        .unwrap();
                    assert_eq!(art.r#type, CONTRACT_RESULT_VIEW);
                }
            }
        }
    }
    assert!(saw_extract_processing);
    assert!(saw_extract_result);
}

#[test]
fn headless_plugins_need_no_ui() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Box::new(Retriever)).unwrap();

    // Single-node assembly: question → sources
    let question = host
        .kernel()
        .put_artifact(Artifact {
            r#type: "demo.v1.Question".into(),
            body: b"q".to_vec(),
            produced_by: "op".into(),
            ..Default::default()
        })
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
    assert!(host.ui_events().is_empty());
}
