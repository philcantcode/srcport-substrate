// Package substrate is the in-process Go SDK for the srcport-substrate
// microkernel: seven primitives (Module · Artifact · Contract · Event ·
// Ledger · Gate · Registry) and one Kernel ABI, conformant to SPEC.md.
//
// The types below are a faithful hand-port of the canonical contract in
// contracts/proto/srcport/substrate/v1/substrate.proto — that proto remains
// the single source of truth. Field names and numbering mirror it; do not
// re-derive the core, widen the proto and follow it.
package substrate

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
)

// ─── enums ──────────────────────────────────────────────────────────────────

type Lifecycle int32

const (
	LifecycleUnspecified Lifecycle = 0
	LifecycleRegistered  Lifecycle = 1
	LifecycleLoaded      Lifecycle = 2
	LifecycleActive      Lifecycle = 3
	LifecycleDeactivated Lifecycle = 4
)

type Decision int32

const (
	DecisionUnspecified Decision = 0
	DecisionPending     Decision = 1
	DecisionApproved    Decision = 2
	DecisionRejected    Decision = 3
)

func (d Decision) String() string {
	switch d {
	case DecisionPending:
		return "PENDING"
	case DecisionApproved:
		return "APPROVED"
	case DecisionRejected:
		return "REJECTED"
	default:
		return "UNSPECIFIED"
	}
}

// ─── 1. Module ──────────────────────────────────────────────────────────────

// Capability is a named thing a module can do, bound to the contract it speaks.
type Capability struct {
	Name     string // e.g. "recon.scan"
	Contract string // contract ref, e.g. "acme.recon.v1.ScanRequest"
}

// ModuleManifest declares what a module provides and requires. It never imports
// another module.
type ModuleManifest struct {
	Name     string
	Version  string
	Provides []Capability
	Requires []string // capability names that must be present
}

// ─── 2. Artifact ────────────────────────────────────────────────────────────

// Artifact is a typed, content-addressed, immutable value that flows between
// modules. The kernel never parses Body; Type (a contract ref) says what it is.
type Artifact struct {
	ID         string // content address, assigned by the kernel
	Type       string // contract ref describing Body
	Body       []byte // opaque encoded value
	Meta       map[string]string
	ProducedBy string // module name
}

type ArtifactRef struct{ ID string }

// ─── 3. Contract ────────────────────────────────────────────────────────────

// Contract is the declarative schema that is the sole coupling point.
type Contract struct {
	Ref    string // fully-qualified name, e.g. "acme.recon.v1.Host"
	Schema string // schema text (proto / JSON Schema); may be empty
}

// ─── 4. Event ───────────────────────────────────────────────────────────────

// Event is a bus message. Modules publish/subscribe to topics; Seq is a total
// order assigned by the kernel.
type Event struct {
	ID      string
	Topic   string // dotted, e.g. "recon.host.found"
	Type    string // contract ref of payload
	Payload []byte
	Source  string // module name
	Seq     uint64 // kernel-assigned, monotonic
}

type Subscription struct {
	Module string
	Topics []string
}

// ─── 5. Ledger ──────────────────────────────────────────────────────────────

// LedgerEntry is one link in the append-only, hash-chained record.
type LedgerEntry struct {
	Seq      uint64
	Kind     string // "module.registered", "artifact.put", "event.published", …
	Subject  string // id of the thing this entry is about
	Detail   []byte
	PrevHash string // hash of entry seq-1 ("" for genesis)
	Hash     string // sha256 over (seq, kind, subject, detail, prev_hash)
}

// ─── 6. Gate ────────────────────────────────────────────────────────────────

type GateRequest struct {
	ID          string
	Action      string // human-readable description of the irreversible act
	Context     []byte // evidence the human decides on
	RequestedBy string // module name
}

type GateDecision struct {
	RequestID string
	Decision  Decision
	DecidedBy string // human identity
	Reason    string
}

type GateTicket struct{ RequestID string }

// ─── 7. Registry ────────────────────────────────────────────────────────────

type RegistrySnapshot struct {
	Modules      []ModuleManifest
	Capabilities []Capability
	Contracts    []Contract
}

// ─── ABI acks ───────────────────────────────────────────────────────────────

type RegisterAck struct{ State Lifecycle }
type PublishAck struct{ Seq uint64 }
type AppendRequest struct {
	Kind    string
	Subject string
	Detail  []byte
}

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
func VerifyChain(entries []LedgerEntry) bool {
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
