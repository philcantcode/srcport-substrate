//! # srcport-framework — Rust host + module plugins (v0.1.0)
//!
//! Opinionated application layer on [`srcport_substrate`]. The kernel never
//! loads plugins or calls UI hooks — only a [`Host`] does. Domain work still
//! flows as immutable artifacts through assemblies; see `framework/SPEC.md`.
//!
//! ```text
//! start_pipeline(policy) → drive / inject
//! Host::drive  →  ClaimReady  →  processing_ui?  →  execute  →  Put/Commit  →  result_ui?
//!                      │                │                              │
//!                      └──────── KernelApi (substrate) ────────────────┘
//! ```
//!
//! **Modes** ([`FrameworkPolicy`]): `converge`, `stream`, `stream_dedupe`,
//! `selective` — presets that compile to kernel `ExecutionPolicy` + host drive rules.

#![deny(missing_docs)]

mod policy;

pub use policy::{
    DriveAfter, DrivePlan, FiringPlan, FrameworkPolicy, NodePlan, RunMode,
};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use srcport_substrate::{
    Artifact, ArtifactRef, Assembly, ClaimRequest, Derivation, InjectInputRequest, KernelApi,
    KernelError, ModuleManifest, NamedArtifact, RequestContext, Run, RunRef, RunRequest, RunState,
    WorkItem,
};

// ── UI profile (srcport.ui.v1) ──────────────────────────────────────────────

/// Contract ref for a processing (in-flight) view artifact.
pub const CONTRACT_PROCESSING_VIEW: &str = "srcport.ui.v1.ProcessingView";
/// Contract ref for a result view artifact.
pub const CONTRACT_RESULT_VIEW: &str = "srcport.ui.v1.ResultView";

/// Coarse processing state for product chrome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStatus {
    /// Not started.
    Pending,
    /// Claimed / executing.
    Running,
    /// Waiting on inputs (host may surface this without a claim).
    Blocked,
    /// Step failed in the product sense (optional; kernel failure is separate).
    Failed,
}

/// Optional step chrome while work is in flight (`srcport.ui.v1.ProcessingView`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingView {
    /// Short label.
    pub title: String,
    /// Coarse status.
    pub status: ProcessingStatus,
    /// Optional detail line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional 0..=1 progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// Run id (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    /// Work id (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub work_id: String,
    /// Assembly node id (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    /// Module name (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub module: String,
}

impl Default for ProcessingView {
    fn default() -> Self {
        Self {
            title: String::new(),
            status: ProcessingStatus::Running,
            detail: None,
            progress: None,
            run_id: String::new(),
            work_id: String::new(),
            node_id: String::new(),
            module: String::new(),
        }
    }
}

/// Coarse result state for product chrome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    /// Outputs produced successfully.
    Ok,
    /// Succeeded with nothing useful to show.
    Empty,
    /// Product-level failure presentation.
    Failed,
}

/// Optional step chrome after outputs exist (`srcport.ui.v1.ResultView`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultView {
    /// Short label.
    pub title: String,
    /// Coarse status.
    pub status: ResultStatus,
    /// Optional summary line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Run id (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    /// Work id (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub work_id: String,
    /// Assembly node id (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    /// Module name (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub module: String,
    /// Output port names produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_ports: Vec<String>,
}

impl Default for ResultView {
    fn default() -> Self {
        Self {
            title: String::new(),
            status: ResultStatus::Ok,
            summary: None,
            run_id: String::new(),
            work_id: String::new(),
            node_id: String::new(),
            module: String::new(),
            output_ports: Vec::new(),
        }
    }
}

/// A UI event observed by the host (and optionally written as an artifact).
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// Processing chrome for a claimed work unit.
    Processing {
        /// View body.
        view: ProcessingView,
        /// Artifact id when `Host` persisted the view; empty if host-local only.
        artifact_id: String,
    },
    /// Result chrome after commit inputs were built.
    Result {
        /// View body.
        view: ResultView,
        /// Artifact id when persisted; empty if host-local only.
        artifact_id: String,
    },
}

