"""Unit tests for assembly cut helpers."""

from __future__ import annotations

import unittest

from srcport_framework import (
    NodePlan,
    materialize_cut,
    memo_key,
)
from srcport_substrate import Assembly, AssemblyNode, Binding, NodeOutput


def diamond() -> Assembly:
    a = Assembly(id="d@1")
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
                module_version="1.0.0",
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


class CutTests(unittest.TestCase):
    def test_after_extract(self) -> None:
        cut = materialize_cut(diamond(), NodePlan.after("extract"))
        self.assertEqual(cut.kept_node_ids, ["retrieve", "write"])
        self.assertEqual(len(cut.skipped), 1)
        self.assertEqual(cut.skipped[0].node_id, "extract")
        self.assertEqual(cut.required_seeds[0].input_name, "__seed/extract/facts")

    def test_from_write(self) -> None:
        cut = materialize_cut(diamond(), NodePlan.from_node("write"))
        self.assertEqual(cut.kept_node_ids, ["write"])
        names = {s.input_name for s in cut.required_seeds}
        self.assertIn("__seed/extract/facts", names)
        self.assertIn("__seed/retrieve/sources", names)

    def test_after_terminal_rejected(self) -> None:
        with self.assertRaises(Exception) as ctx:
            materialize_cut(diamond(), NodePlan.after("write"))
        self.assertIn("terminal", str(ctx.exception))

    def test_memo_key_order_independent(self) -> None:
        a = {"b": "id2", "a": "id1"}
        b = {"a": "id1", "b": "id2"}
        self.assertEqual(
            memo_key("m", "1", "d", "cap", a),
            memo_key("m", "1", "d", "cap", b),
        )


if __name__ == "__main__":
    unittest.main()
