# srcport-substrate — Python SDK (v1.1.0)

The in-process Python realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the seven primitives and the one ABI.
`MemoryKernel` implements `KernelApi`. **Kernel-state durability** is a
`KernelApi` backend concern; **domain** state lives in Modules. One runtime
dependency: the `protobuf` runtime.

> The message types are **generated** from `substrate.proto` (via
> `buf generate`, committed under `src/srcport_substrate/_gen/`) and
> re-exported from `srcport_substrate`, so the SDK can never drift from the
> contract. They are canonical protobuf messages: construct with keyword args
> (`Artifact(type=…, body=…)`) and use fully-qualified enum values
> (`Lifecycle.LIFECYCLE_REGISTERED`). To add capability, widen the proto and run
> `scripts/gen.sh`; do not fork this.

## Install

```sh
pip install "git+https://github.com/philcantcode/srcport-substrate.git#subdirectory=kernel/sdk/python"
```

## Using it

```python
from srcport_substrate import (
    MemoryKernel, ModuleManifest, Capability, Port, Artifact, Event, Contract,
)

kernel = MemoryKernel()

# 1. A module registers, declaring the contracts it speaks.
kernel.register(ModuleManifest(
    name="recon",
    version="0.1.0",
    provides=[Capability(name="recon.scan", outputs=[Port(name="host", contract="acme.recon.v1.Host")])],
))

# 2. It produces an immutable, content-addressed artifact...
host = kernel.put_artifact(Artifact(
    type="acme.recon.v1.Host", body=b"10.0.0.1", produced_by="recon",
))

# 3. ...and publishes an event. Artifact refs are the data plane; coupling is
#    only through contract refs.
kernel.publish(Event(
    topic="recon.host.found", type="acme.recon.v1.Host",
    artifacts=[host], source="recon",
))

# 4. Contracts are immutable identities — put_contract pins ref → digest.
#    Re-registering the same ref with different content is a conflict.
kernel.put_contract(Contract(
    ref="acme.recon.v1.Host",
    media_type="application/schema+json",
    schema='{"type":"object"}',
    version="1.0.0",
))

# 5. The registry always answers "what exists right now."
snapshot = kernel.snapshot()
```

`MemoryKernel` implements `KernelApi` — the unary RPCs one-for-one (including
`transition`). `RequestContext` enforces deadlines and de-duplicates
`put_artifact` / `start_run` / `commit` via `request_key`.
`subscribe()` returns a bounded `queue.Queue[Event]` (`SUBSCRIBER_BUFFER`); a
slow consumer is shed rather than allowed to OOM the kernel. Events arrive in
kernel `seq` order. `MemoryKernel` is thread-safe.

## Convergent runs

A human-owned `Assembly` pins module versions, binds typed capability ports, and
names exactly one terminal output; `start_run` freezes it over immutable input
artifacts. Workers `claim_ready` their exact typed inputs and `commit` a
`Derivation` per node; the declared terminal artifact closes the run, and
`list_derivations` reads back every distinct production path. For a complete,
tested walkthrough see `test_run_feeds_forward_and_closes_on_terminal_answer` in
[`tests/test_conformance.py`](tests/test_conformance.py).

## Conformance

All eleven invariants from `SPEC.md` §Conformance are proven in
[`tests/test_conformance.py`](tests/test_conformance.py), using only the stdlib
— including feed-forward convergence, structural termination, and derivation
preservation, plus canonical ledger reconstruction cross-verified against the
shared known-answer chain hash:

```sh
PYTHONPATH=src python -m unittest discover -s tests -v
```
