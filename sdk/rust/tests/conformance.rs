//! The minimal conformance suite from `SPEC.md` §Conformance. An SDK is
//! conformant iff all of them pass. Each test names the primitive and invariant
//! it pins down. If you widen the contract, add tests here — never weaken these.

use std::collections::BTreeMap;

use srcport_substrate::*;

// 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
#[test]
fn addressing_is_content_derived_and_metamorphic() {
    let k = MemoryKernel::new();

    let a = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        produced_by: "recon".into(),
        ..Default::default()
    }).unwrap();
    // Identical content, different producer/meta — must land the SAME address.
    let b = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        produced_by: "someone-else".into(),
        ..Default::default()
    }).unwrap();
    assert_eq!(a.id, b.id, "same (type, body) must yield the same id");
    assert!(a.id.starts_with("sha256:"));

    // One byte different in the body — must yield a DIFFERENT address.
    let c = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.2".to_vec(),
        ..Default::default()
    }).unwrap();
    assert_ne!(a.id, c.id, "a one-byte change must change the address");

    // Type participates in the address too.
    let d = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Port".into(),
        body: b"10.0.0.1".to_vec(),
        ..Default::default()
    }).unwrap();
    assert_ne!(a.id, d.id, "type must participate in the address");

    // Pure function agrees with the kernel.
    assert_eq!(a.id, artifact_id("acme.recon.v1.Host", b"10.0.0.1"));
}

// 2. IMMUTABILITY — reads back byte-identical; a later put never mutates it.
#[test]
fn artifacts_are_immutable() {
    let k = MemoryKernel::new();

    let mut meta = BTreeMap::new();
    meta.insert("first".into(), "true".into());
    let r = k.put_artifact(Artifact {
        r#type: "t".into(),
        body: b"payload".to_vec(),
        meta,
        ..Default::default()
    }).unwrap();

    let got = k.get_artifact(&r).unwrap();
    assert_eq!(got.body, b"payload", "reads back byte-identical");
    assert_eq!(got.meta.get("first").map(String::as_str), Some("true"));

    // A later put of the same content (same id) with different meta must NOT
    // change what is stored. First write wins.
    let mut meta2 = BTreeMap::new();
    meta2.insert("first".into(), "false".into());
    meta2.insert("sneaky".into(), "yes".into());
    let r2 = k.put_artifact(Artifact {
        r#type: "t".into(),
        body: b"payload".to_vec(),
        meta: meta2,
        ..Default::default()
    }).unwrap();
    assert_eq!(r2.id, r.id, "same content ⇒ same id");

    let after = k.get_artifact(&r).unwrap();
    assert_eq!(after.meta.get("first").map(String::as_str), Some("true"));
    assert!(
        !after.meta.contains_key("sneaky"),
        "stored value was not mutated"
    );
}

// 3. ORDERING & ISOLATION — events reach exactly their subscribers, in seq
//    order, and never reach non-subscribers.
#[test]
fn events_are_ordered_and_isolated() {
    let k = MemoryKernel::new();

    let hosts = k.subscribe(Subscription {
        module: "a".into(),
        topics: vec!["recon.host.found".into()],
    });
    let ports = k.subscribe(Subscription {
        module: "b".into(),
        topics: vec!["recon.port.found".into()],
    });

    let h1 = k
        .put_artifact(Artifact {
            r#type: "acme.recon.v1.Host".into(),
            body: b"h1".to_vec(),
            ..Default::default()
        })
        .unwrap();
    let h2 = k
        .put_artifact(Artifact {
            r#type: "acme.recon.v1.Host".into(),
            body: b"h2".to_vec(),
            ..Default::default()
        })
        .unwrap();
    let p1 = k
        .put_artifact(Artifact {
            r#type: "acme.recon.v1.Port".into(),
            body: b"p1".to_vec(),
            ..Default::default()
        })
        .unwrap();
    let s1 = k
        .publish(Event {
            topic: "recon.host.found".into(),
            r#type: "acme.recon.v1.Host".into(),
            artifacts: vec![h1.clone()],
            ..Default::default()
        })
        .seq;
    let s2 = k
        .publish(Event {
            topic: "recon.host.found".into(),
            r#type: "acme.recon.v1.Host".into(),
            artifacts: vec![h2.clone()],
            ..Default::default()
        })
        .seq;
    let s3 = k
        .publish(Event {
            topic: "recon.port.found".into(),
            r#type: "acme.recon.v1.Port".into(),
            artifacts: vec![p1.clone()],
            ..Default::default()
        })
        .seq;

    // Monotonic total order.
    assert!(s1 < s2 && s2 < s3, "seq is monotonic across all topics");

    // Subscriber A got exactly its two host events, in seq order...
    let e1 = hosts.try_recv().unwrap();
    let e2 = hosts.try_recv().unwrap();
    assert_eq!(e1.seq, s1);
    assert_eq!(e1.artifacts[0].id, h1.id);
    assert_eq!(e2.seq, s2);
    assert_eq!(e2.artifacts[0].id, h2.id);
    assert!(e1.seq < e2.seq, "delivered in seq order");
    assert!(hosts.try_recv().is_err(), "A never received the port event");

    // ...and subscriber B got exactly the one port event.
    let p = ports.try_recv().unwrap();
    assert_eq!(p.seq, s3);
    assert_eq!(p.artifacts[0].id, p1.id);
    assert!(
        ports.try_recv().is_err(),
        "B never received the host events"
    );
}

