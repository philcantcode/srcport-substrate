// The minimal conformance suite from SPEC.md §Conformance. An SDK is
// conformant iff all invariants pass. Each test names the invariant it pins down.
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

func mustPut(t *testing.T, k *MemoryKernel, a *Artifact) *ArtifactRef {
	t.Helper()
	r, err := k.PutArtifact(a)
	if err != nil {
		t.Fatal(err)
	}
	return r
}

// claimOne returns the first claimed item, or an empty WorkItem when none.
func claimOne(t *testing.T, k *MemoryKernel, runID, module string) *WorkItem {
	t.Helper()
	resp, err := k.ClaimReady(&ClaimRequest{RunId: runID, Module: module})
	if err != nil {
		t.Fatal(err)
	}
	if len(resp.Items) == 0 {
		return &WorkItem{}
	}
	return resp.Items[0]
}

// 1. ADDRESSING — same (type, body) ⇒ same id; a one-byte change ⇒ a new id.
func TestAddressingIsContentDerivedAndMetamorphic(t *testing.T) {
	k := NewMemoryKernel()

	a := mustPut(t, k, func() *Artifact { a := ArtifactWithTrait("acme.recon.v1.Host", []byte("10.0.0.1")); a.ProducedBy = "recon"; return a }())
	b := mustPut(t, k, func() *Artifact { a := ArtifactWithTrait("acme.recon.v1.Host", []byte("10.0.0.1")); a.ProducedBy = "someone-else"; return a }())
	if a.Id != b.Id {
		t.Fatalf("same (type, body) must yield the same id: %s != %s", a.Id, b.Id)
	}
	if len(a.Id) < 7 || a.Id[:7] != "sha256:" {
		t.Fatalf("id must be sha256-prefixed, got %s", a.Id)
	}

	c := mustPut(t, k, ArtifactWithTrait("acme.recon.v1.Host", []byte("10.0.0.2")))
	if a.Id == c.Id {
		t.Fatal("a one-byte change must change the address")
	}
	d := mustPut(t, k, ArtifactWithTrait("acme.recon.v1.Port", []byte("10.0.0.1")))
	if a.Id == d.Id {
		t.Fatal("type must participate in the address")
	}
	if a.Id != ArtifactIDSingle("acme.recon.v1.Host", []byte("10.0.0.1")) {
		t.Fatal("pure function must agree with the kernel")
	}
}

