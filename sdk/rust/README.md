# srcport-substrate — Rust SDK (v0.1)

The in-process Rust realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the seven primitives and the one ABI,
nothing more.

> This crate does **not** re-derive the core. Its wire types are generated from
> `substrate.proto` at build time (`build.rs`, via `protox` — no `protoc`
> binary needed), so it can never drift from the contract. To add capability,
> widen the proto; do not fork this.

## Using it

```rust
use srcport_substrate::*;

let kernel = Kernel::new();

// 1. A module registers, declaring the contracts it speaks.
kernel.register(ModuleManifest {
    name: "recon".into(),
    version: "0.1.0".into(),
    provides: vec![Capability {
        name: "recon.scan".into(),
        contract: "acme.recon.v1.Host".into(),
    }],
    requires: vec![],
});

// 2. It produces an immutable, content-addressed artifact...
let host = kernel.put_artifact(Artifact {
    r#type: "acme.recon.v1.Host".into(),
    body: b"10.0.0.1".to_vec(),
    produced_by: "recon".into(),
    ..Default::default()
});

// 3. ...and publishes an event. Coupling is only through contract refs.
kernel.publish(Event {
    topic: "recon.host.found".into(),
    r#type: "acme.recon.v1.Host".into(),
    payload: host.id.into_bytes(),
    source: "recon".into(),
    ..Default::default()
});

// 4. Before anything irreversible, open a human-held gate and wait.
let ticket = kernel.request_gate(GateRequest {
    action: "exploit host 10.0.0.1".into(),
    requested_by: "recon".into(),
    ..Default::default()
});
kernel.ensure_approved(&ticket)?; // Err(GateBlocked) until a human APPROVES.

// 5. The registry always answers "what exists right now."
let snapshot = kernel.snapshot();
```

The `Kernel` methods mirror the `service Kernel` RPCs one-for-one
(`register`, `put_artifact`, `get_artifact`, `publish`, `subscribe`, `append`,
`request_gate`, `decide_gate`, `await_gate`, `snapshot`). `subscribe` returns an
mpsc `Receiver<Event>` as the in-process "stream". The `Kernel` is `Send + Sync`
— share it across module threads behind an `Arc`.

## Conformance

The six invariants from `SPEC.md` §Conformance are proven in
[`tests/conformance.rs`](tests/conformance.rs):

| # | Invariant | Test |
|---|-----------|------|
| 1 | Addressing is content-derived & metamorphic | `addressing_is_content_derived_and_metamorphic` |
| 2 | Artifacts are immutable | `artifacts_are_immutable` |
| 3 | Events are ordered & isolated | `events_are_ordered_and_isolated` |
| 4 | Ledger is tamper-evident | `ledger_is_tamper_evident` |
| 5 | Gates are non-bypassable | `gates_are_non_bypassable` (+ `await_gate_blocks_until_decided`) |
| 6 | Registry reports everything | `registry_reports_everything` |

```sh
cargo test        # runs the conformance suite
cargo clippy --all-targets
```
