package framework

import (
	"fmt"
	"sort"

	substrate "github.com/philcantcode/srcport-substrate/kernel/sdk/go"
)

// Host is the opinionated driver around any Kernel backend.
type Host struct {
	kernel         Kernel
	plugins        map[string]ModulePlugin
	ctx            *substrate.RequestContext
	uiPersist      UiPersist
	stepEvents     []StepEvent
	runPolicies    map[string]FrameworkPolicy
	storage        StorageBackend
	storageSchemas map[string]TableSchema
	runTables      map[string][]string
	memo           MemoStore
	executeCount   uint64
	memoHitCount   uint64
}

// NewHost creates a host over a kernel backend.
func NewHost(k Kernel) *Host {
	return &Host{
		kernel: k,
		plugins: make(map[string]ModulePlugin),
		ctx: &substrate.RequestContext{Caller: "srcport-framework"},
		uiPersist:      UiLocalOnly,
		runPolicies:    make(map[string]FrameworkPolicy),
		storageSchemas: make(map[string]TableSchema),
		runTables:      make(map[string][]string),
	}
}

func (h *Host) WithContext(ctx *substrate.RequestContext) *Host {
	h.ctx = ctx
	return h
}
func (h *Host) WithUiPersist(mode UiPersist) *Host {
	h.uiPersist = mode
	return h
}
func (h *Host) WithStorage(b StorageBackend) *Host {
	h.storage = b
	return h
}
func (h *Host) WithMemo(s MemoStore) *Host {
	h.memo = s
	return h
}

func (h *Host) Kernel() Kernel                  { return h.kernel }
func (h *Host) MemoStore() MemoStore            { return h.memo }
func (h *Host) Storage() StorageBackend         { return h.storage }
func (h *Host) ExecuteCount() uint64            { return h.executeCount }
func (h *Host) MemoHitCount() uint64            { return h.memoHitCount }
func (h *Host) Policy(runID string) (FrameworkPolicy, bool) {
	p, ok := h.runPolicies[runID]
	return p, ok
}
func (h *Host) StepEvents() []StepEvent { return h.stepEvents }
func (h *Host) TakeStepEvents() []StepEvent {
	out := h.stepEvents
	h.stepEvents = nil
	return out
}

// RegisterPlugin registers a plugin on the kernel and host.
func (h *Host) RegisterPlugin(plugin ModulePlugin) error {
	m := plugin.Manifest()
	if m == nil || m.Name == "" {
		return invalid("plugin manifest.name must be non-empty")
	}
	if _, exists := h.plugins[m.Name]; exists {
		return invalid(fmt.Sprintf("plugin already registered: %s", m.Name))
	}
	if schema := plugin.StorageSchema(); schema != nil {
		if schema.Name == "" {
			return invalid(fmt.Sprintf("plugin %s storage_schema.name must be non-empty", m.Name))
		}
		h.storageSchemas[m.Name] = *schema
	}
	h.kernel.Register(m, h.ctx)
	h.plugins[m.Name] = plugin
	return nil
}

// StartPipeline starts a run with an opinionated FrameworkPolicy.
func (h *Host) StartPipeline(runID string, assembly *substrate.Assembly, inputs []*substrate.NamedArtifact, policy FrameworkPolicy) (*substrate.Run, error) {
	if policy.Mode == RunModeSelective && policy.Nodes.Kind == "all" {
		return nil, invalid("RunMode Selective requires NodePlan Only, After, or From")
	}
	if runID == "" {
		return nil, invalid("run_id must be non-empty")
	}
	if _, exists := h.runPolicies[runID]; exists {
		return nil, invalid(fmt.Sprintf("pipeline policy already registered for run_id %s", runID))
	}
	if policy.Storage.Enabled() && h.storage == nil {
		return nil, invalid("StoragePlan enabled but host has no StorageBackend (use WithStorage)")
	}
	if policy.Memo.Enabled && h.memo == nil {
		return nil, invalid("MemoPlan enabled but host has no MemoStore (use WithMemo)")
	}

	cut, err := MaterializeCut(assembly, policy.Nodes)
	if err != nil {
		return nil, err
	}
	if err := ValidateSeedsPresent(cut, inputs); err != nil {
		return nil, err
	}
	if err := h.emitSkipEvents(runID, cut); err != nil {
		return nil, err
	}

	kernelPolicy := policy
	kernelPolicy.Nodes = NodesAll()
	req := kernelPolicy.ApplyToRunRequest(&substrate.RunRequest{
		Id: runID, Assembly: cut.Assembly, Inputs: inputs,
	})
	run, err := h.kernel.StartRun(req, h.ctx)
	if err != nil {
		return nil, kernelErr(err)
	}
	if err := h.ensureRunStorage(runID, policy); err != nil {
		return nil, err
	}
	h.runPolicies[runID] = policy
	return run, nil
}