// 2. IMMUTABILITY — reads back byte-identical; a later put never mutates it.
func TestArtifactsAreImmutable(t *testing.T) {
	k := NewMemoryKernel()

	r := mustPut(t, k, func() *Artifact { a := ArtifactWithTrait("t", []byte("payload")); a.Meta = map[string]string{"first": "true"}; return a }())
	got, err := k.GetArtifact(r)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(GetTrait(got, "t").Body, []byte("payload")) {
		t.Fatal("must read back byte-identical")
	}
	if got.Meta["first"] != "true" {
		t.Fatal("meta must round-trip")
	}

	// A later put of the same content with different meta must NOT change what
	// is stored. First write wins.
	r2 := mustPut(t, k, func() *Artifact { a := ArtifactWithTrait("t", []byte("payload")); a.Meta = map[string]string{"first": "false", "sneaky": "yes"}; return a }())
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

//  3. ORDERING & ISOLATION — events reach exactly their subscribers, in Seq
//     order, and never reach non-subscribers.
func TestEventsAreOrderedAndIsolated(t *testing.T) {
	k := NewMemoryKernel()
	hosts := k.Subscribe(&Subscription{Module: "a", Topics: []string{"recon.host.found"}})
	ports := k.Subscribe(&Subscription{Module: "b", Topics: []string{"recon.port.found"}})

	h1 := mustPut(t, k, ArtifactWithTrait("acme.recon.v1.Host", []byte("h1")))
	h2 := mustPut(t, k, ArtifactWithTrait("acme.recon.v1.Host", []byte("h2")))
	p1 := mustPut(t, k, ArtifactWithTrait("acme.recon.v1.Port", []byte("p1")))
	s1 := k.Publish(&Event{Topic: "recon.host.found", Type: "acme.recon.v1.Host", Artifacts: []*ArtifactRef{h1}}).Seq
	s2 := k.Publish(&Event{Topic: "recon.host.found", Type: "acme.recon.v1.Host", Artifacts: []*ArtifactRef{h2}}).Seq
	s3 := k.Publish(&Event{Topic: "recon.port.found", Type: "acme.recon.v1.Port", Artifacts: []*ArtifactRef{p1}}).Seq

	if !(s1 < s2 && s2 < s3) {
		t.Fatalf("seq must be monotonic across topics: %d %d %d", s1, s2, s3)
	}

	e1, ok1 := recv(t, hosts, time.Second)
	e2, ok2 := recv(t, hosts, time.Second)
	if !ok1 || !ok2 {
		t.Fatal("subscriber A must receive both host events")
	}
	if e1.Seq != s1 || len(e1.Artifacts) != 1 || e1.Artifacts[0].Id != h1.Id ||
		e2.Seq != s2 || len(e2.Artifacts) != 1 || e2.Artifacts[0].Id != h2.Id {
		t.Fatal("A must receive its events in seq order with artifact refs")
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
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{Name: "m", Version: "0.1.0"})
	mustPut(t, k, ArtifactWithTrait("t", []byte("x")))
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

// 6. DISCOVERY — the registry reports every module, capability, and contract.
func TestRegistryReportsEverything(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{
		Name: "recon", Version: "0.1.0",
		Provides: []*Capability{{Name: "recon.scan", Outputs: []*Port{{Name: "out", Traits: []string{"acme.recon.v1.Host"}}}}},
	})
	k.Register(&ModuleManifest{
		Name: "report", Version: "0.2.0",
		Provides: []*Capability{{Name: "report.render", Outputs: []*Port{{Name: "out", Traits: []string{"acme.report.v1.Report"}}}}},
		Requires: []string{"recon.scan"},
	})

	snap := k.Snapshot()
	if !hasModule(snap.Modules, "recon") || !hasModule(snap.Modules, "report") {
		t.Fatal("registry must report every module")
	}
	if !hasCap(snap.Capabilities, "recon.scan") || !hasCap(snap.Capabilities, "report.render") {
		t.Fatal("registry must report every capability")
	}
	if !hasContract(snap.Contracts, "acme.recon.v1.Host") || !hasContract(snap.Contracts, "acme.report.v1.Report") {
		t.Fatal("registry must report every contract")
	}
}

// 6b. CONTRACT IDENTITY — content-addressed under ref; immutable; conflict on
// redefinition; placeholder fill-once; ports bind to the pinned identity.
func TestContractsAreImmutableAndIdentifiable(t *testing.T) {
	k := NewMemoryKernel()

	stored, err := k.PutContract(&Contract{
		Ref: "acme.Host", MediaType: "application/schema+json",
		Schema: `{"type":"object"}`, Version: "1.0.0",
		CompatibleWith: []string{"acme.Host.v0", "acme.legacy.Host"},
	})
	if err != nil {
		t.Fatal(err)
	}
	wantDigest := ContractDigest("application/schema+json", `{"type":"object"}`, "1.0.0",
		[]string{"acme.Host.v0", "acme.legacy.Host"})
	if stored.Digest != wantDigest {
		t.Fatalf("digest:\n got  %s\n want %s", stored.Digest, wantDigest)
	}
	// compatible_with is stored sorted UTF-8 ascending.
	if len(stored.CompatibleWith) != 2 || stored.CompatibleWith[0] != "acme.Host.v0" {
		t.Fatalf("compatible_with not normalized: %v", stored.CompatibleWith)
	}

	// Identical re-put is a no-op (idempotent).
	again, err := k.PutContract(&Contract{
		Ref: "acme.Host", MediaType: "application/schema+json",
		Schema: `{"type":"object"}`, Version: "1.0.0",
		CompatibleWith: []string{"acme.legacy.Host", "acme.Host.v0"}, // unsorted ok
	})
	if err != nil {
		t.Fatal(err)
	}
	if again.Digest != stored.Digest {
		t.Fatal("identical re-put must return the same identity")
	}

	// Different content under the same ref is CONFLICT.
	_, err = k.PutContract(&Contract{
		Ref: "acme.Host", MediaType: "application/schema+json",
		Schema: `{"type":"string"}`, Version: "1.0.0",
	})
	if err == nil || !errors.Is(err, ErrConflict) {
		t.Fatalf("expected ErrConflict on redefinition, got %v", err)
	}

	// Register creates a name-only placeholder; PutContract may fill it once.
	k.Register(&ModuleManifest{
		Name: "mod", Version: "1",
		Provides: []*Capability{{Name: "do", Outputs: []*Port{{Name: "out", Traits: []string{"acme.NewThing"}}}}},
	})
	filled, err := k.PutContract(&Contract{
		Ref: "acme.NewThing", MediaType: "text/x-protobuf",
		Schema: "message NewThing {}", Version: "1",
	})
	if err != nil {
		t.Fatal(err)
	}
	if filled.Digest == "" || IsContractPlaceholder(filled) {
		t.Fatal("placeholder must fill to real content")
	}
	// Second fill with different content conflicts.
	_, err = k.PutContract(&Contract{
		Ref: "acme.NewThing", MediaType: "text/x-protobuf",
		Schema: "message Other {}", Version: "1",
	})
	if err == nil || !errors.Is(err, ErrConflict) {
		t.Fatalf("expected ErrConflict after fill, got %v", err)
	}

	// Mismatched caller-supplied digest is INVALID.
	_, err = k.PutContract(&Contract{
		Ref: "acme.Other", Schema: "x", Digest: "sha256:deadbeef",
	})
	if err == nil || !errors.Is(err, ErrInvalid) {
		t.Fatalf("expected ErrInvalid on digest mismatch, got %v", err)
	}

	// contract.registered lands in the ledger with reconstructable detail.
	chain := k.Ledger()
	found := false
	for _, e := range chain {
		if e.Kind == "contract.registered" && e.Subject == "acme.Host" {
			var c Contract
			if err := proto.Unmarshal(e.Detail, &c); err != nil {
				t.Fatal(err)
			}
			if c.Ref != "acme.Host" || c.Digest != wantDigest {
				t.Fatalf("ledger detail mismatch: %+v", &c)
			}
			found = true
		}
	}
	if !found {
		t.Fatal("contract.registered must appear in the ledger")
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

func TestRunFeedsForwardAndClosesOnTerminalAnswer(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{Name: "extractor", Version: "1.0.0", Provides: []*Capability{{Name: "facts.extract", Inputs: []*Port{{Name: "question", Traits: []string{"demo.Question"}}}, Outputs: []*Port{{Name: "facts", Traits: []string{"demo.Facts"}}}}}})
	k.Register(&ModuleManifest{Name: "writer", Version: "2.0.0", Provides: []*Capability{{Name: "answer.write", Inputs: []*Port{{Name: "question", Traits: []string{"demo.Question"}}, {Name: "facts", Traits: []string{"demo.Facts"}}}, Outputs: []*Port{{Name: "answer", Traits: []string{"demo.Answer"}}}}}})
	question := mustPut(t, k, ArtifactWithTrait("demo.Question", []byte("What follows?")))
	assembly := &Assembly{
		Id:       "answer-pipeline@1",
		Nodes:    []*AssemblyNode{{Id: "extract", Module: "extractor", ModuleVersion: "1.0.0", Capability: "facts.extract"}, {Id: "write", Module: "writer", ModuleVersion: "2.0.0", Capability: "answer.write"}},
		Bindings: []*Binding{{ToNode: "extract", ToPort: "question", Input: "question"}, {ToNode: "write", ToPort: "question", Input: "question"}, {ToNode: "write", ToPort: "facts", FromNode: "extract", FromPort: "facts"}},
		Terminal: &NodeOutput{Node: "write", Port: "answer"},
	}
	run, err := k.StartRun(&RunRequest{Id: "run-1", Assembly: assembly, Inputs: []*NamedArtifact{{Name: "question", Artifact: question}}})
	if err != nil || run.State != RunStateRunning {
		t.Fatalf("start: run=%v err=%v", run, err)
	}
	if work, err := k.ClaimReady(&ClaimRequest{RunId: "run-1", Module: "writer"}); err != nil || len(work.Items) != 0 {
		t.Fatalf("writer must wait for fan-in: work=%v err=%v", work, err)
	}
	extractResp, err := k.ClaimReady(&ClaimRequest{RunId: "run-1", Module: "extractor"})
	if err != nil || len(extractResp.Items) == 0 || extractResp.Items[0].Id == "" {
		t.Fatalf("extract claim: %v %v", extractResp, err)
	}
	extract := extractResp.Items[0]
	facts := mustPut(t, k, ArtifactWithTrait("demo.Facts", []byte("typed flow")))
	if _, err := k.Commit(&Derivation{RunId: "run-1", WorkId: extract.Id, NodeId: extract.NodeId, Outputs: []*NamedArtifact{{Name: "facts", Artifact: facts}}}); err != nil {
		t.Fatal(err)
	}
	writeResp, err := k.ClaimReady(&ClaimRequest{RunId: "run-1", Module: "writer"})
	if err != nil || len(writeResp.Items) == 0 || len(writeResp.Items[0].Inputs) != 2 {
		t.Fatalf("fan-in: work=%v err=%v", writeResp, err)
	}
	write := writeResp.Items[0]
	answer := mustPut(t, k, ArtifactWithTrait("demo.Answer", []byte("Modules converge.")))
	completed, err := k.Commit(&Derivation{RunId: "run-1", WorkId: write.Id, NodeId: write.NodeId, Outputs: []*NamedArtifact{{Name: "answer", Artifact: answer}}})
	if err != nil {
		t.Fatal(err)
	}
	if completed.State != RunStateCompleted || completed.Answer.Id != answer.Id {
		t.Fatalf("run did not converge: %v", completed)
	}
	paths, err := k.ListDerivations(&RunRef{Id: "run-1"})
	if err != nil || len(paths.Derivations) != 2 {
		t.Fatal("each production path must remain observable")
	}
	if _, err := k.ClaimReady(&ClaimRequest{RunId: "run-1", Module: "writer"}); err == nil {
		t.Fatal("completed run was reopened")
	}
}

func TestCyclicAssemblyIsRejected(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{Name: "loop", Version: "1.0.0", Provides: []*Capability{{Name: "loop.step", Inputs: []*Port{{Name: "in", Traits: []string{"demo.Value"}, Optional: true}}, Outputs: []*Port{{Name: "out", Traits: []string{"demo.Value"}}}}}})
	_, err := k.StartRun(&RunRequest{Id: "cycle", Assembly: &Assembly{
		Id:       "cycle@1",
		Nodes:    []*AssemblyNode{{Id: "a", Module: "loop", ModuleVersion: "1.0.0", Capability: "loop.step"}, {Id: "b", Module: "loop", ModuleVersion: "1.0.0", Capability: "loop.step"}},
		Bindings: []*Binding{{ToNode: "a", ToPort: "in", FromNode: "b", FromPort: "out"}, {ToNode: "b", ToPort: "in", FromNode: "a", FromPort: "out"}}, Terminal: &NodeOutput{Node: "b", Port: "out"},
	}})
	if !errors.Is(err, ErrInvalid) {
		t.Fatalf("cycle must be invalid, got %v", err)
	}
}

func TestRunStallsWhenNoNodeCanBecomeReady(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{Name: "source", Version: "1", Provides: []*Capability{{Name: "source.maybe", Outputs: []*Port{{Name: "value", Traits: []string{"demo.Value"}, Optional: true}}}}})
	k.Register(&ModuleManifest{Name: "sink", Version: "1", Provides: []*Capability{{Name: "sink.answer", Inputs: []*Port{{Name: "value", Traits: []string{"demo.Value"}}}, Outputs: []*Port{{Name: "answer", Traits: []string{"demo.Answer"}}}}}})
	_, err := k.StartRun(&RunRequest{Id: "stall", Assembly: &Assembly{Id: "stall@1", Nodes: []*AssemblyNode{{Id: "source", Module: "source", ModuleVersion: "1", Capability: "source.maybe"}, {Id: "sink", Module: "sink", ModuleVersion: "1", Capability: "sink.answer"}}, Bindings: []*Binding{{ToNode: "sink", ToPort: "value", FromNode: "source", FromPort: "value"}}, Terminal: &NodeOutput{Node: "sink", Port: "answer"}}})
	if err != nil {
		t.Fatal(err)
	}
	work := claimOne(t, k, "stall", "source")
	run, err := k.Commit(&Derivation{RunId: "stall", WorkId: work.Id, NodeId: work.NodeId})
	if err != nil || run.State != RunStateStalled {
		t.Fatalf("run must stall: run=%v err=%v", run, err)
	}
}

