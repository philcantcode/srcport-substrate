# srcport-framework — Specification v2.0.0

Opinionated **application layer** on top of [srcport-substrate](../kernel/SPEC.md).
This is a framework. The substrate is not.

> **The boundary.** The kernel (`substrate.proto`, seven primitives, `KernelApi`)
> stays domain-neutral and small. This framework may be opinionated about hosts,
> module plugins, UI profiles, composition helpers, and default run loops. It
> **depends on** the substrate; the substrate **never** depends on it.
>
> If a guarantee must hold for every possible consumer (batch, game, agent,
> foreign language), it belongs in the substrate. If it exists so product teams
> stop reinventing pipelines and step chrome, it belongs here.

---

## What this is (and is not)

| | Substrate | Framework (this) |
|--|-----------|------------------|
| Surface | Seven primitives + `Kernel` ABI | Host, `ModulePlugin`, UI profiles, helpers |
| Module shape | Manifest + worker via kernel RPCs | Optional plugin object the **host** calls |
| UI | Unknown | Optional well-known view contracts as artifacts |
| Composition | Human-owned `Assembly` | Manual assemblies now; auto-composer later |
| Languages | Rust / Go / Python SDKs | Rust first; other languages optional later |
| Stability | `v1.x` contract, `buf breaking` | `v0.x` — may break; pin deliberately |

It **is** the default way *our* products build multi-step typed workflows with
optional per-step UI and a standard claim → execute → commit loop.

It **is not** a second kernel. It never reimplements artifact identity, ledger
hashing, run readiness, or contract immutability. Those stay in the substrate.

---

## Layout in the monorepo

```
srcport-substrate/           # monorepo root
├─ kernel/                   # substrate contract + SDKs
│  ├─ SPEC.md
│  ├─ contracts/
│  └─ sdk/
└─ framework/                # this product
   ├─ SPEC.md                # this charter
   ├─ README.md
   ├─ profiles/              # well-known contracts (not substrate.proto)
   │  └─ ui/
   └─ sdk/
      ├─ rust/               # host + plugin SDK
      ├─ go/
      └─ python/
```

Kernel [`SPEC.md`](../kernel/SPEC.md) and [`contracts/`](../kernel/contracts/)
remain the **only** human-owned substrate contract. Framework profiles live
here and are versioned with the framework, not with `srcport.substrate.v1`.

---

## Core concepts

### 1. Host

The **Host** owns the product process boundary:

1. Registers plugin manifests on a `KernelApi` backend.
2. Starts runs (`StartRun`) with a human- or tool-authored `Assembly`.
3. Drives work: `ClaimReady` → optional UI hooks → plugin `execute` →
   `PutArtifact` → `Commit`.
4. Collects optional UI views for the product shell (and may also put them as
   artifacts so the ledger records what was shown).

The kernel never loads plugins or calls hooks. Only the host does.

### 2. ModulePlugin

A language-native trait/interface (Rust first) implemented by domain code:

| Hook | Required? | Role |
|------|-----------|------|
| `manifest()` | yes | `ModuleManifest` for `Register` |
| `execute(step)` | yes | Domain work; may `step.emit_progress` |
| `on_init(step)` | no | **Init** presentation after claim |
| `on_final(step, result)` | no | **Final** presentation (ok or fail) |
| `storage_schema()` | no | Declare table columns when policy enables storage |
| `on_store(step, result)` | no | Rows to write after step (append / upsert / replace) |
| `processing_ui` / `result_ui` | no | **Legacy** — default `on_init` / `on_final` adapters |

Empty hooks mean “headless.” Modules emit **presentation data only** — never
widgets, HTML, or shell code.

Plugins **must not** import other plugins. Cross-module coupling remains
contract refs and assembly bindings on the kernel — same rule as the substrate.

### 3. Step lifecycle (presentation)

Per **work unit** (not kernel module `REGISTERED→…`):

