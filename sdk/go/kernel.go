package substrate

import (
	"crypto/sha256"
	"errors"
	"fmt"
	"sync"

	"google.golang.org/protobuf/proto"
)

// ─── errors ─────────────────────────────────────────────────────────────────

// ErrNotFound is returned when no artifact or gate exists for a given id.
var ErrNotFound = errors.New("not found")

// ErrNotADecision is returned when DecideGate is called with something other
// than APPROVED or REJECTED.
var ErrNotADecision = errors.New("gate decision must be APPROVED or REJECTED")

var ErrInvalid = errors.New("invalid")
var ErrConflict = errors.New("conflict")

type RunClosedError struct{ State RunState }

func (e *RunClosedError) Error() string { return fmt.Sprintf("run is closed: %s", e.State) }

// GateBlockedError is returned by EnsureApproved when the gate is not APPROVED.
// It carries the current Decision so callers can see why it blocked.
type GateBlockedError struct{ Decision Decision }

func (e *GateBlockedError) Error() string {
	return fmt.Sprintf("gate blocked: decision is %s, not APPROVED", e.Decision)
}

// ─── the Kernel ─────────────────────────────────────────────────────────────

type moduleSlot struct {
	manifest  *ModuleManifest
	lifecycle Lifecycle
}

type runSlot struct {
	run       *Run
	claimed   map[string]*WorkItem
	committed map[string]*Derivation
}

// Kernel is the in-process microkernel. Its methods mirror the service Kernel
// RPCs in substrate.proto one-for-one. It is safe for concurrent use; share one
// *Kernel across module goroutines. Every meaningful action lands one
// append-only ledger entry. Values handed in and out are cloned, so a caller
// can never mutate stored state through a shared pointer.
type Kernel struct {
	mu       sync.Mutex
	gateCond *sync.Cond

	modules      []moduleSlot
	capabilities []*Capability
	contracts    map[string]*Contract
	artifacts    map[string]*Artifact
	subs         []*subscriber
	ledger       []*LedgerEntry
	gates        map[string]*GateDecision
	runs         map[string]*runSlot
	derivations  []*Derivation
	eventSeq     uint64
	gateCounter  uint64
}

// NewKernel returns an empty, ready kernel.
func NewKernel() *Kernel {
	k := &Kernel{
		contracts: map[string]*Contract{},
		artifacts: map[string]*Artifact{},
		gates:     map[string]*GateDecision{},
		runs:      map[string]*runSlot{},
	}
	k.gateCond = sync.NewCond(&k.mu)
	return k
}

func clone[M proto.Message](m M) M { return proto.Clone(m).(M) }

// marshalCanonical encodes m with deterministic field and map ordering. Ledger
// detail is folded into the entry hash, so its encoding MUST be canonical — the
// same logical value has to yield byte-identical detail across SDKs and runs, or
// chains stop cross-verifying (see SPEC.md "Ledger detail"). A marshal error
// here means one of our own well-formed messages failed to encode: a bug, not a
// runtime condition, so we panic rather than widen every ABI method's signature.
func marshalCanonical(m proto.Message) []byte {
	b, err := proto.MarshalOptions{Deterministic: true}.Marshal(m)
	if err != nil {
		panic(fmt.Sprintf("substrate: canonical marshal of %T failed: %v", m, err))
	}
	return b
}

// appendLocked is the ONLY path that creates ledger entries. Caller holds mu.
func (k *Kernel) appendLocked(kind, subject string, detail []byte) *LedgerEntry {
	seq := uint64(len(k.ledger))
	prev := ""
	if n := len(k.ledger); n > 0 {
		prev = k.ledger[n-1].Hash
	}
	e := &LedgerEntry{
		Seq:      seq,
		Kind:     kind,
		Subject:  subject,
		Detail:   detail,
		PrevHash: prev,
		Hash:     ledgerHash(seq, kind, subject, detail, prev),
	}
	k.ledger = append(k.ledger, e)
	return clone(e)
}

// ─── 1. Module ────────────────────────────────────────────────────────────

