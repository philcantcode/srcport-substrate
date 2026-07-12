// The minimal conformance suite from SPEC.md §Conformance. An SDK is
// conformant iff all six pass. Each test names the invariant it pins down.
package substrate

import (
	"bytes"
	"errors"
	"testing"
	"time"

	"google.golang.org/protobuf/proto"
)

func recv(t *testing.T, ch <-chan *Event, d time.Duration) (*Event, bool) {
	t.Helper()
	select {
	case e := <-ch:
		return e, true
	case <-time.After(d):
		return nil, false
	}
}

// 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
func TestAddressingIsContentDerivedAndMetamorphic(t *testing.T) {
	k := NewKernel()

	a := k.PutArtifact(&Artifact{Type: "acme.recon.v1.Host", Body: []byte("10.0.0.1"), ProducedBy: "recon"})
	b := k.PutArtifact(&Artifact{Type: "acme.recon.v1.Host", Body: []byte("10.0.0.1"), ProducedBy: "someone-else"})
	if a.Id != b.Id {
		t.Fatalf("same (type, body) must yield the same id: %s != %s", a.Id, b.Id)
	}
	if len(a.Id) < 7 || a.Id[:7] != "sha256:" {
		t.Fatalf("id must be sha256-prefixed, got %s", a.Id)
	}

	c := k.PutArtifact(&Artifact{Type: "acme.recon.v1.Host", Body: []byte("10.0.0.2")})
	if a.Id == c.Id {
		t.Fatal("a one-byte change must change the address")
	}
	d := k.PutArtifact(&Artifact{Type: "acme.recon.v1.Port", Body: []byte("10.0.0.1")})
	if a.Id == d.Id {
		t.Fatal("type must participate in the address")
	}
	if a.Id != ArtifactID("acme.recon.v1.Host", []byte("10.0.0.1")) {
		t.Fatal("pure function must agree with the kernel")
	}
}