// 4. LEDGER INTEGRITY — the chain verifies; tampering breaks verification.
#[test]
fn ledger_is_tamper_evident() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "m".into(),
        version: "0.1.0".into(),
        ..Default::default()
    });
    k.put_artifact(Artifact {
        r#type: "t".into(),
        body: b"x".to_vec(),
        ..Default::default()
    }).unwrap();
    k.append(AppendRequest {
        kind: "domain.fact".into(),
        subject: "s".into(),
        detail: b"d".to_vec(),
    });

    let chain = k.ledger();
    assert!(chain.len() >= 3);
    assert!(k.verify_ledger(), "the live chain verifies");
    assert!(verify_chain(&chain), "a snapshot of it verifies too");

    // Tamper with a committed entry's subject, leaving its stored hash intact.
    let mut tampered = chain.clone();
    tampered[1].subject = "hacked".into();
    assert!(!verify_chain(&tampered), "tampering breaks verification");

    // Even splicing an entry out is detected (seq / prev_hash linkage).
    let mut spliced = chain.clone();
    spliced.remove(1);
    assert!(
        !verify_chain(&spliced),
        "removing an entry breaks the chain"
    );
}

// 7b. Fat detail for artifact.put and module.registered — both reconstruct from
//     the chain, and the body is cleared (already addressed by the id in
//     `subject`, so the log never duplicates blob content). Provenance is a
//     separate Derivation record, not on the Artifact.
#[test]
fn artifact_and_module_reconstruct_from_the_chain() {
    let k = MemoryKernel::new();

    let mut meta = BTreeMap::new();
    meta.insert("region".into(), "eu".into());
    meta.insert("scan".into(), "full".into());
    let r = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        meta: meta.clone(),
        produced_by: "recon".into(),
        ..Default::default()
    }).unwrap();

    k.register(ModuleManifest {
        name: "recon".into(),
        version: "0.1.0".into(),
        provides: vec![Capability {
            name: "recon.scan".into(),
            outputs: vec![Port {
                name: "host".into(),
                contract: "acme.recon.v1.Host".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        requires: vec!["report.render".into()],
    });

    let chain = k.ledger();

    // artifact.put reconstructs everything but the body.
    let a_entry = chain.iter().find(|e| e.kind == "artifact.put").unwrap();
    let a = Artifact::decode(&a_entry.detail[..]).unwrap();
    assert_eq!(a.id, r.id);
    assert_eq!(a_entry.subject, r.id, "subject is the content address");
    assert_eq!(a.r#type, "acme.recon.v1.Host");
    assert_eq!(a.meta, meta);
    assert_eq!(a.produced_by, "recon");
    assert!(
        a.body.is_empty(),
        "body is cleared — the id already addresses it"
    );

    // module.registered reconstructs the whole manifest.
    let m_entry = chain
        .iter()
        .find(|e| e.kind == "module.registered")
        .unwrap();
    let m = ModuleManifest::decode(&m_entry.detail[..]).unwrap();
    assert_eq!(m.name, "recon");
    assert_eq!(m.version, "0.1.0");
    assert_eq!(m.provides.len(), 1);
    assert_eq!(m.provides[0].name, "recon.scan");
    assert_eq!(m.provides[0].outputs[0].contract, "acme.recon.v1.Host");
    assert_eq!(m.requires, vec!["report.render".to_string()]);

    assert!(k.verify_ledger(), "the chain with fat detail verifies");
}

// 7c. CANONICAL DETAIL — `map<>` fields encode in sorted-key order, so the same
//     logical value hashes to identical bytes across SDKs and runs. `Artifact.meta`
//     is a BTreeMap for exactly this reason; two builds must encode byte-identically.
#[test]
fn map_detail_encodes_canonically() {
    let pairs = [("z", "1"), ("a", "2"), ("m", "3"), ("b", "4")];
    let build = || {
        let mut meta = BTreeMap::new();
        for (key, val) in pairs {
            meta.insert(key.to_string(), val.to_string());
        }
        Artifact {
            r#type: "t".into(),
            meta,
            ..Default::default()
        }
    };
    assert_eq!(
        build().encode_to_vec(),
        build().encode_to_vec(),
        "identical meta must encode to identical bytes (sorted keys)"
    );
}

// 8. ADDRESS INVARIANCE — `meta` and `produced_by` are NOT part of the address;
//    transforming them must not move the `id`. The mirror of #1: an
//    identity-preserving change must NOT change the address (metamorphic).
//    Provenance is a Derivation, not an Artifact field.
#[test]
fn address_ignores_non_identity_fields() {
    let k = MemoryKernel::new();
    let base = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        ..Default::default()
    }).unwrap();

    let mut meta = BTreeMap::new();
    meta.insert("x".into(), "y".into());
    let enriched = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        meta,
        produced_by: "whoever".into(),
        ..Default::default()
    }).unwrap();

    assert_eq!(
        enriched.id, base.id,
        "meta and produced_by must not participate in the address"
    );
    assert_eq!(enriched.id, artifact_id("acme.recon.v1.Host", b"10.0.0.1"));
}

