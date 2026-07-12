// The minimal conformance suite from SPEC.md §Conformance. An SDK is
// conformant iff all six pass. Each test names the invariant it pins down.
package substrate

import (
	"bytes"
	"errors"
	"testing"
	"time"
)

func recv(t *testing.T, ch <-chan Event, d time.Duration) (Event, bool) {
	t.Helper()
	select {
	case e := <-ch:
		return e, true
	case <-time.After(d):
		return Event{}, false
	}
}

// 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
func TestAddressingIsContentDerivedAndMetamorphic(t *testing.T) {
	k := NewKernel()

	a := k.PutArtifact(Artifact{Type: "acme.recon.v1.Host", Body: []byte("10.0.0.1"), ProducedBy: "recon"})
	b := k.PutArtifact(Artifact{Type: "acme.recon.v1.Host", Body: []byte("10.0.0.1"), ProducedBy: "someone-else"})
	if a.ID != b.ID {
		t.Fatalf("same (type, body) must yield the same id: %s != %s", a.ID, b.ID)
	}
	if len(a.ID) < 7 || a.ID[:7] != "sha256:" {
		t.Fatalf("id must be sha256-prefixed, got %s", a.ID)
	}

	c := k.PutArtifact(Artifact{Type: "acme.recon.v1.Host", Body: []byte("10.0.0.2")})
	if a.ID == c.ID {
		t.Fatal("a one-byte change must change the address")
	}
	d := k.PutArtifact(Artifact{Type: "acme.recon.v1.Port", Body: []byte("10.0.0.1")})
	if a.ID == d.ID {
		t.Fatal("type must participate in the address")
	}
	if a.ID != ArtifactID("acme.recon.v1.Host", []byte("10.0.0.1")) {
		t.Fatal("pure function must agree with the kernel")
	}
}

// 2. IMMUTABILITY — reads back byte-identical; a later put never mutates it.
func TestArtifactsAreImmutable(t *testing.T) {
	k := NewKernel()

	r := k.PutArtifact(Artifact{Type: "t", Body: []byte("payload"), Meta: map[string]string{"first": "true"}})
	got, err := k.GetArtifact(r)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got.Body, []byte("payload")) {
		t.Fatal("must read back byte-identical")
	}
	if got.Meta["first"] != "true" {
		t.Fatal("meta must round-trip")
	}

	// A later put of the same content with different meta must NOT change what
	// is stored. First write wins.
	r2 := k.PutArtifact(Artifact{Type: "t", Body: []byte("payload"), Meta: map[string]string{"first": "false", "sneaky": "yes"}})
	if r2.ID != r.ID {
		t.Fatal("same content ⇒ same id")
	}
	after, _ := k.GetArtifact(r)
	if after.Meta["first"] != "true" {
		t.Fatal("stored value was mutated by a later put")
	}
	if _, ok := after.Meta["sneaky"]; ok {
		t.Fatal("stored value was mutated by a later put")
	}
}

// 3. ORDERING & ISOLATION — events reach exactly their subscribers, in Seq
//    order, and never reach non-subscribers.
func TestEventsAreOrderedAndIsolated(t *testing.T) {
	k := NewKernel()
	hosts := k.Subscribe(Subscription{Module: "a", Topics: []string{"recon.host.found"}})
	ports := k.Subscribe(Subscription{Module: "b", Topics: []string{"recon.port.found"}})

	s1 := k.Publish(Event{Topic: "recon.host.found", Payload: []byte("h1")}).Seq
	s2 := k.Publish(Event{Topic: "recon.host.found", Payload: []byte("h2")}).Seq
	s3 := k.Publish(Event{Topic: "recon.port.found", Payload: []byte("p1")}).Seq

	if !(s1 < s2 && s2 < s3) {
		t.Fatalf("seq must be monotonic across topics: %d %d %d", s1, s2, s3)
	}

	e1, ok1 := recv(t, hosts, time.Second)
	e2, ok2 := recv(t, hosts, time.Second)
	if !ok1 || !ok2 {
		t.Fatal("subscriber A must receive both host events")
	}
	if e1.Seq != s1 || !bytes.Equal(e1.Payload, []byte("h1")) || e2.Seq != s2 || !bytes.Equal(e2.Payload, []byte("h2")) {
		t.Fatal("A must receive its events in seq order")
	}
	if _, extra := recv(t, hosts, 50*time.Millisecond); extra {
		t.Fatal("A must never receive the port event")
	}

	p, ok := recv(t, ports, time.Second)
	if !ok || p.Seq != s3 {
		t.Fatal("subscriber B must receive the one port event")
	}
	if _, extra := recv(t, ports, 50*time.Millisecond); extra {
		t.Fatal("B must never receive the host events")
	}
}

