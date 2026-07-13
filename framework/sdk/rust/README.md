# srcport-framework (Rust)

Host loop and `ModulePlugin` trait for products built on `srcport-substrate`.

```bash
cargo test --manifest-path framework/sdk/rust/Cargo.toml
cargo clippy --manifest-path framework/sdk/rust/Cargo.toml --all-targets -- -D warnings
```

## Surface

| Type | Role |
|------|------|
| [`Host`](src/lib.rs) | Plugins, `start_pipeline`, `resume_after`, `drive` / `inject` / `cancel`; optional `with_storage` / `with_memo` |
| [`FrameworkPolicy`](src/policy.rs) | Modes (`converge` / `stream` / `start_after` / `memoized` / …) |
| [`materialize_cut`](src/cut.rs) / [`seeds_from_run`](src/cut.rs) | Cut assembly + seed helpers for skip / start-after |
| [`MemoPlan`](src/memo.rs) / [`MemoryMemo`](src/memo.rs) | Cross-run work cache keyed by module digest + input artifact ids |
| [`ModulePlugin`](src/lib.rs) | `execute`; optional `module_digest`; presentation / storage hooks |
| [`Presentation`](src/presentation.rs) / [`StepEvent`](src/presentation.rs) | Step chrome; `Skipped` (cut) / `Cached` (memo) |
| [`UiPersist`](src/lib.rs) | Host-local events only, or also `PutArtifact` |
| [`StoragePlan`](src/storage.rs) / [`MemoryStorage`](src/storage.rs) | Optional tabular side-channel (PerRun / Shared / step_log) |

Depends only on the public `srcport-substrate` ABI (path dependency on
`kernel/sdk/rust`). See [`../../../kernel/SPEC.md`](../../../kernel/SPEC.md) and
[`../../SPEC.md`](../../SPEC.md).