// ── Plugin surface ──────────────────────────────────────────────────────────

/// One named output port value before the host calls `PutArtifact`.
#[derive(Debug, Clone)]
pub struct PortBody {
    /// Capability output port name (must match the assembly / capability).
    pub port: String,
    /// Contract ref for the artifact `type`.
    pub contract: String,
    /// Inline artifact body bytes.
    pub body: Vec<u8>,
}

/// Result of [`ModulePlugin::execute`].
#[derive(Debug, Clone, Default)]
pub struct StepOutput {
    /// Named domain outputs to put and commit.
    pub outputs: Vec<PortBody>,
}

/// Read-only inputs and identity for one claimed work unit.
#[derive(Debug, Clone)]
pub struct StepContext {
    /// Run id.
    pub run_id: String,
    /// Claimed work item (includes input artifact refs).
    pub work: WorkItem,
    /// Input artifacts loaded from the kernel, keyed by port name.
    pub inputs: HashMap<String, Artifact>,
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
}

impl std::fmt::Display for FrameworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameworkError::Kernel(e) => write!(f, "kernel: {e}"),
            FrameworkError::NoPlugin(m) => write!(f, "no plugin registered for module {m}"),
            FrameworkError::Invalid(r) => write!(f, "invalid: {r}"),
        }
    }
}

impl std::error::Error for FrameworkError {}

impl From<KernelError> for FrameworkError {
    fn from(value: KernelError) -> Self {
        FrameworkError::Kernel(value)
    }
}

/// Domain module as a host-side plugin. Optional UI hooks default to `None`.
///
/// The substrate kernel never sees this trait. Plugins must not import each
/// other; couple only through contract refs and assemblies.
pub trait ModulePlugin: Send {
    /// Manifest passed to `Register`.
    fn manifest(&self) -> ModuleManifest;

    /// Perform domain work for a claimed unit. Return port bodies; the host
    /// puts artifacts and commits the derivation.
    fn execute(&mut self, step: &StepContext) -> Result<StepOutput, FrameworkError>;

    /// Optional processing chrome when work is claimed.
    fn processing_ui(&self, _work: &WorkItem) -> Option<ProcessingView> {
        None
    }

    /// Optional result chrome after outputs are produced (before or after commit).
    fn result_ui(&self, _work: &WorkItem, _outputs: &[NamedArtifact]) -> Option<ResultView> {
        None
    }
}

// ── Host ────────────────────────────────────────────────────────────────────

/// Whether the host should `PutArtifact` UI views onto the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiPersist {
    /// Only collect [`UiEvent`]s host-locally (default for light shells).
    LocalOnly,
    /// Also store UI views as content-addressed artifacts (auditable).
    Artifacts,
}

