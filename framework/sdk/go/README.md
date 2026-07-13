# srcport-framework (Go)

Host loop and `ModulePlugin` interface for products built on
`srcport-substrate`.

```bash
cd framework/sdk/go && go test ./...
```

## Surface

| Type | Role |
|------|------|
| `Host` | Plugins, `StartPipeline`, `ResumeAfter`, `Drive` / `Inject` / `Cancel` |
| `FrameworkPolicy` | Modes (`Converge` / `Stream` / `StartAfter` / `Memoized` / …) |
| `MaterializeCut` / `SeedsFromRun` | Cut assembly + seed helpers |
| `MemoPlan` / `MemoryMemo` | Cross-run work cache |
| `ModulePlugin` | `Execute`; optional `ModuleDigest`, presentation / storage hooks |
| `Presentation` / `StepEvent` | Step chrome; Skipped / Cached events |
| `StoragePlan` / `MemoryStorage` | Optional tabular side-channel |

Depends only on the public kernel Go SDK
(`kernel/sdk/go`). See [`../../../kernel/SPEC.md`](../../../kernel/SPEC.md)
and [`../../SPEC.md`](../../SPEC.md).
