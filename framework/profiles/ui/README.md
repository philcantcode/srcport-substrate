# UI profile — `srcport.ui.v1`

Optional **presentation** contracts for framework hosts. Modules emit structured
data only — never widgets, HTML, or shell code. **Not** part of `substrate.proto`.

## Step lifecycle contracts

| Stage | Contract ref | Schema | When |
|-------|--------------|--------|------|
| **Init** | `srcport.ui.v1.StepInit` | [`step_init.schema.json`](step_init.schema.json) | After claim, before domain `execute` |
| **Progress** | `srcport.ui.v1.StepProgress` | [`step_progress.schema.json`](step_progress.schema.json) | Zero or more during `execute` (`StepContext::emit_progress`) |
| **Final** | `srcport.ui.v1.StepFinal` | [`step_final.schema.json`](step_final.schema.json) | After outputs or on step failure |

Media type: `application/json`. Artifact `body` is UTF-8 JSON when the host uses
`UiPersist::Artifacts`.

## Rules

1. Presentation is a **side channel**. Domain ports carry domain contracts only.
2. Missing presentation never blocks `ClaimReady` / `Commit`.
3. Hosts **may** keep events host-local and/or put them as kernel artifacts.
4. Each emit is a new immutable payload (never mutate a prior presentation artifact).
5. Evolve by additive JSON fields; bump to `srcport.ui.v2` only for breaks.

## Legacy aliases

| Legacy ref | Prefer |
|------------|--------|
| `srcport.ui.v1.ProcessingView` | `StepInit` / `StepProgress` |
| `srcport.ui.v1.ResultView` | `StepFinal` |

Legacy schemas remain for older shells; new code should use step stages.

## Example progress body

```json
{
  "stage": "progress",
  "title": "Extracting facts",
  "status": "running",
  "detail": "hosts 30/100",
  "progress": 0.3,
  "run_id": "run-1",
  "work_id": "work:run-1/extract",
  "node_id": "extract",
  "module": "extractor",
  "phase": "scan"
}
```
