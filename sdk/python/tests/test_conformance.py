"""The minimal conformance suite from SPEC.md §Conformance. An SDK is
conformant iff all invariants pass. Runs with the stdlib: ``python -m unittest``.
"""

import queue
import threading
import unittest

from srcport_substrate import (
    AppendRequest,
    Artifact,
    Assembly,
    AssemblyNode,
    Binding,
    Capability,
    ClaimRequest,
    Decision,
    Derivation,
    Event,
    GateBlocked,
    GateDecision,
    GateRequest,
    Kernel,
    ModuleManifest,
    NamedArtifact,
    NodeOutput,
    Port,
    RunRequest,
    RunRef,
    RunState,
    NotADecision,
    Subscription,
    artifact_id,
    verify_chain,
)


class Conformance(unittest.TestCase):
    # 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
    def test_addressing_is_content_derived_and_metamorphic(self):
        k = Kernel()
        a = k.put_artifact(
            Artifact(type="acme.recon.v1.Host", body=b"10.0.0.1", produced_by="recon")
        )
        b = k.put_artifact(
            Artifact(type="acme.recon.v1.Host", body=b"10.0.0.1", produced_by="other")
        )
        self.assertEqual(a.id, b.id, "same (type, body) must yield the same id")
        self.assertTrue(a.id.startswith("sha256:"))

        c = k.put_artifact(Artifact(type="acme.recon.v1.Host", body=b"10.0.0.2"))
        self.assertNotEqual(a.id, c.id, "a one-byte change must change the address")

        d = k.put_artifact(Artifact(type="acme.recon.v1.Port", body=b"10.0.0.1"))
        self.assertNotEqual(a.id, d.id, "type must participate in the address")

        self.assertEqual(a.id, artifact_id("acme.recon.v1.Host", b"10.0.0.1"))

    # 2. IMMUTABILITY — reads back byte-identical; a later put never mutates it.
    def test_artifacts_are_immutable(self):
        k = Kernel()
        r = k.put_artifact(Artifact(type="t", body=b"payload", meta={"first": "true"}))
        got = k.get_artifact(r)
        self.assertEqual(got.body, b"payload", "reads back byte-identical")
        self.assertEqual(got.meta.get("first"), "true")

        # A later put of the same content with different meta must NOT change
        # what is stored. First write wins.
        r2 = k.put_artifact(
            Artifact(type="t", body=b"payload", meta={"first": "false", "sneaky": "yes"})
        )
        self.assertEqual(r2.id, r.id, "same content ⇒ same id")
        after = k.get_artifact(r)
        self.assertEqual(after.meta.get("first"), "true", "stored value was mutated")
        self.assertNotIn("sneaky", after.meta, "stored value was mutated")

    # 3. ORDERING & ISOLATION — events reach exactly their subscribers, in seq
    #    order, and never reach non-subscribers.
    def test_events_are_ordered_and_isolated(self):
        k = Kernel()
        hosts = k.subscribe(Subscription(module="a", topics=["recon.host.found"]))
        ports = k.subscribe(Subscription(module="b", topics=["recon.port.found"]))

        s1 = k.publish(Event(topic="recon.host.found", payload=b"h1")).seq
        s2 = k.publish(Event(topic="recon.host.found", payload=b"h2")).seq
        s3 = k.publish(Event(topic="recon.port.found", payload=b"p1")).seq
        self.assertTrue(s1 < s2 < s3, "seq is monotonic across topics")

        e1 = hosts.get_nowait()
        e2 = hosts.get_nowait()
        self.assertEqual((e1.seq, e1.payload), (s1, b"h1"))
        self.assertEqual((e2.seq, e2.payload), (s2, b"h2"))
        self.assertTrue(e1.seq < e2.seq, "delivered in seq order")
        with self.assertRaises(queue.Empty):
            hosts.get_nowait()  # A never received the port event

        p = ports.get_nowait()
        self.assertEqual(p.seq, s3)
        with self.assertRaises(queue.Empty):
            ports.get_nowait()  # B never received the host events

    # 4. LEDGER INTEGRITY — the chain verifies; tampering breaks verification.
    def test_ledger_is_tamper_evident(self):
        k = Kernel()
        k.register(ModuleManifest(name="m", version="0.1.0"))
        k.put_artifact(Artifact(type="t", body=b"x"))
        k.append(AppendRequest(kind="domain.fact", subject="s", detail=b"d"))

        chain = k.ledger()
        self.assertGreaterEqual(len(chain), 3)
        self.assertTrue(k.verify_ledger())
        self.assertTrue(verify_chain(chain))

        tampered = k.ledger()
        tampered[1].subject = "hacked"
        self.assertFalse(verify_chain(tampered), "tampering breaks verification")

        spliced = k.ledger()
        del spliced[1]
        self.assertFalse(verify_chain(spliced), "removing an entry breaks the chain")

    # 5. GATE NON-BYPASS — blocked while PENDING/REJECTED; permitted only APPROVED.
    def test_gates_are_non_bypassable(self):
        k = Kernel()
        t = k.request_gate(GateRequest(action="delete production", requested_by="danger"))
        with self.assertRaises(GateBlocked) as cm:
            k.ensure_approved(t)
        self.assertEqual(cm.exception.decision, Decision.DECISION_PENDING)

        k.decide_gate(
            GateDecision(
                request_id=t.request_id, decision=Decision.DECISION_REJECTED, decided_by="phil"
            )
        )
        with self.assertRaises(GateBlocked) as cm:
            k.ensure_approved(t)
        self.assertEqual(cm.exception.decision, Decision.DECISION_REJECTED)

        t2 = k.request_gate(GateRequest(action="delete production"))
        with self.assertRaises(GateBlocked):
            k.ensure_approved(t2)
        k.decide_gate(
            GateDecision(
                request_id=t2.request_id, decision=Decision.DECISION_APPROVED, decided_by="phil"
            )
        )
        k.ensure_approved(t2)  # APPROVED permits — must not raise

        with self.assertRaises(NotADecision):
            k.decide_gate(
                GateDecision(request_id=t2.request_id, decision=Decision.DECISION_PENDING)
            )

    # 5b. await_gate really blocks until a human decides.
    def test_await_gate_blocks_until_decided(self):
        k = Kernel()
        t = k.request_gate(GateRequest(action="irreversible"))

        def decide():
            k.decide_gate(
                GateDecision(
                    request_id=t.request_id, decision=Decision.DECISION_APPROVED, decided_by="phil"
                )
            )

        threading.Timer(0.05, decide).start()
        decision = k.await_gate(t)
        self.assertEqual(decision.decision, Decision.DECISION_APPROVED)

    # 6. DISCOVERY — the registry reports every module, capability, and contract.
    def test_registry_reports_everything(self):
        k = Kernel()
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", contract="acme.recon.v1.ScanRequest")],
            )
        )
        k.register(
            ModuleManifest(
                name="report",
                version="0.2.0",
                provides=[Capability(name="report.render", contract="acme.report.v1.Report")],
                requires=["recon.scan"],
            )
        )

        snap = k.snapshot()
        names = {m.name for m in snap.modules}
        self.assertTrue({"recon", "report"} <= names)
        caps = {c.name for c in snap.capabilities}
        self.assertTrue({"recon.scan", "report.render"} <= caps)
        refs = {c.ref for c in snap.contracts}
        self.assertTrue({"acme.recon.v1.ScanRequest", "acme.report.v1.Report"} <= refs)

    # 7. LEDGER RECONSTRUCTION & CANONICAL DETAIL — a state-bearing entry's detail
    #    decodes to the message named for its kind and reproduces the original
    #    value, so the registry, the artifact store, and the approval record all
    #    round-trip from the tamper-evident chain alone. detail is folded into the
    #    entry hash, so forging it breaks verification.
    def test_ledger_reconstructs_state_from_detail(self):
        k = Kernel()
        r = k.put_artifact(
            Artifact(
                type="acme.recon.v1.Host",
                body=b"10.0.0.1",
                meta={"region": "eu", "scan": "full"},
                produced_by="recon",
                derived_from=["sha256:parent-a", "sha256:parent-b"],
            )
        )
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", contract="acme.recon.v1.ScanRequest")],
                requires=["report.render"],
            )
        )
        t = k.request_gate(
            GateRequest(action="delete production", context=b"rows=42", requested_by="danger")
        )
        k.decide_gate(
            GateDecision(
                request_id=t.request_id,
                decision=Decision.DECISION_APPROVED,
                decided_by="phil",
                reason="reviewed",
            )
        )

        chain = k.ledger()

        def detail_of(kind):
            return next(e for e in chain if e.kind == kind).detail

        # artifact.put reconstructs everything but the body; lineage rides along.
        a_entry = next(e for e in chain if e.kind == "artifact.put")
        a = Artifact()
        a.ParseFromString(a_entry.detail)
        self.assertEqual(a.id, r.id)
        self.assertEqual(a_entry.subject, r.id, "subject is the content address")
        self.assertEqual(a.type, "acme.recon.v1.Host")
        self.assertEqual(a.produced_by, "recon")
        self.assertEqual(dict(a.meta), {"region": "eu", "scan": "full"})
        self.assertEqual(
            list(a.derived_from),
            ["sha256:parent-a", "sha256:parent-b"],
            "derived_from lineage must round-trip through the ledger",
        )
        self.assertEqual(a.body, b"", "body is cleared — the id already addresses it")

        # module.registered reconstructs the whole manifest.
        m = ModuleManifest()
        m.ParseFromString(detail_of("module.registered"))
        self.assertEqual((m.name, m.version), ("recon", "0.1.0"))
        self.assertEqual(len(m.provides), 1)
        self.assertEqual(m.provides[0].name, "recon.scan")
        self.assertEqual(list(m.requires), ["report.render"])

        # gate.requested / gate.decided reconstruct who / what / why.
        req = GateRequest()
        req.ParseFromString(detail_of("gate.requested"))
        self.assertEqual(
            (req.action, req.requested_by, req.context),
            ("delete production", "danger", b"rows=42"),
        )
        dec = GateDecision()
        dec.ParseFromString(detail_of("gate.decided"))
        self.assertEqual(
            (dec.decision, dec.decided_by, dec.reason),
            (Decision.DECISION_APPROVED, "phil", "reviewed"),
        )

        self.assertTrue(k.verify_ledger(), "the chain with fat detail verifies")

        # The approval record is hash-committed: forging who approved it (by
        # re-encoding a different decider into detail) breaks verification.
        forged = k.ledger()
        i = next(i for i, e in enumerate(forged) if e.kind == "gate.decided")
        forged[i].detail = GateDecision(
            request_id=t.request_id,
            decision=Decision.DECISION_APPROVED,
            decided_by="attacker",
            reason="reviewed",
        ).SerializeToString(deterministic=True)
        self.assertFalse(verify_chain(forged), "rewriting the recorded decision breaks the chain")

    # 7c. CANONICAL DETAIL — the same logical value encodes to identical bytes, so
    #     ledger detail hashes reproducibly across runs and SDKs (sorted map keys).
    def test_ledger_detail_encodes_canonically(self):
        def build():
            return Artifact(
                type="t", meta={"z": "1", "a": "2", "m": "3", "b": "4"}
            ).SerializeToString(deterministic=True)

        for _ in range(64):
            self.assertEqual(
                build(), build(), "identical meta must encode to identical bytes (sorted keys)"
            )

    # METAMORPHIC — the address depends ONLY on (type, body). Transforming fields
    # that aren't identity (meta, produced_by, derived_from) is a known no-op: the
    # id must not move, or the address would be keyed on provenance, not content.
    def test_address_ignores_non_identity_fields(self):
        k = Kernel()
        base = k.put_artifact(Artifact(type="acme.recon.v1.Host", body=b"10.0.0.1"))
        enriched = k.put_artifact(
            Artifact(
                type="acme.recon.v1.Host",
                body=b"10.0.0.1",
                meta={"x": "y"},
                produced_by="whoever",
                derived_from=["sha256:some-parent"],
            )
        )
        self.assertEqual(
            enriched.id, base.id,
            "meta, produced_by, derived_from must not participate in the address",
        )
        self.assertEqual(enriched.id, artifact_id("acme.recon.v1.Host", b"10.0.0.1"))

    # CROSS-SDK KNOWN ANSWER — a fixed scenario must produce this exact final
    # ledger hash in every SDK. Go, Rust, and Python all assert the SAME constant,
    # so any drift in canonical detail encoding or the hash rule fails here and the
    # three chains are pinned to cross-verify. If it ever changes, it changes in
    # all three suites in lockstep — never one SDK alone.
    def test_ledger_hash_known_answer_cross_sdk(self):
        want = "985f4980bda5266d03b3e7092ef2bd9eb49b12107b43f17bbe00415deca4ab6a"

        k = Kernel()
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", contract="acme.recon.v1.ScanRequest")],
                requires=["report.render"],
            )
        )
        k.put_artifact(
            Artifact(
                type="acme.recon.v1.Host",
                body=b"10.0.0.1",
                meta={"region": "eu", "scan": "full"},
                produced_by="recon",
                derived_from=["sha256:parent-a", "sha256:parent-b"],
            )
        )
        t = k.request_gate(
            GateRequest(action="delete production", context=b"rows=42", requested_by="danger")
        )
        k.decide_gate(
            GateDecision(
                request_id=t.request_id,
                decision=Decision.DECISION_APPROVED,
                decided_by="phil",
                reason="reviewed",
            )
        )

        chain = k.ledger()
        self.assertTrue(k.verify_ledger(), "the chain must verify")
        self.assertEqual(chain[-1].hash, want, "cross-SDK ledger hash drift")

    def test_run_feeds_forward_and_closes_on_terminal_answer(self):
        k = Kernel()
        k.register(
            ModuleManifest(
                name="extractor",
                version="1.0.0",
                provides=[
                    Capability(
                        name="facts.extract",
                        inputs=[Port(name="question", contract="demo.Question")],
                        outputs=[Port(name="facts", contract="demo.Facts")],
                    )
                ],
            )
        )
        k.register(
            ModuleManifest(
                name="writer",
                version="2.0.0",
                provides=[
                    Capability(
                        name="answer.write",
                        inputs=[
                            Port(name="question", contract="demo.Question"),
                            Port(name="facts", contract="demo.Facts"),
                        ],
                        outputs=[Port(name="answer", contract="demo.Answer")],
                    )
                ],
            )
        )
        question = k.put_artifact(
            Artifact(type="demo.Question", body=b"What follows?")
        )
        assembly = Assembly(
            id="answer-pipeline@1",
            nodes=[
                AssemblyNode(
                    id="extract",
                    module="extractor",
                    module_version="1.0.0",
                    capability="facts.extract",
                ),
                AssemblyNode(
                    id="write",
                    module="writer",
                    module_version="2.0.0",
                    capability="answer.write",
                ),
            ],
            bindings=[
                Binding(to_node="extract", to_port="question", input="question"),
                Binding(to_node="write", to_port="question", input="question"),
                Binding(
                    to_node="write",
                    to_port="facts",
                    from_node="extract",
                    from_port="facts",
                ),
            ],
            terminal=NodeOutput(node="write", port="answer"),
        )
        run = k.start_run(
            RunRequest(
                id="run-1",
                assembly=assembly,
                inputs=[NamedArtifact(name="question", artifact=question)],
            )
        )
        self.assertEqual(run.state, RunState.RUN_STATE_RUNNING)
        self.assertFalse(
            k.claim_ready(ClaimRequest(run_id="run-1", module="writer")).id
        )
        extract = k.claim_ready(
            ClaimRequest(run_id="run-1", module="extractor")
        )
        facts = k.put_artifact(Artifact(type="demo.Facts", body=b"typed flow"))
        k.commit(
            Derivation(
                run_id="run-1",
                work_id=extract.id,
                node_id=extract.node_id,
                outputs=[NamedArtifact(name="facts", artifact=facts)],
            )
        )
        write = k.claim_ready(ClaimRequest(run_id="run-1", module="writer"))
        self.assertEqual(len(write.inputs), 2, "fan-in supplies both inputs")
        answer = k.put_artifact(
            Artifact(type="demo.Answer", body=b"Modules converge.")
        )
        completed = k.commit(
            Derivation(
                run_id="run-1",
                work_id=write.id,
                node_id=write.node_id,
                outputs=[NamedArtifact(name="answer", artifact=answer)],
            )
        )
        self.assertEqual(completed.state, RunState.RUN_STATE_COMPLETED)
        self.assertEqual(completed.answer.id, answer.id)
        self.assertEqual(
            len(k.list_derivations(RunRef(id="run-1")).derivations), 2
        )

    def test_cyclic_assembly_is_rejected(self):
        from srcport_substrate import Invalid

        k = Kernel()
        k.register(
            ModuleManifest(
                name="loop",
                version="1.0.0",
                provides=[
                    Capability(
                        name="loop.step",
                        inputs=[
                            Port(
                                name="in", contract="demo.Value", optional=True
                            )
                        ],
                        outputs=[Port(name="out", contract="demo.Value")],
                    )
                ],
            )
        )
        assembly = Assembly(
            id="cycle@1",
            nodes=[
                AssemblyNode(
                    id="a",
                    module="loop",
                    module_version="1.0.0",
                    capability="loop.step",
                ),
                AssemblyNode(
                    id="b",
                    module="loop",
                    module_version="1.0.0",
                    capability="loop.step",
                ),
            ],
            bindings=[
                Binding(
                    to_node="a", to_port="in", from_node="b", from_port="out"
                ),
                Binding(
                    to_node="b", to_port="in", from_node="a", from_port="out"
                ),
            ],
            terminal=NodeOutput(node="b", port="out"),
        )
        with self.assertRaises(Invalid):
            k.start_run(RunRequest(id="cycle", assembly=assembly))

    def test_run_stalls_when_no_node_can_become_ready(self):
        k = Kernel()
        k.register(ModuleManifest(name="source", version="1", provides=[Capability(name="source.maybe", outputs=[Port(name="value", contract="demo.Value", optional=True)])]))
        k.register(ModuleManifest(name="sink", version="1", provides=[Capability(name="sink.answer", inputs=[Port(name="value", contract="demo.Value")], outputs=[Port(name="answer", contract="demo.Answer")])]))
        assembly = Assembly(id="stall@1", nodes=[AssemblyNode(id="source", module="source", module_version="1", capability="source.maybe"), AssemblyNode(id="sink", module="sink", module_version="1", capability="sink.answer")], bindings=[Binding(to_node="sink", to_port="value", from_node="source", from_port="value")], terminal=NodeOutput(node="sink", port="answer"))
        k.start_run(RunRequest(id="stall", assembly=assembly))
        work = k.claim_ready(ClaimRequest(run_id="stall", module="source"))
        run = k.commit(Derivation(run_id="stall", work_id=work.id, node_id=work.node_id))
        self.assertEqual(run.state, RunState.RUN_STATE_STALLED)

    def test_convergent_run_hashes_match_every_sdk(self):
        k = Kernel()
        k.register(ModuleManifest(name="answerer", version="1.0.0", provides=[Capability(name="answer.write", outputs=[Port(name="answer", contract="demo.Answer")])]))
        k.start_run(RunRequest(id="parity", assembly=Assembly(id="single@1", nodes=[AssemblyNode(id="answer", module="answerer", module_version="1.0.0", capability="answer.write")], terminal=NodeOutput(node="answer", port="answer"))))
        work = k.claim_ready(ClaimRequest(run_id="parity", module="answerer"))
        answer = k.put_artifact(Artifact(type="demo.Answer", body=b"yes"))
        k.commit(Derivation(run_id="parity", work_id=work.id, node_id=work.node_id, outputs=[NamedArtifact(name="answer", artifact=answer)]))
        self.assertEqual(k.derivations()[0].id, "sha256:0e3e167112e6bb8f19d736de4592b72a2856cb494cc4dcb00fbcd5682d595cf6")
        self.assertEqual(k.ledger()[-1].hash, "faad7e3ce2d2e030cf37ff6001fe18f7dec0430ce14642f9ae878d66875bc28f")


if __name__ == "__main__":
    unittest.main()