/// Opinionated driver around any [`KernelApi`] backend.
pub struct Host<K: KernelApi> {
    kernel: K,
    plugins: HashMap<String, Box<dyn ModulePlugin>>,
    ctx: RequestContext,
    ui_persist: UiPersist,
    ui_events: Vec<UiEvent>,
    /// Policy frozen at [`Host::start_pipeline`] (drive / claim filters).
    run_policies: HashMap<String, FrameworkPolicy>,
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
            ui_events: Vec::new(),
            run_policies: HashMap::new(),
        }
    }

    /// Override call metadata (`caller`, idempotency, correlation, …).
    pub fn with_context(mut self, ctx: RequestContext) -> Self {
        self.ctx = ctx;
        self
    }

    /// Persist UI views as kernel artifacts (in addition to host events).
    pub fn with_ui_persist(mut self, mode: UiPersist) -> Self {
        self.ui_persist = mode;
        self
    }

    /// Borrow the underlying kernel.
    pub fn kernel(&self) -> &K {
        &self.kernel
    }

    /// Policy frozen for a run started via [`Host::start_pipeline`], if any.
    pub fn policy(&self, run_id: &str) -> Option<&FrameworkPolicy> {
        self.run_policies.get(run_id)
    }

    /// UI events collected since the last [`Host::take_ui_events`].
    pub fn ui_events(&self) -> &[UiEvent] {
        &self.ui_events
    }

    /// Drain collected UI events.
    pub fn take_ui_events(&mut self) -> Vec<UiEvent> {
        std::mem::take(&mut self.ui_events)
    }

    /// Register a plugin: `Register` on the kernel and store it for claims.
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
        self.kernel.register(manifest, &self.ctx);
        self.plugins.insert(name, plugin);
        Ok(())
    }

    /// Start a pipeline with an opinionated [`FrameworkPolicy`].
    ///
    /// Compiles policy → kernel `ExecutionPolicy` / `include_nodes` / `Limits`,
    /// stores the policy for later [`Host::drive`] / [`Host::inject`].
    pub fn start_pipeline(
        &mut self,
        run_id: impl Into<String>,
        assembly: Assembly,
        inputs: Vec<NamedArtifact>,
        policy: FrameworkPolicy,
    ) -> Result<Run, FrameworkError> {
        if matches!(policy.mode, RunMode::Selective)
            && !matches!(policy.nodes, NodePlan::Only(ref ids) if !ids.is_empty())
        {
            return Err(FrameworkError::Invalid(
                "RunMode::Selective requires NodePlan::Only with at least one node id".into(),
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

        let req = policy.apply_to_run_request(RunRequest {
            id: run_id.clone(),
            assembly: Some(assembly),
            inputs,
            ..Default::default()
        });
        let run = self.kernel.start_run(req, &self.ctx)?;
        self.run_policies.insert(run_id, policy);
        Ok(run)
    }

    /// Freeze an assembly over inputs (`StartRun`) without a framework policy.
    ///
    /// Prefer [`Host::start_pipeline`] for product modes. Raw starts use
    /// [`FrameworkPolicy::converge`] drive defaults when driving.
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

    /// Inject a named run input (kernel `InjectInput`). Optionally re-drive.
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

    /// Cancel a run (kernel `CancelRun`). Drops stored framework policy for the id.
    pub fn cancel(&mut self, run_id: &str) -> Result<Run, FrameworkError> {
        let run = self.kernel.cancel_run(
            &RunRef {
                id: run_id.into(),
            },
            &self.ctx,
        )?;
        self.run_policies.remove(run_id);
        Ok(run)
    }

    /// Drive using the policy frozen at [`Host::start_pipeline`], or
    /// [`DrivePlan::UntilIdle`] if the run was started with raw [`Host::start_run`].
    pub fn drive(&mut self, run_id: &str) -> Result<Run, FrameworkError> {
        let plan = self
            .run_policies
            .get(run_id)
            .map(|p| p.effective_drive())
            .unwrap_or(DrivePlan::UntilIdle);
        self.drive_with(run_id, plan)
    }

    /// Drive with an explicit plan (ignores stored policy's drive field; still
    /// honours `claim_modules` when a policy is stored).
    pub fn drive_with(&mut self, run_id: &str, plan: DrivePlan) -> Result<Run, FrameworkError> {
        let plan = match plan {
            DrivePlan::UntilIdleThenWait => DrivePlan::UntilIdle,
            other => other,
        };
        match plan {
            DrivePlan::OnePass => self.drive_one_pass(run_id),
            DrivePlan::UntilIdle | DrivePlan::UntilIdleThenWait => self.drive_until_idle(run_id),
        }
    }

    fn claim_module_names(&self, run_id: &str) -> Vec<String> {
        let all: Vec<String> = self.plugins.keys().cloned().collect();
        match self.run_policies.get(run_id).and_then(|p| p.claim_modules.as_ref()) {
            None => all,
            Some(allow) => all.into_iter().filter(|m| allow.iter().any(|a| a == m)).collect(),
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

    /// Attempt one claim/execute/commit for `module`. Returns whether work ran.
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

        let processing = {
            let plugin = self
                .plugins
                .get(module)
                .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
            plugin.processing_ui(&work)
        };
        if let Some(mut view) = processing {
            fill_processing_ids(&mut view, run_id, &work);
            self.emit_processing(module, view)?;
        }

        let step = self.load_step(run_id, &work)?;
        let output = {
            let plugin = self
                .plugins
                .get_mut(module)
                .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
            plugin.execute(&step)?
        };

        let mut named_outputs = Vec::with_capacity(output.outputs.len());
        for out in &output.outputs {
            let r = self.kernel.put_artifact(
                Artifact {
                    r#type: out.contract.clone(),
                    body: out.body.clone(),
                    produced_by: module.into(),
                    ..Default::default()
                },
                &self.ctx,
            )?;
            named_outputs.push(NamedArtifact {
                name: out.port.clone(),
                artifact: Some(r),
            });
        }

        let result_ui = {
            let plugin = self
                .plugins
                .get(module)
                .ok_or_else(|| FrameworkError::NoPlugin(module.into()))?;
            plugin.result_ui(&work, &named_outputs)
        };
        if let Some(mut view) = result_ui {
            fill_result_ids(&mut view, run_id, &work, &named_outputs);
            self.emit_result(module, view)?;
        }

        self.kernel.commit(
            Derivation {
                run_id: run_id.into(),
                work_id: work.id,
                node_id: work.node_id,
                outputs: named_outputs,
                ..Default::default()
            },
            &self.ctx,
        )?;

        Ok(true)
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
        })
    }

    fn emit_processing(
        &mut self,
        module: &str,
        view: ProcessingView,
    ) -> Result<(), FrameworkError> {
        let artifact_id = self.maybe_put_ui(module, CONTRACT_PROCESSING_VIEW, &view)?;
        self.ui_events.push(UiEvent::Processing { view, artifact_id });
        Ok(())
    }

    fn emit_result(&mut self, module: &str, view: ResultView) -> Result<(), FrameworkError> {
        let artifact_id = self.maybe_put_ui(module, CONTRACT_RESULT_VIEW, &view)?;
        self.ui_events.push(UiEvent::Result { view, artifact_id });
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
            .map_err(|e| FrameworkError::Invalid(format!("serialize ui view: {e}")))?;
        let r = self.kernel.put_artifact(
            Artifact {
                r#type: contract.into(),
                body,
                produced_by: module.into(),
                ..Default::default()
            },
            &self.ctx,
        )?;
        Ok(r.id)
    }
}

fn fill_processing_ids(view: &mut ProcessingView, run_id: &str, work: &WorkItem) {
    if view.run_id.is_empty() {
        view.run_id = run_id.into();
    }
    if view.work_id.is_empty() {
        view.work_id = work.id.clone();
    }
    if view.node_id.is_empty() {
        view.node_id = work.node_id.clone();
    }
    if view.module.is_empty() {
        view.module = work.module.clone();
    }
}

fn fill_result_ids(
    view: &mut ResultView,
    run_id: &str,
    work: &WorkItem,
    outputs: &[NamedArtifact],
) {
    if view.run_id.is_empty() {
        view.run_id = run_id.into();
    }
    if view.work_id.is_empty() {
        view.work_id = work.id.clone();
    }
    if view.node_id.is_empty() {
        view.node_id = work.node_id.clone();
    }
    if view.module.is_empty() {
        view.module = work.module.clone();
    }
    if view.output_ports.is_empty() {
        view.output_ports = outputs.iter().map(|o| o.name.clone()).collect();
    }
}

/// Helper: `NamedArtifact` from a put result.
pub fn named_ref(name: &str, r: &ArtifactRef) -> NamedArtifact {
    NamedArtifact {
        name: name.into(),
        artifact: Some(r.clone()),
    }
}
