"""Cross-run work memoisation (optional framework mode)."""

from __future__ import annotations

import hashlib
import threading
from dataclasses import dataclass, field
from typing import Protocol

from srcport_substrate import ArtifactRef, NamedArtifact, WorkItem

@dataclass
class MemoRecord:
    key: str
    module: str
    module_version: str
    module_digest: str
    capability: str
    node_id: str
    inputs: dict[str, str] = field(default_factory=dict)
    outputs: dict[str, str] = field(default_factory=dict)
    source_run_id: str = ""
    source_work_id: str = ""


@dataclass
class MemoNodes:
    kind: str = "all"  # all | only | except
    ids: list[str] = field(default_factory=list)

    def allows(self, node_id: str) -> bool:
        if self.kind == "only":
            return node_id in self.ids
        if self.kind == "except":
            return node_id not in self.ids
        return True


@dataclass
class MemoPlan:
    enabled: bool = False
    require_digest: bool = False
    nodes: MemoNodes = field(default_factory=MemoNodes)

    @staticmethod
    def off() -> MemoPlan:
        return MemoPlan()

    @staticmethod
    def on() -> MemoPlan:
        return MemoPlan(enabled=True, require_digest=True)


class MemoStore(Protocol):
    def get(self, key: str) -> MemoRecord | None: ...
    def put(self, record: MemoRecord) -> None: ...
    def __len__(self) -> int: ...
    def clear(self) -> None: ...


class MemoryMemo:
    def __init__(self) -> None:
        self._mu = threading.Lock()
        self._data: dict[str, MemoRecord] = {}

    def get(self, key: str) -> MemoRecord | None:
        with self._mu:
            return self._data.get(key)

    def put(self, record: MemoRecord) -> None:
        with self._mu:
            self._data[record.key] = record

    def __len__(self) -> int:
        with self._mu:
            return len(self._data)

    def clear(self) -> None:
        with self._mu:
            self._data.clear()


def memo_key(
    module: str,
    module_version: str,
    module_digest: str,
    capability: str,
    inputs: dict[str, str],
) -> str:
    h = hashlib.sha256()
    h.update(module.encode())
    h.update(b"\x00")
    h.update(module_version.encode())
    h.update(b"\x00")
    h.update(module_digest.encode())
    h.update(b"\x00")
    h.update(capability.encode())
    h.update(b"\x00")
    for port in sorted(inputs):
        h.update(port.encode())
        h.update(b"\x00")
        h.update(inputs[port].encode())
        h.update(b"\x00")
    return "sha256:" + h.hexdigest()


def input_fingerprint_map(work: WorkItem) -> dict[str, str]:
    m: dict[str, str] = {}
    for na in work.inputs:
        if na.artifact and na.name and na.artifact.id:
            m[na.name] = na.artifact.id
    return m


def record_to_named_outputs(record: MemoRecord) -> list[NamedArtifact]:
    return [
        NamedArtifact(name=port, artifact=ArtifactRef(id=aid))
        for port, aid in sorted(record.outputs.items())
    ]


def build_record(
    key: str,
    work: WorkItem,
    module_digest: str,
    outputs: list[NamedArtifact],
    run_id: str,
) -> MemoRecord:
    out: dict[str, str] = {}
    for na in outputs:
        if na.artifact and na.name and na.artifact.id:
            out[na.name] = na.artifact.id
    return MemoRecord(
        key=key,
        module=work.module,
        module_version=work.module_version,
        module_digest=module_digest,
        capability=work.capability,
        node_id=work.node_id,
        inputs=input_fingerprint_map(work),
        outputs=out,
        source_run_id=run_id,
        source_work_id=work.id,
    )
