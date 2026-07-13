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
- **[`rust/`](rust/)** — first implementation (host + `ModulePlugin`)

## Quick start (Rust)

```bash
cargo test --manifest-path framework/rust/Cargo.toml
```

```rust
use srcport_framework::{
    DriveAfter, FrameworkPolicy, Host, ModulePlugin, PortBody, Presentation,
    StepContext, StepOutput, StepResult,
};

// ModulePlugin: execute(&mut step); optional on_init / on_final;
// step.emit_progress(Presentation::progress("…", Some(0.5)));

// host.start_pipeline(id, assembly, inputs, FrameworkPolicy::converge())?;
// host.drive(id)?;
// let events = host.take_step_events(); // Init / Progress* / Final

// stream:
// host.start_pipeline(id, assembly, inputs, FrameworkPolicy::stream())?;
// host.inject(id, named_input, DriveAfter::UntilIdle)?;
```

| Preset | Use when |
|--------|----------|
| `FrameworkPolicy::converge()` | One answer, then done |
| `FrameworkPolicy::stream()` | Keep run open; re-fire on inject |
| `FrameworkPolicy::stream_dedupe()` | Stream but once per key |
| `FrameworkPolicy::selective(nodes)` | Only some assembly nodes |

See `framework/rust/tests/` (`host_drive`, `modes`).

## Layout

```
framework/
├─ SPEC.md
├─ README.md
├─ profiles/ui/          # contract docs + JSON schemas
└─ rust/                 # srcport-framework crate
```

## Status

**`v0.1.0`** — scaffolding. Manual assemblies, Rust host, optional processing /
result UI as JSON artifacts. Auto-composer not yet implemented.

Minimum substrate: **v1.1.0**.
