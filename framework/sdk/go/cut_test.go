package framework

import (
	"strings"
	"testing"

	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

func diamond() *substrate.Assembly {
	return &substrate.Assembly{
		Id: "d@1",
		Nodes: []*substrate.AssemblyNode{
			{Id: "extract", Module: "extractor", ModuleVersion: "1.0.0", Capability: "facts.extract"},
			{Id: "retrieve", Module: "retriever", ModuleVersion: "1.0.0", Capability: "sources.retrieve"},
			{Id: "write", Module: "writer", ModuleVersion: "1.0.0", Capability: "answer.write"},
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

func TestAfterExtractKeepsParallelBranchAndSeedsFacts(t *testing.T) {
	cut, err := MaterializeCut(diamond(), NodesAfter("extract"))
	if err != nil {
		t.Fatal(err)
	}
	if len(cut.KeptNodeIDs) != 2 || cut.KeptNodeIDs[0] != "retrieve" || cut.KeptNodeIDs[1] != "write" {
		t.Fatalf("kept=%v", cut.KeptNodeIDs)
	}
	if len(cut.Skipped) != 1 || cut.Skipped[0].NodeID != "extract" {
		t.Fatalf("skipped=%v", cut.Skipped)
	}
	if len(cut.RequiredSeeds) != 1 || cut.RequiredSeeds[0].InputName != "__seed/extract/facts" {
		t.Fatalf("seeds=%v", cut.RequiredSeeds)
	}
	found := false
	for _, b := range cut.Assembly.Bindings {
		if b.ToPort == "facts" && b.Input == "__seed/extract/facts" {
			found = true
		}
	}
	if !found {
		t.Fatal("facts binding not rewritten to seed")
	}
}

func TestFromWriteSeedsBothProducers(t *testing.T) {
	cut, err := MaterializeCut(diamond(), NodesFrom("write"))
	if err != nil {
		t.Fatal(err)
	}
	if len(cut.KeptNodeIDs) != 1 || cut.KeptNodeIDs[0] != "write" {
		t.Fatalf("kept=%v", cut.KeptNodeIDs)
	}
	names := map[string]bool{}
	for _, s := range cut.RequiredSeeds {
		names[s.InputName] = true
	}
	if !names["__seed/extract/facts"] || !names["__seed/retrieve/sources"] {
		t.Fatalf("seeds=%v", names)
	}
}

func TestAfterTerminalRejected(t *testing.T) {
	_, err := MaterializeCut(diamond(), NodesAfter("write"))
	if err == nil || !strings.Contains(err.Error(), "terminal") {
		t.Fatalf("expected terminal error, got %v", err)
	}
}

func TestMemoKeyStable(t *testing.T) {
	a := map[string]string{"b": "id2", "a": "id1"}
	b := map[string]string{"a": "id1", "b": "id2"}
	if MemoKey("m", "1", "d", "cap", a) != MemoKey("m", "1", "d", "cap", b) {
		t.Fatal("key should be order-independent")
	}
}
