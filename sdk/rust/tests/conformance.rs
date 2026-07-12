//! The minimal conformance suite from `SPEC.md` §Conformance. An SDK is
//! conformant iff all of them pass. Each test names the primitive and invariant
//! it pins down. If you widen the contract, add tests here — never weaken these.

use std::collections::HashMap;
use std::thread;

use srcport_substrate::*;

// 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
#[test]
fn addressing_is_content_derived_and_metamorphic() {
    let k = Kernel::new();

    let a = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        produced_by: "recon".into(),
        ..Default::default()
    });
    // Identical content, different producer/meta — must land the SAME address.
    let b = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.1".to_vec(),
        produced_by: "someone-else".into(),
        ..Default::default()
    });
    assert_eq!(a.id, b.id, "same (type, body) must yield the same id");
    assert!(a.id.starts_with("sha256:"));

    // One byte different in the body — must yield a DIFFERENT address.
    let c = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Host".into(),
        body: b"10.0.0.2".to_vec(),
        ..Default::default()
    });
    assert_ne!(a.id, c.id, "a one-byte change must change the address");

    // Type participates in the address too.
    let d = k.put_artifact(Artifact {
        r#type: "acme.recon.v1.Port".into(),
        body: b"10.0.0.1".to_vec(),
        ..Default::default()
    });
    assert_ne!(a.id, d.id, "type must participate in the address");

    // Pure function agrees with the kernel.
    assert_eq!(a.id, artifact_id("acme.recon.v1.Host", b"10.0.0.1"));
}

// 2. IMMUTABILITY — reads back byte-identical; a later put never mutates it.
#[test]
fn artifacts_are_immutable() {
    let k = Kernel::new();

    let mut meta = HashMap::new();
    meta.insert("first".into(), "true".into());
    let r = k.put_artifact(Artifact {
        r#type: "t".into(),
        body: b"payload".to_vec(),
        meta,
        ..Default::default()
    });

    let got = k.get_artifact(&r).unwrap();
    assert_eq!(got.body, b"payload", "reads back byte-identical");
    assert_eq!(got.meta.get("first").map(String::as_str), Some("true"));

    // A later put of the same content (same id) with different meta must NOT
    // change what is stored. First write wins.
    let mut meta2 = HashMap::new();
    meta2.insert("first".into(), "false".into());
    meta2.insert("sneaky".into(), "yes".into());
    let r2 = k.put_artifact(Artifact {
        r#type: "t".into(),
        body: b"payload".to_vec(),
        meta: meta2,
        ..Default::default()
    });
    assert_eq!(r2.id, r.id, "same content ⇒ same id");

    let after = k.get_artifact(&r).unwrap();
    assert_eq!(after.meta.get("first").map(String::as_str), Some("true"));
    assert!(!after.meta.contains_key("sneaky"), "stored value was not mutated");
}

// 3. ORDERING & ISOLATION — events reach exactly their subscribers, in seq
//    order, and never reach non-subscribers.
#[test]
fn events_are_ordered_and_isolated() {
    let k = Kernel::new();

    let hosts = k.subscribe(Subscription {
        module: "a".into(),
        topics: vec!["recon.host.found".into()],
    });
    let ports = k.subscribe(Subscription {
        module: "b".into(),
        topics: vec!["recon.port.found".into()],
    });

    let s1 = k
        .publish(Event {
            topic: "recon.host.found".into(),
            payload: b"h1".to_vec(),
            ..Default::default()
        })
        .seq;
    let s2 = k
        .publish(Event {
            topic: "recon.host.found".into(),
            payload: b"h2".to_vec(),
            ..Default::default()
        })
        .seq;
    let s3 = k
        .publish(Event {
            topic: "recon.port.found".into(),
            payload: b"p1".to_vec(),
            ..Default::default()
        })
        .seq;

    // Monotonic total order.
    assert!(s1 < s2 && s2 < s3, "seq is monotonic across all topics");

    // Subscriber A got exactly its two host events, in seq order...
    let e1 = hosts.try_recv().unwrap();
    let e2 = hosts.try_recv().unwrap();
    assert_eq!((e1.seq, &e1.payload[..]), (s1, &b"h1"[..]));
    assert_eq!((e2.seq, &e2.payload[..]), (s2, &b"h2"[..]));
    assert!(e1.seq < e2.seq, "delivered in seq order");
    assert!(hosts.try_recv().is_err(), "A never received the port event");

    // ...and subscriber B got exactly the one port event.
    let p = ports.try_recv().unwrap();
    assert_eq!(p.seq, s3);
    assert!(ports.try_recv().is_err(), "B never received the host events");
}

// 4. LEDGER INTEGRITY — the chain verifies; tampering breaks verification.
#[test]
fn ledger_is_tamper_evident() {
    let k = Kernel::new();
    k.register(ModuleManifest {
        name: "m".into(),
        version: "0.1.0".into(),
        ..Default::default()
    });
    k.put_artifact(Artifact {
        r#type: "t".into(),
        body: b"x".to_vec(),
        ..Default::default()
    });
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
    assert!(!verify_chain(&spliced), "removing an entry breaks the chain");
}

