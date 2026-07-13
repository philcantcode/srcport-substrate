"""Message types (re-exported from the generated protobuf code) + the two hash
rules SPEC.md pins down.

The messages are GENERATED from the canonical contract in
``contracts/proto/srcport/substrate/v1/substrate.proto`` (see buf.gen.yaml and
scripts/gen.sh) and re-exported here, so this SDK can never drift from the
contract. To add capability, widen the proto and regenerate; do not re-derive
the core.

Note the generated ergonomics: construct with keyword args
(``Artifact(type="…", body=b"…")``); enum values are fully qualified
(``Lifecycle.LIFECYCLE_REGISTERED``, ``RunState.RUN_STATE_RUNNING``).
"""

from __future__ import annotations

import hashlib

from ._gen.srcport.substrate.v1.substrate_pb2 import (  # noqa: F401
    AppendRequest,
    Artifact,
    ArtifactStorePolicy,
    Trait,
    ArtifactRef,
    Assembly,
    AssemblyNode,
    Binding,
    BlobData,
    BlobIngestMode,
    BlobRef,
    Capability,
    ClaimRequest,
    ClaimResponse,
    Closure,
    Contract,
    Derivation,
    DerivationList,
    Error,
    ErrorCode,
    Event,
    ExecutionPolicy,
    FailWorkRequest,
    Firing,
    GetBlobRequest,
    HasBlobRequest,
    HasBlobResponse,
    HeartbeatRequest,
    HeartbeatResponse,
    InjectInputRequest,
    LedgerEntry,
    Lifecycle,
    Limits,
    ModuleManifest,
    NamedArtifact,
    NodeOutput,
    ObjectRef,
    Port,
    PublishAck,
    PutBlobRequest,
    RegisterAck,
    RegistrySnapshot,
    RequestContext,
    Run,
    RunRef,
    RunRequest,
    RunState,
    SnapshotRequest,
    StoreDurability,
    Subscription,
    TransitionAck,
    TransitionRequest,
    WorkFailure,
    WorkItem,
)

# ─── addressing & ledger hashing ────────────────────────────────────────────

_SEP = b"\x00"

# Default hard max for a single Trait.body when ArtifactStorePolicy.max_inline_bytes
# is 0 at construction. Larger payloads must put_blob + ObjectRef.
MAX_INLINE_ARTIFACT_BYTES = 1 << 20  # 1 MiB

# Defaults when Limits.default_lease_ms / max_attempts are 0 at StartRun.
DEFAULT_LEASE_MS = 60_000
DEFAULT_MAX_ATTEMPTS = 3


def normalize_store_policy(policy: ArtifactStorePolicy | None = None) -> ArtifactStorePolicy:
    """Fill defaults and reject unsupported ingest modes. Frozen at construction."""
    p = ArtifactStorePolicy() if policy is None else ArtifactStorePolicy()
    if policy is not None:
        p.CopyFrom(policy)
    if p.max_inline_bytes == 0:
        p.max_inline_bytes = MAX_INLINE_ARTIFACT_BYTES
    if p.ingest_mode in (
        BlobIngestMode.BLOB_INGEST_MODE_UNSPECIFIED,
        BlobIngestMode.BLOB_INGEST_MODE_COPY_VERIFY,
    ):
        p.ingest_mode = BlobIngestMode.BLOB_INGEST_MODE_COPY_VERIFY
    else:
        raise ValueError(
            f"unsupported BlobIngestMode: {p.ingest_mode} (only COPY_VERIFY in v2)"
        )
    if p.durability == StoreDurability.STORE_DURABILITY_UNSPECIFIED:
        p.durability = StoreDurability.STORE_DURABILITY_EPHEMERAL
    return p


def default_store_policy() -> ArtifactStorePolicy:
    """Normalised MemoryKernel defaults (EPHEMERAL, 1 MiB inline)."""
    return normalize_store_policy(None)


def blob_id(data: bytes) -> str:
    """Pure blob identity: ``"sha256:" + hex(sha256(data))``."""
    return "sha256:" + hashlib.sha256(data).hexdigest()


