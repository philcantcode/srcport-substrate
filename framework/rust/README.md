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
| [`ModulePlugin`](src/lib.rs) | `manifest` + `execute`; optional UI hooks |
| [`ProcessingView`](src/lib.rs) / [`ResultView`](src/lib.rs) | `srcport.ui.v1` JSON bodies |
| [`UiPersist`](src/lib.rs) | Host-local events only, or also `PutArtifact` |

Depends only on the public `srcport-substrate` ABI (path dependency on
`kernel/sdk/rust`). See [`../../kernel/SPEC.md`](../../kernel/SPEC.md) and
[`../SPEC.md`](../SPEC.md).
