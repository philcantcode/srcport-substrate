# srcport-substrate — Rust SDK (v1.1.0)

The in-process Rust realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the seven primitives and the one ABI,
nothing more. The in-memory type is [`MemoryKernel`](src/lib.rs); it implements
the [`KernelApi`](src/lib.rs) trait. **Kernel-state durability** is a
`KernelApi` backend concern; **domain** state lives in Modules.
`MemoryKernel` is one backend, not the authority.

> This crate does **not** re-derive the core. Its wire types are generated from
> `substrate.proto` at build time (`build.rs`, via `protox` — no `protoc`
> binary needed), so it can never drift from the contract. To add capability,
> widen the proto; do not fork this.

## Using it

```rust
use srcport_substrate::*;

let kernel = MemoryKernel::new();

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
// Small values inline; large values put_blob then place a verified ObjectRef.
let host = kernel.put_artifact(Artifact {
    r#type: "acme.recon.v1.Host".into(),
    body: b"10.0.0.1".to_vec(),
    produced_by: "recon".into(),
    ..Default::default()
})?;
// Large evidence without copying into the artifact store:
// let blob = kernel.put_blob(PutBlobRequest { namespace: "evidence".into(), data: pcap });
// let capture = kernel.put_artifact(Artifact {
//     r#type: "observer.v1.Capture".into(),
//     object: Some(ObjectRef { digest: blob.digest, byte_count: blob.byte_count, namespace: blob.namespace }),
//     ..Default::default()
// })?;

// 3. ...and publishes an event. Artifact refs are the data plane; coupling is
//    only through contract refs.
kernel.publish(Event {
    topic: "recon.host.found".into(),
    r#type: "acme.recon.v1.Host".into(),
    artifacts: vec![host.clone()],
    source: "recon".into(),
    ..Default::default()
});

// 4. Contracts are immutable identities — put_contract pins ref → digest.
//    Re-registering the same ref with different content is a conflict.
kernel.put_contract(Contract {
    r#ref: "acme.recon.v1.Host".into(),
    media_type: "application/schema+json".into(),
    schema: r#"{"type":"object"}"#.into(),
    version: "1.0.0".into(),
    ..Default::default()
})?;

// 5. The registry always answers "what exists right now."
let snapshot = kernel.snapshot();
```

`MemoryKernel` implements `KernelApi` — the unary RPCs one-for-one (including
`Transition`). `RequestContext` enforces deadlines and de-duplicates
`PutArtifact` / `StartRun` / `Commit` via `request_key`
(`register`, `put_artifact`, `get_artifact`, `put_blob`, `get_blob`, `has_blob`,
`put_contract`, `publish`, `append`, `snapshot`, and the convergent-run methods).
`subscribe` returns a bounded mpsc `Receiver<Event>` (`SUBSCRIBER_BUFFER`).
`MemoryKernel` is `Send + Sync` — share it across module threads behind an
`Arc`.

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
