//! # srcport-substrate — Rust SDK (v0.1, in-process)
//!
//! One pluggable core: **seven primitives** (Module · Artifact · Contract ·
//! Event · Ledger · Gate · Registry) and **one ABI** (the [`Kernel`]). This
//! crate realises the ABI *in-process* — the [`Kernel`] methods mirror the
//! `service Kernel` RPCs in `substrate.proto` one-for-one — and upholds every
//! invariant in `SPEC.md`. The wire types are generated from the canonical
//! proto (see `build.rs`); nothing here re-derives the contract.
//!
//! The kernel knows about no domain. Targets, findings, entities, terrain and
//! content are all Modules built *on top* of this, coupling only through
//! contract refs. See `SPEC.md` for the two-page human-owned specification.

use std::collections::HashMap;
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
        let prev_hash = state.ledger.last().map(|e| e.hash.clone()).unwrap_or_default();
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
        }
        let name = manifest.name.clone();
        state.modules.push(ModuleSlot {
            manifest,
            lifecycle: Lifecycle::Registered,
        });
        Self::append_locked(&mut state, "module.registered", &name, Vec::new());
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
            state.artifacts.insert(id.clone(), artifact);
            Self::append_locked(&mut state, "artifact.put", &id, Vec::new());
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
        Self::append_locked(&mut state, "event.published", &event.topic, Vec::new());
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