// 5. GATE NON-BYPASS — blocked while PENDING/REJECTED; permitted only APPROVED.
#[test]
fn gates_are_non_bypassable() {
    let k = Kernel::new();

    // PENDING blocks.
    let t = k.request_gate(GateRequest {
        action: "delete production".into(),
        requested_by: "danger-module".into(),
        ..Default::default()
    });
    assert_eq!(
        k.ensure_approved(&t),
        Err(KernelError::GateBlocked(Decision::Pending)),
        "an irreversible act is blocked while PENDING"
    );

    // REJECTED still blocks.
    k.decide_gate(GateDecision {
        request_id: t.request_id.clone(),
        decision: Decision::Rejected as i32,
        decided_by: "phil".into(),
        reason: "no".into(),
    })
    .unwrap();
    assert_eq!(
        k.ensure_approved(&t),
        Err(KernelError::GateBlocked(Decision::Rejected)),
        "REJECTED blocks too"
    );

    // A fresh gate, APPROVED, permits — and only then.
    let t2 = k.request_gate(GateRequest {
        action: "delete production".into(),
        ..Default::default()
    });
    assert!(k.ensure_approved(&t2).is_err());
    k.decide_gate(GateDecision {
        request_id: t2.request_id.clone(),
        decision: Decision::Approved as i32,
        decided_by: "phil".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(k.ensure_approved(&t2).is_ok(), "APPROVED permits the act");

    // A non-decision (PENDING/UNSPECIFIED) is rejected at the ABI.
    assert_eq!(
        k.decide_gate(GateDecision {
            request_id: t2.request_id.clone(),
            decision: Decision::Pending as i32,
            ..Default::default()
        }),
        Err(KernelError::NotADecision)
    );
}

// 5b. AwaitGate really blocks until a human decides (exercises the condvar).
#[test]
fn await_gate_blocks_until_decided() {
    use std::sync::Arc;
    let k = Arc::new(Kernel::new());
    let t = k.request_gate(GateRequest {
        action: "irreversible".into(),
        ..Default::default()
    });

    let k2 = Arc::clone(&k);
    let id = t.request_id.clone();
    let decider = thread::spawn(move || {
        // Decide from another thread; the waiter must wake.
        k2.decide_gate(GateDecision {
            request_id: id,
            decision: Decision::Approved as i32,
            decided_by: "phil".into(),
            ..Default::default()
        })
        .unwrap();
    });

    let decision = k.await_gate(&t).unwrap();
    assert_eq!(decision.decision(), Decision::Approved);
    decider.join().unwrap();
}

// 7. LEDGER RECONSTRUCTION & CANONICAL DETAIL — a gate's request and decision
//    round-trip from the tamper-evident chain alone (`detail` carries the
//    canonical message), and forging the recorded decision breaks verification.
#[test]
fn gate_request_and_decision_are_in_the_chain() {
    let k = Kernel::new();

    let t = k.request_gate(GateRequest {
        action: "delete production".into(),
        context: b"rows=42".to_vec(),
        requested_by: "danger-module".into(),
        ..Default::default()
    });
    k.decide_gate(GateDecision {
        request_id: t.request_id.clone(),
        decision: Decision::Approved as i32,
        decided_by: "phil".into(),
        reason: "reviewed the evidence".into(),
    })
    .unwrap();

    let chain = k.ledger();

    // The request reconstructs from the chain: action, requester, and evidence.
    let req_entry = chain.iter().find(|e| e.kind == "gate.requested").unwrap();
    let req = GateRequest::decode(&req_entry.detail[..]).unwrap();
    assert_eq!(req.action, "delete production");
    assert_eq!(req.requested_by, "danger-module");
    assert_eq!(req.context, b"rows=42");

    // The decision reconstructs too: who decided, what, and why.
    let dec_entry = chain.iter().find(|e| e.kind == "gate.decided").unwrap();
    let dec = GateDecision::decode(&dec_entry.detail[..]).unwrap();
    assert_eq!(dec.decision(), Decision::Approved);
    assert_eq!(dec.decided_by, "phil");
    assert_eq!(dec.reason, "reviewed the evidence");

    assert!(k.verify_ledger(), "the chain with fat detail still verifies");

    // The approval record is now hash-committed: forging who approved it (by
    // re-encoding a different decider into `detail`) breaks verification.
    let mut forged = chain.clone();
    let idx = forged.iter().position(|e| e.kind == "gate.decided").unwrap();
    forged[idx].detail = GateDecision {
        request_id: t.request_id.clone(),
        decision: Decision::Approved as i32,
        decided_by: "attacker".into(),
        reason: "reviewed the evidence".into(),
    }
    .encode_to_vec();
    assert!(
        !verify_chain(&forged),
        "rewriting the recorded decision must break the chain"
    );
}

// 6. DISCOVERY — the registry reports every module, capability, and contract.
#[test]
fn registry_reports_everything() {
    let k = Kernel::new();
    k.register(ModuleManifest {
        name: "recon".into(),
        version: "0.1.0".into(),
        provides: vec![Capability {
            name: "recon.scan".into(),
            contract: "acme.recon.v1.ScanRequest".into(),
        }],
        requires: vec![],
    });
    k.register(ModuleManifest {
        name: "report".into(),
        version: "0.2.0".into(),
        provides: vec![Capability {
            name: "report.render".into(),
            contract: "acme.report.v1.Report".into(),
        }],
        requires: vec!["recon.scan".into()],
    });

    let snap = k.snapshot();

    let names: Vec<_> = snap.modules.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"recon") && names.contains(&"report"));

    let caps: Vec<_> = snap.capabilities.iter().map(|c| c.name.as_str()).collect();
    assert!(caps.contains(&"recon.scan") && caps.contains(&"report.render"));

    let contracts: Vec<_> = snap.contracts.iter().map(|c| c.r#ref.as_str()).collect();
    assert!(contracts.contains(&"acme.recon.v1.ScanRequest"));
    assert!(contracts.contains(&"acme.report.v1.Report"));
}
