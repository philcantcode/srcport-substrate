"""Opinionated run modes that compile to kernel ExecutionPolicy + host drive rules."""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from enum import Enum, auto

from srcport_substrate import Assembly, Closure, ExecutionPolicy, Firing, RunRequest

from .memo import MemoPlan
from .storage import StoragePlan

# Default host concurrency when policy leaves it unset.
DEFAULT_CONCURRENCY = 8


class RunMode(Enum):
    CONVERGE = auto()
    STREAM = auto()
    DEDUPE_STREAM = auto()
    SELECTIVE = auto()
    MANUAL = auto()


class DrivePlan(Enum):
    UNTIL_IDLE = auto()
    ONE_PASS = auto()
    UNTIL_IDLE_THEN_WAIT = auto()


class DriveAfter(Enum):
    NO = auto()
    UNTIL_IDLE = auto()
    ONE_PASS = auto()


@dataclass
class FiringPlan:
    kind: str = "defaults"  # defaults | all | map
    all: Firing = Firing.FIRING_UNSPECIFIED
    default: Firing = Firing.FIRING_UNSPECIFIED
    by_node: dict[str, Firing] = field(default_factory=dict)

    @staticmethod
    def capability_defaults() -> FiringPlan:
        return FiringPlan(kind="defaults")

    @staticmethod
    def force_all(f: Firing) -> FiringPlan:
        return FiringPlan(kind="all", all=f)


@dataclass
class NodePlan:
    kind: str = "all"  # all | only | after | from
    ids: list[str] = field(default_factory=list)
    node: str = ""

    @staticmethod
    def all() -> NodePlan:
        return NodePlan(kind="all")

    @staticmethod
    def only(*ids: str) -> NodePlan:
        return NodePlan(kind="only", ids=list(ids))

    @staticmethod
    def after(node: str) -> NodePlan:
        return NodePlan(kind="after", node=node)

    @staticmethod
    def from_node(node: str) -> NodePlan:
        return NodePlan(kind="from", node=node)


