# Concurrency

Short guide to **leased, concurrent work units** on srcport-substrate: how the
kernel schedules claims safely, and how the framework drives modules in
parallel for throughput.

## The one picture

```text
                    ┌─────────────────────────────────────┐
  ready units       │  Kernel (authority)                 │
  (by firing)  ───► │  ClaimReady(max_items, filter)      │
                    │    READY ──lease──► CLAIMED         │
                    │       ▲                │            │
                    │       │ lease expiry   ├─ Commit ──► DONE
                    │       │ or Fail(retry) ├─ Fail(final)► DONE
                    │       └────────────────┘            │
                    │  max_in_flight caps CLAIMED         │
                    └─────────────────────────────────────┘
                                      ▲
                                      │ batch claim / commit / fail / heartbeat
                    ┌─────────────────┴───────────────────┐
                    │  Framework Host                     │
                    │  pool of N workers (concurrency)    │
                    │  claim → hooks → execute → put/commit
                    └─────────────────────────────────────┘
```

**Kernel = leased work queue. Framework = worker pool that drains it.**

Firing modes (`ONCE` / `ONCE_PER_KEY` / `ALWAYS`) still decide *which* units
exist. Concurrency decides *how many* can be in flight and *how* the host
runs them.

## Work-unit lifecycle

| State | Meaning |
|-------|---------|
| **READY** | Inputs resolved; not DONE; not currently leased |
| **CLAIMED** | Held under a lease by a worker; not claimable by others |
| **DONE** | Terminal success (`Commit`) or terminal failure (`FailWork` / attempts exhausted) |

```text
READY ──ClaimReady──► CLAIMED ──Commit──────────────► DONE
                         │
                         ├─ FailWork(terminal) ─────► DONE
                         ├─ FailWork(retryable) ────► READY  (if attempts remain)
                         └─ lease expiry ───────────► READY  (or DONE if max_attempts)
```

Invariants:

1. A unit is **committed at most once** (same firing identity as before).
2. While **CLAIMED**, no other worker holds that `unit_key`.
3. Claim is **leased**, not permanent — dead workers do not poison the run.
4. Downstream readiness still requires a successful **Commit** (fan-in unchanged).
5. `Limits.max_in_flight` bounds concurrent CLAIMED units (0 = unbounded).

## Kernel API (essentials)

### ClaimReady (batch)

```text
ClaimRequest
  run_id
  module / modules / node_ids   # soft filters; empty = any
  max_items                     # 0 → treat as 1

ClaimResponse
  items[] WorkItem              # may be fewer than max_items; empty = nothing
```

Each `WorkItem` includes:

| Field | Role |
|-------|------|
| `unit_key` | Stable identity from firing (`once:…` / `key:…` / `always:…`) |
| `attempt` | 1-based claim count for this unit in the run |
| `lease_until_unix_ms` | Absolute lease deadline (live; **zeroed in ledger detail**) |

### Heartbeat

Extend leases on long `execute`. Renew listed work ids first (including
barely-expired still-held claims), then reap other expired units.

### FailWork

```text
FailWorkRequest
  run_id, work_id, reason
  terminal   # true → DONE; false → READY if attempts remain
```

Returns the `Run` (may stall under `FIRST_TERMINAL` when nothing is ready or
in flight).

### Limits (frozen at StartRun)

| Field | Zero means | Typical default |
|-------|------------|-----------------|
| `max_steps` | node count | mode-dependent |
| `max_in_flight` | unbounded | host concurrency |
| `default_lease_ms` | SDK default | **60_000** |
| `max_attempts` | SDK default | **3** |

### Stall rules

Under `FIRST_TERMINAL`, the run **STALLs** only when there is no READY unit
**and** no CLAIMED unit (after reaping expired leases). CLAIMED work still
counts as in flight.

### Ledger

| kind | detail |
|------|--------|
| `work.claimed` | `WorkItem` with `lease_until_unix_ms = 0` (chain-stable) |
| `work.expired` | reaped lease |
| `work.failed` | `WorkFailure` |
| `derivation.committed` | unchanged |

## Framework: parallel by default

### Policy knobs

| Builder | Effect |
|---------|--------|
| `with_concurrency(n)` | Host worker pool size; sets kernel `max_in_flight` (default **8**) |
| `with_claim_batch(n)` | `ClaimReady.max_items` per wave (default = concurrency) |
| `with_lease_ms(ms)` | Kernel lease duration |
| `with_max_attempts(n)` | Kernel retry budget |
| `with_claim_modules(…)` | Soft allow-list for claims |

Presets (`converge`, `stream`, `stream_dedupe`, cut modes, `memoized`) all use
the same drive machinery. Serial escape hatch:

```text
FrameworkPolicy::converge().with_concurrency(1)
```

### Drive loop

```text
while RUNNING:
  capacity ← concurrency
  items ← ClaimReady(run, filters, max_items=batch)
  if empty → idle return
  for each item (parallel when concurrency > 1):
    memo hit? → commit cached outputs
    else → on_init → execute → on_final → Put* → Commit
    on execute error → FailWork(retryable) → StepFailed
```

Rust host may run domain `execute` concurrently for independent units; put /
commit / fail stay serialised on the host. Go and Python drain the batch
serially but still batch-claim and map concurrency into kernel limits (multi-
process workers can claim in parallel against a shared kernel).

### Plugins

- Domain `execute` must be safe under shared use (Rust: `Send + Sync`,
  `execute(&self)` — use interior mutability for counters).
- Presentation / storage / memo remain optional side channels.
- Step events are ordered **per work unit**, not globally, under parallel drive.

## Getting maximum throughput

1. **Widen the graph** where the domain allows: independent nodes and
   `ONCE_PER_KEY` fan-out create ready units the pool can drain.
2. **Raise concurrency** to match CPU / I/O / API budgets:
   `with_concurrency(32)` (and keep `max_in_flight` aligned).
3. **Keep execute off the hot path for huge bytes** — `PutBlob` + `ObjectRef`,
   not multi-MiB inline traits.
4. **Memo** pure steps so recomputation never occupies a worker.
5. **Heartbeat** (or a long enough lease) for steps that exceed `default_lease_ms`.
6. **External workers** may call `ClaimReady` / `Commit` / `FailWork` directly
   against any `KernelApi` backend; leases make multi-worker safe.

## What not to put here

| Non-goal | Owner |
|----------|--------|
| Domain batching / vectorized keys | Module design |
| Priority queues / fair sharing policy | Product host |
| Kernel thread pool | Host / runtime |
| Multi-tenant isolation | Outside substrate (trusted host model) |

## Further reading

- Kernel law: [`kernel/SPEC.md`](../kernel/SPEC.md) — run closure, leased concurrency
- Wire types: `kernel/contracts/proto/.../substrate.proto` (`ClaimRequest`,
  `ClaimResponse`, `Heartbeat*`, `FailWorkRequest`, `Limits`)
- Framework charter: [`framework/SPEC.md`](../framework/SPEC.md)
- Host presets: [`Framework.md`](Framework.md)
- Modules / firing: [`Module.md`](Module.md)
- Artifacts / blobs: [`Artifact.md`](Artifact.md)
