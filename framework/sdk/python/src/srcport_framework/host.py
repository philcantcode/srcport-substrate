"""Opinionated Host driver around any KernelApi backend."""

from __future__ import annotations

import json
from enum import Enum, auto
from typing import Any

from srcport_substrate import (
    Artifact,
    ClaimRequest,
    Derivation,
    InjectInputRequest,
    KernelApi,
    ModuleManifest,
    NamedArtifact,
    RequestContext,
    Run,
    RunRef,
    RunRequest,
    RunState,
    WorkItem,
    artifact_with_trait,
)

from .cut import (
    AssemblyCut,
    is_seed_input_name,
    materialize_cut,
    merge_inputs,
    seeds_from_run,
    validate_seeds_present,
)
from .errors import FrameworkError, invalid, kernel_err, no_plugin, step_failed
from .memo import (
    MemoStore,
    build_record,
    input_fingerprint_map,
    memo_key,
    record_to_named_outputs,
)
from .plugin import ModulePlugin, StepContext
from .policy import DriveAfter, DrivePlan, FrameworkPolicy, NodePlan, RunMode
from .presentation import (
    Presentation,
    PresentationStatus,
    StepEvent,
    StepResult,
    StepStage,
)
from .storage import (
    StorageBackend,
    StorageMode,
    StorageRetention,
    TableSchema,
    WriteMode,
    inject_identity,
    qualify_table,
    step_log_qualified,
)


class UiPersist(Enum):
    LOCAL_ONLY = auto()
    ARTIFACTS = auto()


