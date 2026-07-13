# AGENTS.md — srcport-substrate

Guidance for coding agents (and humans) working in this monorepo.

## What this repo is

**Monorepo: kernel + optional framework.**

| Product | Path | Role | Stability |
|---------|------|------|-----------|
| **Kernel** | `kernel/` | Domain-neutral microkernel: contract, SDKs, conformance | Substrate v2.x; proto package `srcport.substrate.v1` |
| **Framework** | `framework/` | Opinionated host, module plugins, UI profiles, storage/memo | Framework v2.x on substrate ≥ v2.0.0 |

```text
srcport-substrate/
├─ kernel/                 # substrate SPEC · proto · SDKs
│  ├─ SPEC.md              # ← canonical kernel contract (read this)
│  ├─ contracts/proto/     # substrate.proto (buf-checked)
│  ├─ scripts/gen.sh       # regenerate Go/Python from proto
│  └─ sdk/{rust,go,python}
├─ framework/              # host · plugins · UI profiles
│  ├─ SPEC.md              # ← framework charter
│  ├─ profiles/ui/         # srcport.ui.v1 JSON schemas
│  └─ sdk/{rust,go,python} # host + ModulePlugin (Rust is primary)
├─ docs/                   # short concept guides (Module, Artifact, Framework)
├─ README.md
└─ AGENTS.md               # you are here
```

### The one rule

> **One canonical kernel contract, many conforming implementations.**  
> Widen the kernel by *adding* to the contract. Put product opinions in
> `framework/`. The kernel **never** depends on the framework.

```text
product / UI shell
        │
        ▼
  framework/Host     claim → hooks → execute → Put/Commit
        │ KernelApi only
        ▼
  kernel             artifacts · runs · ledger · registry
```

- Framework **may** depend on kernel SDKs.
- Kernel code **must not** import anything under `framework/`.
- Domain modules couple only via **contract refs** and assembly bindings — never by importing each other.

---

## Kernel concepts (use these correctly)

The kernel is **seven primitives + one `KernelApi`**. Full law:
[`kernel/SPEC.md`](kernel/SPEC.md). Wire types:
[`kernel/contracts/proto/srcport/substrate/v1/substrate.proto`](kernel/contracts/proto/srcport/substrate/v1/substrate.proto).

| # | Primitive | What agents must remember |
|---|-----------|---------------------------|
| 1 | **Module** | Vertical slice. Typed I/O only on **ports**. Lifecycle: `REGISTERED → LOADED → ACTIVE → DEACTIVATED` (one step at a time). Never imports another module. |
| 2 | **Artifact** | Immutable **trait bag** (contract ref → `Trait`). Identity = `H(canonical traits)`. Inline `body` or external `ObjectRef` after `PutBlob`. `meta` / `produced_by` / `entity_id` / `supersedes` are **not** part of the id. |
| 3 | **Contract** | Sole coupling point. `ref` pinned to content `digest`. Same ref + different content → `CONFLICT`. Ports declare trait **sets** (inclusion matching). |
| 4 | **Event** | Notification only. Artifact **refs** are the data plane; the bus never carries domain value bytes. |
| 5 | **Ledger** | Append-only, hash-chained. Every meaningful action writes an entry. Reconstructable history. |
| 6 | **Registry** | Discovery snapshot: modules, capabilities, contracts, store policy. |
| 7 | **Run** | Assembly (acyclic, one terminal) + inputs + `ExecutionPolicy`. Claim work units → Commit `Derivation`. Closure: `FIRST_TERMINAL` or `OPEN`. |

### Mental model: the whole loop

1. **Register** modules (manifests with capabilities/ports) and **PutContract** schemas.
2. Author an **Assembly** (human-owned): pin module versions, bind ports, name one terminal.
3. **StartRun** with named input artifacts.
4. Workers **ClaimReady** → produce output artifacts → **Commit** a derivation.
5. Kernel releases downstream nodes; run completes when the terminal appears (or stalls / fails / cancels).
6. Everything lands on the **Ledger**; **Registry** answers “what exists.”

### Artifacts and blobs (data plane)

| Mode | Field | Bytes live in | Hashed into artifact id |
|------|--------|---------------|-------------------------|
| **Inline** (small) | `Trait.body` | artifact record | body bytes |
| **External** (large) | `Trait.object` | blob store via `PutBlob` | `ObjectRef` metadata, not raw blob |

