package framework

import (
	"encoding/json"

	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

const (
	ContractStepInit     = "srcport.ui.v1.StepInit"
	ContractStepProgress = "srcport.ui.v1.StepProgress"
	ContractStepFinal    = "srcport.ui.v1.StepFinal"
	ContractStepSkipped  = "srcport.ui.v1.StepSkipped"
	ContractStepCached   = "srcport.ui.v1.StepCached"
)

// StepStage is the per-work-unit presentation lifecycle stage.
type StepStage string

const (
	StageInit     StepStage = "init"
	StageProgress StepStage = "progress"
	StageFinal    StepStage = "final"
	StageSkipped  StepStage = "skipped"
	StageCached   StepStage = "cached"
)

func (s StepStage) ContractRef() string {
	switch s {
	case StageInit:
		return ContractStepInit
	case StageProgress:
		return ContractStepProgress
	case StageFinal:
		return ContractStepFinal
	case StageSkipped:
		return ContractStepSkipped
	case StageCached:
		return ContractStepCached
	default:
		return ContractStepProgress
	}
}

// PresentationStatus is coarse chrome status.
type PresentationStatus string

const (
	StatusPending PresentationStatus = "pending"
	StatusRunning PresentationStatus = "running"
	StatusBlocked PresentationStatus = "blocked"
	StatusOk      PresentationStatus = "ok"
	StatusEmpty   PresentationStatus = "empty"
	StatusFailed  PresentationStatus = "failed"
)

// Presentation is structured step chrome — never UI toolkit types.
type Presentation struct {
	Stage          StepStage          `json:"stage"`
	Title          string             `json:"title"`
	Status         PresentationStatus `json:"status"`
	Detail         string             `json:"detail,omitempty"`
	Progress       *float64           `json:"progress,omitempty"`
	RunID          string             `json:"run_id,omitempty"`
	WorkID         string             `json:"work_id,omitempty"`
	NodeID         string             `json:"node_id,omitempty"`
	Module         string             `json:"module,omitempty"`
	Capability     string             `json:"capability,omitempty"`
	Phase          string             `json:"phase,omitempty"`
	HighlightPorts []string           `json:"highlight_ports,omitempty"`
	OutputPorts    []string           `json:"output_ports,omitempty"`
	Meta           map[string]string  `json:"meta,omitempty"`
}

func PresentationInit(title string) Presentation {
	return Presentation{Stage: StageInit, Title: title, Status: StatusRunning}
}

func PresentationProgress(title string, fraction *float64) Presentation {
	return Presentation{Stage: StageProgress, Title: title, Status: StatusRunning, Progress: fraction}
}

func PresentationFinalOk(title string) Presentation {
	one := 1.0
	return Presentation{Stage: StageFinal, Title: title, Status: StatusOk, Progress: &one}
}

func PresentationFinalFailed(title, detail string) Presentation {
	return Presentation{Stage: StageFinal, Title: title, Status: StatusFailed, Detail: detail}
}

func PresentationSkipped(title, detail string) Presentation {
	one := 1.0
	return Presentation{Stage: StageSkipped, Title: title, Status: StatusEmpty, Detail: detail, Progress: &one}
}

func PresentationCached(title, detail string) Presentation {
	one := 1.0
	return Presentation{Stage: StageCached, Title: title, Status: StatusOk, Detail: detail, Progress: &one}
}

func (p Presentation) WithDetail(d string) Presentation { p.Detail = d; return p }
func (p Presentation) WithPhase(ph string) Presentation { p.Phase = ph; return p }

func (p *Presentation) FillIdentity(runID string, work *substrate.WorkItem) {
	if p.RunID == "" {
		p.RunID = runID
	}
	if work == nil {
		return
	}
	if p.WorkID == "" {
		p.WorkID = work.Id
	}
	if p.NodeID == "" {
		p.NodeID = work.NodeId
	}
	if p.Module == "" {
		p.Module = work.Module
	}
	if p.Capability == "" {
		p.Capability = work.Capability
	}
}

// StepResult is the outcome of a domain step for OnFinal / OnStore.
type StepResult struct {
	OK      bool
	Outputs []*substrate.NamedArtifact
	Error   string
}

// StepEvent is one presentation emit observed by the host.
type StepEvent struct {
	Stage        StepStage
	Presentation Presentation
	ArtifactID   string
}

func marshalPresentation(p *Presentation) ([]byte, error) {
	return json.Marshal(p)
}
