# srcport-substrate — Go SDK (v0.1)

The in-process Go realisation of the `Kernel` ABI defined in
[`../../contracts/proto/srcport/substrate/v1/substrate.proto`](../../contracts/proto/srcport/substrate/v1/substrate.proto).
It conforms to [`SPEC.md`](../../SPEC.md) — the seven primitives and the one ABI.
Stdlib only; no dependencies.

> The message types in [`substrate.go`](substrate.go) are a faithful hand-port
> of `substrate.proto`, which stays the single source of truth. To add
> capability, widen the proto; do not fork this.

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
	k.Register(substrate.ModuleManifest{
		Name:    "recon",
		Version: "0.1.0",
		Provides: []substrate.Capability{
			{Name: "recon.scan", Contract: "acme.recon.v1.Host"},
		},
	})

	// 2. It produces an immutable, content-addressed artifact...
	host := k.PutArtifact(substrate.Artifact{
		Type:       "acme.recon.v1.Host",
		Body:       []byte("10.0.0.1"),
		ProducedBy: "recon",
	})

	// 3. ...and publishes an event. Coupling is only through contract refs.
	k.Publish(substrate.Event{
		Topic:   "recon.host.found",
		Type:    "acme.recon.v1.Host",
		Payload: []byte(host.ID),
		Source:  "recon",
	})

	// 4. Before anything irreversible, open a human-held gate and wait.
	ticket := k.RequestGate(substrate.GateRequest{
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
returns an unbounded `<-chan Event` (a background pump means the publisher never
blocks). The `Kernel` is safe for concurrent use across goroutines.

## Conformance

The six invariants from `SPEC.md` §Conformance are proven in
[`conformance_test.go`](conformance_test.go):

```sh
go test ./...
go vet ./...
```