// Register records a module, its capabilities, and (implicitly) the contracts
// those capabilities speak. The module lands in REGISTERED; advance it with
// Transition. Mirrors rpc Register.
func (k *Kernel) Register(m *ModuleManifest) *RegisterAck {
	m = clone(m)
	k.mu.Lock()
	defer k.mu.Unlock()
	for _, c := range m.Provides {
		k.capabilities = append(k.capabilities, clone(c))
		if _, ok := k.contracts[c.Contract]; !ok {
			k.contracts[c.Contract] = &Contract{Ref: c.Contract}
		}
		for _, p := range append(append([]*Port{}, c.Inputs...), c.Outputs...) {
			if _, ok := k.contracts[p.Contract]; !ok {
				k.contracts[p.Contract] = &Contract{Ref: p.Contract}
			}
		}
	}
	k.modules = append(k.modules, moduleSlot{manifest: m, lifecycle: LifecycleRegistered})
	// The full manifest lands in the tamper-evident chain, so the registry is
	// reconstructable from the ledger alone. detail is the canonical manifest.
	k.appendLocked("module.registered", m.Name, marshalCanonical(m))
	return &RegisterAck{State: LifecycleRegistered}
}

// Transition advances a module along REGISTERED → LOADED → ACTIVE →
// DEACTIVATED. Only forward moves are honoured; anything else is a no-op that
// returns the current state. Returns ErrNotFound if the module is unknown.
func (k *Kernel) Transition(module string, to Lifecycle) (Lifecycle, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	for i := range k.modules {
		if k.modules[i].manifest.Name == module {
			if to == k.modules[i].lifecycle+1 {
				k.modules[i].lifecycle = to
				k.appendLocked("module."+lifecycleVerb(to), module, nil)
			}
			return k.modules[i].lifecycle, nil
		}
	}
	return LifecycleUnspecified, fmt.Errorf("%w: %s", ErrNotFound, module)
}

// ─── 2. Artifact ──────────────────────────────────────────────────────────

// PutArtifact content-addresses the value, stores it immutably (first write
// wins — a later put of the same id never mutates what is stored), and returns
// its ref. Mirrors rpc PutArtifact.
func (k *Kernel) PutArtifact(a *Artifact) *ArtifactRef {
	id := ArtifactID(a.Type, a.Body)
	k.mu.Lock()
	defer k.mu.Unlock()
	if _, ok := k.artifacts[id]; !ok {
		stored := clone(a)
		stored.Id = id
		k.artifacts[id] = stored
		// The ledger commits to everything but the body: subject (the id) already
		// addresses (type, body), so re-inlining the body would duplicate the
		// store into a log it can never prune. detail is the canonical Artifact
		// with body cleared; meta, produced_by, and derived_from ride along.
		forLog := clone(stored)
		forLog.Body = nil
		k.appendLocked("artifact.put", id, marshalCanonical(forLog))
	}
	return &ArtifactRef{Id: id}
}

// GetArtifact reads an artifact back byte-identical. Mirrors rpc GetArtifact.
func (k *Kernel) GetArtifact(ref *ArtifactRef) (*Artifact, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	a, ok := k.artifacts[ref.Id]
	if !ok {
		return nil, fmt.Errorf("%w: %s", ErrNotFound, ref.Id)
	}
	return clone(a), nil
}

// ─── 3. Contract ──────────────────────────────────────────────────────────

// PutContract registers (or attaches schema text to) a contract explicitly.
// Capabilities already auto-register their contract ref via Register.
func (k *Kernel) PutContract(c *Contract) {
	k.mu.Lock()
	defer k.mu.Unlock()
	k.contracts[c.Ref] = clone(c)
}

// ─── 4. Event ─────────────────────────────────────────────────────────────

// Subscribe returns a channel of events on the given topics, in kernel Seq
// order. A subscriber only ever receives events on topics it named. The channel
// is unbounded (a background pump forwards from an internal queue), so the
// publisher is never blocked. Mirrors rpc Subscribe (stream Event).
func (k *Kernel) Subscribe(s *Subscription) <-chan *Event {
	sub := newSubscriber(s.Topics)
	k.mu.Lock()
	k.subs = append(k.subs, sub)
	k.mu.Unlock()
	return sub.out
}