class Host:
    def __init__(self, kernel: KernelApi) -> None:
        self._kernel = kernel
        self._plugins: dict[str, ModulePlugin] = {}
        self._ctx = RequestContext(caller="srcport-framework")
        self._ui_persist = UiPersist.LOCAL_ONLY
        self._step_events: list[StepEvent] = []
        self._run_policies: dict[str, FrameworkPolicy] = {}
        self._storage: StorageBackend | None = None
        self._storage_schemas: dict[str, TableSchema] = {}
        self._run_tables: dict[str, list[str]] = {}
        self._memo: MemoStore | None = None
        self._execute_count = 0
        self._memo_hit_count = 0

    def with_context(self, ctx: RequestContext) -> Host:
        self._ctx = ctx
        return self

    def with_ui_persist(self, mode: UiPersist) -> Host:
        self._ui_persist = mode
        return self

    def with_storage(self, backend: StorageBackend) -> Host:
        self._storage = backend
        return self

    def with_memo(self, store: MemoStore) -> Host:
        self._memo = store
        return self

    @property
    def kernel(self) -> KernelApi:
        return self._kernel

    @property
    def execute_count(self) -> int:
        return self._execute_count

    @property
    def memo_hit_count(self) -> int:
        return self._memo_hit_count

    def take_step_events(self) -> list[StepEvent]:
        out = self._step_events
        self._step_events = []
        return out

    def register_plugin(self, plugin: ModulePlugin) -> None:
        m = plugin.manifest()
        if not m.name:
            raise invalid("plugin manifest.name must be non-empty")
        if m.name in self._plugins:
            raise invalid(f"plugin already registered: {m.name}")
        schema = plugin.storage_schema()
        if schema is not None:
            if not schema.name:
                raise invalid(f"plugin {m.name} storage_schema.name must be non-empty")
            self._storage_schemas[m.name] = schema
        self._kernel.register(m, self._ctx)
        self._plugins[m.name] = plugin

    def start_pipeline(
        self,
        run_id: str,
        assembly: Any,
        inputs: list[NamedArtifact],
        policy: FrameworkPolicy,
    ) -> Run:
        if policy.mode == RunMode.SELECTIVE and policy.nodes.kind == "all":
            raise invalid("RunMode selective requires NodePlan only, after, or from")
        if not run_id:
            raise invalid("run_id must be non-empty")
        if run_id in self._run_policies:
            raise invalid(f"pipeline policy already registered for run_id {run_id}")
        if policy.storage.enabled() and self._storage is None:
            raise invalid("StoragePlan enabled but host has no StorageBackend")
        if policy.memo.enabled and self._memo is None:
            raise invalid("MemoPlan enabled but host has no MemoStore")

        cut = materialize_cut(assembly, policy.nodes)
        validate_seeds_present(cut, inputs)
        self._emit_skip_events(run_id, cut)

        kernel_policy = FrameworkPolicy(
            mode=policy.mode,
            firing=policy.firing,
            nodes=NodePlan.all(),
            max_steps=policy.max_steps,
            drive=policy.drive,
            claim_modules=policy.claim_modules,
            storage=policy.storage,
            memo=policy.memo,
            manual_closure=policy.manual_closure,
        )
        req = kernel_policy.apply_to_run_request(
            RunRequest(id=run_id, assembly=cut.assembly, inputs=inputs)
        )
        try:
            run = self._kernel.start_run(req, self._ctx)
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e
        self._ensure_run_storage(run_id, policy)
        self._run_policies[run_id] = policy
        return run

    def resume_after(
        self,
        new_run_id: str,
        prior_run_id: str,
        after_node: str,
        policy: FrameworkPolicy,
    ) -> Run:
        prior = self.get_run(prior_run_id)
        if prior.assembly is None:
            raise invalid("prior run has no assembly")
        policy = policy.with_nodes(NodePlan.after(after_node))
        cut = materialize_cut(prior.assembly, policy.nodes)
        cut_nodes = [s.node_id for s in cut.skipped]
        if after_node not in cut_nodes:
            cut_nodes.append(after_node)
        seeds = seeds_from_run(self._kernel, prior_run_id, cut_nodes, self._ctx)
        base = [i for i in prior.inputs if not is_seed_input_name(i.name)]
        inputs = merge_inputs(base, seeds)
        return self.start_pipeline(new_run_id, prior.assembly, inputs, policy)

    def get_run(self, run_id: str) -> Run:
        try:
            return self._kernel.get_run(RunRef(id=run_id), self._ctx)
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e

    def inject(
        self, run_id: str, input: NamedArtifact, after: DriveAfter = DriveAfter.NO
    ) -> Run:
        try:
            run = self._kernel.inject_input(
                InjectInputRequest(run_id=run_id, input=input), self._ctx
            )
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e
        if after == DriveAfter.UNTIL_IDLE:
            return self.drive_with(run_id, DrivePlan.UNTIL_IDLE)
        if after == DriveAfter.ONE_PASS:
            return self.drive_with(run_id, DrivePlan.ONE_PASS)
        return run

    def cancel(self, run_id: str) -> Run:
        try:
            run = self._kernel.cancel_run(RunRef(id=run_id), self._ctx)
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e
        self._finish_run_storage(run_id)
        return run

    def drive(self, run_id: str) -> Run:
        plan = DrivePlan.UNTIL_IDLE
        if run_id in self._run_policies:
            plan = self._run_policies[run_id].effective_drive()
        return self.drive_with(run_id, plan)

    def drive_with(self, run_id: str, plan: DrivePlan) -> Run:
        if plan == DrivePlan.UNTIL_IDLE_THEN_WAIT:
            plan = DrivePlan.UNTIL_IDLE
        if plan == DrivePlan.ONE_PASS:
            run = self._drive_one_pass(run_id)
        else:
            run = self._drive_until_idle(run_id)
        if run.state != RunState.RUN_STATE_RUNNING:
            self._finish_run_storage(run_id)
        return run

    def _claim_module_names(self, run_id: str) -> list[str]:
        all_names = sorted(self._plugins)
        policy = self._run_policies.get(run_id)
        if policy is None or policy.claim_modules is None:
            return all_names
        allow = set(policy.claim_modules)
        return [m for m in all_names if m in allow]

    def _drive_until_idle(self, run_id: str) -> Run:
        while True:
            run = self.get_run(run_id)
            if run.state != RunState.RUN_STATE_RUNNING:
                return run
            progressed = False
            for module in self._claim_module_names(run_id):
                run = self.get_run(run_id)
                if run.state != RunState.RUN_STATE_RUNNING:
                    return run
                if self.try_step(run_id, module):
                    progressed = True
            if not progressed:
                return self.get_run(run_id)

    def _drive_one_pass(self, run_id: str) -> Run:
        run = self.get_run(run_id)
        if run.state != RunState.RUN_STATE_RUNNING:
            return run
        for module in self._claim_module_names(run_id):
            run = self.get_run(run_id)
            if run.state != RunState.RUN_STATE_RUNNING:
                return run
            self.try_step(run_id, module)
        return self.get_run(run_id)

    def try_step(self, run_id: str, module: str) -> bool:
        try:
            work = self._kernel.claim_ready(
                ClaimRequest(run_id=run_id, module=module), self._ctx
            )
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e
        if not work.id:
            return False

        step = self._load_step(run_id, work)
        hit = self._try_memo_hit(run_id, module, work)
        if hit is not None:
            key, named, source_run = hit
            return self._commit_memo_hit(run_id, module, work, step, key, named, source_run)

        plugin = self._plugins.get(module)
        if plugin is None:
            raise no_plugin(module)

        init = plugin.on_init(step)
        if init is not None:
            init.stage = StepStage.INIT
            init.fill_identity(run_id, work)
            self._emit_presentation(module, init)

        try:
            output = plugin.execute(step)
            exec_err: Exception | None = None
        except Exception as e:  # noqa: BLE001
            output = None
            exec_err = e
        self._execute_count += 1

        for p in step.take_progress():
            self._emit_presentation(module, p)

        if exec_err is not None:
            msg = str(exec_err)
            sr = StepResult(ok=False, error=msg)
            final = plugin.on_final(step, sr) or Presentation.final_failed("Step failed", msg)
            final.stage = StepStage.FINAL
            final.status = PresentationStatus.FAILED
            final.fill_identity(run_id, work)
            try:
                self._emit_presentation(module, final)
            except FrameworkError:
                pass
            try:
                self._apply_step_storage(run_id, module, step, sr)
            except FrameworkError:
                pass
            raise step_failed(msg)

        assert output is not None
        named: list[NamedArtifact] = []
        for out in output.outputs:
            art = Artifact()
            art.produced_by = module
            if out.entity_id:
                art.entity_id = out.entity_id
            for c, b in out.traits.items():
                art.traits[c].body = bytes(b)
            try:
                ref = self._kernel.put_artifact(art, self._ctx)
            except Exception as e:  # noqa: BLE001
                raise kernel_err(e) from e
            named.append(NamedArtifact(name=out.port, artifact=ref))

        sr = StepResult(ok=True, outputs=named)
        final = plugin.on_final(step, sr)
        if final is not None:
            final.stage = StepStage.FINAL
            final.fill_identity(run_id, work)
            if not final.output_ports:
                final.output_ports = [o.name for o in named]
            self._emit_presentation(module, final)

        try:
            self._kernel.commit(
                Derivation(
                    run_id=run_id,
                    work_id=work.id,
                    node_id=work.node_id,
                    outputs=named,
                ),
                self._ctx,
            )
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e

        self._store_memo_after_success(run_id, module, work, named)
        self._apply_step_storage(run_id, module, step, sr)
        return True

    def _try_memo_hit(
        self, run_id: str, module: str, work: WorkItem
    ) -> tuple[str, list[NamedArtifact], str] | None:
        policy = self._run_policies.get(run_id)
        if policy is None or not policy.memo.enabled or self._memo is None:
            return None
        if not policy.memo.nodes.allows(work.node_id):
            return None
        plugin = self._plugins.get(module)
        if plugin is None:
            return None
        digest = plugin.module_digest() or ""
        if not digest:
            return None
        inputs = input_fingerprint_map(work)
        key = memo_key(module, work.module_version, digest, work.capability, inputs)
        rec = self._memo.get(key)
        if rec is None:
            return None
        named = record_to_named_outputs(rec)
        for na in named:
            if na.artifact is None:
                return None
            try:
                self._kernel.get_artifact(na.artifact, self._ctx)
            except Exception:
                return None
        if not named and rec.outputs:
            return None
        return key, named, rec.source_run_id

    def _commit_memo_hit(
        self,
        run_id: str,
        module: str,
        work: WorkItem,
        step: StepContext,
        key: str,
        named: list[NamedArtifact],
        source_run: str,
    ) -> bool:
        p = Presentation.cached(
            f"Cached {work.node_id}", f"memo hit; outputs from run {source_run}"
        )
        p.fill_identity(run_id, work)
        p.output_ports = [o.name for o in named]
        p.meta = {"memo": "hit", "memo_key": key, "memo_source_run": source_run}
        self._emit_presentation(module, p)
        try:
            self._kernel.commit(
                Derivation(
                    run_id=run_id,
                    work_id=work.id,
                    node_id=work.node_id,
                    outputs=named,
                ),
                self._ctx,
            )
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e
        self._memo_hit_count += 1
        self._apply_step_storage(run_id, module, step, StepResult(ok=True, outputs=named))
        return True

    def _store_memo_after_success(
        self,
        run_id: str,
        module: str,
        work: WorkItem,
        named: list[NamedArtifact],
    ) -> None:
        policy = self._run_policies.get(run_id)
        if policy is None or not policy.memo.enabled or self._memo is None:
            return
        if not policy.memo.nodes.allows(work.node_id):
            return
        plugin = self._plugins.get(module)
        if plugin is None:
            return
        digest = plugin.module_digest() or ""
        if not digest:
            return
        inputs = input_fingerprint_map(work)
        key = memo_key(module, work.module_version, digest, work.capability, inputs)
        self._memo.put(build_record(key, work, digest, named, run_id))

    def _load_step(self, run_id: str, work: WorkItem) -> StepContext:
        inputs: dict[str, Artifact] = {}
        for na in work.inputs:
            if na.artifact is None:
                continue
            try:
                inputs[na.name] = self._kernel.get_artifact(na.artifact, self._ctx)
            except Exception as e:  # noqa: BLE001
                raise kernel_err(e) from e
        return StepContext(run_id=run_id, work=work, inputs=inputs)

    def _emit_skip_events(self, run_id: str, cut: AssemblyCut) -> None:
        if not cut.skipped:
            return
        seed_by_node: dict[str, list] = {}
        for s in cut.required_seeds:
            seed_by_node.setdefault(s.from_node, []).append(s)
        for skipped in cut.skipped:
            seeds = seed_by_node.get(skipped.node_id, [])
            if seeds:
                ports = ", ".join(s.from_port for s in seeds)
                detail = f"skipped (seeded ports: {ports}); cut from run"
            else:
                detail = "skipped (no outputs required by kept nodes)"
            p = Presentation.skipped(f"Skip {skipped.node_id}", detail)
            p.run_id = run_id
            p.node_id = skipped.node_id
            p.module = skipped.module
            p.capability = skipped.capability
            p.meta = {"cut": "true"}
            for s in seeds:
                p.meta[f"seed:{s.from_port}"] = s.input_name
            self._emit_presentation(skipped.module, p)

    def _emit_presentation(self, module: str, presentation: Presentation) -> None:
        stage = presentation.stage
        artifact_id = self._maybe_put_ui(module, stage.contract_ref(), presentation)
        self._step_events.append(
            StepEvent(stage=stage, presentation=presentation, artifact_id=artifact_id)
        )

    def _maybe_put_ui(self, module: str, contract: str, presentation: Presentation) -> str:
        if self._ui_persist != UiPersist.ARTIFACTS:
            return ""
        body = json.dumps(presentation.to_dict()).encode()
        art = artifact_with_trait(contract, body)
        art.produced_by = module
        try:
            ref = self._kernel.put_artifact(art, self._ctx)
        except Exception as e:  # noqa: BLE001
            raise kernel_err(e) from e
        return ref.id

    def _ensure_run_storage(self, run_id: str, policy: FrameworkPolicy) -> None:
        if not policy.storage.enabled():
            return
        if self._storage is None:
            raise invalid("storage enabled without backend")
        physical: list[str] = []
        if policy.storage.module_tables():
            for module, schema in self._storage_schemas.items():
                q = qualify_table(policy.storage.mode, run_id, module, schema)
                self._storage.ensure_table(q)
                physical.append(q.physical_name)
        if policy.storage.step_log:
            q = step_log_qualified(policy.storage.mode, run_id)
            self._storage.ensure_table(q)
            physical.append(q.physical_name)
        if physical:
            self._run_tables[run_id] = physical

    def _apply_step_storage(
        self, run_id: str, module: str, step: StepContext, result: StepResult
    ) -> None:
        policy = self._run_policies.get(run_id)
        if policy is None or not policy.storage.enabled() or self._storage is None:
            return
        if policy.storage.module_tables() and module in self._storage_schemas:
            schema = self._storage_schemas[module]
            plugin = self._plugins[module]
            write = plugin.on_store(step, result)
            if write is not None and write.rows:
                mode = write.mode or schema.write_mode
                q = qualify_table(policy.storage.mode, run_id, module, schema)
                for row in write.rows:
                    inject_identity(row, run_id, step.work.id, step.work.node_id, module)
                self._storage.write_rows(
                    q.physical_name, mode, write.rows, schema.primary_key, run_id
                )
        if policy.storage.step_log:
            q = step_log_qualified(policy.storage.mode, run_id)
            row = {
                "run_id": run_id,
                "work_id": step.work.id,
                "node_id": step.work.node_id,
                "module": module,
                "capability": step.work.capability,
                "ok": result.ok,
                "output_ports": [o.name for o in result.outputs],
            }
            if result.error:
                row["error"] = result.error
            self._storage.ensure_table(q)
            self._storage.write_rows(q.physical_name, WriteMode.APPEND, [row], [], run_id)

    def _finish_run_storage(self, run_id: str) -> None:
        policy = self._run_policies.get(run_id)
        retention = StorageRetention.KEEP
        mode = StorageMode.OFF
        if policy is not None:
            retention = policy.storage.retention
            mode = policy.storage.mode
        if (
            retention == StorageRetention.DROP_ON_END
            and mode == StorageMode.PER_RUN
            and self._storage is not None
        ):
            for t in self._run_tables.pop(run_id, []):
                self._storage.drop_table(t)
        else:
            self._run_tables.pop(run_id, None)
        self._run_policies.pop(run_id, None)
