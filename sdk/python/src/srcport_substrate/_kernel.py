"""The in-memory microkernel. Methods mirror ``service Kernel`` in the proto."""

from __future__ import annotations

import copy
import hashlib
import queue
import threading
import time
from typing import Protocol, runtime_checkable

from ._types import (
    AppendRequest,
    Artifact,
    ArtifactRef,
    Assembly,
    AssemblyNode,
    BlobData,
    BlobRef,
    Capability,
    ClaimRequest,
    Closure,
    Contract,
    Derivation,
    DerivationList,
    Error,
    ErrorCode,
    Event,
    Firing,
    GetBlobRequest,
    HasBlobRequest,
    HasBlobResponse,
    InjectInputRequest,
    LedgerEntry,
    Lifecycle,
    ModuleManifest,
    NamedArtifact,
    ObjectRef,
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
    Subscription,
    TransitionAck,
    TransitionRequest,
    WorkItem,
    _ledger_hash,
    artifact_id_of,
    blob_id,
    contract_digest,
    has_external_object,
    is_contract_placeholder,
    verify_chain,
)

# Bound on a single subscriber's undelivered-event backlog. The bus is
# notification, not the data plane; a subscriber that falls this far behind is
# shed rather than allowed to OOM the kernel.
SUBSCRIBER_BUFFER = 1024

# ─── errors ─────────────────────────────────────────────────────────────────


class KernelError(Exception):
    """Base for everything that can go wrong at the ABI seam."""

    def code(self) -> ErrorCode:
        """Portable ErrorCode this failure maps to (same across every SDK)."""
        return ErrorCode.ERROR_CODE_UNSPECIFIED

    def retryable(self) -> bool:
        """Whether re-issuing the identical call may later succeed."""
        return False

    def to_proto(self) -> Error:
        """Project this exception onto the portable Error wire message."""
        return Error(
            code=self.code(),
            message=str(self),
            retryable=self.retryable(),
        )


class NotFound(KernelError):
    """No artifact, blob, or run exists for the given id."""

    def code(self) -> ErrorCode:
        return ErrorCode.ERROR_CODE_NOT_FOUND


class Invalid(KernelError):
    """The assembly, binding, work result, or transition is invalid."""

    def code(self) -> ErrorCode:
        return ErrorCode.ERROR_CODE_INVALID


class Conflict(KernelError):
    """An id or unit of work already exists."""

    def code(self) -> ErrorCode:
        return ErrorCode.ERROR_CODE_CONFLICT

    def to_proto(self) -> Error:
        e = super().to_proto()
        e.conflict_subject = str(self)
        return e


class RunClosed(KernelError):
    """A terminal run is immutable and accepts no further work."""

    def __init__(self, state: RunState) -> None:
        self.state = state
        super().__init__(f"run is closed: {RunState.Name(state)}")

    def code(self) -> ErrorCode:
        return ErrorCode.ERROR_CODE_FAILED_PRECONDITION

    def to_proto(self) -> Error:
        e = super().to_proto()
        e.failed_precondition = str(self)
        return e


class FailedPrecondition(KernelError):
    """A call precondition failed (e.g. absolute deadline already passed)."""

    def code(self) -> ErrorCode:
        return ErrorCode.ERROR_CODE_FAILED_PRECONDITION

    def to_proto(self) -> Error:
        e = super().to_proto()
        e.failed_precondition = str(self)
        return e


class BlobIntegrity(KernelError):
    """Stored blob bytes do not match the claimed digest or byte_count."""

    def code(self) -> ErrorCode:
        return ErrorCode.ERROR_CODE_BLOB_INTEGRITY

    def to_proto(self) -> Error:
        e = super().to_proto()
        e.failed_precondition = str(self)
        return e


def _check_deadline(ctx: RequestContext | None) -> None:
    if ctx is None or not ctx.deadline_unix_ms:
        return
    now_ms = int(time.time() * 1000)
    if now_ms > ctx.deadline_unix_ms:
        raise FailedPrecondition("deadline exceeded")


def _idempotency_key(op: str, ctx: RequestContext | None) -> str | None:
    if ctx is None or not ctx.request_key:
        return None
    return f"{op}\0{ctx.caller}\0{ctx.request_key}"


def _is_sha256_digest(d: str) -> bool:
    if not d.startswith("sha256:") or len(d) != len("sha256:") + 64:
        return False
    return all(c in "0123456789abcdef" for c in d[len("sha256:"):])


_LIFECYCLE_VERB = {
    Lifecycle.LIFECYCLE_UNSPECIFIED: "unspecified",
    Lifecycle.LIFECYCLE_REGISTERED: "registered",
    Lifecycle.LIFECYCLE_LOADED: "loaded",
    Lifecycle.LIFECYCLE_ACTIVE: "activated",
    Lifecycle.LIFECYCLE_DEACTIVATED: "deactivated",
}


def _canonical(msg) -> bytes:
    """Serialize ``msg`` with deterministic field and map ordering.

    Ledger detail is folded into the entry hash, so its encoding MUST be
    canonical — the same logical value has to yield byte-identical detail across
    SDKs and runs, or chains stop cross-verifying (see SPEC.md "Ledger detail").
    """
    return msg.SerializeToString(deterministic=True)


