"""The minimal conformance suite from SPEC.md §Conformance. An SDK is
conformant iff all six pass. Runs with the stdlib: ``python -m unittest``.
"""

import queue
import threading
import unittest

from srcport_substrate import (
    AppendRequest,
    Artifact,
    Capability,
    Decision,
    Event,
    GateBlocked,
    GateDecision,
    GateRequest,
    Kernel,
    ModuleManifest,
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


if __name__ == "__main__":
    unittest.main()
