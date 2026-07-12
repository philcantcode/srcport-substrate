"""The in-process microkernel. Methods mirror ``service Kernel`` in the proto."""

from __future__ import annotations

import copy
import hashlib
import queue
import threading

from ._types import (
    AppendRequest,
    Artifact,
    ArtifactRef,
    Assembly,
    AssemblyNode,
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
    ModuleManifest,
    NamedArtifact,
    PublishAck,
    RegisterAck,
    RegistrySnapshot,
    Run,
    RunRef,
    RunRequest,
    RunState,
    Subscription,
    WorkItem,
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


class Invalid(KernelError):
    """The assembly, binding, work result, or transition is invalid."""


class Conflict(KernelError):
    """An id or unit of work already exists."""


class RunClosed(KernelError):
    """A terminal run is immutable and accepts no further work."""

    def __init__(self, state: RunState) -> None:
        self.state = state
        super().__init__(f"run is closed: {RunState.Name(state)}")


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
        self._runs: dict[str, dict] = {}
        self._derivations: list[Derivation] = []
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
                for port in list(cap.inputs) + list(cap.outputs):
                    self._contracts.setdefault(port.contract, Contract(ref=port.contract))
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
            for_log = copy.deepcopy(event)
            for_log.ClearField("payload")
            self._append_locked("event.published", event.topic, _canonical(for_log))
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

    # ── 8. Run / convergence ──────────────────────────────────────────────

    def start_run(self, req: RunRequest) -> Run:
        """Validate and freeze one finite feed-forward assembly."""
        with self._lock:
            if not req.id:
                raise Invalid("run id is required")
            if req.id in self._runs:
                raise Conflict(f"run {req.id} already exists")
            if not req.HasField("assembly"):
                raise Invalid("assembly is required")
            self._validate_assembly(req.assembly, req.inputs)
            max_steps = req.limits.max_steps if req.HasField("limits") else 0
            max_steps = max_steps or len(req.assembly.nodes)
            if not max_steps:
                raise Invalid("max_steps must be positive")
            run = Run(
                id=req.id,
                assembly=req.assembly,
                inputs=req.inputs,
                state=RunState.RUN_STATE_RUNNING,
                max_steps=max_steps,
            )
            self._runs[run.id] = {
                "run": run,
                "claimed": {},
                "committed": {},
            }
            self._append_locked("run.started", run.id, _canonical(run))
            return copy.deepcopy(run)

    def claim_ready(self, req: ClaimRequest) -> WorkItem:
        """Atomically claim one ready node for a module, or return an empty item."""
        with self._lock:
            slot = self._runs.get(req.run_id)
            if slot is None:
                raise NotFound(req.run_id)
            run = slot["run"]
            if run.state != RunState.RUN_STATE_RUNNING:
                raise RunClosed(run.state)
            selected = None
            selected_inputs = None
            any_ready = False
            for node in run.assembly.nodes:
                if node.id in slot["claimed"] or node.id in slot["committed"]:
                    continue
                inputs = self._resolve_inputs(slot, node)
                if inputs is not None:
                    any_ready = True
                    if selected is None and node.module == req.module:
                        selected, selected_inputs = node, inputs
            if selected is not None:
                work = WorkItem(
                    id=f"work:{req.run_id}/{selected.id}",
                    run_id=req.run_id,
                    node_id=selected.id,
                    module=selected.module,
                    module_version=selected.module_version,
                    capability=selected.capability,
                    inputs=selected_inputs,
                )
                slot["claimed"][selected.id] = work
                self._append_locked("work.claimed", work.id, _canonical(work))
                return copy.deepcopy(work)
            if not any_ready and not slot["claimed"]:
                run.state = RunState.RUN_STATE_STALLED
                run.reason = "no node is ready and no work is in flight"
                self._append_locked("run.stalled", run.id, _canonical(run))
            return WorkItem()

    def commit(self, submitted: Derivation) -> Run:
        """Commit one claimed transformation and release its downstream nodes."""
        with self._lock:
            slot = self._runs.get(submitted.run_id)
            if slot is None:
                raise NotFound(submitted.run_id)
            run = slot["run"]
            if run.state != RunState.RUN_STATE_RUNNING:
                raise RunClosed(run.state)
            work = slot["claimed"].get(submitted.node_id)
            if work is None:
                raise Conflict("node was not claimed")
            if submitted.work_id != work.id:
                raise Invalid("work_id does not match the claim")
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
            del slot["claimed"][work.node_id]
            slot["committed"][work.node_id] = derivation
            run.steps += 1
            closed = False
            terminal = run.assembly.terminal
            if terminal.node == work.node_id:
                for output in derivation.outputs:
                    if output.name == terminal.port:
                        run.answer.CopyFrom(output.artifact)
                        run.state = RunState.RUN_STATE_COMPLETED
                        closed = True
                        break
            if not closed and run.steps >= run.max_steps:
                run.state = RunState.RUN_STATE_FAILED
                run.reason = "max_steps exhausted before the terminal output"
                closed = True
            if not closed:
                any_ready = any(
                    node.id not in slot["claimed"]
                    and node.id not in slot["committed"]
                    and self._resolve_inputs(slot, node) is not None
                    for node in run.assembly.nodes
                )
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
            return copy.deepcopy(run)

    def get_run(self, ref: RunRef) -> Run:
        with self._lock:
            slot = self._runs.get(ref.id)
            if slot is None:
                raise NotFound(ref.id)
            return copy.deepcopy(slot["run"])

    def cancel_run(self, ref: RunRef) -> Run:
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

    def list_derivations(self, ref: RunRef) -> DerivationList:
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
                derivation = slot["committed"].get(binding.from_node)
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
