"""Message types (re-exported from the generated protobuf code) + the two hash
rules SPEC.md pins down.

The messages are GENERATED from the canonical contract in
``contracts/proto/srcport/substrate/v1/substrate.proto`` (see buf.gen.yaml and
scripts/gen.sh) and re-exported here, so this SDK can never drift from the
contract. To add capability, widen the proto and regenerate; do not re-derive
the core.

Note the generated ergonomics: construct with keyword args
(``Artifact(type="…", body=b"…")``); enum values are fully qualified
(``Decision.DECISION_APPROVED``, ``Lifecycle.LIFECYCLE_REGISTERED``).
"""

from __future__ import annotations

import hashlib

from ._gen.srcport.substrate.v1.substrate_pb2 import (  # noqa: F401
    AppendRequest,
    Artifact,
    ArtifactRef,
    Assembly,
    AssemblyNode,
    Binding,
    Capability,
    ClaimRequest,
    Contract,
    Decision,
    Derivation,
    DerivationList,
    Event,
    GateDecision,
    GateRequest,
    GateTicket,
    LedgerEntry,
    Lifecycle,
    Limits,
    ModuleManifest,
    NamedArtifact,
    NodeOutput,
    Port,
    PublishAck,
    RegisterAck,
    RegistrySnapshot,
    Run,
    RunRef,
    RunRequest,
    RunState,
    Subscription,
    WorkItem,
)

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


def verify_chain(entries: "list[LedgerEntry]") -> bool:
    """Verify a ledger end-to-end. Tampering with any committed entry breaks it."""
    prev = ""
    for i, e in enumerate(entries):
        if e.seq != i or e.prev_hash != prev:
            return False
        if _ledger_hash(e.seq, e.kind, e.subject, e.detail, e.prev_hash) != e.hash:
            return False
        prev = e.hash
    return True