func TestConvergentRunHashesMatchEverySDK(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{Name: "answerer", Version: "1.0.0", Provides: []*Capability{{Name: "answer.write", Outputs: []*Port{{Name: "answer", Traits: []string{"demo.Answer"}}}}}})
	_, err := k.StartRun(&RunRequest{Id: "parity", Assembly: &Assembly{Id: "single@1", Nodes: []*AssemblyNode{{Id: "answer", Module: "answerer", ModuleVersion: "1.0.0", Capability: "answer.write"}}, Terminal: &NodeOutput{Node: "answer", Port: "answer"}}})
	if err != nil {
		t.Fatal(err)
	}
	work := claimOne(t, k, "parity", "answerer")
	answer := mustPut(t, k, ArtifactWithTrait("demo.Answer", []byte("yes")))
	if _, err := k.Commit(&Derivation{RunId: "parity", WorkId: work.Id, NodeId: work.NodeId, Outputs: []*NamedArtifact{{Name: "answer", Artifact: answer}}}); err != nil {
		t.Fatal(err)
	}
	if got := k.Derivations()[0].Id; got != "sha256:8f7f99a396dbf79c7f2287d2f9fca7f4167343831a9283cdfbeb2fe010c8414c" {
		t.Fatalf("derivation parity: %s", got)
	}
	const LEDGER = "faa944642933bb3b1b2d3789fb940a3ed8eb9802d06bf17444677f72fe974335"
	if got := k.Ledger()[len(k.Ledger())-1].Hash; got != LEDGER {
		t.Fatalf("ledger parity: %s", got)
	}
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

