//! # srcport-substrate — Rust SDK (v0.1, in-process)
//!
//! One pluggable core: **eight primitives** (Module · Artifact · Contract ·
//! Event · Ledger · Gate · Registry · Run) and **one ABI** (the [`Kernel`]). This
//! crate realises the ABI *in-process* — the [`Kernel`] methods mirror the
//! `service Kernel` RPCs in `substrate.proto` one-for-one — and upholds every
//! invariant in `SPEC.md`. The wire types are generated from the canonical
//! proto (see `build.rs`); nothing here re-derives the contract.
//!
//! The kernel knows about no domain. Targets, findings, entities, terrain and
//! content are all Modules built *on top* of this, coupling only through
//! contract refs. See `SPEC.md` for the two-page human-owned specification.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Condvar, Mutex};

use sha2::{Digest, Sha256};

/// The generated protobuf types. These ARE the contract — do not hand-edit;
/// change `substrate.proto` and rebuild.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/srcport.substrate.v1.rs"));
}

pub use proto::*;

/// The protobuf codec trait, re-exported so callers — and the conformance
/// suite — can encode/decode `detail` payloads and artifact bodies without
/// taking a direct `prost` dependency.
pub use prost::Message;

// ─────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────

/// Everything that can go wrong at the ABI seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelError {
    /// No artifact / gate exists for the given id.
    NotFound(String),
    /// A [`GateDecision`] carried something other than APPROVED/REJECTED.
    NotADecision,
    /// An irreversible action was attempted while the gate was not APPROVED.
    /// Carries the current [`Decision`] so callers can see *why* it blocked.
    GateBlocked(Decision),
    /// A manifest, assembly, binding, work result, or state transition is invalid.
    Invalid(String),
    /// An id already exists or work has already been claimed/committed.
    Conflict(String),
    /// A terminal run is immutable and accepts no more work.
    RunClosed(RunState),
}

impl std::fmt::Display for KernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KernelError::NotFound(id) => write!(f, "not found: {id}"),
            KernelError::NotADecision => {
                write!(f, "gate decision must be APPROVED or REJECTED")
            }
            KernelError::GateBlocked(d) => {
                write!(f, "gate blocked: decision is {d:?}, not APPROVED")
            }
            KernelError::Invalid(reason) => write!(f, "invalid: {reason}"),
            KernelError::Conflict(reason) => write!(f, "conflict: {reason}"),
            KernelError::RunClosed(state) => write!(f, "run is closed: {state:?}"),
        }
    }
}

impl std::error::Error for KernelError {}

pub type Result<T> = std::result::Result<T, KernelError>;

// ─────────────────────────────────────────────────────────────────────────
// Content addressing & ledger hashing — the two hash rules the spec pins down.
// ─────────────────────────────────────────────────────────────────────────

const SEP: u8 = 0x00;

/// The Artifact address: `id = "sha256:" + hex(sha256(type + 0x00 + body))`.
///
/// Same `(type, body)` ⇒ same id; a one-byte change ⇒ a different id. `meta`
/// and `produced_by` are deliberately NOT part of the address.
pub fn artifact_id(r#type: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(r#type.as_bytes());
    h.update([SEP]);
    h.update(body);
    format!("sha256:{}", hex(&h.finalize()))
}

/// A ledger entry's hash: `sha256` over `(seq, kind, subject, detail, prev_hash)`
/// in that order, each field length-delimited by a `0x00` separator.
fn ledger_hash(seq: u64, kind: &str, subject: &str, detail: &[u8], prev_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(seq.to_be_bytes());
    h.update([SEP]);
    h.update(kind.as_bytes());
    h.update([SEP]);
    h.update(subject.as_bytes());
    h.update([SEP]);
    h.update(detail);
    h.update([SEP]);
    h.update(prev_hash.as_bytes());
    hex(&h.finalize())
}

/// Verify a ledger chain end-to-end: every entry's `hash` must recompute from
/// its own fields, and every `prev_hash` must equal the previous entry's `hash`
/// (genesis links to `""`). Tampering with any committed entry breaks this.
pub fn verify_chain(entries: &[LedgerEntry]) -> bool {
    let mut prev = String::new();
    for (i, e) in entries.iter().enumerate() {
        if e.seq != i as u64 || e.prev_hash != prev {
            return false;
        }
        let recomputed = ledger_hash(e.seq, &e.kind, &e.subject, &e.detail, &e.prev_hash);
        if recomputed != e.hash {
            return false;
        }
        prev = e.hash.clone();
    }
    true
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0x0f) as usize] as char);
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────
// The Kernel — the one ABI. Methods mirror `service Kernel` in the proto.
// ─────────────────────────────────────────────────────────────────────────