```text
ClaimReady → on_init (Init) → execute { emit_progress* } → on_final (Final) → Put/Commit → on_store
```

Host cut (before any claim), when nodes are dropped by a node plan:

```text
start_pipeline → materialize_cut → StepStage::Skipped (per dropped node) → StartRun
```

| Stage | How | Contract |
|-------|-----|----------|
| **Init** | `on_init` | `srcport.ui.v1.StepInit` |
| **Progress** | `StepContext::emit_progress` (0..N) | `srcport.ui.v1.StepProgress` |
| **Final** | `on_final` (success or failure) | `srcport.ui.v1.StepFinal` |
| **Skipped** | host cut at `start_pipeline` | `srcport.ui.v1.StepSkipped` |
| **Cached** | memo hit after claim (no `execute`) | `srcport.ui.v1.StepCached` |
| **Store** | `on_store` (after commit; best-effort on fail) | tabular rows on `StorageBackend` |

Host collects [`StepEvent`]s (`stage` + `Presentation` + optional artifact id).
`UiPersist::Artifacts` also `PutArtifact`s JSON bodies for audit/replay.

On execute **failure**: host still emits Progress buffer + Final (failed), does
**not** commit a derivation, returns `StepFailed`. Storage / step_log may still
record the failed outcome.

### 4. Step context & output

`execute` receives a **mutable step context** (run id, work item, inputs) and
returns **named port outputs**. The host puts domain artifacts and commits the
derivation so provenance stays on the ledger.

Enrichment is ordinary: produce a **new** trait-bag artifact (add traits via
merge / multi-trait `PortBody`); bind it downstream. Projection isolates one
trait for consumers that need only the base fact. There is no mutable bag —
immutability still holds; “mutation over time” is succession of bags + optional
`entity_id` / `supersedes` lineage hints.

### 5. UI profile (`srcport.ui.v1`)

Optional well-known contract refs. Bodies are JSON (UTF-8) as a single trait
on the presentation artifact when persisted.

| Contract ref | Stage |
|--------------|--------|
| `srcport.ui.v1.StepInit` | Init |
| `srcport.ui.v1.StepProgress` | Progress |
| `srcport.ui.v1.StepFinal` | Final |
| `ProcessingView` / `ResultView` | **Legacy** aliases |

Schemas live under [`profiles/ui/`](profiles/ui/). Presentation is a **side
channel** — never required for readiness or domain ports.

### 6. Composition (now vs later)

| Mode | Status |
|------|--------|
| **Manual** `Assembly` | Supported — author nodes and bindings explicitly |
| **Auto-composer** | Planned — registry + port contracts → proposed `Assembly` |

Auto-resolution is a framework concern. The kernel continues to reject invalid
assemblies; it does not invent them.

### 7. Run modes (`FrameworkPolicy`)

Product-facing presets compile to kernel `ExecutionPolicy` / `include_nodes` /
`Limits` plus host drive rules. **Presets are the API; raw kernel fields are
the escape hatch** (`RunMode::Manual`, `Host::start_run`).

| Preset | Intent | Kernel | Host |
|--------|--------|--------|------|
| **`converge()`** | One-shot → terminal | `FIRST_TERMINAL`, capability firing | `drive` until idle / complete |
| **`stream()`** | Loop on new data | `OPEN`, force `ALWAYS` per node | drain then wait; `inject` + re-drive |
| **`stream_dedupe()`** | Stream once per key | `OPEN`, force `ONCE_PER_KEY` | same as stream |
| **`selective(nodes)`** | Only some steps | materialised cut assembly | seed required; drive subset |
| **`start_after(node)`** | Skip node + predecessors | materialised cut assembly | seed cut edges; run the rest |
| **`from_node(node)`** | Only node + successors | materialised cut assembly | seed cut edges; must reach terminal |
| **`memoized()`** | Converge + work cache | same as converge | skip `execute` on memo hit |
| **`manual(closure)`** | Escape hatch | caller closure | defaults; override with builders |