def object_ref_bytes(obj: ObjectRef) -> bytes:
    """Address payload for an external trait value.

    ``digest ‖ 0x00 ‖ uint64_be(byte_count) ‖ 0x00 ‖ namespace``
    """
    return (
        obj.digest.encode("utf-8")
        + _SEP
        + int(obj.byte_count).to_bytes(8, "big")
        + _SEP
        + obj.namespace.encode("utf-8")
    )


def trait_has_external(trait: Trait) -> bool:
    """Whether this trait holds a verified external ObjectRef."""
    return bool(trait.object and trait.object.digest)


def trait_content(trait: Trait) -> bytes:
    """Bytes folded into value identity for one trait."""
    if trait_has_external(trait):
        return object_ref_bytes(trait.object)
    return bytes(trait.body)


def artifact_canonical_bytes(artifact: Artifact) -> bytes:
    """Canonical encoding of a trait bag for content addressing.

    For each contract_ref in UTF-8 ascending order::

        contract_ref ‖ 0x00 ‖ trait_content ‖ 0x00
    """
    out = bytearray()
    for key in sorted(artifact.traits.keys()):
        out.extend(key.encode("utf-8"))
        out.extend(_SEP)
        out.extend(trait_content(artifact.traits[key]))
        out.extend(_SEP)
    return bytes(out)


def artifact_id_of(artifact: Artifact) -> str:
    """Content address of a full trait-bag Artifact.

    ``meta``, ``produced_by``, ``entity_id``, and ``supersedes`` are NOT part
    of the address.
    """
    return "sha256:" + hashlib.sha256(artifact_canonical_bytes(artifact)).hexdigest()


def artifact_id_single(contract: str, content: bytes) -> str:
    """Content address of a single-trait bag."""
    return artifact_id_of(artifact_with_trait(contract, content))


def artifact_with_trait(contract: str, body: bytes = b"") -> Artifact:
    """Build an in-memory single-trait artifact (not yet stored)."""
    a = Artifact()
    a.traits[contract].body = bytes(body)
    return a


def artifact_with_external_trait(contract: str, obj: ObjectRef) -> Artifact:
    """Build a single external-object trait artifact."""
    a = Artifact()
    a.traits[contract].object.CopyFrom(obj)
    return a


def has_traits(artifact: Artifact, required: list[str]) -> bool:
    """True when the artifact contains every listed trait contract ref."""
    return all(r in artifact.traits for r in required)


def trait_set_covers(have: list[str], need: list[str]) -> bool:
    """True when *have* is a superset of *need*."""
    s = set(have)
    return all(n in s for n in need)


def get_trait(artifact: Artifact, contract: str) -> Trait | None:
    """Return the trait for *contract*, or None."""
    if contract not in artifact.traits:
        return None
    return artifact.traits[contract]


def has_external_object(artifact: Artifact) -> bool:
    """Whether any trait holds a verified external ObjectRef."""
    return any(trait_has_external(f) for f in artifact.traits.values())

def contract_digest(
    media_type: str,
    schema: str,
    version: str,
    compatible_with: list[str] | None = None,
) -> str:
    """Contract content address.

    ``digest = "sha256:" + hex(sha256(
      media_type ‖ 0x00 ‖ schema ‖ 0x00 ‖ version ‖ 0x00 ‖
      compatible_with… (UTF-8 ascending; each entry followed by 0x00)
    ))``

    ``ref`` is the registry key and is NOT folded into the digest. Pass
    ``compatible_with`` already sorted, or use :func:`contract_digest_of`.
    """
    h = hashlib.sha256()
    h.update(media_type.encode("utf-8"))
    h.update(_SEP)
    h.update(schema.encode("utf-8"))
    h.update(_SEP)
    h.update(version.encode("utf-8"))
    h.update(_SEP)
    for c in compatible_with or []:
        h.update(c.encode("utf-8"))
        h.update(_SEP)
    return "sha256:" + h.hexdigest()


def contract_digest_of(contract: Contract) -> str:
    """Compute the content digest, sorting ``compatible_with`` first."""
    compat = sorted(contract.compatible_with)
    return contract_digest(
        contract.media_type, contract.schema, contract.version, compat
    )


def is_contract_placeholder(contract: Contract) -> bool:
    """True for a name-only stub (empty content fields)."""
    return (
        not contract.media_type
        and not contract.schema
        and not contract.version
        and len(contract.compatible_with) == 0
    )


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
