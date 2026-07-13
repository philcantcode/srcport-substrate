# srcport-framework (Python)

Host loop and `ModulePlugin` for products built on `srcport-substrate`.

```bash
# from monorepo root (editable install of kernel + framework)
pip install ./kernel/sdk/python ./framework/sdk/python
python -m unittest discover -s framework/sdk/python/tests -v
```

## Surface

| Type | Role |
|------|------|
| `Host` | Plugins, `start_pipeline`, `resume_after`, `drive` / `inject` / `cancel` |
| `FrameworkPolicy` | Modes (`converge` / `stream` / `start_after` / `memoized` / …) |
| `materialize_cut` / `seeds_from_run` | Cut assembly + seed helpers |
| `MemoPlan` / `MemoryMemo` | Cross-run work cache |
| `ModulePlugin` | `execute`; optional `module_digest`, presentation / storage hooks |
| `Presentation` / `StepEvent` | Step chrome; Skipped / Cached events |
| `StoragePlan` / `MemoryStorage` | Optional tabular side-channel |

Depends only on the public kernel Python SDK. See
[`../../../kernel/SPEC.md`](../../../kernel/SPEC.md) and
[`../../SPEC.md`](../../SPEC.md).
