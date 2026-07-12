package substrate

import (
	"crypto/sha256"
	"errors"
	"fmt"
	"sync"
	"time"

	"google.golang.org/protobuf/proto"
)

// ─── errors ─────────────────────────────────────────────────────────────────

// ErrNotFound is returned when no artifact, blob, or run exists for a given id.
var ErrNotFound = errors.New("not found")

var ErrInvalid = errors.New("invalid")
var ErrConflict = errors.New("conflict")

// ErrFailedPrecondition is returned when a call precondition fails (e.g. the
// absolute deadline in RequestContext has already passed).
var ErrFailedPrecondition = errors.New("failed precondition")

// ErrBlobIntegrity is returned when stored blob bytes do not match the claimed
// digest or byte_count (verified external refs).
var ErrBlobIntegrity = errors.New("blob integrity check failed")

// RunClosedError is returned when a terminal run accepts no more work.
type RunClosedError struct{ State RunState }

func (e *RunClosedError) Error() string { return fmt.Sprintf("run is closed: %s", e.State) }

// ToError projects a native error onto the portable Error wire message so
// failure semantics are identical across languages and across the wire.
// Mirrors KernelError::to_proto in the Rust SDK.
func ToError(err error) *Error {
	if err == nil {
		return nil
	}
	e := &Error{Message: err.Error(), Retryable: false}
	var closed *RunClosedError
	switch {
	case errors.Is(err, ErrNotFound):
		e.Code = ErrorCodeNotFound
	case errors.Is(err, ErrInvalid):
		e.Code = ErrorCodeInvalid
	case errors.Is(err, ErrConflict):
		e.Code = ErrorCodeConflict
		e.ConflictSubject = err.Error()
	case errors.Is(err, ErrBlobIntegrity):
		e.Code = ErrorCodeBlobIntegrity
		e.FailedPrecondition = err.Error()
	case errors.Is(err, ErrFailedPrecondition):
		e.Code = ErrorCodeFailedPrecondition
		e.FailedPrecondition = err.Error()
	case errors.As(err, &closed):
		e.Code = ErrorCodeFailedPrecondition
		e.FailedPrecondition = closed.Error()
	default:
		e.Code = ErrorCodeUnspecified
	}
	return e
}

// SubscriberBuffer is the bound on a single subscriber's undelivered-event
// backlog. The event bus is notification, not the data plane; a subscriber
// that falls this far behind is shed rather than allowed to OOM the kernel.
const SubscriberBuffer = 1024


// ─── MemoryKernel (in-memory KernelApi) ─────────────────────────────────────

type moduleSlot struct {
	manifest  *ModuleManifest
	lifecycle Lifecycle
}

type runSlot struct {
	run       *Run
	claimed   map[string]*WorkItem
	committed map[string]*Derivation
}

// blobKey is the blob store address: namespace + digest. Digest is content
// identity; namespace is storage routing / tenancy.
type blobKey struct {
	namespace string
	digest    string
}

type blobSlot struct {
	data []byte
	ref  *BlobRef
}

// idempotentResult caches a successful PutArtifact / StartRun / Commit response
// under a non-empty RequestContext.request_key.
type idempotentResult struct {
	artifact *ArtifactRef
	run      *Run
}

// MemoryKernel is the in-memory realisation of KernelApi. Its methods mirror
// the service Kernel RPCs in substrate.proto one-for-one. It is safe for
// concurrent use; share one *MemoryKernel across module goroutines. Every
// meaningful action lands one append-only ledger entry. Values handed in and
// out are cloned, so a caller can never mutate stored state through a shared
// pointer.
//
// Durability of kernel state is the job of a KernelApi backend; domain state
// lives in Modules. MemoryKernel is one backend, not the authority.
type MemoryKernel struct {
	mu           sync.Mutex
	modules      []moduleSlot
	capabilities []*Capability
	contracts    map[string]*Contract
	artifacts    map[string]*Artifact
	blobs        map[blobKey]*blobSlot
	subs         []*subscriber
	ledger       []*LedgerEntry
	runs         map[string]*runSlot
	derivations  []*Derivation
	eventSeq     uint64
	idempotency  map[string]idempotentResult
}

