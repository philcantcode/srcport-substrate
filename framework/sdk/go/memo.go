package framework

import (
	"crypto/sha256"
	"encoding/hex"
	"sort"
	"sync"

	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

// MemoRecord is one cached production path (ids only).
type MemoRecord struct {
	Key           string
	Module        string
	ModuleVersion string
	ModuleDigest  string
	Capability    string
	NodeID        string
	Inputs        map[string]string // port → artifact id
	Outputs       map[string]string
	SourceRunID   string
	SourceWorkID  string
}

// MemoNodes filters which assembly nodes may use the memo store.
type MemoNodes struct {
	// Kind: "all" | "only" | "except"
	Kind string
	IDs  []string
}

func (m MemoNodes) Allows(nodeID string) bool {
	switch m.Kind {
	case "only":
		for _, id := range m.IDs {
			if id == nodeID {
				return true
			}
		}
		return false
	case "except":
		for _, id := range m.IDs {
			if id == nodeID {
				return false
			}
		}
		return true
	default:
		return true
	}
}

// MemoPlan is host-only memo policy.
type MemoPlan struct {
	Enabled       bool
	RequireDigest bool
	Nodes         MemoNodes
}

func MemoOff() MemoPlan { return MemoPlan{} }
func MemoOn() MemoPlan {
	return MemoPlan{Enabled: true, RequireDigest: true, Nodes: MemoNodes{Kind: "all"}}
}

// MemoStore indexes memo key → record.
type MemoStore interface {
	Get(key string) (*MemoRecord, error)
	Put(record MemoRecord) error
	Len() int
	Clear()
}

// MemoryMemo is a process-local memo store.
type MemoryMemo struct {
	mu   sync.Mutex
	data map[string]MemoRecord
}

func NewMemoryMemo() *MemoryMemo {
	return &MemoryMemo{data: map[string]MemoRecord{}}
}

func (m *MemoryMemo) Get(key string) (*MemoRecord, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r, ok := m.data[key]
	if !ok {
		return nil, nil
	}
	cp := r
	return &cp, nil
}

func (m *MemoryMemo) Put(record MemoRecord) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.data == nil {
		m.data = map[string]MemoRecord{}
	}
	m.data[record.Key] = record
	return nil
}

func (m *MemoryMemo) Len() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return len(m.data)
}

func (m *MemoryMemo) Clear() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.data = map[string]MemoRecord{}
}

// MemoKey builds the stable memo key for a work unit.
func MemoKey(module, moduleVersion, moduleDigest, capability string, inputs map[string]string) string {
	h := sha256.New()
	h.Write([]byte(module))
	h.Write([]byte{0})
	h.Write([]byte(moduleVersion))
	h.Write([]byte{0})
	h.Write([]byte(moduleDigest))
	h.Write([]byte{0})
	h.Write([]byte(capability))
	h.Write([]byte{0})
	keys := make([]string, 0, len(inputs))
	for k := range inputs {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, port := range keys {
		h.Write([]byte(port))
		h.Write([]byte{0})
		h.Write([]byte(inputs[port]))
		h.Write([]byte{0})
	}
	return "sha256:" + hex.EncodeToString(h.Sum(nil))
}

func InputFingerprintMap(work *substrate.WorkItem) map[string]string {
	m := map[string]string{}
	if work == nil {
		return m
	}
	for _, na := range work.Inputs {
		if na.Artifact != nil && na.Name != "" && na.Artifact.Id != "" {
			m[na.Name] = na.Artifact.Id
		}
	}
	return m
}

func RecordToNamedOutputs(record *MemoRecord) []*substrate.NamedArtifact {
	if record == nil {
		return nil
	}
	keys := make([]string, 0, len(record.Outputs))
	for k := range record.Outputs {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	out := make([]*substrate.NamedArtifact, 0, len(keys))
	for _, port := range keys {
		out = append(out, &substrate.NamedArtifact{
			Name:     port,
			Artifact: &substrate.ArtifactRef{Id: record.Outputs[port]},
		})
	}
	return out
}

func BuildRecord(key string, work *substrate.WorkItem, moduleDigest string, outputs []*substrate.NamedArtifact, runID string) MemoRecord {
	out := map[string]string{}
	for _, na := range outputs {
		if na.Artifact != nil && na.Name != "" && na.Artifact.Id != "" {
			out[na.Name] = na.Artifact.Id
		}
	}
	return MemoRecord{
		Key:           key,
		Module:        work.Module,
		ModuleVersion: work.ModuleVersion,
		ModuleDigest:  moduleDigest,
		Capability:    work.Capability,
		NodeID:        work.NodeId,
		Inputs:        InputFingerprintMap(work),
		Outputs:       out,
		SourceRunID:   runID,
		SourceWorkID:  work.Id,
	}
}