// Publish assigns a monotonic Seq (the total order), delivers to exactly the
// subscribers of Event.Topic and never to anyone else, and returns the assigned
// Seq. Mirrors rpc Publish.
func (k *Kernel) Publish(e *Event) *PublishAck {
	k.mu.Lock()
	defer k.mu.Unlock()
	k.eventSeq++
	e = clone(e)
	e.Seq = k.eventSeq
	for _, sub := range k.subs {
		if sub.wants(e.Topic) {
			sub.enqueue(clone(e))
		}
	}
	forLog := clone(e)
	forLog.Payload = nil
	k.appendLocked("event.published", e.Topic, marshalCanonical(forLog))
	return &PublishAck{Seq: e.Seq}
}

// ─── 5. Ledger ────────────────────────────────────────────────────────────

// Append lets modules write their own domain facts into the same tamper-evident
// chain. Mirrors rpc Append.
func (k *Kernel) Append(r *AppendRequest) *LedgerEntry {
	k.mu.Lock()
	defer k.mu.Unlock()
	return k.appendLocked(r.Kind, r.Subject, r.Detail)
}

// Ledger returns a snapshot (deep copy) of the whole ledger, for
// verification/audit. Mutating the result never affects the kernel.
func (k *Kernel) Ledger() []*LedgerEntry {
	k.mu.Lock()
	defer k.mu.Unlock()
	out := make([]*LedgerEntry, len(k.ledger))
	for i, e := range k.ledger {
		out[i] = clone(e)
	}
	return out
}

// VerifyLedger verifies the kernel's own live ledger.
func (k *Kernel) VerifyLedger() bool {
	k.mu.Lock()
	defer k.mu.Unlock()
	return VerifyChain(k.ledger)
}

// ─── 6. Gate ──────────────────────────────────────────────────────────────

// RequestGate opens a human-held checkpoint in PENDING. The module must not
// proceed with the guarded action until a human decides. Mirrors rpc RequestGate.
func (k *Kernel) RequestGate(r *GateRequest) *GateTicket {
	k.mu.Lock()
	defer k.mu.Unlock()
	id := r.Id
	if id == "" {
		k.gateCounter++
		id = fmt.Sprintf("gate-%d", k.gateCounter)
	}
	k.gates[id] = &GateDecision{RequestId: id, Decision: DecisionPending}
	// The full request lands in the tamper-evident chain — action, requested_by,
	// and context are the evidence a human (or an auditor after a restart)
	// reconstructs. detail is the canonical GateRequest, with its assigned id.
	req := clone(r)
	req.Id = id
	k.appendLocked("gate.requested", id, marshalCanonical(req))
	return &GateTicket{RequestId: id}
}

// DecideGate records a human's APPROVED or REJECTED and wakes any AwaitGate
// waiters. Mirrors rpc DecideGate.
func (k *Kernel) DecideGate(d *GateDecision) (*GateDecision, error) {
	if d.Decision != DecisionApproved && d.Decision != DecisionRejected {
		return nil, ErrNotADecision
	}
	d = clone(d)
	k.mu.Lock()
	defer k.mu.Unlock()
	if _, ok := k.gates[d.RequestId]; !ok {
		return nil, fmt.Errorf("%w: %s", ErrNotFound, d.RequestId)
	}
	k.gates[d.RequestId] = d
	// The decision itself — who decided, what, and why — is hash-committed, so
	// the approval record can't be rewritten without breaking the chain.
	k.appendLocked("gate.decided", d.RequestId, marshalCanonical(d))
	k.gateCond.Broadcast()
	return clone(d), nil
}

// AwaitGate blocks until the gate is no longer PENDING, then returns the human's
// decision. Mirrors rpc AwaitGate.
func (k *Kernel) AwaitGate(t *GateTicket) (*GateDecision, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	for {
		g, ok := k.gates[t.RequestId]
		if !ok {
			return nil, fmt.Errorf("%w: %s", ErrNotFound, t.RequestId)
		}
		if g.Decision != DecisionPending {
			return clone(g), nil
		}
		k.gateCond.Wait()
	}
}

