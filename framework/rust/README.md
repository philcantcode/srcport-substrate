# srcport-framework (Rust)

Host loop and `ModulePlugin` trait for products built on `srcport-substrate`.

```bash
cargo test --manifest-path framework/rust/Cargo.toml
cargo clippy --manifest-path framework/rust/Cargo.toml --all-targets -- -D warnings
```

## Surface

| Type | Role |
|------|------|
| [`Host`](src/lib.rs) | Plugins, `start_pipeline`, `drive` / `inject` / `cancel` |
| [`FrameworkPolicy`](src/policy.rs) | Modes: converge / stream / stream_dedupe / selective |
| [`ModulePlugin`](src/lib.rs) | `execute`; `on_init` / `emit_progress` / `on_final` |
| [`Presentation`](src/presentation.rs) / [`StepEvent`](src/presentation.rs) | Step lifecycle chrome (no real UI) |
| [`UiPersist`](src/lib.rs) | Host-local events only, or also `PutArtifact` |

Depends only on the public `srcport-substrate` ABI (path dependency on
`kernel/sdk/rust`). See [`../../kernel/SPEC.md`](../../kernel/SPEC.md) and
[`../SPEC.md`](../SPEC.md).
