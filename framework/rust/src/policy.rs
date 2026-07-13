//! Opinionated run modes that compile to kernel [`ExecutionPolicy`] + host drive rules.
//!
//! Presets are the product API; raw kernel fields remain the escape hatch via
//! [`RunMode::Manual`] and [`Host::start_run`](crate::Host::start_run).

use std::collections::{BTreeMap, HashMap};

use srcport_substrate::{Assembly, Closure, ExecutionPolicy, Firing, Limits, RunRequest};

/// Product-facing run mode. Maps to kernel [`Closure`] (and default firing for some presets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunMode {
    /// One-shot pipeline: terminal output → `COMPLETED` (kernel `FIRST_TERMINAL`).
    Converge,
    /// Stay `RUNNING` for more data (kernel `OPEN`); default firing prefers re-fire on inject.
    Stream,
    /// Like [`Stream`], but work-unit identity is `ONCE_PER_KEY` by default.
    DedupeStream,
    /// Subset of assembly nodes (`include_nodes`); converges like [`Converge`] unless overridden.
    /// Pair with [`NodePlan::Only`].
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

/// Which assembly nodes participate (`RunRequest.include_nodes`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NodePlan {
    /// Full assembly.
    #[default]
    All,
    /// Only these assembly node ids (terminal must remain — kernel validates).
    Only(Vec<String>),
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
/// fields (`drive`, `claim_modules`) never enter the kernel.
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
}

impl Default for FrameworkPolicy {
    fn default() -> Self {
        Self::converge()
    }
}

impl FrameworkPolicy {
    /// Classic feed-forward run → first terminal completes the run.
    pub fn converge() -> Self {
        Self {
            mode: RunMode::Converge,
            firing: FiringPlan::CapabilityDefaults,
            nodes: NodePlan::All,
            max_steps: None,
            drive: DrivePlan::UntilIdle,
            claim_modules: None,
        }
    }

    /// Open run that re-fires on new/reinjected inputs (`ALWAYS` default).
    pub fn stream() -> Self {
        Self {
            mode: RunMode::Stream,
            firing: FiringPlan::All(Firing::Always),
            nodes: NodePlan::All,
            max_steps: None,
            drive: DrivePlan::UntilIdleThenWait,
            claim_modules: None,
        }
    }

    /// Open run with `ONCE_PER_KEY` default (modules should mark `Port.key` where needed).
    pub fn stream_dedupe() -> Self {
        Self {
            mode: RunMode::DedupeStream,
            firing: FiringPlan::All(Firing::OncePerKey),
            nodes: NodePlan::All,
            max_steps: None,
            drive: DrivePlan::UntilIdleThenWait,
            claim_modules: None,
        }
    }

    /// Only the given assembly node ids participate; converges on terminal.
    pub fn selective(node_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            mode: RunMode::Selective,
            firing: FiringPlan::CapabilityDefaults,
            nodes: NodePlan::Only(node_ids.into_iter().map(Into::into).collect()),
            max_steps: None,
            drive: DrivePlan::UntilIdle,
            claim_modules: None,
        }
    }

    /// Escape hatch: pick closure explicitly.
    pub fn manual(closure: Closure) -> Self {
        Self {
            mode: RunMode::Manual { closure },
            firing: FiringPlan::CapabilityDefaults,
            nodes: NodePlan::All,
            max_steps: None,
            drive: DrivePlan::UntilIdle,
            claim_modules: None,
        }
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
    pub fn include_nodes(&self) -> Vec<String> {
        match &self.nodes {
            NodePlan::All => Vec::new(),
            NodePlan::Only(ids) => ids.clone(),
        }
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
