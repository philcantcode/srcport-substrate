# srcport-framework (Rust)

Host loop and `ModulePlugin` trait for products built on `srcport-substrate`.

```bash
cargo test --manifest-path framework/rust/Cargo.toml
cargo clippy --manifest-path framework/rust/Cargo.toml --all-targets -- -D warnings
```

## Surface

| Type | Role |
|------|------|
| [`Host`](src/lib.rs) | Plugins, `start_pipeline`, `drive` / `inject` / `cancel`; optional `with_storage` |
| [`FrameworkPolicy`](src/policy.rs) | Modes + optional [`StoragePlan`](src/storage.rs) |
| [`ModulePlugin`](src/lib.rs) | `execute`; presentation hooks; optional `storage_schema` / `on_store` |
| [`Presentation`](src/presentation.rs) / [`StepEvent`](src/presentation.rs) | Step lifecycle chrome (no real UI) |
| [`UiPersist`](src/lib.rs) | Host-local events only, or also `PutArtifact` |
| [`StoragePlan`](src/storage.rs) / [`MemoryStorage`](src/storage.rs) | Optional tabular side-channel (PerRun / Shared / step_log) |

Depends only on the public `srcport-substrate` ABI (path dependency on
`kernel/sdk/rust`). See [`../../kernel/SPEC.md`](../../kernel/SPEC.md) and
[`../SPEC.md`](../SPEC.md).
