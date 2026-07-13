"""Assembly cut + seed helpers for start-after / from without a hard-coded DAG."""

from __future__ import annotations

from collections import defaultdict, deque
from dataclasses import dataclass, field

from srcport_substrate import Assembly, Binding, KernelApi, NamedArtifact, RequestContext, RunRef

from .errors import invalid, kernel_err
from .policy import NodePlan

SEED_INPUT_PREFIX = "__seed/"


def seed_input_name(from_node: str, from_port: str) -> str:
    return f"{SEED_INPUT_PREFIX}{from_node}/{from_port}"


def is_seed_input_name(name: str) -> bool:
    return name.startswith(SEED_INPUT_PREFIX)


@dataclass
class SeedSpec:
    input_name: str
    from_node: str
    from_port: str
    to_node: str
    to_port: str


@dataclass
class SkippedNode:
    node_id: str
    module: str
    capability: str
    module_version: str


@dataclass
class AssemblyCut:
    assembly: Assembly
    skipped: list[SkippedNode] = field(default_factory=list)
    required_seeds: list[SeedSpec] = field(default_factory=list)
    kept_node_ids: list[str] = field(default_factory=list)


def resolve_kept_nodes(assembly: Assembly, plan: NodePlan) -> set[str]:
    if not assembly.nodes:
        raise invalid("assembly has no nodes")
    known = {n.id for n in assembly.nodes}
    if assembly.terminal is None:
        raise invalid("assembly terminal is required")
    if assembly.terminal.node not in known:
        raise invalid(f"terminal node {assembly.terminal.node} is not in the assembly")

    if plan.kind == "all":
        return set(known)

    if plan.kind == "only":
        if not plan.ids:
            raise invalid("NodePlan only requires at least one node id")
        if len(plan.ids) != len(set(plan.ids)):
            raise invalid("NodePlan only contains duplicate node id")
        kept: set[str] = set()
        for id_ in plan.ids:
            if id_ not in known:
                raise invalid(f"NodePlan only references unknown node {id_}")
            kept.add(id_)
        if assembly.terminal.node not in kept:
            raise invalid("node plan must retain the terminal node")
        return kept

    if plan.kind == "after":
        if plan.node not in known:
            raise invalid(f"NodePlan after references unknown node {plan.node}")
        preds = _transitive_predecessors(assembly, plan.node)
        dropped = preds | {plan.node}
        if assembly.terminal.node in dropped:
            raise invalid(f"NodePlan after({plan.node}) would drop the terminal node")
        kept = {n.id for n in assembly.nodes if n.id not in dropped}
        if not kept:
            raise invalid("NodePlan after left no nodes to run")
        return kept

    if plan.kind == "from":
        if plan.node not in known:
            raise invalid(f"NodePlan from references unknown node {plan.node}")
        reach = _reachable_from(assembly, plan.node)
        if assembly.terminal.node not in reach:
            raise invalid(
                f"NodePlan from({plan.node}): terminal {assembly.terminal.node} "
                "is not reachable from that node"
            )
        return reach

    raise invalid(f"unknown NodePlan kind {plan.kind!r}")


def materialize_cut(assembly: Assembly, plan: NodePlan) -> AssemblyCut:
    kept = resolve_kept_nodes(assembly, plan)
    if plan.kind == "all" or len(kept) == len(assembly.nodes):
        return AssemblyCut(
            assembly=assembly,
            kept_node_ids=[n.id for n in assembly.nodes],
        )

    skipped = [
        SkippedNode(
            node_id=n.id,
            module=n.module,
            capability=n.capability,
            module_version=n.module_version,
        )
        for n in assembly.nodes
        if n.id not in kept
    ]

    seed_by_key: dict[tuple[str, str], SeedSpec] = {}
    bindings: list[Binding] = []
    for b in assembly.bindings:
        if b.to_node not in kept:
            continue
        if b.input:
            bindings.append(b)
            continue
        if not b.from_node:
            raise invalid(f"binding to {b.to_node}.{b.to_port} has neither input nor from_node")
        if b.from_node in kept:
            bindings.append(b)
            continue
        input_name = seed_input_name(b.from_node, b.from_port)
        key = (b.from_node, b.from_port)
        seed_by_key.setdefault(
            key,
            SeedSpec(
                input_name=input_name,
                from_node=b.from_node,
                from_port=b.from_port,
                to_node=b.to_node,
                to_port=b.to_port,
            ),
        )
        bindings.append(
            Binding(to_node=b.to_node, to_port=b.to_port, input=input_name)
        )

    nodes = [n for n in assembly.nodes if n.id in kept]
    required = [seed_by_key[k] for k in sorted(seed_by_key)]
    cut_asm = Assembly(
        id=assembly.id,
        nodes=nodes,
        bindings=bindings,
        terminal=assembly.terminal,
    )
    return AssemblyCut(
        assembly=cut_asm,
        skipped=skipped,
        required_seeds=required,
        kept_node_ids=[n.id for n in nodes],
    )