// ResumeAfter seeds cut nodes from a prior run and starts a new pipeline.
func (h *Host) ResumeAfter(newRunID, priorRunID, afterNode string, policy FrameworkPolicy) (*substrate.Run, error) {
	prior, err := h.GetRun(priorRunID)
	if err != nil {
		return nil, err
	}
	if prior.Assembly == nil {
		return nil, invalid("prior run has no assembly")
	}
	policy.Nodes = NodesAfter(afterNode)
	cut, err := MaterializeCut(prior.Assembly, policy.Nodes)
	if err != nil {
		return nil, err
	}
	cutNodes := make([]string, 0, len(cut.Skipped)+1)
	for _, s := range cut.Skipped {
		cutNodes = append(cutNodes, s.NodeID)
	}
	found := false
	for _, n := range cutNodes {
		if n == afterNode {
			found = true
			break
		}
	}
	if !found {
		cutNodes = append(cutNodes, afterNode)
	}
	seeds, err := SeedsFromRun(h.kernel, priorRunID, cutNodes, h.ctx)
	if err != nil {
		return nil, err
	}
	var base []*substrate.NamedArtifact
	for _, i := range prior.Inputs {
		if !IsSeedInputName(i.Name) {
			base = append(base, i)
		}
	}
	inputs := MergeInputs(base, seeds)
	return h.StartPipeline(newRunID, prior.Assembly, inputs, policy)
}

func (h *Host) StartRun(req *substrate.RunRequest) (*substrate.Run, error) {
	run, err := h.kernel.StartRun(req, h.ctx)
	return run, kernelErr(err)
}

func (h *Host) GetRun(runID string) (*substrate.Run, error) {
	run, err := h.kernel.GetRun(&substrate.RunRef{Id: runID}, h.ctx)
	return run, kernelErr(err)
}

func (h *Host) Inject(runID string, input *substrate.NamedArtifact, after DriveAfter) (*substrate.Run, error) {
	run, err := h.kernel.InjectInput(&substrate.InjectInputRequest{RunId: runID, Input: input}, h.ctx)
	if err != nil {
		return nil, kernelErr(err)
	}
	switch after {
	case DriveAfterUntilIdle:
		return h.DriveWith(runID, DriveUntilIdle)
	case DriveAfterOnePass:
		return h.DriveWith(runID, DriveOnePass)
	default:
		return run, nil
	}
}

func (h *Host) Cancel(runID string) (*substrate.Run, error) {
	run, err := h.kernel.CancelRun(&substrate.RunRef{Id: runID}, h.ctx)
	if err != nil {
		return nil, kernelErr(err)
	}
	h.finishRunStorage(runID)
	return run, nil
}

func (h *Host) Drive(runID string) (*substrate.Run, error) {
	plan := DriveUntilIdle
	if p, ok := h.runPolicies[runID]; ok {
		plan = p.EffectiveDrive()
	}
	return h.DriveWith(runID, plan)
}

func (h *Host) DriveWith(runID string, plan DrivePlan) (*substrate.Run, error) {
	if plan == DriveUntilIdleThenWait {
		plan = DriveUntilIdle
	}
	var run *substrate.Run
	var err error
	if plan == DriveOnePass {
		run, err = h.driveOnePass(runID)
	} else {
		run, err = h.driveUntilIdle(runID)
	}
	if err == nil && run != nil && run.State != substrate.RunStateRunning {
		h.finishRunStorage(runID)
	}
	return run, err
}

func (h *Host) claimModuleNames(runID string) []string {
	all := make([]string, 0, len(h.plugins))
	for name := range h.plugins {
		all = append(all, name)
	}
	sort.Strings(all)
	p, ok := h.runPolicies[runID]
	if !ok || p.ClaimModules == nil {
		return all
	}
	allow := map[string]struct{}{}
	for _, m := range p.ClaimModules {
		allow[m] = struct{}{}
	}
	var out []string
	for _, m := range all {
		if _, ok := allow[m]; ok {
			out = append(out, m)
		}
	}
	return out
}

