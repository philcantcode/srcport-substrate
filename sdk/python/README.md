# srcport-substrate — Python SDK (v0.1)

The in-process Python realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the seven primitives and the one ABI.
Stdlib only; no runtime dependencies.

> The dataclasses in [`_types.py`](src/srcport_substrate/_types.py) are a
> faithful hand-port of `substrate.proto`, which stays the single source of
> truth. To add capability, widen the proto; do not fork this.

## Install

```sh
pip install "git+https://github.com/philcantcode/srcport-substrate.git#subdirectory=sdk/python"
```

## Using it

```python
from srcport_substrate import (
    Kernel, ModuleManifest, Capability, Artifact, Event, GateRequest,
)

kernel = Kernel()

# 1. A module registers, declaring the contracts it speaks.
kernel.register(ModuleManifest(
    name="recon",
    version="0.1.0",
    provides=[Capability(name="recon.scan", contract="acme.recon.v1.Host")],
))

# 2. It produces an immutable, content-addressed artifact...
host = kernel.put_artifact(Artifact(
    type="acme.recon.v1.Host", body=b"10.0.0.1", produced_by="recon",
))

# 3. ...and publishes an event. Coupling is only through contract refs.
kernel.publish(Event(
    topic="recon.host.found", type="acme.recon.v1.Host",
    payload=host.id.encode(), source="recon",
))

# 4. Before anything irreversible, open a human-held gate and wait.
ticket = kernel.request_gate(GateRequest(
    action="exploit host 10.0.0.1", requested_by="recon",
))
kernel.ensure_approved(ticket)  # raises GateBlocked until a human APPROVES

# 5. The registry always answers "what exists right now."
snapshot = kernel.snapshot()
```

The `Kernel` methods mirror the `service Kernel` RPCs one-for-one.
`subscribe()` returns a `queue.Queue[Event]` as the in-process "stream"; events
arrive in kernel `seq` order. The `Kernel` is thread-safe.

## Conformance

The six invariants from `SPEC.md` §Conformance are proven in
[`tests/test_conformance.py`](tests/test_conformance.py), using only the stdlib:

```sh
PYTHONPATH=src python -m unittest discover -s tests -v
```
