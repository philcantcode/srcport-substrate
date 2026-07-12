# srcport-substrate — Rust SDK (v0.1)

The in-process Rust realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the eight primitives and the one ABI,
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
        outputs: vec![Port { name: "host".into(), contract: "acme.recon.v1.Host".into(), ..Default::default() }],
        ..Default::default()
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

// 3. ...and publishes an event. Artifact refs are the data plane; coupling is
//    only through contract refs.
kernel.publish(Event {
    topic: "recon.host.found".into(),
    r#type: "acme.recon.v1.Host".into(),
    artifacts: vec![host.clone()],
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
`request_gate`, `decide_gate`, `await_gate`, `snapshot`, and the convergent-run
methods `start_run`, `claim_ready`, `commit`, `get_run`, `cancel_run`,
`list_derivations`). `subscribe` returns an mpsc `Receiver<Event>` as the
in-process "stream". The `Kernel` is `Send + Sync` — share it across module
threads behind an `Arc`.

## Convergent runs

A human-owned `Assembly` pins module versions, binds typed capability ports, and
names exactly one terminal output; `start_run` freezes it over immutable input
artifacts. Workers `claim_ready` their exact typed inputs and `commit` a
`Derivation` per node; the declared terminal artifact closes the run, and
`list_derivations` reads back every distinct production path. For a complete,
tested walkthrough see `run_feeds_forward_and_closes_on_its_terminal_answer` in
[`tests/conformance.rs`](tests/conformance.rs).

## Conformance

All eleven invariants from `SPEC.md` §Conformance are proven in
[`tests/conformance.rs`](tests/conformance.rs) — including feed-forward
convergence, structural termination, and derivation preservation, plus canonical
ledger reconstruction cross-verified against the shared known-answer chain hash:

```sh
cargo test        # runs the conformance suite
cargo clippy --all-targets
```