// 2. IMMUTABILITY — reads back byte-identical; a later put never mutates it.
func TestArtifactsAreImmutable(t *testing.T) {
	k := NewKernel()

	r := k.PutArtifact(&Artifact{Type: "t", Body: []byte("payload"), Meta: map[string]string{"first": "true"}})
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
	r2 := k.PutArtifact(&Artifact{Type: "t", Body: []byte("payload"), Meta: map[string]string{"first": "false", "sneaky": "yes"}})
	if r2.Id != r.Id {
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
	hosts := k.Subscribe(&Subscription{Module: "a", Topics: []string{"recon.host.found"}})
	ports := k.Subscribe(&Subscription{Module: "b", Topics: []string{"recon.port.found"}})

	s1 := k.Publish(&Event{Topic: "recon.host.found", Payload: []byte("h1")}).Seq
	s2 := k.Publish(&Event{Topic: "recon.host.found", Payload: []byte("h2")}).Seq
	s3 := k.Publish(&Event{Topic: "recon.port.found", Payload: []byte("p1")}).Seq

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
	k.Register(&ModuleManifest{Name: "m", Version: "0.1.0"})
	k.PutArtifact(&Artifact{Type: "t", Body: []byte("x")})
	k.Append(&AppendRequest{Kind: "domain.fact", Subject: "s", Detail: []byte("d")})

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

	full := k.Ledger()
	spliced := append(full[:1], full[2:]...)
	if VerifyChain(spliced) {
		t.Fatal("removing an entry must break the chain")
	}
}

// 5. GATE NON-BYPASS — blocked while PENDING/REJECTED; permitted only APPROVED.
func TestGatesAreNonBypassable(t *testing.T) {
	k := NewKernel()

	tkt := k.RequestGate(&GateRequest{Action: "delete production", RequestedBy: "danger"})
	var blocked *GateBlockedError
	if err := k.EnsureApproved(tkt); !errors.As(err, &blocked) || blocked.Decision != DecisionPending {
		t.Fatalf("must be blocked while PENDING, got %v", err)
	}

	if _, err := k.DecideGate(&GateDecision{RequestId: tkt.RequestId, Decision: DecisionRejected, DecidedBy: "phil", Reason: "no"}); err != nil {
		t.Fatal(err)
	}
	if err := k.EnsureApproved(tkt); !errors.As(err, &blocked) || blocked.Decision != DecisionRejected {
		t.Fatalf("REJECTED must block, got %v", err)
	}

	t2 := k.RequestGate(&GateRequest{Action: "delete production"})
	if err := k.EnsureApproved(t2); err == nil {
		t.Fatal("fresh gate must block")
	}
	if _, err := k.DecideGate(&GateDecision{RequestId: t2.RequestId, Decision: DecisionApproved, DecidedBy: "phil"}); err != nil {
		t.Fatal(err)
	}
	if err := k.EnsureApproved(t2); err != nil {
		t.Fatalf("APPROVED must permit the act, got %v", err)
	}

	// A non-decision is rejected at the ABI.
	if _, err := k.DecideGate(&GateDecision{RequestId: t2.RequestId, Decision: DecisionPending}); !errors.Is(err, ErrNotADecision) {
		t.Fatalf("PENDING is not a decision, got %v", err)
	}
}

// 5b. AwaitGate really blocks until a human decides.
func TestAwaitGateBlocksUntilDecided(t *testing.T) {
	k := NewKernel()
	tkt := k.RequestGate(&GateRequest{Action: "irreversible"})

	go func() {
		k.DecideGate(&GateDecision{RequestId: tkt.RequestId, Decision: DecisionApproved, DecidedBy: "phil"})
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
	k.Register(&ModuleManifest{
		Name: "recon", Version: "0.1.0",
		Provides: []*Capability{{Name: "recon.scan", Contract: "acme.recon.v1.ScanRequest"}},
	})
	k.Register(&ModuleManifest{
		Name: "report", Version: "0.2.0",
		Provides: []*Capability{{Name: "report.render", Contract: "acme.report.v1.Report"}},
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

func hasModule(ms []*ModuleManifest, name string) bool {
	for _, m := range ms {
		if m.Name == name {
			return true
		}
	}
	return false
}
func hasCap(cs []*Capability, name string) bool {
	for _, c := range cs {
		if c.Name == name {
			return true
		}
	}
	return false
}
func hasContract(cs []*Contract, ref string) bool {
	for _, c := range cs {
		if c.Ref == ref {
			return true
		}
	}
	return false
}

func findKind(chain []*LedgerEntry, kind string) *LedgerEntry {
	for _, e := range chain {
		if e.Kind == kind {
			return e
		}
	}
	return nil
}

func indexKind(chain []*LedgerEntry, kind string) int {
	for i, e := range chain {
		if e.Kind == kind {
			return i
		}
	}
	return -1
}

// 7. LEDGER RECONSTRUCTION & CANONICAL DETAIL — a state-bearing entry's Detail
//    decodes to the message named for its Kind and reproduces the original
//    value, so the registry, the artifact store, and the approval record all
//    round-trip from the tamper-evident chain alone. Detail is folded into the
//    entry hash, so forging it breaks verification.
func TestLedgerReconstructsStateFromDetail(t *testing.T) {
	k := NewKernel()

	r := k.PutArtifact(&Artifact{
		Type:        "acme.recon.v1.Host",
		Body:        []byte("10.0.0.1"),
		Meta:        map[string]string{"region": "eu", "scan": "full"},
		ProducedBy:  "recon",
		DerivedFrom: []string{"sha256:parent-a", "sha256:parent-b"},
	})
	k.Register(&ModuleManifest{
		Name: "recon", Version: "0.1.0",
		Provides: []*Capability{{Name: "recon.scan", Contract: "acme.recon.v1.ScanRequest"}},
		Requires: []string{"report.render"},
	})
	tkt := k.RequestGate(&GateRequest{Action: "delete production", Context: []byte("rows=42"), RequestedBy: "danger"})
	if _, err := k.DecideGate(&GateDecision{RequestId: tkt.RequestId, Decision: DecisionApproved, DecidedBy: "phil", Reason: "reviewed"}); err != nil {
		t.Fatal(err)
	}

	chain := k.Ledger()

	// artifact.put reconstructs everything but the body; lineage rides along.
	var a Artifact
	aEntry := findKind(chain, "artifact.put")
	if err := proto.Unmarshal(aEntry.Detail, &a); err != nil {
		t.Fatal(err)
	}
	if a.Id != r.Id || aEntry.Subject != r.Id {
		t.Fatal("subject and detail id must both be the content address")
	}
	if a.Type != "acme.recon.v1.Host" || a.ProducedBy != "recon" {
		t.Fatal("type and producer must round-trip")
	}
	if a.Meta["region"] != "eu" || a.Meta["scan"] != "full" {
		t.Fatal("meta must round-trip")
	}
	if len(a.DerivedFrom) != 2 || a.DerivedFrom[0] != "sha256:parent-a" || a.DerivedFrom[1] != "sha256:parent-b" {
		t.Fatal("derived_from lineage must round-trip through the ledger")
	}
	if len(a.Body) != 0 {
		t.Fatal("body must be cleared — the id in subject already addresses it")
	}

	// module.registered reconstructs the whole manifest.
	var m ModuleManifest
	if err := proto.Unmarshal(findKind(chain, "module.registered").Detail, &m); err != nil {
		t.Fatal(err)
	}
	if m.Name != "recon" || m.Version != "0.1.0" || len(m.Provides) != 1 || m.Provides[0].Name != "recon.scan" {
		t.Fatal("manifest name/version/provides must round-trip")
	}
	if len(m.Requires) != 1 || m.Requires[0] != "report.render" {
		t.Fatal("requires must round-trip")
	}

	// gate.requested / gate.decided reconstruct who / what / why.
	var req GateRequest
	if err := proto.Unmarshal(findKind(chain, "gate.requested").Detail, &req); err != nil {
		t.Fatal(err)
	}
	if req.Action != "delete production" || req.RequestedBy != "danger" || !bytes.Equal(req.Context, []byte("rows=42")) {
		t.Fatal("gate request must round-trip from the chain")
	}
	var dec GateDecision
	if err := proto.Unmarshal(findKind(chain, "gate.decided").Detail, &dec); err != nil {
		t.Fatal(err)
	}
	if dec.Decision != DecisionApproved || dec.DecidedBy != "phil" || dec.Reason != "reviewed" {
		t.Fatal("gate decision must round-trip from the chain")
	}

	if !k.VerifyLedger() {
		t.Fatal("the chain with fat detail must verify")
	}

	// The approval record is hash-committed: forging who approved it (re-encoding
	// a different decider into Detail) must break verification.
	forged := k.Ledger()
	i := indexKind(forged, "gate.decided")
	forged[i].Detail = marshalCanonical(&GateDecision{RequestId: tkt.RequestId, Decision: DecisionApproved, DecidedBy: "attacker", Reason: "reviewed"})
	if VerifyChain(forged) {
		t.Fatal("rewriting the recorded decision must break the chain")
	}
}

// 7c. CANONICAL DETAIL — the same logical value encodes to identical bytes every
//     time, so ledger detail hashes reproducibly across runs and SDKs. Go map
//     iteration is randomized; this pins that the kernel's canonical marshal
//     (Deterministic: sorted keys) defeats it.
func TestLedgerDetailEncodesCanonically(t *testing.T) {
	build := func() []byte {
		return marshalCanonical(&Artifact{
			Type: "t",
			Meta: map[string]string{"z": "1", "a": "2", "m": "3", "b": "4"},
		})
	}
	for i := 0; i < 64; i++ {
		if !bytes.Equal(build(), build()) {
			t.Fatal("identical meta must encode to identical bytes (deterministic, sorted keys)")
		}
	}
}

// METAMORPHIC — the address depends ONLY on (type, body). Transforming fields
// that aren't identity (meta, produced_by, derived_from) is a known no-op: the
// id must not move. If it did, the address would be keyed on provenance, not
// content — exactly the overfit a metamorphic test exists to catch.
func TestAddressIgnoresNonIdentityFields(t *testing.T) {
	k := NewKernel()
	base := k.PutArtifact(&Artifact{Type: "acme.recon.v1.Host", Body: []byte("10.0.0.1")})

	enriched := k.PutArtifact(&Artifact{
		Type: "acme.recon.v1.Host", Body: []byte("10.0.0.1"),
		Meta:        map[string]string{"x": "y"},
		ProducedBy:  "whoever",
		DerivedFrom: []string{"sha256:some-parent"},
	})
	if enriched.Id != base.Id {
		t.Fatal("meta, produced_by, and derived_from must not participate in the address")
	}
	if enriched.Id != ArtifactID("acme.recon.v1.Host", []byte("10.0.0.1")) {
		t.Fatal("address must equal the pure (type, body) function regardless of provenance")
	}
}

// CROSS-SDK KNOWN ANSWER — a fixed scenario must produce this exact final ledger
// hash in every SDK. Go, Rust, and Python all assert the SAME constant, so any
// drift in canonical detail encoding or the hash rule fails here and the three
// chains are pinned to cross-verify. If this constant ever changes, it changes
// in all three suites in lockstep — never one SDK alone.
func TestLedgerHashKnownAnswerCrossSDK(t *testing.T) {
	const want = "985f4980bda5266d03b3e7092ef2bd9eb49b12107b43f17bbe00415deca4ab6a"

	k := NewKernel()
	k.Register(&ModuleManifest{
		Name: "recon", Version: "0.1.0",
		Provides: []*Capability{{Name: "recon.scan", Contract: "acme.recon.v1.ScanRequest"}},
		Requires: []string{"report.render"},
	})
	k.PutArtifact(&Artifact{
		Type: "acme.recon.v1.Host", Body: []byte("10.0.0.1"),
		Meta: map[string]string{"region": "eu", "scan": "full"}, ProducedBy: "recon",
		DerivedFrom: []string{"sha256:parent-a", "sha256:parent-b"},
	})
	tkt := k.RequestGate(&GateRequest{Action: "delete production", Context: []byte("rows=42"), RequestedBy: "danger"})
	if _, err := k.DecideGate(&GateDecision{RequestId: tkt.RequestId, Decision: DecisionApproved, DecidedBy: "phil", Reason: "reviewed"}); err != nil {
		t.Fatal(err)
	}

	chain := k.Ledger()
	if !k.VerifyLedger() {
		t.Fatal("the chain must verify")
	}
	if got := chain[len(chain)-1].Hash; got != want {
		t.Fatalf("cross-SDK ledger hash drift:\n got  %s\n want %s", got, want)
	}
}