// GateStatus is a non-blocking peek at a gate's current decision.
func (k *Kernel) GateStatus(t *GateTicket) (Decision, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	g, ok := k.gates[t.RequestId]
	if !ok {
		return DecisionUnspecified, fmt.Errorf("%w: %s", ErrNotFound, t.RequestId)
	}
	return g.Decision, nil
}

// EnsureApproved is the non-bypass guard. It returns nil ONLY when the gate is
// APPROVED; PENDING and REJECTED both return *GateBlockedError. Call it
// immediately before an irreversible act.
func (k *Kernel) EnsureApproved(t *GateTicket) error {
	d, err := k.GateStatus(t)
	if err != nil {
		return err
	}
	if d != DecisionApproved {
		return &GateBlockedError{Decision: d}
	}
	return nil
}

// ─── 7. Registry ──────────────────────────────────────────────────────────

// Snapshot answers "what exists right now": every registered module,
// capability, and contract. Mirrors rpc Snapshot.
func (k *Kernel) Snapshot() *RegistrySnapshot {
	k.mu.Lock()
	defer k.mu.Unlock()
	snap := &RegistrySnapshot{
		Modules:      make([]*ModuleManifest, 0, len(k.modules)),
		Capabilities: make([]*Capability, 0, len(k.capabilities)),
	}
	for _, m := range k.modules {
		snap.Modules = append(snap.Modules, clone(m.manifest))
	}
	for _, c := range k.capabilities {
		snap.Capabilities = append(snap.Capabilities, clone(c))
	}
	for _, c := range k.contracts {
		snap.Contracts = append(snap.Contracts, clone(c))
	}
	return snap
}

// ─── 8. Run / convergence ────────────────────────────────────────────────────────────

// StartRun validates and freezes a finite feed-forward assembly. Mirrors rpc
// StartRun. No domain payload is interpreted; readiness follows typed bindings.
func (k *Kernel) StartRun(req *RunRequest) (*Run, error) {
	req = clone(req)
	k.mu.Lock()
	defer k.mu.Unlock()
	if req.Id == "" {
		return nil, fmt.Errorf("%w: run id is required", ErrInvalid)
	}
	if _, ok := k.runs[req.Id]; ok {
		return nil, fmt.Errorf("%w: run %s already exists", ErrConflict, req.Id)
	}
	if req.Assembly == nil {
		return nil, fmt.Errorf("%w: assembly is required", ErrInvalid)
	}
	if err := k.validateAssembly(req.Assembly, req.Inputs); err != nil {
		return nil, err
	}
	max := uint64(len(req.Assembly.Nodes))
	if req.Limits != nil && req.Limits.MaxSteps != 0 {
		max = req.Limits.MaxSteps
	}
	if max == 0 {
		return nil, fmt.Errorf("%w: max_steps must be positive", ErrInvalid)
	}
	run := &Run{Id: req.Id, Assembly: req.Assembly, Inputs: req.Inputs, State: RunStateRunning, MaxSteps: max}
	k.runs[run.Id] = &runSlot{run: run, claimed: map[string]*WorkItem{}, committed: map[string]*Derivation{}}
	k.appendLocked("run.started", run.Id, marshalCanonical(run))
	return clone(run), nil
}