func (h *Host) driveUntilIdle(runID string) (*substrate.Run, error) {
	for {
		run, err := h.GetRun(runID)
		if err != nil {
			return nil, err
		}
		if run.State != substrate.RunStateRunning {
			return run, nil
		}
		progressed := false
		for _, module := range h.claimModuleNames(runID) {
			run, err = h.GetRun(runID)
			if err != nil {
				return nil, err
			}
			if run.State != substrate.RunStateRunning {
				return run, nil
			}
			ok, err := h.TryStep(runID, module)
			if err != nil {
				return nil, err
			}
			if ok {
				progressed = true
			}
		}
		if !progressed {
			return h.GetRun(runID)
		}
	}
}

func (h *Host) driveOnePass(runID string) (*substrate.Run, error) {
	run, err := h.GetRun(runID)
	if err != nil {
		return nil, err
	}
	if run.State != substrate.RunStateRunning {
		return run, nil
	}
	for _, module := range h.claimModuleNames(runID) {
		run, err = h.GetRun(runID)
		if err != nil {
			return nil, err
		}
		if run.State != substrate.RunStateRunning {
			return run, nil
		}
		if _, err := h.TryStep(runID, module); err != nil {
			return nil, err
		}
	}
	return h.GetRun(runID)
}

// TryStep claims → (memo hit | init → execute → final) → put/commit.
func (h *Host) TryStep(runID, module string) (bool, error) {
	work, err := h.kernel.ClaimReady(&substrate.ClaimRequest{RunId: runID, Module: module}, h.ctx)
	if err != nil {
		return false, kernelErr(err)
	}
	if work == nil || work.Id == "" {
		return false, nil
	}
	step, err := h.loadStep(runID, work)
	if err != nil {
		return false, err
	}

	if key, named, sourceRun, hit := h.tryMemoHit(runID, module, work); hit {
		return h.commitMemoHit(runID, module, work, step, key, named, sourceRun)
	}

	plugin, ok := h.plugins[module]
	if !ok {
		return false, noPlugin(module)
	}
	if init := plugin.OnInit(step); init != nil {
		p := *init
		p.Stage = StageInit
		p.FillIdentity(runID, work)
		if err := h.emitPresentation(module, p); err != nil {
			return false, err
		}
	}

	out, execErr := plugin.Execute(step)
	h.executeCount++
	for _, p := range step.takeProgress() {
		if err := h.emitPresentation(module, p); err != nil {
			return false, err
		}
	}

	if execErr != nil {
		msg := execErr.Error()
		sr := &StepResult{OK: false, Error: msg}
		final := plugin.OnFinal(step, sr)
		if final == nil {
			f := PresentationFinalFailed("Step failed", msg)
			final = &f
		}
		p := *final
		p.Stage = StageFinal
		p.Status = StatusFailed
		p.FillIdentity(runID, work)
		_ = h.emitPresentation(module, p)
		_ = h.applyStepStorage(runID, module, step, sr)
		return false, stepFailed(msg)
	}

	var named []*substrate.NamedArtifact
	for _, pb := range out.Outputs {
		traits := map[string]*substrate.Trait{}
		for c, body := range pb.Traits {
			traits[c] = &substrate.Trait{Body: body}
		}
		ref, err := h.kernel.PutArtifact(&substrate.Artifact{
			Traits: traits, ProducedBy: module, EntityId: pb.EntityID,
		}, h.ctx)
		if err != nil {
			return false, kernelErr(err)
		}
		named = append(named, &substrate.NamedArtifact{Name: pb.Port, Artifact: ref})
	}

	sr := &StepResult{OK: true, Outputs: named}
	if final := plugin.OnFinal(step, sr); final != nil {
		p := *final
		p.Stage = StageFinal
		p.FillIdentity(runID, work)
		if len(p.OutputPorts) == 0 {
			for _, o := range named {
				p.OutputPorts = append(p.OutputPorts, o.Name)
			}
		}
		if err := h.emitPresentation(module, p); err != nil {
			return false, err
		}
	}

	if _, err := h.kernel.Commit(&substrate.Derivation{
		RunId: runID, WorkId: work.Id, NodeId: work.NodeId, Outputs: named,
	}, h.ctx); err != nil {
		return false, kernelErr(err)
	}
	if err := h.storeMemoAfterSuccess(runID, module, work, named); err != nil {
		return false, err
	}
	if err := h.applyStepStorage(runID, module, step, sr); err != nil {
		return false, err
	}
	return true, nil
}

