package framework

import (
	"fmt"
	"sort"

	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

// SEED_INPUT_PREFIX is the synthetic run-input name prefix for cut edges.
const SEED_INPUT_PREFIX = "__seed/"

func SeedInputName(fromNode, fromPort string) string {
	return SEED_INPUT_PREFIX + fromNode + "/" + fromPort
}

func IsSeedInputName(name string) bool {
	return len(name) >= len(SEED_INPUT_PREFIX) && name[:len(SEED_INPUT_PREFIX)] == SEED_INPUT_PREFIX
}

// SeedSpec describes one binding rewritten from a dropped node into a run input.
type SeedSpec struct {
	InputName string
	FromNode  string
	FromPort  string
	ToNode    string
	ToPort    string
}

// SkippedNode is a node excluded from the materialised assembly.
type SkippedNode struct {
	NodeID         string
	Module         string
	Capability     string
	ModuleVersion  string
}

// AssemblyCut is the result of applying a NodePlan cut.
type AssemblyCut struct {
	Assembly       *substrate.Assembly
	Skipped        []SkippedNode
	RequiredSeeds  []SeedSpec
	KeptNodeIDs    []string
}

func ResolveKeptNodes(assembly *substrate.Assembly, plan NodePlan) (map[string]struct{}, error) {
	if assembly == nil || len(assembly.Nodes) == 0 {
		return nil, invalid("assembly has no nodes")
	}
	known := make(map[string]struct{}, len(assembly.Nodes))
	for _, n := range assembly.Nodes {
		known[n.Id] = struct{}{}
	}
	if assembly.Terminal == nil {
		return nil, invalid("assembly terminal is required")
	}
	if _, ok := known[assembly.Terminal.Node]; !ok {
		return nil, invalid(fmt.Sprintf("terminal node %s is not in the assembly", assembly.Terminal.Node))
	}

	kept := make(map[string]struct{})
	switch plan.Kind {
	case "all", "":
		for _, n := range assembly.Nodes {
			kept[n.Id] = struct{}{}
		}
	case "only":
		if len(plan.IDs) == 0 {
			return nil, invalid("NodePlan Only requires at least one node id")
		}
		seen := map[string]struct{}{}
		for _, id := range plan.IDs {
			if _, dup := seen[id]; dup {
				return nil, invalid(fmt.Sprintf("NodePlan Only contains duplicate node id %s", id))
			}
			seen[id] = struct{}{}
			if _, ok := known[id]; !ok {
				return nil, invalid(fmt.Sprintf("NodePlan Only references unknown node %s", id))
			}
			kept[id] = struct{}{}
		}
		if _, ok := kept[assembly.Terminal.Node]; !ok {
			return nil, invalid("node plan must retain the terminal node")
		}
	case "after":
		if _, ok := known[plan.Node]; !ok {
			return nil, invalid(fmt.Sprintf("NodePlan After references unknown node %s", plan.Node))
		}
		preds := transitivePredecessors(assembly, plan.Node)
		dropped := map[string]struct{}{plan.Node: {}}
		for p := range preds {
			dropped[p] = struct{}{}
		}
		if _, ok := dropped[assembly.Terminal.Node]; ok {
			return nil, invalid(fmt.Sprintf("NodePlan After(%s) would drop the terminal node", plan.Node))
		}
		for _, n := range assembly.Nodes {
			if _, d := dropped[n.Id]; !d {
				kept[n.Id] = struct{}{}
			}
		}
		if len(kept) == 0 {
			return nil, invalid("NodePlan After left no nodes to run")
		}
	case "from":
		if _, ok := known[plan.Node]; !ok {
			return nil, invalid(fmt.Sprintf("NodePlan From references unknown node %s", plan.Node))
		}
		reach := reachableFrom(assembly, plan.Node)
		if _, ok := reach[assembly.Terminal.Node]; !ok {
			return nil, invalid(fmt.Sprintf("NodePlan From(%s): terminal %s is not reachable from that node", plan.Node, assembly.Terminal.Node))
		}
		kept = reach
	default:
		return nil, invalid(fmt.Sprintf("unknown NodePlan kind %q", plan.Kind))
	}
	return kept, nil
}

// MaterializeCut drops nodes and rebinds crossing edges to __seed/… inputs.
func MaterializeCut(assembly *substrate.Assembly, plan NodePlan) (*AssemblyCut, error) {
	kept, err := ResolveKeptNodes(assembly, plan)
	if err != nil {
		return nil, err
	}
	if plan.Kind == "all" || plan.Kind == "" || len(kept) == len(assembly.Nodes) {
		ids := make([]string, 0, len(assembly.Nodes))
		for _, n := range assembly.Nodes {
			ids = append(ids, n.Id)
		}
		return &AssemblyCut{
			Assembly:    cloneAssembly(assembly),
			KeptNodeIDs: ids,
		}, nil
	}

	var skipped []SkippedNode
	for _, n := range assembly.Nodes {
		if _, ok := kept[n.Id]; !ok {
			skipped = append(skipped, SkippedNode{
				NodeID: n.Id, Module: n.Module, Capability: n.Capability, ModuleVersion: n.ModuleVersion,
			})
		}
	}

	seedByKey := map[string]SeedSpec{}
	var bindings []*substrate.Binding
	for _, b := range assembly.Bindings {
		if _, ok := kept[b.ToNode]; !ok {
			continue
		}
		if b.Input != "" {
			bindings = append(bindings, cloneBinding(b))
			continue
		}
		if b.FromNode == "" {
			return nil, invalid(fmt.Sprintf("binding to %s.%s has neither input nor from_node", b.ToNode, b.ToPort))
		}
		if _, ok := kept[b.FromNode]; ok {
			bindings = append(bindings, cloneBinding(b))
			continue
		}
		inputName := SeedInputName(b.FromNode, b.FromPort)
		key := b.FromNode + "\x00" + b.FromPort
		if _, exists := seedByKey[key]; !exists {
			seedByKey[key] = SeedSpec{
				InputName: inputName,
				FromNode:  b.FromNode,
				FromPort:  b.FromPort,
				ToNode:    b.ToNode,
				ToPort:    b.ToPort,
			}
		}
		bindings = append(bindings, &substrate.Binding{
			ToNode: b.ToNode, ToPort: b.ToPort, Input: inputName,
		})
	}

	var nodes []*substrate.AssemblyNode
	var keptIDs []string
	for _, n := range assembly.Nodes {
		if _, ok := kept[n.Id]; ok {
			nodes = append(nodes, cloneNode(n))
			keptIDs = append(keptIDs, n.Id)
		}
	}

	keys := make([]string, 0, len(seedByKey))
	for k := range seedByKey {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	seeds := make([]SeedSpec, 0, len(keys))
	for _, k := range keys {
		seeds = append(seeds, seedByKey[k])
	}

	return &AssemblyCut{
		Assembly: &substrate.Assembly{
			Id: assembly.Id, Nodes: nodes, Bindings: bindings, Terminal: assembly.Terminal,
		},
		Skipped:       skipped,
		RequiredSeeds: seeds,
		KeptNodeIDs:   keptIDs,
	}, nil
}

// derivationLister is satisfied by MemoryKernel and framework.Kernel.
type derivationLister interface {
	ListDerivations(ref *substrate.RunRef, ctx ...*substrate.RequestContext) (*substrate.DerivationList, error)
}

// SeedsFromRun collects seed NamedArtifacts from a prior run's latest derivations.
func SeedsFromRun(k derivationLister, runID string, cutNodes []string, ctx *substrate.RequestContext) ([]*substrate.NamedArtifact, error) {
	list, err := k.ListDerivations(&substrate.RunRef{Id: runID}, ctx)
	if err != nil {
		return nil, kernelErr(err)
	}
	want := map[string]struct{}{}
	for _, n := range cutNodes {
		want[n] = struct{}{}
	}
	if len(want) == 0 {
		return nil, nil
	}
	latest := map[string]*substrate.Derivation{}
	for _, d := range list.Derivations {
		if _, ok := want[d.NodeId]; ok {
			latest[d.NodeId] = d
		}
	}
	nodes := make([]string, 0, len(want))
	for n := range want {
		nodes = append(nodes, n)
	}
	sort.Strings(nodes)
	var out []*substrate.NamedArtifact
	seen := map[string]struct{}{}
	for _, nodeID := range nodes {
		d, ok := latest[nodeID]
		if !ok {
			return nil, invalid(fmt.Sprintf("seeds_from_run: no derivation for node %s on run %s", nodeID, runID))
		}
		for _, o := range d.Outputs {
			if o.Name == "" {
				continue
			}
			name := SeedInputName(nodeID, o.Name)
			if _, dup := seen[name]; dup {
				continue
			}
			seen[name] = struct{}{}
			if o.Artifact == nil {
				return nil, invalid(fmt.Sprintf("seeds_from_run: output %s.%s has no artifact ref", nodeID, o.Name))
			}
			out = append(out, &substrate.NamedArtifact{Name: name, Artifact: o.Artifact})
		}
	}
	return out, nil
}

// MergeInputs merges base with seeds; seeds win on name collision.
func MergeInputs(base, seeds []*substrate.NamedArtifact) []*substrate.NamedArtifact {
	by := map[string]*substrate.NamedArtifact{}
	order := []string{}
	for _, na := range base {
		if _, ok := by[na.Name]; !ok {
			order = append(order, na.Name)
		}
		by[na.Name] = na
	}
	for _, na := range seeds {
		if _, ok := by[na.Name]; !ok {
			order = append(order, na.Name)
		}
		by[na.Name] = na
	}
	sort.Strings(order)
	out := make([]*substrate.NamedArtifact, 0, len(order))
	for _, n := range order {
		out = append(out, by[n])
	}
	return out
}

// ValidateSeedsPresent fails closed if required seeds are missing.
func ValidateSeedsPresent(cut *AssemblyCut, inputs []*substrate.NamedArtifact) error {
	if cut == nil || len(cut.RequiredSeeds) == 0 {
		return nil
	}
	have := map[string]*substrate.NamedArtifact{}
	for _, i := range inputs {
		have[i.Name] = i
	}
	var missing []string
	for _, s := range cut.RequiredSeeds {
		na, ok := have[s.InputName]
		if !ok {
			missing = append(missing, fmt.Sprintf("%s (from %s.%s → %s.%s)", s.InputName, s.FromNode, s.FromPort, s.ToNode, s.ToPort))
			continue
		}
		if na.Artifact == nil {
			missing = append(missing, fmt.Sprintf("%s (present but artifact ref is empty)", s.InputName))
		}
	}
	if len(missing) == 0 {
		return nil
	}
	msg := "cut requires seed inputs that were not provided: "
	for i, m := range missing {
		if i > 0 {
			msg += "; "
		}
		msg += m
	}
	return invalid(msg)
}

func transitivePredecessors(assembly *substrate.Assembly, nodeID string) map[string]struct{} {
	incoming := map[string][]string{}
	for _, b := range assembly.Bindings {
		if b.FromNode == "" || b.ToNode == "" {
			continue
		}
		incoming[b.ToNode] = append(incoming[b.ToNode], b.FromNode)
	}
	out := map[string]struct{}{}
	q := []string{nodeID}
	visited := map[string]struct{}{nodeID: {}}
	for len(q) > 0 {
		cur := q[0]
		q = q[1:]
		for _, p := range incoming[cur] {
			if _, seen := out[p]; !seen {
				out[p] = struct{}{}
			}
			if _, v := visited[p]; !v {
				visited[p] = struct{}{}
				q = append(q, p)
			}
		}
	}
	return out
}

func reachableFrom(assembly *substrate.Assembly, nodeID string) map[string]struct{} {
	outgoing := map[string][]string{}
	for _, b := range assembly.Bindings {
		if b.FromNode == "" || b.ToNode == "" {
			continue
		}
		outgoing[b.FromNode] = append(outgoing[b.FromNode], b.ToNode)
	}
	out := map[string]struct{}{nodeID: {}}
	q := []string{nodeID}
	for len(q) > 0 {
		cur := q[0]
		q = q[1:]
		for _, n := range outgoing[cur] {
			if _, ok := out[n]; !ok {
				out[n] = struct{}{}
				q = append(q, n)
			}
		}
	}
	return out
}

func cloneAssembly(a *substrate.Assembly) *substrate.Assembly {
	if a == nil {
		return nil
	}
	out := &substrate.Assembly{Id: a.Id, Terminal: a.Terminal}
	for _, n := range a.Nodes {
		out.Nodes = append(out.Nodes, cloneNode(n))
	}
	for _, b := range a.Bindings {
		out.Bindings = append(out.Bindings, cloneBinding(b))
	}
	return out
}

func cloneNode(n *substrate.AssemblyNode) *substrate.AssemblyNode {
	return &substrate.AssemblyNode{
		Id: n.Id, Module: n.Module, ModuleVersion: n.ModuleVersion, Capability: n.Capability,
	}
}

func cloneBinding(b *substrate.Binding) *substrate.Binding {
	return &substrate.Binding{
		ToNode: b.ToNode, ToPort: b.ToPort, FromNode: b.FromNode, FromPort: b.FromPort, Input: b.Input,
	}
}