Builders: `with_firing`, `with_nodes`, `with_drive`, `with_max_steps`,
`with_concurrency` / `with_claim_batch` / `with_lease_ms` / `with_max_attempts`
(leased parallel drive; maps into kernel `Limits`),
`with_claim_modules` (soft host claim allow-list), `with_storage(StoragePlan)`,
`with_memo(MemoPlan)`.

**Cut + seed (skip / start-after).** There is no hard-coded step index. A non-
`NodePlan::All` plan is materialised by the host before `StartRun`:

1. Resolve kept vs dropped nodes (`Only` / `After` / `From`).
2. Drop excluded nodes from the assembly.
3. Rewrite crossing edges (dropped producer → kept consumer) to synthetic run
   inputs named `__seed/{from_node}/{from_port}`.
4. Fail closed if any required seed is missing from `inputs`.
5. Emit `StepStage::Skipped` presentation events for dropped nodes (side
   channel only — not domain readiness).
6. Pass the materialised assembly to the kernel with `include_nodes` empty
   (already cut).

Helpers: `seed_input_name`, `seeds_from_run` (latest derivation outputs from a
prior run), `Host::resume_after` (start_after + auto-seed from prior run).

**Memo (skip re-work).** Optional cross-run cache. Key:

```text
H(module ‖ module_version ‖ module_digest ‖ capability ‖ sorted (port → artifact_id))
```

On claim, if `MemoPlan` is enabled and the key hits and all output artifacts
still exist: emit `StepStage::Cached`, `Commit` the prior output refs (no
`execute`), and continue. On miss: normal execute, then `MemoStore.put`.

- Plugins declare `module_digest()` — empty/None ⇒ uncacheable
- Invalidation is automatic: digest or input artifact id change ⇒ miss; new
  output ids cascade to downstream nodes
- Host needs `Host::with_memo(MemoryMemo::new())` (or another `MemoStore`)
- Requires same kernel artifact store across runs (ids must still resolve)

**Firing note.** The kernel resolves `by_node` → capability.firing →
policy.default → `ONCE`. `FiringPlan::All` therefore pins **every assembly
node** in `by_node` so stream modes override module-declared `ONCE`.

Host entrypoints:

- `start_pipeline(id, assembly, inputs, policy)`
- `resume_after(new_id, prior_id, after_node, policy)`
- `drive` / `drive_with(DrivePlan)`
- `inject(run_id, input, DriveAfter::{No, UntilIdle, OnePass})`
- `cancel(run_id)`
- `Host::with_storage(backend)` — required when `StoragePlan` is enabled
- `Host::with_memo(store)` — required when `MemoPlan` is enabled

### 8. Storage phase (`StoragePlan`) — optional

Tabular **side-channel** (not the kernel ledger). Domain provenance still goes
through artifacts + commit. Storage is for product tables (query, export,
step audit).

| Mode | Intent | Physical table name |
|------|--------|---------------------|
| **`Off`** (default) | No framework tables | — |
| **`PerRun`** | Isolated tables for one run | `{run_id}__{module}__{logical}` |
| **`Shared`** | Durable tables across runs | `{module}__{logical}` |
| **`step_log`** (flag) | Framework audit of every step | `_srcport_step_log` (or per-run) |

Retention for PerRun: **`DropOnEnd`** (default) or **`Keep`**.

Lifecycle:

```text
register_plugin  → remember storage_schema() if Any
start_pipeline   → ensure_table for each schema (+ step_log)
try_step ok      → commit domain → on_store → write_rows
try_step err     → best-effort on_store / step_log (no commit)
run end / cancel → drop PerRun tables when DropOnEnd
```

Module responsibilities:

- **`storage_schema()`** — columns, primary key, default `WriteMode`
- **`on_store()`** — which rows and whether to **Append** / **Upsert** / **Replace**

