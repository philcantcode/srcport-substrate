//! # srcport-framework — Rust host + module plugins
//!
//! Opinionated application layer on [`srcport_substrate`]. The kernel never
//! loads plugins or calls presentation / storage hooks — only a [`Host`] does.
//!
//! ```text
//! start_pipeline(policy)  → ensure storage tables (if StoragePlan)
//! Host::drive → ClaimReady → on_init → execute → on_final → Put/Commit → on_store
//!                  │              │         │         │                    │
//!                  └──────── KernelApi ─────┴── presentation ──┘    storage side-channel
//! ```
//!
//! **Modes** ([`FrameworkPolicy`]): converge / stream / stream_dedupe / selective /
//! start_after / from_node (cut + seed) / memoized.  
//! **Step lifecycle**: Init → Progress\* → Final ([`Presentation`], [`StepEvent`]);  
//! optional **Skipped** (cut) and **Cached** (memo hit) events.  
//! **Storage** (optional): [`StoragePlan`] + module [`TableSchema`] / [`StoreWrite`].  
//! **Memo** (optional): [`MemoPlan`] + [`MemoStore`] — skip `execute` when module
//! digest and input artifact ids match a prior run.

#![deny(missing_docs)]

mod cut;
mod memo;
mod policy;
mod presentation;
mod storage;

pub use cut::{
    is_seed_input_name, materialize_cut, merge_inputs, resolve_kept_nodes, seed_input_name,
    seeds_from_run, validate_seeds_present, AssemblyCut, SeedSpec, SkippedNode, SEED_INPUT_PREFIX,
};
pub use memo::{
    build_record, input_fingerprint_map, memo_key, record_to_named_outputs, MemoNodes, MemoPlan,
    MemoRecord, MemoStore, MemoryMemo,
};
pub use policy::{
    DriveAfter, DrivePlan, FiringPlan, FrameworkPolicy, NodePlan, RunMode,
};
pub use presentation::{
    Presentation, PresentationStatus, ProcessingStatus, ProcessingView, ResultStatus, ResultView,
    StepEvent, StepResult, StepStage, UiEvent, CONTRACT_PROCESSING_VIEW, CONTRACT_RESULT_VIEW,
    CONTRACT_STEP_CACHED, CONTRACT_STEP_FINAL, CONTRACT_STEP_INIT, CONTRACT_STEP_PROGRESS,
    CONTRACT_STEP_SKIPPED,
};
pub use storage::{
    store_row, ColumnDef, ColumnType, MemoryStorage, QualifiedTable, StorageBackend, StorageMode,
    StoragePlan, StorageRetention, StoreRow, StoreValue, StoreWrite, TableSchema, WriteMode,
    STEP_LOG_TABLE,
};

use std::collections::HashMap;

use serde::Serialize;
use srcport_substrate::{
    Artifact, ArtifactRef, Assembly, ClaimRequest, Derivation, InjectInputRequest, KernelApi,
    KernelError, ModuleManifest, NamedArtifact, RequestContext, Run, RunRef, RunRequest, RunState,
    WorkItem,
};

// ── Plugin surface ──────────────────────────────────────────────────────────

/// One named output port value before the host calls `PutArtifact`.
///
/// Values are **trait bags**: one or more contract refs with inline bodies.
/// Use [`PortBody::with_trait`] for the common single-trait case.
#[derive(Debug, Clone)]
pub struct PortBody {
    /// Capability output port name (must match the assembly / capability).
    pub port: String,
    /// Trait bag: contract ref → inline body bytes.
    pub traits: std::collections::BTreeMap<String, Vec<u8>>,
    /// Optional stable entity id (not part of value identity).
    pub entity_id: String,
}

impl PortBody {
    /// Single-trait output (most modules).
    pub fn with_trait(
        port: impl Into<String>,
        contract: impl Into<String>,
        body: impl Into<Vec<u8>>,
    ) -> Self {
        let mut traits = std::collections::BTreeMap::new();
        traits.insert(contract.into(), body.into());
        Self {
            port: port.into(),
            traits,
            entity_id: String::new(),
        }
    }

    /// Multi-trait output (enrichment / trait bags).
    pub fn with_traits(
        port: impl Into<String>,
        pairs: impl IntoIterator<Item = (impl Into<String>, impl Into<Vec<u8>>)>,
    ) -> Self {
        let mut traits = std::collections::BTreeMap::new();
        for (c, b) in pairs {
            traits.insert(c.into(), b.into());
        }
        Self {
            port: port.into(),
            traits,
            entity_id: String::new(),
        }
    }

    /// Attach an entity id (copied onto the artifact).
    pub fn with_entity_id(mut self, id: impl Into<String>) -> Self {
        self.entity_id = id.into();
        self
    }
}

/// Result of [`ModulePlugin::execute`].
#[derive(Debug, Clone, Default)]
pub struct StepOutput {
    /// Named domain outputs to put and commit.
    pub outputs: Vec<PortBody>,
}

