"""The minimal conformance suite from SPEC.md §Conformance. An SDK is
conformant iff all invariants pass. Runs with the stdlib: ``python -m unittest``.
"""

import queue
import unittest

from srcport_substrate import (
    artifact_with_trait,
    artifact_with_external_trait,
    artifact_id_single,
    artifact_id_of,
    get_trait,

    AppendRequest,
    Artifact,
    ArtifactStorePolicy,
    Assembly,
    AssemblyNode,
    Binding,
    BlobIngestMode,
    BlobIntegrity,
    BlobRef,
    Capability,
    ClaimRequest,
    Closure,
    Conflict,
    Contract,
    Derivation,
    Event,
    ExecutionPolicy,
    FailWorkRequest,
    FailedPrecondition,
    Firing,
    GetBlobRequest,
    HasBlobRequest,
    HeartbeatRequest,
    Invalid,
    Lifecycle,
    Limits,
    MAX_INLINE_ARTIFACT_BYTES,
    MemoryKernel,
    ModuleManifest,
    NamedArtifact,
    NodeOutput,
    NotFound,
    ObjectRef,
    Port,
    PutBlobRequest,
    ResourceExhausted,
    RunClosed,
    StoreDurability,
    RequestContext,
    RunRequest,
    RunRef,
    RunState,
    Subscription,
    TransitionRequest,
    blob_id,
    contract_digest,
    is_contract_placeholder,
    object_ref_bytes,
    verify_chain,
)


def _claim_one(k, run_id, module):
    """First claimed item, or empty WorkItem when none."""
    resp = k.claim_ready(ClaimRequest(run_id=run_id, module=module))
    if resp.items:
        return resp.items[0]
    from srcport_substrate import WorkItem
    return WorkItem()


def _art(contract, body, produced_by="", meta=None):
    a = artifact_with_trait(contract, body)
    if produced_by:
        a.produced_by = produced_by
    if meta:
        a.meta.update(meta)
    return a


def _ext(contract, obj, produced_by=""):
    a = artifact_with_external_trait(contract, obj)
    if produced_by:
        a.produced_by = produced_by
    return a