func (h *Host) tryMemoHit(runID, module string, work *substrate.WorkItem) (string, []*substrate.NamedArtifact, string, bool) {
	policy, ok := h.runPolicies[runID]
	if !ok || !policy.Memo.Enabled || h.memo == nil {
		return "", nil, "", false
	}
	if !policy.Memo.Nodes.Allows(work.NodeId) {
		return "", nil, "", false
	}
	plugin, ok := h.plugins[module]
	if !ok {
		return "", nil, "", false
	}
	digest := plugin.ModuleDigest()
	if digest == "" {
		return "", nil, "", false
	}
	inputs := InputFingerprintMap(work)
	key := MemoKey(module, work.ModuleVersion, digest, work.Capability, inputs)
	rec, err := h.memo.Get(key)
	if err != nil || rec == nil {
		return "", nil, "", false
	}
	named := RecordToNamedOutputs(rec)
	for _, na := range named {
		if na.Artifact == nil {
			return "", nil, "", false
		}
		if _, err := h.kernel.GetArtifact(na.Artifact, h.ctx); err != nil {
			return "", nil, "", false
		}
	}
	if len(named) == 0 && len(rec.Outputs) > 0 {
		return "", nil, "", false
	}
	return key, named, rec.SourceRunID, true
}

func (h *Host) commitMemoHit(runID, module string, work *substrate.WorkItem, step *StepContext, key string, named []*substrate.NamedArtifact, sourceRun string) (bool, error) {
	p := PresentationCached(fmt.Sprintf("Cached %s", work.NodeId), fmt.Sprintf("memo hit; outputs from run %s", sourceRun))
	p.FillIdentity(runID, work)
	for _, o := range named {
		p.OutputPorts = append(p.OutputPorts, o.Name)
	}
	if p.Meta == nil {
		p.Meta = map[string]string{}
	}
	p.Meta["memo"] = "hit"
	p.Meta["memo_key"] = key
	p.Meta["memo_source_run"] = sourceRun
	if err := h.emitPresentation(module, p); err != nil {
		return false, err
	}
	if _, err := h.kernel.Commit(&substrate.Derivation{
		RunId: runID, WorkId: work.Id, NodeId: work.NodeId, Outputs: named,
	}, h.ctx); err != nil {
		return false, kernelErr(err)
	}
	h.memoHitCount++
	sr := &StepResult{OK: true, Outputs: named}
	if err := h.applyStepStorage(runID, module, step, sr); err != nil {
		return false, err
	}
	return true, nil
}

func (h *Host) storeMemoAfterSuccess(runID, module string, work *substrate.WorkItem, named []*substrate.NamedArtifact) error {
	policy, ok := h.runPolicies[runID]
	if !ok || !policy.Memo.Enabled || h.memo == nil {
		return nil
	}
	if !policy.Memo.Nodes.Allows(work.NodeId) {
		return nil
	}
	plugin, ok := h.plugins[module]
	if !ok {
		return nil
	}
	digest := plugin.ModuleDigest()
	if digest == "" {
		return nil
	}
	inputs := InputFingerprintMap(work)
	key := MemoKey(module, work.ModuleVersion, digest, work.Capability, inputs)
	return h.memo.Put(BuildRecord(key, work, digest, named, runID))
}

func (h *Host) loadStep(runID string, work *substrate.WorkItem) (*StepContext, error) {
	inputs := map[string]*substrate.Artifact{}
	for _, na := range work.Inputs {
		if na.Artifact == nil {
			continue
		}
		art, err := h.kernel.GetArtifact(na.Artifact, h.ctx)
		if err != nil {
			return nil, kernelErr(err)
		}
		inputs[na.Name] = art
	}
	return &StepContext{RunID: runID, Work: work, Inputs: inputs}, nil
}

func (h *Host) emitSkipEvents(runID string, cut *AssemblyCut) error {
	if cut == nil || len(cut.Skipped) == 0 {
		return nil
	}
	seedByNode := map[string][]SeedSpec{}
	for _, s := range cut.RequiredSeeds {
		seedByNode[s.FromNode] = append(seedByNode[s.FromNode], s)
	}
	for _, skipped := range cut.Skipped {
		seeds := seedByNode[skipped.NodeID]
		detail := "skipped (no outputs required by kept nodes)"
		if len(seeds) > 0 {
			ports := make([]string, len(seeds))
			for i, s := range seeds {
				ports[i] = s.FromPort
			}
			detail = fmt.Sprintf("skipped (seeded ports: %v); cut from run", ports)
		}
		p := PresentationSkipped(fmt.Sprintf("Skip %s", skipped.NodeID), detail)
		p.RunID = runID
		p.NodeID = skipped.NodeID
		p.Module = skipped.Module
		p.Capability = skipped.Capability
		if p.Meta == nil {
			p.Meta = map[string]string{}
		}
		p.Meta["cut"] = "true"
		for _, s := range seeds {
			p.Meta["seed:"+s.FromPort] = s.InputName
		}
		if err := h.emitPresentation(skipped.Module, p); err != nil {
			return err
		}
	}
	return nil
}