// NewMemoryKernel returns an empty, ready in-memory kernel.
func NewMemoryKernel() *MemoryKernel {
	return &MemoryKernel{
		contracts:   map[string]*Contract{},
		artifacts:   map[string]*Artifact{},
		blobs:       map[blobKey]*blobSlot{},
		runs:        map[string]*runSlot{},
		idempotency: map[string]idempotentResult{},
	}
}

// KernelApi is the portable ABI: the unary RPCs of service Kernel (including
// Transition). Streaming Subscribe is inherent-only on MemoryKernel.
// RequestContext rides as call metadata (variadic, optional) and is
// deliberately not folded into ledger detail.
//
// Enforced context semantics: deadline_unix_ms rejects past deadlines with
// ErrFailedPrecondition; non-empty request_key de-duplicates PutArtifact,
// StartRun, and Commit by (caller, request_key, operation).
type KernelApi interface {
	Register(m *ModuleManifest, ctx ...*RequestContext) *RegisterAck
	Transition(req *TransitionRequest, ctx ...*RequestContext) (*TransitionAck, error)
	PutArtifact(a *Artifact, ctx ...*RequestContext) (*ArtifactRef, error)
	GetArtifact(ref *ArtifactRef, ctx ...*RequestContext) (*Artifact, error)
	PutBlob(req *PutBlobRequest, ctx ...*RequestContext) *BlobRef
	GetBlob(req *GetBlobRequest, ctx ...*RequestContext) (*BlobData, error)
	HasBlob(req *HasBlobRequest, ctx ...*RequestContext) *HasBlobResponse
	PutContract(c *Contract, ctx ...*RequestContext) (*Contract, error)
	Publish(e *Event, ctx ...*RequestContext) *PublishAck
	Append(r *AppendRequest, ctx ...*RequestContext) *LedgerEntry
	Snapshot(ctx ...*RequestContext) *RegistrySnapshot
	StartRun(req *RunRequest, ctx ...*RequestContext) (*Run, error)
	ClaimReady(req *ClaimRequest, ctx ...*RequestContext) (*WorkItem, error)
	Commit(submitted *Derivation, ctx ...*RequestContext) (*Run, error)
	GetRun(ref *RunRef, ctx ...*RequestContext) (*Run, error)
	CancelRun(ref *RunRef, ctx ...*RequestContext) (*Run, error)
	ListDerivations(ref *RunRef, ctx ...*RequestContext) (*DerivationList, error)
}

func firstCtx(ctx []*RequestContext) *RequestContext {
	if len(ctx) > 0 && ctx[0] != nil {
		return ctx[0]
	}
	return &RequestContext{}
}

func checkDeadline(ctx *RequestContext) error {
	if ctx == nil || ctx.DeadlineUnixMs <= 0 {
		return nil
	}
	now := time.Now().UnixMilli()
	if now > ctx.DeadlineUnixMs {
		return fmt.Errorf("%w: deadline exceeded", ErrFailedPrecondition)
	}
	return nil
}

func idempotencyKey(op string, ctx *RequestContext) (string, bool) {
	if ctx == nil || ctx.RequestKey == "" {
		return "", false
	}
	return op + "\x00" + ctx.Caller + "\x00" + ctx.RequestKey, true
}