/// Context for one claimed work unit: inputs, identity, and progress emission.
///
/// Domain ports stay on the kernel data plane. Presentation is a side channel
/// via [`StepContext::emit_progress`] and plugin lifecycle hooks.
pub struct StepContext {
    /// Run id.
    pub run_id: String,
    /// Claimed work item (includes input artifact refs).
    pub work: WorkItem,
    /// Input artifacts loaded from the kernel, keyed by port name.
    pub inputs: HashMap<String, Artifact>,
    /// Buffered progress presentations (drained by the host after execute).
    progress_buf: Vec<Presentation>,
}

impl StepContext {
    /// Emit a **Progress** presentation mid-execute.
    ///
    /// The host records [`StepEvent`]s (and optionally puts artifacts) after
    /// `execute` returns, in emission order. Stage is forced to [`StepStage::Progress`].
    pub fn emit_progress(&mut self, mut presentation: Presentation) {
        presentation.stage = StepStage::Progress;
        if presentation.status == PresentationStatus::Pending {
            presentation.status = PresentationStatus::Running;
        }
        presentation.fill_identity(&self.run_id, &self.work);
        self.progress_buf.push(presentation);
    }

    /// Number of progress emits buffered so far.
    pub fn progress_count(&self) -> usize {
        self.progress_buf.len()
    }

    fn take_progress(&mut self) -> Vec<Presentation> {
        std::mem::take(&mut self.progress_buf)
    }
}

/// Framework-level failure (wraps kernel errors and plugin mistakes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameworkError {
    /// Kernel ABI failure.
    Kernel(KernelError),
    /// No plugin registered for a module name.
    NoPlugin(String),
    /// Plugin or host usage error.
    Invalid(String),
    /// Domain step failed (execute returned err after optional final presentation).
    StepFailed(String),
}

impl std::fmt::Display for FrameworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameworkError::Kernel(e) => write!(f, "kernel: {e}"),
            FrameworkError::NoPlugin(m) => write!(f, "no plugin registered for module {m}"),
            FrameworkError::Invalid(r) => write!(f, "invalid: {r}"),
            FrameworkError::StepFailed(r) => write!(f, "step failed: {r}"),
        }
    }
}

impl std::error::Error for FrameworkError {}

impl From<KernelError> for FrameworkError {
    fn from(value: KernelError) -> Self {
        FrameworkError::Kernel(value)
    }
}

/// Domain module as a host-side plugin.
///
/// Presentation and storage hooks are optional. Modules must not import each
/// other; couple only through contract refs and assemblies. Never return real
/// UI toolkits — only [`Presentation`] data. Storage writes are tabular data
/// only — the host owns the backend.
pub trait ModulePlugin: Send {
    /// Manifest passed to `Register`.
    fn manifest(&self) -> ModuleManifest;

    /// Content identity of this implementation for [`MemoPlan`] caching.
    ///
    /// Return a stable digest of code / config that affects outputs. Empty /
    /// `None` means the node is **uncacheable** (always executes). Read on each
    /// claim when memo is enabled so digests can be updated deliberately.
    ///
    /// Examples: build-time hash of the crate, wasm digest, or an explicit
    /// version pin the author bumps when behaviour changes.
    fn module_digest(&self) -> Option<String> {
        None
    }

    /// Perform domain work for a claimed unit.
    ///
    /// Call [`StepContext::emit_progress`] zero or more times for Progress stages.
    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError>;

    /// Optional **Init** presentation after claim, before `execute`.
    ///
    /// Default: maps legacy [`ModulePlugin::processing_ui`] if implemented.
    /// Not called on memo cache hits.
    fn on_init(&self, step: &StepContext) -> Option<Presentation> {
        self.processing_ui(&step.work).map(Presentation::from)
    }

    /// Optional **Final** presentation after outputs (or on failure).
    ///
    /// Default: maps legacy [`ModulePlugin::result_ui`] on success.
    fn on_final(&self, step: &StepContext, result: &StepResult) -> Option<Presentation> {
        if result.ok {
            self.result_ui(&step.work, &result.outputs)
                .map(Presentation::from)
        } else {
            None
        }
    }

    /// Optional table schema when policy enables module storage.
    ///
    /// Called at [`Host::register_plugin`]. Return `None` to skip tables even
    /// when the run uses [`StorageMode::PerRun`] / [`StorageMode::Shared`].
    fn storage_schema(&self) -> Option<TableSchema> {
        None
    }

    /// Optional rows to write after a step (success or failure).
    ///
    /// Invoked by the host when the run's [`StoragePlan`] enables module tables.
    /// The module chooses append / upsert / replace via [`StoreWrite::mode`].
    /// Framework injects `_run_id`, `_work_id`, `_node_id`, `_module` when missing.
    fn on_store(&self, _step: &StepContext, _result: &StepResult) -> Option<StoreWrite> {
        None
    }

    /// Legacy hook — prefer [`ModulePlugin::on_init`].
    fn processing_ui(&self, _work: &WorkItem) -> Option<ProcessingView> {
        None
    }

    /// Legacy hook — prefer [`ModulePlugin::on_final`].
    fn result_ui(&self, _work: &WorkItem, _outputs: &[NamedArtifact]) -> Option<ResultView> {
        None
    }
}

// ── Host ────────────────────────────────────────────────────────────────────

/// Whether the host should `PutArtifact` presentation payloads onto the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiPersist {
    /// Only collect [`StepEvent`]s host-locally (default for light shells).
    LocalOnly,
    /// Also store presentation as content-addressed artifacts (auditable).
    Artifacts,
}

