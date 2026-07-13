//! Opinionated run modes that compile to kernel [`ExecutionPolicy`] + host drive rules.
//!
//! Presets are the product API; raw kernel fields remain the escape hatch via
//! [`RunMode::Manual`] and [`Host::start_run`](crate::Host::start_run).

use std::collections::{BTreeMap, HashMap};

use srcport_substrate::{Assembly, Closure, ExecutionPolicy, Firing, Limits, RunRequest};

use crate::memo::MemoPlan;
use crate::storage::StoragePlan;

/// Product-facing run mode. Maps to kernel [`Closure`] (and default firing for some presets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunMode {
    /// One-shot pipeline: terminal output → `COMPLETED` (kernel `FIRST_TERMINAL`).
    Converge,
    /// Stay `RUNNING` for more data (kernel `OPEN`); default firing prefers re-fire on inject.
    Stream,
    /// Like [`Stream`], but work-unit identity is `ONCE_PER_KEY` by default.
    DedupeStream,
    /// Subset of assembly nodes; converges like [`Converge`] unless overridden.
    /// Pair with [`NodePlan::Only`], [`NodePlan::After`], or [`NodePlan::From`].
    /// The host materialises a cut (rewrites crossing edges to seed inputs).
    Selective,
    /// Caller picks closure; firing/nodes/drive still apply.
    Manual {
        /// Kernel run closure policy.
        closure: Closure,
    },
}

/// How nodes may fire within the run (kernel `Firing` / `ExecutionPolicy`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FiringPlan {
    /// Honour each capability's `firing` from `Register` (kernel default resolution).
    #[default]
    CapabilityDefaults,
    /// Force the same firing on every node.
    All(Firing),
    /// Default plus per **assembly node id** overrides.
    Map {
        /// Used when a node is not listed in `by_node`.
        default: Firing,
        /// Assembly node id → firing.
        by_node: HashMap<String, Firing>,
    },
}

/// Which assembly nodes participate in a run.
///
/// Non-[`All`] plans are **cut** by the host before `StartRun`: dropped nodes
/// are removed and edges that cross the cut become synthetic `__seed/…` run
/// inputs (see [`crate::materialize_cut`]). The kernel still sees a normal
/// acyclic assembly — there is no hard-coded step index.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NodePlan {
    /// Full assembly.
    #[default]
    All,
    /// Only these assembly node ids (terminal must remain).
    /// Crossing edges from omitted producers become required seed inputs.
    Only(Vec<String>),
    /// Start **after** this node: drop it and its transitive predecessors;
    /// keep parallel branches and everything else. Seed outputs of the cut.
    After(String),
    /// Start **from** this node: keep it and nodes reachable from it (must
    /// include the terminal). Seed every crossing edge into the kept set.
    From(String),
}

/// How [`crate::Host::drive`] schedules claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrivePlan {
    /// Claim modules until a full pass finds no work or the run leaves `RUNNING`.
    #[default]
    UntilIdle,
    /// Single round-robin over claimable modules, then return.
    OnePass,
    /// Same as [`UntilIdle`] for an open run: drain ready work, then return while still
    /// `RUNNING` so the caller can inject later. Named for product clarity on stream modes.
    UntilIdleThenWait,
}

/// After [`crate::Host::inject`], optionally re-drive the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DriveAfter {
    /// Only inject; caller will drive later.
    #[default]
    No,
    /// `drive` with [`DrivePlan::UntilIdle`] / wait semantics.
    UntilIdle,
    /// `drive` with [`DrivePlan::OnePass`].
    OnePass,
}

/// Opinionated framework policy for one pipeline run.
///
/// Compiles to kernel `ExecutionPolicy`, `include_nodes`, and `Limits`. Host-only
/// fields (`drive`, `claim_modules`, `storage`, `memo`) never enter the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameworkPolicy {
    /// Named product mode.
    pub mode: RunMode,
    /// Work-unit firing plan.
    pub firing: FiringPlan,
    /// Assembly node filter.
    pub nodes: NodePlan,
    /// Optional cap on committed work units. `None` → sensible default for the mode.
    pub max_steps: Option<u64>,
    /// Host claim loop style.
    pub drive: DrivePlan,
    /// Soft allow-list of **module names** the host will claim.
    /// `None` = all registered plugins. Does not remove nodes from the assembly.
    pub claim_modules: Option<Vec<String>>,
    /// Optional tabular storage phase (module tables + optional step log).
    pub storage: StoragePlan,
    /// Optional cross-run work memoisation (requires [`crate::Host::with_memo`]).
    pub memo: MemoPlan,
}