- Default max inline: **1 MiB** (`ArtifactStorePolicy.max_inline_bytes`).
- `PutBlob` **copies** bytes (never “reference caller path in place”).
- Large evidence path: `PutBlob` → `PutArtifact` with `ObjectRef`.

Short guides: [`docs/Module.md`](docs/Module.md), [`docs/Artifact.md`](docs/Artifact.md).

### Firing and work units

A **work unit** is claimed and committed at most once per run. Effective firing:

`ExecutionPolicy.by_node[node]` → capability `firing` → policy `default` → `ONCE`

| Firing | Meaning |
|--------|---------|
| `ONCE` | One unit per node (default) |
| `ONCE_PER_KEY` | One per `(node, input_key)` over ports with `key=true` |
| `ALWAYS` | Re-fire when delivery generation changes (inject / upstream commit) |

### Two durability homes

| Concern | Owner |
|---------|--------|
| Kernel state (registry, ledger, runs, blob index) | `KernelApi` backend (`MemoryKernel` shipped; durable backends later) |
| Domain state (findings, evidence, worlds) | Modules via artifacts/blobs or their own store |

`MemoryKernel` is **one** implementation for conformance — not the authority of the contract.

### Kernel evolution rules (do not break these)

- Evolve the contract by **addition** within a major: new fields/messages/RPCs only.
- **Never** change/reuse field numbers or silently break v1 consumers.
- Run `buf breaking` for proto changes (`kernel/buf.yaml`).
- SDKs must pass the shared **conformance** suites (addressing, immutability, ledger chain, contract identity, run closure, etc.).
- Prefer path dependencies and keep Go/Python generated code in sync via `bash kernel/scripts/gen.sh`.

---

## Framework concepts (use these correctly)

The framework is an **application layer**, not a second kernel. Charter:
[`framework/SPEC.md`](framework/SPEC.md). Short guide: [`docs/Framework.md`](docs/Framework.md).

| Concept | Role |
|---------|------|
| **Host** | Registers plugins; `start_pipeline` / `drive` / `inject` / `cancel` / `resume_after` |
| **ModulePlugin** | Domain work (`execute`); optional UI / storage / memo hooks |
| **FrameworkPolicy** | Product presets → kernel `ExecutionPolicy` + host drive rules |
| **Step lifecycle** | Init → Progress\* → Final (presentation side channel; optional) |
| **Cut / seed** | Skip or subset nodes; crossing edges become `__seed/…` inputs |
| **StoragePlan** | Optional tabular side-channel (**not** the ledger) |
| **MemoPlan** | Optional cross-run work cache |

### Presets (prefer these over raw kernel fields)

| Preset | Intent |
|--------|--------|
| `converge()` | One-shot → terminal (`FIRST_TERMINAL`) |
| `stream()` | Stay open; force `ALWAYS` |
| `stream_dedupe()` | Stream once per key (`ONCE_PER_KEY`) |
| `selective(nodes)` | Only listed nodes + cut/seed |
| `start_after(node)` | Skip node + predecessors |
| `from_node(node)` | Only node + successors |
| `memoized()` | Converge + work cache |
| `manual(closure)` | Escape hatch |

Builders: `with_firing`, `with_nodes`, `with_drive`, `with_max_steps`,
`with_claim_modules`, `with_storage`, `with_memo`.

### Step lifecycle (presentation)

Per **work unit** (not module lifecycle):

```text
ClaimReady → on_init → execute { emit_progress* } → on_final → Put/Commit → on_store
```

| Stage | Contract ref |
|-------|--------------|
| Init | `srcport.ui.v1.StepInit` |
| Progress | `srcport.ui.v1.StepProgress` |
| Final | `srcport.ui.v1.StepFinal` |
| Skipped | host cut at `start_pipeline` |
| Cached | memo hit after claim |

**UI is optional.** Omitting hooks must never change run completion or domain commit semantics.
Schemas: [`framework/profiles/ui/`](framework/profiles/ui/).

### Cut / seed and memo

- **Cut**: host materialises a subset assembly before `StartRun`; dropped nodes get `StepStage::Skipped`; required seeds must be supplied or the host fails closed.
- **Memo key**: `H(module ‖ version ‖ digest ‖ capability ‖ sorted port→artifact_id)`. Empty `module_digest()` ⇒ uncacheable. On hit: `Cached` + commit prior outputs, skip `execute`.

### Framework invariants

