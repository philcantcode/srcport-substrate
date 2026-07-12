"""srcport-substrate — Python SDK (v0.1, in-process).

One pluggable core: seven primitives (Module · Artifact · Contract · Event ·
Ledger · Gate · Registry) and one Kernel ABI, conformant to SPEC.md. The
:class:`Kernel` methods mirror the ``service Kernel`` RPCs in substrate.proto.
"""

from ._kernel import GateBlocked, Kernel, KernelError, NotADecision, NotFound
from ._types import (
    AppendRequest,
    Artifact,
    ArtifactRef,
    Capability,
    Contract,
    Decision,
    Event,
    GateDecision,
    GateRequest,
    GateTicket,
    LedgerEntry,
    Lifecycle,
    ModuleManifest,
    PublishAck,
    RegisterAck,
    RegistrySnapshot,
    Subscription,
    artifact_id,
    verify_chain,
)

__version__ = "0.1.0"

__all__ = [
    "Kernel",
    "KernelError",
    "NotFound",
    "NotADecision",
    "GateBlocked",
    "Lifecycle",
    "Decision",
    "Capability",
    "ModuleManifest",
    "Artifact",
    "ArtifactRef",
    "Contract",
    "Event",
    "Subscription",
    "LedgerEntry",
    "GateRequest",
    "GateDecision",
    "GateTicket",
    "RegistrySnapshot",
    "RegisterAck",
    "PublishAck",
    "AppendRequest",
    "artifact_id",
    "verify_chain",
    "__version__",
]
