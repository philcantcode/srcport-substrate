//! Cross-run memoisation: hit/miss, digest invalidation, input cascade.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use srcport_framework::{
    FrameworkError, FrameworkPolicy, Host, MemoNodes, MemoPlan, MemoryMemo, ModulePlugin,
    PortBody, StepContext, StepOutput, StepStage,
};
use srcport_substrate::{
    artifact_with_trait, Assembly, AssemblyNode, Binding, Capability, MemoryKernel, ModuleManifest,
    NamedArtifact, NodeOutput, Port, RunState,
};

// ── Plugins with digests + execute counters ─────────────────────────────────

struct Extractor {
    digest: Arc<String>,
    executes: Arc<AtomicU32>,
}

struct Writer {
    digest: Arc<String>,
    executes: Arc<AtomicU32>,
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

    fn module_digest(&self) -> Option<String> {
        Some((*self.digest).clone())
    }

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        self.executes.fetch_add(1, Ordering::SeqCst);
        let q = step
            .inputs
            .get("question")
            .and_then(|a| a.traits.values().next().map(|f| f.body.as_slice()))
            .unwrap_or(b"");
        // Include digest byte in output so digest changes produce new artifact ids.
        let body = format!(
            "facts:{}:{}",
            String::from_utf8_lossy(q),
            self.digest.as_str()
        );
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait(
                "facts",
                "demo.v1.Facts",
                body.into_bytes(),
            )],
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

    fn module_digest(&self) -> Option<String> {
        Some((*self.digest).clone())
    }

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        self.executes.fetch_add(1, Ordering::SeqCst);
        let mut body = b"answer:".to_vec();
        if let Some(f) = step.inputs.get("facts") {
            if let Some(tr) = f.traits.values().next() {
                body.extend_from_slice(&tr.body);
            }
        }
        body.extend_from_slice(format!(":{}", self.digest.as_str()).as_bytes());
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("answer", "demo.v1.Answer", body)],
        })
    }
}

struct NoDigestEcho {
    executes: Arc<AtomicU32>,
}