//  7. LEDGER RECONSTRUCTION & CANONICAL DETAIL — a state-bearing entry's Detail
//     decodes to the message named for its Kind and reproduces the original
//     value, so the registry, the artifact store, and the approval record all
//     round-trip from the tamper-evident chain alone. Detail is folded into the
//     entry hash, so forging it breaks verification.
func TestLedgerReconstructsStateFromDetail(t *testing.T) {
	k := NewMemoryKernel()

	r := mustPut(t, k, func() *Artifact { a := ArtifactWithTrait("acme.recon.v1.Host", []byte("10.0.0.1")); a.Meta = map[string]string{"region": "eu", "scan": "full"}; a.ProducedBy = "recon"; return a }())
	k.Register(&ModuleManifest{
		Name: "recon", Version: "0.1.0",
		Provides: []*Capability{{Name: "recon.scan", Outputs: []*Port{{Name: "out", Traits: []string{"acme.recon.v1.Host"}}}}},
		Requires: []string{"report.render"},
	})
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
	if firstTraitKey(&a) != "acme.recon.v1.Host" || a.ProducedBy != "recon" {
		t.Fatal("type and producer must round-trip")
	}
	if a.Meta["region"] != "eu" || a.Meta["scan"] != "full" {
		t.Fatal("meta must round-trip")
	}
	if string(firstTraitBody(&a)) != "10.0.0.1" {
		t.Fatal("inline trait bodies remain in the ledger")
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


	if !k.VerifyLedger() {
		t.Fatal("the chain with fat detail must verify")
	}

}


// 7c. CANONICAL DETAIL — the same logical value encodes to identical bytes every
//
//	time, so ledger detail hashes reproducibly across runs and SDKs. Go map
//	iteration is randomized; this pins that the kernel's canonical marshal
//	(Deterministic: sorted keys) defeats it.
func TestLedgerDetailEncodesCanonically(t *testing.T) {
	build := func() []byte {
		return marshalCanonical(func() *Artifact { a := ArtifactWithTrait("t", nil); a.Meta = map[string]string{"z": "1", "a": "2", "m": "3", "b": "4"}; return a }())
	}
	for i := 0; i < 64; i++ {
		if !bytes.Equal(build(), build()) {
			t.Fatal("identical meta must encode to identical bytes (deterministic, sorted keys)")
		}
	}
}

// METAMORPHIC — the address depends ONLY on (type, body). Transforming fields
// that aren't identity (meta, produced_by) is a known no-op: the
// id must not move. If it did, the address would be keyed on provenance, not
// content — exactly the overfit a metamorphic test exists to catch.
func TestAddressIgnoresNonIdentityFields(t *testing.T) {
	k := NewMemoryKernel()
	base := mustPut(t, k, ArtifactWithTrait("acme.recon.v1.Host", []byte("10.0.0.1")))

	enriched := mustPut(t, k, func() *Artifact { a := ArtifactWithTrait("acme.recon.v1.Host", []byte("10.0.0.1")); a.Meta = map[string]string{"x": "y"}; a.ProducedBy = "whoever"; return a }())
	if enriched.Id != base.Id {
		t.Fatal("meta and produced_by must not participate in the address")
	}
	if enriched.Id != ArtifactIDSingle("acme.recon.v1.Host", []byte("10.0.0.1")) {
		t.Fatal("address must equal the pure (type, body) function regardless of provenance")
	}
}

// CROSS-SDK KNOWN ANSWER — a fixed scenario must produce this exact final ledger
// hash in every SDK. Go, Rust, and Python all assert the SAME constant, so any
// drift in canonical detail encoding or the hash rule fails here and the three
// chains are pinned to cross-verify. If this constant ever changes, it changes
// in all three suites in lockstep — never one SDK alone.
func TestLedgerHashKnownAnswerCrossSDK(t *testing.T) {
	const want = "3f0957aaae7a7a939dc3b5dba74145b03af065e3f04ce302ef602bc01424f350"

	k := NewMemoryKernel()
	k.Register(&ModuleManifest{
		Name: "recon", Version: "0.1.0",
		Provides: []*Capability{{Name: "recon.scan", Outputs: []*Port{{Name: "host", Traits: []string{"acme.recon.v1.Host"}}}}},
		Requires: []string{"report.render"},
	})
	mustPut(t, k, func() *Artifact { a := ArtifactWithTrait("acme.recon.v1.Host", []byte("10.0.0.1")); a.Meta = map[string]string{"region": "eu", "scan": "full"}; a.ProducedBy = "recon"; return a }())
	chain := k.Ledger()
	if !k.VerifyLedger() {
		t.Fatal("the chain must verify")
	}
	if got := chain[len(chain)-1].Hash; got != want {
		t.Fatalf("cross-SDK ledger hash drift:\n got  %s\n want %s", got, want)
	}
}

// 12. PRODUCTION ARTIFACT BOUNDARY — small values inline; large values are
// verified ObjectRefs into the blob store. Typed value identity ≠ blob identity.
func TestBlobStoreIsContentAddressedAndImmutable(t *testing.T) {
	k := NewMemoryKernel()
	data := []byte("pcap-or-apk-bytes-go-here")

	a, err := k.PutBlob(&PutBlobRequest{Namespace: "evidence", Data: data})
	if err != nil {
		t.Fatal(err)
	}
	if a.Digest != BlobID(data) {
		t.Fatalf("blob digest must be pure content id: %s", a.Digest)
	}
	if a.ByteCount != uint64(len(data)) || a.Namespace != "evidence" {
		t.Fatal("blob ref must carry size and namespace")
	}

	// First write wins: a later put of the same content is a no-op.
	b, err := k.PutBlob(&PutBlobRequest{Namespace: "evidence", Data: data})
	if err != nil {
		t.Fatal(err)
	}
	if a.Digest != b.Digest {
		t.Fatal("same bytes ⇒ same blob digest")
	}

	got, err := k.GetBlob(&GetBlobRequest{Digest: a.Digest, Namespace: "evidence"})
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got.Data, data) {
		t.Fatal("GetBlob must read back byte-identical")
	}

	has := k.HasBlob(&HasBlobRequest{Digest: a.Digest, Namespace: "evidence"})
	if !has.Exists || has.ByteCount != uint64(len(data)) {
		t.Fatal("HasBlob must report the stored blob")
	}
	if k.HasBlob(&HasBlobRequest{Digest: a.Digest, Namespace: "other"}).Exists {
		t.Fatal("namespace isolates blob storage")
	}

	// Ledger records blob.put without data bytes.
	entry := findKind(k.Ledger(), "blob.put")
	if entry == nil || entry.Subject != a.Digest {
		t.Fatal("blob.put must land with digest as subject")
	}
	var ref BlobRef
	if err := proto.Unmarshal(entry.Detail, &ref); err != nil {
		t.Fatal(err)
	}
	if ref.Digest != a.Digest || ref.ByteCount != a.ByteCount || ref.Namespace != "evidence" {
		t.Fatal("blob.put detail must be the BlobRef metadata")
	}
}

