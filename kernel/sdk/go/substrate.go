// Package substrate is the in-process Go SDK for the srcport-substrate
// microkernel: seven primitives (Module · Artifact · Contract · Event ·
// Ledger · Registry · Run) and one Kernel ABI, conformant to SPEC.md.
//
// MemoryKernel is the in-memory realisation of KernelApi. Durability lives in
// Modules (or other KernelApi backends), not the core.
//
// The message types are GENERATED from the canonical contract in
// contracts/proto/srcport/substrate/v1/substrate.proto (see buf.gen.yaml and
// scripts/gen.sh) and re-exported here as aliases, so this SDK can never drift
// from the contract. To add capability, widen the proto and regenerate; do not
// re-derive the core.
package substrate

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"fmt"

	"google.golang.org/protobuf/proto"

	pb "github.com/philcantcode/srcport-substrate/kernel/sdk/go/internal/genpb/srcport/substrate/v1"
)

// ─── the seven primitives + ABI acks, aliased to the generated types ────────
// These ARE the protobuf messages; construct with a pointer, e.g.
// &substrate.Artifact{Type: "…", Body: …}.

type (
	Capability       = pb.Capability
	ModuleManifest   = pb.ModuleManifest
	BlobRef          = pb.BlobRef
	ObjectRef        = pb.ObjectRef
	Artifact         = pb.Artifact
	Trait            = pb.Trait
	ArtifactRef      = pb.ArtifactRef
	PutBlobRequest   = pb.PutBlobRequest
	GetBlobRequest   = pb.GetBlobRequest
	BlobData         = pb.BlobData
	HasBlobRequest   = pb.HasBlobRequest
	HasBlobResponse  = pb.HasBlobResponse
	Contract         = pb.Contract
	Event            = pb.Event
	Subscription     = pb.Subscription
	LedgerEntry      = pb.LedgerEntry
	RegistrySnapshot = pb.RegistrySnapshot
	RegisterAck      = pb.RegisterAck
	PublishAck       = pb.PublishAck
	AppendRequest    = pb.AppendRequest
	Port             = pb.Port
	NamedArtifact    = pb.NamedArtifact
	AssemblyNode     = pb.AssemblyNode
	Binding          = pb.Binding
	NodeOutput       = pb.NodeOutput
	Limits            = pb.Limits
	Assembly          = pb.Assembly
	RunRequest        = pb.RunRequest
	Run               = pb.Run
	RunRef            = pb.RunRef
	InjectInputRequest = pb.InjectInputRequest
	ClaimRequest      = pb.ClaimRequest
	WorkItem          = pb.WorkItem
	Derivation        = pb.Derivation
	DerivationList    = pb.DerivationList
	ExecutionPolicy   = pb.ExecutionPolicy
	RequestContext    = pb.RequestContext
	Error             = pb.Error
	ErrorCode         = pb.ErrorCode
	SnapshotRequest   = pb.SnapshotRequest
	TransitionRequest = pb.TransitionRequest
	TransitionAck     = pb.TransitionAck

	Lifecycle = pb.Lifecycle
	RunState  = pb.RunState
	Firing    = pb.Firing
	Closure   = pb.Closure
)

// Portable ErrorCode values (same integers on every SDK and over the wire).
const (
	ErrorCodeUnspecified        = pb.ErrorCode_ERROR_CODE_UNSPECIFIED
	ErrorCodeNotFound           = pb.ErrorCode_ERROR_CODE_NOT_FOUND
	ErrorCodeInvalid            = pb.ErrorCode_ERROR_CODE_INVALID
	ErrorCodeConflict           = pb.ErrorCode_ERROR_CODE_CONFLICT
	ErrorCodeFailedPrecondition = pb.ErrorCode_ERROR_CODE_FAILED_PRECONDITION
	ErrorCodeResourceExhausted  = pb.ErrorCode_ERROR_CODE_RESOURCE_EXHAUSTED
	ErrorCodeBlobIntegrity      = pb.ErrorCode_ERROR_CODE_BLOB_INTEGRITY
)

// MaxInlineArtifactBytes is the advisory ceiling for a single Trait.body.
// Larger payloads should PutBlob and place a verified ObjectRef on the trait.
// The kernel does not hard-reject oversized inline bodies; modules and hosts
// SHOULD honour this boundary in production.
const MaxInlineArtifactBytes = 1 << 20 // 1 MiB

// Bounded run states.
const (
	RunStateUnspecified = pb.RunState_RUN_STATE_UNSPECIFIED
	RunStateRunning     = pb.RunState_RUN_STATE_RUNNING
	RunStateCompleted   = pb.RunState_RUN_STATE_COMPLETED
	RunStateStalled     = pb.RunState_RUN_STATE_STALLED
	RunStateFailed      = pb.RunState_RUN_STATE_FAILED
	RunStateCancelled   = pb.RunState_RUN_STATE_CANCELLED
)