@runtime_checkable
class KernelApi(Protocol):
    """The portable ABI: unary RPCs of ``service Kernel`` (including Transition).

    Streaming ``subscribe`` stays inherent-only on :class:`MemoryKernel`.
    ``RequestContext`` rides as call metadata (optional kwarg) and is
    deliberately not folded into ledger detail.

    Enforced context semantics: past ``deadline_unix_ms`` raises
    :class:`FailedPrecondition`; non-empty ``request_key`` de-duplicates
    ``put_artifact`` / ``start_run`` / ``commit`` by
    ``(caller, request_key, operation)``.
    """

    def register(
        self, manifest: ModuleManifest, ctx: RequestContext | None = None
    ) -> RegisterAck: ...
    def transition(
        self, req: TransitionRequest, ctx: RequestContext | None = None
    ) -> TransitionAck: ...
    def put_artifact(
        self, artifact: Artifact, ctx: RequestContext | None = None
    ) -> ArtifactRef: ...
    def get_artifact(
        self, ref: ArtifactRef, ctx: RequestContext | None = None
    ) -> Artifact: ...
    def put_blob(
        self, req: PutBlobRequest, ctx: RequestContext | None = None
    ) -> BlobRef: ...
    def get_blob(
        self, req: GetBlobRequest, ctx: RequestContext | None = None
    ) -> BlobData: ...
    def has_blob(
        self, req: HasBlobRequest, ctx: RequestContext | None = None
    ) -> HasBlobResponse: ...
    def put_contract(
        self, contract: Contract, ctx: RequestContext | None = None
    ) -> Contract: ...
    def publish(
        self, event: Event, ctx: RequestContext | None = None
    ) -> PublishAck: ...
    def append(
        self, req: AppendRequest, ctx: RequestContext | None = None
    ) -> LedgerEntry: ...
    def snapshot(
        self, req: SnapshotRequest | None = None, ctx: RequestContext | None = None
    ) -> RegistrySnapshot: ...
    def start_run(
        self, req: RunRequest, ctx: RequestContext | None = None
    ) -> Run: ...
    def inject_input(
        self, req: InjectInputRequest, ctx: RequestContext | None = None
    ) -> Run: ...
    def claim_ready(
        self, req: ClaimRequest, ctx: RequestContext | None = None
    ) -> WorkItem: ...
    def commit(
        self, submitted: Derivation, ctx: RequestContext | None = None
    ) -> Run: ...
    def get_run(
        self, ref: RunRef, ctx: RequestContext | None = None
    ) -> Run: ...
    def cancel_run(
        self, ref: RunRef, ctx: RequestContext | None = None
    ) -> Run: ...
    def list_derivations(
        self, ref: RunRef, ctx: RequestContext | None = None
    ) -> DerivationList: ...