// Compile-time check: MemoryKernel implements KernelApi.
var _ KernelApi = (*MemoryKernel)(nil)

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
func (k *MemoryKernel) appendLocked(kind, subject string, detail []byte) *LedgerEntry {
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

// Register records a module, its capabilities, and (implicitly) name-only
// placeholders for the contracts named on those capabilities' ports. The module
// lands in REGISTERED; advance it with Transition. Mirrors rpc Register.
// Placeholders may be filled once via PutContract; they do not write
// contract.registered ledger entries (module.registered already names the refs).
func (k *MemoryKernel) Register(m *ModuleManifest, ctx ...*RequestContext) *RegisterAck {
	m = clone(m)
	k.mu.Lock()
	defer k.mu.Unlock()
	for _, c := range m.Provides {
		k.capabilities = append(k.capabilities, clone(c))
		for _, p := range append(append([]*Port{}, c.Inputs...), c.Outputs...) {
			k.ensureContractPlaceholderLocked(p.Contract)
		}
	}
	k.modules = append(k.modules, moduleSlot{manifest: m, lifecycle: LifecycleRegistered})
	// The full manifest lands in the tamper-evident chain, so the registry is
	// reconstructable from the ledger alone. detail is the canonical manifest.
	k.appendLocked("module.registered", m.Name, marshalCanonical(m))
	return &RegisterAck{State: LifecycleRegistered}
}

// ensureContractPlaceholderLocked records a name-only stub if ref is new and
// non-empty. Caller holds mu.
func (k *MemoryKernel) ensureContractPlaceholderLocked(ref string) {
	if ref == "" {
		return
	}
	if _, ok := k.contracts[ref]; ok {
		return
	}
	k.contracts[ref] = &Contract{
		Ref:    ref,
		Digest: ContractDigest("", "", "", nil),
	}
}

// Transition is rpc Transition(TransitionRequest) -> TransitionAck. Advances a
// module along REGISTERED → LOADED → ACTIVE → DEACTIVATED. Only a single
// forward step is applied; anything else is a no-op that returns the current
// state. Returns ErrNotFound if the module is unknown.
func (k *MemoryKernel) Transition(req *TransitionRequest, ctx ...*RequestContext) (*TransitionAck, error) {
	c := firstCtx(ctx)
	if err := checkDeadline(c); err != nil {
		return nil, err
	}
	if req == nil {
		return nil, fmt.Errorf("%w: transition request is required", ErrInvalid)
	}
	k.mu.Lock()
	defer k.mu.Unlock()
	for i := range k.modules {
		if k.modules[i].manifest.Name == req.Module {
			if req.To == k.modules[i].lifecycle+1 {
				k.modules[i].lifecycle = req.To
				k.appendLocked("module."+lifecycleVerb(req.To), req.Module, nil)
			}
			return &TransitionAck{State: k.modules[i].lifecycle}, nil
		}
	}
	return nil, fmt.Errorf("%w: %s", ErrNotFound, req.Module)
}

// ─── 2. Artifact ──────────────────────────────────────────────────────────

// PutArtifact content-addresses the typed value, stores it immutably (first
// write wins), and returns its ref. Mirrors rpc PutArtifact.
//
// Inline: pass body, leave object unset. External: PutBlob first, then pass
// object (digest, byte_count, namespace) with body empty. The blob must already
// exist and match. Exactly one of body or object may carry the value.
// Honour RequestContext deadline and request_key idempotency.
func (k *MemoryKernel) PutArtifact(a *Artifact, ctx ...*RequestContext) (*ArtifactRef, error) {
	c := firstCtx(ctx)
	if err := checkDeadline(c); err != nil {
		return nil, err
	}
	a = clone(a)
	if err := validateArtifactContent(a); err != nil {
		return nil, err
	}
	k.mu.Lock()
	defer k.mu.Unlock()
	if key, ok := idempotencyKey("put_artifact", c); ok {
		if cached, hit := k.idempotency[key]; hit && cached.artifact != nil {
			return clone(cached.artifact), nil
		}
	}
	if HasExternalObject(a) {
		if err := k.verifyObjectRefLocked(a.Object); err != nil {
			return nil, err
		}
	}
	id := ArtifactIDOf(a)
	if _, ok := k.artifacts[id]; !ok {
		stored := clone(a)
		stored.Id = id
		k.artifacts[id] = stored
		// Ledger: clear large inline body; keep ObjectRef (small, part of value
		// identity) so external artifacts reconstruct without blob bytes.
		forLog := clone(stored)
		forLog.Body = nil
		k.appendLocked("artifact.put", id, marshalCanonical(forLog))
	}
	ref := &ArtifactRef{Id: id}
	if key, ok := idempotencyKey("put_artifact", c); ok {
		if _, hit := k.idempotency[key]; !hit {
			k.idempotency[key] = idempotentResult{artifact: clone(ref)}
		}
	}
	return ref, nil
}

// GetArtifact reads an artifact back byte-identical. Mirrors rpc GetArtifact.
func (k *MemoryKernel) GetArtifact(ref *ArtifactRef, ctx ...*RequestContext) (*Artifact, error) {
	if err := checkDeadline(firstCtx(ctx)); err != nil {
		return nil, err
	}
	k.mu.Lock()
	defer k.mu.Unlock()
	a, ok := k.artifacts[ref.Id]
	if !ok {
		return nil, fmt.Errorf("%w: %s", ErrNotFound, ref.Id)
	}
	return clone(a), nil
}

// PutBlob content-addresses raw bytes and stores them immutably under
// (namespace, digest). First write wins. Mirrors rpc PutBlob.
func (k *MemoryKernel) PutBlob(req *PutBlobRequest, ctx ...*RequestContext) *BlobRef {
	data := append([]byte(nil), req.Data...)
	digest := BlobID(data)
	ns := req.Namespace
	key := blobKey{namespace: ns, digest: digest}
	ref := &BlobRef{Digest: digest, ByteCount: uint64(len(data)), Namespace: ns}

	k.mu.Lock()
	defer k.mu.Unlock()
	if _, ok := k.blobs[key]; !ok {
		k.blobs[key] = &blobSlot{data: data, ref: clone(ref)}
		// Never chain raw blob data — subject is the digest; detail is BlobRef.
		k.appendLocked("blob.put", digest, marshalCanonical(ref))
	}
	return clone(ref)
}

// GetBlob streams back (in-process: returns) verified blob bytes. Re-hashes on
// read and rejects digest/size mismatches. Mirrors rpc GetBlob.
func (k *MemoryKernel) GetBlob(req *GetBlobRequest, ctx ...*RequestContext) (*BlobData, error) {
	if err := checkDeadline(firstCtx(ctx)); err != nil {
		return nil, err
	}
	k.mu.Lock()
	defer k.mu.Unlock()
	slot, ok := k.blobs[blobKey{namespace: req.Namespace, digest: req.Digest}]
	if !ok {
		return nil, fmt.Errorf("%w: blob %s", ErrNotFound, req.Digest)
	}
	if err := verifyBlobData(slot.data, req.Digest, uint64(len(slot.data))); err != nil {
		return nil, err
	}
	// Re-check stored claim against recomputed digest.
	if got := BlobID(slot.data); got != slot.ref.Digest || uint64(len(slot.data)) != slot.ref.ByteCount {
		return nil, fmt.Errorf("%w: stored blob corrupted", ErrBlobIntegrity)
	}
	return &BlobData{
		Digest:    slot.ref.Digest,
		ByteCount: slot.ref.ByteCount,
		Namespace: slot.ref.Namespace,
		Data:      append([]byte(nil), slot.data...),
	}, nil
}

// HasBlob reports whether (namespace, digest) exists. Mirrors rpc HasBlob.
func (k *MemoryKernel) HasBlob(req *HasBlobRequest, ctx ...*RequestContext) *HasBlobResponse {
	k.mu.Lock()
	defer k.mu.Unlock()
	if slot, ok := k.blobs[blobKey{namespace: req.Namespace, digest: req.Digest}]; ok {
		return &HasBlobResponse{Exists: true, ByteCount: slot.ref.ByteCount}
	}
	return &HasBlobResponse{Exists: false}
}

// PutArtifactWithBlob puts the blob then an external artifact referencing it.
// Convenience for the production path: large data → blob store + ObjectRef.
func (k *MemoryKernel) PutArtifactWithBlob(typ, namespace string, data []byte, producedBy string, ctx ...*RequestContext) (*ArtifactRef, *BlobRef, error) {
	blob := k.PutBlob(&PutBlobRequest{Namespace: namespace, Data: data})
	ref, err := k.PutArtifact(&Artifact{
		Type:       typ,
		ProducedBy: producedBy,
		Object: &ObjectRef{
			Digest:    blob.Digest,
			ByteCount: blob.ByteCount,
			Namespace: blob.Namespace,
		},
	})
	return ref, blob, err
}

func validateArtifactContent(a *Artifact) error {
	if a == nil {
		return fmt.Errorf("%w: artifact is required", ErrInvalid)
	}
	if a.Type == "" {
		return fmt.Errorf("%w: artifact type is required", ErrInvalid)
	}
	hasObj := HasExternalObject(a)
	hasBody := len(a.Body) > 0
	if hasObj && hasBody {
		return fmt.Errorf("%w: artifact must not set both body and object", ErrInvalid)
	}
	if !hasObj && a.Object != nil && a.Object.Digest == "" && (a.Object.ByteCount != 0 || a.Object.Namespace != "") {
		return fmt.Errorf("%w: object.digest is required when object is set", ErrInvalid)
	}
	if hasObj && !isSHA256Digest(a.Object.Digest) {
		return fmt.Errorf("%w: object.digest must be sha256:<hex>", ErrInvalid)
	}
	return nil
}

func isSHA256Digest(d string) bool {
	const prefix = "sha256:"
	if len(d) != len(prefix)+64 || d[:len(prefix)] != prefix {
		return false
	}
	for i := len(prefix); i < len(d); i++ {
		c := d[i]
		if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')) {
			return false
		}
	}
	return true
}