// ClaimReady atomically claims one ready node for module. An empty WorkItem
// means this module has no ready node. If no work exists anywhere, the run is
// closed as STALLED.
func (k *Kernel) ClaimReady(req *ClaimRequest) (*WorkItem, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	slot, ok := k.runs[req.RunId]
	if !ok {
		return nil, fmt.Errorf("%w: %s", ErrNotFound, req.RunId)
	}
	if slot.run.State != RunStateRunning {
		return nil, &RunClosedError{State: slot.run.State}
	}
	var selected *AssemblyNode
	var selectedInputs []*NamedArtifact
	anyReady := false
	for _, node := range slot.run.Assembly.Nodes {
		if slot.claimed[node.Id] != nil || slot.committed[node.Id] != nil {
			continue
		}
		inputs, ready := resolveInputs(slot, node)
		if ready {
			anyReady = true
			if selected == nil && node.Module == req.Module {
				selected, selectedInputs = node, inputs
			}
		}
	}
	if selected != nil {
		work := &WorkItem{Id: "work:" + req.RunId + "/" + selected.Id, RunId: req.RunId, NodeId: selected.Id, Module: selected.Module, ModuleVersion: selected.ModuleVersion, Capability: selected.Capability, Inputs: selectedInputs}
		slot.claimed[selected.Id] = work
		k.appendLocked("work.claimed", work.Id, marshalCanonical(work))
		return clone(work), nil
	}
	if !anyReady && len(slot.claimed) == 0 {
		slot.run.State = RunStateStalled
		slot.run.Reason = "no node is ready and no work is in flight"
		k.appendLocked("run.stalled", slot.run.Id, marshalCanonical(slot.run))
	}
	return &WorkItem{}, nil
}

// Commit validates and records one production path, then releases downstream
// nodes or closes the run when the declared terminal artifact appears.
func (k *Kernel) Commit(submitted *Derivation) (*Run, error) {
	submitted = clone(submitted)
	k.mu.Lock()
	defer k.mu.Unlock()
	slot, ok := k.runs[submitted.RunId]
	if !ok {
		return nil, fmt.Errorf("%w: %s", ErrNotFound, submitted.RunId)
	}
	if slot.run.State != RunStateRunning {
		return nil, &RunClosedError{State: slot.run.State}
	}
	work := slot.claimed[submitted.NodeId]
	if work == nil {
		return nil, fmt.Errorf("%w: node was not claimed", ErrConflict)
	}
	if submitted.WorkId != work.Id {
		return nil, fmt.Errorf("%w: work_id does not match the claim", ErrInvalid)
	}
	cap, err := k.capabilityFor(work.Module, work.ModuleVersion, work.Capability)
	if err != nil {
		return nil, err
	}
	if err := k.validateOutputs(cap, submitted.Outputs); err != nil {
		return nil, err
	}
	d := &Derivation{RunId: work.RunId, WorkId: work.Id, NodeId: work.NodeId, Module: work.Module, ModuleVersion: work.ModuleVersion, Capability: work.Capability, Inputs: cloneNamed(work.Inputs), Outputs: submitted.Outputs}
	d.Id = derivationID(d)
	delete(slot.claimed, work.NodeId)
	slot.committed[work.NodeId] = d
	slot.run.Steps++
	closed := false
	terminal := slot.run.Assembly.Terminal
	if terminal.Node == work.NodeId {
		for _, output := range d.Outputs {
			if output.Name == terminal.Port {
				slot.run.Answer = clone(output.Artifact)
				slot.run.State = RunStateCompleted
				closed = true
				break
			}
		}
	}
	if !closed && slot.run.Steps >= slot.run.MaxSteps {
		slot.run.State = RunStateFailed
		slot.run.Reason = "max_steps exhausted before the terminal output"
		closed = true
	}
	if !closed {
		anyReady := false
		for _, node := range slot.run.Assembly.Nodes {
			if slot.claimed[node.Id] == nil && slot.committed[node.Id] == nil {
				if _, ready := resolveInputs(slot, node); ready {
					anyReady = true
					break
				}
			}
		}
		if !anyReady && len(slot.claimed) == 0 {
			slot.run.State = RunStateStalled
			slot.run.Reason = "no node is ready and no work is in flight"
			closed = true
		}
	}
	k.derivations = append(k.derivations, clone(d))
	k.appendLocked("derivation.committed", d.Id, marshalCanonical(d))
	kind := "run.progressed"
	if slot.run.State == RunStateCompleted {
		kind = "run.completed"
	} else if slot.run.State == RunStateStalled {
		kind = "run.stalled"
	} else if closed {
		kind = "run.failed"
	}
	k.appendLocked(kind, slot.run.Id, marshalCanonical(slot.run))
	return clone(slot.run), nil
}