/// Opinionated driver around any [`KernelApi`] backend.
pub struct Host<K: KernelApi> {
    kernel: K,
    plugins: HashMap<String, Box<dyn ModulePlugin>>,
    ctx: RequestContext,
    ui_persist: UiPersist,
    step_events: Vec<StepEvent>,
    /// Policy frozen at [`Host::start_pipeline`] (drive / claim filters / storage / memo).
    run_policies: HashMap<String, FrameworkPolicy>,
    /// Optional tabular backend (required when any run uses storage).
    storage: Option<Box<dyn StorageBackend>>,
    /// Schemas captured at [`Host::register_plugin`] (module → schema).
    storage_schemas: HashMap<String, TableSchema>,
    /// Physical tables created for a run (for PerRun cleanup).
    run_tables: HashMap<String, Vec<String>>,
    /// Optional cross-run memo store (required when any run enables [`MemoPlan`]).
    memo: Option<Box<dyn MemoStore>>,
    /// How many times domain `execute` ran (tests / metrics).
    execute_count: u64,
    /// How many memo cache hits were applied (tests / metrics).
    memo_hit_count: u64,
}

impl<K: KernelApi> Host<K> {
    /// Create a host over a kernel backend.
    pub fn new(kernel: K) -> Self {
        Self {
            kernel,
            plugins: HashMap::new(),
            ctx: RequestContext {
                caller: "srcport-framework".into(),
                ..Default::default()
            },
            ui_persist: UiPersist::LocalOnly,
            step_events: Vec::new(),
            run_policies: HashMap::new(),
            storage: None,
            storage_schemas: HashMap::new(),
            run_tables: HashMap::new(),
            memo: None,
            execute_count: 0,
            memo_hit_count: 0,
        }
    }

    /// Override call metadata (`caller`, idempotency, correlation, …).
    pub fn with_context(mut self, ctx: RequestContext) -> Self {
        self.ctx = ctx;
        self
    }

    /// Persist presentation as kernel artifacts (in addition to host events).
    pub fn with_ui_persist(mut self, mode: UiPersist) -> Self {
        self.ui_persist = mode;
        self
    }

    /// Attach a tabular storage backend for [`StoragePlan`]-enabled runs.
    pub fn with_storage(mut self, backend: impl StorageBackend + 'static) -> Self {
        self.storage = Some(Box::new(backend));
        self
    }

    /// Attach a memo store for [`MemoPlan`]-enabled runs.
    pub fn with_memo(mut self, store: impl MemoStore + 'static) -> Self {
        self.memo = Some(Box::new(store));
        self
    }

    /// Borrow the underlying kernel.
    pub fn kernel(&self) -> &K {
        &self.kernel
    }

    /// Borrow the memo store, if configured.
    pub fn memo_store(&self) -> Option<&(dyn MemoStore + 'static)> {
        self.memo.as_deref()
    }

    /// Mutable memo store (e.g. tests clearing entries).
    pub fn memo_store_mut(&mut self) -> Option<&mut (dyn MemoStore + 'static)> {
        self.memo.as_deref_mut()
    }

    /// Total domain `execute` invocations since host creation.
    pub fn execute_count(&self) -> u64 {
        self.execute_count
    }

    /// Total memo cache hits applied since host creation.
    pub fn memo_hit_count(&self) -> u64 {
        self.memo_hit_count
    }

    /// Borrow the storage backend, if configured.
    pub fn storage(&self) -> Option<&(dyn StorageBackend + 'static)> {
        self.storage.as_deref()
    }

    /// Mutable storage backend (e.g. for tests inspecting rows).
    pub fn storage_mut(&mut self) -> Option<&mut (dyn StorageBackend + 'static)> {
        self.storage.as_deref_mut()
    }

    /// Policy frozen for a run started via [`Host::start_pipeline`], if any.
    pub fn policy(&self, run_id: &str) -> Option<&FrameworkPolicy> {
        self.run_policies.get(run_id)
    }

    /// Step lifecycle events since the last [`Host::take_step_events`].
    pub fn step_events(&self) -> &[StepEvent] {
        &self.step_events
    }

    /// Drain step lifecycle events.
    pub fn take_step_events(&mut self) -> Vec<StepEvent> {
        std::mem::take(&mut self.step_events)
    }

    /// Alias for [`Host::step_events`] (older name).
    pub fn ui_events(&self) -> &[StepEvent] {
        self.step_events()
    }

    /// Alias for [`Host::take_step_events`] (older name).
    pub fn take_ui_events(&mut self) -> Vec<StepEvent> {
        self.take_step_events()
    }