func verifyBlobData(data []byte, digest string, byteCount uint64) error {
	if uint64(len(data)) != byteCount {
		return fmt.Errorf("%w: size %d != claimed %d", ErrBlobIntegrity, len(data), byteCount)
	}
	if BlobID(data) != digest {
		return fmt.Errorf("%w: digest mismatch", ErrBlobIntegrity)
	}
	return nil
}

func (k *MemoryKernel) verifyObjectRefLocked(o *ObjectRef) error {
	slot, ok := k.blobs[blobKey{namespace: o.Namespace, digest: o.Digest}]
	if !ok {
		return fmt.Errorf("%w: blob %s (namespace %q)", ErrNotFound, o.Digest, o.Namespace)
	}
	if slot.ref.ByteCount != o.ByteCount {
		return fmt.Errorf("%w: object.byte_count %d != stored %d", ErrBlobIntegrity, o.ByteCount, slot.ref.ByteCount)
	}
	if BlobID(slot.data) != o.Digest || uint64(len(slot.data)) != o.ByteCount {
		return fmt.Errorf("%w: blob does not match object ref", ErrBlobIntegrity)
	}
	return nil
}

// ─── 3. Contract ──────────────────────────────────────────────────────────

// PutContract registers a contract immutably under its ref. Returns the stored
// contract (digest assigned). Identical re-puts are no-ops; a different content
// under the same ref is ErrConflict. A name-only placeholder created by
// Register may be filled once. Mirrors rpc PutContract.
func (k *MemoryKernel) PutContract(c *Contract, ctx ...*RequestContext) (*Contract, error) {
	if err := checkDeadline(firstCtx(ctx)); err != nil {
		return nil, err
	}
	if c == nil || c.Ref == "" {
		return nil, fmt.Errorf("%w: contract ref is required", ErrInvalid)
	}
	c = clone(c)
	// Normalize compatible_with to UTF-8 ascending for stable identity.
	sortStringsUTF8(c.CompatibleWith)
	digest := ContractDigest(c.MediaType, c.Schema, c.Version, c.CompatibleWith)
	if c.Digest != "" && c.Digest != digest {
		return nil, fmt.Errorf("%w: contract digest mismatch", ErrInvalid)
	}
	c.Digest = digest

	k.mu.Lock()
	defer k.mu.Unlock()
	if existing, ok := k.contracts[c.Ref]; ok {
		if existing.Digest == c.Digest {
			return clone(existing), nil
		}
		// Empty placeholder → first real content is allowed once.
		if IsContractPlaceholder(existing) && !IsContractPlaceholder(c) {
			stored := clone(c)
			k.contracts[c.Ref] = stored
			k.appendLocked("contract.registered", c.Ref, marshalCanonical(stored))
			return clone(stored), nil
		}
		return nil, fmt.Errorf("%w: contract %s already registered with different content", ErrConflict, c.Ref)
	}
	stored := clone(c)
	k.contracts[c.Ref] = stored
	k.appendLocked("contract.registered", c.Ref, marshalCanonical(stored))
	return clone(stored), nil
}