def seeds_from_run(
    kernel: KernelApi,
    run_id: str,
    cut_nodes: list[str],
    ctx: RequestContext | None = None,
) -> list[NamedArtifact]:
    try:
        lst = kernel.list_derivations(RunRef(id=run_id), ctx)
    except Exception as e:  # noqa: BLE001 — map kernel failures
        raise kernel_err(e) from e
    want = set(cut_nodes)
    if not want:
        return []
    latest: dict[str, object] = {}
    for d in lst.derivations:
        if d.node_id in want:
            latest[d.node_id] = d
    out: list[NamedArtifact] = []
    seen: set[str] = set()
    for node_id in sorted(want):
        d = latest.get(node_id)
        if d is None:
            raise invalid(f"seeds_from_run: no derivation for node {node_id} on run {run_id}")
        for o in d.outputs:  # type: ignore[attr-defined]
            if not o.name:
                continue
            name = seed_input_name(node_id, o.name)
            if name in seen:
                continue
            seen.add(name)
            if o.artifact is None:
                raise invalid(f"seeds_from_run: output {node_id}.{o.name} has no artifact ref")
            out.append(NamedArtifact(name=name, artifact=o.artifact))
    return out


def merge_inputs(
    base: list[NamedArtifact], seeds: list[NamedArtifact]
) -> list[NamedArtifact]:
    by: dict[str, NamedArtifact] = {}
    for na in base:
        by[na.name] = na
    for na in seeds:
        by[na.name] = na
    return [by[k] for k in sorted(by)]


def validate_seeds_present(cut: AssemblyCut, inputs: list[NamedArtifact]) -> None:
    if not cut.required_seeds:
        return
    have = {i.name: i for i in inputs}
    missing: list[str] = []
    for s in cut.required_seeds:
        na = have.get(s.input_name)
        if na is None:
            missing.append(
                f"{s.input_name} (from {s.from_node}.{s.from_port} → {s.to_node}.{s.to_port})"
            )
        elif na.artifact is None:
            missing.append(f"{s.input_name} (present but artifact ref is empty)")
    if missing:
        raise invalid(
            "cut requires seed inputs that were not provided: " + "; ".join(missing)
        )


def _transitive_predecessors(assembly: Assembly, node_id: str) -> set[str]:
    incoming: dict[str, list[str]] = defaultdict(list)
    for b in assembly.bindings:
        if b.from_node and b.to_node:
            incoming[b.to_node].append(b.from_node)
    out: set[str] = set()
    q: deque[str] = deque([node_id])
    visited = {node_id}
    while q:
        cur = q.popleft()
        for p in incoming.get(cur, []):
            out.add(p)
            if p not in visited:
                visited.add(p)
                q.append(p)
    return out


def _reachable_from(assembly: Assembly, node_id: str) -> set[str]:
    outgoing: dict[str, list[str]] = defaultdict(list)
    for b in assembly.bindings:
        if b.from_node and b.to_node:
            outgoing[b.from_node].append(b.to_node)
    out = {node_id}
    q: deque[str] = deque([node_id])
    while q:
        cur = q.popleft()
        for n in outgoing.get(cur, []):
            if n not in out:
                out.add(n)
                q.append(n)
    return out
