# srcport-substrate — Go SDK (v1.0.0)

The in-process Go realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the seven primitives and the one ABI.
`MemoryKernel` implements `KernelApi`. **Durability lives in Modules, not the
core** — the in-memory kernel is one backend, not the authority.

> The message types are **generated** from `substrate.proto` (via
> `buf generate`, committed under [`internal/genpb/`](internal/genpb/)) and
> re-exported from package `substrate`, so the SDK can never drift from the
> contract. They are the `google.golang.org/protobuf` message types — construct
> them with a pointer (`&substrate.Artifact{…}`). To add capability, widen the
> proto and run `scripts/gen.sh`; do not fork this.

## Install

```sh
go get github.com/philcantcode/srcport-substrate/sdk/go
```

## Using it

```go
package main

import (
	"fmt"

	substrate "github.com/philcantcode/srcport-substrate/sdk/go"
)

func main() {
	k := substrate.NewMemoryKernel()

	// 1. A module registers, declaring the contracts it speaks.
	k.Register(&substrate.ModuleManifest{
		Name:    "recon",
		Version: "0.1.0",
		Provides: []*substrate.Capability{
			{Name: "recon.scan", Outputs: []*substrate.Port{{Name: "host", Contract: "acme.recon.v1.Host"}}},
		},
	})

	// 2. Small values inline; large values PutBlob then place a verified ObjectRef.
	host, err := k.PutArtifact(&substrate.Artifact{
		Type:       "acme.recon.v1.Host",
		Body:       []byte("10.0.0.1"),
		ProducedBy: "recon",
	})
	if err != nil {
		panic(err)
	}
	// Large evidence without copying into the artifact store:
	// blob := k.PutBlob(&substrate.PutBlobRequest{Namespace: "evidence", Data: pcap})
	// capture, _ := k.PutArtifact(&substrate.Artifact{Type: "observer.v1.Capture",
	//   Object: &substrate.ObjectRef{Digest: blob.Digest, ByteCount: blob.ByteCount, Namespace: blob.Namespace}})

	// 3. ...and publishes an event. Artifact refs are the data plane; coupling
	//    is only through contract refs.
	k.Publish(&substrate.Event{
		Topic:     "recon.host.found",
		Type:      "acme.recon.v1.Host",
		Artifacts: []*substrate.ArtifactRef{host},
		Source:    "recon",
	})

	// 4. Contracts are immutable identities — PutContract pins ref → digest.
	//    Re-registering the same ref with different content is a conflict.
	if _, err := k.PutContract(&substrate.Contract{
		Ref: "acme.recon.v1.Host", MediaType: "application/schema+json",
		Schema: `{"type":"object"}`, Version: "1.0.0",
	}); err != nil {
		fmt.Println("contract registration failed:", err)
	}

	// 5. The registry always answers "what exists right now."
	_ = k.Snapshot()
}
```

The `MemoryKernel` methods mirror the `service Kernel` RPCs one-for-one (and
implement `KernelApi`). `Subscribe` returns a bounded `<-chan *Event`
(`SubscriberBuffer`); a slow consumer is shed rather than allowed to OOM the
kernel. Values handed in and out are cloned, so a caller can never mutate stored
state through a shared pointer. `MemoryKernel` is safe for concurrent use across
goroutines.

## Convergent runs

A human-owned `Assembly` pins module versions, binds typed capability ports, and
names exactly one terminal output; `StartRun` freezes it over immutable input
artifacts. Workers `ClaimReady` their exact typed inputs and `Commit` a
`Derivation` per node; the declared terminal artifact closes the run, and
`ListDerivations` reads back every distinct production path. For a complete,
tested walkthrough see `TestRunFeedsForwardAndClosesOnTerminalAnswer` in
[`conformance_test.go`](conformance_test.go).

## Conformance

All eleven invariants from `SPEC.md` §Conformance are proven in
[`conformance_test.go`](conformance_test.go) — including feed-forward
convergence, structural termination, and derivation preservation, plus canonical
ledger reconstruction cross-verified against the shared known-answer chain hash:

```sh
go test ./...
go vet ./...
```

## Regenerating

The types in `internal/genpb/` are generated from the contract. After changing
`substrate.proto`, run `scripts/gen.sh` from the repo root (needs
[`buf`](https://buf.build); no `protoc` binary required). CI fails if the
committed codegen is stale.
