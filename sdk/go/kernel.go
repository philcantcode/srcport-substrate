package substrate

import (
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
	eventSeq     uint64
	gateCounter  uint64
}

// NewKernel returns an empty, ready kernel.
func NewKernel() *Kernel {
	k := &Kernel{
		contracts: map[string]*Contract{},
		artifacts: map[string]*Artifact{},
		gates:     map[string]*GateDecision{},
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
	k.appendLocked("event.published", e.Topic, nil)
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
