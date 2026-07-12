"""Message types + the two hash rules SPEC.md pins down.

These dataclasses are a faithful hand-port of the canonical contract in
``contracts/proto/srcport/substrate/v1/substrate.proto`` — that proto remains
the single source of truth. Field names mirror it; do not re-derive the core,
widen the proto and follow it.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass, field
from enum import IntEnum

# ─── enums ──────────────────────────────────────────────────────────────────


class Lifecycle(IntEnum):
    UNSPECIFIED = 0
    REGISTERED = 1
    LOADED = 2
    ACTIVE = 3
    DEACTIVATED = 4


class Decision(IntEnum):
    UNSPECIFIED = 0
    PENDING = 1
    APPROVED = 2
    REJECTED = 3


# ─── 1. Module ──────────────────────────────────────────────────────────────


@dataclass
class Capability:
    """A named thing a module can do, bound to the contract it speaks."""

    name: str = ""  # e.g. "recon.scan"
    contract: str = ""  # contract ref, e.g. "acme.recon.v1.ScanRequest"


@dataclass
class ModuleManifest:
    """Declares what a module provides and requires. Never imports another."""

    name: str = ""
    version: str = ""
    provides: list[Capability] = field(default_factory=list)
    requires: list[str] = field(default_factory=list)


# ─── 2. Artifact ────────────────────────────────────────────────────────────


@dataclass
class Artifact:
    """A typed, content-addressed, immutable value that flows between modules."""

    id: str = ""  # content address, assigned by the kernel
    type: str = ""  # contract ref describing body
    body: bytes = b""  # opaque encoded value
    meta: dict[str, str] = field(default_factory=dict)
    produced_by: str = ""  # module name


@dataclass
class ArtifactRef:
    id: str = ""


# ─── 3. Contract ────────────────────────────────────────────────────────────


@dataclass
class Contract:
    """The declarative schema that is the sole coupling point."""

    ref: str = ""  # fully-qualified name, e.g. "acme.recon.v1.Host"
    schema: str = ""  # schema text (proto / JSON Schema); may be empty


# ─── 4. Event ───────────────────────────────────────────────────────────────


@dataclass
class Event:
    """A bus message. ``seq`` is a total order assigned by the kernel."""

    id: str = ""
    topic: str = ""  # dotted, e.g. "recon.host.found"
    type: str = ""  # contract ref of payload
    payload: bytes = b""
    source: str = ""  # module name
    seq: int = 0  # kernel-assigned, monotonic


@dataclass
class Subscription:
    module: str = ""
    topics: list[str] = field(default_factory=list)


# ─── 5. Ledger ──────────────────────────────────────────────────────────────


@dataclass
class LedgerEntry:
    """One link in the append-only, hash-chained record."""

    seq: int = 0
    kind: str = ""  # "module.registered", "artifact.put", "event.published", …
    subject: str = ""  # id of the thing this entry is about
    detail: bytes = b""
    prev_hash: str = ""  # hash of entry seq-1 ("" for genesis)
    hash: str = ""  # sha256 over (seq, kind, subject, detail, prev_hash)


# ─── 6. Gate ────────────────────────────────────────────────────────────────


@dataclass
class GateRequest:
    id: str = ""
    action: str = ""  # human-readable description of the irreversible act
    context: bytes = b""  # evidence the human decides on
    requested_by: str = ""  # module name


@dataclass
class GateDecision:
    request_id: str = ""
    decision: Decision = Decision.UNSPECIFIED
    decided_by: str = ""  # human identity
    reason: str = ""


@dataclass
class GateTicket:
    request_id: str = ""


# ─── 7. Registry ────────────────────────────────────────────────────────────


@dataclass
class RegistrySnapshot:
    modules: list[ModuleManifest] = field(default_factory=list)
    capabilities: list[Capability] = field(default_factory=list)
    contracts: list[Contract] = field(default_factory=list)


# ─── ABI acks ───────────────────────────────────────────────────────────────


@dataclass
class RegisterAck:
    state: Lifecycle = Lifecycle.UNSPECIFIED


@dataclass
class PublishAck:
    seq: int = 0


@dataclass
class AppendRequest:
    kind: str = ""
    subject: str = ""
    detail: bytes = b""


# ─── addressing & ledger hashing ────────────────────────────────────────────

_SEP = b"\x00"


def artifact_id(type: str, body: bytes) -> str:
    """The content address: ``"sha256:" + hex(sha256(type + 0x00 + body))``.

    Same ``(type, body)`` yields the same id; a one-byte change yields a new
    id. ``meta`` and ``produced_by`` are deliberately NOT part of the address.
    """
    h = hashlib.sha256()
    h.update(type.encode("utf-8"))
    h.update(_SEP)
    h.update(body)
    return "sha256:" + h.hexdigest()


def _ledger_hash(
    seq: int, kind: str, subject: str, detail: bytes, prev_hash: str
) -> str:
    """sha256 over (seq, kind, subject, detail, prev_hash), 0x00-delimited."""
    h = hashlib.sha256()
    h.update(seq.to_bytes(8, "big"))
    h.update(_SEP)
    h.update(kind.encode("utf-8"))
    h.update(_SEP)
    h.update(subject.encode("utf-8"))
    h.update(_SEP)
    h.update(detail)
    h.update(_SEP)
    h.update(prev_hash.encode("utf-8"))
    return h.hexdigest()


def verify_chain(entries: list[LedgerEntry]) -> bool:
    """Verify a ledger end-to-end. Tampering with any committed entry breaks it."""
    prev = ""
    for i, e in enumerate(entries):
        if e.seq != i or e.prev_hash != prev:
            return False
        if _ledger_hash(e.seq, e.kind, e.subject, e.detail, e.prev_hash) != e.hash:
            return False
        prev = e.hash
    return True