func (k *Kernel) GetRun(ref *RunRef) (*Run, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	if slot := k.runs[ref.Id]; slot != nil {
		return clone(slot.run), nil
	}
	return nil, fmt.Errorf("%w: %s", ErrNotFound, ref.Id)
}

func (k *Kernel) CancelRun(ref *RunRef) (*Run, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	slot := k.runs[ref.Id]
	if slot == nil {
		return nil, fmt.Errorf("%w: %s", ErrNotFound, ref.Id)
	}
	if slot.run.State != RunStateRunning {
		return nil, &RunClosedError{State: slot.run.State}
	}
	slot.run.State = RunStateCancelled
	slot.run.Reason = "cancelled"
	slot.claimed = map[string]*WorkItem{}
	k.appendLocked("run.cancelled", slot.run.Id, marshalCanonical(slot.run))
	return clone(slot.run), nil
}

func (k *Kernel) Derivations() []*Derivation {
	k.mu.Lock()
	defer k.mu.Unlock()
	out := make([]*Derivation, len(k.derivations))
	for i, d := range k.derivations {
		out[i] = clone(d)
	}
	return out
}

func (k *Kernel) ListDerivations(ref *RunRef) (*DerivationList, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	if k.runs[ref.Id] == nil {
		return nil, fmt.Errorf("%w: %s", ErrNotFound, ref.Id)
	}
	out := &DerivationList{}
	for _, d := range k.derivations {
		if d.RunId == ref.Id {
			out.Derivations = append(out.Derivations, clone(d))
		}
	}
	return out, nil
}

func cloneNamed(values []*NamedArtifact) []*NamedArtifact {
	out := make([]*NamedArtifact, len(values))
	for i, value := range values {
		out[i] = clone(value)
	}
	return out
}

func (k *Kernel) capabilityFor(module, version, capability string) (*Capability, error) {
	var found *Capability
	for _, m := range k.modules {
		if m.manifest.Name == module && m.manifest.Version == version {
			for _, c := range m.manifest.Provides {
				if c.Name == capability {
					if found != nil {
						return nil, fmt.Errorf("%w: %s@%s provides %s ambiguously", ErrInvalid, module, version, capability)
					}
					found = c
				}
			}
		}
	}
	if found != nil {
		return found, nil
	}
	return nil, fmt.Errorf("%w: %s@%s does not provide %s", ErrInvalid, module, version, capability)
}

func findPort(ports []*Port, name string) *Port {
	for _, p := range ports {
		if p.Name == name {
			return p
		}
	}
	return nil
}