/// One live subscription: the topics it wants, and the channel we push to.
struct Sub {
    topics: Vec<String>,
    tx: Sender<Event>,
}

/// A registered module plus its current lifecycle state.
struct ModuleSlot {
    manifest: ModuleManifest,
    lifecycle: Lifecycle,
}

#[derive(Clone)]
struct RunSlot {
    run: Run,
    claimed: HashMap<String, WorkItem>,
    committed: HashMap<String, Derivation>,
}

/// Everything the kernel owns, behind one lock. Small on purpose.
#[derive(Default)]
struct State {
    modules: Vec<ModuleSlot>,
    capabilities: Vec<Capability>,
    contracts: HashMap<String, Contract>,
    artifacts: HashMap<String, Artifact>,
    subs: Vec<Sub>,
    ledger: Vec<LedgerEntry>,
    gates: HashMap<String, GateDecision>,
    runs: HashMap<String, RunSlot>,
    derivations: Vec<Derivation>,
    event_seq: u64,
    gate_counter: u64,
}

/// The in-process microkernel. `Send + Sync`; share it behind an `Arc` across
/// module threads. Every meaningful action lands one append-only ledger entry.
pub struct Kernel {
    state: Mutex<State>,
    /// Signalled whenever a gate decision changes, so [`Kernel::await_gate`] wakes.
    gate_signal: Condvar,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    pub fn new() -> Self {
        Kernel {
            state: Mutex::new(State::default()),
            gate_signal: Condvar::new(),
        }
    }

    // ── ledger helper: the ONLY way entries are ever created ───────────────
    // Caller must hold the state lock. Returns the committed entry.
    fn append_locked(state: &mut State, kind: &str, subject: &str, detail: Vec<u8>) -> LedgerEntry {
        let seq = state.ledger.len() as u64;
        let prev_hash = state
            .ledger
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_default();
        let hash = ledger_hash(seq, kind, subject, &detail, &prev_hash);
        let entry = LedgerEntry {
            seq,
            kind: kind.to_string(),
            subject: subject.to_string(),
            detail,
            prev_hash,
            hash,
        };
        state.ledger.push(entry.clone());
        entry
    }

    // ── 1. MODULE ─────────────────────────────────────────────────────────

    /// `rpc Register(ModuleManifest) -> RegisterAck`. Records the module, its
    /// capabilities, and (implicitly) the contracts those capabilities speak.
    /// The module lands in `REGISTERED`; advance it with [`Kernel::transition`].
    pub fn register(&self, manifest: ModuleManifest) -> RegisterAck {
        let mut state = self.state.lock().unwrap();
        for cap in &manifest.provides {
            state.capabilities.push(cap.clone());
            // Registering a capability registers the contract it speaks.
            state
                .contracts
                .entry(cap.contract.clone())
                .or_insert_with(|| Contract {
                    r#ref: cap.contract.clone(),
                    schema: String::new(),
                });
            for port in cap.inputs.iter().chain(cap.outputs.iter()) {
                state
                    .contracts
                    .entry(port.contract.clone())
                    .or_insert_with(|| Contract {
                        r#ref: port.contract.clone(),
                        schema: String::new(),
                    });
            }
        }
        let name = manifest.name.clone();
        // The full manifest lands in the tamper-evident chain, so the registry
        // is reconstructable from the ledger alone. `detail` is the canonical
        // ModuleManifest; see SPEC.md "Ledger detail".
        let detail = manifest.encode_to_vec();
        state.modules.push(ModuleSlot {
            manifest,
            lifecycle: Lifecycle::Registered,
        });
        Self::append_locked(&mut state, "module.registered", &name, detail);
        RegisterAck {
            state: Lifecycle::Registered as i32,
        }
    }

    /// Advance a module along `REGISTERED → LOADED → ACTIVE → DEACTIVATED`.
    /// Only forward moves are honoured; anything else is a no-op returning the
    /// current state. Returns `NotFound` if the module was never registered.
    pub fn transition(&self, module: &str, to: Lifecycle) -> Result<Lifecycle> {
        let mut state = self.state.lock().unwrap();
        let slot = state
            .modules
            .iter_mut()
            .find(|m| m.manifest.name == module)
            .ok_or_else(|| KernelError::NotFound(module.to_string()))?;
        if to as i32 == slot.lifecycle as i32 + 1 {
            slot.lifecycle = to;
            let kind = format!("module.{}", lifecycle_verb(to));
            Self::append_locked(&mut state, &kind, module, Vec::new());
        }
        let now = state
            .modules
            .iter()
            .find(|m| m.manifest.name == module)
            .map(|m| m.lifecycle)
            .unwrap();
        Ok(now)
    }