// CROSS-SDK KNOWN ANSWER — a fixed scenario must produce this exact final ledger
// hash in every SDK. Go, Rust, and Python all assert the SAME constant, so any
// drift in canonical detail encoding or the hash rule fails here and the three
// chains are pinned to cross-verify. If this constant ever changes, it changes in
// all three suites in lockstep — never one SDK alone.
#[test]
fn ledger_hash_known_answer_cross_sdk() {
    const WANT: &str = "5d9dea28f0fa779b7d76dd6137c9b6079561289b12ed6dff022a889b94d69cd2";

    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "recon".into(),
        version: "0.1.0".into(),
        provides: vec![Capability {
            name: "recon.scan".into(),
            outputs: vec![Port {
                name: "host".into(),
                contract: "acme.recon.v1.Host".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        requires: vec!["report.render".into()],
    });
    let mut meta = BTreeMap::new();
    meta.insert("region".into(), "eu".into());
    meta.insert("scan".into(), "full".into());
    k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        meta,
        produced_by: "recon".into(),
        ..Default::default()
    }).unwrap();
    let chain = k.ledger();
    assert!(k.verify_ledger(), "the chain must verify");
    assert_eq!(
        chain.last().unwrap().hash,
        WANT,
        "cross-SDK ledger hash drift"
    );
}