func (k *Kernel) validateAssembly(a *Assembly, inputs []*NamedArtifact) error {
	if a.Id == "" {
		return fmt.Errorf("%w: assembly id is required", ErrInvalid)
	}
	if len(a.Nodes) == 0 || a.Terminal == nil {
		return fmt.Errorf("%w: assembly needs nodes and a terminal", ErrInvalid)
	}
	nodes := map[string]*AssemblyNode{}
	for _, n := range a.Nodes {
		if n.Id == "" || nodes[n.Id] != nil {
			return fmt.Errorf("%w: duplicate or empty node id %s", ErrInvalid, n.Id)
		}
		nodes[n.Id] = n
		cap, err := k.capabilityFor(n.Module, n.ModuleVersion, n.Capability)
		if err != nil {
			return err
		}
		for _, ports := range [][]*Port{cap.Inputs, cap.Outputs} {
			names := map[string]bool{}
			for _, p := range ports {
				if p.Name == "" || p.Contract == "" || names[p.Name] {
					return fmt.Errorf("%w: %s has an empty or duplicate typed port", ErrInvalid, n.Id)
				}
				names[p.Name] = true
			}
		}
	}
	tn := nodes[a.Terminal.Node]
	if tn == nil {
		return fmt.Errorf("%w: terminal node does not exist", ErrInvalid)
	}
	tc, _ := k.capabilityFor(tn.Module, tn.ModuleVersion, tn.Capability)
	tp := findPort(tc.Outputs, a.Terminal.Port)
	if tp == nil {
		return fmt.Errorf("%w: terminal output does not exist", ErrInvalid)
	}
	if tp.Multiple {
		return fmt.Errorf("%w: terminal output must be scalar", ErrInvalid)
	}
	in := map[string]*NamedArtifact{}
	for _, value := range inputs {
		if value.Name == "" || in[value.Name] != nil {
			return fmt.Errorf("%w: duplicate or empty run input %s", ErrInvalid, value.Name)
		}
		if value.Artifact == nil {
			return fmt.Errorf("%w: input %s has no artifact", ErrInvalid, value.Name)
		}
		if k.artifacts[value.Artifact.Id] == nil {
			return fmt.Errorf("%w: %s", ErrNotFound, value.Artifact.Id)
		}
		in[value.Name] = value
	}
	counts := map[string]int{}
	edges := map[string][]string{}
	for _, b := range a.Bindings {
		tn := nodes[b.ToNode]
		if tn == nil {
			return fmt.Errorf("%w: unknown target %s", ErrInvalid, b.ToNode)
		}
		cap, _ := k.capabilityFor(tn.Module, tn.ModuleVersion, tn.Capability)
		tp := findPort(cap.Inputs, b.ToPort)
		if tp == nil {
			return fmt.Errorf("%w: unknown input %s.%s", ErrInvalid, b.ToNode, b.ToPort)
		}
		counts[b.ToNode+"\x00"+b.ToPort]++
		upstream, external := b.FromNode != "" || b.FromPort != "", b.Input != ""
		if upstream == external {
			return fmt.Errorf("%w: binding must have exactly one source", ErrInvalid)
		}
		var contract string
		if external {
			value := in[b.Input]
			if value == nil {
				return fmt.Errorf("%w: unknown run input %s", ErrInvalid, b.Input)
			}
			contract = k.artifacts[value.Artifact.Id].Type
		} else {
			sn := nodes[b.FromNode]
			if sn == nil {
				return fmt.Errorf("%w: unknown source %s", ErrInvalid, b.FromNode)
			}
			sc, _ := k.capabilityFor(sn.Module, sn.ModuleVersion, sn.Capability)
			sp := findPort(sc.Outputs, b.FromPort)
			if sp == nil {
				return fmt.Errorf("%w: unknown output %s.%s", ErrInvalid, b.FromNode, b.FromPort)
			}
			contract = sp.Contract
			edges[b.FromNode] = append(edges[b.FromNode], b.ToNode)
		}
		if contract != tp.Contract {
			return fmt.Errorf("%w: contract mismatch at %s.%s", ErrInvalid, b.ToNode, b.ToPort)
		}
	}
	for _, n := range a.Nodes {
		cap, _ := k.capabilityFor(n.Module, n.ModuleVersion, n.Capability)
		for _, p := range cap.Inputs {
			count := counts[n.Id+"\x00"+p.Name]
			if count == 0 && !p.Optional {
				return fmt.Errorf("%w: required input %s.%s is unbound", ErrInvalid, n.Id, p.Name)
			}
			if count > 1 && !p.Multiple {
				return fmt.Errorf("%w: input %s.%s is not multiple", ErrInvalid, n.Id, p.Name)
			}
		}
	}
	visiting, done := map[string]bool{}, map[string]bool{}
	var visit func(string) bool
	visit = func(node string) bool {
		if done[node] {
			return true
		}
		if visiting[node] {
			return false
		}
		visiting[node] = true
		for _, next := range edges[node] {
			if !visit(next) {
				return false
			}
		}
		delete(visiting, node)
		done[node] = true
		return true
	}
	for id := range nodes {
		if !visit(id) {
			return fmt.Errorf("%w: assembly contains a cycle", ErrInvalid)
		}
	}
	return nil
}