    // ── 2. ARTIFACT ────────────────────────────────────────────────────────

    /// `rpc PutArtifact(Artifact) -> ArtifactRef`. Content-addresses the value,
    /// stores it **immutably** (first write wins — a later put of the same id
    /// never mutates what is stored), and returns its ref.
    pub fn put_artifact(&self, mut artifact: Artifact) -> ArtifactRef {
        let id = artifact_id(&artifact.r#type, &artifact.body);
        artifact.id = id.clone();
        let mut state = self.state.lock().unwrap();
        // Immutability: only the first writer of an id sets the stored bytes.
        if !state.artifacts.contains_key(&id) {
            // The ledger commits to everything but the body: `subject` (the id)
            // already addresses (type, body), so re-inlining the body would
            // duplicate the store into a log it can never prune. `detail` is the
            // canonical Artifact with `body` cleared; see SPEC.md "Ledger detail".
            let body = std::mem::take(&mut artifact.body);
            let detail = artifact.encode_to_vec();
            artifact.body = body;
            state.artifacts.insert(id.clone(), artifact);
            Self::append_locked(&mut state, "artifact.put", &id, detail);
        }
        ArtifactRef { id }
    }

    /// `rpc GetArtifact(ArtifactRef) -> Artifact`. Reads back byte-identical.
    pub fn get_artifact(&self, r#ref: &ArtifactRef) -> Result<Artifact> {
        let state = self.state.lock().unwrap();
        state
            .artifacts
            .get(&r#ref.id)
            .cloned()
            .ok_or_else(|| KernelError::NotFound(r#ref.id.clone()))
    }

    // ── 3. CONTRACT ────────────────────────────────────────────────────────

    /// Register (or attach schema text to) a contract explicitly. Capabilities
    /// already auto-register their contract ref via [`Kernel::register`]; use
    /// this when you also want to carry the schema.
    pub fn put_contract(&self, contract: Contract) {
        let mut state = self.state.lock().unwrap();
        state.contracts.insert(contract.r#ref.clone(), contract);
    }

    // ── 4. EVENT ───────────────────────────────────────────────────────────

    /// `rpc Subscribe(Subscription) -> stream Event`. In-process the "stream"
    /// is an mpsc [`Receiver`]; events arrive in kernel `seq` order. A module
    /// only ever receives events on topics it named here.
    pub fn subscribe(&self, sub: Subscription) -> Receiver<Event> {
        let (tx, rx) = channel();
        let mut state = self.state.lock().unwrap();
        state.subs.push(Sub {
            topics: sub.topics,
            tx,
        });
        rx
    }

    /// `rpc Publish(Event) -> PublishAck`. Assigns a monotonic `seq` (the total
    /// order), delivers to exactly the subscribers of `event.topic`, and never
    /// to anyone else. Returns the assigned seq.
    pub fn publish(&self, mut event: Event) -> PublishAck {
        let mut state = self.state.lock().unwrap();
        state.event_seq += 1;
        let seq = state.event_seq;
        event.seq = seq;

        // Deliver in-order to matching subs; drop any whose receiver is gone.
        state.subs.retain(|sub| {
            if sub.topics.iter().any(|t| t == &event.topic) {
                sub.tx.send(event.clone()).is_ok()
            } else {
                true
            }
        });
        let mut for_log = event.clone();
        for_log.payload.clear();
        let detail = for_log.encode_to_vec();
        Self::append_locked(&mut state, "event.published", &event.topic, detail);
        PublishAck { seq }
    }

    // ── 5. LEDGER ──────────────────────────────────────────────────────────

    /// `rpc Append(AppendRequest) -> LedgerEntry`. Modules may write their own
    /// domain facts into the same tamper-evident chain.
    pub fn append(&self, req: AppendRequest) -> LedgerEntry {
        let mut state = self.state.lock().unwrap();
        Self::append_locked(&mut state, &req.kind, &req.subject, req.detail)
    }

    /// A snapshot copy of the whole ledger, for verification and audit.
    pub fn ledger(&self) -> Vec<LedgerEntry> {
        self.state.lock().unwrap().ledger.clone()
    }

    /// Verify the kernel's own live ledger. Equivalent to
    /// `verify_chain(&self.ledger())`.
    pub fn verify_ledger(&self) -> bool {
        verify_chain(&self.state.lock().unwrap().ledger)
    }

    // ── 6. GATE ────────────────────────────────────────────────────────────

    /// `rpc RequestGate(GateRequest) -> GateTicket`. Opens a human-held
    /// checkpoint in `PENDING`. The module must not proceed with the guarded
    /// action until a human decides. The full request is committed to the
    /// tamper-evident ledger.
    pub fn request_gate(&self, mut req: GateRequest) -> GateTicket {
        let mut state = self.state.lock().unwrap();
        if req.id.is_empty() {
            state.gate_counter += 1;
            req.id = format!("gate-{}", state.gate_counter);
        }
        let id = req.id.clone();
        state.gates.insert(
            id.clone(),
            GateDecision {
                request_id: id.clone(),
                decision: Decision::Pending as i32,
                decided_by: String::new(),
                reason: String::new(),
            },
        );
        // The full request lands in the tamper-evident chain — action,
        // requested_by, and context are the evidence an auditor (or a human
        // after a restart) reconstructs. `detail` is the canonical encoding of
        // the GateRequest; see SPEC.md "Ledger detail".
        let detail = req.encode_to_vec();
        Self::append_locked(&mut state, "gate.requested", &id, detail);
        GateTicket { request_id: id }
    }

    /// `rpc DecideGate(GateDecision) -> GateDecision`. A human records
    /// APPROVED or REJECTED. Wakes anyone in [`Kernel::await_gate`]. The
    /// decision is committed to the tamper-evident ledger.
    pub fn decide_gate(&self, decision: GateDecision) -> Result<GateDecision> {
        let d = decision.decision();
        if d != Decision::Approved && d != Decision::Rejected {
            return Err(KernelError::NotADecision);
        }
        let mut state = self.state.lock().unwrap();
        if !state.gates.contains_key(&decision.request_id) {
            return Err(KernelError::NotFound(decision.request_id.clone()));
        }
        let id = decision.request_id.clone();
        state.gates.insert(id.clone(), decision.clone());
        // The decision itself — who decided, what, and why — is hash-committed,
        // so the approval record can't be rewritten without breaking the chain.
        // Previously only the gate id landed here, leaving the decision outside
        // the tamper-evident log. `detail` is the canonical GateDecision.
        let detail = decision.encode_to_vec();
        Self::append_locked(&mut state, "gate.decided", &id, detail);
        drop(state);
        self.gate_signal.notify_all();
        Ok(decision)
    }

    /// `rpc AwaitGate(GateTicket) -> GateDecision`. Blocks until the gate is no
    /// longer `PENDING`, then returns the human's decision.
    pub fn await_gate(&self, ticket: &GateTicket) -> Result<GateDecision> {
        let mut state = self.state.lock().unwrap();
        loop {
            match state.gates.get(&ticket.request_id) {
                None => return Err(KernelError::NotFound(ticket.request_id.clone())),
                Some(g) if g.decision() != Decision::Pending => return Ok(g.clone()),
                Some(_) => state = self.gate_signal.wait(state).unwrap(),
            }
        }
    }

    /// Non-blocking peek at a gate's current decision.
    pub fn gate_status(&self, ticket: &GateTicket) -> Result<Decision> {
        let state = self.state.lock().unwrap();
        state
            .gates
            .get(&ticket.request_id)
            .map(|g| g.decision())
            .ok_or_else(|| KernelError::NotFound(ticket.request_id.clone()))
    }

    /// The non-bypass guard. Returns `Ok(())` **only** when the gate is
    /// APPROVED; `PENDING` and `REJECTED` both return `GateBlocked`. Call this
    /// immediately before an irreversible act.
    pub fn ensure_approved(&self, ticket: &GateTicket) -> Result<()> {
        match self.gate_status(ticket)? {
            Decision::Approved => Ok(()),
            other => Err(KernelError::GateBlocked(other)),
        }
    }

    // ── 7. REGISTRY ────────────────────────────────────────────────────────

    /// `rpc Snapshot(SnapshotRequest) -> RegistrySnapshot`. "What exists right
    /// now": every registered module, capability, and contract.
    pub fn snapshot(&self) -> RegistrySnapshot {
        let state = self.state.lock().unwrap();
        RegistrySnapshot {
            modules: state.modules.iter().map(|m| m.manifest.clone()).collect(),
            capabilities: state.capabilities.clone(),
            contracts: state.contracts.values().cloned().collect(),
        }
    }

    // ── 8. RUN / CONVERGENCE ─────────────────────────────────────────────

    /// Validate and start one finite feed-forward assembly. The graph, module
    /// versions, bindings, input artifacts, and terminal output are frozen into
    /// the returned Run and committed to the ledger.
    pub fn start_run(&self, req: RunRequest) -> Result<Run> {
        let mut state = self.state.lock().unwrap();
        if req.id.is_empty() {
            return Err(KernelError::Invalid("run id is required".into()));
        }
        if state.runs.contains_key(&req.id) {
            return Err(KernelError::Conflict(format!(
                "run {} already exists",
                req.id
            )));
        }
        let assembly = req
            .assembly
            .clone()
            .ok_or_else(|| KernelError::Invalid("assembly is required".into()))?;
        validate_assembly(&state, &assembly, &req.inputs)?;
        let requested_max = req.limits.as_ref().map(|l| l.max_steps).unwrap_or(0);
        let max_steps = if requested_max == 0 {
            assembly.nodes.len() as u64
        } else {
            requested_max
        };
        if max_steps == 0 {
            return Err(KernelError::Invalid("max_steps must be positive".into()));
        }
        let run = Run {
            id: req.id.clone(),
            assembly: Some(assembly),
            inputs: req.inputs,
            state: RunState::Running as i32,
            answer: None,
            steps: 0,
            max_steps,
            reason: String::new(),
        };
        state.runs.insert(
            run.id.clone(),
            RunSlot {
                run: run.clone(),
                claimed: HashMap::new(),
                committed: HashMap::new(),
            },
        );
        Self::append_locked(&mut state, "run.started", &run.id, run.encode_to_vec());
        Ok(run)
    }

    /// Atomically claim the first ready node for a module. An empty WorkItem
    /// means that this module has no ready node. If the whole graph has neither
    /// ready nor in-flight work, the run closes as STALLED.
    pub fn claim_ready(&self, req: ClaimRequest) -> Result<WorkItem> {
        let mut state = self.state.lock().unwrap();
        let slot = state
            .runs
            .get(&req.run_id)
            .ok_or_else(|| KernelError::NotFound(req.run_id.clone()))?;
        let run_state = slot.run.state();
        if run_state != RunState::Running {
            return Err(KernelError::RunClosed(run_state));
        }

        let assembly = slot.run.assembly.as_ref().unwrap();
        let mut selected = None;
        let mut any_ready = false;
        for node in &assembly.nodes {
            if slot.committed.contains_key(&node.id) || slot.claimed.contains_key(&node.id) {
                continue;
            }
            if let Some(inputs) = resolve_inputs(slot, node) {
                any_ready = true;
                if selected.is_none() && node.module == req.module {
                    selected = Some((node.clone(), inputs));
                }
            }
        }

        if let Some((node, inputs)) = selected {
            let work = WorkItem {
                id: format!("work:{}/{}", req.run_id, node.id),
                run_id: req.run_id.clone(),
                node_id: node.id.clone(),
                module: node.module,
                module_version: node.module_version,
                capability: node.capability,
                inputs,
            };
            state
                .runs
                .get_mut(&req.run_id)
                .unwrap()
                .claimed
                .insert(node.id, work.clone());
            Self::append_locked(&mut state, "work.claimed", &work.id, work.encode_to_vec());
            return Ok(work);
        }

        if !any_ready && slot.claimed.is_empty() {
            let run = &mut state.runs.get_mut(&req.run_id).unwrap().run;
            run.state = RunState::Stalled as i32;
            run.reason = "no node is ready and no work is in flight".into();
            let closed = run.clone();
            Self::append_locked(
                &mut state,
                "run.stalled",
                &closed.id,
                closed.encode_to_vec(),
            );
        }
        Ok(WorkItem::default())
    }

    /// Commit one claimed transformation. The kernel validates every output
    /// contract, records a separate immutable Derivation, releases downstream
    /// work, and closes the run when its declared terminal output appears.
    pub fn commit(&self, submitted: Derivation) -> Result<Run> {
        let mut state = self.state.lock().unwrap();
        let slot = state
            .runs
            .get(&submitted.run_id)
            .ok_or_else(|| KernelError::NotFound(submitted.run_id.clone()))?;
        let run_state = slot.run.state();
        if run_state != RunState::Running {
            return Err(KernelError::RunClosed(run_state));
        }
        let work = slot
            .claimed
            .get(&submitted.node_id)
            .cloned()
            .ok_or_else(|| KernelError::Conflict("node was not claimed".into()))?;
        if submitted.work_id != work.id {
            return Err(KernelError::Invalid(
                "work_id does not match the claim".into(),
            ));
        }
        if slot.run.steps >= slot.run.max_steps {
            return Err(KernelError::RunClosed(RunState::Failed));
        }
        let cap = capability_for_work(&state, &work)?;
        validate_outputs(&state, &cap, &submitted.outputs)?;

        let mut derivation = Derivation {
            id: String::new(),
            run_id: work.run_id.clone(),
            work_id: work.id.clone(),
            node_id: work.node_id.clone(),
            module: work.module.clone(),
            module_version: work.module_version.clone(),
            capability: work.capability.clone(),
            inputs: work.inputs.clone(),
            outputs: submitted.outputs,
        };
        derivation.id = derivation_id(&derivation);

        let run_now = {
            let slot = state.runs.get_mut(&work.run_id).unwrap();
            slot.claimed.remove(&work.node_id);
            slot.committed
                .insert(work.node_id.clone(), derivation.clone());
            slot.run.steps += 1;
            let terminal = slot
                .run
                .assembly
                .as_ref()
                .unwrap()
                .terminal
                .as_ref()
                .unwrap();
            let mut closed = false;
            if terminal.node == work.node_id {
                if let Some(answer) = derivation
                    .outputs
                    .iter()
                    .find(|o| o.name == terminal.port)
                    .and_then(|o| o.artifact.clone())
                {
                    slot.run.answer = Some(answer);
                    slot.run.state = RunState::Completed as i32;
                    closed = true;
                }
            }
            if !closed && slot.run.steps >= slot.run.max_steps {
                slot.run.state = RunState::Failed as i32;
                slot.run.reason = "max_steps exhausted before the terminal output".into();
                closed = true;
            }
            if !closed {
                let any_ready = slot
                    .run
                    .assembly
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|node| {
                        !slot.committed.contains_key(&node.id)
                            && !slot.claimed.contains_key(&node.id)
                            && resolve_inputs(slot, node).is_some()
                    });
                if !any_ready && slot.claimed.is_empty() {
                    slot.run.state = RunState::Stalled as i32;
                    slot.run.reason = "no node is ready and no work is in flight".into();
                }
            }
            slot.run.clone()
        };
        state.derivations.push(derivation.clone());
        Self::append_locked(
            &mut state,
            "derivation.committed",
            &derivation.id,
            derivation.encode_to_vec(),
        );
        let kind = match run_now.state() {
            RunState::Completed => "run.completed",
            RunState::Stalled => "run.stalled",
            RunState::Failed => "run.failed",
            _ => "run.progressed",
        };
        Self::append_locked(&mut state, kind, &run_now.id, run_now.encode_to_vec());
        Ok(run_now)
    }

    /// Read a run's current immutable snapshot.
    pub fn get_run(&self, r: &RunRef) -> Result<Run> {
        self.state
            .lock()
            .unwrap()
            .runs
            .get(&r.id)
            .map(|slot| slot.run.clone())
            .ok_or_else(|| KernelError::NotFound(r.id.clone()))
    }

    /// Explicitly close an unfinished run. Cancellation is terminal and, like
    /// every other closure, is committed to the ledger.
    pub fn cancel_run(&self, r: &RunRef) -> Result<Run> {
        let mut state = self.state.lock().unwrap();
        let slot = state
            .runs
            .get_mut(&r.id)
            .ok_or_else(|| KernelError::NotFound(r.id.clone()))?;
        if slot.run.state() != RunState::Running {
            return Err(KernelError::RunClosed(slot.run.state()));
        }
        slot.run.state = RunState::Cancelled as i32;
        slot.run.reason = "cancelled".into();
        slot.claimed.clear();
        let run = slot.run.clone();
        Self::append_locked(&mut state, "run.cancelled", &run.id, run.encode_to_vec());
        Ok(run)
    }

    /// All committed production paths, including distinct paths that produced
    /// the same content-addressed Artifact.
    pub fn derivations(&self) -> Vec<Derivation> {
        self.state.lock().unwrap().derivations.clone()
    }

    /// `rpc ListDerivations`: every immutable production path for one run.
    pub fn list_derivations(&self, r: &RunRef) -> Result<DerivationList> {
        let state = self.state.lock().unwrap();
        if !state.runs.contains_key(&r.id) {
            return Err(KernelError::NotFound(r.id.clone()));
        }
        Ok(DerivationList {
            derivations: state
                .derivations
                .iter()
                .filter(|d| d.run_id == r.id)
                .cloned()
                .collect(),
        })
    }
}

fn capability_for_node<'a>(state: &'a State, node: &AssemblyNode) -> Result<&'a Capability> {
    let mut matches = state
        .modules
        .iter()
        .filter(|m| m.manifest.name == node.module && m.manifest.version == node.module_version)
        .flat_map(|m| {
            m.manifest
                .provides
                .iter()
                .filter(|c| c.name == node.capability)
        });
    let found = matches.next().ok_or_else(|| {
        KernelError::Invalid(format!(
            "{}@{} does not provide {}",
            node.module, node.module_version, node.capability
        ))
    })?;
    if matches.next().is_some() {
        return Err(KernelError::Invalid(format!(
            "{}@{} provides {} ambiguously",
            node.module, node.module_version, node.capability
        )));
    }
    Ok(found)
}

fn capability_for_work(state: &State, work: &WorkItem) -> Result<Capability> {
    capability_for_node(
        state,
        &AssemblyNode {
            id: work.node_id.clone(),
            module: work.module.clone(),
            module_version: work.module_version.clone(),
            capability: work.capability.clone(),
        },
    )
    .cloned()
}

fn port<'a>(ports: &'a [Port], name: &str) -> Option<&'a Port> {
    ports.iter().find(|p| p.name == name)
}

