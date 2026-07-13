package framework_test

import (
	"fmt"
	"testing"

	framework "github.com/philcantcode/srcport-substrate/framework/sdk/go"
	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

type extractor struct{ framework.BasePlugin }

func (e *extractor) Manifest() *substrate.ModuleManifest {
	return &substrate.ModuleManifest{
		Name: "extractor", Version: "1.0.0",
		Provides: []*substrate.Capability{{
			Name:    "facts.extract",
			Inputs:  []*substrate.Port{{Name: "question", Traits: []string{"demo.v1.Question"}}},
			Outputs: []*substrate.Port{{Name: "facts", Traits: []string{"demo.v1.Facts"}}},
		}},
	}
}
func (e *extractor) ModuleDigest() string { return "extract-v1" }
func (e *extractor) Execute(step *framework.StepContext) (*framework.StepOutput, error) {
	q := step.Inputs["question"]
	body := []byte{}
	if q != nil {
		for _, t := range q.Traits {
			body = t.Body
			break
		}
	}
	half := 0.5
	step.EmitProgress(framework.PresentationProgress("Extracting facts", &half).WithDetail("Reading question…"))
	facts := []byte(fmt.Sprintf("facts-from:%s", body))
	return &framework.StepOutput{Outputs: []framework.PortBody{
		framework.PortBodyTrait("facts", "demo.v1.Facts", facts),
	}}, nil
}
func (e *extractor) OnInit(*framework.StepContext) *framework.Presentation {
	p := framework.PresentationInit("Extracting facts").WithDetail("Starting…")
	return &p
}
func (e *extractor) OnFinal(*framework.StepContext, *framework.StepResult) *framework.Presentation {
	p := framework.PresentationFinalOk("Facts ready").WithDetail("Extracted facts")
	return &p
}

type retriever struct{ framework.BasePlugin }

func (r *retriever) Manifest() *substrate.ModuleManifest {
	return &substrate.ModuleManifest{
		Name: "retriever", Version: "1.0.0",
		Provides: []*substrate.Capability{{
			Name:    "sources.retrieve",
			Inputs:  []*substrate.Port{{Name: "question", Traits: []string{"demo.v1.Question"}}},
			Outputs: []*substrate.Port{{Name: "sources", Traits: []string{"demo.v1.Sources"}}},
		}},
	}
}
func (r *retriever) ModuleDigest() string { return "retrieve-v1" }
func (r *retriever) Execute(*framework.StepContext) (*framework.StepOutput, error) {
	return &framework.StepOutput{Outputs: []framework.PortBody{
		framework.PortBodyTrait("sources", "demo.v1.Sources", []byte("SPEC.md")),
	}}, nil
}

type writer struct{ framework.BasePlugin }

func (w *writer) Manifest() *substrate.ModuleManifest {
	return &substrate.ModuleManifest{
		Name: "writer", Version: "2.0.0",
		Provides: []*substrate.Capability{{
			Name: "answer.write",
			Inputs: []*substrate.Port{
				{Name: "question", Traits: []string{"demo.v1.Question"}},
				{Name: "facts", Traits: []string{"demo.v1.Facts"}},
				{Name: "sources", Traits: []string{"demo.v1.Sources"}},
			},
			Outputs: []*substrate.Port{{Name: "answer", Traits: []string{"demo.v1.Answer"}}},
		}},
	}
}
func (w *writer) ModuleDigest() string { return "write-v1" }
func (w *writer) Execute(step *framework.StepContext) (*framework.StepOutput, error) {
	var facts, sources []byte
	if a := step.Inputs["facts"]; a != nil {
		for _, t := range a.Traits {
			facts = t.Body
			break
		}
	}
	if a := step.Inputs["sources"]; a != nil {
		for _, t := range a.Traits {
			sources = t.Body
			break
		}
	}
	body := append([]byte("answer:"), facts...)
	body = append(body, '+')
	body = append(body, sources...)
	return &framework.StepOutput{Outputs: []framework.PortBody{
		framework.PortBodyTrait("answer", "demo.v1.Answer", body),
	}}, nil
}
func (w *writer) OnInit(*framework.StepContext) *framework.Presentation {
	p := framework.PresentationInit("Writing answer")
	return &p
}
func (w *writer) OnFinal(*framework.StepContext, *framework.StepResult) *framework.Presentation {
	p := framework.PresentationFinalOk("Answer ready")
	return &p
}

func diamondAssembly() *substrate.Assembly {
	return &substrate.Assembly{
		Id: "answer-pipeline@1",
		Nodes: []*substrate.AssemblyNode{
			{Id: "extract", Module: "extractor", ModuleVersion: "1.0.0", Capability: "facts.extract"},
			{Id: "retrieve", Module: "retriever", ModuleVersion: "1.0.0", Capability: "sources.retrieve"},
			{Id: "write", Module: "writer", ModuleVersion: "2.0.0", Capability: "answer.write"},
		},
		Bindings: []*substrate.Binding{
			{ToNode: "extract", ToPort: "question", Input: "question"},
			{ToNode: "retrieve", ToPort: "question", Input: "question"},
			{ToNode: "write", ToPort: "question", Input: "question"},
			{ToNode: "write", ToPort: "facts", FromNode: "extract", FromPort: "facts"},
			{ToNode: "write", ToPort: "sources", FromNode: "retrieve", FromPort: "sources"},
		},
		Terminal: &substrate.NodeOutput{Node: "write", Port: "answer"},
	}
}

func TestHostDrivesDiamondWithStepLifecycle(t *testing.T) {
	k := substrate.NewMemoryKernel()
	host := framework.NewHost(k).WithUiPersist(framework.UiArtifacts)
	if err := host.RegisterPlugin(&extractor{}); err != nil {
		t.Fatal(err)
	}
	if err := host.RegisterPlugin(&retriever{}); err != nil {
		t.Fatal(err)
	}
	if err := host.RegisterPlugin(&writer{}); err != nil {
		t.Fatal(err)
	}

	qArt := substrate.ArtifactWithTrait("demo.v1.Question", []byte("What is substrate?"))
	qArt.ProducedBy = "operator"
	question, err := k.PutArtifact(qArt)
	if err != nil {
		t.Fatal(err)
	}

	run, err := host.StartPipeline("run-1", diamondAssembly(), []*substrate.NamedArtifact{
		{Name: "question", Artifact: question},
	}, framework.Converge())
	if err != nil {
		t.Fatal(err)
	}
	if run.State != substrate.RunStateRunning {
		t.Fatalf("state=%v", run.State)
	}

	done, err := host.Drive("run-1")
	if err != nil {
		t.Fatal(err)
	}
	if done.State != substrate.RunStateCompleted {
		t.Fatalf("expected completed, got %v", done.State)
	}
	if done.Answer == nil || done.Answer.Id == "" {
		t.Fatal("missing terminal answer")
	}

	events := host.TakeStepEvents()
	stages := map[framework.StepStage]int{}
	for _, e := range events {
		stages[e.Stage]++
	}
	if stages[framework.StageInit] < 1 || stages[framework.StageFinal] < 1 {
		t.Fatalf("expected init/final events, got %v", stages)
	}
	if host.ExecuteCount() != 3 {
		t.Fatalf("execute_count=%d", host.ExecuteCount())
	}
}

func TestMemoizedSecondRunHitsCache(t *testing.T) {
	k := substrate.NewMemoryKernel()
	host := framework.NewHost(k).WithMemo(framework.NewMemoryMemo())
	_ = host.RegisterPlugin(&extractor{})
	_ = host.RegisterPlugin(&retriever{})
	_ = host.RegisterPlugin(&writer{})

	qArt := substrate.ArtifactWithTrait("demo.v1.Question", []byte("memo me"))
	question, _ := k.PutArtifact(qArt)
	inputs := []*substrate.NamedArtifact{{Name: "question", Artifact: question}}

	if _, err := host.StartPipeline("r1", diamondAssembly(), inputs, framework.Memoized()); err != nil {
		t.Fatal(err)
	}
	if _, err := host.Drive("r1"); err != nil {
		t.Fatal(err)
	}
	firstExec := host.ExecuteCount()
	if firstExec != 3 {
		t.Fatalf("first exec=%d", firstExec)
	}

	if _, err := host.StartPipeline("r2", diamondAssembly(), inputs, framework.Memoized()); err != nil {
		t.Fatal(err)
	}
	if _, err := host.Drive("r2"); err != nil {
		t.Fatal(err)
	}
	if host.ExecuteCount() != firstExec {
		t.Fatalf("second run should skip execute: %d vs %d", host.ExecuteCount(), firstExec)
	}
	if host.MemoHitCount() != 3 {
		t.Fatalf("memo hits=%d", host.MemoHitCount())
	}
}
