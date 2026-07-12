// Package substrate is the in-process Go SDK for the srcport-substrate
// microkernel: seven primitives (Module · Artifact · Contract · Event ·
// Ledger · Gate · Registry) and one Kernel ABI, conformant to SPEC.md.
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

// ─── the seven primitives + ABI acks, aliased to the generated types ─────────
// These ARE the protobuf messages; construct with a pointer, e.g.
// &substrate.Artifact{Type: "…", Body: …}.

type (
	Capability       = pb.Capability
	ModuleManifest   = pb.ModuleManifest
	Artifact         = pb.Artifact
	ArtifactRef      = pb.ArtifactRef
	Contract         = pb.Contract
	Event            = pb.Event
	Subscription     = pb.Subscription
	LedgerEntry      = pb.LedgerEntry
	GateRequest      = pb.GateRequest
	GateDecision     = pb.GateDecision
	GateTicket       = pb.GateTicket
	RegistrySnapshot = pb.RegistrySnapshot
	RegisterAck      = pb.RegisterAck
	PublishAck       = pb.PublishAck
	AppendRequest    = pb.AppendRequest

	Lifecycle = pb.Lifecycle
	Decision  = pb.Decision
)

// Lifecycle values (REGISTERED → LOADED → ACTIVE → DEACTIVATED).
const (
	LifecycleUnspecified = pb.Lifecycle_LIFECYCLE_UNSPECIFIED
	LifecycleRegistered  = pb.Lifecycle_LIFECYCLE_REGISTERED
	LifecycleLoaded      = pb.Lifecycle_LIFECYCLE_LOADED
	LifecycleActive      = pb.Lifecycle_LIFECYCLE_ACTIVE
	LifecycleDeactivated = pb.Lifecycle_LIFECYCLE_DEACTIVATED
)

// Gate decisions.
const (
	DecisionUnspecified = pb.Decision_DECISION_UNSPECIFIED
	DecisionPending     = pb.Decision_DECISION_PENDING
	DecisionApproved    = pb.Decision_DECISION_APPROVED
	DecisionRejected    = pb.Decision_DECISION_REJECTED
)

// ─── addressing & ledger hashing — the two hash rules SPEC.md pins down ──────

const sep = 0x00

// ArtifactID computes the content address:
//
//	id = "sha256:" + hex(sha256(type + 0x00 + body))
//
// Same (type, body) ⇒ same id; a one-byte change ⇒ a different id. Meta and
// ProducedBy are deliberately NOT part of the address.
func ArtifactID(typ string, body []byte) string {
	h := sha256.New()
	h.Write([]byte(typ))
	h.Write([]byte{sep})
	h.Write(body)
	return "sha256:" + hex.EncodeToString(h.Sum(nil))
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
