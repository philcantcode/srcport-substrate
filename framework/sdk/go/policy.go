package framework

import (
	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

// RunMode is the product-facing run mode.
type RunMode int

const (
	RunModeConverge RunMode = iota
	RunModeStream
	RunModeDedupeStream
	RunModeSelective
	RunModeManual
)

// DEFAULT_CONCURRENCY is the host concurrency when policy leaves it unset.
const DEFAULT_CONCURRENCY uint32 = 8

// FiringPlan controls work-unit firing within a run.
type FiringPlan struct {
	// Kind: "defaults" | "all" | "map"
	Kind    string
	All     substrate.Firing
	Default substrate.Firing
	ByNode  map[string]substrate.Firing
}

func FiringCapabilityDefaults() FiringPlan { return FiringPlan{Kind: "defaults"} }
func FiringAll(f substrate.Firing) FiringPlan {
	return FiringPlan{Kind: "all", All: f}
}

// NodePlan selects which assembly nodes participate.
type NodePlan struct {
	// Kind: "all" | "only" | "after" | "from"
	Kind string
	IDs  []string
	Node string
}

func NodesAll() NodePlan               { return NodePlan{Kind: "all"} }
func NodesOnly(ids ...string) NodePlan { return NodePlan{Kind: "only", IDs: append([]string{}, ids...)} }
func NodesAfter(node string) NodePlan  { return NodePlan{Kind: "after", Node: node} }
func NodesFrom(node string) NodePlan   { return NodePlan{Kind: "from", Node: node} }

// DrivePlan is how Host.Drive schedules claims.
type DrivePlan int

const (
	DriveUntilIdle DrivePlan = iota
	DriveOnePass
	DriveUntilIdleThenWait
)

// DriveAfter controls re-drive after Inject.
type DriveAfter int

const (
	DriveAfterNo DriveAfter = iota
	DriveAfterUntilIdle
	DriveAfterOnePass
)

// FrameworkPolicy is the opinionated product API for one pipeline run.
//
// Compiles to kernel ExecutionPolicy, include_nodes, and Limits. Host-only
// fields (drive, claim_modules, storage, memo, concurrency) never enter the
// kernel except where mapped into Limits (in-flight / lease / attempts).
type FrameworkPolicy struct {
	Mode         RunMode
	Firing       FiringPlan
	Nodes        NodePlan
	MaxSteps     *uint64
	Drive        DrivePlan
	ClaimModules []string // nil = all plugins
	Storage      StoragePlan
	Memo         MemoPlan
	// ManualClosure used when Mode == RunModeManual.
	ManualClosure substrate.Closure
	// Concurrency is max parallel host workers (and kernel max_in_flight).
	// nil → DEFAULT_CONCURRENCY. Use &1 for strict serial drive.
	Concurrency *uint32
	// ClaimBatch is items per ClaimReady call. nil → effective concurrency.
	ClaimBatch *uint32
	// LeaseMs is kernel lease duration. nil → kernel default (60s).
	LeaseMs *uint64
	// MaxAttempts is kernel max claim attempts. nil → kernel default (3).
	MaxAttempts *uint32
}

func basePolicy(mode RunMode, firing FiringPlan, nodes NodePlan, drive DrivePlan) FrameworkPolicy {
	return FrameworkPolicy{
		Mode:    mode,
		Firing:  firing,
		Nodes:   nodes,
		Drive:   drive,
		Storage: StorageOff(),
		Memo:    MemoOff(),
	}
}

func Converge() FrameworkPolicy {
	return basePolicy(RunModeConverge, FiringCapabilityDefaults(), NodesAll(), DriveUntilIdle)
}

func Memoized() FrameworkPolicy {
	p := Converge()
	p.Memo = MemoOn()
	return p
}

func Stream() FrameworkPolicy {
	return basePolicy(RunModeStream, FiringAll(substrate.FiringAlways), NodesAll(), DriveUntilIdleThenWait)
}

func StreamDedupe() FrameworkPolicy {
	return basePolicy(RunModeDedupeStream, FiringAll(substrate.FiringOncePerKey), NodesAll(), DriveUntilIdleThenWait)
}

func Selective(nodeIDs ...string) FrameworkPolicy {
	return basePolicy(RunModeSelective, FiringCapabilityDefaults(), NodesOnly(nodeIDs...), DriveUntilIdle)
}

func StartAfter(node string) FrameworkPolicy {
	return basePolicy(RunModeSelective, FiringCapabilityDefaults(), NodesAfter(node), DriveUntilIdle)
}

func FromNode(node string) FrameworkPolicy {
	return basePolicy(RunModeSelective, FiringCapabilityDefaults(), NodesFrom(node), DriveUntilIdle)
}

func Manual(closure substrate.Closure) FrameworkPolicy {
	p := basePolicy(RunModeManual, FiringCapabilityDefaults(), NodesAll(), DriveUntilIdle)
	p.ManualClosure = closure
	return p
}

func (p FrameworkPolicy) WithFiring(f FiringPlan) FrameworkPolicy { p.Firing = f; return p }
func (p FrameworkPolicy) WithNodes(n NodePlan) FrameworkPolicy    { p.Nodes = n; return p }
func (p FrameworkPolicy) WithDrive(d DrivePlan) FrameworkPolicy   { p.Drive = d; return p }
func (p FrameworkPolicy) WithMaxSteps(n uint64) FrameworkPolicy   { p.MaxSteps = &n; return p }
func (p FrameworkPolicy) WithClaimModules(m ...string) FrameworkPolicy {
	p.ClaimModules = append([]string{}, m...)
	return p
}
func (p FrameworkPolicy) WithStorage(s StoragePlan) FrameworkPolicy { p.Storage = s; return p }
func (p FrameworkPolicy) WithMemo(m MemoPlan) FrameworkPolicy       { p.Memo = m; return p }

// WithConcurrency sets max parallel host workers (also kernel max_in_flight).
func (p FrameworkPolicy) WithConcurrency(n uint32) FrameworkPolicy {
	if n < 1 {
		n = 1
	}
	p.Concurrency = &n
	return p
}

// WithClaimBatch sets ClaimReady.max_items per host claim wave.
func (p FrameworkPolicy) WithClaimBatch(n uint32) FrameworkPolicy {
	if n < 1 {
		n = 1
	}
	p.ClaimBatch = &n
	return p
}

// WithLeaseMs sets the kernel work-unit lease duration in milliseconds.
func (p FrameworkPolicy) WithLeaseMs(ms uint64) FrameworkPolicy {
	p.LeaseMs = &ms
	return p
}

// WithMaxAttempts sets the kernel max claim attempts per work unit.
func (p FrameworkPolicy) WithMaxAttempts(n uint32) FrameworkPolicy {
	if n < 1 {
		n = 1
	}
	p.MaxAttempts = &n
	return p
}

// EffectiveConcurrency returns host concurrency (≥ 1).
func (p FrameworkPolicy) EffectiveConcurrency() uint32 {
	if p.Concurrency != nil {
		if *p.Concurrency < 1 {
			return 1
		}
		return *p.Concurrency
	}
	return DEFAULT_CONCURRENCY
}

// EffectiveClaimBatch returns claim batch size (≥ 1).
func (p FrameworkPolicy) EffectiveClaimBatch() uint32 {
	if p.ClaimBatch != nil {
		if *p.ClaimBatch < 1 {
			return 1
		}
		return *p.ClaimBatch
	}
	return p.EffectiveConcurrency()
}

func (p FrameworkPolicy) Closure() substrate.Closure {
	switch p.Mode {
	case RunModeStream, RunModeDedupeStream:
		return substrate.ClosureOpen
	case RunModeManual:
		return p.ManualClosure
	default:
		return substrate.ClosureFirstTerminal
	}
}

func (p FrameworkPolicy) NeedsCut() bool {
	return p.Nodes.Kind != "all"
}

func (p FrameworkPolicy) EffectiveDrive() DrivePlan {
	if p.Drive == DriveUntilIdleThenWait {
		return DriveUntilIdle
	}
	return p.Drive
}

func (p FrameworkPolicy) IncludeNodes() []string {
	if p.Nodes.Kind == "only" {
		return append([]string{}, p.Nodes.IDs...)
	}
	return nil
}

func (p FrameworkPolicy) ExecutionPolicyFor(assembly *substrate.Assembly) *substrate.ExecutionPolicy {
	ep := &substrate.ExecutionPolicy{Closure: p.Closure()}
	switch p.Firing.Kind {
	case "all":
		ep.Default = p.Firing.All
		if assembly != nil {
			ep.ByNode = make(map[string]substrate.Firing, len(assembly.Nodes))
			for _, n := range assembly.Nodes {
				ep.ByNode[n.Id] = p.Firing.All
			}
		}
	case "map":
		ep.Default = p.Firing.Default
		ep.ByNode = make(map[string]substrate.Firing, len(p.Firing.ByNode))
		for k, v := range p.Firing.ByNode {
			ep.ByNode[k] = v
		}
	default:
		ep.Default = substrate.FiringUnspecified
	}
	return ep
}

func (p FrameworkPolicy) ResolveMaxSteps(nodeCount uint64) uint64 {
	if p.MaxSteps != nil {
		return *p.MaxSteps
	}
	if p.Closure() == substrate.ClosureOpen {
		n := nodeCount * 10000
		if n < 10000 {
			n = 10000
		}
		return n
	}
	return 0
}

func (p FrameworkPolicy) ApplyToRunRequest(req *substrate.RunRequest) *substrate.RunRequest {
	var nodeCount uint64
	if req.Assembly != nil {
		nodeCount = uint64(len(req.Assembly.Nodes))
	}
	req.Policy = p.ExecutionPolicyFor(req.Assembly)
	req.IncludeNodes = p.IncludeNodes()
	var leaseMs uint64
	if p.LeaseMs != nil {
		leaseMs = *p.LeaseMs
	}
	var maxAttempts uint32
	if p.MaxAttempts != nil {
		maxAttempts = *p.MaxAttempts
	}
	req.Limits = &substrate.Limits{
		MaxSteps:       p.ResolveMaxSteps(nodeCount),
		MaxInFlight:    uint64(p.EffectiveConcurrency()),
		DefaultLeaseMs: leaseMs,
		MaxAttempts:    maxAttempts,
	}
	return req
}