// Lifecycle values (REGISTERED → LOADED → ACTIVE → DEACTIVATED).
const (
	LifecycleUnspecified = pb.Lifecycle_LIFECYCLE_UNSPECIFIED
	LifecycleRegistered  = pb.Lifecycle_LIFECYCLE_REGISTERED
	LifecycleLoaded      = pb.Lifecycle_LIFECYCLE_LOADED
	LifecycleActive      = pb.Lifecycle_LIFECYCLE_ACTIVE
	LifecycleDeactivated = pb.Lifecycle_LIFECYCLE_DEACTIVATED
)

// Work-unit firing policies (module default + optional run override).
const (
	FiringUnspecified = pb.Firing_FIRING_UNSPECIFIED
	FiringOnce        = pb.Firing_FIRING_ONCE
	FiringAlways      = pb.Firing_FIRING_ALWAYS
	FiringOncePerKey  = pb.Firing_FIRING_ONCE_PER_KEY
)

// Run closure policies.
const (
	ClosureUnspecified   = pb.Closure_CLOSURE_UNSPECIFIED
	ClosureFirstTerminal = pb.Closure_CLOSURE_FIRST_TERMINAL
	ClosureOpen          = pb.Closure_CLOSURE_OPEN
)


// ─── addressing & ledger hashing — the hash rules SPEC.md pins down ──────────

const sep = 0x00

// BlobID is pure content identity: "sha256:" + hex(sha256(data)). Namespace and
// typed Artifact fields are NOT part of blob identity.
func BlobID(data []byte) string {
	sum := sha256.Sum256(data)
	return "sha256:" + hex.EncodeToString(sum[:])
}

// ObjectRefBytes is the address payload for an external trait value:
//
//	digest ‖ 0x00 ‖ uint64_be(byte_count) ‖ 0x00 ‖ namespace
func ObjectRefBytes(o *ObjectRef) []byte {
	if o == nil {
		return nil
	}
	var count [8]byte
	binary.BigEndian.PutUint64(count[:], o.ByteCount)
	out := make([]byte, 0, len(o.Digest)+1+8+1+len(o.Namespace))
	out = append(out, o.Digest...)
	out = append(out, sep)
	out = append(out, count[:]...)
	out = append(out, sep)
	out = append(out, o.Namespace...)
	return out
}

// TraitContent returns bytes folded into value identity for one trait.
func TraitContent(f *Trait) []byte {
	if f != nil && f.Object != nil && f.Object.Digest != "" {
		return ObjectRefBytes(f.Object)
	}
	if f == nil {
		return nil
	}
	return f.Body
}

// TraitHasExternal reports whether the trait holds a verified ObjectRef.
func TraitHasExternal(f *Trait) bool {
	return f != nil && f.Object != nil && f.Object.Digest != ""
}

// ArtifactCanonicalBytes encodes a trait bag for content addressing:
//
//	for each contract_ref in UTF-8 ascending order:
//	    contract_ref ‖ 0x00 ‖ trait_content ‖ 0x00
func ArtifactCanonicalBytes(a *Artifact) []byte {
	if a == nil || len(a.Traits) == 0 {
		return nil
	}
	keys := make([]string, 0, len(a.Traits))
	for k := range a.Traits {
		keys = append(keys, k)
	}
	sortStringsUTF8(keys)
	var out []byte
	for _, k := range keys {
		out = append(out, k...)
		out = append(out, sep)
		out = append(out, TraitContent(a.Traits[k])...)
		out = append(out, sep)
	}
	return out
}

// ArtifactIDOf is the content address of a full trait-bag Artifact.
// Meta, ProducedBy, EntityId, and Supersedes are NOT part of the address.
func ArtifactIDOf(a *Artifact) string {
	sum := sha256.Sum256(ArtifactCanonicalBytes(a))
	return "sha256:" + hex.EncodeToString(sum[:])
}

// ArtifactIDSingle is the content address of a single-trait bag.
func ArtifactIDSingle(contract string, content []byte) string {
	return ArtifactIDOf(ArtifactWithTrait(contract, content))
}

// ArtifactWithTrait builds an in-memory single-trait artifact (not stored).
func ArtifactWithTrait(contract string, body []byte) *Artifact {
	return &Artifact{
		Traits: map[string]*Trait{
			contract: {Body: append([]byte(nil), body...)},
		},
	}
}

// ArtifactWithExternalTrait builds a single external-object trait artifact.
func ArtifactWithExternalTrait(contract string, obj *ObjectRef) *Artifact {
	return &Artifact{
		Traits: map[string]*Trait{
			contract: {Object: obj},
		},
	}
}

// HasTraits reports whether the artifact contains every listed contract ref.
func HasTraits(a *Artifact, required []string) bool {
	if a == nil {
		return false
	}
	for _, r := range required {
		if _, ok := a.Traits[r]; !ok {
			return false
		}
	}
	return true
}

// TraitSetCovers reports whether have ⊇ need.
func TraitSetCovers(have, need []string) bool {
	set := map[string]struct{}{}
	for _, h := range have {
		set[h] = struct{}{}
	}
	for _, n := range need {
		if _, ok := set[n]; !ok {
			return false
		}
	}
	return true
}

// GetTrait returns the trait for a contract ref, or nil.
func GetTrait(a *Artifact, contract string) *Trait {
	if a == nil {
		return nil
	}
	return a.Traits[contract]
}