    /// Register a plugin: `Register` on the kernel and store it for claims.
    ///
    /// Also asks [`ModulePlugin::storage_schema`] and remembers it for later
    /// `ensure_table` when a run enables module storage.
    pub fn register_plugin(
        &mut self,
        plugin: Box<dyn ModulePlugin>,
    ) -> Result<(), FrameworkError> {
        let manifest = plugin.manifest();
        if manifest.name.is_empty() {
            return Err(FrameworkError::Invalid(
                "plugin manifest.name must be non-empty".into(),
            ));
        }
        if self.plugins.contains_key(&manifest.name) {
            return Err(FrameworkError::Invalid(format!(
                "plugin already registered: {}",
                manifest.name
            )));
        }
        let name = manifest.name.clone();
        if let Some(schema) = plugin.storage_schema() {
            if schema.name.is_empty() {
                return Err(FrameworkError::Invalid(format!(
                    "plugin {name} storage_schema.name must be non-empty"
                )));
            }
            self.storage_schemas.insert(name.clone(), schema);
        }
        self.kernel.register(manifest, &self.ctx);
        self.plugins.insert(name, plugin);
        Ok(())
    }

    /// Start a pipeline with an opinionated [`FrameworkPolicy`].
    ///
    /// When `policy.nodes` is not [`NodePlan::All`], the host **materialises a
    /// cut**: drops excluded nodes, rewrites crossing edges to `__seed/…`
    /// inputs, and fails closed if any required seed is missing from `inputs`.
    /// Skipped nodes emit [`StepStage::Skipped`] events (presentation only).
    ///
    /// When `policy.storage` is enabled, ensures module tables (and optional
    /// step log) on the host storage backend.
    pub fn start_pipeline(
        &mut self,
        run_id: impl Into<String>,
        assembly: Assembly,
        inputs: Vec<NamedArtifact>,
        policy: FrameworkPolicy,
    ) -> Result<Run, FrameworkError> {
        if matches!(policy.mode, RunMode::Selective) && matches!(policy.nodes, NodePlan::All) {
            return Err(FrameworkError::Invalid(
                "RunMode::Selective requires NodePlan::Only, After, or From".into(),
            ));
        }
        let run_id = run_id.into();
        if run_id.is_empty() {
            return Err(FrameworkError::Invalid("run_id must be non-empty".into()));
        }
        if self.run_policies.contains_key(&run_id) {
            return Err(FrameworkError::Invalid(format!(
                "pipeline policy already registered for run_id {run_id}"
            )));
        }

        if policy.storage.enabled() && self.storage.is_none() {
            return Err(FrameworkError::Invalid(
                "StoragePlan enabled but host has no StorageBackend (use Host::with_storage)"
                    .into(),
            ));
        }
        if policy.memo.enabled && self.memo.is_none() {
            return Err(FrameworkError::Invalid(
                "MemoPlan enabled but host has no MemoStore (use Host::with_memo)".into(),
            ));
        }

        let cut = materialize_cut(&assembly, &policy.nodes)?;
        validate_seeds_present(&cut, &inputs)?;
        self.emit_skip_events(&run_id, &cut)?;

        // Assembly is already materialised; do not apply include_nodes again.
        let mut kernel_policy = policy.clone();
        kernel_policy.nodes = NodePlan::All;

        let req = kernel_policy.apply_to_run_request(RunRequest {
            id: run_id.clone(),
            assembly: Some(cut.assembly),
            inputs,
            ..Default::default()
        });
        let run = self.kernel.start_run(req, &self.ctx)?;
        self.ensure_run_storage(&run_id, &policy)?;
        self.run_policies.insert(run_id, policy);
        Ok(run)
    }

    /// Resume a prior run **after** a completed node: seed that node (and its
    /// transitive predecessors) from the prior run's latest derivations, then
    /// start a new pipeline with [`FrameworkPolicy::start_after`].
    ///
    /// The new run uses the prior run's frozen assembly and original non-seed
    /// inputs, plus seeds collected from derivations. `policy` defaults should
    /// usually be [`FrameworkPolicy::start_after`]; storage/firing builders may
    /// be layered on top (the node plan is forced to `After(after_node)`).
    pub fn resume_after(
        &mut self,
        new_run_id: impl Into<String>,
        prior_run_id: &str,
        after_node: impl Into<String>,
        mut policy: FrameworkPolicy,
    ) -> Result<Run, FrameworkError> {
        let after_node = after_node.into();
        let prior = self.get_run(prior_run_id)?;
        let assembly = prior
            .assembly
            .clone()
            .ok_or_else(|| FrameworkError::Invalid("prior run has no assembly".into()))?;

        // Cut against the *full* prior assembly (not an already-cut subset).
        policy.nodes = NodePlan::After(after_node.clone());
        let cut = materialize_cut(&assembly, &policy.nodes)?;

        let mut cut_nodes: Vec<String> = cut.skipped.iter().map(|s| s.node_id.clone()).collect();
        // Include the frontier node itself (it is in skipped for After).
        if !cut_nodes.iter().any(|n| n == &after_node) {
            cut_nodes.push(after_node.clone());
        }
        let seeds = seeds_from_run(&self.kernel, prior_run_id, &cut_nodes, &self.ctx)?;

        // Carry original run inputs (non-seed); seeds overlay.
        let base_inputs: Vec<NamedArtifact> = prior
            .inputs
            .into_iter()
            .filter(|i| !is_seed_input_name(&i.name))
            .collect();
        let inputs = merge_inputs(base_inputs, seeds);

        self.start_pipeline(new_run_id, assembly, inputs, policy)
    }

