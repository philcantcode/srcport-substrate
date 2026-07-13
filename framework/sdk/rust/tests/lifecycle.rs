//! Step lifecycle: Init → Progress* → Final, failure path, legacy hooks.

use srcport_framework::{
    FrameworkError, FrameworkPolicy, Host, ModulePlugin, PortBody, Presentation, PresentationStatus,
    StepContext, StepOutput, StepResult, StepStage, UiPersist, CONTRACT_STEP_PROGRESS,
};
use srcport_substrate::{
    artifact_with_trait, has_traits, Assembly, AssemblyNode, Binding, Capability, MemoryKernel,
    ModuleManifest, NamedArtifact, NodeOutput, Port, RunState,
};

struct Scanner;

impl ModulePlugin for Scanner {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "scanner".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "scan.run".into(),
                inputs: vec![Port {
                    name: "target".into(),
                    traits: vec!["demo.v1.Target".into()],
                    ..Default::default()
                }],
                outputs: vec![Port {
                    name: "report".into(),
                    traits: vec!["demo.v1.Report".into()],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn on_init(&self, _step: &StepContext) -> Option<Presentation> {
        Some(Presentation::init("Scan").with_detail("Warming up").with_phase("init"))
    }

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        step.emit_progress(
            Presentation::progress("Scan", Some(0.25))
                .with_detail("phase A")
                .with_phase("a"),
        );
        step.emit_progress(
            Presentation::progress("Scan", Some(0.75))
                .with_detail("phase B")
                .with_phase("b"),
        );
        let body = step
            .inputs
            .get("target")
            .and_then(|a| a.traits.values().next().map(|f| f.body.clone()))
            .unwrap_or_default();
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("report", "demo.v1.Report", body)],
        })
    }

    fn on_final(&self, _step: &StepContext, result: &StepResult) -> Option<Presentation> {
        if result.ok {
            Some(Presentation::final_ok("Scan complete").with_phase("done"))
        } else {
            Some(Presentation::final_failed(
                "Scan failed",
                result.error.clone().unwrap_or_default(),
            ))
        }
    }
}

// with_phase is on Presentation in the crate

struct Boom;

impl ModulePlugin for Boom {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "boom".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "boom.run".into(),
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
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn on_init(&self, _: &StepContext) -> Option<Presentation> {
        Some(Presentation::init("Boom"))
    }

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        step.emit_progress(Presentation::progress("Boom", Some(0.1)));
        Err(FrameworkError::Invalid("kaboom".into()))
    }

    fn on_final(&self, _: &StepContext, result: &StepResult) -> Option<Presentation> {
        Some(Presentation::final_failed(
            "Boom final",
            result.error.clone().unwrap_or_else(|| "err".into()),
        ))
    }
}

fn assembly_one(module: &str, cap: &str, in_port: &str, out_port: &str) -> Assembly {
    Assembly {
        id: format!("{module}@1"),
        nodes: vec![AssemblyNode {
            id: "n".into(),
            module: module.into(),
            module_version: "1.0.0".into(),
            capability: cap.into(),
        }],
        bindings: vec![Binding {
            to_node: "n".into(),
            to_port: in_port.into(),
            input: "in".into(),
            ..Default::default()
        }],
        terminal: Some(NodeOutput {
            node: "n".into(),
            port: out_port.into(),
        }),
    }
}

#[test]
fn init_progress_final_sequence_and_artifacts() {
    let mut host = Host::new(MemoryKernel::new()).with_ui_persist(UiPersist::Artifacts);
    host.register_plugin(Box::new(Scanner)).unwrap();

    let target = host
        .kernel()
        .put_artifact({ let mut __a = artifact_with_trait("demo.v1.Target", b"host1".to_vec()); __a.produced_by = "op".into(); __a })
        .unwrap();

    host.start_pipeline(
        "life-1",
        assembly_one("scanner", "scan.run", "target", "report"),
        vec![NamedArtifact {
            name: "in".into(),
            artifact: Some(target),
        }],
        FrameworkPolicy::converge(),
    )
    .unwrap();

    let run = host.drive("life-1").unwrap();
    assert_eq!(run.state(), RunState::Completed);

    let events = host.take_step_events();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].stage, StepStage::Init);
    assert_eq!(events[1].stage, StepStage::Progress);
    assert_eq!(events[1].presentation.progress, Some(0.25));
    assert_eq!(events[2].stage, StepStage::Progress);
    assert_eq!(events[2].presentation.progress, Some(0.75));
    assert_eq!(events[3].stage, StepStage::Final);
    assert_eq!(events[3].presentation.status, PresentationStatus::Ok);

    let prog = host
        .kernel()
        .get_artifact(&srcport_substrate::ArtifactRef {
            id: events[1].artifact_id.clone(),
        })
        .unwrap();
    assert!(has_traits(&prog, &[CONTRACT_STEP_PROGRESS]));
}

#[test]
fn failure_emits_final_failed_without_commit() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Box::new(Boom)).unwrap();

    let inp = host
        .kernel()
        .put_artifact({ let mut __a = artifact_with_trait("demo.v1.In", b"x".to_vec()); __a.produced_by = "op".into(); __a })
        .unwrap();

    host.start_pipeline(
        "boom-1",
        Assembly {
            id: "boom@1".into(),
            nodes: vec![AssemblyNode {
                id: "n".into(),
                module: "boom".into(),
                module_version: "1.0.0".into(),
                capability: "boom.run".into(),
            }],
            bindings: vec![Binding {
                to_node: "n".into(),
                to_port: "in".into(),
                input: "in".into(),
                ..Default::default()
            }],
            terminal: Some(NodeOutput {
                node: "n".into(),
                port: "out".into(),
            }),
        },
        vec![NamedArtifact {
            name: "in".into(),
            artifact: Some(inp),
        }],
        FrameworkPolicy::converge(),
    )
    .unwrap();

    let err = host.drive("boom-1").unwrap_err();
    assert!(matches!(err, FrameworkError::StepFailed(_)), "{err}");

    let events = host.take_step_events();
    assert!(events.iter().any(|e| e.stage == StepStage::Init));
    assert!(events.iter().any(|e| e.stage == StepStage::Progress));
    let finals: Vec<_> = events.iter().filter(|e| e.stage == StepStage::Final).collect();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].presentation.status, PresentationStatus::Failed);
    assert!(finals[0]
        .presentation
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("kaboom"));

    let run = host.get_run("boom-1").unwrap();
    // no successful terminal commit
    assert!(run.answer.is_none());
}