impl Default for FrameworkPolicy {
    fn default() -> Self {
        Self::converge()
    }
}

impl FrameworkPolicy {
    fn base(
        mode: RunMode,
        firing: FiringPlan,
        nodes: NodePlan,
        drive: DrivePlan,
    ) -> Self {
        Self {
            mode,
            firing,
            nodes,
            max_steps: None,
            drive,
            claim_modules: None,
            storage: StoragePlan::off(),
            memo: MemoPlan::off(),
        }
    }

    /// Classic feed-forward run → first terminal completes the run.
    pub fn converge() -> Self {
        Self::base(
            RunMode::Converge,
            FiringPlan::CapabilityDefaults,
            NodePlan::All,
            DrivePlan::UntilIdle,
        )
    }

    /// Converge with work memoisation enabled (requires [`crate::Host::with_memo`]).
    ///
    /// Nodes that declare a non-empty [`crate::ModulePlugin::module_digest`] and
    /// whose input artifact ids match a prior successful run skip `execute`.
    pub fn memoized() -> Self {
        Self::converge().with_memo(MemoPlan::on())
    }

    /// Open run that re-fires on new/reinjected inputs (`ALWAYS` default).
    pub fn stream() -> Self {
        Self::base(
            RunMode::Stream,
            FiringPlan::All(Firing::Always),
            NodePlan::All,
            DrivePlan::UntilIdleThenWait,
        )
    }

    /// Open run with `ONCE_PER_KEY` default (modules should mark `Port.key` where needed).
    pub fn stream_dedupe() -> Self {
        Self::base(
            RunMode::DedupeStream,
            FiringPlan::All(Firing::OncePerKey),
            NodePlan::All,
            DrivePlan::UntilIdleThenWait,
        )
    }

    /// Only the given assembly node ids participate; converges on terminal.
    ///
    /// Crossing edges from omitted nodes become `__seed/…` inputs that must be
    /// supplied on `start_pipeline` (or via [`crate::seeds_from_run`]).
    pub fn selective(node_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::base(
            RunMode::Selective,
            FiringPlan::CapabilityDefaults,
            NodePlan::Only(node_ids.into_iter().map(Into::into).collect()),
            DrivePlan::UntilIdle,
        )
    }

    /// Skip `node` and its transitive predecessors; run the rest (seed cut edges).
    ///
    /// Example: after `extract` has run (or you have fixture facts), continue
    /// with `retrieve` + `write` without re-executing extract.
    pub fn start_after(node: impl Into<String>) -> Self {
        Self::base(
            RunMode::Selective,
            FiringPlan::CapabilityDefaults,
            NodePlan::After(node.into()),
            DrivePlan::UntilIdle,
        )
    }

    /// Run only `node` and nodes reachable from it (must reach terminal); seed the rest.
    pub fn from_node(node: impl Into<String>) -> Self {
        Self::base(
            RunMode::Selective,
            FiringPlan::CapabilityDefaults,
            NodePlan::From(node.into()),
            DrivePlan::UntilIdle,
        )
    }

    /// Escape hatch: pick closure explicitly.
    pub fn manual(closure: Closure) -> Self {
        Self::base(
            RunMode::Manual { closure },
            FiringPlan::CapabilityDefaults,
            NodePlan::All,
            DrivePlan::UntilIdle,
        )
    }

    /// Override firing plan.
    pub fn with_firing(mut self, firing: FiringPlan) -> Self {
        self.firing = firing;
        self
    }

    /// Override node plan.
    pub fn with_nodes(mut self, nodes: NodePlan) -> Self {
        self.nodes = nodes;
        self
    }

    /// Override drive plan.
    pub fn with_drive(mut self, drive: DrivePlan) -> Self {
        self.drive = drive;
        self
    }

    /// Cap committed work units (kernel `Limits.max_steps`).
    pub fn with_max_steps(mut self, max_steps: u64) -> Self {
        self.max_steps = Some(max_steps);
        self
    }

    /// Soft host claim filter (module names).
    pub fn with_claim_modules(
        mut self,
        modules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.claim_modules = Some(modules.into_iter().map(Into::into).collect());
        self
    }

    /// Optional tabular storage phase ([`StoragePlan`]).
    ///
    /// Requires a [`crate::StorageBackend`] on the host (`Host::with_storage`).
    pub fn with_storage(mut self, storage: StoragePlan) -> Self {
        self.storage = storage;
        self
    }