    fn emit_skip_events(
        &mut self,
        run_id: &str,
        cut: &AssemblyCut,
    ) -> Result<(), FrameworkError> {
        if cut.skipped.is_empty() {
            return Ok(());
        }
        let seed_by_node: HashMap<&str, Vec<&SeedSpec>> = {
            let mut m: HashMap<&str, Vec<&SeedSpec>> = HashMap::new();
            for s in &cut.required_seeds {
                m.entry(s.from_node.as_str()).or_default().push(s);
            }
            m
        };
        for skipped in &cut.skipped {
            let seeds = seed_by_node.get(skipped.node_id.as_str()).map(|v| v.as_slice());
            let detail = match seeds {
                Some(list) if !list.is_empty() => {
                    let ports: Vec<_> = list.iter().map(|s| s.from_port.as_str()).collect();
                    format!(
                        "skipped (seeded ports: {}); cut from run",
                        ports.join(", ")
                    )
                }
                _ => "skipped (no outputs required by kept nodes)".into(),
            };
            let mut p = Presentation::skipped(format!("Skip {}", skipped.node_id), detail);
            p.run_id = run_id.into();
            p.node_id = skipped.node_id.clone();
            p.module = skipped.module.clone();
            p.capability = skipped.capability.clone();
            p.meta.insert("cut".into(), "true".into());
            if let Some(list) = seeds {
                for s in list {
                    p.meta
                        .insert(format!("seed:{}", s.from_port), s.input_name.clone());
                }
            }
            self.emit_presentation(&skipped.module, p)?;
        }
        Ok(())
    }

    /// Freeze an assembly over inputs without a framework policy.
    pub fn start_run(&self, req: RunRequest) -> Result<Run, FrameworkError> {
        Ok(self.kernel.start_run(req, &self.ctx)?)
    }

    /// Snapshot a run.
    pub fn get_run(&self, run_id: &str) -> Result<Run, FrameworkError> {
        Ok(self.kernel.get_run(
            &RunRef {
                id: run_id.into(),
            },
            &self.ctx,
        )?)
    }

    /// Inject a named run input. Optionally re-drive.
    pub fn inject(
        &mut self,
        run_id: &str,
        input: NamedArtifact,
        after: DriveAfter,
    ) -> Result<Run, FrameworkError> {
        let run = self.kernel.inject_input(
            InjectInputRequest {
                run_id: run_id.into(),
                input: Some(input),
            },
            &self.ctx,
        )?;
        match after {
            DriveAfter::No => Ok(run),
            DriveAfter::UntilIdle => self.drive_with(run_id, DrivePlan::UntilIdle),
            DriveAfter::OnePass => self.drive_with(run_id, DrivePlan::OnePass),
        }
    }

    /// Cancel a run. Drops stored framework policy and per-run tables when retention says so.
    pub fn cancel(&mut self, run_id: &str) -> Result<Run, FrameworkError> {
        let run = self.kernel.cancel_run(
            &RunRef {
                id: run_id.into(),
            },
            &self.ctx,
        )?;
        self.finish_run_storage(run_id);
        Ok(run)
    }

    /// Drive using the policy frozen at [`Host::start_pipeline`].
    pub fn drive(&mut self, run_id: &str) -> Result<Run, FrameworkError> {
        let plan = self
            .run_policies
            .get(run_id)
            .map(|p| p.effective_drive())
            .unwrap_or(DrivePlan::UntilIdle);
        self.drive_with(run_id, plan)
    }

    /// Drive with an explicit plan.
    pub fn drive_with(&mut self, run_id: &str, plan: DrivePlan) -> Result<Run, FrameworkError> {
        let plan = match plan {
            DrivePlan::UntilIdleThenWait => DrivePlan::UntilIdle,
            other => other,
        };
        let result = match plan {
            DrivePlan::OnePass => self.drive_one_pass(run_id),
            DrivePlan::UntilIdle | DrivePlan::UntilIdleThenWait => self.drive_until_idle(run_id),
        };
        if let Ok(ref run) = result {
            if run.state() != RunState::Running {
                self.finish_run_storage(run_id);
            }
        }
        result
    }

    fn claim_module_names(&self, run_id: &str) -> Vec<String> {
        let all: Vec<String> = self.plugins.keys().cloned().collect();
        match self
            .run_policies
            .get(run_id)
            .and_then(|p| p.claim_modules.as_ref())
        {
            None => all,
            Some(allow) => all
                .into_iter()
                .filter(|m| allow.iter().any(|a| a == m))
                .collect(),
        }
    }

    fn drive_until_idle(&mut self, run_id: &str) -> Result<Run, FrameworkError> {
        loop {
            let run = self.get_run(run_id)?;
            if run.state() != RunState::Running {
                return Ok(run);
            }

            let module_names = self.claim_module_names(run_id);
            let mut progressed = false;

            for module in module_names {
                let run = self.get_run(run_id)?;
                if run.state() != RunState::Running {
                    return Ok(run);
                }
                if self.try_step(run_id, &module)? {
                    progressed = true;
                }
            }

            if !progressed {
                return self.get_run(run_id);
            }
        }
    }

