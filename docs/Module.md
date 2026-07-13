# Modules

Short guide to how **modules** work in srcport-substrate.

## What a module is

A **module** is a self-contained vertical slice. It talks only to the kernel
via `KernelApi` — never imports another module. Coupling is exclusively
through **contract refs** on ports; artifact refs are the data plane.

Wire types: `ModuleManifest`, `Capability`, `Port`, `Lifecycle`,
`TransitionRequest`.

## Manifest

```text
ModuleManifest
  name        unique id (kebab-case)
  version     semver
  provides[]  capabilities (typed I/O)
  requires[]  availability hint only — never run dataflow
```

`requires` is a boot/load hint. The kernel does **not** gate `LOADED` or
`ClaimReady` on it. Run dataflow is expressed only by capability ports and
Assembly bindings.

## Capabilities and ports

A **capability** is a named thing the module can do. Typed I/O lives only on
**ports**:

| Side | `Port.traits` means |
|------|---------------------|
| **Input** | artifact must contain every listed trait (superset OK) |
| **Output** | module guarantees those traits (may add more) |

Assembly binding: source guarantees ⊇ target requires. The kernel never
parses trait bodies.

| Port flag | Effect |
|-----------|--------|
| `multiple` | more than one binding allowed |
| `optional` | unbound input permitted |
| `key` | participates in `FIRING_ONCE_PER_KEY` identity (artifact ids only) |

Default **firing** is declared on the capability; a run may override via
`ExecutionPolicy`:

| `Firing` | Work units per run |
|----------|--------------------|
| `ONCE` | at most one per node (default if unspecified) |
| `ALWAYS` | re-fire when input delivery generation changes |
| `ONCE_PER_KEY` | at most one per `(node, input_key)` |

`input_key` hashes artifact ids on ports with `key=true` (or all inputs if
none are marked).

## Lifecycle

Exactly four states; `Transition` advances **one forward step** at a time:

```text
REGISTERED ──► LOADED ──► ACTIVE ──► DEACTIVATED
   Register     deps ok     activate    shut down
```

No back-doors, no skipping.

## How modules connect

Modules never call each other. Data and control stay separate:

```text
  Module A ──PutArtifact──► kernel ──ClaimReady/Commit──► Module B
                │
                └── Publish(event with artifact refs)  (notify only)
```

- **Artifacts** carry values (refs on events / run bindings).
- **Events** only notify; the bus never carries domain value bytes.
- In a **Run**, a node is claimable only when every bound input artifact
  exists (fan-in waits; it does not race).
- Claims are **leased**: `ClaimReady` may return a batch; `Commit` or terminal
  `FailWork` completes the unit; lease expiry / retryable fail returns it to
  READY (until `max_attempts`). See kernel SPEC run-closure / leased concurrency.

## Further reading

- Kernel overview: [`kernel/README.md`](../kernel/README.md) — module
  lifecycle + seven primitives
- Contract: [`kernel/SPEC.md`](../kernel/SPEC.md) — Module primitive + Run
- Wire types: [`kernel/contracts/proto/.../substrate.proto`](../kernel/contracts/proto/srcport/substrate/v1/substrate.proto)
- Concurrency / leases: [`Concurrency.md`](Concurrency.md)
- Sibling: [`Artifact.md`](Artifact.md)