class MemoryKernel:
    """In-memory realisation of :class:`KernelApi`. Thread-safe; share one
    instance across module threads. Every meaningful action lands one
    append-only ledger entry. Values handed in and out are copied, so a caller
    can never mutate stored state through a shared message.

    Kernel-state durability is a :class:`KernelApi` backend concern; domain
    state lives in Modules. This type is one backend, not the authority.
    """

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._modules: list[list] = []  # [manifest, lifecycle] pairs
        self._capabilities: list = []
        self._contracts: dict = {}
        self._artifacts: dict = {}
        # (namespace, digest) -> {"data": bytes, "ref": BlobRef}
        self._blobs: dict[tuple[str, str], dict] = {}
        self._subs: list[tuple[list[str], queue.Queue]] = []
        self._ledger: list[LedgerEntry] = []
        self._runs: dict[str, dict] = {}
        self._derivations: list[Derivation] = []
        self._event_seq = 0
        # op\\0caller\\0request_key -> ArtifactRef | Run
        self._idempotency: dict[str, object] = {}

    # ── ledger helper: the ONLY path that creates entries. Caller holds lock.
    def _append_locked(
        self, kind: str, subject: str, detail: bytes = b""
    ) -> LedgerEntry:
        seq = len(self._ledger)
        prev_hash = self._ledger[-1].hash if self._ledger else ""
        entry = LedgerEntry(
            seq=seq,
            kind=kind,
            subject=subject,
            detail=detail,
            prev_hash=prev_hash,
            hash=_ledger_hash(seq, kind, subject, detail, prev_hash),
        )
        self._ledger.append(entry)
        return copy.deepcopy(entry)

    # ── 1. Module ──────────────────────────────────────────────────────────

    def register(self, manifest: ModuleManifest, ctx: RequestContext | None = None) -> RegisterAck:
        """rpc Register. Records the module, its capabilities, and (implicitly)
        name-only placeholders for contracts named on ports. Lands in REGISTERED;
        advance with :meth:`transition`. Placeholders may be filled once via
        :meth:`put_contract`.
        """
        with self._lock:
            for cap in manifest.provides:
                self._capabilities.append(copy.deepcopy(cap))
                for port in list(cap.inputs) + list(cap.outputs):
                    self._ensure_contract_placeholder(port.contract)
            self._modules.append(
                [copy.deepcopy(manifest), Lifecycle.LIFECYCLE_REGISTERED]
            )
            # The full manifest lands in the tamper-evident chain, so the registry
            # is reconstructable from the ledger alone. detail is the canonical
            # ModuleManifest; see SPEC.md "Ledger detail".
            self._append_locked(
                "module.registered", manifest.name, _canonical(manifest)
            )
            return RegisterAck(state=Lifecycle.LIFECYCLE_REGISTERED)

    def _ensure_contract_placeholder(self, ref: str) -> None:
        """Caller holds lock. Name-only stub if ref is new and non-empty."""
        if not ref or ref in self._contracts:
            return
        self._contracts[ref] = Contract(
            ref=ref, digest=contract_digest("", "", "", [])
        )

    def transition(
        self,
        req: TransitionRequest | str,
        to: Lifecycle | None = None,
        ctx: RequestContext | None = None,
    ) -> TransitionAck | Lifecycle:
        """rpc Transition. Advance REGISTERED → LOADED → ACTIVE → DEACTIVATED.

        Accepts either a :class:`TransitionRequest` (ABI form) or
        ``(module, to)`` for convenience. Only a single forward step is
        applied; anything else is a no-op returning the current state.
        """
        if isinstance(req, str):
            if to is None:
                raise Invalid("lifecycle target is required")
            module = req
            target = to
        else:
            module = req.module
            target = req.to
            if ctx is None:
                pass
        _check_deadline(ctx)
        with self._lock:
            for slot in self._modules:
                if slot[0].name == module:
                    if int(target) == int(slot[1]) + 1:
                        slot[1] = target
                        self._append_locked(
                            f"module.{_LIFECYCLE_VERB[target]}", module
                        )
                    if isinstance(req, str):
                        return slot[1]
                    return TransitionAck(state=slot[1])
            raise NotFound(module)

    # ── 2. Artifact ────────────────────────────────────────────────────────

    def put_artifact(self, artifact: Artifact, ctx: RequestContext | None = None) -> ArtifactRef:
        """rpc PutArtifact. Content-addresses the typed value and stores it
        immutably (first write wins).

        Inline: set ``body``, leave ``object`` unset. External: ``put_blob``
        first, then set ``object`` with empty ``body``. The blob must already
        exist and match. Exactly one of body or object may carry the value.
        Honour deadline and ``request_key`` idempotency.
        """
        _check_deadline(ctx)
        self._validate_artifact_content(artifact)
        with self._lock:
            key = _idempotency_key("put_artifact", ctx)
            if key is not None and key in self._idempotency:
                return copy.deepcopy(self._idempotency[key])  # type: ignore[return-value]
            if has_external_object(artifact):
                self._verify_object_ref_locked(artifact.object)
            aid = artifact_id_of(artifact)
            if aid not in self._artifacts:
                stored = copy.deepcopy(artifact)
                stored.id = aid
                self._artifacts[aid] = stored
                # Clear large inline body; keep ObjectRef (small, part of value
                # identity) so external artifacts reconstruct without blob bytes.
                for_log = copy.deepcopy(stored)
                for_log.ClearField("body")
                self._append_locked("artifact.put", aid, _canonical(for_log))
            ref = ArtifactRef(id=aid)
            if key is not None and key not in self._idempotency:
                self._idempotency[key] = copy.deepcopy(ref)
            return ref

    def get_artifact(self, ref: ArtifactRef, ctx: RequestContext | None = None) -> Artifact:
        """rpc GetArtifact. Reads back byte-identical."""
        _check_deadline(ctx)
        with self._lock:
            a = self._artifacts.get(ref.id)
            if a is None:
                raise NotFound(ref.id)
            return copy.deepcopy(a)

    def put_blob(self, req: PutBlobRequest, ctx: RequestContext | None = None) -> BlobRef:
        """rpc PutBlob. Content-addresses raw bytes under (namespace, digest)."""
        data = bytes(req.data)
        digest = blob_id(data)
        ns = req.namespace
        ref = BlobRef(digest=digest, byte_count=len(data), namespace=ns)
        key = (ns, digest)
        with self._lock:
            if key not in self._blobs:
                self._blobs[key] = {"data": data, "ref": copy.deepcopy(ref)}
                self._append_locked("blob.put", digest, _canonical(ref))
            return copy.deepcopy(ref)

    def get_blob(self, req: GetBlobRequest, ctx: RequestContext | None = None) -> BlobData:
        """rpc GetBlob. Returns verified blob bytes (re-hashes on read)."""
        _check_deadline(ctx)
        with self._lock:
            slot = self._blobs.get((req.namespace, req.digest))
            if slot is None:
                raise NotFound(f"blob {req.digest}")
            data = slot["data"]
            ref = slot["ref"]
            if blob_id(data) != ref.digest or len(data) != ref.byte_count:
                raise BlobIntegrity("stored blob corrupted")
            return BlobData(
                digest=ref.digest,
                byte_count=ref.byte_count,
                namespace=ref.namespace,
                data=data,
            )

    def has_blob(self, req: HasBlobRequest, ctx: RequestContext | None = None) -> HasBlobResponse:
        """rpc HasBlob."""
        with self._lock:
            slot = self._blobs.get((req.namespace, req.digest))
            if slot is None:
                return HasBlobResponse(exists=False)
            return HasBlobResponse(exists=True, byte_count=slot["ref"].byte_count)

    def put_artifact_with_blob(
        self,
        type: str,
        namespace: str,
        data: bytes,
        produced_by: str = "",
        ctx: RequestContext | None = None,
    ) -> tuple[ArtifactRef, BlobRef]:
        """Put the blob then an external artifact referencing it."""
        blob = self.put_blob(PutBlobRequest(namespace=namespace, data=data))
        ref = self.put_artifact(
            Artifact(
                type=type,
                produced_by=produced_by,
                object=ObjectRef(
                    digest=blob.digest,
                    byte_count=blob.byte_count,
                    namespace=blob.namespace,
                ),
            )
        )
        return ref, blob

    @staticmethod
    def _validate_artifact_content(artifact: Artifact) -> None:
        if not artifact.type:
            raise Invalid("artifact type is required")
        has_obj = has_external_object(artifact)
        has_body = bool(artifact.body)
        if has_obj and has_body:
            raise Invalid("artifact must not set both body and object")
        obj = artifact.object
        if obj.digest == "" and (obj.byte_count != 0 or obj.namespace != ""):
            raise Invalid("object.digest is required when object is set")
        if has_obj and not _is_sha256_digest(obj.digest):
            raise Invalid("object.digest must be sha256:<hex>")

    def _verify_object_ref_locked(self, obj: ObjectRef) -> None:
        slot = self._blobs.get((obj.namespace, obj.digest))
        if slot is None:
            raise NotFound(f"blob {obj.digest} (namespace {obj.namespace!r})")
        ref = slot["ref"]
        data = slot["data"]
        if ref.byte_count != obj.byte_count:
            raise BlobIntegrity(
                f"object.byte_count {obj.byte_count} != stored {ref.byte_count}"
            )
        if blob_id(data) != obj.digest or len(data) != obj.byte_count:
            raise BlobIntegrity("blob does not match object ref")

    # ── 3. Contract ────────────────────────────────────────────────────────

    def put_contract(self, contract: Contract, ctx: RequestContext | None = None) -> Contract:
        """rpc PutContract. Register a contract immutably under its ref.

        Returns the stored contract (digest assigned). Identical re-puts are
        no-ops; different content under the same ref raises :class:`Conflict`.
        A name-only placeholder from :meth:`register` may be filled once.
        """
        _check_deadline(ctx)
        if not contract.ref:
            raise Invalid("contract ref is required")
        c = copy.deepcopy(contract)
        # Normalize compatible_with to UTF-8 ascending for stable identity.
        c.ClearField("compatible_with")
        c.compatible_with.extend(sorted(contract.compatible_with))
        digest = contract_digest(c.media_type, c.schema, c.version, list(c.compatible_with))
        if c.digest and c.digest != digest:
            raise Invalid("contract digest mismatch")
        c.digest = digest

        with self._lock:
            existing = self._contracts.get(c.ref)
            if existing is not None:
                if existing.digest == c.digest:
                    return copy.deepcopy(existing)
                if is_contract_placeholder(existing) and not is_contract_placeholder(c):
                    stored = copy.deepcopy(c)
                    self._contracts[c.ref] = stored
                    self._append_locked(
                        "contract.registered", c.ref, _canonical(stored)
                    )
                    return copy.deepcopy(stored)
                raise Conflict(
                    f"contract {c.ref} already registered with different content"
                )
            stored = copy.deepcopy(c)
            self._contracts[c.ref] = stored
            self._append_locked("contract.registered", c.ref, _canonical(stored))
            return copy.deepcopy(stored)

    # ── 4. Event ───────────────────────────────────────────────────────────

    def subscribe(self, sub: Subscription, ctx: RequestContext | None = None) -> "queue.Queue[Event]":
        """rpc Subscribe. In-process the "stream" is a bounded Queue; events
        arrive in kernel seq order. A module only receives events on topics it
        named. A subscriber that falls SUBSCRIBER_BUFFER behind is shed on
        publish so one slow consumer cannot OOM the kernel.
        """
        q: "queue.Queue[Event]" = queue.Queue(maxsize=SUBSCRIBER_BUFFER)
        with self._lock:
            self._subs.append((list(sub.topics), q))
        return q

    def publish(self, event: Event, ctx: RequestContext | None = None) -> PublishAck:
        """rpc Publish. Assigns a monotonic seq, delivers to exactly the
        subscribers of event.topic and no one else, returns the assigned seq.
        A slow subscriber whose buffer is full is shed; dropped notifications
        remain reconstructable from the ledger.
        """
        with self._lock:
            self._event_seq += 1
            event = copy.deepcopy(event)
            event.seq = self._event_seq
            alive: list[tuple[list[str], queue.Queue]] = []
            for topics, q in self._subs:
                if event.topic in topics:
                    try:
                        q.put_nowait(copy.deepcopy(event))
                        alive.append((topics, q))
                    except queue.Full:
                        pass  # shed
                else:
                    alive.append((topics, q))
            self._subs = alive
            # Artifact refs are the data plane; the Event lands fully in the chain.
            self._append_locked("event.published", event.topic, _canonical(event))
            return PublishAck(seq=event.seq)

    # ── 5. Ledger ──────────────────────────────────────────────────────────

    def append(self, req: AppendRequest, ctx: RequestContext | None = None) -> LedgerEntry:
        """rpc Append. Modules write their own domain facts into the same chain."""
        with self._lock:
            return self._append_locked(req.kind, req.subject, req.detail)

    def ledger(self) -> list[LedgerEntry]:
        """A snapshot copy of the whole ledger, for verification/audit."""
        with self._lock:
            return [copy.deepcopy(e) for e in self._ledger]

    def verify_ledger(self) -> bool:
        """Verify the kernel's own live ledger."""
        with self._lock:
            return verify_chain(self._ledger)

    # ── 6. Registry ────────────────────────────────────────────────────────

    def snapshot(self, req: SnapshotRequest | None = None, ctx: RequestContext | None = None) -> RegistrySnapshot:
        """rpc Snapshot. "What exists right now": every module, capability, contract."""
        with self._lock:
            return RegistrySnapshot(
                modules=[copy.deepcopy(s[0]) for s in self._modules],
                capabilities=[copy.deepcopy(c) for c in self._capabilities],
                contracts=[copy.deepcopy(c) for c in self._contracts.values()],
            )

    # ── 7. Run / convergence ──────────────────────────────────────────────

    def start_run(self, req: RunRequest, ctx: RequestContext | None = None) -> Run:
        """Validate and freeze one finite feed-forward assembly.

        Honour deadline and ``request_key`` idempotency.
        """
        _check_deadline(ctx)
        with self._lock:
            key = _idempotency_key("start_run", ctx)
            if key is not None and key in self._idempotency:
                return copy.deepcopy(self._idempotency[key])  # type: ignore[return-value]
            if not req.id:
                raise Invalid("run id is required")
            if req.id in self._runs:
                raise Conflict(f"run {req.id} already exists")
            if not req.HasField("assembly"):
                raise Invalid("assembly is required")
            assembly = _materialize_assembly(req.assembly, list(req.include_nodes))
            self._validate_assembly(assembly, req.inputs)
            max_steps = req.limits.max_steps if req.HasField("limits") else 0
            max_steps = max_steps or len(assembly.nodes)
            if not max_steps:
                raise Invalid("max_steps must be positive")
            run = Run(
                id=req.id,
                assembly=assembly,
                inputs=req.inputs,
                state=RunState.RUN_STATE_RUNNING,
                max_steps=max_steps,
            )
            if req.HasField("policy"):
                run.policy.CopyFrom(req.policy)
            epochs = {item.name: 0 for item in req.inputs}
            self._runs[run.id] = {
                "run": run,
                "claimed": {},
                "latest": {},
                "done_units": set(),
                "input_epochs": epochs,
                "node_commits": {},
            }
            self._append_locked("run.started", run.id, _canonical(run))
            out = copy.deepcopy(run)
            if key is not None and key not in self._idempotency:
                self._idempotency[key] = copy.deepcopy(out)
            return out

    def inject_input(
        self, req: InjectInputRequest, ctx: RequestContext | None = None
    ) -> Run:
        """Admit or replace a named run input while RUNNING."""
        _check_deadline(ctx)
        with self._lock:
            slot = self._runs.get(req.run_id)
            if slot is None:
                raise NotFound(req.run_id)
            run = slot["run"]
            if run.state != RunState.RUN_STATE_RUNNING:
                raise RunClosed(run.state)
            if not req.HasField("input") or not req.input.name:
                raise Invalid("input is required")
            if not req.input.HasField("artifact"):
                raise Invalid(f"input {req.input.name} has no artifact")
            if req.input.artifact.id not in self._artifacts:
                raise NotFound(req.input.artifact.id)
            used = any(b.input == req.input.name for b in run.assembly.bindings)
            if not used:
                raise Invalid(
                    f"run input {req.input.name} is not bound in the assembly"
                )
            found = False
            for i, item in enumerate(run.inputs):
                if item.name == req.input.name:
                    run.inputs[i].CopyFrom(req.input)
                    found = True
                    break
            if not found:
                run.inputs.append(req.input)
            slot["input_epochs"][req.input.name] = (
                slot["input_epochs"].get(req.input.name, 0) + 1
            )
            self._append_locked(
                "run.input_injected", run.id, _canonical(req.input)
            )
            return copy.deepcopy(run)

    def claim_ready(self, req: ClaimRequest, ctx: RequestContext | None = None) -> WorkItem:
        """Atomically claim one ready work unit for a module, or return empty."""
        _check_deadline(ctx)
        with self._lock:
            slot = self._runs.get(req.run_id)
            if slot is None:
                raise NotFound(req.run_id)
            run = slot["run"]
            if run.state != RunState.RUN_STATE_RUNNING:
                raise RunClosed(run.state)
            selected = None
            selected_inputs = None
            unit_key = None
            any_ready = False
            for node in run.assembly.nodes:
                inputs = self._resolve_inputs(slot, node)
                if inputs is None:
                    continue
                try:
                    cap = self._capability_for(
                        node.module, node.module_version, node.capability
                    )
                except Invalid:
                    continue
                firing = _effective_firing(slot, node, cap)
                key = _work_unit_key(slot, node, cap, firing, inputs)
                if key is None or key in slot["done_units"]:
                    continue
                any_ready = True
                if selected is None and node.module == req.module:
                    selected, selected_inputs, unit_key = node, inputs, key
            if selected is not None and unit_key is not None:
                if unit_key.startswith("once:"):
                    work_id = f"work:{req.run_id}/{selected.id}"
                else:
                    work_id = f"work:{req.run_id}/{unit_key}"
                work = WorkItem(
                    id=work_id,
                    run_id=req.run_id,
                    node_id=selected.id,
                    module=selected.module,
                    module_version=selected.module_version,
                    capability=selected.capability,
                    inputs=selected_inputs,
                )
                slot["done_units"].add(unit_key)
                slot["claimed"][work.id] = work
                self._append_locked("work.claimed", work.id, _canonical(work))
                return copy.deepcopy(work)
            if (
                not any_ready
                and not slot["claimed"]
                and not _is_open_closure(slot)
            ):
                run.state = RunState.RUN_STATE_STALLED
                run.reason = "no node is ready and no work is in flight"
                self._append_locked("run.stalled", run.id, _canonical(run))
            return WorkItem()

    def commit(self, submitted: Derivation, ctx: RequestContext | None = None) -> Run:
        """Commit one claimed transformation and release its downstream work.

        Honour deadline and ``request_key`` idempotency.
        """
        _check_deadline(ctx)
        with self._lock:
            key = _idempotency_key("commit", ctx)
            if key is not None and key in self._idempotency:
                return copy.deepcopy(self._idempotency[key])  # type: ignore[return-value]
            slot = self._runs.get(submitted.run_id)
            if slot is None:
                raise NotFound(submitted.run_id)
            run = slot["run"]
            if run.state != RunState.RUN_STATE_RUNNING:
                raise RunClosed(run.state)
            work = slot["claimed"].get(submitted.work_id)
            if work is None:
                raise Conflict("work was not claimed")
            if submitted.node_id and submitted.node_id != work.node_id:
                raise Invalid("node_id does not match the claim")
            cap = self._capability_for(
                work.module, work.module_version, work.capability
            )
            self._validate_outputs(cap, submitted.outputs)
            derivation = Derivation(
                run_id=work.run_id,
                work_id=work.id,
                node_id=work.node_id,
                module=work.module,
                module_version=work.module_version,
                capability=work.capability,
                inputs=work.inputs,
                outputs=submitted.outputs,
            )
            derivation.id = _derivation_id(derivation)
            del slot["claimed"][work.id]
            slot["latest"][work.node_id] = derivation
            slot["node_commits"][work.node_id] = (
                slot["node_commits"].get(work.node_id, 0) + 1
            )
            run.steps += 1
            closed = False
            open_run = _is_open_closure(slot)
            terminal = run.assembly.terminal
            if terminal.node == work.node_id:
                for output in derivation.outputs:
                    if output.name == terminal.port:
                        run.answer.CopyFrom(output.artifact)
                        if not open_run:
                            run.state = RunState.RUN_STATE_COMPLETED
                            closed = True
                        break
            if not closed and run.steps >= run.max_steps:
                run.state = RunState.RUN_STATE_FAILED
                run.reason = "max_steps exhausted before the terminal output"
                closed = True
            if not closed and not open_run:
                any_ready = self._assembly_any_ready(slot)
                if not any_ready and not slot["claimed"]:
                    run.state = RunState.RUN_STATE_STALLED
                    run.reason = "no node is ready and no work is in flight"
                    closed = True
            self._derivations.append(copy.deepcopy(derivation))
            self._append_locked(
                "derivation.committed",
                derivation.id,
                _canonical(derivation),
            )
            kind = "run.progressed"
            if run.state == RunState.RUN_STATE_COMPLETED:
                kind = "run.completed"
            elif run.state == RunState.RUN_STATE_STALLED:
                kind = "run.stalled"
            elif closed:
                kind = "run.failed"
            self._append_locked(kind, run.id, _canonical(run))
            out = copy.deepcopy(run)
            if key is not None and key not in self._idempotency:
                self._idempotency[key] = copy.deepcopy(out)
            return out

    def get_run(self, ref: RunRef, ctx: RequestContext | None = None) -> Run:
        _check_deadline(ctx)
        with self._lock:
            slot = self._runs.get(ref.id)
            if slot is None:
                raise NotFound(ref.id)
            return copy.deepcopy(slot["run"])

    def cancel_run(self, ref: RunRef, ctx: RequestContext | None = None) -> Run:
        _check_deadline(ctx)
        with self._lock:
            slot = self._runs.get(ref.id)
            if slot is None:
                raise NotFound(ref.id)
            run = slot["run"]
            if run.state != RunState.RUN_STATE_RUNNING:
                raise RunClosed(run.state)
            run.state = RunState.RUN_STATE_CANCELLED
            run.reason = "cancelled"
            slot["claimed"].clear()
            self._append_locked("run.cancelled", run.id, _canonical(run))
            return copy.deepcopy(run)

    def derivations(self) -> list[Derivation]:
        with self._lock:
            return copy.deepcopy(self._derivations)

    def list_derivations(self, ref: RunRef, ctx: RequestContext | None = None) -> DerivationList:
        _check_deadline(ctx)
        with self._lock:
            if ref.id not in self._runs:
                raise NotFound(ref.id)
            return DerivationList(
                derivations=[
                    copy.deepcopy(item)
                    for item in self._derivations
                    if item.run_id == ref.id
                ]
            )

    def _capability_for(self, module: str, version: str, capability: str):
        matches = []
        for manifest, _ in self._modules:
            if manifest.name == module and manifest.version == version:
                for cap in manifest.provides:
                    if cap.name == capability:
                        matches.append(cap)
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            raise Invalid(f"{module}@{version} provides {capability} ambiguously")
        raise Invalid(f"{module}@{version} does not provide {capability}")

    @staticmethod
    def _port(ports, name: str):
        return next((port for port in ports if port.name == name), None)

    def _validate_assembly(self, assembly: Assembly, inputs) -> None:
        if not assembly.id:
            raise Invalid("assembly id is required")
        if not assembly.nodes or not assembly.HasField("terminal"):
            raise Invalid("assembly needs nodes and a terminal")
        nodes = {}
        for node in assembly.nodes:
            if not node.id or node.id in nodes:
                raise Invalid(f"duplicate or empty node id: {node.id}")
            nodes[node.id] = node
            cap = self._capability_for(
                node.module, node.module_version, node.capability
            )
            for ports in (cap.inputs, cap.outputs):
                names = set()
                for port in ports:
                    if (
                        not port.name
                        or not port.contract
                        or port.name in names
                    ):
                        raise Invalid(
                            f"{node.id} has an empty or duplicate typed port"
                        )
                    names.add(port.name)
        terminal_node = nodes.get(assembly.terminal.node)
        if terminal_node is None:
            raise Invalid("terminal node does not exist")
        terminal_cap = self._capability_for(
            terminal_node.module,
            terminal_node.module_version,
            terminal_node.capability,
        )
        terminal_port = self._port(terminal_cap.outputs, assembly.terminal.port)
        if terminal_port is None:
            raise Invalid("terminal output does not exist")
        if terminal_port.multiple:
            raise Invalid("terminal output must be scalar")
        named_inputs = {}
        for item in inputs:
            if not item.name or item.name in named_inputs:
                raise Invalid(f"duplicate or empty run input: {item.name}")
            if not item.HasField("artifact"):
                raise Invalid(f"input {item.name} has no artifact")
            if item.artifact.id not in self._artifacts:
                raise NotFound(item.artifact.id)
            named_inputs[item.name] = item
        counts = {}
        edges = {}
        for binding in assembly.bindings:
            target_node = nodes.get(binding.to_node)
            if target_node is None:
                raise Invalid(f"unknown target {binding.to_node}")
            target_cap = self._capability_for(
                target_node.module,
                target_node.module_version,
                target_node.capability,
            )
            target = self._port(target_cap.inputs, binding.to_port)
            if target is None:
                raise Invalid(f"unknown input {binding.to_node}.{binding.to_port}")
            key = (binding.to_node, binding.to_port)
            counts[key] = counts.get(key, 0) + 1
            upstream = bool(binding.from_node or binding.from_port)
            external = bool(binding.input)
            if upstream == external:
                raise Invalid("binding must have exactly one source")
            if external:
                item = named_inputs.get(binding.input)
                if item is None:
                    raise Invalid(f"unknown run input {binding.input}")
                contract = self._artifacts[item.artifact.id].type
            else:
                source_node = nodes.get(binding.from_node)
                if source_node is None:
                    raise Invalid(f"unknown source {binding.from_node}")
                source_cap = self._capability_for(
                    source_node.module,
                    source_node.module_version,
                    source_node.capability,
                )
                source = self._port(source_cap.outputs, binding.from_port)
                if source is None:
                    raise Invalid(
                        f"unknown output {binding.from_node}.{binding.from_port}"
                    )
                contract = source.contract
                edges.setdefault(binding.from_node, []).append(binding.to_node)
            if contract != target.contract:
                raise Invalid(
                    f"contract mismatch at {binding.to_node}.{binding.to_port}"
                )
        for node in assembly.nodes:
            cap = self._capability_for(
                node.module, node.module_version, node.capability
            )
            for port in cap.inputs:
                count = counts.get((node.id, port.name), 0)
                if not count and not port.optional:
                    raise Invalid(f"required input {node.id}.{port.name} is unbound")
                if count > 1 and not port.multiple:
                    raise Invalid(f"input {node.id}.{port.name} is not multiple")
        visiting, done = set(), set()

        def visit(node):
            if node in done:
                return
            if node in visiting:
                raise Invalid("assembly contains a cycle")
            visiting.add(node)
            for downstream in edges.get(node, []):
                visit(downstream)
            visiting.remove(node)
            done.add(node)

        for node in nodes:
            visit(node)

    def _assembly_any_ready(self, slot) -> bool:
        for node in slot["run"].assembly.nodes:
            inputs = self._resolve_inputs(slot, node)
            if inputs is None:
                continue
            try:
                cap = self._capability_for(
                    node.module, node.module_version, node.capability
                )
            except Invalid:
                continue
            firing = _effective_firing(slot, node, cap)
            key = _work_unit_key(slot, node, cap, firing, inputs)
            if key is not None and key not in slot["done_units"]:
                return True
        return False

    def _validate_outputs(self, cap, outputs) -> None:
        for expected in cap.outputs:
            matching = [item for item in outputs if item.name == expected.name]
            if not matching and not expected.optional:
                raise Invalid(f"required output {expected.name} is absent")
            if len(matching) > 1 and not expected.multiple:
                raise Invalid(f"output {expected.name} is not multiple")
        for output in outputs:
            expected = self._port(cap.outputs, output.name)
            if expected is None or not output.HasField("artifact"):
                raise Invalid(f"undeclared or empty output {output.name}")
            artifact = self._artifacts.get(output.artifact.id)
            if artifact is None:
                raise NotFound(output.artifact.id)
            if artifact.type != expected.contract:
                raise Invalid(
                    f"output {output.name} has contract {artifact.type}, "
                    f"want {expected.contract}"
                )

    @staticmethod
    def _resolve_inputs(slot, node: AssemblyNode):
        resolved = []
        for binding in slot["run"].assembly.bindings:
            if binding.to_node != node.id:
                continue
            ref = None
            if binding.input:
                item = next(
                    (
                        item
                        for item in slot["run"].inputs
                        if item.name == binding.input
                    ),
                    None,
                )
                ref = item.artifact if item is not None else None
            else:
                derivation = slot["latest"].get(binding.from_node)
                if derivation is not None:
                    item = next(
                        (
                            item
                            for item in derivation.outputs
                            if item.name == binding.from_port
                        ),
                        None,
                    )
                    ref = item.artifact if item is not None else None
            if ref is None:
                return None
            resolved.append(NamedArtifact(name=binding.to_port, artifact=ref))
        return resolved