impl ModulePlugin for NoDigestEcho {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "echo".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "echo.run".into(),
                inputs: vec![port("in", "demo.v1.In")],
                outputs: vec![port("out", "demo.v1.Out")],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        self.executes.fetch_add(1, Ordering::SeqCst);
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

fn port(name: &str, contract: &str) -> Port {
    Port {
        name: name.into(),
        traits: vec![contract.into()],
        ..Default::default()
    }
}

fn pipeline() -> Assembly {
    Assembly {
        id: "memo-pipe@1".into(),
        nodes: vec![
            AssemblyNode {
                id: "extract".into(),
                module: "extractor".into(),
                module_version: "1.0.0".into(),
                capability: "facts.extract".into(),
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
        ],
        terminal: Some(NodeOutput {
            node: "write".into(),
            port: "answer".into(),
        }),
    }
}

fn put_q(host: &Host<MemoryKernel>, body: &[u8]) -> srcport_substrate::ArtifactRef {
    host.kernel()
        .put_artifact({
            let mut a = artifact_with_trait("demo.v1.Question", body.to_vec());
            a.produced_by = "op".into();
            a
        })
        .unwrap()
}

fn register_pair(
    host: &mut Host<MemoryKernel>,
    extract_digest: Arc<String>,
    write_digest: Arc<String>,
    extract_exec: Arc<AtomicU32>,
    write_exec: Arc<AtomicU32>,
) {
    host.register_plugin(Box::new(Extractor {
        digest: extract_digest,
        executes: extract_exec,
    }))
    .unwrap();
    host.register_plugin(Box::new(Writer {
        digest: write_digest,
        executes: write_exec,
    }))
    .unwrap();
}

#[test]
fn memo_plan_without_store_errors() {
    let mut host = Host::new(MemoryKernel::new());
    let ex = Arc::new(AtomicU32::new(0));
    let wr = Arc::new(AtomicU32::new(0));
    register_pair(
        &mut host,
        Arc::new("d1".into()),
        Arc::new("d1".into()),
        ex,
        wr,
    );
    let q = put_q(&host, b"q");
    let err = host
        .start_pipeline(
            "r",
            pipeline(),
            vec![NamedArtifact {
                name: "question".into(),
                artifact: Some(q),
            }],
            FrameworkPolicy::memoized(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("MemoStore"));
}

#[test]
fn second_run_hits_memo_skips_execute() {
    let extract_exec = Arc::new(AtomicU32::new(0));
    let write_exec = Arc::new(AtomicU32::new(0));
    let mut host = Host::new(MemoryKernel::new()).with_memo(MemoryMemo::new());
    register_pair(
        &mut host,
        Arc::new("extract-v1".into()),
        Arc::new("write-v1".into()),
        extract_exec.clone(),
        write_exec.clone(),
    );

    let q = put_q(&host, b"hello");
    host.start_pipeline(
        "run-1",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q.clone()),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    let r1 = host.drive("run-1").unwrap();
    assert_eq!(r1.state(), RunState::Completed);
    assert_eq!(r1.steps, 2);
    assert_eq!(extract_exec.load(Ordering::SeqCst), 1);
    assert_eq!(write_exec.load(Ordering::SeqCst), 1);
    assert_eq!(host.execute_count(), 2);
    assert_eq!(host.memo_hit_count(), 0);
    assert_eq!(host.memo_store().unwrap().len(), 2);

    let answer1 = host
        .kernel()
        .get_artifact(r1.answer.as_ref().unwrap())
        .unwrap();

    // Same inputs + digests → full memo hit.
    host.start_pipeline(
        "run-2",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    let events = host.take_step_events(); // clear skip/none from start
    let r2 = host.drive("run-2").unwrap();
    assert_eq!(r2.state(), RunState::Completed);
    assert_eq!(r2.steps, 2);
    assert_eq!(extract_exec.load(Ordering::SeqCst), 1, "extract must not re-execute");
    assert_eq!(write_exec.load(Ordering::SeqCst), 1, "write must not re-execute");
    assert_eq!(host.execute_count(), 2);
    assert_eq!(host.memo_hit_count(), 2);

    let cached: Vec<_> = host
        .take_step_events()
        .into_iter()
        .chain(events)
        .filter(|e| e.stage == StepStage::Cached)
        .collect();
    // drive events only
    let drive_events = host.step_events();
    let _ = drive_events;
    // re-fetch: take already consumed; check via counters and answer equality
    let answer2 = host
        .kernel()
        .get_artifact(r2.answer.as_ref().unwrap())
        .unwrap();
    assert_eq!(answer1.id, answer2.id, "same content-addressed answer");
    let _ = cached;
}

#[test]
fn memo_cached_events_emitted() {
    let extract_exec = Arc::new(AtomicU32::new(0));
    let write_exec = Arc::new(AtomicU32::new(0));
    let mut host = Host::new(MemoryKernel::new()).with_memo(MemoryMemo::new());
    register_pair(
        &mut host,
        Arc::new("e".into()),
        Arc::new("w".into()),
        extract_exec,
        write_exec,
    );
    let q = put_q(&host, b"x");
    host.start_pipeline(
        "a",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q.clone()),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    host.drive("a").unwrap();
    host.take_step_events();

    host.start_pipeline(
        "b",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    host.drive("b").unwrap();
    let events = host.take_step_events();
    let cached: Vec<_> = events
        .iter()
        .filter(|e| e.stage == StepStage::Cached)
        .collect();
    assert_eq!(cached.len(), 2);
    assert!(cached.iter().any(|e| e.presentation.node_id == "extract"));
    assert!(cached.iter().any(|e| e.presentation.node_id == "write"));
    assert!(cached[0].presentation.meta.get("memo").map(|s| s == "hit").unwrap_or(false));
}

#[test]
fn digest_change_invalidates_node_and_cascades() {
    let extract_digest = Arc::new(std::sync::RwLock::new("extract-v1".to_string()));
    let write_digest = Arc::new("write-v1".to_string());
    let extract_exec = Arc::new(AtomicU32::new(0));
    let write_exec = Arc::new(AtomicU32::new(0));

    // Plugin that reads digest from RwLock so we can bump it between runs.
    struct DynExtract {
        digest: Arc<std::sync::RwLock<String>>,
        executes: Arc<AtomicU32>,
    }
    impl ModulePlugin for DynExtract {
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
        fn module_digest(&self) -> Option<String> {
            Some(self.digest.read().unwrap().clone())
        }
        fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
            self.executes.fetch_add(1, Ordering::SeqCst);
            let q = step
                .inputs
                .get("question")
                .and_then(|a| a.traits.values().next().map(|f| f.body.as_slice()))
                .unwrap_or(b"");
            let d = self.digest.read().unwrap().clone();
            Ok(StepOutput {
                outputs: vec![PortBody::with_trait(
                    "facts",
                    "demo.v1.Facts",
                    format!("facts:{}:{}", String::from_utf8_lossy(q), d).into_bytes(),
                )],
            })
        }
    }

    let mut host = Host::new(MemoryKernel::new()).with_memo(MemoryMemo::new());
    host.register_plugin(Box::new(DynExtract {
        digest: extract_digest.clone(),
        executes: extract_exec.clone(),
    }))
    .unwrap();
    host.register_plugin(Box::new(Writer {
        digest: write_digest,
        executes: write_exec.clone(),
    }))
    .unwrap();

    let q = put_q(&host, b"q");
    host.start_pipeline(
        "r1",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q.clone()),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    host.drive("r1").unwrap();
    assert_eq!(extract_exec.load(Ordering::SeqCst), 1);
    assert_eq!(write_exec.load(Ordering::SeqCst), 1);

    // Bump extract digest → extract misses → new facts id → write misses.
    *extract_digest.write().unwrap() = "extract-v2".into();

    host.start_pipeline(
        "r2",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    host.drive("r2").unwrap();
    assert_eq!(extract_exec.load(Ordering::SeqCst), 2);
    assert_eq!(write_exec.load(Ordering::SeqCst), 2);
    assert_eq!(host.memo_hit_count(), 0);
}

#[test]
fn input_change_invalidates_from_first_dirty_node() {
    let extract_exec = Arc::new(AtomicU32::new(0));
    let write_exec = Arc::new(AtomicU32::new(0));
    let mut host = Host::new(MemoryKernel::new()).with_memo(MemoryMemo::new());
    register_pair(
        &mut host,
        Arc::new("e".into()),
        Arc::new("w".into()),
        extract_exec.clone(),
        write_exec.clone(),
    );

    let q1 = put_q(&host, b"one");
    host.start_pipeline(
        "r1",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q1),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    host.drive("r1").unwrap();

    let q2 = put_q(&host, b"two");
    host.start_pipeline(
        "r2",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q2),
        }],
        FrameworkPolicy::memoized(),
    )
    .unwrap();
    host.drive("r2").unwrap();
    assert_eq!(extract_exec.load(Ordering::SeqCst), 2);
    assert_eq!(write_exec.load(Ordering::SeqCst), 2);
}

#[test]
fn missing_digest_never_caches() {
    let executes = Arc::new(AtomicU32::new(0));
    let mut host = Host::new(MemoryKernel::new()).with_memo(MemoryMemo::new());
    host.register_plugin(Box::new(NoDigestEcho {
        executes: executes.clone(),
    }))
    .unwrap();

    let put = |host: &Host<MemoryKernel>, b: &[u8]| {
        host.kernel()
            .put_artifact({
                let mut a = artifact_with_trait("demo.v1.In", b.to_vec());
                a.produced_by = "op".into();
                a
            })
            .unwrap()
    };
    let assembly = Assembly {
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
    };

    let inp = put(&host, b"same");
    for id in ["m1", "m2"] {
        host.start_pipeline(
            id,
            assembly.clone(),
            vec![NamedArtifact {
                name: "in".into(),
                artifact: Some(inp.clone()),
            }],
            FrameworkPolicy::memoized(),
        )
        .unwrap();
        host.drive(id).unwrap();
    }
    assert_eq!(executes.load(Ordering::SeqCst), 2);
    assert_eq!(host.memo_store().unwrap().len(), 0);
    assert_eq!(host.memo_hit_count(), 0);
}

#[test]
fn memo_nodes_only_caches_selected() {
    let extract_exec = Arc::new(AtomicU32::new(0));
    let write_exec = Arc::new(AtomicU32::new(0));
    let mut host = Host::new(MemoryKernel::new()).with_memo(MemoryMemo::new());
    register_pair(
        &mut host,
        Arc::new("e".into()),
        Arc::new("w".into()),
        extract_exec.clone(),
        write_exec.clone(),
    );

    let policy = FrameworkPolicy::converge().with_memo(
        MemoPlan::on().with_nodes(MemoNodes::Only(vec!["extract".into()])),
    );
    let q = put_q(&host, b"q");
    host.start_pipeline(
        "r1",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q.clone()),
        }],
        policy.clone(),
    )
    .unwrap();
    host.drive("r1").unwrap();
    assert_eq!(host.memo_store().unwrap().len(), 1);

    host.start_pipeline(
        "r2",
        pipeline(),
        vec![NamedArtifact {
            name: "question".into(),
            artifact: Some(q),
        }],
        policy,
    )
    .unwrap();
    host.drive("r2").unwrap();
    // extract hit, write always executed (not in memo nodes)
    assert_eq!(extract_exec.load(Ordering::SeqCst), 1);
    assert_eq!(write_exec.load(Ordering::SeqCst), 2);
    assert_eq!(host.memo_hit_count(), 1);
}

#[test]
fn converge_without_memo_always_executes() {
    let extract_exec = Arc::new(AtomicU32::new(0));
    let write_exec = Arc::new(AtomicU32::new(0));
    let mut host = Host::new(MemoryKernel::new()).with_memo(MemoryMemo::new());
    register_pair(
        &mut host,
        Arc::new("e".into()),
        Arc::new("w".into()),
        extract_exec.clone(),
        write_exec.clone(),
    );
    let q = put_q(&host, b"q");
    for id in ["c1", "c2"] {
        host.start_pipeline(
            id,
            pipeline(),
            vec![NamedArtifact {
                name: "question".into(),
                artifact: Some(q.clone()),
            }],
            FrameworkPolicy::converge(),
        )
        .unwrap();
        host.drive(id).unwrap();
    }
    assert_eq!(extract_exec.load(Ordering::SeqCst), 2);
    assert_eq!(write_exec.load(Ordering::SeqCst), 2);
    assert_eq!(host.memo_hit_count(), 0);
}