func TestExternalArtifactRefsLargeDataWithoutInlining(t *testing.T) {
	k := NewMemoryKernel()
	// Multi-chunk "evidence" that should never sit in Artifact.body.
	payload := bytes.Repeat([]byte("EVIDENCE-BUNDLE-"), 64*1024) // 1 MiB-ish

	blob, err := k.PutBlob(&PutBlobRequest{Namespace: "observer", Data: payload})
	if err != nil {
		t.Fatal(err)
	}
	ref, err := k.PutArtifact(func() *Artifact { a := ArtifactWithExternalTrait("observer.v1.Capture", &ObjectRef{
			Digest:    blob.Digest,
			ByteCount: blob.ByteCount,
			Namespace: blob.Namespace,
		}); a.ProducedBy = "observer"; return a }())
	if err != nil {
		t.Fatal(err)
	}

	// Typed value identity hashes object_ref_bytes, not the PCAP bytes.
	wantID := ArtifactIDSingle("observer.v1.Capture", ObjectRefBytes(&ObjectRef{
		Digest: blob.Digest, ByteCount: blob.ByteCount, Namespace: blob.Namespace,
	}))
	if ref.Id != wantID {
		t.Fatalf("external artifact id mismatch:\n got  %s\n want %s", ref.Id, wantID)
	}
	if ref.Id == BlobID(payload) {
		t.Fatal("typed value identity must not equal raw blob identity")
	}

	got, err := k.GetArtifact(ref)
	if err != nil {
		t.Fatal(err)
	}
	f := GetTrait(got, "observer.v1.Capture")
	if f == nil || len(f.Body) != 0 {
		t.Fatal("external artifact must not inline blob bytes in body")
	}
	if f.Object == nil || f.Object.Digest != blob.Digest || f.Object.ByteCount != blob.ByteCount {
		t.Fatal("external artifact must retain verified ObjectRef")
	}

	// Same ObjectRef ⇒ same artifact id (convergence).
	ref2, err := k.PutArtifact(ArtifactWithExternalTrait("observer.v1.Capture", &ObjectRef{
			Digest: blob.Digest, ByteCount: blob.ByteCount, Namespace: blob.Namespace,
		}))
	if err != nil || ref2.Id != ref.Id {
		t.Fatalf("equal external values must converge: %v %v", ref2, err)
	}

	// Convenience path: PutArtifactWithBlob.
	ref3, blob3, err := k.PutArtifactWithBlob("observer.v1.Capture", "observer", payload, "observer")
	if err != nil {
		t.Fatal(err)
	}
	if ref3.Id != ref.Id || blob3.Digest != blob.Digest {
		t.Fatal("PutArtifactWithBlob must be equivalent to PutBlob + PutArtifact")
	}

	// Materialize bytes only when needed.
	data, err := k.GetBlob(&GetBlobRequest{Digest: f.Object.Digest, Namespace: f.Object.Namespace})
	if err != nil || !bytes.Equal(data.Data, payload) {
		t.Fatal("blob must stream back the full evidence without living in the artifact")
	}

	// Ledger artifact.put keeps ObjectRef, clears body.
	var a Artifact
	if err := proto.Unmarshal(findKind(k.Ledger(), "artifact.put").Detail, &a); err != nil {
		t.Fatal(err)
	}
	lf := GetTrait(&a, "observer.v1.Capture")
	if lf == nil || len(lf.Body) != 0 || lf.Object == nil || lf.Object.Digest != blob.Digest {
		t.Fatal("ledger must retain ObjectRef and clear body")
	}
}

