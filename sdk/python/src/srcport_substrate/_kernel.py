"""The in-process microkernel. Methods mirror ``service Kernel`` in the proto."""

from __future__ import annotations

import copy
import queue
import threading

from ._types import (
    AppendRequest,
    Artifact,
    ArtifactRef,
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
    _ledger_hash,
    artifact_id,
    verify_chain,
)

# ─── errors ─────────────────────────────────────────────────────────────────


class KernelError(Exception):
    """Base for everything that can go wrong at the ABI seam."""


class NotFound(KernelError):
    """No artifact or gate exists for the given id."""


class NotADecision(KernelError):
    """A GateDecision carried something other than APPROVED/REJECTED."""


class GateBlocked(KernelError):
    """An irreversible action was attempted while the gate was not APPROVED."""

    def __init__(self, decision: Decision) -> None:
        self.decision = decision
        name = Decision.Name(decision)
        super().__init__(f"gate blocked: decision is {name}, not APPROVED")


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


class Kernel:
    """The in-process microkernel. Thread-safe; share one instance across
    module threads. Every meaningful action lands one append-only ledger entry.
    Values handed in and out are copied, so a caller can never mutate stored
    state through a shared message.
    """

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._gate_cv = threading.Condition(self._lock)
        self._modules: list[list] = []  # [manifest, lifecycle] pairs
        self._capabilities: list = []
        self._contracts: dict = {}
        self._artifacts: dict = {}
        self._subs: list[tuple[list[str], queue.Queue]] = []
        self._ledger: list[LedgerEntry] = []
        self._gates: dict[str, GateDecision] = {}
        self._event_seq = 0
        self._gate_counter = 0

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

    def register(self, manifest: ModuleManifest) -> RegisterAck:
        """rpc Register. Records the module, its capabilities, and (implicitly)
        the contracts they speak. Lands in REGISTERED; advance with transition().
        """
        with self._lock:
            for cap in manifest.provides:
                self._capabilities.append(copy.deepcopy(cap))
                self._contracts.setdefault(cap.contract, Contract(ref=cap.contract))
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

    def transition(self, module: str, to: Lifecycle) -> Lifecycle:
        """Advance REGISTERED → LOADED → ACTIVE → DEACTIVATED. Only forward
        moves are honoured; anything else is a no-op returning the current state.
        """
        with self._lock:
            for slot in self._modules:
                if slot[0].name == module:
                    if int(to) == int(slot[1]) + 1:
                        slot[1] = to
                        self._append_locked(f"module.{_LIFECYCLE_VERB[to]}", module)
                    return slot[1]
            raise NotFound(module)

    # ── 2. Artifact ────────────────────────────────────────────────────────

    def put_artifact(self, artifact: Artifact) -> ArtifactRef:
        """rpc PutArtifact. Content-addresses and stores immutably (first write
        wins — a later put of the same id never mutates what is stored).
        """
        aid = artifact_id(artifact.type, artifact.body)
        with self._lock:
            if aid not in self._artifacts:
                stored = copy.deepcopy(artifact)
                stored.id = aid
                self._artifacts[aid] = stored
                # The ledger commits to everything but the body: subject (the id)
                # already addresses (type, body), so re-inlining the body would
                # duplicate the store into a log it can never prune. detail is the
                # canonical Artifact with body cleared; meta, produced_by, and
                # derived_from ride along.
                for_log = copy.deepcopy(stored)
                for_log.ClearField("body")
                self._append_locked("artifact.put", aid, _canonical(for_log))
            return ArtifactRef(id=aid)

    def get_artifact(self, ref: ArtifactRef) -> Artifact:
        """rpc GetArtifact. Reads back byte-identical."""
        with self._lock:
            a = self._artifacts.get(ref.id)
            if a is None:
                raise NotFound(ref.id)
            return copy.deepcopy(a)

    # ── 3. Contract ────────────────────────────────────────────────────────

    def put_contract(self, contract: Contract) -> None:
        """Register (or attach schema text to) a contract explicitly."""
        with self._lock:
            self._contracts[contract.ref] = copy.deepcopy(contract)

    # ── 4. Event ───────────────────────────────────────────────────────────

    def subscribe(self, sub: Subscription) -> "queue.Queue[Event]":
        """rpc Subscribe. In-process the "stream" is a Queue; events arrive in
        kernel seq order. A module only receives events on topics it named.
        """
        q: "queue.Queue[Event]" = queue.Queue()
        with self._lock:
            self._subs.append((list(sub.topics), q))
        return q

    def publish(self, event: Event) -> PublishAck:
        """rpc Publish. Assigns a monotonic seq, delivers to exactly the
        subscribers of event.topic and no one else, returns the assigned seq.
        """
        with self._lock:
            self._event_seq += 1
            event = copy.deepcopy(event)
            event.seq = self._event_seq
            for topics, q in self._subs:
                if event.topic in topics:
                    q.put(copy.deepcopy(event))
            self._append_locked("event.published", event.topic)
            return PublishAck(seq=event.seq)

    # ── 5. Ledger ──────────────────────────────────────────────────────────

    def append(self, req: AppendRequest) -> LedgerEntry:
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

    # ── 6. Gate ────────────────────────────────────────────────────────────

    def request_gate(self, req: GateRequest) -> GateTicket:
        """rpc RequestGate. Opens a human-held checkpoint in PENDING."""
        with self._lock:
            rid = req.id
            if not rid:
                self._gate_counter += 1
                rid = f"gate-{self._gate_counter}"
            self._gates[rid] = GateDecision(
                request_id=rid, decision=Decision.DECISION_PENDING
            )
            # The full request lands in the tamper-evident chain — action,
            # requested_by, and context are the evidence a human (or an auditor
            # after a restart) reconstructs. detail is the canonical GateRequest,
            # with its assigned id.
            logged = copy.deepcopy(req)
            logged.id = rid
            self._append_locked("gate.requested", rid, _canonical(logged))
            return GateTicket(request_id=rid)

    def decide_gate(self, decision: GateDecision) -> GateDecision:
        """rpc DecideGate. Records APPROVED/REJECTED and wakes await_gate()."""
        if decision.decision not in (Decision.DECISION_APPROVED, Decision.DECISION_REJECTED):
            raise NotADecision()
        with self._gate_cv:
            if decision.request_id not in self._gates:
                raise NotFound(decision.request_id)
            self._gates[decision.request_id] = copy.deepcopy(decision)
            # The decision itself — who decided, what, and why — is hash-committed,
            # so the approval record can't be rewritten without breaking the chain.
            self._append_locked(
                "gate.decided", decision.request_id, _canonical(decision)
            )
            self._gate_cv.notify_all()
            return copy.deepcopy(decision)

    def await_gate(self, ticket: GateTicket) -> GateDecision:
        """rpc AwaitGate. Blocks until the gate is no longer PENDING."""
        with self._gate_cv:
            while True:
                g = self._gates.get(ticket.request_id)
                if g is None:
                    raise NotFound(ticket.request_id)
                if g.decision != Decision.DECISION_PENDING:
                    return copy.deepcopy(g)
                self._gate_cv.wait()

    def gate_status(self, ticket: GateTicket) -> Decision:
        """Non-blocking peek at a gate's current decision."""
        with self._lock:
            g = self._gates.get(ticket.request_id)
            if g is None:
                raise NotFound(ticket.request_id)
            return g.decision

    def ensure_approved(self, ticket: GateTicket) -> None:
        """The non-bypass guard: returns None only when APPROVED; PENDING and
        REJECTED both raise GateBlocked. Call before an irreversible act.
        """
        d = self.gate_status(ticket)
        if d != Decision.DECISION_APPROVED:
            raise GateBlocked(d)

    # ── 7. Registry ────────────────────────────────────────────────────────

    def snapshot(self) -> RegistrySnapshot:
        """rpc Snapshot. "What exists right now": every module, capability, contract."""
        with self._lock:
            return RegistrySnapshot(
                modules=[copy.deepcopy(s[0]) for s in self._modules],
                capabilities=[copy.deepcopy(c) for c in self._capabilities],
                contracts=[copy.deepcopy(c) for c in self._contracts.values()],
            )