class Conformance(unittest.TestCase):
    # 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
    def test_addressing_is_content_derived_and_metamorphic(self):
        k = MemoryKernel()
        a = k.put_artifact(
            _art("acme.recon.v1.Host", b"10.0.0.1", produced_by="recon")
        )
        b = k.put_artifact(
            _art("acme.recon.v1.Host", b"10.0.0.1", produced_by="other")
        )
        self.assertEqual(a.id, b.id, "same (type, body) must yield the same id")
        self.assertTrue(a.id.startswith("sha256:"))

        c = k.put_artifact(artifact_with_trait("acme.recon.v1.Host", b"10.0.0.2"))
        self.assertNotEqual(a.id, c.id, "a one-byte change must change the address")

        d = k.put_artifact(artifact_with_trait("acme.recon.v1.Port", b"10.0.0.1"))
        self.assertNotEqual(a.id, d.id, "type must participate in the address")

        self.assertEqual(a.id, artifact_id_single("acme.recon.v1.Host", b"10.0.0.1"))

    # 2. IMMUTABILITY — reads back byte-identical; a later put never mutates it.
    def test_artifacts_are_immutable(self):
        k = MemoryKernel()
        r = k.put_artifact(_art("t", b"payload", meta={"first": "true"}))
        got = k.get_artifact(r)
        self.assertEqual(get_trait(got, "t").body, b"payload", "reads back byte-identical")
        self.assertEqual(got.meta.get("first"), "true")

        # A later put of the same content with different meta must NOT change
        # what is stored. First write wins.
        r2 = k.put_artifact(
            _art("t", b"payload", meta={"first": "false", "sneaky": "yes"})
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

        h1 = k.put_artifact(artifact_with_trait("acme.recon.v1.Host", b"h1"))
        h2 = k.put_artifact(artifact_with_trait("acme.recon.v1.Host", b"h2"))
        p1 = k.put_artifact(artifact_with_trait("acme.recon.v1.Port", b"p1"))
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
        k.put_artifact(artifact_with_trait("t", b"x"))
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
                provides=[Capability(name="recon.scan", outputs=[Port(name="out", traits=["acme.recon.v1.Host"])])],
            )
        )
        k.register(
            ModuleManifest(
                name="report",
                version="0.2.0",
                provides=[Capability(name="report.render", outputs=[Port(name="out", traits=["acme.report.v1.Report"])])],
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
                provides=[Capability(name="do", outputs=[Port(name="out", traits=["acme.NewThing"])])],
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
            _art("acme.recon.v1.Host", b"10.0.0.1", produced_by="recon", meta={"region": "eu", "scan": "full"})
        )
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", outputs=[Port(name="out", traits=["acme.recon.v1.Host"])])],
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
        self.assertIn("acme.recon.v1.Host", a.traits)
        self.assertEqual(a.produced_by, "recon")
        self.assertEqual(dict(a.meta), {"region": "eu", "scan": "full"})
        self.assertEqual(get_trait(a, "acme.recon.v1.Host").body, b"10.0.0.1")

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
            return _art("t", b"", meta={"z": "1", "a": "2", "m": "3", "b": "4"}).SerializeToString(deterministic=True)

        for _ in range(64):
            self.assertEqual(
                build(), build(), "identical meta must encode to identical bytes (sorted keys)"
            )

    # METAMORPHIC — the address depends ONLY on (type, body). Transforming fields
    # that aren't identity (meta, produced_by) is a known no-op: the
    # id must not move, or the address would be keyed on provenance, not content.
    def test_address_ignores_non_identity_fields(self):
        k = MemoryKernel()
        base = k.put_artifact(artifact_with_trait("acme.recon.v1.Host", b"10.0.0.1"))
        enriched = k.put_artifact(
            _art("acme.recon.v1.Host", b"10.0.0.1", produced_by="whoever", meta={"x": "y"})
        )
        self.assertEqual(
            enriched.id, base.id,
            "meta and produced_by must not participate in the address",
        )
        self.assertEqual(enriched.id, artifact_id_single("acme.recon.v1.Host", b"10.0.0.1"))

    # CROSS-SDK KNOWN ANSWER — a fixed scenario must produce this exact final
    # ledger hash in every SDK. Go, Rust, and Python all assert the SAME constant,
    # so any drift in canonical detail encoding or the hash rule fails here and the
    # three chains are pinned to cross-verify. If it ever changes, it changes in
    # all three suites in lockstep — never one SDK alone.
    def test_ledger_hash_known_answer_cross_sdk(self):
        want = "3f0957aaae7a7a939dc3b5dba74145b03af065e3f04ce302ef602bc01424f350"

        k = MemoryKernel()
        k.register(
            ModuleManifest(
                name="recon",
                version="0.1.0",
                provides=[Capability(name="recon.scan", outputs=[Port(name="host", traits=["acme.recon.v1.Host"])])],
                requires=["report.render"],
            )
        )
        k.put_artifact(
            _art("acme.recon.v1.Host", b"10.0.0.1", produced_by="recon", meta={"region": "eu", "scan": "full"})
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
                        inputs=[Port(name="question", traits=["demo.Question"])],
                        outputs=[Port(name="facts", traits=["demo.Facts"])],
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
                            Port(name="question", traits=["demo.Question"]),
                            Port(name="facts", traits=["demo.Facts"]),
                        ],
                        outputs=[Port(name="answer", traits=["demo.Answer"])],
                    )
                ],
            )
        )
        question = k.put_artifact(
            artifact_with_trait("demo.Question", b"What follows?")
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
            _claim_one(k, "run-1", "writer").id
        )
        extract = _claim_one(k, "run-1", "extractor")
        facts = k.put_artifact(artifact_with_trait("demo.Facts", b"typed flow"))
        k.commit(
            Derivation(
                run_id="run-1",
                work_id=extract.id,
                node_id=extract.node_id,
                outputs=[NamedArtifact(name="facts", artifact=facts)],
            )
        )
        write = _claim_one(k, "run-1", "writer")
        self.assertEqual(len(write.inputs), 2, "fan-in supplies both inputs")
        answer = k.put_artifact(
            artifact_with_trait("demo.Answer", b"Modules converge.")
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
                                name="in", traits=["demo.Value"], optional=True
                            )
                        ],
                        outputs=[Port(name="out", traits=["demo.Value"])],
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
        k.register(ModuleManifest(name="source", version="1", provides=[Capability(name="source.maybe", outputs=[Port(name="value", traits=["demo.Value"], optional=True)])]))
        k.register(ModuleManifest(name="sink", version="1", provides=[Capability(name="sink.answer", inputs=[Port(name="value", traits=["demo.Value"])], outputs=[Port(name="answer", traits=["demo.Answer"])])]))
        assembly = Assembly(id="stall@1", nodes=[AssemblyNode(id="source", module="source", module_version="1", capability="source.maybe"), AssemblyNode(id="sink", module="sink", module_version="1", capability="sink.answer")], bindings=[Binding(to_node="sink", to_port="value", from_node="source", from_port="value")], terminal=NodeOutput(node="sink", port="answer"))
        k.start_run(RunRequest(id="stall", assembly=assembly))
        work = _claim_one(k, "stall", "source")
        run = k.commit(Derivation(run_id="stall", work_id=work.id, node_id=work.node_id))
        self.assertEqual(run.state, RunState.RUN_STATE_STALLED)

    def test_convergent_run_hashes_match_every_sdk(self):
        k = MemoryKernel()
        k.register(ModuleManifest(name="answerer", version="1.0.0", provides=[Capability(name="answer.write", outputs=[Port(name="answer", traits=["demo.Answer"])])]))
        k.start_run(RunRequest(id="parity", assembly=Assembly(id="single@1", nodes=[AssemblyNode(id="answer", module="answerer", module_version="1.0.0", capability="answer.write")], terminal=NodeOutput(node="answer", port="answer"))))
        work = _claim_one(k, "parity", "answerer")
        answer = k.put_artifact(artifact_with_trait("demo.Answer", b"yes"))
        k.commit(Derivation(run_id="parity", work_id=work.id, node_id=work.node_id, outputs=[NamedArtifact(name="answer", artifact=answer)]))
        self.assertEqual(k.derivations()[0].id, "sha256:8f7f99a396dbf79c7f2287d2f9fca7f4167343831a9283cdfbeb2fe010c8414c")
        self.assertEqual(k.ledger()[-1].hash, "faa944642933bb3b1b2d3789fb940a3ed8eb9802d06bf17444677f72fe974335")

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
            _ext("observer.v1.Capture", ObjectRef(
                    digest=blob.digest,
                    byte_count=blob.byte_count,
                    namespace=blob.namespace,
                ), produced_by="observer")
        )
        want_art = artifact_with_external_trait(
            "observer.v1.Capture",
            ObjectRef(
                digest=blob.digest,
                byte_count=blob.byte_count,
                namespace=blob.namespace,
            ),
        )
        self.assertEqual(ref.id, artifact_id_of(want_art))
        self.assertNotEqual(ref.id, blob_id(payload))

        got = k.get_artifact(ref)
        self.assertEqual(list(got.traits.values())[0].body, b"")
        self.assertEqual(list(got.traits.values())[0].object.digest, blob.digest)
        self.assertEqual(list(got.traits.values())[0].object.byte_count, blob.byte_count)

        ref2 = k.put_artifact(
            artifact_with_external_trait("observer.v1.Capture", ObjectRef(
                    digest=blob.digest,
                    byte_count=blob.byte_count,
                    namespace=blob.namespace,
                ))
        )
        self.assertEqual(ref2.id, ref.id)

        ref3, blob3 = k.put_artifact_with_blob(
            "observer.v1.Capture", "observer", payload, "observer"
        )
        self.assertEqual(ref3.id, ref.id)
        self.assertEqual(blob3.digest, blob.digest)

        data = k.get_blob(
            GetBlobRequest(digest=list(got.traits.values())[0].object.digest, namespace=list(got.traits.values())[0].object.namespace)
        )
        self.assertEqual(data.data, payload)

        entry = next(e for e in k.ledger() if e.kind == "artifact.put")
        a = Artifact()
        a.ParseFromString(entry.detail)
        self.assertEqual(list(a.traits.values())[0].body, b"")
        self.assertEqual(list(a.traits.values())[0].object.digest, blob.digest)

    def test_external_artifact_rejects_missing_or_mismatched_blob(self):
        k = MemoryKernel()
        data = b"small-but-external"
        blob = k.put_blob(PutBlobRequest(namespace="ns", data=data))

        with self.assertRaises(NotFound):
            k.put_artifact(
                artifact_with_external_trait("t", ObjectRef(
                        digest=blob_id(b"nope"), byte_count=4, namespace="ns"
                    ))
            )
        with self.assertRaises(BlobIntegrity):
            k.put_artifact(
                artifact_with_external_trait("t", ObjectRef(
                        digest=blob.digest,
                        byte_count=blob.byte_count + 1,
                        namespace="ns",
                    ))
            )
        with self.assertRaises(Invalid):
            both = artifact_with_trait("t", b"x")
            both.traits["t"].object.CopyFrom(
                ObjectRef(
                    digest=blob.digest,
                    byte_count=blob.byte_count,
                    namespace="ns",
                )
            )
            k.put_artifact(both)
        with self.assertRaises(NotFound):
            k.put_artifact(
                artifact_with_external_trait("t", ObjectRef(
                        digest=blob.digest,
                        byte_count=blob.byte_count,
                        namespace="other",
                    ))
            )

    def test_value_identity_independent_of_blob_identity(self):
        k = MemoryKernel()
        data = b"shared-raw-bytes"
        blob_a = k.put_blob(PutBlobRequest(namespace="a", data=data))
        blob_b = k.put_blob(PutBlobRequest(namespace="b", data=data))
        self.assertEqual(blob_a.digest, blob_b.digest)

        art_a = k.put_artifact(
            artifact_with_external_trait("t", ObjectRef(
                    digest=blob_a.digest,
                    byte_count=blob_a.byte_count,
                    namespace="a",
                ))
        )
        art_b = k.put_artifact(
            artifact_with_external_trait("t", ObjectRef(
                    digest=blob_b.digest,
                    byte_count=blob_b.byte_count,
                    namespace="b",
                ))
        )
        self.assertNotEqual(art_a.id, art_b.id)

        art_c = k.put_artifact(
            artifact_with_external_trait("other", ObjectRef(
                    digest=blob_a.digest,
                    byte_count=blob_a.byte_count,
                    namespace="a",
                ))
        )
        self.assertNotEqual(art_c.id, art_a.id)

    def test_request_context_deadline_and_idempotency(self):
        k = MemoryKernel()
        past = RequestContext(deadline_unix_ms=1)
        with self.assertRaises(FailedPrecondition):
            k.put_artifact(artifact_with_trait("t", b"x"), ctx=past)
        ctx = RequestContext(caller="worker", request_key="put-once")
        a = k.put_artifact(artifact_with_trait("t", b"unique-body"), ctx=ctx)
        before = len(k.ledger())
        b = k.put_artifact(artifact_with_trait("t", b"unique-body"), ctx=ctx)
        self.assertEqual(a.id, b.id)
        self.assertEqual(len(k.ledger()), before, "idempotent replay must not append")

    def test_transition_is_on_the_abi(self):
        k = MemoryKernel()
        k.register(ModuleManifest(name="m", version="1"))
        ack = k.transition(TransitionRequest(module="m", to=Lifecycle.LIFECYCLE_LOADED))
        self.assertEqual(ack.state, Lifecycle.LIFECYCLE_LOADED)
        self.assertTrue(any(e.kind == "module.loaded" for e in k.ledger()))

    def test_store_policy_defaults_and_snapshot(self):
        k = MemoryKernel()
        p = k.store_policy
        self.assertEqual(p.max_inline_bytes, MAX_INLINE_ARTIFACT_BYTES)
        self.assertEqual(p.max_blob_bytes, 0)
        self.assertEqual(p.ingest_mode, BlobIngestMode.BLOB_INGEST_MODE_COPY_VERIFY)
        self.assertEqual(p.durability, StoreDurability.STORE_DURABILITY_EPHEMERAL)
        snap = k.snapshot()
        self.assertEqual(snap.store_policy.max_inline_bytes, p.max_inline_bytes)

    def test_store_policy_rejects_oversized_inline_body(self):
        k = MemoryKernel(
            ArtifactStorePolicy(
                max_inline_bytes=8,
                ingest_mode=BlobIngestMode.BLOB_INGEST_MODE_COPY_VERIFY,
                durability=StoreDurability.STORE_DURABILITY_EPHEMERAL,
            )
        )
        k.put_artifact(artifact_with_trait("t", b"ok-small"))
        with self.assertRaises(ResourceExhausted):
            k.put_artifact(artifact_with_trait("t", b"too-large!"))

    def test_store_policy_rejects_oversized_blob(self):
        k = MemoryKernel(
            ArtifactStorePolicy(
                max_inline_bytes=MAX_INLINE_ARTIFACT_BYTES,
                max_blob_bytes=16,
                ingest_mode=BlobIngestMode.BLOB_INGEST_MODE_COPY_VERIFY,
                durability=StoreDurability.STORE_DURABILITY_EPHEMERAL,
            )
        )
        k.put_blob(PutBlobRequest(namespace="n", data=b"fifteen-bytes!"))
        with self.assertRaises(ResourceExhausted):
            k.put_blob(PutBlobRequest(namespace="n", data=b"seventeen-bytes!!!"))


    # 12b. LEASED CONCURRENCY — batch claim, max_in_flight, fail/retry, lease reclaim.
    def test_batch_claim_and_max_in_flight(self):
        k = MemoryKernel()
        k.register(
            ModuleManifest(
                name="worker",
                version="1",
                provides=[
                    Capability(
                        name="work.item",
                        firing=Firing.FIRING_ONCE_PER_KEY,
                        inputs=[
                            Port(
                                name="key",
                                traits=["demo.Key"],
                                key=True,
                            )
                        ],
                        outputs=[Port(name="out", traits=["demo.Out"])],
                    )
                ],
            )
        )
        a = k.put_artifact(artifact_with_trait("demo.Key", b"a"))
        b = k.put_artifact(artifact_with_trait("demo.Key", b"b"))
        k.start_run(
            RunRequest(
                id="batch",
                assembly=Assembly(
                    id="batch@1",
                    nodes=[
                        AssemblyNode(
                            id="w1",
                            module="worker",
                            module_version="1",
                            capability="work.item",
                        ),
                        AssemblyNode(
                            id="w2",
                            module="worker",
                            module_version="1",
                            capability="work.item",
                        ),
                    ],
                    bindings=[
                        Binding(to_node="w1", to_port="key", input="k1"),
                        Binding(to_node="w2", to_port="key", input="k2"),
                    ],
                    terminal=NodeOutput(node="w1", port="out"),
                ),
                inputs=[
                    NamedArtifact(name="k1", artifact=a),
                    NamedArtifact(name="k2", artifact=b),
                ],
                limits=Limits(max_steps=10, max_in_flight=1),
                policy=ExecutionPolicy(closure=Closure.CLOSURE_OPEN),
            )
        )

        first = k.claim_ready(ClaimRequest(run_id="batch", max_items=2))
        self.assertEqual(len(first.items), 1, "max_in_flight=1")
        self.assertEqual(first.items[0].attempt, 1)
        self.assertTrue(first.items[0].unit_key)
        self.assertGreater(first.items[0].lease_until_unix_ms, 0)

        blocked = k.claim_ready(ClaimRequest(run_id="batch", max_items=2))
        self.assertEqual(len(blocked.items), 0, "at capacity")

        out = k.put_artifact(artifact_with_trait("demo.Out", b"1"))
        k.commit(
            Derivation(
                run_id="batch",
                work_id=first.items[0].id,
                node_id=first.items[0].node_id,
                outputs=[NamedArtifact(name="out", artifact=out)],
            )
        )

        second = k.claim_ready(ClaimRequest(run_id="batch", max_items=2))
        self.assertEqual(len(second.items), 1)

    def test_fail_work_retries_then_terminals(self):
        k = MemoryKernel()
        k.register(
            ModuleManifest(
                name="flaky",
                version="1",
                provides=[
                    Capability(
                        name="flaky.run",
                        outputs=[Port(name="out", traits=["demo.Out"])],
                    )
                ],
            )
        )
        k.start_run(
            RunRequest(
                id="fail",
                assembly=Assembly(
                    id="fail@1",
                    nodes=[
                        AssemblyNode(
                            id="n",
                            module="flaky",
                            module_version="1",
                            capability="flaky.run",
                        )
                    ],
                    terminal=NodeOutput(node="n", port="out"),
                ),
                limits=Limits(max_steps=10, max_attempts=2),
            )
        )

        w1 = _claim_one(k, "fail", "flaky")
        self.assertEqual(w1.attempt, 1)
        k.fail_work(
            FailWorkRequest(
                run_id="fail",
                work_id=w1.id,
                reason="boom",
                terminal=False,
            )
        )

        w2 = _claim_one(k, "fail", "flaky")
        self.assertEqual(w2.attempt, 2)
        self.assertEqual(w2.id, w1.id)
        stalled = k.fail_work(
            FailWorkRequest(
                run_id="fail",
                work_id=w2.id,
                reason="boom again",
                terminal=False,
            )
        )
        # attempts exhausted → DONE; no READY/CLAIMED → STALLED under FIRST_TERMINAL.
        self.assertEqual(stalled.state, RunState.RUN_STATE_STALLED)
        with self.assertRaises(RunClosed):
            k.claim_ready(ClaimRequest(run_id="fail", module="flaky"))

    def test_lease_expiry_returns_unit_to_ready(self):
        import time

        k = MemoryKernel()
        k.register(
            ModuleManifest(
                name="slow",
                version="1",
                provides=[
                    Capability(
                        name="slow.run",
                        outputs=[Port(name="out", traits=["demo.Out"])],
                    )
                ],
            )
        )
        k.start_run(
            RunRequest(
                id="lease",
                assembly=Assembly(
                    id="lease@1",
                    nodes=[
                        AssemblyNode(
                            id="n",
                            module="slow",
                            module_version="1",
                            capability="slow.run",
                        )
                    ],
                    terminal=NodeOutput(node="n", port="out"),
                ),
                limits=Limits(
                    max_steps=10, default_lease_ms=1, max_attempts=3
                ),
                policy=ExecutionPolicy(closure=Closure.CLOSURE_OPEN),
            )
        )

        w1 = _claim_one(k, "lease", "slow")
        self.assertTrue(w1.id)
        # Second claim while leased → empty.
        self.assertFalse(_claim_one(k, "lease", "slow").id)
        time.sleep(0.005)
        w2 = _claim_one(k, "lease", "slow")
        self.assertTrue(w2.id, "reclaimed after lease expiry")
        self.assertEqual(w2.attempt, 2)

        # Heartbeat keeps lease alive.
        k.heartbeat(
            HeartbeatRequest(
                run_id="lease",
                work_ids=[w2.id],
                extend_lease_ms=60_000,
            )
        )
        time.sleep(0.005)
        self.assertFalse(
            _claim_one(k, "lease", "slow").id,
            "still leased after heartbeat",
        )

    def test_concurrent_claimants_do_not_double_claim(self):
        import threading

        k = MemoryKernel()
        k.register(
            ModuleManifest(
                name="solo",
                version="1",
                provides=[
                    Capability(
                        name="solo.run",
                        outputs=[Port(name="out", traits=["demo.Out"])],
                    )
                ],
            )
        )
        k.start_run(
            RunRequest(
                id="race",
                assembly=Assembly(
                    id="race@1",
                    nodes=[
                        AssemblyNode(
                            id="n",
                            module="solo",
                            module_version="1",
                            capability="solo.run",
                        )
                    ],
                    terminal=NodeOutput(node="n", port="out"),
                ),
            )
        )

        results = []

        def worker():
            resp = k.claim_ready(
                ClaimRequest(run_id="race", module="solo", max_items=1)
            )
            results.append(len(resp.items))

        threads = [threading.Thread(target=worker) for _ in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        self.assertEqual(sum(results), 1, "exactly one claimant wins")



if __name__ == "__main__":
    unittest.main()
