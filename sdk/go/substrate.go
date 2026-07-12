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

	pb "github.com/philcantcode/srcport-substrate/sdk/go/internal/genpb/srcport/substrate/v1"
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
	Limits           = pb.Limits
	Assembly         = pb.Assembly
	RunRequest       = pb.RunRequest
	Run              = pb.Run
	RunRef           = pb.RunRef
	ClaimRequest     = pb.ClaimRequest
	WorkItem         = pb.WorkItem
	Derivation       = pb.Derivation
	DerivationList   = pb.DerivationList
	RequestContext    = pb.RequestContext
	Error             = pb.Error
	ErrorCode         = pb.ErrorCode
	SnapshotRequest   = pb.SnapshotRequest
	TransitionRequest = pb.TransitionRequest
	TransitionAck     = pb.TransitionAck

	Lifecycle = pb.Lifecycle
	RunState  = pb.RunState
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

// MaxInlineArtifactBytes is the advisory ceiling for Artifact.body. Larger
// payloads should PutBlob and place a verified ObjectRef on the Artifact.
// The kernel does not hard-reject oversized inline bodies (backward compat);
// modules and hosts SHOULD honour this boundary in production.
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


// ─── addressing & ledger hashing — the hash rules SPEC.md pins down ──────────

const sep = 0x00

// BlobID is pure content identity: "sha256:" + hex(sha256(data)). Namespace and
// typed Artifact fields are NOT part of blob identity.
func BlobID(data []byte) string {
	sum := sha256.Sum256(data)
	return "sha256:" + hex.EncodeToString(sum[:])
}

// ObjectRefBytes is the address payload for an external Artifact value:
//
//	digest ‖ 0x00 ‖ uint64_be(byte_count) ‖ 0x00 ‖ namespace
//
// This is what is hashed with type to form the Artifact id when object is set.
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

// ArtifactContent returns the bytes folded into the Artifact address: the
// inline body, or object_ref_bytes(object) when a verified external ref is set.
func ArtifactContent(a *Artifact) []byte {
	if a != nil && a.Object != nil && a.Object.Digest != "" {
		return ObjectRefBytes(a.Object)
	}
	if a == nil {
		return nil
	}
	return a.Body
}

// ArtifactID computes a content address over an explicit content payload:
//
//	id = "sha256:" + hex(sha256(type + 0x00 + content))
//
// For inline values pass body; for external values pass ObjectRefBytes(object).
// Prefer ArtifactIDOf when you have a full Artifact. Meta and ProducedBy are
// deliberately NOT part of the address.
func ArtifactID(typ string, content []byte) string {
	h := sha256.New()
	h.Write([]byte(typ))
	h.Write([]byte{sep})
	h.Write(content)
	return "sha256:" + hex.EncodeToString(h.Sum(nil))
}

// ArtifactIDOf is the typed value address for a full Artifact (inline or external).
func ArtifactIDOf(a *Artifact) string {
	return ArtifactID(a.Type, ArtifactContent(a))
}

// HasExternalObject reports whether a holds a verified external ObjectRef.
func HasExternalObject(a *Artifact) bool {
	return a != nil && a.Object != nil && a.Object.Digest != ""
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
