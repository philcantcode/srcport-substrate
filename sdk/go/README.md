# srcport-substrate — Go SDK (v0.1)

The in-process Go realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the seven primitives and the one ABI.

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
	k := substrate.NewKernel()

	// 1. A module registers, declaring the contracts it speaks.
	k.Register(&substrate.ModuleManifest{
		Name:    "recon",
		Version: "0.1.0",
		Provides: []*substrate.Capability{
			{Name: "recon.scan", Contract: "acme.recon.v1.Host"},
		},
	})

	// 2. It produces an immutable, content-addressed artifact...
	host := k.PutArtifact(&substrate.Artifact{
		Type:       "acme.recon.v1.Host",
		Body:       []byte("10.0.0.1"),
		ProducedBy: "recon",
	})

	// 3. ...and publishes an event. Coupling is only through contract refs.
	k.Publish(&substrate.Event{
		Topic:   "recon.host.found",
		Type:    "acme.recon.v1.Host",
		Payload: []byte(host.Id),
		Source:  "recon",
	})

	// 4. Before anything irreversible, open a human-held gate and wait.
	ticket := k.RequestGate(&substrate.GateRequest{
		Action:      "exploit host 10.0.0.1",
		RequestedBy: "recon",
	})
	if err := k.EnsureApproved(ticket); err != nil {
		fmt.Println("blocked until a human APPROVES:", err)
	}

	// 5. The registry always answers "what exists right now."
	_ = k.Snapshot()
}
```

The `Kernel` methods mirror the `service Kernel` RPCs one-for-one. `Subscribe`
returns an unbounded `<-chan *Event` (a background pump means the publisher
never blocks). Values handed in and out are cloned, so a caller can never mutate
stored state through a shared pointer. The `Kernel` is safe for concurrent use
across goroutines.

## Conformance

The six invariants from `SPEC.md` §Conformance are proven in
[`conformance_test.go`](conformance_test.go):

```sh
go test ./...
go vet ./...
```

## Regenerating

The types in `internal/genpb/` are generated from the contract. After changing
`substrate.proto`, run `scripts/gen.sh` from the repo root (needs
[`buf`](https://buf.build); no `protoc` binary required). CI fails if the
committed codegen is stale.