func (h *Host) emitPresentation(module string, p Presentation) error {
	stage := p.Stage
	id, err := h.maybePutUI(module, stage.ContractRef(), &p)
	if err != nil {
		return err
	}
	h.stepEvents = append(h.stepEvents, StepEvent{Stage: stage, Presentation: p, ArtifactID: id})
	return nil
}

func (h *Host) maybePutUI(module, contract string, p *Presentation) (string, error) {
	if h.uiPersist != UiArtifacts {
		return "", nil
	}
	body, err := marshalPresentation(p)
	if err != nil {
		return "", invalid(fmt.Sprintf("serialize presentation: %v", err))
	}
	art := substrate.ArtifactWithTrait(contract, body)
	art.ProducedBy = module
	ref, err := h.kernel.PutArtifact(art, h.ctx)
	if err != nil {
		return "", kernelErr(err)
	}
	return ref.Id, nil
}

func (h *Host) ensureRunStorage(runID string, policy FrameworkPolicy) error {
	if !policy.Storage.Enabled() {
		return nil
	}
	if h.storage == nil {
		return invalid("storage enabled without backend")
	}
	var physical []string
	if policy.Storage.ModuleTables() {
		for module, schema := range h.storageSchemas {
			q := QualifyTable(policy.Storage.Mode, runID, module, schema)
			if err := h.storage.EnsureTable(q); err != nil {
				return err
			}
			physical = append(physical, q.PhysicalName)
		}
	}
	if policy.Storage.StepLog {
		q := StepLogQualified(policy.Storage.Mode, runID)
		if err := h.storage.EnsureTable(q); err != nil {
			return err
		}
		physical = append(physical, q.PhysicalName)
	}
	if len(physical) > 0 {
		h.runTables[runID] = physical
	}
	return nil
}

func (h *Host) applyStepStorage(runID, module string, step *StepContext, result *StepResult) error {
	policy, ok := h.runPolicies[runID]
	if !ok || !policy.Storage.Enabled() || h.storage == nil {
		return nil
	}
	if policy.Storage.ModuleTables() {
		if schema, ok := h.storageSchemas[module]; ok {
			plugin := h.plugins[module]
			if write := plugin.OnStore(step, result); write != nil && len(write.Rows) > 0 {
				mode := schema.WriteMode
				if write.Mode != nil {
					mode = *write.Mode
				}
				if mode == "" {
					mode = WriteAppend
				}
				q := QualifyTable(policy.Storage.Mode, runID, module, schema)
				for _, row := range write.Rows {
					InjectIdentity(row, runID, step.Work.Id, step.Work.NodeId, module)
				}
				if err := h.storage.WriteRows(q.PhysicalName, mode, write.Rows, schema.PrimaryKey, runID); err != nil {
					return err
				}
			}
		}
	}
	if policy.Storage.StepLog {
		q := StepLogQualified(policy.Storage.Mode, runID)
		ports := make([]string, 0, len(result.Outputs))
		for _, o := range result.Outputs {
			ports = append(ports, o.Name)
		}
		row := StoreRow{
			"run_id":       runID,
			"work_id":      step.Work.Id,
			"node_id":      step.Work.NodeId,
			"module":       module,
			"capability":   step.Work.Capability,
			"ok":           result.OK,
			"output_ports": ports,
		}
		if result.Error != "" {
			row["error"] = result.Error
		}
		if err := h.storage.EnsureTable(q); err != nil {
			return err
		}
		if err := h.storage.WriteRows(q.PhysicalName, WriteAppend, []StoreRow{row}, nil, runID); err != nil {
			return err
		}
	}
	return nil
}

func (h *Host) finishRunStorage(runID string) {
	policy, ok := h.runPolicies[runID]
	retention := RetentionKeep
	mode := StorageModeOff
	if ok {
		retention = policy.Storage.Retention
		mode = policy.Storage.Mode
	}
	if retention == RetentionDropOnEnd && mode == StorageModePerRun {
		if tables, ok := h.runTables[runID]; ok && h.storage != nil {
			for _, t := range tables {
				_ = h.storage.DropTable(t)
			}
		}
	}
	delete(h.runTables, runID)
	delete(h.runPolicies, runID)
}
