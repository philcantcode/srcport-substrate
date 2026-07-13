"""End-to-end host drive tests (diamond pipeline)."""

from __future__ import annotations

import unittest

from srcport_framework import (
    FrameworkPolicy,
    Host,
    MemoryMemo,
    ModulePlugin,
    PortBody,
    Presentation,
    StepContext,
    StepOutput,
    StepResult,
    StepStage,
    UiPersist,
)
from srcport_substrate import (
    Assembly,
    AssemblyNode,
    Binding,
    Capability,
    MemoryKernel,
    ModuleManifest,
    NamedArtifact,
    NodeOutput,
    Port,
    RunState,
    artifact_with_trait,
)


class Extractor(ModulePlugin):
    def manifest(self) -> ModuleManifest:
        m = ModuleManifest(name="extractor", version="1.0.0")
        cap = Capability(name="facts.extract")
        cap.inputs.append(Port(name="question", traits=["demo.v1.Question"]))
        cap.outputs.append(Port(name="facts", traits=["demo.v1.Facts"]))
        m.provides.append(cap)
        return m

    def module_digest(self) -> str | None:
        return "extract-v1"

    def execute(self, step: StepContext) -> StepOutput:
        q = step.inputs.get("question")
        body = b""
        if q is not None and q.traits:
            body = list(q.traits.values())[0].body
        step.emit_progress(
            Presentation.at_progress("Extracting facts", 0.5).with_detail("Reading…")
        )
        facts = f"facts-from:{body.decode()}".encode()
        return StepOutput(
            outputs=[PortBody.with_trait("facts", "demo.v1.Facts", facts)]
        )

    def on_init(self, step: StepContext) -> Presentation | None:
        return Presentation.init("Extracting facts").with_detail("Starting…")

    def on_final(self, step: StepContext, result: StepResult) -> Presentation | None:
        return Presentation.final_ok("Facts ready")


class Retriever(ModulePlugin):
    def manifest(self) -> ModuleManifest:
        m = ModuleManifest(name="retriever", version="1.0.0")
        cap = Capability(name="sources.retrieve")
        cap.inputs.append(Port(name="question", traits=["demo.v1.Question"]))
        cap.outputs.append(Port(name="sources", traits=["demo.v1.Sources"]))
        m.provides.append(cap)
        return m

    def module_digest(self) -> str | None:
        return "retrieve-v1"

    def execute(self, step: StepContext) -> StepOutput:
        return StepOutput(
            outputs=[PortBody.with_trait("sources", "demo.v1.Sources", b"SPEC.md")]
        )


class Writer(ModulePlugin):
    def manifest(self) -> ModuleManifest:
        m = ModuleManifest(name="writer", version="2.0.0")
        cap = Capability(name="answer.write")
        cap.inputs.extend(
            [
                Port(name="question", traits=["demo.v1.Question"]),
                Port(name="facts", traits=["demo.v1.Facts"]),
                Port(name="sources", traits=["demo.v1.Sources"]),
            ]
        )
        cap.outputs.append(Port(name="answer", traits=["demo.v1.Answer"]))
        m.provides.append(cap)
        return m

    def module_digest(self) -> str | None:
        return "write-v1"

    def execute(self, step: StepContext) -> StepOutput:
        facts = b""
        sources = b""
        if "facts" in step.inputs and step.inputs["facts"].traits:
            facts = list(step.inputs["facts"].traits.values())[0].body
        if "sources" in step.inputs and step.inputs["sources"].traits:
            sources = list(step.inputs["sources"].traits.values())[0].body
        body = b"answer:" + facts + b"+" + sources
        return StepOutput(
            outputs=[PortBody.with_trait("answer", "demo.v1.Answer", body)]
        )

    def on_init(self, step: StepContext) -> Presentation | None:
        return Presentation.init("Writing answer")

    def on_final(self, step: StepContext, result: StepResult) -> Presentation | None:
        return Presentation.final_ok("Answer ready")


def diamond_assembly() -> Assembly:
    a = Assembly(id="answer-pipeline@1")
    a.nodes.extend(
        [
            AssemblyNode(
                id="extract",
                module="extractor",
                module_version="1.0.0",
                capability="facts.extract",
            ),
            AssemblyNode(
                id="retrieve",
                module="retriever",
                module_version="1.0.0",
                capability="sources.retrieve",
            ),
            AssemblyNode(
                id="write",
                module="writer",
                module_version="2.0.0",
                capability="answer.write",
            ),
        ]
    )
    a.bindings.extend(
        [
            Binding(to_node="extract", to_port="question", input="question"),
            Binding(to_node="retrieve", to_port="question", input="question"),
            Binding(to_node="write", to_port="question", input="question"),
            Binding(
                to_node="write",
                to_port="facts",
                from_node="extract",
                from_port="facts",
            ),
            Binding(
                to_node="write",
                to_port="sources",
                from_node="retrieve",
                from_port="sources",
            ),
        ]
    )
    a.terminal.CopyFrom(NodeOutput(node="write", port="answer"))
    return a


class HostDriveTests(unittest.TestCase):
    def test_host_drives_diamond(self) -> None:
        k = MemoryKernel()
        host = Host(k).with_ui_persist(UiPersist.ARTIFACTS)
        host.register_plugin(Extractor())
        host.register_plugin(Retriever())
        host.register_plugin(Writer())

        q = artifact_with_trait("demo.v1.Question", b"What is substrate?")
        q.produced_by = "operator"
        question = k.put_artifact(q)

        run = host.start_pipeline(
            "run-1",
            diamond_assembly(),
            [NamedArtifact(name="question", artifact=question)],
            FrameworkPolicy.converge(),
        )
        self.assertEqual(run.state, RunState.RUN_STATE_RUNNING)

        done = host.drive("run-1")
        self.assertEqual(done.state, RunState.RUN_STATE_COMPLETED)
        self.assertTrue(done.answer.id)

        events = host.take_step_events()
        stages = {e.stage for e in events}
        self.assertIn(StepStage.INIT, stages)
        self.assertIn(StepStage.FINAL, stages)
        self.assertEqual(host.execute_count, 3)

    def test_memoized_second_run(self) -> None:
        k = MemoryKernel()
        host = Host(k).with_memo(MemoryMemo())
        host.register_plugin(Extractor())
        host.register_plugin(Retriever())
        host.register_plugin(Writer())

        question = k.put_artifact(artifact_with_trait("demo.v1.Question", b"memo me"))
        inputs = [NamedArtifact(name="question", artifact=question)]

        host.start_pipeline("r1", diamond_assembly(), inputs, FrameworkPolicy.memoized())
        host.drive("r1")
        first = host.execute_count
        self.assertEqual(first, 3)

        host.start_pipeline("r2", diamond_assembly(), inputs, FrameworkPolicy.memoized())
        host.drive("r2")
        self.assertEqual(host.execute_count, first)
        self.assertEqual(host.memo_hit_count, 3)


if __name__ == "__main__":
    unittest.main()