    /// Optional cross-run memoisation ([`MemoPlan`]).
    ///
    /// Requires a [`crate::MemoStore`] on the host (`Host::with_memo`).
    pub fn with_memo(mut self, memo: MemoPlan) -> Self {
        self.memo = memo;
        self
    }

    /// Kernel closure for this mode.
    pub fn closure(&self) -> Closure {
        match &self.mode {
            RunMode::Converge | RunMode::Selective => Closure::FirstTerminal,
            RunMode::Stream | RunMode::DedupeStream => Closure::Open,
            RunMode::Manual { closure } => *closure,
        }
    }

    /// Build kernel [`ExecutionPolicy`].
    ///
    /// When `assembly` is provided and firing is [`FiringPlan::All`], every
    /// assembly node id is pinned in `by_node`. That matters because the kernel
    /// resolves **capability.firing before policy.default** — a bare default
    /// would not override module-declared `ONCE`.
    pub fn execution_policy(&self) -> ExecutionPolicy {
        self.execution_policy_for(None)
    }

    /// Like [`FrameworkPolicy::execution_policy`], optionally pinning `All` onto assembly nodes.
    pub fn execution_policy_for(&self, assembly: Option<&Assembly>) -> ExecutionPolicy {
        let (default, by_node) = match &self.firing {
            FiringPlan::CapabilityDefaults => {
                // Unspecified default → kernel uses capability / ONCE.
                (Firing::Unspecified as i32, BTreeMap::new())
            }
            FiringPlan::All(f) => {
                let mut by_node = BTreeMap::new();
                if let Some(a) = assembly {
                    for n in &a.nodes {
                        by_node.insert(n.id.clone(), *f as i32);
                    }
                }
                // Also set default for any edge case / future nodes.
                (*f as i32, by_node)
            }
            FiringPlan::Map { default, by_node } => (
                *default as i32,
                by_node
                    .iter()
                    .map(|(k, v)| (k.clone(), *v as i32))
                    .collect::<BTreeMap<_, _>>(),
            ),
        };
        ExecutionPolicy {
            default,
            by_node,
            closure: self.closure() as i32,
        }
    }

    /// `include_nodes` for the run request (empty = all).
    ///
    /// When the host has already materialised a cut assembly, it forces
    /// [`NodePlan::All`] on the kernel request so nodes are not filtered twice.
    /// This method remains useful for callers that pass a full assembly and an
    /// explicit [`NodePlan::Only`] list without host cut materialisation.
    pub fn include_nodes(&self) -> Vec<String> {
        match &self.nodes {
            NodePlan::All | NodePlan::After(_) | NodePlan::From(_) => Vec::new(),
            NodePlan::Only(ids) => ids.clone(),
        }
    }

    /// True when the host must materialise a cut (drop nodes + seed rebind).
    pub fn needs_cut(&self) -> bool {
        !matches!(self.nodes, NodePlan::All)
    }

    /// Resolve `max_steps` for the kernel.
    ///
    /// - explicit `max_steps` wins
    /// - converge/selective: `0` (kernel = node count)
    /// - stream modes: large bound from node count
    pub fn resolve_max_steps(&self, assembly: &Assembly) -> u64 {
        if let Some(n) = self.max_steps {
            return n;
        }
        self.default_max_steps(assembly.nodes.len() as u64)
    }

    fn default_max_steps(&self, node_count: u64) -> u64 {
        if self.closure() == Closure::Open {
            node_count.saturating_mul(10_000).max(10_000)
        } else {
            0 // kernel: zero ⇒ number of nodes
        }
    }

    /// Apply this policy onto a partially filled [`RunRequest`] (id/assembly/inputs set by caller).
    pub fn apply_to_run_request(&self, mut req: RunRequest) -> RunRequest {
        let node_count = req
            .assembly
            .as_ref()
            .map(|a| a.nodes.len() as u64)
            .unwrap_or(0);
        let max_steps = self
            .max_steps
            .unwrap_or_else(|| self.default_max_steps(node_count));
        req.policy = Some(self.execution_policy_for(req.assembly.as_ref()));
        req.include_nodes = self.include_nodes();
        req.limits = Some(Limits { max_steps });
        req
    }

    /// Effective drive plan for host loops (`UntilIdleThenWait` ≡ until-idle drain).
    pub fn effective_drive(&self) -> DrivePlan {
        match self.drive {
            DrivePlan::UntilIdleThenWait => DrivePlan::UntilIdle,
            other => other,
        }
    }
}