    fn drive_one_pass(&mut self, run_id: &str) -> Result<Run, FrameworkError> {
        let run = self.get_run(run_id)?;
        if run.state() != RunState::Running {
            return Ok(run);
        }
        let module_names = self.claim_module_names(run_id);
        for module in module_names {
            let run = self.get_run(run_id)?;
            if run.state() != RunState::Running {
                return Ok(run);
            }
            let _ = self.try_step(run_id, &module)?;
        }
        self.get_run(run_id)
    }

    /// Claim → (memo hit | Init → execute → Final) → put/commit for `module`.
    ///
    /// When [`MemoPlan`] is enabled and a matching entry exists, domain
    /// `execute` is skipped and prior output artifact refs are committed.
    ///
    /// Returns whether a work unit ran (including memo hits). On domain execute
    /// failure, emits Final (if any) and returns [`FrameworkError::StepFailed`]
    /// without committing.
    pub fn try_step(&mut self, run_id: &str, module: &str) -> Result<bool, FrameworkError> {
        let work = self.kernel.claim_ready(
            ClaimRequest {
                run_id: run_id.into(),
                module: module.into(),
            },
            &self.ctx,
        )?;

        if work.id.is_empty() {
            return Ok(false);
        }

        let mut step = self.load_step(run_id, &work)?;

        // ── Memo lookup (optional) ────────────────────────────────────────
        if let Some((key, named_outputs, source_run)) =
            self.try_memo_hit(run_id, module, &work)?
        {
            return self.commit_memo_hit(
                run_id,
                module,
                &work,
                &step,
                &key,
                named_outputs,
                &source_run,
            );
        }

        // ── Init ──────────────────────────────────────────────────────────
        let init = {
            let plugin = self
                .plugins
                .get(module)
                .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
            plugin.on_init(&step)
        };
        if let Some(mut p) = init {
            p.stage = StepStage::Init;
            p.fill_identity(run_id, &work);
            self.emit_presentation(module, p)?;
        }

        // ── Execute (+ buffered Progress) ─────────────────────────────────
        let exec_result = {
            let plugin = self
                .plugins
                .get_mut(module)
                .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
            plugin.execute(&mut step)
        };
        self.execute_count = self.execute_count.saturating_add(1);

        let progress = step.take_progress();
        for p in progress {
            self.emit_presentation(module, p)?;
        }

        match exec_result {
            Ok(output) => {
                let mut named_outputs = Vec::with_capacity(output.outputs.len());
                for out in &output.outputs {
                    let mut traits = std::collections::BTreeMap::new();
                    for (contract, body) in &out.traits {
                        traits.insert(
                            contract.clone(),
                            srcport_substrate::Trait {
                                body: body.clone(),
                                object: None,
                            },
                        );
                    }
                    let r = self.kernel.put_artifact(
                        Artifact {
                            traits,
                            produced_by: module.into(),
                            entity_id: out.entity_id.clone(),
                            ..Default::default()
                        },
                        &self.ctx,
                    )?;
                    named_outputs.push(NamedArtifact {
                        name: out.port.clone(),
                        artifact: Some(r),
                    });
                }

                let step_result = StepResult {
                    ok: true,
                    outputs: named_outputs.clone(),
                    error: None,
                };
                let final_p = {
                    let plugin = self
                        .plugins
                        .get(module)
                        .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
                    plugin.on_final(&step, &step_result)
                };
                if let Some(mut p) = final_p {
                    p.stage = StepStage::Final;
                    p.fill_identity(run_id, &work);
                    if p.output_ports.is_empty() {
                        p.output_ports = named_outputs.iter().map(|o| o.name.clone()).collect();
                    }
                    self.emit_presentation(module, p)?;
                }

                self.kernel.commit(
                    Derivation {
                        run_id: run_id.into(),
                        work_id: work.id.clone(),
                        node_id: work.node_id.clone(),
                        outputs: named_outputs.clone(),
                        ..Default::default()
                    },
                    &self.ctx,
                )?;

                self.store_memo_after_success(run_id, module, &work, &named_outputs)?;

                // ── Storage (after successful commit) ─────────────────────
                self.apply_step_storage(run_id, module, &step, &step_result)?;

                Ok(true)
            }
            Err(e) => {
                let msg = e.to_string();
                let step_result = StepResult {
                    ok: false,
                    outputs: Vec::new(),
                    error: Some(msg.clone()),
                };
                let final_p = {
                    let plugin = self
                        .plugins
                        .get(module)
                        .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
                    plugin
                        .on_final(&step, &step_result)
                        .or_else(|| Some(Presentation::final_failed("Step failed", msg.clone())))
                };
                if let Some(mut p) = final_p {
                    p.stage = StepStage::Final;
                    p.status = PresentationStatus::Failed;
                    p.fill_identity(run_id, &work);
                    let _ = self.emit_presentation(module, p);
                }
                // Best-effort storage / step_log on failure (no commit).
                let _ = self.apply_step_storage(run_id, module, &step, &step_result);
                // Do not commit; work unit may remain claimed depending on kernel —
                // MemoryKernel leaves claimed work; product may cancel run.
                Err(FrameworkError::StepFailed(msg))
            }
        }
    }

