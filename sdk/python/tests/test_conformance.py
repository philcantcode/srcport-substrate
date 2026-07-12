"""The minimal conformance suite from SPEC.md §Conformance. An SDK is
conformant iff all invariants pass. Runs with the stdlib: ``python -m unittest``.
"""

import queue
import unittest

from srcport_substrate import (
    AppendRequest,
    Artifact,
    Assembly,
    AssemblyNode,
    Binding,
    BlobIntegrity,
    BlobRef,
    Capability,
    ClaimRequest,
    Conflict,
    Contract,
    Derivation,
    Event,
    FailedPrecondition,
    GetBlobRequest,
    HasBlobRequest,
    Invalid,
    Lifecycle,
    MemoryKernel,
    ModuleManifest,
    NamedArtifact,
    NodeOutput,
    NotFound,
    ObjectRef,
    Port,
    PutBlobRequest,
    RequestContext,
    RunRequest,
    RunRef,
    RunState,
    Subscription,
    TransitionRequest,
    artifact_id,
    blob_id,
    contract_digest,
    is_contract_placeholder,
    object_ref_bytes,
    verify_chain,
)


class Conformance(unittest.TestCase):
    # 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
    def test_addressing_is_content_derived_and_metamorphic(self):
        k = MemoryKernel()
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
        k = MemoryKernel()
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
        k = MemoryKernel()
        hosts = k.subscribe(Subscription(module="a", topics=["recon.host.found"]))
        ports = k.subscribe(Subscription(module="b", topics=["recon.port.found"]))

        h1 = k.put_artifact(Artifact(type="acme.recon.v1.Host", body=b"h1"))
        h2 = k.put_artifact(Artifact(type="acme.recon.v1.Host", body=b"h2"))
        p1 = k.put_artifact(Artifact(type="acme.recon.v1.Port", body=b"p1"))
        s1 = k.publish(
            Event(topic="recon.host.found", type="acme.recon.v1.Host", artifacts=[h1])
        ).seq
        s2 = k.publish(
            Event(topic="recon.host.found", type="acme.recon.v1.Host", artifacts=[h2])
        ).seq
        s3 = k.publish(
            Event(topic="recon.port.found", type="acme.recon.v1.Port", artifacts=[p1])
        ).seq
        self.assertTrue(s1 < s2 < s3, "seq is monotonic across topics")

        e1 = hosts.get_nowait()
        e2 = hosts.get_nowait()
        self.assertEqual(e1.seq, s1)
        self.assertEqual(e1.artifacts[0].id, h1.id)
        self.assertEqual(e2.seq, s2)
        self.assertEqual(e2.artifacts[0].id, h2.id)
        self.assertTrue(e1.seq < e2.seq, "delivered in seq order")
        with self.assertRaises(queue.Empty):
            hosts.get_nowait()  # A never received the port event

        p = ports.get_nowait()
        self.assertEqual(p.seq, s3)
        self.assertEqual(p.artifacts[0].id, p1.id)
        with self.assertRaises(queue.Empty):
            ports.get_nowait()  # B never received the host events

    # 4. LEDGER INTEGRITY — the chain verifies; tampering breaks verification.
    def test_ledger_is_tamper_evident(self):
        k = MemoryKernel()
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

    # 6. DISCOVERY — the registry reports every module, capability, and contract.
    def test_registry_reports_everything(self):
        k = MemoryKernel()
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", outputs=[Port(name="out", contract="acme.recon.v1.Host")])],
            )
        )
        k.register(
            ModuleManifest(
                name="report",
                version="0.2.0",
                provides=[Capability(name="report.render", outputs=[Port(name="out", contract="acme.report.v1.Report")])],
                requires=["recon.scan"],
            )
        )

        snap = k.snapshot()
        names = {m.name for m in snap.modules}
        self.assertTrue({"recon", "report"} <= names)
        caps = {c.name for c in snap.capabilities}
        self.assertTrue({"recon.scan", "report.render"} <= caps)
        refs = {c.ref for c in snap.contracts}
        self.assertTrue({"acme.recon.v1.Host", "acme.report.v1.Report"} <= refs)

    # 6b. CONTRACT IDENTITY — content-addressed under ref; immutable; conflict on
    # redefinition; placeholder fill-once; ports bind to the pinned identity.
    def test_contracts_are_immutable_and_identifiable(self):
        k = MemoryKernel()

        stored = k.put_contract(
            Contract(
                ref="acme.Host",
                media_type="application/schema+json",
                schema='{"type":"object"}',
                version="1.0.0",
                compatible_with=["acme.Host.v0", "acme.legacy.Host"],
            )
        )
        want = contract_digest(
            "application/schema+json",
            '{"type":"object"}',
            "1.0.0",
            ["acme.Host.v0", "acme.legacy.Host"],
        )
        self.assertEqual(stored.digest, want)
        self.assertEqual(list(stored.compatible_with), ["acme.Host.v0", "acme.legacy.Host"])

        # Identical re-put (unsorted compatible_with) is idempotent.
        again = k.put_contract(
            Contract(
                ref="acme.Host",
                media_type="application/schema+json",
                schema='{"type":"object"}',
                version="1.0.0",
                compatible_with=["acme.legacy.Host", "acme.Host.v0"],
            )
        )
        self.assertEqual(again.digest, stored.digest)

        with self.assertRaises(Conflict):
            k.put_contract(
                Contract(
                    ref="acme.Host",
                    media_type="application/schema+json",
                    schema='{"type":"string"}',
                    version="1.0.0",
                )
            )

        # Register creates a name-only placeholder; PutContract may fill it once.
        k.register(
            ModuleManifest(
                name="mod",
                version="1",
                provides=[Capability(name="do", outputs=[Port(name="out", contract="acme.NewThing")])],
            )
        )
        filled = k.put_contract(
            Contract(
                ref="acme.NewThing",
                media_type="text/x-protobuf",
                schema="message NewThing {}",
                version="1",
            )
        )
        self.assertFalse(is_contract_placeholder(filled))
        self.assertTrue(filled.digest)

        with self.assertRaises(Conflict):
            k.put_contract(
                Contract(
                    ref="acme.NewThing",
                    media_type="text/x-protobuf",
                    schema="message Other {}",
                    version="1",
                )
            )

        with self.assertRaises(Invalid):
            k.put_contract(Contract(ref="acme.Other", schema="x", digest="sha256:deadbeef"))

        # contract.registered lands in the ledger with reconstructable detail.
        found = False
        for e in k.ledger():
            if e.kind == "contract.registered" and e.subject == "acme.Host":
                c = Contract()
                c.ParseFromString(e.detail)
                self.assertEqual(c.ref, "acme.Host")
                self.assertEqual(c.digest, want)
                found = True
        self.assertTrue(found, "contract.registered must appear in the ledger")

    # 7. LEDGER RECONSTRUCTION & CANONICAL DETAIL — a state-bearing entry's detail
    #    decodes to the message named for its kind and reproduces the original
    #    value, so the registry, the artifact store, and the approval record all
    #    round-trip from the tamper-evident chain alone. detail is folded into the
    #    entry hash, so forging it breaks verification.
    def test_ledger_reconstructs_state_from_detail(self):
        k = MemoryKernel()
        r = k.put_artifact(
            Artifact(
                type="acme.recon.v1.Host",
                body=b"10.0.0.1",
                meta={"region": "eu", "scan": "full"},
                produced_by="recon",
            )
        )
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", outputs=[Port(name="out", contract="acme.recon.v1.Host")])],
                requires=["report.render"],
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
        self.assertEqual(a.body, b"", "body is cleared — the id already addresses it")

        # module.registered reconstructs the whole manifest.
        m = ModuleManifest()
        m.ParseFromString(detail_of("module.registered"))
        self.assertEqual((m.name, m.version), ("recon", "0.1.0"))
        self.assertEqual(len(m.provides), 1)
        self.assertEqual(m.provides[0].name, "recon.scan")
        self.assertEqual(list(m.requires), ["report.render"])


        self.assertTrue(k.verify_ledger(), "the chain with fat detail verifies")


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
    # that aren't identity (meta, produced_by) is a known no-op: the
    # id must not move, or the address would be keyed on provenance, not content.
    def test_address_ignores_non_identity_fields(self):
        k = MemoryKernel()
        base = k.put_artifact(Artifact(type="acme.recon.v1.Host", body=b"10.0.0.1"))
        enriched = k.put_artifact(
            Artifact(
                type="acme.recon.v1.Host",
                body=b"10.0.0.1",
                meta={"x": "y"},
                produced_by="whoever",
            )
        )
        self.assertEqual(
            enriched.id, base.id,
            "meta and produced_by must not participate in the address",
        )
        self.assertEqual(enriched.id, artifact_id("acme.recon.v1.Host", b"10.0.0.1"))

    # CROSS-SDK KNOWN ANSWER — a fixed scenario must produce this exact final
    # ledger hash in every SDK. Go, Rust, and Python all assert the SAME constant,
    # so any drift in canonical detail encoding or the hash rule fails here and the
    # three chains are pinned to cross-verify. If it ever changes, it changes in
    # all three suites in lockstep — never one SDK alone.
    def test_ledger_hash_known_answer_cross_sdk(self):
        want = "5d9dea28f0fa779b7d76dd6137c9b6079561289b12ed6dff022a889b94d69cd2"

        k = MemoryKernel()
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", outputs=[Port(name="host", contract="acme.recon.v1.Host")])],
                requires=["report.render"],
            )
        )
        k.put_artifact(
            Artifact(
                type="acme.recon.v1.Host",
                body=b"10.0.0.1",
                meta={"region": "eu", "scan": "full"},
                produced_by="recon",
            )
        )
        chain = k.ledger()
        self.assertTrue(k.verify_ledger(), "the chain must verify")
        self.assertEqual(chain[-1].hash, want, "cross-SDK ledger hash drift")

    def test_run_feeds_forward_and_closes_on_terminal_answer(self):
        k = MemoryKernel()
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

        k = MemoryKernel()
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
        k = MemoryKernel()
        k.register(ModuleManifest(name="source", version="1", provides=[Capability(name="source.maybe", outputs=[Port(name="value", contract="demo.Value", optional=True)])]))
        k.register(ModuleManifest(name="sink", version="1", provides=[Capability(name="sink.answer", inputs=[Port(name="value", contract="demo.Value")], outputs=[Port(name="answer", contract="demo.Answer")])]))
        assembly = Assembly(id="stall@1", nodes=[AssemblyNode(id="source", module="source", module_version="1", capability="source.maybe"), AssemblyNode(id="sink", module="sink", module_version="1", capability="sink.answer")], bindings=[Binding(to_node="sink", to_port="value", from_node="source", from_port="value")], terminal=NodeOutput(node="sink", port="answer"))
        k.start_run(RunRequest(id="stall", assembly=assembly))
        work = k.claim_ready(ClaimRequest(run_id="stall", module="source"))
        run = k.commit(Derivation(run_id="stall", work_id=work.id, node_id=work.node_id))
        self.assertEqual(run.state, RunState.RUN_STATE_STALLED)

    def test_convergent_run_hashes_match_every_sdk(self):
        k = MemoryKernel()
        k.register(ModuleManifest(name="answerer", version="1.0.0", provides=[Capability(name="answer.write", outputs=[Port(name="answer", contract="demo.Answer")])]))
        k.start_run(RunRequest(id="parity", assembly=Assembly(id="single@1", nodes=[AssemblyNode(id="answer", module="answerer", module_version="1.0.0", capability="answer.write")], terminal=NodeOutput(node="answer", port="answer"))))
        work = k.claim_ready(ClaimRequest(run_id="parity", module="answerer"))
        answer = k.put_artifact(Artifact(type="demo.Answer", body=b"yes"))
        k.commit(Derivation(run_id="parity", work_id=work.id, node_id=work.node_id, outputs=[NamedArtifact(name="answer", artifact=answer)]))
        self.assertEqual(k.derivations()[0].id, "sha256:0e3e167112e6bb8f19d736de4592b72a2856cb494cc4dcb00fbcd5682d595cf6")
        self.assertEqual(k.ledger()[-1].hash, "faad7e3ce2d2e030cf37ff6001fe18f7dec0430ce14642f9ae878d66875bc28f")

    # 12. PRODUCTION ARTIFACT BOUNDARY — inline small; external verified ObjectRef.
    def test_blob_store_is_content_addressed_and_immutable(self):
        k = MemoryKernel()
        data = b"pcap-or-apk-bytes-go-here"
        a = k.put_blob(PutBlobRequest(namespace="evidence", data=data))
        self.assertEqual(a.digest, blob_id(data))
        self.assertEqual(a.byte_count, len(data))
        self.assertEqual(a.namespace, "evidence")

        b = k.put_blob(PutBlobRequest(namespace="evidence", data=data))
        self.assertEqual(a.digest, b.digest)

        got = k.get_blob(GetBlobRequest(digest=a.digest, namespace="evidence"))
        self.assertEqual(got.data, data)

        has = k.has_blob(HasBlobRequest(digest=a.digest, namespace="evidence"))
        self.assertTrue(has.exists)
        self.assertEqual(has.byte_count, len(data))
        self.assertFalse(
            k.has_blob(HasBlobRequest(digest=a.digest, namespace="other")).exists
        )

        entry = next(e for e in k.ledger() if e.kind == "blob.put")
        self.assertEqual(entry.subject, a.digest)
        ref = BlobRef()
        ref.ParseFromString(entry.detail)
        self.assertEqual(ref.digest, a.digest)
        self.assertEqual(ref.namespace, "evidence")

    def test_external_artifact_refs_large_data_without_inlining(self):
        k = MemoryKernel()
        payload = b"EVIDENCE-BUNDLE-" * (64 * 1024)

        blob = k.put_blob(PutBlobRequest(namespace="observer", data=payload))
        ref = k.put_artifact(
            Artifact(
                type="observer.v1.Capture",
                produced_by="observer",
                object=ObjectRef(
                    digest=blob.digest,
                    byte_count=blob.byte_count,
                    namespace=blob.namespace,
                ),
            )
        )
        want = artifact_id(
            "observer.v1.Capture",
            object_ref_bytes(
                ObjectRef(
                    digest=blob.digest,
                    byte_count=blob.byte_count,
                    namespace=blob.namespace,
                )
            ),
        )
        self.assertEqual(ref.id, want)
        self.assertNotEqual(ref.id, blob_id(payload))

        got = k.get_artifact(ref)
        self.assertEqual(got.body, b"")
        self.assertEqual(got.object.digest, blob.digest)
        self.assertEqual(got.object.byte_count, blob.byte_count)

        ref2 = k.put_artifact(
            Artifact(
                type="observer.v1.Capture",
                object=ObjectRef(
                    digest=blob.digest,
                    byte_count=blob.byte_count,
                    namespace=blob.namespace,
                ),
            )
        )
        self.assertEqual(ref2.id, ref.id)

        ref3, blob3 = k.put_artifact_with_blob(
            "observer.v1.Capture", "observer", payload, "observer"
        )
        self.assertEqual(ref3.id, ref.id)
        self.assertEqual(blob3.digest, blob.digest)

        data = k.get_blob(
            GetBlobRequest(digest=got.object.digest, namespace=got.object.namespace)
        )
        self.assertEqual(data.data, payload)

        entry = next(e for e in k.ledger() if e.kind == "artifact.put")
        a = Artifact()
        a.ParseFromString(entry.detail)
        self.assertEqual(a.body, b"")
        self.assertEqual(a.object.digest, blob.digest)

    def test_external_artifact_rejects_missing_or_mismatched_blob(self):
        k = MemoryKernel()
        data = b"small-but-external"
        blob = k.put_blob(PutBlobRequest(namespace="ns", data=data))

        with self.assertRaises(NotFound):
            k.put_artifact(
                Artifact(
                    type="t",
                    object=ObjectRef(
                        digest=blob_id(b"nope"), byte_count=4, namespace="ns"
                    ),
                )
            )
        with self.assertRaises(BlobIntegrity):
            k.put_artifact(
                Artifact(
                    type="t",
                    object=ObjectRef(
                        digest=blob.digest,
                        byte_count=blob.byte_count + 1,
                        namespace="ns",
                    ),
                )
            )
        with self.assertRaises(Invalid):
            k.put_artifact(
                Artifact(
                    type="t",
                    body=b"x",
                    object=ObjectRef(
                        digest=blob.digest,
                        byte_count=blob.byte_count,
                        namespace="ns",
                    ),
                )
            )
        with self.assertRaises(NotFound):
            k.put_artifact(
                Artifact(
                    type="t",
                    object=ObjectRef(
                        digest=blob.digest,
                        byte_count=blob.byte_count,
                        namespace="other",
                    ),
                )
            )

    def test_value_identity_independent_of_blob_identity(self):
        k = MemoryKernel()
        data = b"shared-raw-bytes"
        blob_a = k.put_blob(PutBlobRequest(namespace="a", data=data))
        blob_b = k.put_blob(PutBlobRequest(namespace="b", data=data))
        self.assertEqual(blob_a.digest, blob_b.digest)

        art_a = k.put_artifact(
            Artifact(
                type="t",
                object=ObjectRef(
                    digest=blob_a.digest,
                    byte_count=blob_a.byte_count,
                    namespace="a",
                ),
            )
        )
        art_b = k.put_artifact(
            Artifact(
                type="t",
                object=ObjectRef(
                    digest=blob_b.digest,
                    byte_count=blob_b.byte_count,
                    namespace="b",
                ),
            )
        )
        self.assertNotEqual(art_a.id, art_b.id)

        art_c = k.put_artifact(
            Artifact(
                type="other",
                object=ObjectRef(
                    digest=blob_a.digest,
                    byte_count=blob_a.byte_count,
                    namespace="a",
                ),
            )
        )
        self.assertNotEqual(art_c.id, art_a.id)

    def test_request_context_deadline_and_idempotency(self):
        k = MemoryKernel()
        past = RequestContext(deadline_unix_ms=1)
        with self.assertRaises(FailedPrecondition):
            k.put_artifact(Artifact(type="t", body=b"x"), ctx=past)
        ctx = RequestContext(caller="worker", request_key="put-once")
        a = k.put_artifact(Artifact(type="t", body=b"unique-body"), ctx=ctx)
        before = len(k.ledger())
        b = k.put_artifact(Artifact(type="t", body=b"unique-body"), ctx=ctx)
        self.assertEqual(a.id, b.id)
        self.assertEqual(len(k.ledger()), before, "idempotent replay must not append")

    def test_transition_is_on_the_abi(self):
        k = MemoryKernel()
        k.register(ModuleManifest(name="m", version="1"))
        ack = k.transition(TransitionRequest(module="m", to=Lifecycle.LIFECYCLE_LOADED))
        self.assertEqual(ack.state, Lifecycle.LIFECYCLE_LOADED)
        self.assertTrue(any(e.kind == "module.loaded" for e in k.ledger()))

if __name__ == "__main__":
    unittest.main()