Framework responsibilities:

- Qualify table names by mode
- Inject identity columns `_run_id`, `_work_id`, `_node_id`, `_module`
- Invoke lifecycle; never invent domain columns
- Host supplies `StorageBackend` (`MemoryStorage` for tests; SQL later)

`Replace` under Shared deletes prior rows for the same `_run_id` then inserts;
under PerRun it truncates the whole (already run-scoped) table.

---

## Invariants this framework upholds

1. **Substrate is law.** Every durable effect goes through `KernelApi`. No
   shadow run state that disagrees with the kernel.
2. **Artifacts remain immutable.** Hooks produce new artifacts; they never
   mutate prior ones.
3. **UI is optional.** Omitting UI hooks never changes run completion.
4. **Storage is optional.** Omitting `StoragePlan` / schemas never changes
   readiness or domain commit semantics.
5. **Plugins are not principals.** Same as substrate: no kernel authz; host
   trust model is product-owned.
6. **No reverse dependency.** `kernel/sdk/*` and `kernel/contracts/` must not
   import `framework/`.

---

## Versioning

- Framework line: **`v2.x`** on substrate **v2.0.0+**. Additive features within
  a minor; break only on framework major.
- Always declare the minimum substrate version (today: **substrate v2.0.0**).
- Substrate evolves only by its own rules (`kernel/SPEC.md`); this framework
  never forces a substrate bump for host-only features.

### Changelog

| Version | Notes |
|---------|--------|
| `2.0.0` | Initial charter; Rust host + `ModulePlugin`; UI profile stubs; manual assemblies only |
| `2.0.0` (+modes) | `FrameworkPolicy` presets: converge / stream / stream_dedupe / selective; `start_pipeline`, `inject`, `DrivePlan` |
| `2.0.0` (+lifecycle) | Step presentation lifecycle Init → Progress* → Final; `StepEvent`; UI schemas; legacy view adapters |
| `2.0.0` (+storage) | Optional `StoragePlan` (Off / PerRun / Shared + step_log); `storage_schema` / `on_store`; `MemoryStorage` |
| **`2.1.0`** | **Cut/seed** (`start_after` / `from_node` / `resume_after`, `__seed/…` inputs, `StepStage::Skipped`) and **memo** (`MemoPlan` / `MemoryMemo`, `module_digest`, `memoized()`, `StepStage::Cached`, cascade invalidation). Substrate unchanged at v2.0.0. |
| **`2.2.0`** | **Multi-language SDKs** under `framework/sdk/{rust,go,python}` (path move from `framework/rust/`). Go and Python host + `ModulePlugin` parity with Rust (policy, cut/seed, memo, storage, presentation). CI covers all three. |
| **`2.3.0`** | **Leased concurrency**: `with_concurrency` / claim batch / lease / max_attempts map to kernel `Limits`; host batch-claims and may parallel-`execute`; step failure calls `FailWork`. Requires substrate leased-claim ABI. |

---

## Non-goals (v0.1)

- Changing `substrate.proto` or the seven primitives
- Multi-language plugin ABIs (WASM, gRPC workers) — later if needed
- Built-in auth, multi-tenant isolation, or a full widget toolkit
- Production SQL drivers (trait + memory backend only; adapters later)

---

## Conformance (framework)

A framework implementation is “good enough” for `0.1` when:

1. A plugin with no presentation hooks can complete a multi-node run via the host.
2. Init / Progress / Final emit in order; Progress supports multiple emits.
3. Failure emits Final(failed) without committing domain outputs.
4. Headless and presentation-enabled plugins coexist in one run.
5. Optional artifact persist uses step contract refs (`StepInit` / `Progress` / `Final`).
6. With `StoragePlan::Off`, no tables are created even if plugins declare schemas.
7. With PerRun + schema, tables are ensured at start and written after successful commit.
8. The substrate conformance suite still passes with zero framework imports.