    /// Attempt a memo hit. Returns `(key, outputs, source_run_id)` when valid.
    fn try_memo_hit(
        &self,
        run_id: &str,
        module: &str,
        work: &WorkItem,
    ) -> Result<Option<(String, Vec<NamedArtifact>, String)>, FrameworkError> {
        let Some(policy) = self.run_policies.get(run_id) else {
            return Ok(None);
        };
        if !policy.memo.enabled {
            return Ok(None);
        }
        if !policy.memo.nodes.allows(&work.node_id) {
            return Ok(None);
        }
        let digest = self
            .plugins
            .get(module)
            .and_then(|p| p.module_digest())
            .filter(|d| !d.is_empty());
        let Some(digest) = digest else {
            // Uncacheable without a digest (always miss, never error).
            let _ = policy.memo.require_digest;
            return Ok(None);
        };
        let Some(store) = self.memo.as_ref() else {
            return Ok(None);
        };

        let inputs = input_fingerprint_map(work);
        let key = memo_key(
            module,
            &work.module_version,
            &digest,
            &work.capability,
            &inputs,
        );
        let Some(record) = store.get(&key)? else {
            return Ok(None);
        };

        // Verify every output artifact still exists in the kernel.
        let named = record_to_named_outputs(&record);
        for na in &named {
            let Some(r) = na.artifact.as_ref() else {
                return Ok(None);
            };
            if self.kernel.get_artifact(r, &self.ctx).is_err() {
                return Ok(None);
            }
        }
        if named.is_empty() && !record.outputs.is_empty() {
            return Ok(None);
        }
        Ok(Some((key, named, record.source_run_id.clone())))
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_memo_hit(
        &mut self,
        run_id: &str,
        module: &str,
        work: &WorkItem,
        step: &StepContext,
        key: &str,
        named_outputs: Vec<NamedArtifact>,
        source_run: &str,
    ) -> Result<bool, FrameworkError> {
        let mut p = Presentation::cached(
            format!("Cached {}", work.node_id),
            format!("memo hit; outputs from run {source_run}"),
        );
        p.fill_identity(run_id, work);
        p.output_ports = named_outputs.iter().map(|o| o.name.clone()).collect();
        p.meta.insert("memo".into(), "hit".into());
        p.meta.insert("memo_key".into(), key.into());
        p.meta
            .insert("memo_source_run".into(), source_run.into());
        self.emit_presentation(module, p)?;

        self.kernel.commit(
            Derivation {
                run_id: run_id.into(),
                work_id: work.id.clone(),
                node_id: work.node_id.clone(),
                outputs: named_outputs.clone(),
                ..Default::default()
            },
            &self.ctx,
        )?;

        self.memo_hit_count = self.memo_hit_count.saturating_add(1);

        let step_result = StepResult {
            ok: true,
            outputs: named_outputs,
            error: None,
        };
        // Step log / optional storage still records the (reused) step.
        self.apply_step_storage(run_id, module, step, &step_result)?;
        Ok(true)
    }

    fn store_memo_after_success(
        &mut self,
        run_id: &str,
        module: &str,
        work: &WorkItem,
        named_outputs: &[NamedArtifact],
    ) -> Result<(), FrameworkError> {
        let Some(policy) = self.run_policies.get(run_id) else {
            return Ok(());
        };
        if !policy.memo.enabled {
            return Ok(());
        }
        if !policy.memo.nodes.allows(&work.node_id) {
            return Ok(());
        }
        let digest = match self
            .plugins
            .get(module)
            .and_then(|p| p.module_digest())
            .filter(|d| !d.is_empty())
        {
            Some(d) => d,
            None => return Ok(()),
        };
        let Some(store) = self.memo.as_mut() else {
            return Ok(());
        };
        let inputs = input_fingerprint_map(work);
        let key = memo_key(
            module,
            &work.module_version,
            &digest,
            &work.capability,
            &inputs,
        );
        let record = build_record(key, work, &digest, named_outputs, run_id);
        store.put(record)
    }

    fn load_step(&self, run_id: &str, work: &WorkItem) -> Result<StepContext, FrameworkError> {
        let mut inputs = HashMap::new();
        for na in &work.inputs {
            let Some(r) = na.artifact.as_ref() else {
                continue;
            };
            let art = self.kernel.get_artifact(r, &self.ctx)?;
            inputs.insert(na.name.clone(), art);
        }
        Ok(StepContext {
            run_id: run_id.into(),
            work: work.clone(),
            inputs,
            progress_buf: Vec::new(),
        })
    }

    fn emit_presentation(
        &mut self,
        module: &str,
        presentation: Presentation,
    ) -> Result<(), FrameworkError> {
        let stage = presentation.stage;
        let contract = stage.contract_ref();
        let artifact_id = self.maybe_put_ui(module, contract, &presentation)?;
        self.step_events.push(StepEvent {
            stage,
            presentation,
            artifact_id,
        });
        Ok(())
    }

    fn maybe_put_ui<T: Serialize>(
        &self,
        module: &str,
        contract: &str,
        view: &T,
    ) -> Result<String, FrameworkError> {
        if self.ui_persist != UiPersist::Artifacts {
            return Ok(String::new());
        }
        let body = serde_json::to_vec(view)
            .map_err(|e| FrameworkError::Invalid(format!("serialize presentation: {e}")))?;
        let mut art = srcport_substrate::artifact_with_trait(contract, body);
        art.produced_by = module.into();
        let r = self.kernel.put_artifact(art, &self.ctx)?;
        Ok(r.id)
    }

    // ── Storage helpers ───────────────────────────────────────────────────

    fn ensure_run_storage(
        &mut self,
        run_id: &str,
        policy: &FrameworkPolicy,
    ) -> Result<(), FrameworkError> {
        if !policy.storage.enabled() {
            return Ok(());
        }
        if self.storage.is_none() {
            return Err(FrameworkError::Invalid(
                "storage enabled without backend".into(),
            ));
        }

        let mut physical = Vec::new();
        let mut to_ensure: Vec<storage::QualifiedTable> = Vec::new();

        if policy.storage.module_tables() {
            let mode = policy.storage.mode;
            for (module, schema) in &self.storage_schemas {
                to_ensure.push(storage::qualify_table(mode, run_id, module, schema));
            }
        }

        if policy.storage.step_log {
            to_ensure.push(storage::step_log_qualified(policy.storage.mode, run_id));
        }

        let backend = self.storage.as_mut().expect("checked above");
        for q in &to_ensure {
            backend.ensure_table(q)?;
            physical.push(q.physical_name.clone());
        }

        if !physical.is_empty() {
            self.run_tables.insert(run_id.into(), physical);
        }
        Ok(())
    }

    fn apply_step_storage(
        &mut self,
        run_id: &str,
        module: &str,
        step: &StepContext,
        result: &StepResult,
    ) -> Result<(), FrameworkError> {
        let Some(policy) = self.run_policies.get(run_id).cloned() else {
            return Ok(());
        };
        if !policy.storage.enabled() {
            return Ok(());
        }

        if policy.storage.module_tables() {
            if let Some(schema) = self.storage_schemas.get(module).cloned() {
                let write = {
                    let plugin = self
                        .plugins
                        .get(module)
                        .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
                    plugin.on_store(step, result)
                };
                if let Some(write) = write {
                    if !write.rows.is_empty() {
                        let mode = write.mode.unwrap_or(schema.write_mode);
                        let q = storage::qualify_table(
                            policy.storage.mode,
                            run_id,
                            module,
                            &schema,
                        );
                        let mut rows = write.rows;
                        for row in &mut rows {
                            storage::inject_identity(
                                row,
                                run_id,
                                &step.work.id,
                                &step.work.node_id,
                                module,
                            );
                        }
                        let backend = self.storage.as_mut().ok_or_else(|| {
                            FrameworkError::Invalid("storage missing at write".into())
                        })?;
                        backend.write_rows(
                            &q.physical_name,
                            mode,
                            &rows,
                            &schema.primary_key,
                            run_id,
                        )?;
                    }
                }
            }
        }

        if policy.storage.step_log {
            let q = storage::step_log_qualified(policy.storage.mode, run_id);
            let ports: Vec<String> = result.outputs.iter().map(|o| o.name.clone()).collect();
            let ports_json = serde_json::to_value(&ports).unwrap_or(serde_json::Value::Null);
            let mut row = StoreRow::new();
            row.insert("run_id".into(), StoreValue::Text(run_id.into()));
            row.insert(
                "work_id".into(),
                StoreValue::Text(step.work.id.clone()),
            );
            row.insert(
                "node_id".into(),
                StoreValue::Text(step.work.node_id.clone()),
            );
            row.insert("module".into(), StoreValue::Text(module.into()));
            row.insert(
                "capability".into(),
                StoreValue::Text(step.work.capability.clone()),
            );
            row.insert("ok".into(), StoreValue::Boolean(result.ok));
            if let Some(err) = &result.error {
                row.insert("error".into(), StoreValue::Text(err.clone()));
            }
            row.insert("output_ports".into(), StoreValue::Json(ports_json));

            let backend = self.storage.as_mut().ok_or_else(|| {
                FrameworkError::Invalid("storage missing at step_log write".into())
            })?;
            // ensure in case only step_log (already ensured at start, but Shared is fine)
            backend.ensure_table(&q)?;
            backend.write_rows(
                &q.physical_name,
                WriteMode::Append,
                &[row],
                &[],
                run_id,
            )?;
        }

        Ok(())
    }

    /// Drop PerRun tables when retention is DropOnEnd; always forget run policy.
    fn finish_run_storage(&mut self, run_id: &str) {
        let retention = self
            .run_policies
            .get(run_id)
            .map(|p| p.storage.retention)
            .unwrap_or(StorageRetention::Keep);
        let mode = self
            .run_policies
            .get(run_id)
            .map(|p| p.storage.mode)
            .unwrap_or(StorageMode::Off);

        if retention == StorageRetention::DropOnEnd
            && mode == StorageMode::PerRun
        {
            if let Some(tables) = self.run_tables.remove(run_id) {
                if let Some(backend) = self.storage.as_mut() {
                    for t in tables {
                        let _ = backend.drop_table(&t);
                    }
                }
            }
        } else {
            self.run_tables.remove(run_id);
        }
        self.run_policies.remove(run_id);
    }
}

/// Helper: `NamedArtifact` from a put result.
pub fn named_ref(name: &str, r: &ArtifactRef) -> NamedArtifact {
    NamedArtifact {
        name: name.into(),
        artifact: Some(r.clone()),
    }
}
