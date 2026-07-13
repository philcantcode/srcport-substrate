# Framework

Short guide to **srcport-framework**: the opinionated application layer on
the substrate kernel.

## What it is

The framework owns the **product host loop**. The kernel stays domain-neutral;
this layer adds plugins, step presentation, run presets, optional storage, and
memoisation.

```text
product / UI shell
        │
        ▼
  Host  (framework)   claim → hooks → execute → Put/Commit
        │
        ▼ KernelApi only
  kernel              artifacts · runs · ledger
```

| Concept | Role |
|---------|------|
| **Host** | Registers plugins; `start_pipeline` / `drive` / `inject` / `cancel` |
| **ModulePlugin** | Domain work (`execute`); optional UI / storage / memo hooks |
| **FrameworkPolicy** | Product presets → kernel `ExecutionPolicy` + host drive rules |
| **Step lifecycle** | Init → Progress\* → Final (side channel; optional) |
| **Cut / seed** | Skip or subset nodes; crossing edges become `__seed/…` inputs |
| **StoragePlan** | Optional tabular side-channel (not the ledger) |
| **MemoPlan** | Optional cross-run work cache |

Plugins never import each other. Coupling is still contract refs + assembly
bindings on the kernel.

## FrameworkPolicy (available configs)

**Presets are the API.** Raw kernel fields are the escape hatch
(`FrameworkPolicy::manual`, `Host::start_run`).

### Run presets

| Preset | Intent | Kernel closure | Host |
|--------|--------|----------------|------|
| `converge()` | One-shot → terminal | `FIRST_TERMINAL` | `drive` until idle |
| `stream()` | Stay open for new data | `OPEN`, force `ALWAYS` | drain, then wait |
| `stream_dedupe()` | Stream once per key | `OPEN`, force `ONCE_PER_KEY` | same as stream |
| `selective(nodes)` | Only listed nodes | `FIRST_TERMINAL` + cut | seed cut edges |
| `start_after(node)` | Skip node + predecessors | cut (`NodePlan::After`) | seed cut edges |
| `from_node(node)` | Only node + successors | cut (`NodePlan::From`) | seed cut edges |
| `memoized()` | Converge + work cache | same as converge | skip `execute` on hit |
| `manual(closure)` | Escape hatch | caller `Closure` | defaults |

Entry points: `start_pipeline`, `resume_after` (start_after + auto-seed from a
prior run), `drive` / `drive_with`, `inject(…, DriveAfter)`, `cancel`.

### Builders (compose on any preset)

| Builder | Sets | Notes |
|---------|------|--------|
| `with_firing(FiringPlan)` | Work-unit firing | `CapabilityDefaults` · `All(Firing)` · `Map { default, by_node }` |
| `with_nodes(NodePlan)` | Which assembly nodes run | `All` · `Only` · `After` · `From` (non-`All` → host cut) |
| `with_drive(DrivePlan)` | Claim loop style | `UntilIdle` · `OnePass` · `UntilIdleThenWait` |
| `with_max_steps(n)` | Cap committed work units | Kernel `Limits.max_steps` |
| `with_concurrency(n)` | Parallel host workers | Also sets kernel `Limits.max_in_flight` (default 8) |
| `with_claim_batch(n)` | Items per `ClaimReady` | Default = concurrency |
| `with_lease_ms(ms)` | Work-unit lease duration | Kernel `Limits.default_lease_ms` |
| `with_max_attempts(n)` | Claim retries after fail/expiry | Kernel `Limits.max_attempts` |
| `with_claim_modules(…)` | Soft host claim allow-list | Module names; does **not** remove assembly nodes |
| `with_storage(StoragePlan)` | Tabular side-channel | Requires `Host::with_storage(…)` |
| `with_memo(MemoPlan)` | Cross-run memo | Requires `Host::with_memo(…)` |

### StoragePlan

Host-only. Domain provenance still goes through artifacts + commit.

| Config | Intent |
|--------|--------|
| `off()` | Default — no framework tables |
| `per_run()` | Isolated tables; drop when run ends |
| `per_run_keep()` | Per-run tables kept after complete |
| `shared()` | Durable `{module}__{logical}` across runs |
| `step_log_only()` | Framework step audit only (`_srcport_step_log`) |
| `per_run_with_step_log()` | Per-run module tables + step log |
| `with_step_log()` / `with_retention(…)` | Builders on any plan |

Plugins declare `storage_schema()` / `on_store()` when enabled.

### MemoPlan

Host-only. Key:
`H(module ‖ version ‖ digest ‖ capability ‖ sorted port→artifact_id)`.

| Config | Intent |
|--------|--------|
| `off()` | Default — no memo |
| `on()` | Cache; require non-empty `module_digest()` |
| `on_optional_digest()` | Enable without requiring digest (still uncacheable if empty) |
| `with_nodes(MemoNodes)` | `All` · `Only` · `Except` |
| `with_require_digest(bool)` | Toggle digest requirement |

On hit: emit `StepStage::Cached`, commit prior output refs, skip `execute`.

### Host-local options

| Option | Role |
|--------|------|
| `Host::with_ui_persist(UiPersist)` | Events only vs also `PutArtifact` presentation bodies |
| `Host::with_storage(backend)` | Required when `StoragePlan` is enabled |
| `Host::with_memo(store)` | Required when `MemoPlan` is enabled |
| `Host::with_context(…)` | Request context for kernel calls |

## Step lifecycle (presentation)

Per work unit (not module lifecycle):

```text
ClaimReady(batch) → [parallel execute when concurrency > 1] →
  on_init → execute { emit_progress* } → on_final → Put/Commit → on_store
  on failure → FailWork (retryable; kernel may terminal after max_attempts)
```

Work units are **leased** on the kernel: dead workers release via lease expiry;
the host heartbeats long steps when needed. Prefer `with_concurrency(n)` over
hand-rolling worker pools.

| Stage | Contract ref |
|-------|--------------|
| Init | `srcport.ui.v1.StepInit` |
| Progress | `srcport.ui.v1.StepProgress` |
| Final | `srcport.ui.v1.StepFinal` |
| Skipped | host cut at `start_pipeline` |
| Cached | memo hit after claim |

UI is optional. Omitting hooks never changes run completion.

## Further reading

- Charter: [`framework/SPEC.md`](../framework/SPEC.md)
- Usage / quick start: [`framework/README.md`](../framework/README.md)
- UI schemas: [`framework/profiles/ui/`](../framework/profiles/ui/)
- Rust SDK: [`framework/sdk/rust/`](../framework/sdk/rust/)
- Concurrency / leases: [`Concurrency.md`](Concurrency.md)
- Siblings: [`Module.md`](Module.md), [`Artifact.md`](Artifact.md)
- Kernel: [`kernel/SPEC.md`](../kernel/SPEC.md)