// ─── 4. Event ─────────────────────────────────────────────────────────────

// Subscribe returns a channel of events on the given topics, in kernel Seq
// order. A subscriber only ever receives events on topics it named. The channel
// is buffered (SubscriberBuffer); delivery is non-blocking. A subscriber that
// falls behind is shed on Publish so one slow consumer cannot OOM the kernel.
// Mirrors rpc Subscribe (stream Event).
func (k *MemoryKernel) Subscribe(s *Subscription, ctx ...*RequestContext) <-chan *Event {
	sub := newSubscriber(s.Topics)
	k.mu.Lock()
	k.subs = append(k.subs, sub)
	k.mu.Unlock()
	return sub.out
}

// Publish assigns a monotonic Seq (the total order), delivers to exactly the
// subscribers of Event.Topic and never to anyone else, and returns the assigned
// Seq. Mirrors rpc Publish. A slow subscriber whose buffer is full is shed;
// dropped notifications remain reconstructable from the ledger.
func (k *MemoryKernel) Publish(e *Event, ctx ...*RequestContext) *PublishAck {
	k.mu.Lock()
	defer k.mu.Unlock()
	k.eventSeq++
	e = clone(e)
	e.Seq = k.eventSeq
	alive := k.subs[:0]
	for _, sub := range k.subs {
		if sub.wants(e.Topic) {
			if sub.tryEnqueue(clone(e)) {
				alive = append(alive, sub)
			}
			// else shed: buffer full or receiver gone
		} else {
			alive = append(alive, sub)
		}
	}
	k.subs = alive
	// Artifact refs are the data plane; the Event lands fully in the chain.
	k.appendLocked("event.published", e.Topic, marshalCanonical(e))
	return &PublishAck{Seq: e.Seq}
}