fn validate_assembly(state: &State, assembly: &Assembly, inputs: &[NamedArtifact]) -> Result<()> {
    if assembly.id.is_empty() {
        return Err(KernelError::Invalid("assembly id is required".into()));
    }
    if assembly.nodes.is_empty() {
        return Err(KernelError::Invalid("assembly has no nodes".into()));
    }
    let terminal = assembly
        .terminal
        .as_ref()
        .ok_or_else(|| KernelError::Invalid("assembly terminal is required".into()))?;
    let mut nodes = HashMap::new();
    for node in &assembly.nodes {
        if node.id.is_empty() || nodes.insert(node.id.as_str(), node).is_some() {
            return Err(KernelError::Invalid(format!(
                "duplicate or empty node id: {}",
                node.id
            )));
        }
        let cap = capability_for_node(state, node)?;
        for ports in [&cap.inputs[..], &cap.outputs[..]] {
            let mut names = HashSet::new();
            for p in ports {
                if p.name.is_empty() || p.contract.is_empty() || !names.insert(&p.name) {
                    return Err(KernelError::Invalid(format!(
                        "{} has an empty or duplicate typed port",
                        node.id
                    )));
                }
            }
        }
    }
    let terminal_node = nodes
        .get(terminal.node.as_str())
        .ok_or_else(|| KernelError::Invalid("terminal node does not exist".into()))?;
    let terminal_port = port(
        &capability_for_node(state, terminal_node)?.outputs,
        &terminal.port,
    );
    if terminal_port.is_none() {
        return Err(KernelError::Invalid(
            "terminal output port does not exist".into(),
        ));
    }
    if terminal_port.unwrap().multiple {
        return Err(KernelError::Invalid(
            "terminal output must be scalar".into(),
        ));
    }

    let mut named_inputs = HashMap::new();
    for input in inputs {
        if input.name.is_empty() || named_inputs.insert(input.name.as_str(), input).is_some() {
            return Err(KernelError::Invalid(format!(
                "duplicate or empty run input: {}",
                input.name
            )));
        }
        let r = input
            .artifact
            .as_ref()
            .ok_or_else(|| KernelError::Invalid(format!("input {} has no artifact", input.name)))?;
        if !state.artifacts.contains_key(&r.id) {
            return Err(KernelError::NotFound(r.id.clone()));
        }
    }

    let mut counts: HashMap<(&str, &str), usize> = HashMap::new();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &assembly.bindings {
        let target_node = nodes.get(b.to_node.as_str()).ok_or_else(|| {
            KernelError::Invalid(format!("binding target {} is unknown", b.to_node))
        })?;
        let target = port(&capability_for_node(state, target_node)?.inputs, &b.to_port)
            .ok_or_else(|| {
                KernelError::Invalid(format!("input port {}.{} is unknown", b.to_node, b.to_port))
            })?;
        *counts.entry((&b.to_node, &b.to_port)).or_default() += 1;
        let upstream = !b.from_node.is_empty() || !b.from_port.is_empty();
        let external = !b.input.is_empty();
        if upstream == external {
            return Err(KernelError::Invalid(
                "binding must have exactly one source".into(),
            ));
        }
        let source_contract = if external {
            let input = named_inputs
                .get(b.input.as_str())
                .ok_or_else(|| KernelError::Invalid(format!("run input {} is unknown", b.input)))?;
            let r = input.artifact.as_ref().unwrap();
            state.artifacts.get(&r.id).unwrap().r#type.as_str()
        } else {
            let source_node = nodes.get(b.from_node.as_str()).ok_or_else(|| {
                KernelError::Invalid(format!("binding source {} is unknown", b.from_node))
            })?;
            let source = port(
                &capability_for_node(state, source_node)?.outputs,
                &b.from_port,
            )
            .ok_or_else(|| {
                KernelError::Invalid(format!(
                    "output port {}.{} is unknown",
                    b.from_node, b.from_port
                ))
            })?;
            edges.entry(&b.from_node).or_default().push(&b.to_node);
            source.contract.as_str()
        };
        if source_contract != target.contract {
            return Err(KernelError::Invalid(format!(
                "contract mismatch at {}.{}: {} != {}",
                b.to_node, b.to_port, source_contract, target.contract
            )));
        }
    }
    for node in &assembly.nodes {
        let cap = capability_for_node(state, node)?;
        for input in &cap.inputs {
            let n = counts
                .get(&(node.id.as_str(), input.name.as_str()))
                .copied()
                .unwrap_or(0);
            if n == 0 && !input.optional {
                return Err(KernelError::Invalid(format!(
                    "required input {}.{} is unbound",
                    node.id, input.name
                )));
            }
            if n > 1 && !input.multiple {
                return Err(KernelError::Invalid(format!(
                    "input {}.{} is not multiple",
                    node.id, input.name
                )));
            }
        }
    }
    fn visit<'a>(
        node: &'a str,
        edges: &HashMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        done: &mut HashSet<&'a str>,
    ) -> bool {
        if done.contains(node) {
            return true;
        }
        if !visiting.insert(node) {
            return false;
        }
        for next in edges.get(node).into_iter().flatten() {
            if !visit(next, edges, visiting, done) {
                return false;
            }
        }
        visiting.remove(node);
        done.insert(node);
        true
    }
    let mut visiting = HashSet::new();
    let mut done = HashSet::new();
    for node in nodes.keys() {
        if !visit(node, &edges, &mut visiting, &mut done) {
            return Err(KernelError::Invalid("assembly contains a cycle".into()));
        }
    }
    Ok(())
}

