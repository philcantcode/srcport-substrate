//! Framework modes: converge, stream (loop on inject), selective nodes.

use srcport_framework::{
    DriveAfter, FrameworkError, FrameworkPolicy, Host, ModulePlugin, PortBody, StepContext,
    StepOutput,
};
use srcport_substrate::{
    Artifact, Assembly, AssemblyNode, Binding, Capability, Firing, MemoryKernel, ModuleManifest,
    NamedArtifact, NodeOutput, Port, RunState,
};

struct Echo {
    hits: u32,
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
                    contract: "demo.v1.In".into(),
                    ..Default::default()
                }],
                outputs: vec![Port {
                    name: "out".into(),
                    contract: "demo.v1.Out".into(),
                    ..Default::default()
                }],
                // Capability default is ONCE; stream policy overrides to ALWAYS.
                firing: Firing::Once as i32,
            }],
            ..Default::default()
        }
    }

    fn execute(&mut self, step: &StepContext) -> Result<StepOutput, FrameworkError> {
        self.hits += 1;
        let body = step
            .inputs
            .get("in")
            .map(|a| a.body.clone())
            .unwrap_or_default();
        Ok(StepOutput {
            outputs: vec![PortBody {
                port: "out".into(),
                contract: "demo.v1.Out".into(),
                body,
            }],
        })
    }
}

struct Extractor;
struct Writer;

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
        let q = step.inputs.get("question").map(|a| &a.body[..]).unwrap_or(b"");
        Ok(StepOutput {
            outputs: vec![PortBody {
                port: "facts".into(),
                contract: "demo.v1.Facts".into(),
                body: format!("facts:{}" , String::from_utf8_lossy(q)).into_bytes(),
            }],
        })
    }
}

impl ModulePlugin for Writer {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "writer".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "answer.write".into(),
                inputs: vec![
                    port("question", "demo.v1.Question"),
                    port("facts", "demo.v1.Facts"),
                ],
                outputs: vec![port("answer", "demo.v1.Answer")],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn execute(&mut self, step: &StepContext) -> Result<StepOutput, FrameworkError> {
        let mut body = b"answer:".to_vec();
        if let Some(f) = step.inputs.get("facts") {
            body.extend_from_slice(&f.body);
        }
        Ok(StepOutput {
            outputs: vec![PortBody {
                port: "answer".into(),
                contract: "demo.v1.Answer".into(),
                body,
            }],
        })
    }
}

fn port(name: &str, contract: &str) -> Port {
    Port {
        name: name.into(),
        contract: contract.into(),
        ..Default::default()
    }
}

fn put_in(host: &Host<MemoryKernel>, body: &[u8]) -> srcport_substrate::ArtifactRef {
    host.kernel()
        .put_artifact(Artifact {
            r#type: "demo.v1.In".into(),
            body: body.to_vec(),
            produced_by: "op".into(),
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn stream_mode_loops_on_inject() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Box::new(Echo { hits: 0 })).unwrap();

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
    host.register_plugin(Box::new(Extractor)).unwrap();
    host.register_plugin(Box::new(Writer)).unwrap();
    // retriever deliberately not registered / not in include_nodes

    let question = host
        .kernel()
        .put_artifact(Artifact {
            r#type: "demo.v1.Question".into(),
            body: b"q".to_vec(),
            produced_by: "op".into(),
            ..Default::default()
        })
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

    let run = host.drive("sel-1").unwrap();
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(run.steps, 2);
    assert!(run.answer.is_some());
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
