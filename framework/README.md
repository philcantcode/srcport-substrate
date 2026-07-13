# srcport-framework

**Opinionated application layer on the [kernel](../kernel/SPEC.md).**

This directory is a **framework**: host loop, module plugins, optional UI
profiles, and (later) composition helpers. The substrate under
[`../kernel/`](../kernel/) stays a small, domain-neutral microkernel.

```text
Your product / UI shell
        │
        ▼
┌─────────────────────────┐
│  framework/ (this tree) │  ← hooks, host, profiles
└───────────┬─────────────┘
            │ KernelApi only
            ▼
┌─────────────────────────┐
│  kernel/                │  ← seven primitives, unchanged
└─────────────────────────┘
```

| Want | Use |
|------|-----|
| Shared run/artifact/ledger ABI only | [`kernel/sdk/`](../kernel/sdk/) — no framework |
| Product modules with step UI + standard drive loop | **This package** |

## Docs

- **[`SPEC.md`](SPEC.md)** — charter, boundary, concepts, versioning
- **[`profiles/ui/`](profiles/ui/)** — well-known UI contract refs
- **[`sdk/rust/`](sdk/rust/)** — Rust host + `ModulePlugin`
- **[`sdk/go/`](sdk/go/)** — Go host + `ModulePlugin`
- **[`sdk/python/`](sdk/python/)** — Python host + `ModulePlugin`

## Quick start

```bash
# Rust
cargo test --manifest-path framework/sdk/rust/Cargo.toml

# Go
cd framework/sdk/go && go test ./...

# Python (install kernel SDK first)
pip install ./kernel/sdk/python ./framework/sdk/python
python -m unittest discover -s framework/sdk/python/tests -v
```

```rust
use srcport_framework::{
    DriveAfter, FrameworkPolicy, Host, MemoryStorage, ModulePlugin, PortBody,
    Presentation, StepContext, StepOutput, StepResult, StoragePlan,
};

// ModulePlugin: execute(&mut step); optional on_init / on_final;
// optional storage_schema / on_store when policy enables storage
// step.emit_progress(Presentation::progress("…", Some(0.5)));

// host.start_pipeline(id, assembly, inputs, FrameworkPolicy::converge())?;
// host.drive(id)?;
// let events = host.take_step_events(); // Init / Progress* / Final

// stream:
// host.start_pipeline(id, assembly, inputs, FrameworkPolicy::stream())?;
// host.inject(id, named_input, DriveAfter::UntilIdle)?;

// start after a step (seed frontier outputs; skip node + predecessors):
// host.start_pipeline(id, assembly, inputs_with_seeds, FrameworkPolicy::start_after("extract"))?;
// host.resume_after("run-2", "run-1", "extract", FrameworkPolicy::start_after("extract"))?;

// memoised re-runs (skip execute when module_digest + inputs match):
// let mut host = Host::new(kernel).with_memo(MemoryMemo::new());
// // ModulePlugin::module_digest() → Some("build-sha…")
// host.start_pipeline(id, assembly, inputs, FrameworkPolicy::memoized())?;

// storage (optional):
// let mut host = Host::new(kernel).with_storage(MemoryStorage::new());
// FrameworkPolicy::converge().with_storage(StoragePlan::per_run())
```

| Preset | Use when |
|--------|----------|
| `FrameworkPolicy::converge()` | One answer, then done |
| `FrameworkPolicy::stream()` | Keep run open; re-fire on inject |
| `FrameworkPolicy::stream_dedupe()` | Stream but once per key |
| `FrameworkPolicy::selective(nodes)` | Only some assembly nodes (seed cut edges) |
| `FrameworkPolicy::start_after(node)` | Skip that node + predecessors; seed outputs |
| `FrameworkPolicy::from_node(node)` | Run only that node + successors; seed the rest |
| `FrameworkPolicy::memoized()` | Converge + cross-run work cache |

| Storage | Use when |
|---------|----------|
| `StoragePlan::off()` | Default — no framework tables |
| `StoragePlan::per_run()` | Isolated tables per run (drop on end) |
| `StoragePlan::per_run_keep()` | Per-run tables kept after complete |
| `StoragePlan::shared()` | Durable module tables across runs |
| `StoragePlan::step_log_only()` | Framework step audit only |

See `framework/sdk/rust/tests/` (`host_drive`, `modes`, `lifecycle`, `storage`).

## Layout

```
framework/
├─ SPEC.md
├─ README.md
├─ profiles/ui/          # contract docs + JSON schemas
└─ sdk/
   ├─ rust/              # srcport-framework crate
   ├─ go/                # Go package
   └─ python/            # srcport_framework package
```

## Status

**`v2.2.0`** — host + `ModulePlugin` in **Rust, Go, and Python** under
`framework/sdk/{rust,go,python}` (layout move from `framework/rust/`). Builds on
v2.1 cut/seed and memo, plus v2.0 step lifecycle and `StoragePlan`.

Minimum substrate: **v2.0.0** (kernel **v2.1.0** recommended for store policy).