def _materialize_assembly(assembly: Assembly, include: list[str]) -> Assembly:
    if not include:
        return assembly
    if len(set(include)) != len(include):
        raise Invalid("include_nodes contains duplicates")
    known = {n.id for n in assembly.nodes}
    for node_id in include:
        if node_id not in known:
            raise Invalid(f"include_nodes references unknown node {node_id}")
    if not assembly.HasField("terminal") or assembly.terminal.node not in include:
        raise Invalid("include_nodes must retain the terminal node")
    want = set(include)
    out = Assembly(id=assembly.id, terminal=assembly.terminal)
    for node in assembly.nodes:
        if node.id in want:
            out.nodes.append(node)
    for binding in assembly.bindings:
        if binding.to_node not in want:
            continue
        if not binding.input and binding.from_node not in want:
            continue
        out.bindings.append(binding)
    return out


def _is_open_closure(slot) -> bool:
    run = slot["run"]
    return run.HasField("policy") and run.policy.closure == Closure.CLOSURE_OPEN


def _effective_firing(slot, node: AssemblyNode, cap: Capability) -> Firing:
    run = slot["run"]
    if run.HasField("policy") and node.id in run.policy.by_node:
        f = run.policy.by_node[node.id]
        if f != Firing.FIRING_UNSPECIFIED:
            return f
    if cap.firing != Firing.FIRING_UNSPECIFIED:
        return cap.firing
    if run.HasField("policy") and run.policy.default != Firing.FIRING_UNSPECIFIED:
        return run.policy.default
    return Firing.FIRING_ONCE