@dataclass
class FrameworkPolicy:
    mode: RunMode = RunMode.CONVERGE
    firing: FiringPlan = field(default_factory=FiringPlan.capability_defaults)
    nodes: NodePlan = field(default_factory=NodePlan.all)
    max_steps: int | None = None
    drive: DrivePlan = DrivePlan.UNTIL_IDLE
    claim_modules: list[str] | None = None
    storage: StoragePlan = field(default_factory=StoragePlan.off)
    memo: MemoPlan = field(default_factory=MemoPlan.off)
    manual_closure: Closure = Closure.CLOSURE_FIRST_TERMINAL
    # Max parallel host workers (and kernel max_in_flight). None → DEFAULT_CONCURRENCY.
    concurrency: int | None = None
    # Items per ClaimReady. None → effective concurrency.
    claim_batch: int | None = None
    # Kernel lease duration ms. None → kernel default (60s).
    lease_ms: int | None = None
    # Kernel max claim attempts. None → kernel default (3).
    max_attempts: int | None = None

    @staticmethod
    def converge() -> FrameworkPolicy:
        return FrameworkPolicy()

    @staticmethod
    def memoized() -> FrameworkPolicy:
        return FrameworkPolicy.converge().with_memo(MemoPlan.on())

    @staticmethod
    def stream() -> FrameworkPolicy:
        return FrameworkPolicy(
            mode=RunMode.STREAM,
            firing=FiringPlan.force_all(Firing.FIRING_ALWAYS),
            drive=DrivePlan.UNTIL_IDLE_THEN_WAIT,
        )

    @staticmethod
    def stream_dedupe() -> FrameworkPolicy:
        return FrameworkPolicy(
            mode=RunMode.DEDUPE_STREAM,
            firing=FiringPlan.force_all(Firing.FIRING_ONCE_PER_KEY),
            drive=DrivePlan.UNTIL_IDLE_THEN_WAIT,
        )

    @staticmethod
    def selective(*node_ids: str) -> FrameworkPolicy:
        return FrameworkPolicy(
            mode=RunMode.SELECTIVE,
            nodes=NodePlan.only(*node_ids),
        )

    @staticmethod
    def start_after(node: str) -> FrameworkPolicy:
        return FrameworkPolicy(
            mode=RunMode.SELECTIVE,
            nodes=NodePlan.after(node),
        )

    @staticmethod
    def from_node(node: str) -> FrameworkPolicy:
        return FrameworkPolicy(
            mode=RunMode.SELECTIVE,
            nodes=NodePlan.from_node(node),
        )

    def with_firing(self, firing: FiringPlan) -> FrameworkPolicy:
        return replace(self, firing=firing)

    def with_nodes(self, nodes: NodePlan) -> FrameworkPolicy:
        return replace(self, nodes=nodes)

    def with_drive(self, drive: DrivePlan) -> FrameworkPolicy:
        return replace(self, drive=drive)

    def with_max_steps(self, n: int) -> FrameworkPolicy:
        return replace(self, max_steps=n)

    def with_claim_modules(self, *modules: str) -> FrameworkPolicy:
        return replace(self, claim_modules=list(modules))

    def with_storage(self, storage: StoragePlan) -> FrameworkPolicy:
        return replace(self, storage=storage)

    def with_memo(self, memo: MemoPlan) -> FrameworkPolicy:
        return replace(self, memo=memo)

    def with_concurrency(self, n: int) -> FrameworkPolicy:
        return replace(self, concurrency=max(1, n))

    def with_claim_batch(self, n: int) -> FrameworkPolicy:
        return replace(self, claim_batch=max(1, n))

    def with_lease_ms(self, ms: int) -> FrameworkPolicy:
        return replace(self, lease_ms=ms)

    def with_max_attempts(self, n: int) -> FrameworkPolicy:
        return replace(self, max_attempts=max(1, n))

    def effective_concurrency(self) -> int:
        n = self.concurrency if self.concurrency is not None else DEFAULT_CONCURRENCY
        return max(1, n)

    def effective_claim_batch(self) -> int:
        n = (
            self.claim_batch
            if self.claim_batch is not None
            else self.effective_concurrency()
        )
        return max(1, n)

    def closure(self) -> Closure:
        if self.mode in (RunMode.STREAM, RunMode.DEDUPE_STREAM):
            return Closure.CLOSURE_OPEN
        if self.mode == RunMode.MANUAL:
            return self.manual_closure
        return Closure.CLOSURE_FIRST_TERMINAL

    def needs_cut(self) -> bool:
        return self.nodes.kind != "all"

    def effective_drive(self) -> DrivePlan:
        if self.drive == DrivePlan.UNTIL_IDLE_THEN_WAIT:
            return DrivePlan.UNTIL_IDLE
        return self.drive

    def include_nodes(self) -> list[str]:
        if self.nodes.kind == "only":
            return list(self.nodes.ids)
        return []

    def execution_policy_for(self, assembly: Assembly | None) -> ExecutionPolicy:
        ep = ExecutionPolicy()
        ep.closure = self.closure()
        if self.firing.kind == "all":
            ep.default = self.firing.all
            if assembly is not None:
                for n in assembly.nodes:
                    ep.by_node[n.id] = self.firing.all
        elif self.firing.kind == "map":
            ep.default = self.firing.default
            for k, v in self.firing.by_node.items():
                ep.by_node[k] = v
        else:
            ep.default = Firing.FIRING_UNSPECIFIED
        return ep

    def resolve_max_steps(self, node_count: int) -> int:
        if self.max_steps is not None:
            return self.max_steps
        if self.closure() == Closure.CLOSURE_OPEN:
            return max(node_count * 10_000, 10_000)
        return 0

    def apply_to_run_request(self, req: RunRequest) -> RunRequest:
        node_count = len(req.assembly.nodes) if req.assembly else 0
        req.policy.CopyFrom(self.execution_policy_for(req.assembly))
        del req.include_nodes[:]
        req.include_nodes.extend(self.include_nodes())
        req.limits.max_steps = self.resolve_max_steps(node_count)
        req.limits.max_in_flight = self.effective_concurrency()
        req.limits.default_lease_ms = self.lease_ms if self.lease_ms is not None else 0
        req.limits.max_attempts = (
            self.max_attempts if self.max_attempts is not None else 0
        )
        return req
