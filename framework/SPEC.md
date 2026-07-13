# srcport-framework — Specification v0.1.0

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
   └─ rust/                  # first host + plugin SDK
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
| `processing_ui` / `result_ui` | no | **Legacy** — default `on_init` / `on_final` adapters |

Empty hooks mean “headless.” Modules emit **presentation data only** — never
widgets, HTML, or shell code.

Plugins **must not** import other plugins. Cross-module coupling remains
contract refs and assembly bindings on the kernel — same rule as the substrate.

### 3. Step lifecycle (presentation)

Per **work unit** (not kernel module `REGISTERED→…`):

```text
ClaimReady → on_init (Init) → execute { emit_progress* } → on_final (Final) → Put/Commit
```

| Stage | How | Contract |
|-------|-----|----------|
| **Init** | `on_init` | `srcport.ui.v1.StepInit` |
| **Progress** | `StepContext::emit_progress` (0..N) | `srcport.ui.v1.StepProgress` |
| **Final** | `on_final` (success or failure) | `srcport.ui.v1.StepFinal` |

Host collects [`StepEvent`]s (`stage` + `Presentation` + optional artifact id).
`UiPersist::Artifacts` also `PutArtifact`s JSON bodies for audit/replay.

On execute **failure**: host still emits Progress buffer + Final (failed), does
**not** commit a derivation, returns `StepFailed`.

### 4. Step context & output

`execute` receives a **mutable step context** (run id, work item, inputs) and
returns **named port outputs**. The host puts domain artifacts and commits the
derivation so provenance stays on the ledger.

Enrichment is ordinary: produce a new typed artifact; bind it downstream.
There is no mutable “enrichment bag” in the kernel.

### 5. UI profile (`srcport.ui.v1`)

Optional well-known contract refs. Bodies are JSON (UTF-8) in `Artifact.body`
when persisted.

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
| **`selective(nodes)`** | Only some steps | `include_nodes`, converge | drive subset assembly |
| **`manual(closure)`** | Escape hatch | caller closure | defaults; override with builders |

Builders: `with_firing`, `with_nodes`, `with_drive`, `with_max_steps`,
`with_claim_modules` (soft host claim allow-list).

**Firing note.** The kernel resolves `by_node` → capability.firing →
policy.default → `ONCE`. `FiringPlan::All` therefore pins **every assembly
node** in `by_node` so stream modes override module-declared `ONCE`.

Host entrypoints:

- `start_pipeline(id, assembly, inputs, policy)`
- `drive` / `drive_with(DrivePlan)`
- `inject(run_id, input, DriveAfter::{No, UntilIdle, OnePass})`
- `cancel(run_id)`

---

## Invariants this framework upholds

1. **Substrate is law.** Every durable effect goes through `KernelApi`. No
   shadow run state that disagrees with the kernel.
2. **Artifacts remain immutable.** Hooks produce new artifacts; they never
   mutate prior ones.
3. **UI is optional.** Omitting UI hooks never changes run completion.
4. **Plugins are not principals.** Same as substrate: no kernel authz; host
   trust model is product-owned.
5. **No reverse dependency.** `kernel/sdk/*` and `kernel/contracts/` must not
   import `framework/`.

---

## Versioning

- Framework line: **`v0.x`** until the host loop and UI profile stabilize.
- Breaking changes allowed in `0.x` with a changelog note in this file.
- Always declare the minimum substrate version (today: **substrate v1.1.0**).
- When freezing `v1.0.0` of the framework: pin UI contract refs and the plugin
  trait surface; still evolve substrate only by its own rules.

### Changelog

| Version | Notes |
|---------|--------|
| `0.1.0` | Initial charter; Rust host + `ModulePlugin`; UI profile stubs; manual assemblies only |
| `0.1.0` (+modes) | `FrameworkPolicy` presets: converge / stream / stream_dedupe / selective; `start_pipeline`, `inject`, `DrivePlan` |
| `0.1.0` (+lifecycle) | Step presentation lifecycle Init → Progress* → Final; `StepEvent`; UI schemas; legacy view adapters |

---

## Non-goals (v0.1)

- Changing `substrate.proto` or the seven primitives
- Multi-language plugin ABIs (WASM, gRPC workers) — later if needed
- Built-in auth, multi-tenant isolation, or a full widget toolkit
- Replacing `kernel/example/` (kernel-only demo stays; framework has its own tests)

---

## Conformance (framework)

A framework implementation is “good enough” for `0.1` when:

1. A plugin with no presentation hooks can complete a multi-node run via the host.
2. Init / Progress / Final emit in order; Progress supports multiple emits.
3. Failure emits Final(failed) without committing domain outputs.
4. Headless and presentation-enabled plugins coexist in one run.
5. Optional artifact persist uses step contract refs (`StepInit` / `Progress` / `Final`).
6. The substrate conformance suite still passes with zero framework imports.