func resolveInputs(slot *runSlot, node *AssemblyNode) ([]*NamedArtifact, bool) {
	var out []*NamedArtifact
	for _, b := range slot.run.Assembly.Bindings {
		if b.ToNode != node.Id {
			continue
		}
		var ref *ArtifactRef
		if b.Input != "" {
			for _, input := range slot.run.Inputs {
				if input.Name == b.Input {
					ref = input.Artifact
					break
				}
			}
		} else if d := slot.committed[b.FromNode]; d != nil {
			for _, output := range d.Outputs {
				if output.Name == b.FromPort {
					ref = output.Artifact
					break
				}
			}
		}
		if ref == nil {
			return nil, false
		}
		out = append(out, &NamedArtifact{Name: b.ToPort, Artifact: clone(ref)})
	}
	return out, true
}

func (k *Kernel) validateOutputs(cap *Capability, outputs []*NamedArtifact) error {
	for _, expected := range cap.Outputs {
		count := 0
		for _, o := range outputs {
			if o.Name == expected.Name {
				count++
			}
		}
		if count == 0 && !expected.Optional {
			return fmt.Errorf("%w: required output %s is absent", ErrInvalid, expected.Name)
		}
		if count > 1 && !expected.Multiple {
			return fmt.Errorf("%w: output %s is not multiple", ErrInvalid, expected.Name)
		}
	}
	for _, output := range outputs {
		expected := findPort(cap.Outputs, output.Name)
		if expected == nil || output.Artifact == nil {
			return fmt.Errorf("%w: undeclared or empty output %s", ErrInvalid, output.Name)
		}
		a := k.artifacts[output.Artifact.Id]
		if a == nil {
			return fmt.Errorf("%w: %s", ErrNotFound, output.Artifact.Id)
		}
		if a.Type != expected.Contract {
			return fmt.Errorf("%w: output %s has contract %s, want %s", ErrInvalid, output.Name, a.Type, expected.Contract)
		}
	}
	return nil
}

func derivationID(d *Derivation) string {
	h := sha256.New()
	write := func(s string) { h.Write([]byte(s)); h.Write([]byte{sep}) }
	for _, s := range []string{d.RunId, d.WorkId, d.NodeId, d.Module, d.ModuleVersion, d.Capability} {
		write(s)
	}
	for _, value := range append(append([]*NamedArtifact{}, d.Inputs...), d.Outputs...) {
		write(value.Name)
		if value.Artifact != nil {
			write(value.Artifact.Id)
		} else {
			write("")
		}
	}
	return "sha256:" + fmt.Sprintf("%x", h.Sum(nil))
}

// ─── subscriber: an unbounded, in-order, non-blocking delivery queue ────────

type subscriber struct {
	topics []string
	out    chan *Event

	mu   sync.Mutex
	cond *sync.Cond
	buf  []*Event
}

func newSubscriber(topics []string) *subscriber {
	s := &subscriber{topics: append([]string(nil), topics...), out: make(chan *Event)}
	s.cond = sync.NewCond(&s.mu)
	go s.pump()
	return s
}

func (s *subscriber) wants(topic string) bool {
	for _, t := range s.topics {
		if t == topic {
			return true
		}
	}
	return false
}

// enqueue appends under the subscriber's own lock — quick, never blocks the
// publisher even if the consumer is slow.
func (s *subscriber) enqueue(e *Event) {
	s.mu.Lock()
	s.buf = append(s.buf, e)
	s.cond.Signal()
	s.mu.Unlock()
}

// pump forwards buffered events to out in FIFO (== Seq) order.
func (s *subscriber) pump() {
	for {
		s.mu.Lock()
		for len(s.buf) == 0 {
			s.cond.Wait()
		}
		e := s.buf[0]
		s.buf = s.buf[1:]
		s.mu.Unlock()
		s.out <- e
	}
}

func lifecycleVerb(l Lifecycle) string {
	switch l {
	case LifecycleRegistered:
		return "registered"
	case LifecycleLoaded:
		return "loaded"
	case LifecycleActive:
		return "activated"
	case LifecycleDeactivated:
		return "deactivated"
	default:
		return "unspecified"
	}
}
