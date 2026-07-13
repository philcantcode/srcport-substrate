//! Step lifecycle presentation data (`srcport.ui.v1`).
//!
//! Modules emit structured chrome only — never widgets or shell code.
//! Stages: **Init** → **Progress**\* → **Final**.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use srcport_substrate::{NamedArtifact, WorkItem};

/// Contract ref: step claimed / about to run.
pub const CONTRACT_STEP_INIT: &str = "srcport.ui.v1.StepInit";
/// Contract ref: mid-execute progress (zero or more).
pub const CONTRACT_STEP_PROGRESS: &str = "srcport.ui.v1.StepProgress";
/// Contract ref: step finished (success or failure presentation).
pub const CONTRACT_STEP_FINAL: &str = "srcport.ui.v1.StepFinal";

/// Legacy contract (prefer [`CONTRACT_STEP_INIT`] / [`CONTRACT_STEP_PROGRESS`]).
pub const CONTRACT_PROCESSING_VIEW: &str = "srcport.ui.v1.ProcessingView";
/// Legacy contract (prefer [`CONTRACT_STEP_FINAL`]).
pub const CONTRACT_RESULT_VIEW: &str = "srcport.ui.v1.ResultView";

/// Stage in the per-work-unit presentation lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStage {
    /// After claim, before domain execute.
    Init,
    /// During execute (`StepContext::emit_progress`).
    Progress,
    /// After outputs or on step error.
    Final,
}

impl StepStage {
    /// Contract ref for this stage's artifact type.
    pub fn contract_ref(self) -> &'static str {
        match self {
            StepStage::Init => CONTRACT_STEP_INIT,
            StepStage::Progress => CONTRACT_STEP_PROGRESS,
            StepStage::Final => CONTRACT_STEP_FINAL,
        }
    }
}

/// Coarse presentation status for product chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PresentationStatus {
    /// Not started.
    Pending,
    /// In flight.
    #[default]
    Running,
    /// Waiting (host may surface without a claim).
    Blocked,
    /// Succeeded with useful result.
    Ok,
    /// Succeeded with nothing useful to show.
    Empty,
    /// Failed (product presentation; kernel error is separate).
    Failed,
}

/// Structured presentation payload — no UI toolkit types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    /// Lifecycle stage (also selects contract ref when persisted).
    pub stage: StepStage,
    /// Short label for chrome.
    pub title: String,
    /// Coarse status.
    pub status: PresentationStatus,
    /// Optional detail line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional 0..=1 progress (mainly [`StepStage::Progress`]).
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
    /// Capability name (host fills if empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub capability: String,
    /// Optional product phase label (e.g. `"scan"`, `"write"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Output port names the shell may highlight (mainly final).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub highlight_ports: Vec<String>,
    /// Output port names produced (mainly final).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_ports: Vec<String>,
    /// Free-form string hints (icon key, locale key, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            stage: StepStage::Progress,
            title: String::new(),
            status: PresentationStatus::Running,
            detail: None,
            progress: None,
            run_id: String::new(),
            work_id: String::new(),
            node_id: String::new(),
            module: String::new(),
            capability: String::new(),
            phase: None,
            highlight_ports: Vec::new(),
            output_ports: Vec::new(),
            meta: BTreeMap::new(),
        }
    }
}

impl Presentation {
    /// Build an init presentation.
    pub fn init(title: impl Into<String>) -> Self {
        Self {
            stage: StepStage::Init,
            title: title.into(),
            status: PresentationStatus::Running,
            ..Default::default()
        }
    }

    /// Build a progress presentation (`fraction` is optional 0..=1).
    pub fn progress(title: impl Into<String>, fraction: Option<f64>) -> Self {
        Self {
            stage: StepStage::Progress,
            title: title.into(),
            status: PresentationStatus::Running,
            progress: fraction,
            ..Default::default()
        }
    }

