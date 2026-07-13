package framework

import (
	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

// PortBody is one named output port value before PutArtifact.
type PortBody struct {
	Port     string
	Traits   map[string][]byte // contract ref → inline body
	EntityID string
}

func PortBodyTrait(port, contract string, body []byte) PortBody {
	return PortBody{
		Port:   port,
		Traits: map[string][]byte{contract: body},
	}
}

// StepOutput is the result of ModulePlugin.Execute.
type StepOutput struct {
	Outputs []PortBody
}

// StepContext is the context for one claimed work unit.
type StepContext struct {
	RunID       string
	Work        *substrate.WorkItem
	Inputs      map[string]*substrate.Artifact
	progressBuf []Presentation
}

// EmitProgress buffers a Progress presentation for the host.
func (s *StepContext) EmitProgress(p Presentation) {
	p.Stage = StageProgress
	if p.Status == "" || p.Status == StatusPending {
		p.Status = StatusRunning
	}
	p.FillIdentity(s.RunID, s.Work)
	s.progressBuf = append(s.progressBuf, p)
}

func (s *StepContext) takeProgress() []Presentation {
	out := s.progressBuf
	s.progressBuf = nil
	return out
}

// ModulePlugin is a domain module as a host-side plugin.
//
// Optional hooks may return nil. Implementers typically embed BasePlugin.
type ModulePlugin interface {
	Manifest() *substrate.ModuleManifest
	// ModuleDigest returns content identity for memoisation; empty = uncacheable.
	ModuleDigest() string
	Execute(step *StepContext) (*StepOutput, error)
	OnInit(step *StepContext) *Presentation
	OnFinal(step *StepContext, result *StepResult) *Presentation
	StorageSchema() *TableSchema
	OnStore(step *StepContext, result *StepResult) *StoreWrite
}

// BasePlugin provides no-op defaults for optional ModulePlugin methods.
type BasePlugin struct{}

func (BasePlugin) ModuleDigest() string                            { return "" }
func (BasePlugin) OnInit(*StepContext) *Presentation               { return nil }
func (BasePlugin) OnFinal(*StepContext, *StepResult) *Presentation { return nil }
func (BasePlugin) StorageSchema() *TableSchema                     { return nil }
func (BasePlugin) OnStore(*StepContext, *StepResult) *StoreWrite   { return nil }

// UiPersist controls whether presentations are PutArtifact'd onto the kernel.
type UiPersist int

const (
	UiLocalOnly UiPersist = iota
	UiArtifacts
)

// Kernel is the subset of substrate operations the host needs
// (includes InjectInput, which is on MemoryKernel but not the narrow KernelApi).
type Kernel interface {
	Register(m *substrate.ModuleManifest, ctx ...*substrate.RequestContext) *substrate.RegisterAck
	PutArtifact(a *substrate.Artifact, ctx ...*substrate.RequestContext) (*substrate.ArtifactRef, error)
	GetArtifact(ref *substrate.ArtifactRef, ctx ...*substrate.RequestContext) (*substrate.Artifact, error)
	StartRun(req *substrate.RunRequest, ctx ...*substrate.RequestContext) (*substrate.Run, error)
	InjectInput(req *substrate.InjectInputRequest, ctx ...*substrate.RequestContext) (*substrate.Run, error)
	ClaimReady(req *substrate.ClaimRequest, ctx ...*substrate.RequestContext) (*substrate.ClaimResponse, error)
	FailWork(req *substrate.FailWorkRequest, ctx ...*substrate.RequestContext) (*substrate.Run, error)
	Commit(submitted *substrate.Derivation, ctx ...*substrate.RequestContext) (*substrate.Run, error)
	GetRun(ref *substrate.RunRef, ctx ...*substrate.RequestContext) (*substrate.Run, error)
	CancelRun(ref *substrate.RunRef, ctx ...*substrate.RequestContext) (*substrate.Run, error)
	ListDerivations(ref *substrate.RunRef, ctx ...*substrate.RequestContext) (*substrate.DerivationList, error)
}