// ProjectTraits returns a new bag with only the named traits.
func ProjectTraits(a *Artifact, contracts []string) (*Artifact, error) {
	if a == nil {
		return nil, fmt.Errorf("%w: artifact is required", ErrInvalid)
	}
	out := &Artifact{
		Traits:     map[string]*Trait{},
		Meta:       a.Meta,
		ProducedBy: a.ProducedBy,
		EntityId:   a.EntityId,
		Supersedes: a.Supersedes,
	}
	for _, c := range contracts {
		f, ok := a.Traits[c]
		if !ok {
			return nil, fmt.Errorf("%w: artifact missing trait %s", ErrInvalid, c)
		}
		out.Traits[c] = proto.Clone(f).(*Trait)
	}
	return out, nil
}

// MergeTraits unions two bags; add wins on collision. Sets Supersedes to base.Id.
func MergeTraits(base, add *Artifact) *Artifact {
	out := &Artifact{Traits: map[string]*Trait{}}
	if base != nil {
		for k, v := range base.Traits {
			out.Traits[k] = proto.Clone(v).(*Trait)
		}
		out.EntityId = base.EntityId
		if base.Id != "" {
			out.Supersedes = base.Id
		}
	}
	if add != nil {
		for k, v := range add.Traits {
			out.Traits[k] = proto.Clone(v).(*Trait)
		}
		if add.EntityId != "" {
			out.EntityId = add.EntityId
		}
		out.ProducedBy = add.ProducedBy
		out.Meta = add.Meta
	}
	return out
}

// HasExternalObject reports whether any trait holds a verified ObjectRef.
func HasExternalObject(a *Artifact) bool {
	if a == nil {
		return false
	}
	for _, f := range a.Traits {
		if TraitHasExternal(f) {
			return true
		}
	}
	return false
}

// ContractDigest is the content address of a Contract's schema identity:
//
//	digest = "sha256:" + hex(sha256(
//	  media_type ‖ 0x00 ‖ schema ‖ 0x00 ‖ version ‖ 0x00 ‖
//	  compatible_with… (UTF-8 ascending; each entry followed by 0x00)
//	))
//
// ref is the registry key and is NOT folded into the digest. Callers should
// pass compatible_with already sorted, or use ContractDigestOf which normalizes.
func ContractDigest(mediaType, schema, version string, compatibleWith []string) string {
	h := sha256.New()
	h.Write([]byte(mediaType))
	h.Write([]byte{sep})
	h.Write([]byte(schema))
	h.Write([]byte{sep})
	h.Write([]byte(version))
	h.Write([]byte{sep})
	for _, c := range compatibleWith {
		h.Write([]byte(c))
		h.Write([]byte{sep})
	}
	return "sha256:" + hex.EncodeToString(h.Sum(nil))
}

// ContractDigestOf computes the content digest for a Contract, sorting
// compatible_with ascending as raw UTF-8 before hashing.
func ContractDigestOf(c *Contract) string {
	if c == nil {
		return ContractDigest("", "", "", nil)
	}
	compat := append([]string(nil), c.CompatibleWith...)
	sortStringsUTF8(compat)
	return ContractDigest(c.MediaType, c.Schema, c.Version, compat)
}

// IsContractPlaceholder reports a name-only stub (empty content fields).
func IsContractPlaceholder(c *Contract) bool {
	return c != nil &&
		c.MediaType == "" && c.Schema == "" && c.Version == "" &&
		len(c.CompatibleWith) == 0
}

func sortStringsUTF8(ss []string) {
	// Insertion sort is fine — compatible_with lists are tiny.
	for i := 1; i < len(ss); i++ {
		j := i
		for j > 0 && ss[j-1] > ss[j] {
			ss[j-1], ss[j] = ss[j], ss[j-1]
			j--
		}
	}
}

// ledgerHash is sha256 over (seq, kind, subject, detail, prev_hash), each field
// delimited by a 0x00 separator, seq as 8 big-endian bytes.
func ledgerHash(seq uint64, kind, subject string, detail []byte, prevHash string) string {
	h := sha256.New()
	var seqBuf [8]byte
	binary.BigEndian.PutUint64(seqBuf[:], seq)
	h.Write(seqBuf[:])
	h.Write([]byte{sep})
	h.Write([]byte(kind))
	h.Write([]byte{sep})
	h.Write([]byte(subject))
	h.Write([]byte{sep})
	h.Write(detail)
	h.Write([]byte{sep})
	h.Write([]byte(prevHash))
	return hex.EncodeToString(h.Sum(nil))
}

// VerifyChain checks a ledger end-to-end: every entry's Hash must recompute
// from its own fields, and every PrevHash must equal the previous entry's Hash
// (genesis links to ""). Tampering with any committed entry breaks this.
func VerifyChain(entries []*LedgerEntry) bool {
	prev := ""
	for i, e := range entries {
		if e.Seq != uint64(i) || e.PrevHash != prev {
			return false
		}
		if ledgerHash(e.Seq, e.Kind, e.Subject, e.Detail, e.PrevHash) != e.Hash {
			return false
		}
		prev = e.Hash
	}
	return true
}