func TestExternalArtifactRejectsMissingOrMismatchedBlob(t *testing.T) {
	k := NewMemoryKernel()
	data := []byte("small-but-external")
	blob, err := k.PutBlob(&PutBlobRequest{Namespace: "ns", Data: data})
	if err != nil {
		t.Fatal(err)
	}

	// Missing blob.
	if _, err := k.PutArtifact(ArtifactWithExternalTrait("t", &ObjectRef{Digest: BlobID([]byte("nope")), ByteCount: 4, Namespace: "ns"})); !errors.Is(err, ErrNotFound) {
		t.Fatalf("missing blob must be NotFound, got %v", err)
	}

	// Wrong size.
	if _, err := k.PutArtifact(ArtifactWithExternalTrait("t", &ObjectRef{Digest: blob.Digest, ByteCount: blob.ByteCount + 1, Namespace: "ns"})); !errors.Is(err, ErrBlobIntegrity) {
		t.Fatalf("size mismatch must be integrity error, got %v", err)
	}

	// Both body and object.
	if _, err := k.PutArtifact(func() *Artifact { a := ArtifactWithTrait("t", []byte("x")); a.Traits["t"].Object = &ObjectRef{Digest: blob.Digest, ByteCount: blob.ByteCount, Namespace: "ns"}; return a }()); !errors.Is(err, ErrInvalid) {
		t.Fatalf("body+object must be invalid, got %v", err)
	}

	// Wrong namespace.
	if _, err := k.PutArtifact(ArtifactWithExternalTrait("t", &ObjectRef{Digest: blob.Digest, ByteCount: blob.ByteCount, Namespace: "other"})); !errors.Is(err, ErrNotFound) {
		t.Fatalf("wrong namespace must be NotFound, got %v", err)
	}
}

func TestValueIdentityIndependentOfBlobIdentity(t *testing.T) {
	k := NewMemoryKernel()
	data := []byte("shared-raw-bytes")
	blob, err := k.PutBlob(&PutBlobRequest{Namespace: "a", Data: data})
	if err != nil {
		t.Fatal(err)
	}
	// Same bytes, different namespace ⇒ different blob slot and different ObjectRef.
	blobB, err := k.PutBlob(&PutBlobRequest{Namespace: "b", Data: data})
	if err != nil {
		t.Fatal(err)
	}
	if blob.Digest != blobB.Digest {
		t.Fatal("blob identity is pure content — digest must match across namespaces")
	}

	artA := mustPut(t, k, ArtifactWithExternalTrait("t", &ObjectRef{Digest: blob.Digest, ByteCount: blob.ByteCount, Namespace: "a"}))
	artB := mustPut(t, k, ArtifactWithExternalTrait("t", &ObjectRef{Digest: blobB.Digest, ByteCount: blobB.ByteCount, Namespace: "b"}))
	if artA.Id == artB.Id {
		t.Fatal("namespace is part of ObjectRef value identity")
	}

	// Different type, same object ref ⇒ different artifact id.
	artC := mustPut(t, k, ArtifactWithExternalTrait("other", &ObjectRef{Digest: blob.Digest, ByteCount: blob.ByteCount, Namespace: "a"}))
	if artC.Id == artA.Id {
		t.Fatal("type participates in typed value identity")
	}
}

func TestRequestContextDeadlineAndIdempotency(t *testing.T) {
	k := NewMemoryKernel()
	past := &RequestContext{DeadlineUnixMs: 1}
	if _, err := k.PutArtifact(ArtifactWithTrait("t", []byte("x")), past); err == nil || !errors.Is(err, ErrFailedPrecondition) {
		t.Fatalf("past deadline must fail: %v", err)
	}
	ctx := &RequestContext{Caller: "worker", RequestKey: "put-once"}
	a, err := k.PutArtifact(ArtifactWithTrait("t", []byte("unique-body")), ctx)
	if err != nil {
		t.Fatal(err)
	}
	before := len(k.Ledger())
	b, err := k.PutArtifact(ArtifactWithTrait("t", []byte("unique-body")), ctx)
	if err != nil {
		t.Fatal(err)
	}
	if a.Id != b.Id {
		t.Fatal("idempotent put must return same ref")
	}
	if len(k.Ledger()) != before {
		t.Fatal("idempotent replay must not append ledger entries")
	}
}

func TestTransitionIsOnTheABI(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{Name: "m", Version: "1"})
	ack, err := k.Transition(&TransitionRequest{Module: "m", To: LifecycleLoaded})
	if err != nil {
		t.Fatal(err)
	}
	if ack.State != LifecycleLoaded {
		t.Fatalf("want LOADED, got %v", ack.State)
	}
	found := false
	for _, e := range k.Ledger() {
		if e.Kind == "module.loaded" {
			found = true
		}
	}
	if !found {
		t.Fatal("module.loaded must land in the ledger")
	}
}