1. Every durable domain effect goes through `KernelApi` — no shadow run state.
2. Artifacts stay immutable; hooks produce **new** bags.
3. Storage and UI are optional side channels; they do not redefine readiness.
4. Plugins must not import other plugins.
5. Never reverse-depend into `kernel/` from framework-only features that belong only in the host.

---

## Where to change what

| Goal | Touch |
|------|--------|
| Invariant shared by every consumer | `kernel/SPEC.md` + `substrate.proto` + all three SDKs + conformance |
| Product host loop, plugins, step UI, storage, memo, cut/seed | `framework/` only |
| Concept explainers for humans/agents | `docs/` |
| Generated Go/Python wire types | Edit proto, then `bash kernel/scripts/gen.sh` — do not hand-edit `_gen/` |

### Language layout

| Layer | Rust | Go | Python |
|-------|------|----|--------|
| Kernel SDK | `kernel/sdk/rust` | `kernel/sdk/go` | `kernel/sdk/python` |
| Framework SDK | `framework/sdk/rust` (primary) | `framework/sdk/go` | `framework/sdk/python` |

Rust is the reference implementation for host/plugin behaviour. Keep multi-language kernel behaviour aligned via shared known-answer / conformance tests.

---

## Commands agents should run

```bash
# Kernel — regenerate types after proto edits
bash kernel/scripts/gen.sh

# Kernel tests
cargo test --manifest-path kernel/sdk/rust/Cargo.toml
cd kernel/sdk/go && go test ./...
pip install ./kernel/sdk/python && python -m unittest discover -s kernel/sdk/python/tests -v

# Framework tests (Rust primary)
cargo test --manifest-path framework/sdk/rust/Cargo.toml
cargo clippy --manifest-path framework/sdk/rust/Cargo.toml --all-targets -- -D warnings
```

After kernel ABI changes that the framework uses, re-run **both** kernel and framework tests.

---

## Working style in this codebase

1. **Read the SPECs before inventing abstractions.** If a need is universal, widen the kernel; if it is product chrome, put it in the framework.
2. **Do not re-derive the contract.** Identity formulas, ledger hashing, claim/commit, and port matching are defined once — implement them, do not redesign them in a PR unless the SPEC is explicitly evolving.
3. **Preserve immutability.** Never mutate stored artifacts; produce new trait bags. Provenance is a `Derivation`, not fields on identity.
4. **Keep the data plane clean.** Events notify; artifact refs carry values; blobs are content-addressed copies.
5. **Prefer presets and builders** (`FrameworkPolicy::converge()`, …) over assembling raw `ExecutionPolicy` unless the escape hatch is required.
6. **Fail closed** on cuts (missing seeds), contract conflicts, and oversized payloads (`RESOURCE_EXHAUSTED`).
7. **Match existing style** in the SDK you are editing (error types, naming, test patterns). Prefer extending conformance over ad-hoc demos for kernel guarantees.
8. **Docs live next to the product.** Kernel law in `kernel/SPEC.md`; framework charter in `framework/SPEC.md`; short how-tos in `docs/`. Update them when behaviour changes.

---

## Doc map (read in this order when stuck)

| Question | Document |
|----------|----------|
| What is the monorepo? | [`README.md`](README.md) |
| What may the kernel guarantee? | [`kernel/SPEC.md`](kernel/SPEC.md) |
| What may the framework add? | [`framework/SPEC.md`](framework/SPEC.md) |
| How do modules work? | [`docs/Module.md`](docs/Module.md) |
| How do artifacts / blobs work? | [`docs/Artifact.md`](docs/Artifact.md) |
| How do host presets / storage / memo work? | [`docs/Framework.md`](docs/Framework.md) |
| Wire format | `kernel/contracts/proto/.../substrate.proto` |
| Framework quick start | [`framework/README.md`](framework/README.md) |
| Rust host surface | [`framework/sdk/rust/README.md`](framework/sdk/rust/README.md) |

---

## Non-goals (do not “helpfully” add)

- Kernel-level authorisation or multi-tenant isolation (trusted host model).
- Domain types (targets, findings, tiles, …) inside the kernel.
- Putting framework UI/storage semantics into `substrate.proto`.
- In-place filesystem path refs as `ObjectRef` (would need a new, weaker payload kind).
- Reverse imports: `kernel/**` → `framework/**`.

---

## License

Dual-licensed MIT OR Apache-2.0. See `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`.