def _work_unit_key(slot, node, cap, firing, inputs):
    if firing == Firing.FIRING_ALWAYS:
        fp = _delivery_fingerprint(slot, node, inputs)
        if fp is None:
            return None
        return f"always:{node.id}:{fp}"
    if firing == Firing.FIRING_ONCE_PER_KEY:
        return f"key:{node.id}:{_input_key(cap, inputs)}"
    return f"once:{node.id}"


def _input_key(cap, inputs) -> str:
    marked = [p.name for p in cap.inputs if p.key]
    pairs = []
    for item in inputs:
        if not item.HasField("artifact"):
            continue
        if not marked or item.name in marked:
            pairs.append((item.name, item.artifact.id))
    pairs.sort(key=lambda p: p[0])
    h = hashlib.sha256()
    for name, art_id in pairs:
        h.update(name.encode())
        h.update(b"\x00")
        h.update(art_id.encode())
        h.update(b"\x00")
    return h.hexdigest()


def _delivery_fingerprint(slot, node, inputs):
    rows = []
    for binding in slot["run"].assembly.bindings:
        if binding.to_node != node.id:
            continue
        art = next((i for i in inputs if i.name == binding.to_port), None)
        if art is None or not art.HasField("artifact"):
            return None
        if binding.input:
            epoch = slot["input_epochs"].get(binding.input, 0)
        else:
            epoch = slot["node_commits"].get(binding.from_node, 0)
        rows.append((binding.to_port, art.artifact.id, epoch))
    rows.sort(key=lambda r: r[0])
    h = hashlib.sha256()
    for port, art_id, epoch in rows:
        h.update(port.encode())
        h.update(b"\x00")
        h.update(art_id.encode())
        h.update(b"\x00")
        h.update(int(epoch).to_bytes(8, "big"))
        h.update(b"\x00")
    return h.hexdigest()


def _derivation_id(derivation: Derivation) -> str:
    h = hashlib.sha256()

    def write(value: str) -> None:
        h.update(value.encode())
        h.update(b"\x00")

    for value in (
        derivation.run_id,
        derivation.work_id,
        derivation.node_id,
        derivation.module,
        derivation.module_version,
        derivation.capability,
    ):
        write(value)
    for value in list(derivation.inputs) + list(derivation.outputs):
        write(value.name)
        write(value.artifact.id if value.HasField("artifact") else "")
    return "sha256:" + h.hexdigest()