fn resolve_inputs(slot: &RunSlot, node: &AssemblyNode) -> Option<Vec<NamedArtifact>> {
    let assembly = slot.run.assembly.as_ref()?;
    let mut resolved = Vec::new();
    for b in assembly.bindings.iter().filter(|b| b.to_node == node.id) {
        let artifact = if !b.input.is_empty() {
            slot.run
                .inputs
                .iter()
                .find(|i| i.name == b.input)
                .and_then(|i| i.artifact.clone())
        } else {
            slot.committed
                .get(&b.from_node)
                .and_then(|d| d.outputs.iter().find(|o| o.name == b.from_port))
                .and_then(|o| o.artifact.clone())
        }?;
        resolved.push(NamedArtifact {
            name: b.to_port.clone(),
            artifact: Some(artifact),
        });
    }
    Some(resolved)
}

fn validate_outputs(state: &State, cap: &Capability, outputs: &[NamedArtifact]) -> Result<()> {
    for expected in &cap.outputs {
        let matching: Vec<_> = outputs.iter().filter(|o| o.name == expected.name).collect();
        if matching.is_empty() && !expected.optional {
            return Err(KernelError::Invalid(format!(
                "required output {} is absent",
                expected.name
            )));
        }
        if matching.len() > 1 && !expected.multiple {
            return Err(KernelError::Invalid(format!(
                "output {} is not multiple",
                expected.name
            )));
        }
    }
    for output in outputs {
        let expected = port(&cap.outputs, &output.name)
            .ok_or_else(|| KernelError::Invalid(format!("undeclared output {}", output.name)))?;
        let r = output.artifact.as_ref().ok_or_else(|| {
            KernelError::Invalid(format!("output {} has no artifact", output.name))
        })?;
        let artifact = state
            .artifacts
            .get(&r.id)
            .ok_or_else(|| KernelError::NotFound(r.id.clone()))?;
        if artifact.r#type != expected.contract {
            return Err(KernelError::Invalid(format!(
                "output {} has contract {}, want {}",
                output.name, artifact.r#type, expected.contract
            )));
        }
    }
    Ok(())
}

fn derivation_id(d: &Derivation) -> String {
    let mut h = Sha256::new();
    for value in [
        &d.run_id,
        &d.work_id,
        &d.node_id,
        &d.module,
        &d.module_version,
        &d.capability,
    ] {
        h.update(value.as_bytes());
        h.update([SEP]);
    }
    for value in d.inputs.iter().chain(d.outputs.iter()) {
        h.update(value.name.as_bytes());
        h.update([SEP]);
        if let Some(r) = &value.artifact {
            h.update(r.id.as_bytes());
        }
        h.update([SEP]);
    }
    format!("sha256:{}", hex(&h.finalize()))
}

fn lifecycle_verb(l: Lifecycle) -> &'static str {
    match l {
        Lifecycle::Unspecified => "unspecified",
        Lifecycle::Registered => "registered",
        Lifecycle::Loaded => "loaded",
        Lifecycle::Active => "activated",
        Lifecycle::Deactivated => "deactivated",
    }
}