// 6. DISCOVERY — the registry reports every module, capability, and contract.
#[test]
fn registry_reports_everything() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "recon".into(),
        version: "0.1.0".into(),
        provides: vec![Capability {
            name: "recon.scan".into(),
            outputs: vec![Port {
                name: "host".into(),
                contract: "acme.recon.v1.Host".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        requires: vec![],
    });
    k.register(ModuleManifest {
        name: "report".into(),
        version: "0.2.0".into(),
        provides: vec![Capability {
            name: "report.render".into(),
            outputs: vec![Port {
                name: "report".into(),
                contract: "acme.report.v1.Report".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        requires: vec!["recon.scan".into()],
    });

    let snap = k.snapshot();

    let names: Vec<_> = snap.modules.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"recon") && names.contains(&"report"));

    let caps: Vec<_> = snap.capabilities.iter().map(|c| c.name.as_str()).collect();
    assert!(caps.contains(&"recon.scan") && caps.contains(&"report.render"));

    let contracts: Vec<_> = snap.contracts.iter().map(|c| c.r#ref.as_str()).collect();
    assert!(contracts.contains(&"acme.recon.v1.Host"));
    assert!(contracts.contains(&"acme.report.v1.Report"));
}

// 6b. CONTRACT IDENTITY — content-addressed under ref; immutable; conflict on
// redefinition; placeholder fill-once; ports bind to the pinned identity.
#[test]
fn contracts_are_immutable_and_identifiable() {
    let k = MemoryKernel::new();

    let stored = k
        .put_contract(Contract {
            r#ref: "acme.Host".into(),
            media_type: "application/schema+json".into(),
            schema: r#"{"type":"object"}"#.into(),
            version: "1.0.0".into(),
            compatible_with: vec!["acme.Host.v0".into(), "acme.legacy.Host".into()],
            ..Default::default()
        })
        .unwrap();
    let want = contract_digest(
        "application/schema+json",
        r#"{"type":"object"}"#,
        "1.0.0",
        &["acme.Host.v0".into(), "acme.legacy.Host".into()],
    );
    assert_eq!(stored.digest, want);
    assert_eq!(
        stored.compatible_with,
        vec!["acme.Host.v0".to_string(), "acme.legacy.Host".to_string()]
    );

    // Identical re-put (unsorted compatible_with) is idempotent.
    let again = k
        .put_contract(Contract {
            r#ref: "acme.Host".into(),
            media_type: "application/schema+json".into(),
            schema: r#"{"type":"object"}"#.into(),
            version: "1.0.0".into(),
            compatible_with: vec!["acme.legacy.Host".into(), "acme.Host.v0".into()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(again.digest, stored.digest);

    // Different content under the same ref is CONFLICT.
    let conflict = k.put_contract(Contract {
        r#ref: "acme.Host".into(),
        media_type: "application/schema+json".into(),
        schema: r#"{"type":"string"}"#.into(),
        version: "1.0.0".into(),
        ..Default::default()
    });
    assert!(matches!(conflict, Err(KernelError::Conflict(_))));

    // Register creates a name-only placeholder; PutContract may fill it once.
    k.register(ModuleManifest {
        name: "mod".into(),
        version: "1".into(),
        provides: vec![Capability {
            name: "do".into(),
            outputs: vec![Port {
                name: "out".into(),
                contract: "acme.NewThing".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let filled = k
        .put_contract(Contract {
            r#ref: "acme.NewThing".into(),
            media_type: "text/x-protobuf".into(),
            schema: "message NewThing {}".into(),
            version: "1".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(!is_contract_placeholder(&filled));
    assert!(!filled.digest.is_empty());

    let refill = k.put_contract(Contract {
        r#ref: "acme.NewThing".into(),
        media_type: "text/x-protobuf".into(),
        schema: "message Other {}".into(),
        version: "1".into(),
        ..Default::default()
    });
    assert!(matches!(refill, Err(KernelError::Conflict(_))));

    // Mismatched caller-supplied digest is INVALID.
    let bad = k.put_contract(Contract {
        r#ref: "acme.Other".into(),
        schema: "x".into(),
        digest: "sha256:deadbeef".into(),
        ..Default::default()
    });
    assert!(matches!(bad, Err(KernelError::Invalid(_))));

    // contract.registered lands in the ledger with reconstructable detail.
    let chain = k.ledger();
    let entry = chain
        .iter()
        .find(|e| e.kind == "contract.registered" && e.subject == "acme.Host")
        .expect("contract.registered must appear in the ledger");
    let c = Contract::decode(entry.detail.as_slice()).unwrap();
    assert_eq!(c.r#ref, "acme.Host");
    assert_eq!(c.digest, want);
}

// 9. CONVERGENCE — typed artifacts flow through a pinned finite assembly;
// fan-in waits for all inputs, the terminal artifact closes the run, and a
// closed run cannot be reopened.
#[test]
fn run_feeds_forward_and_closes_on_its_terminal_answer() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "extractor".into(),
        version: "1.0.0".into(),
        provides: vec![Capability {
            name: "facts.extract".into(),
            inputs: vec![Port {
                name: "question".into(),
                contract: "demo.Question".into(),
                ..Default::default()
            }],
            outputs: vec![Port {
                name: "facts".into(),
                contract: "demo.Facts".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    k.register(ModuleManifest {
        name: "writer".into(),
        version: "2.0.0".into(),
        provides: vec![Capability {
            name: "answer.write".into(),
            inputs: vec![
                Port {
                    name: "question".into(),
                    contract: "demo.Question".into(),
                    ..Default::default()
                },
                Port {
                    name: "facts".into(),
                    contract: "demo.Facts".into(),
                    ..Default::default()
                },
            ],
            outputs: vec![Port {
                name: "answer".into(),
                contract: "demo.Answer".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let question = k.put_artifact(Artifact {
        r#type: "demo.Question".into(),
        body: b"What follows?".to_vec(),
        ..Default::default()
    }).unwrap();
    let assembly = Assembly {
        id: "answer-pipeline@1".into(),
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
                module_version: "2.0.0".into(),
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
    };
    let started = k
        .start_run(RunRequest {
            id: "run-1".into(),
            assembly: Some(assembly),
            inputs: vec![NamedArtifact {
                name: "question".into(),
                artifact: Some(question),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(started.state(), RunState::Running);

    // Writer is not ready: its facts input has not been produced.
    assert!(k
        .claim_ready(ClaimRequest {
            run_id: "run-1".into(),
            module: "writer".into(),
        })
        .unwrap()
        .id
        .is_empty());
    let extract = k
        .claim_ready(ClaimRequest {
            run_id: "run-1".into(),
            module: "extractor".into(),
        })
        .unwrap();
    let facts = k.put_artifact(Artifact {
        r#type: "demo.Facts".into(),
        body: b"typed flow".to_vec(),
        ..Default::default()
    }).unwrap();
    let progressed = k
        .commit(Derivation {
            run_id: "run-1".into(),
            work_id: extract.id,
            node_id: extract.node_id,
            outputs: vec![NamedArtifact {
                name: "facts".into(),
                artifact: Some(facts),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(progressed.state(), RunState::Running);

    let write = k
        .claim_ready(ClaimRequest {
            run_id: "run-1".into(),
            module: "writer".into(),
        })
        .unwrap();
    assert_eq!(write.inputs.len(), 2, "fan-in supplies both typed inputs");
    let answer = k.put_artifact(Artifact {
        r#type: "demo.Answer".into(),
        body: b"Modules converge.".to_vec(),
        ..Default::default()
    }).unwrap();
    let completed = k
        .commit(Derivation {
            run_id: "run-1".into(),
            work_id: write.id,
            node_id: write.node_id,
            outputs: vec![NamedArtifact {
                name: "answer".into(),
                artifact: Some(answer.clone()),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(completed.state(), RunState::Completed);
    assert_eq!(completed.answer.unwrap().id, answer.id);
    assert_eq!(
        k.list_derivations(&RunRef { id: "run-1".into() })
            .unwrap()
            .derivations
            .len(),
        2
    );
    assert!(matches!(
        k.claim_ready(ClaimRequest {
            run_id: "run-1".into(),
            module: "writer".into(),
        }),
        Err(KernelError::RunClosed(RunState::Completed))
    ));
}

#[test]
fn cyclic_assembly_is_rejected_before_it_can_expand_forever() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "loop".into(),
        version: "1.0.0".into(),
        provides: vec![Capability {
            name: "loop.step".into(),
            inputs: vec![Port {
                name: "in".into(),
                contract: "demo.Value".into(),
                optional: true,
                ..Default::default()
            }],
            outputs: vec![Port {
                name: "out".into(),
                contract: "demo.Value".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let result = k.start_run(RunRequest {
        id: "cycle".into(),
        assembly: Some(Assembly {
            id: "cycle@1".into(),
            nodes: vec![
                AssemblyNode {
                    id: "a".into(),
                    module: "loop".into(),
                    module_version: "1.0.0".into(),
                    capability: "loop.step".into(),
                },
                AssemblyNode {
                    id: "b".into(),
                    module: "loop".into(),
                    module_version: "1.0.0".into(),
                    capability: "loop.step".into(),
                },
            ],
            bindings: vec![
                Binding {
                    to_node: "a".into(),
                    to_port: "in".into(),
                    from_node: "b".into(),
                    from_port: "out".into(),
                    ..Default::default()
                },
                Binding {
                    to_node: "b".into(),
                    to_port: "in".into(),
                    from_node: "a".into(),
                    from_port: "out".into(),
                    ..Default::default()
                },
            ],
            terminal: Some(NodeOutput {
                node: "b".into(),
                port: "out".into(),
            }),
        }),
        ..Default::default()
    });
    assert!(matches!(result, Err(KernelError::Invalid(reason)) if reason.contains("cycle")));
}

#[test]
fn run_stalls_when_no_remaining_node_can_become_ready() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "source".into(),
        version: "1".into(),
        provides: vec![Capability {
            name: "source.maybe".into(),
            outputs: vec![Port {
                name: "value".into(),
                contract: "demo.Value".into(),
                optional: true,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    k.register(ModuleManifest {
        name: "sink".into(),
        version: "1".into(),
        provides: vec![Capability {
            name: "sink.answer".into(),
            inputs: vec![Port {
                name: "value".into(),
                contract: "demo.Value".into(),
                ..Default::default()
            }],
            outputs: vec![Port {
                name: "answer".into(),
                contract: "demo.Answer".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    k.start_run(RunRequest {
        id: "stall".into(),
        assembly: Some(Assembly {
            id: "stall@1".into(),
            nodes: vec![
                AssemblyNode {
                    id: "source".into(),
                    module: "source".into(),
                    module_version: "1".into(),
                    capability: "source.maybe".into(),
                },
                AssemblyNode {
                    id: "sink".into(),
                    module: "sink".into(),
                    module_version: "1".into(),
                    capability: "sink.answer".into(),
                },
            ],
            bindings: vec![Binding {
                to_node: "sink".into(),
                to_port: "value".into(),
                from_node: "source".into(),
                from_port: "value".into(),
                ..Default::default()
            }],
            terminal: Some(NodeOutput {
                node: "sink".into(),
                port: "answer".into(),
            }),
        }),
        ..Default::default()
    })
    .unwrap();
    let work = k
        .claim_ready(ClaimRequest {
            run_id: "stall".into(),
            module: "source".into(),
        })
        .unwrap();
    let stalled = k
        .commit(Derivation {
            run_id: "stall".into(),
            work_id: work.id,
            node_id: work.node_id,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(stalled.state(), RunState::Stalled);
}

#[test]
fn convergent_run_hashes_match_every_sdk() {
    const DERIVATION: &str =
        "sha256:0e3e167112e6bb8f19d736de4592b72a2856cb494cc4dcb00fbcd5682d595cf6";
    const LEDGER: &str = "faad7e3ce2d2e030cf37ff6001fe18f7dec0430ce14642f9ae878d66875bc28f";
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "answerer".into(),
        version: "1.0.0".into(),
        provides: vec![Capability {
            name: "answer.write".into(),
            outputs: vec![Port {
                name: "answer".into(),
                contract: "demo.Answer".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    k.start_run(RunRequest {
        id: "parity".into(),
        assembly: Some(Assembly {
            id: "single@1".into(),
            nodes: vec![AssemblyNode {
                id: "answer".into(),
                module: "answerer".into(),
                module_version: "1.0.0".into(),
                capability: "answer.write".into(),
            }],
            terminal: Some(NodeOutput {
                node: "answer".into(),
                port: "answer".into(),
            }),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let work = k
        .claim_ready(ClaimRequest {
            run_id: "parity".into(),
            module: "answerer".into(),
        })
        .unwrap();
    let answer = k.put_artifact(Artifact {
        r#type: "demo.Answer".into(),
        body: b"yes".to_vec(),
        ..Default::default()
    }).unwrap();
    k.commit(Derivation {
        run_id: "parity".into(),
        work_id: work.id,
        node_id: work.node_id,
        outputs: vec![NamedArtifact {
            name: "answer".into(),
            artifact: Some(answer),
        }],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(k.derivations()[0].id, DERIVATION);
    assert_eq!(k.ledger().last().unwrap().hash, LEDGER);
}

// 12b. WORK-UNIT FIRING — module ONCE_PER_KEY + inject ALWAYS; include_nodes.
#[test]
fn once_per_key_suppresses_duplicate_keys_and_include_nodes_filters() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "scanner".into(),
        version: "1".into(),
        provides: vec![Capability {
            name: "scan.host".into(),
            firing: Firing::OncePerKey as i32,
            inputs: vec![Port {
                name: "host".into(),
                contract: "demo.Host".into(),
                key: true,
                ..Default::default()
            }],
            outputs: vec![Port {
                name: "report".into(),
                contract: "demo.Report".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    k.register(ModuleManifest {
        name: "extra".into(),
        version: "1".into(),
        provides: vec![Capability {
            name: "extra.noop".into(),
            inputs: vec![Port {
                name: "host".into(),
                contract: "demo.Host".into(),
                ..Default::default()
            }],
            outputs: vec![Port {
                name: "side".into(),
                contract: "demo.Side".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let host_a = k
        .put_artifact(Artifact {
            r#type: "demo.Host".into(),
            body: b"10.0.0.1".to_vec(),
            ..Default::default()
        })
        .unwrap();
    let host_b = k
        .put_artifact(Artifact {
            r#type: "demo.Host".into(),
            body: b"10.0.0.2".to_vec(),
            ..Default::default()
        })
        .unwrap();
    k.start_run(RunRequest {
        id: "scan".into(),
        assembly: Some(Assembly {
            id: "scan@1".into(),
            nodes: vec![
                AssemblyNode {
                    id: "scan".into(),
                    module: "scanner".into(),
                    module_version: "1".into(),
                    capability: "scan.host".into(),
                },
                AssemblyNode {
                    id: "extra".into(),
                    module: "extra".into(),
                    module_version: "1".into(),
                    capability: "extra.noop".into(),
                },
            ],
            bindings: vec![
                Binding {
                    to_node: "scan".into(),
                    to_port: "host".into(),
                    input: "host".into(),
                    ..Default::default()
                },
                Binding {
                    to_node: "extra".into(),
                    to_port: "host".into(),
                    input: "host".into(),
                    ..Default::default()
                },
            ],
            terminal: Some(NodeOutput {
                node: "scan".into(),
                port: "report".into(),
            }),
            ..Default::default()
        }),
        include_nodes: vec!["scan".into()],
        inputs: vec![NamedArtifact {
            name: "host".into(),
            artifact: Some(host_a.clone()),
        }],
        policy: Some(ExecutionPolicy {
            closure: Closure::Open as i32,
            ..Default::default()
        }),
        limits: Some(Limits { max_steps: 10 }),
        ..Default::default()
    })
    .unwrap();

    let work = k
        .claim_ready(ClaimRequest {
            run_id: "scan".into(),
            module: "scanner".into(),
        })
        .unwrap();
    assert!(!work.id.is_empty());
    assert!(
        k.claim_ready(ClaimRequest {
            run_id: "scan".into(),
            module: "extra".into(),
        })
        .unwrap()
        .id
        .is_empty(),
        "include_nodes dropped extra"
    );
    let report = k
        .put_artifact(Artifact {
            r#type: "demo.Report".into(),
            body: b"a".to_vec(),
            ..Default::default()
        })
        .unwrap();
    let run = k
        .commit(Derivation {
            run_id: "scan".into(),
            work_id: work.id,
            node_id: work.node_id,
            outputs: vec![NamedArtifact {
                name: "report".into(),
                artifact: Some(report),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(run.state(), RunState::Running, "OPEN stays running");

    // Same host again → suppressed under ONCE_PER_KEY.
    k.inject_input(InjectInputRequest {
        run_id: "scan".into(),
        input: Some(NamedArtifact {
            name: "host".into(),
            artifact: Some(host_a),
        }),
    })
    .unwrap();
    assert!(
        k.claim_ready(ClaimRequest {
            run_id: "scan".into(),
            module: "scanner".into(),
        })
        .unwrap()
        .id
        .is_empty()
    );

    // New host → new work unit.
    k.inject_input(InjectInputRequest {
        run_id: "scan".into(),
        input: Some(NamedArtifact {
            name: "host".into(),
            artifact: Some(host_b),
        }),
    })
    .unwrap();
    let work2 = k
        .claim_ready(ClaimRequest {
            run_id: "scan".into(),
            module: "scanner".into(),
        })
        .unwrap();
    assert!(!work2.id.is_empty());
}

#[test]
fn always_refires_on_reinject_of_same_artifact() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "echo".into(),
        version: "1".into(),
        provides: vec![Capability {
            name: "echo.run".into(),
            firing: Firing::Always as i32,
            inputs: vec![Port {
                name: "in".into(),
                contract: "demo.In".into(),
                ..Default::default()
            }],
            outputs: vec![Port {
                name: "out".into(),
                contract: "demo.Out".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let value = k
        .put_artifact(Artifact {
            r#type: "demo.In".into(),
            body: b"same".to_vec(),
            ..Default::default()
        })
        .unwrap();
    k.start_run(RunRequest {
        id: "always".into(),
        assembly: Some(Assembly {
            id: "always@1".into(),
            nodes: vec![AssemblyNode {
                id: "echo".into(),
                module: "echo".into(),
                module_version: "1".into(),
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
            ..Default::default()
        }),
        inputs: vec![NamedArtifact {
            name: "in".into(),
            artifact: Some(value.clone()),
        }],
        policy: Some(ExecutionPolicy {
            closure: Closure::Open as i32,
            ..Default::default()
        }),
        limits: Some(Limits { max_steps: 10 }),
        ..Default::default()
    })
    .unwrap();

    for i in 0..2 {
        let work = k
            .claim_ready(ClaimRequest {
                run_id: "always".into(),
                module: "echo".into(),
            })
            .unwrap();
        assert!(!work.id.is_empty(), "fire {i}");
        let out = k
            .put_artifact(Artifact {
                r#type: "demo.Out".into(),
                body: format!("out-{i}").into_bytes(),
                ..Default::default()
            })
            .unwrap();
        k.commit(Derivation {
            run_id: "always".into(),
            work_id: work.id,
            node_id: work.node_id,
            outputs: vec![NamedArtifact {
                name: "out".into(),
                artifact: Some(out),
            }],
            ..Default::default()
        })
        .unwrap();
        k.inject_input(InjectInputRequest {
            run_id: "always".into(),
            input: Some(NamedArtifact {
                name: "in".into(),
                artifact: Some(value.clone()),
            }),
        })
        .unwrap();
    }
    assert_eq!(k.list_derivations(&RunRef { id: "always".into() }).unwrap().derivations.len(), 2);
}

// 12. PRODUCTION ARTIFACT BOUNDARY — inline small; external verified ObjectRef.
#[test]
fn blob_store_is_content_addressed_and_immutable() {
    let k = MemoryKernel::new();
    let data = b"pcap-or-apk-bytes-go-here".to_vec();

    let a = k.put_blob(PutBlobRequest {
        namespace: "evidence".into(),
        data: data.clone(),
    });
    assert_eq!(a.digest, blob_id(&data));
    assert_eq!(a.byte_count, data.len() as u64);
    assert_eq!(a.namespace, "evidence");

    let b = k.put_blob(PutBlobRequest {
        namespace: "evidence".into(),
        data: data.clone(),
    });
    assert_eq!(a.digest, b.digest);

    let got = k
        .get_blob(GetBlobRequest {
            digest: a.digest.clone(),
            namespace: "evidence".into(),
        })
        .unwrap();
    assert_eq!(got.data, data);

    let has = k.has_blob(HasBlobRequest {
        digest: a.digest.clone(),
        namespace: "evidence".into(),
    });
    assert!(has.exists);
    assert_eq!(has.byte_count, data.len() as u64);
    assert!(!k
        .has_blob(HasBlobRequest {
            digest: a.digest.clone(),
            namespace: "other".into(),
        })
        .exists);

    let entry = k.ledger().into_iter().find(|e| e.kind == "blob.put").unwrap();
    assert_eq!(entry.subject, a.digest);
    let ref_msg = BlobRef::decode(entry.detail.as_slice()).unwrap();
    assert_eq!(ref_msg.digest, a.digest);
    assert_eq!(ref_msg.namespace, "evidence");
}

#[test]
fn external_artifact_refs_large_data_without_inlining() {
    let k = MemoryKernel::new();
    let payload = b"EVIDENCE-BUNDLE-".repeat(64 * 1024);

    let blob = k.put_blob(PutBlobRequest {
        namespace: "observer".into(),
        data: payload.clone(),
    });
    let r#ref = k
        .put_artifact(Artifact {
            r#type: "observer.v1.Capture".into(),
            produced_by: "observer".into(),
            object: Some(ObjectRef {
                digest: blob.digest.clone(),
                byte_count: blob.byte_count,
                namespace: blob.namespace.clone(),
            }),
            ..Default::default()
        })
        .unwrap();

    let want = artifact_id(
        "observer.v1.Capture",
        &object_ref_bytes(&ObjectRef {
            digest: blob.digest.clone(),
            byte_count: blob.byte_count,
            namespace: blob.namespace.clone(),
        }),
    );
    assert_eq!(r#ref.id, want);
    assert_ne!(r#ref.id, blob_id(&payload));

    let got = k.get_artifact(&r#ref).unwrap();
    assert!(got.body.is_empty());
    let obj = got.object.as_ref().unwrap();
    assert_eq!(obj.digest, blob.digest);
    assert_eq!(obj.byte_count, blob.byte_count);

    let ref2 = k
        .put_artifact(Artifact {
            r#type: "observer.v1.Capture".into(),
            object: Some(ObjectRef {
                digest: blob.digest.clone(),
                byte_count: blob.byte_count,
                namespace: blob.namespace.clone(),
            }),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ref2.id, r#ref.id);

    let (ref3, blob3) = k
        .put_artifact_with_blob("observer.v1.Capture", "observer", &payload, "observer")
        .unwrap();
    assert_eq!(ref3.id, r#ref.id);
    assert_eq!(blob3.digest, blob.digest);

    let data = k
        .get_blob(GetBlobRequest {
            digest: obj.digest.clone(),
            namespace: obj.namespace.clone(),
        })
        .unwrap();
    assert_eq!(data.data, payload);

    let entry = k
        .ledger()
        .into_iter()
        .find(|e| e.kind == "artifact.put")
        .unwrap();
    let a = Artifact::decode(entry.detail.as_slice()).unwrap();
    assert!(a.body.is_empty());
    assert_eq!(a.object.as_ref().unwrap().digest, blob.digest);
}

#[test]
fn external_artifact_rejects_missing_or_mismatched_blob() {
    let k = MemoryKernel::new();
    let data = b"small-but-external".to_vec();
    let blob = k.put_blob(PutBlobRequest {
        namespace: "ns".into(),
        data: data.clone(),
    });

    let missing = k.put_artifact(Artifact {
        r#type: "t".into(),
        object: Some(ObjectRef {
            digest: blob_id(b"nope"),
            byte_count: 4,
            namespace: "ns".into(),
        }),
        ..Default::default()
    });
    assert!(matches!(missing, Err(KernelError::NotFound(_))));

    let bad_size = k.put_artifact(Artifact {
        r#type: "t".into(),
        object: Some(ObjectRef {
            digest: blob.digest.clone(),
            byte_count: blob.byte_count + 1,
            namespace: "ns".into(),
        }),
        ..Default::default()
    });
    assert!(matches!(bad_size, Err(KernelError::BlobIntegrity(_))));

    let both = k.put_artifact(Artifact {
        r#type: "t".into(),
        body: b"x".to_vec(),
        object: Some(ObjectRef {
            digest: blob.digest.clone(),
            byte_count: blob.byte_count,
            namespace: "ns".into(),
        }),
        ..Default::default()
    });
    assert!(matches!(both, Err(KernelError::Invalid(_))));

    let wrong_ns = k.put_artifact(Artifact {
        r#type: "t".into(),
        object: Some(ObjectRef {
            digest: blob.digest,
            byte_count: blob.byte_count,
            namespace: "other".into(),
        }),
        ..Default::default()
    });
    assert!(matches!(wrong_ns, Err(KernelError::NotFound(_))));
}

#[test]
fn value_identity_independent_of_blob_identity() {
    let k = MemoryKernel::new();
    let data = b"shared-raw-bytes".to_vec();
    let blob_a = k.put_blob(PutBlobRequest {
        namespace: "a".into(),
        data: data.clone(),
    });
    let blob_b = k.put_blob(PutBlobRequest {
        namespace: "b".into(),
        data: data.clone(),
    });
    assert_eq!(blob_a.digest, blob_b.digest);

    let art_a = k
        .put_artifact(Artifact {
            r#type: "t".into(),
            object: Some(ObjectRef {
                digest: blob_a.digest.clone(),
                byte_count: blob_a.byte_count,
                namespace: "a".into(),
            }),
            ..Default::default()
        })
        .unwrap();
    let art_b = k
        .put_artifact(Artifact {
            r#type: "t".into(),
            object: Some(ObjectRef {
                digest: blob_b.digest.clone(),
                byte_count: blob_b.byte_count,
                namespace: "b".into(),
            }),
            ..Default::default()
        })
        .unwrap();
    assert_ne!(art_a.id, art_b.id);

    let art_c = k
        .put_artifact(Artifact {
            r#type: "other".into(),
            object: Some(ObjectRef {
                digest: blob_a.digest,
                byte_count: blob_a.byte_count,
                namespace: "a".into(),
            }),
            ..Default::default()
        })
        .unwrap();
    assert_ne!(art_c.id, art_a.id);
}

// RequestContext: deadline rejects past absolute times; request_key de-duplicates
// PutArtifact / StartRun / Commit without re-applying side effects.
#[test]
fn request_context_deadline_and_idempotency() {
    let k = MemoryKernel::new();
    let past = RequestContext {
        deadline_unix_ms: 1, // 1970-01-01
        ..Default::default()
    };
    assert!(matches!(
        KernelApi::put_artifact(
            &k,
            Artifact {
                r#type: "t".into(),
                body: b"x".to_vec(),
                ..Default::default()
            },
            &past
        ),
        Err(KernelError::FailedPrecondition(_))
    ));

    let key_ctx = RequestContext {
        caller: "worker".into(),
        request_key: "put-once".into(),
        ..Default::default()
    };
    let a = KernelApi::put_artifact(
        &k,
        Artifact {
            r#type: "t".into(),
            body: b"unique-body".to_vec(),
            ..Default::default()
        },
        &key_ctx,
    )
    .unwrap();
    let before = k.ledger().len();
    let b = KernelApi::put_artifact(
        &k,
        Artifact {
            r#type: "t".into(),
            body: b"unique-body".to_vec(),
            ..Default::default()
        },
        &key_ctx,
    )
    .unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(
        k.ledger().len(),
        before,
        "idempotent replay must not append another ledger entry"
    );
}

// Transition is on the Kernel ABI (not MemoryKernel-only).
#[test]
fn transition_is_on_the_abi() {
    let k = MemoryKernel::new();
    k.register(ModuleManifest {
        name: "m".into(),
        version: "1".into(),
        ..Default::default()
    });
    let ack = KernelApi::transition(
        &k,
        TransitionRequest {
            module: "m".into(),
            to: Lifecycle::Loaded as i32,
        },
        &RequestContext::default(),
    )
    .unwrap();
    assert_eq!(ack.state, Lifecycle::Loaded as i32);
    let chain = k.ledger();
    assert!(chain.iter().any(|e| e.kind == "module.loaded"));
}