func TestStorePolicyDefaultsAndSnapshot(t *testing.T) {
	k := NewMemoryKernel()
	p := k.StorePolicy()
	if p.MaxInlineBytes != MaxInlineArtifactBytes {
		t.Fatalf("default max_inline: got %d", p.MaxInlineBytes)
	}
	if p.MaxBlobBytes != 0 {
		t.Fatal("default max_blob must be unlimited (0)")
	}
	if p.IngestMode != BlobIngestModeCopyVerify {
		t.Fatal("default ingest is COPY_VERIFY")
	}
	if p.Durability != StoreDurabilityEphemeral {
		t.Fatal("MemoryKernel durability is EPHEMERAL")
	}
	snap := k.Snapshot()
	if snap.StorePolicy == nil || snap.StorePolicy.MaxInlineBytes != p.MaxInlineBytes {
		t.Fatal("snapshot must expose store_policy")
	}
}

func TestStorePolicyRejectsOversizedInlineBody(t *testing.T) {
	k, err := NewMemoryKernelWithStorePolicy(&ArtifactStorePolicy{
		MaxInlineBytes: 8,
		IngestMode:     BlobIngestModeCopyVerify,
		Durability:     StoreDurabilityEphemeral,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := k.PutArtifact(ArtifactWithTrait("t", []byte("ok-small"))); err != nil {
		t.Fatal(err)
	}
	if _, err := k.PutArtifact(ArtifactWithTrait("t", []byte("too-large!"))); !errors.Is(err, ErrResourceExhausted) {
		t.Fatalf("want resource exhausted, got %v", err)
	}
}

func TestStorePolicyRejectsOversizedBlob(t *testing.T) {
	k, err := NewMemoryKernelWithStorePolicy(&ArtifactStorePolicy{
		MaxInlineBytes: MaxInlineArtifactBytes,
		MaxBlobBytes:   16,
		IngestMode:     BlobIngestModeCopyVerify,
		Durability:     StoreDurabilityEphemeral,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := k.PutBlob(&PutBlobRequest{Namespace: "n", Data: []byte("fifteen-bytes!")}); err != nil {
		t.Fatal(err)
	}
	if _, err := k.PutBlob(&PutBlobRequest{Namespace: "n", Data: []byte("seventeen-bytes!!!")}); !errors.Is(err, ErrResourceExhausted) {
		t.Fatalf("want resource exhausted, got %v", err)
	}
}

func firstTraitKey(a *Artifact) string {
	for k := range a.Traits {
		return k
	}
	return ""
}
func firstTraitBody(a *Artifact) []byte {
	for _, f := range a.Traits {
		if f != nil {
			return f.Body
		}
	}
	return nil
}
func traitBodyLen(a *Artifact) int { return len(firstTraitBody(a)) }


// 12b. LEASED CONCURRENCY — batch claim, max_in_flight, fail/retry, lease reclaim.

func TestBatchClaimAndMaxInFlight(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{
		Name: "worker", Version: "1",
		Provides: []*Capability{{
			Name: "work.item", Firing: FiringOncePerKey,
			Inputs:  []*Port{{Name: "key", Traits: []string{"demo.Key"}, Key: true}},
			Outputs: []*Port{{Name: "out", Traits: []string{"demo.Out"}}},
		}},
	})
	a := mustPut(t, k, ArtifactWithTrait("demo.Key", []byte("a")))
	b := mustPut(t, k, ArtifactWithTrait("demo.Key", []byte("b")))
	_, err := k.StartRun(&RunRequest{
		Id: "batch",
		Assembly: &Assembly{
			Id: "batch@1",
			Nodes: []*AssemblyNode{
				{Id: "w1", Module: "worker", ModuleVersion: "1", Capability: "work.item"},
				{Id: "w2", Module: "worker", ModuleVersion: "1", Capability: "work.item"},
			},
			Bindings: []*Binding{
				{ToNode: "w1", ToPort: "key", Input: "k1"},
				{ToNode: "w2", ToPort: "key", Input: "k2"},
			},
			Terminal: &NodeOutput{Node: "w1", Port: "out"},
		},
		Inputs: []*NamedArtifact{
			{Name: "k1", Artifact: a},
			{Name: "k2", Artifact: b},
		},
		Limits: &Limits{MaxSteps: 10, MaxInFlight: 1},
		Policy: &ExecutionPolicy{Closure: ClosureOpen},
	})
	if err != nil {
		t.Fatal(err)
	}

	first, err := k.ClaimReady(&ClaimRequest{RunId: "batch", MaxItems: 2})
	if err != nil {
		t.Fatal(err)
	}
	if len(first.Items) != 1 {
		t.Fatalf("max_in_flight=1, got %d items", len(first.Items))
	}
	if first.Items[0].Attempt != 1 {
		t.Fatalf("attempt: %d", first.Items[0].Attempt)
	}
	if first.Items[0].UnitKey == "" {
		t.Fatal("unit_key required")
	}
	if first.Items[0].LeaseUntilUnixMs <= 0 {
		t.Fatal("lease_until required")
	}

	blocked, err := k.ClaimReady(&ClaimRequest{RunId: "batch", MaxItems: 2})
	if err != nil {
		t.Fatal(err)
	}
	if len(blocked.Items) != 0 {
		t.Fatal("at capacity")
	}

	out := mustPut(t, k, ArtifactWithTrait("demo.Out", []byte("1")))
	if _, err := k.Commit(&Derivation{
		RunId: "batch", WorkId: first.Items[0].Id, NodeId: first.Items[0].NodeId,
		Outputs: []*NamedArtifact{{Name: "out", Artifact: out}},
	}); err != nil {
		t.Fatal(err)
	}

	second, err := k.ClaimReady(&ClaimRequest{RunId: "batch", MaxItems: 2})
	if err != nil {
		t.Fatal(err)
	}
	if len(second.Items) != 1 {
		t.Fatalf("expected second claim, got %d", len(second.Items))
	}
}

func TestFailWorkRetriesThenTerminals(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{
		Name: "flaky", Version: "1",
		Provides: []*Capability{{
			Name: "flaky.run",
			Outputs: []*Port{{Name: "out", Traits: []string{"demo.Out"}}},
		}},
	})
	if _, err := k.StartRun(&RunRequest{
		Id: "fail",
		Assembly: &Assembly{
			Id:       "fail@1",
			Nodes:    []*AssemblyNode{{Id: "n", Module: "flaky", ModuleVersion: "1", Capability: "flaky.run"}},
			Terminal: &NodeOutput{Node: "n", Port: "out"},
		},
		Limits: &Limits{MaxSteps: 10, MaxAttempts: 2},
	}); err != nil {
		t.Fatal(err)
	}

	w1 := claimOne(t, k, "fail", "flaky")
	if w1.Attempt != 1 {
		t.Fatalf("attempt: %d", w1.Attempt)
	}
	if _, err := k.FailWork(&FailWorkRequest{RunId: "fail", WorkId: w1.Id, Reason: "boom", Terminal: false}); err != nil {
		t.Fatal(err)
	}

	w2 := claimOne(t, k, "fail", "flaky")
	if w2.Attempt != 2 {
		t.Fatalf("attempt: %d", w2.Attempt)
	}
	if w2.Id != w1.Id {
		t.Fatalf("reclaim must reuse work id: %s vs %s", w2.Id, w1.Id)
	}
	stalled, err := k.FailWork(&FailWorkRequest{RunId: "fail", WorkId: w2.Id, Reason: "boom again", Terminal: false})
	if err != nil {
		t.Fatal(err)
	}
	// attempts exhausted → DONE; no READY/CLAIMED → STALLED under FIRST_TERMINAL.
	if stalled.State != RunStateStalled {
		t.Fatalf("want STALLED, got %v", stalled.State)
	}
	if _, err := k.ClaimReady(&ClaimRequest{RunId: "fail", Module: "flaky"}); err == nil {
		t.Fatal("stalled run must reject claim")
	} else {
		var closed *RunClosedError
		if !errors.As(err, &closed) || closed.State != RunStateStalled {
			t.Fatalf("want RunClosed STALLED, got %v", err)
		}
	}
}

func TestLeaseExpiryReturnsUnitToReady(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{
		Name: "slow", Version: "1",
		Provides: []*Capability{{
			Name: "slow.run",
			Outputs: []*Port{{Name: "out", Traits: []string{"demo.Out"}}},
		}},
	})
	if _, err := k.StartRun(&RunRequest{
		Id: "lease",
		Assembly: &Assembly{
			Id:       "lease@1",
			Nodes:    []*AssemblyNode{{Id: "n", Module: "slow", ModuleVersion: "1", Capability: "slow.run"}},
			Terminal: &NodeOutput{Node: "n", Port: "out"},
		},
		Limits: &Limits{MaxSteps: 10, DefaultLeaseMs: 1, MaxAttempts: 3},
		Policy: &ExecutionPolicy{Closure: ClosureOpen},
	}); err != nil {
		t.Fatal(err)
	}

	w1 := claimOne(t, k, "lease", "slow")
	if w1.Id == "" {
		t.Fatal("expected claim")
	}
	if claimOne(t, k, "lease", "slow").Id != "" {
		t.Fatal("second claim while leased → empty")
	}
	time.Sleep(5 * time.Millisecond)
	w2 := claimOne(t, k, "lease", "slow")
	if w2.Id == "" {
		t.Fatal("reclaimed after lease expiry")
	}
	if w2.Attempt != 2 {
		t.Fatalf("attempt after reclaim: %d", w2.Attempt)
	}

	if _, err := k.Heartbeat(&HeartbeatRequest{
		RunId: "lease", WorkIds: []string{w2.Id}, ExtendLeaseMs: 60_000,
	}); err != nil {
		t.Fatal(err)
	}
	time.Sleep(5 * time.Millisecond)
	if claimOne(t, k, "lease", "slow").Id != "" {
		t.Fatal("still leased after heartbeat")
	}
}

func TestConcurrentClaimantsDoNotDoubleClaim(t *testing.T) {
	k := NewMemoryKernel()
	k.Register(&ModuleManifest{
		Name: "solo", Version: "1",
		Provides: []*Capability{{
			Name: "solo.run",
			Outputs: []*Port{{Name: "out", Traits: []string{"demo.Out"}}},
		}},
	})
	if _, err := k.StartRun(&RunRequest{
		Id: "race",
		Assembly: &Assembly{
			Id:       "race@1",
			Nodes:    []*AssemblyNode{{Id: "n", Module: "solo", ModuleVersion: "1", Capability: "solo.run"}},
			Terminal: &NodeOutput{Node: "n", Port: "out"},
		},
	}); err != nil {
		t.Fatal(err)
	}

	const n = 8
	results := make(chan int, n)
	for i := 0; i < n; i++ {
		go func() {
			resp, err := k.ClaimReady(&ClaimRequest{RunId: "race", Module: "solo", MaxItems: 1})
			if err != nil {
				results <- 0
				return
			}
			results <- len(resp.Items)
		}()
	}
	got := 0
	for i := 0; i < n; i++ {
		got += <-results
	}
	if got != 1 {
		t.Fatalf("exactly one claimant wins, got %d", got)
	}
}