    /// Set detail line (builder style).
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set product phase label (builder style).
    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }

    /// Build a successful final presentation.
    pub fn final_ok(title: impl Into<String>) -> Self {
        Self {
            stage: StepStage::Final,
            title: title.into(),
            status: PresentationStatus::Ok,
            progress: Some(1.0),
            ..Default::default()
        }
    }

    /// Build a failed final presentation.
    pub fn final_failed(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            stage: StepStage::Final,
            title: title.into(),
            status: PresentationStatus::Failed,
            detail: Some(detail.into()),
            ..Default::default()
        }
    }

    /// Fill identity fields from a work item when empty.
    pub fn fill_identity(&mut self, run_id: &str, work: &WorkItem) {
        if self.run_id.is_empty() {
            self.run_id = run_id.into();
        }
        if self.work_id.is_empty() {
            self.work_id = work.id.clone();
        }
        if self.node_id.is_empty() {
            self.node_id = work.node_id.clone();
        }
        if self.module.is_empty() {
            self.module = work.module.clone();
        }
        if self.capability.is_empty() {
            self.capability = work.capability.clone();
        }
    }
}

/// Outcome of a step for [`crate::ModulePlugin::on_final`].
#[derive(Debug, Clone)]
pub struct StepResult {
    /// Domain execute succeeded and outputs were put.
    pub ok: bool,
    /// Named domain outputs (empty on failure).
    pub outputs: Vec<NamedArtifact>,
    /// Error message when `ok` is false.
    pub error: Option<String>,
}

/// One presentation event observed by the host (and optionally stored as an artifact).
#[derive(Debug, Clone, PartialEq)]
pub struct StepEvent {
    /// Stage for this emit.
    pub stage: StepStage,
    /// Presentation body.
    pub presentation: Presentation,
    /// Artifact id when persisted; empty if host-local only.
    pub artifact_id: String,
}

// ── Legacy view types (mapped into Presentation) ────────────────────────────

/// Legacy processing status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStatus {
    /// Not started.
    Pending,
    /// Running.
    Running,
    /// Blocked.
    Blocked,
    /// Failed.
    Failed,
}

/// Legacy in-flight view (`srcport.ui.v1.ProcessingView`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingView {
    /// Title.
    pub title: String,
    /// Status.
    pub status: ProcessingStatus,
    /// Detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Progress 0..=1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// Run id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    /// Work id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub work_id: String,
    /// Node id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    /// Module.
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

impl From<ProcessingView> for Presentation {
    fn from(v: ProcessingView) -> Self {
        Presentation {
            stage: StepStage::Init,
            title: v.title,
            status: match v.status {
                ProcessingStatus::Pending => PresentationStatus::Pending,
                ProcessingStatus::Running => PresentationStatus::Running,
                ProcessingStatus::Blocked => PresentationStatus::Blocked,
                ProcessingStatus::Failed => PresentationStatus::Failed,
            },
            detail: v.detail,
            progress: v.progress,
            run_id: v.run_id,
            work_id: v.work_id,
            node_id: v.node_id,
            module: v.module,
            ..Default::default()
        }
    }
}

/// Legacy result status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    /// Ok.
    Ok,
    /// Empty.
    Empty,
    /// Failed.
    Failed,
}

/// Legacy result view (`srcport.ui.v1.ResultView`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultView {
    /// Title.
    pub title: String,
    /// Status.
    pub status: ResultStatus,
    /// Summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Run id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    /// Work id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub work_id: String,
    /// Node id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    /// Module.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub module: String,
    /// Output ports.
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

impl From<ResultView> for Presentation {
    fn from(v: ResultView) -> Self {
        Presentation {
            stage: StepStage::Final,
            title: v.title,
            status: match v.status {
                ResultStatus::Ok => PresentationStatus::Ok,
                ResultStatus::Empty => PresentationStatus::Empty,
                ResultStatus::Failed => PresentationStatus::Failed,
            },
            detail: v.summary,
            progress: match v.status {
                ResultStatus::Ok | ResultStatus::Empty => Some(1.0),
                ResultStatus::Failed => None,
            },
            run_id: v.run_id,
            work_id: v.work_id,
            node_id: v.node_id,
            module: v.module,
            output_ports: v.output_ports.clone(),
            highlight_ports: v.output_ports,
            ..Default::default()
        }
    }
}

/// Alias for older code that used `UiEvent`.
pub type UiEvent = StepEvent;