// 4. LEDGER INTEGRITY — the chain verifies; tampering breaks verification.
func TestLedgerIsTamperEvident(t *testing.T) {
	k := NewKernel()
	k.Register(ModuleManifest{Name: "m", Version: "0.1.0"})
	k.PutArtifact(Artifact{Type: "t", Body: []byte("x")})
	k.Append(AppendRequest{Kind: "domain.fact", Subject: "s", Detail: []byte("d")})

	chain := k.Ledger()
	if len(chain) < 3 {
		t.Fatalf("expected >= 3 entries, got %d", len(chain))
	}
	if !k.VerifyLedger() || !VerifyChain(chain) {
		t.Fatal("the chain must verify")
	}

	tampered := k.Ledger()
	tampered[1].Subject = "hacked"
	if VerifyChain(tampered) {
		t.Fatal("tampering must break verification")
	}

	spliced := append(k.Ledger()[:1], k.Ledger()[2:]...)
	if VerifyChain(spliced) {
		t.Fatal("removing an entry must break the chain")
	}
}

// 5. GATE NON-BYPASS — blocked while PENDING/REJECTED; permitted only APPROVED.
func TestGatesAreNonBypassable(t *testing.T) {
	k := NewKernel()

	tkt := k.RequestGate(GateRequest{Action: "delete production", RequestedBy: "danger"})
	var blocked *GateBlockedError
	if err := k.EnsureApproved(tkt); !errors.As(err, &blocked) || blocked.Decision != DecisionPending {
		t.Fatalf("must be blocked while PENDING, got %v", err)
	}

	if _, err := k.DecideGate(GateDecision{RequestID: tkt.RequestID, Decision: DecisionRejected, DecidedBy: "phil", Reason: "no"}); err != nil {
		t.Fatal(err)
	}
	if err := k.EnsureApproved(tkt); !errors.As(err, &blocked) || blocked.Decision != DecisionRejected {
		t.Fatalf("REJECTED must block, got %v", err)
	}

	t2 := k.RequestGate(GateRequest{Action: "delete production"})
	if err := k.EnsureApproved(t2); err == nil {
		t.Fatal("fresh gate must block")
	}
	if _, err := k.DecideGate(GateDecision{RequestID: t2.RequestID, Decision: DecisionApproved, DecidedBy: "phil"}); err != nil {
		t.Fatal(err)
	}
	if err := k.EnsureApproved(t2); err != nil {
		t.Fatalf("APPROVED must permit the act, got %v", err)
	}

	// A non-decision is rejected at the ABI.
	if _, err := k.DecideGate(GateDecision{RequestID: t2.RequestID, Decision: DecisionPending}); !errors.Is(err, ErrNotADecision) {
		t.Fatalf("PENDING is not a decision, got %v", err)
	}
}

// 5b. AwaitGate really blocks until a human decides.
func TestAwaitGateBlocksUntilDecided(t *testing.T) {
	k := NewKernel()
	tkt := k.RequestGate(GateRequest{Action: "irreversible"})

	go func() {
		k.DecideGate(GateDecision{RequestID: tkt.RequestID, Decision: DecisionApproved, DecidedBy: "phil"})
	}()

	d, err := k.AwaitGate(tkt)
	if err != nil {
		t.Fatal(err)
	}
	if d.Decision != DecisionApproved {
		t.Fatalf("expected APPROVED, got %s", d.Decision)
	}
}

// 6. DISCOVERY — the registry reports every module, capability, and contract.
func TestRegistryReportsEverything(t *testing.T) {
	k := NewKernel()
	k.Register(ModuleManifest{
		Name: "recon", Version: "0.1.0",
		Provides: []Capability{{Name: "recon.scan", Contract: "acme.recon.v1.ScanRequest"}},
	})
	k.Register(ModuleManifest{
		Name: "report", Version: "0.2.0",
		Provides: []Capability{{Name: "report.render", Contract: "acme.report.v1.Report"}},
		Requires: []string{"recon.scan"},
	})

	snap := k.Snapshot()
	if !hasModule(snap.Modules, "recon") || !hasModule(snap.Modules, "report") {
		t.Fatal("registry must report every module")
	}
	if !hasCap(snap.Capabilities, "recon.scan") || !hasCap(snap.Capabilities, "report.render") {
		t.Fatal("registry must report every capability")
	}
	if !hasContract(snap.Contracts, "acme.recon.v1.ScanRequest") || !hasContract(snap.Contracts, "acme.report.v1.Report") {
		t.Fatal("registry must report every contract")
	}
}

func hasModule(ms []ModuleManifest, name string) bool {
	for _, m := range ms {
		if m.Name == name {
			return true
		}
	}
	return false
}
func hasCap(cs []Capability, name string) bool {
	for _, c := range cs {
		if c.Name == name {
			return true
		}
	}
	return false
}
func hasContract(cs []Contract, ref string) bool {
	for _, c := range cs {
		if c.Ref == ref {
			return true
		}
	}
	return false
}