// ─── 5. Ledger ────────────────────────────────────────────────────────────

// Append lets modules write their own domain facts into the same tamper-evident
// chain. Mirrors rpc Append.
func (k *MemoryKernel) Append(r *AppendRequest, ctx ...*RequestContext) *LedgerEntry {
	k.mu.Lock()
	defer k.mu.Unlock()
	return k.appendLocked(r.Kind, r.Subject, r.Detail)
}

// Ledger returns a snapshot (deep copy) of the whole ledger, for
// verification/audit. Mutating the result never affects the kernel.
func (k *MemoryKernel) Ledger() []*LedgerEntry {
	k.mu.Lock()
	defer k.mu.Unlock()
	out := make([]*LedgerEntry, len(k.ledger))
	for i, e := range k.ledger {
		out[i] = clone(e)
	}
	return out
}

// VerifyLedger verifies the kernel's own live ledger.
func (k *MemoryKernel) VerifyLedger() bool {
	k.mu.Lock()
	defer k.mu.Unlock()
	return VerifyChain(k.ledger)
}

// ─── 6. Registry ──────────────────────────────────────────────────────────

// Snapshot answers "what exists right now": every registered module,
// capability, and contract. Mirrors rpc Snapshot.
func (k *MemoryKernel) Snapshot(ctx ...*RequestContext) *RegistrySnapshot {
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
// Honour RequestContext deadline and request_key idempotency.
func (k *MemoryKernel) StartRun(req *RunRequest, ctx ...*RequestContext) (*Run, error) {
	c := firstCtx(ctx)
	if err := checkDeadline(c); err != nil {
		return nil, err
	}
	req = clone(req)
	k.mu.Lock()
	defer k.mu.Unlock()
	if key, ok := idempotencyKey("start_run", c); ok {
		if cached, hit := k.idempotency[key]; hit && cached.run != nil {
			return clone(cached.run), nil
		}
	}
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
	out := clone(run)
	if key, ok := idempotencyKey("start_run", c); ok {
		if _, hit := k.idempotency[key]; !hit {
			k.idempotency[key] = idempotentResult{run: clone(out)}
		}
	}
	return out, nil
}

// ClaimReady atomically claims one ready node for module. An empty WorkItem
// means this module has no ready node. If no work exists anywhere, the run is
// closed as STALLED.
func (k *MemoryKernel) ClaimReady(req *ClaimRequest, ctx ...*RequestContext) (*WorkItem, error) {
	if err := checkDeadline(firstCtx(ctx)); err != nil {
		return nil, err
	}
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
// Honour RequestContext deadline and request_key idempotency.
func (k *MemoryKernel) Commit(submitted *Derivation, ctx ...*RequestContext) (*Run, error) {
	c := firstCtx(ctx)
	if err := checkDeadline(c); err != nil {
		return nil, err
	}
	submitted = clone(submitted)
	k.mu.Lock()
	defer k.mu.Unlock()
	if key, ok := idempotencyKey("commit", c); ok {
		if cached, hit := k.idempotency[key]; hit && cached.run != nil {
			return clone(cached.run), nil
		}
	}
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
	out := clone(slot.run)
	if key, ok := idempotencyKey("commit", c); ok {
		if _, hit := k.idempotency[key]; !hit {
			k.idempotency[key] = idempotentResult{run: clone(out)}
		}
	}
	return out, nil
}

func (k *MemoryKernel) GetRun(ref *RunRef, ctx ...*RequestContext) (*Run, error) {
	if err := checkDeadline(firstCtx(ctx)); err != nil {
		return nil, err
	}
	k.mu.Lock()
	defer k.mu.Unlock()
	if slot := k.runs[ref.Id]; slot != nil {
		return clone(slot.run), nil
	}
	return nil, fmt.Errorf("%w: %s", ErrNotFound, ref.Id)
}

func (k *MemoryKernel) CancelRun(ref *RunRef, ctx ...*RequestContext) (*Run, error) {
	if err := checkDeadline(firstCtx(ctx)); err != nil {
		return nil, err
	}
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

func (k *MemoryKernel) Derivations() []*Derivation {
	k.mu.Lock()
	defer k.mu.Unlock()
	out := make([]*Derivation, len(k.derivations))
	for i, d := range k.derivations {
		out[i] = clone(d)
	}
	return out
}

func (k *MemoryKernel) ListDerivations(ref *RunRef, ctx ...*RequestContext) (*DerivationList, error) {
	if err := checkDeadline(firstCtx(ctx)); err != nil {
		return nil, err
	}
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

func (k *MemoryKernel) capabilityFor(module, version, capability string) (*Capability, error) {
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

func (k *MemoryKernel) validateAssembly(a *Assembly, inputs []*NamedArtifact) error {
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

func (k *MemoryKernel) validateOutputs(cap *Capability, outputs []*NamedArtifact) error {
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

// ─── subscriber: bounded, in-order, non-blocking delivery ───────────────────

type subscriber struct {
	topics []string
	out    chan *Event // capacity SubscriberBuffer
}

func newSubscriber(topics []string) *subscriber {
	return &subscriber{
		topics: append([]string(nil), topics...),
		out:    make(chan *Event, SubscriberBuffer),
	}
}

func (s *subscriber) wants(topic string) bool {
	for _, t := range s.topics {
		if t == topic {
			return true
		}
	}
	return false
}

// tryEnqueue delivers without blocking. Returns false if the buffer is full
// (caller should shed this subscriber).
func (s *subscriber) tryEnqueue(e *Event) bool {
	select {
	case s.out <- e:
		return true
	default:
		return false
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
