//! # srcport-substrate — Rust SDK (v1.1.0, in-process)
//!
//! One pluggable core: **seven primitives** (Module · Artifact · Contract ·
//! Event · Ledger · Registry · Run) and **one ABI** (the [`KernelApi`] trait).
//! This crate realises that ABI *in-process* as [`MemoryKernel`] — whose methods
//! mirror the `service Kernel` RPCs in `substrate.proto` one-for-one — and
//! upholds every invariant in `SPEC.md`. The wire types are generated from the
//! canonical proto (see `build.rs`); nothing here re-derives the contract.
//!
//! **Durability lives in Modules, not the core.** [`MemoryKernel`] keeps all
//! state in memory; it is one implementation of [`KernelApi`], not *the*
//! authority. A consumer that needs to survive a restart implements the same
//! trait over durable storage, or layers persistence as a Module — the core
//! stays small, bounded, and domain-neutral either way.
//!
//! The kernel knows about no domain. Targets, findings, entities, terrain and
//! content are all Modules built *on top* of this, coupling only through
//! contract refs. See `SPEC.md` for the two-page human-owned specification.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Mutex;

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
    /// No artifact / blob / run exists for the given id.
    NotFound(String),
    /// A manifest, assembly, binding, work result, or state transition is invalid.
    Invalid(String),
    /// An id already exists or work has already been claimed/committed.
    Conflict(String),
    /// A terminal run is immutable and accepts no more work.
    RunClosed(RunState),
    /// A call precondition failed (e.g. absolute deadline already passed).
    FailedPrecondition(String),
    /// Stored blob bytes do not match the claimed digest or byte_count.
    BlobIntegrity(String),
}

impl std::fmt::Display for KernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KernelError::NotFound(id) => write!(f, "not found: {id}"),
            KernelError::Invalid(reason) => write!(f, "invalid: {reason}"),
            KernelError::Conflict(reason) => write!(f, "conflict: {reason}"),
            KernelError::RunClosed(state) => write!(f, "run is closed: {state:?}"),
            KernelError::FailedPrecondition(reason) => write!(f, "failed precondition: {reason}"),
            KernelError::BlobIntegrity(reason) => write!(f, "blob integrity: {reason}"),
        }
    }
}

impl std::error::Error for KernelError {}

impl KernelError {
    /// The portable [`ErrorCode`] this failure maps to. Every SDK — in-process
    /// or gRPC, Rust/Go/Python — reports the same code for the same condition,
    /// so failure semantics are identical across languages and over the wire.
    pub fn code(&self) -> ErrorCode {
        match self {
            KernelError::NotFound(_) => ErrorCode::NotFound,
            KernelError::Invalid(_) => ErrorCode::Invalid,
            KernelError::Conflict(_) => ErrorCode::Conflict,
            KernelError::RunClosed(_) | KernelError::FailedPrecondition(_) => {
                ErrorCode::FailedPrecondition
            }
            KernelError::BlobIntegrity(_) => ErrorCode::BlobIntegrity,
        }
    }

    /// Whether re-issuing the identical call may later succeed. None of the
    /// current conditions clear on retry alone; a bounded-buffer backpressure
    /// (`RESOURCE_EXHAUSTED`, wire-only) would be the retryable case.
    pub fn retryable(&self) -> bool {
        false
    }

    /// Project this native error onto the portable [`Error`] wire message, so a
    /// caller across the ABI sees identical failure semantics regardless of the
    /// SDK's implementation language.
    pub fn to_proto(&self) -> Error {
        let mut e = Error {
            code: self.code() as i32,
            message: self.to_string(),
            retryable: self.retryable(),
            conflict_subject: String::new(),
            failed_precondition: String::new(),
        };
        match self {
            KernelError::Conflict(subject) => e.conflict_subject = subject.clone(),
            KernelError::RunClosed(state) => {
                e.failed_precondition = format!("run closed: {state:?}")
            }
            KernelError::FailedPrecondition(reason) | KernelError::BlobIntegrity(reason) => {
                e.failed_precondition = reason.clone()
            }
            _ => {}
        }
        e
    }
}

pub type Result<T> = std::result::Result<T, KernelError>;

// ─────────────────────────────────────────────────────────────────────────
// Content addressing & ledger hashing — the two hash rules the spec pins down.
// ─────────────────────────────────────────────────────────────────────────

const SEP: u8 = 0x00;

/// Advisory ceiling for `Artifact.body`. Larger payloads should `put_blob` and
/// place a verified [`ObjectRef`]. The kernel does not hard-reject oversized
/// inline bodies (backward compat); production modules SHOULD honour this.
pub const MAX_INLINE_ARTIFACT_BYTES: usize = 1 << 20; // 1 MiB

/// Bound on a single subscriber's undelivered-event backlog. The event bus is
/// notification, not the data plane (artifact refs are), and the ledger is the
/// durable record — so rather than let one slow subscriber grow the kernel's
/// memory without bound (an unbounded queue is unsuitable inside an evidence
/// kernel), a subscriber that falls this far behind is shed: its `Receiver`
/// disconnects. Over the wire this surfaces as `ERROR_CODE_RESOURCE_EXHAUSTED`.
pub const SUBSCRIBER_BUFFER: usize = 1024;

/// Pure blob identity: `"sha256:" + hex(sha256(data))`. Namespace and typed
/// Artifact fields are NOT part of blob identity.
pub fn blob_id(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("sha256:{}", hex(&h.finalize()))
}

/// Address payload for an external Artifact value:
/// `digest ‖ 0x00 ‖ uint64_be(byte_count) ‖ 0x00 ‖ namespace`.
pub fn object_ref_bytes(object: &ObjectRef) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        object.digest.len() + 1 + 8 + 1 + object.namespace.len(),
    );
    out.extend_from_slice(object.digest.as_bytes());
    out.push(SEP);
    out.extend_from_slice(&object.byte_count.to_be_bytes());
    out.push(SEP);
    out.extend_from_slice(object.namespace.as_bytes());
    out
}

/// Bytes folded into the Artifact address: inline body, or object_ref_bytes.
pub fn artifact_content(artifact: &Artifact) -> Vec<u8> {
    if let Some(obj) = artifact.object.as_ref() {
        if !obj.digest.is_empty() {
            return object_ref_bytes(obj);
        }
    }
    artifact.body.clone()
}

/// The Artifact address over an explicit content payload:
/// `id = "sha256:" + hex(sha256(type + 0x00 + content))`.
///
/// For inline values pass body; for external values pass [`object_ref_bytes`].
/// Prefer [`artifact_id_of`] when you have a full Artifact. `meta` and
/// `produced_by` are deliberately NOT part of the address.
pub fn artifact_id(r#type: &str, content: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(r#type.as_bytes());
    h.update([SEP]);
    h.update(content);
    format!("sha256:{}", hex(&h.finalize()))
}

/// Typed value address for a full Artifact (inline or external).
pub fn artifact_id_of(artifact: &Artifact) -> String {
    artifact_id(&artifact.r#type, &artifact_content(artifact))
}

/// Whether `artifact` holds a verified external [`ObjectRef`].
pub fn has_external_object(artifact: &Artifact) -> bool {
    artifact
        .object
        .as_ref()
        .map(|o| !o.digest.is_empty())
        .unwrap_or(false)
}

/// Contract content address:
///
/// ```text
/// digest = "sha256:" + hex(sha256(
///   media_type ‖ 0x00 ‖ schema ‖ 0x00 ‖ version ‖ 0x00 ‖
///   compatible_with… (UTF-8 ascending; each entry followed by 0x00)
/// ))
/// ```
///
/// `ref` is the registry key and is NOT folded into the digest. Pass
/// `compatible_with` already sorted, or use [`contract_digest_of`].
pub fn contract_digest(
    media_type: &str,
    schema: &str,
    version: &str,
    compatible_with: &[String],
) -> String {
    let mut h = Sha256::new();
    h.update(media_type.as_bytes());
    h.update([SEP]);
    h.update(schema.as_bytes());
    h.update([SEP]);
    h.update(version.as_bytes());
    h.update([SEP]);
    for c in compatible_with {
        h.update(c.as_bytes());
        h.update([SEP]);
    }
    format!("sha256:{}", hex(&h.finalize()))
}

/// Compute the content digest for a [`Contract`], sorting `compatible_with`
/// ascending as raw UTF-8 before hashing.
pub fn contract_digest_of(c: &Contract) -> String {
    let mut compat = c.compatible_with.clone();
    compat.sort();
    contract_digest(&c.media_type, &c.schema, &c.version, &compat)
}

/// A name-only stub (empty content fields) created by [`MemoryKernel::register`].
pub fn is_contract_placeholder(c: &Contract) -> bool {
    c.media_type.is_empty()
        && c.schema.is_empty()
        && c.version.is_empty()
        && c.compatible_with.is_empty()
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
// The MemoryKernel — the in-memory implementation of the ABI. Methods mirror
// `service Kernel` in the proto.
// ─────────────────────────────────────────────────────────────────────────

/// One live subscription: the topics it wants, and the bounded channel we push
/// to. The channel is `sync_channel(SUBSCRIBER_BUFFER)` so delivery is
/// non-blocking but memory-bounded; see [`SUBSCRIBER_BUFFER`].
struct Sub {
    topics: Vec<String>,
    tx: SyncSender<Event>,
}

/// A registered module plus its current lifecycle state.
struct ModuleSlot {
    manifest: ModuleManifest,
    lifecycle: Lifecycle,
}

#[derive(Clone)]
struct RunSlot {
    run: Run,
    /// In-flight work, keyed by `WorkItem.id`.
    claimed: HashMap<String, WorkItem>,
    /// Latest committed derivation per assembly node (for downstream resolve).
    latest: HashMap<String, Derivation>,
    /// Work-unit keys already claimed or committed (at most once each).
    done_units: HashSet<String>,
    /// Delivery generation per named run input (bumped by InjectInput).
    input_epochs: HashMap<String, u64>,
    /// Commit count per node (source generation for ALWAYS fingerprints).
    node_commits: HashMap<String, u64>,
}

#[derive(Clone)]
struct BlobSlot {
    data: Vec<u8>,
    r#ref: BlobRef,
}

/// Successful responses cached for non-empty `RequestContext.request_key`.
#[derive(Clone)]
enum IdempotentResult {
    ArtifactRef(ArtifactRef),
    Run(Box<Run>),
}

/// Everything the kernel owns, behind one lock. Small on purpose.
#[derive(Default)]
struct State {
    modules: Vec<ModuleSlot>,
    capabilities: Vec<Capability>,
    contracts: HashMap<String, Contract>,
    artifacts: HashMap<String, Artifact>,
    /// Keyed by `(namespace, digest)`.
    blobs: HashMap<(String, String), BlobSlot>,
    subs: Vec<Sub>,
    ledger: Vec<LedgerEntry>,
    runs: HashMap<String, RunSlot>,
    derivations: Vec<Derivation>,
    event_seq: u64,
    /// `(operation, caller, request_key)` → first successful response.
    idempotency: HashMap<String, IdempotentResult>,
}

/// The in-memory realisation of [`KernelApi`]. `Send + Sync`; share it behind
/// an `Arc` across module threads. Every meaningful action lands one
/// append-only ledger entry. Durability lives in Modules (or other
/// [`KernelApi`] implementations), not in this type.
pub struct MemoryKernel {
    state: Mutex<State>,
}

impl Default for MemoryKernel {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryKernel {
    pub fn new() -> Self {
        MemoryKernel {
            state: Mutex::new(State::default()),
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
    /// capabilities, and (implicitly) name-only placeholders for the contracts
    /// named on those capabilities' **ports**. The module lands in `REGISTERED`;
    /// advance it with [`MemoryKernel::transition`]. Placeholders may be filled
    /// once via [`MemoryKernel::put_contract`].
    pub fn register(&self, manifest: ModuleManifest) -> RegisterAck {
        self.register_ctx(manifest, &RequestContext::default())
    }

    /// Like [`MemoryKernel::register`] with an explicit context. Deadline is
    /// not enforceable on this infallible RPC shape; use Result-returning
    /// methods (`put_artifact_ctx`, `start_run_ctx`, `commit_ctx`,
    /// `transition_req`) when deadlines matter.
    pub fn register_ctx(&self, manifest: ModuleManifest, _ctx: &RequestContext) -> RegisterAck {
        let mut state = self.state.lock().unwrap();
        for cap in &manifest.provides {
            state.capabilities.push(cap.clone());
            for port in cap.inputs.iter().chain(cap.outputs.iter()) {
                Self::ensure_contract_placeholder(&mut state, &port.contract);
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

    fn ensure_contract_placeholder(state: &mut State, r#ref: &str) {
        if r#ref.is_empty() {
            return;
        }
        state.contracts.entry(r#ref.to_string()).or_insert_with(|| Contract {
            r#ref: r#ref.to_string(),
            digest: contract_digest("", "", "", &[]),
            ..Default::default()
        });
    }

    /// `rpc Transition(TransitionRequest) -> TransitionAck`. Advances a module
    /// along `REGISTERED → LOADED → ACTIVE → DEACTIVATED`. Only a single
    /// forward step is applied; anything else is a no-op returning the current
    /// state. Returns `NotFound` if the module was never registered.
    pub fn transition(&self, module: &str, to: Lifecycle) -> Result<Lifecycle> {
        let ack = self.transition_req(
            TransitionRequest {
                module: module.to_string(),
                to: to as i32,
            },
            &RequestContext::default(),
        )?;
        Ok(Lifecycle::try_from(ack.state).unwrap_or(Lifecycle::Unspecified))
    }

    /// Full ABI form of [`MemoryKernel::transition`].
    pub fn transition_req(
        &self,
        req: TransitionRequest,
        ctx: &RequestContext,
    ) -> Result<TransitionAck> {
        check_deadline(ctx)?;
        let mut state = self.state.lock().unwrap();
        let slot = state
            .modules
            .iter_mut()
            .find(|m| m.manifest.name == req.module)
            .ok_or_else(|| KernelError::NotFound(req.module.clone()))?;
        let to = Lifecycle::try_from(req.to).unwrap_or(Lifecycle::Unspecified);
        if to as i32 == slot.lifecycle as i32 + 1 {
            slot.lifecycle = to;
            let kind = format!("module.{}", lifecycle_verb(to));
            Self::append_locked(&mut state, &kind, &req.module, Vec::new());
        }
        let now = state
            .modules
            .iter()
            .find(|m| m.manifest.name == req.module)
            .map(|m| m.lifecycle)
            .unwrap();
        Ok(TransitionAck { state: now as i32 })
    }

    // ── 2. ARTIFACT ────────────────────────────────────────────────────────

    /// `rpc PutArtifact(Artifact) -> ArtifactRef`. Content-addresses the typed
    /// value, stores it **immutably** (first write wins), and returns its ref.
    ///
    /// Inline: set `body`, leave `object` unset. External: `put_blob` first,
    /// then set `object` with empty `body`. The blob must already exist and
    /// match. Exactly one of body or object may carry the value.
    pub fn put_artifact(&self, artifact: Artifact) -> Result<ArtifactRef> {
        self.put_artifact_ctx(artifact, &RequestContext::default())
    }

    /// Like [`MemoryKernel::put_artifact`], honouring deadline and idempotency
    /// (`request_key`) from [`RequestContext`].
    pub fn put_artifact_ctx(
        &self,
        mut artifact: Artifact,
        ctx: &RequestContext,
    ) -> Result<ArtifactRef> {
        check_deadline(ctx)?;
        validate_artifact_content(&artifact)?;
        let mut state = self.state.lock().unwrap();
        if let Some(IdempotentResult::ArtifactRef(r)) = idempotent_get(&state, "put_artifact", ctx) {
            return Ok(r);
        }
        if has_external_object(&artifact) {
            verify_object_ref_locked(&state, artifact.object.as_ref().unwrap())?;
        }
        let id = artifact_id_of(&artifact);
        artifact.id = id.clone();
        // Immutability: only the first writer of an id sets the stored bytes.
        if !state.artifacts.contains_key(&id) {
            // Clear large inline body in the ledger; keep ObjectRef (small,
            // part of value identity) so external artifacts reconstruct.
            let body = std::mem::take(&mut artifact.body);
            let detail = artifact.encode_to_vec();
            artifact.body = body;
            state.artifacts.insert(id.clone(), artifact);
            Self::append_locked(&mut state, "artifact.put", &id, detail);
        }
        let r = ArtifactRef { id };
        idempotent_put(
            &mut state,
            "put_artifact",
            ctx,
            IdempotentResult::ArtifactRef(r.clone()),
        );
        Ok(r)
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

    /// `rpc PutBlob(PutBlobRequest) -> BlobRef`. Content-addresses raw bytes
    /// under `(namespace, digest)`. First write wins.
    pub fn put_blob(&self, req: PutBlobRequest) -> BlobRef {
        let digest = blob_id(&req.data);
        let r#ref = BlobRef {
            digest: digest.clone(),
            byte_count: req.data.len() as u64,
            namespace: req.namespace.clone(),
        };
        let key = (req.namespace, digest.clone());
        let mut state = self.state.lock().unwrap();
        // First write wins; use entry to avoid double-lookup (clippy map_entry).
        if let std::collections::hash_map::Entry::Vacant(e) = state.blobs.entry(key) {
            let detail = r#ref.encode_to_vec();
            e.insert(BlobSlot {
                data: req.data,
                r#ref: r#ref.clone(),
            });
            Self::append_locked(&mut state, "blob.put", &digest, detail);
        }
        r#ref
    }

    /// `rpc GetBlob(GetBlobRequest) -> BlobData`. Returns verified blob bytes.
    pub fn get_blob(&self, req: GetBlobRequest) -> Result<BlobData> {
        let state = self.state.lock().unwrap();
        let slot = state
            .blobs
            .get(&(req.namespace.clone(), req.digest.clone()))
            .ok_or_else(|| KernelError::NotFound(format!("blob {}", req.digest)))?;
        if blob_id(&slot.data) != slot.r#ref.digest
            || slot.data.len() as u64 != slot.r#ref.byte_count
        {
            return Err(KernelError::BlobIntegrity(
                "stored blob corrupted".into(),
            ));
        }
        Ok(BlobData {
            digest: slot.r#ref.digest.clone(),
            byte_count: slot.r#ref.byte_count,
            namespace: slot.r#ref.namespace.clone(),
            data: slot.data.clone(),
        })
    }

    /// `rpc HasBlob(HasBlobRequest) -> HasBlobResponse`.
    pub fn has_blob(&self, req: HasBlobRequest) -> HasBlobResponse {
        let state = self.state.lock().unwrap();
        if let Some(slot) = state.blobs.get(&(req.namespace, req.digest)) {
            HasBlobResponse {
                exists: true,
                byte_count: slot.r#ref.byte_count,
            }
        } else {
            HasBlobResponse {
                exists: false,
                byte_count: 0,
            }
        }
    }

    /// Put the blob then an external artifact referencing it (production path).
    pub fn put_artifact_with_blob(
        &self,
        r#type: &str,
        namespace: &str,
        data: &[u8],
        produced_by: &str,
    ) -> Result<(ArtifactRef, BlobRef)> {
        let blob = self.put_blob(PutBlobRequest {
            namespace: namespace.into(),
            data: data.to_vec(),
        });
        let artifact = self.put_artifact(Artifact {
            r#type: r#type.into(),
            produced_by: produced_by.into(),
            object: Some(ObjectRef {
                digest: blob.digest.clone(),
                byte_count: blob.byte_count,
                namespace: blob.namespace.clone(),
            }),
            ..Default::default()
        })?;
        Ok((artifact, blob))
    }

    // ── 3. CONTRACT ────────────────────────────────────────────────────────

    /// `rpc PutContract(Contract) -> Contract`. Registers a contract immutably
    /// under its ref. Returns the stored contract (digest assigned). Identical
    /// re-puts are no-ops; different content under the same ref is
    /// [`KernelError::Conflict`]. A name-only placeholder from
    /// [`MemoryKernel::register`] may be filled once.
    pub fn put_contract(&self, mut contract: Contract) -> Result<Contract> {
        if contract.r#ref.is_empty() {
            return Err(KernelError::Invalid("contract ref is required".into()));
        }
        contract.compatible_with.sort();
        let digest = contract_digest(
            &contract.media_type,
            &contract.schema,
            &contract.version,
            &contract.compatible_with,
        );
        if !contract.digest.is_empty() && contract.digest != digest {
            return Err(KernelError::Invalid("contract digest mismatch".into()));
        }
        contract.digest = digest;

        let mut state = self.state.lock().unwrap();
        if let Some(existing) = state.contracts.get(&contract.r#ref) {
            if existing.digest == contract.digest {
                return Ok(existing.clone());
            }
            if is_contract_placeholder(existing) && !is_contract_placeholder(&contract) {
                let detail = contract.encode_to_vec();
                let r#ref = contract.r#ref.clone();
                state.contracts.insert(r#ref.clone(), contract.clone());
                Self::append_locked(&mut state, "contract.registered", &r#ref, detail);
                return Ok(contract);
            }
            return Err(KernelError::Conflict(format!(
                "contract {} already registered with different content",
                contract.r#ref
            )));
        }
        let detail = contract.encode_to_vec();
        let r#ref = contract.r#ref.clone();
        state.contracts.insert(r#ref.clone(), contract.clone());
        Self::append_locked(&mut state, "contract.registered", &r#ref, detail);
        Ok(contract)
    }

    // ── 4. EVENT ───────────────────────────────────────────────────────────

    /// `rpc Subscribe(Subscription) -> stream Event`. In-process the "stream"
    /// is an mpsc [`Receiver`]; events arrive in kernel `seq` order. A module
    /// only ever receives events on topics it named here.
    pub fn subscribe(&self, sub: Subscription) -> Receiver<Event> {
        let (tx, rx) = sync_channel(SUBSCRIBER_BUFFER);
        let mut state = self.state.lock().unwrap();
        state.subs.push(Sub {
            topics: sub.topics,
            tx,
        });
        rx
    }

    /// `rpc Publish(Event) -> PublishAck`. Assigns a monotonic `seq` (the total
    /// order), delivers to exactly the subscribers of `event.topic`, and never
    /// to anyone else. Returns the assigned seq. Artifact refs on the event are
    /// the data plane.
    pub fn publish(&self, mut event: Event) -> PublishAck {
        let mut state = self.state.lock().unwrap();
        state.event_seq += 1;
        let seq = state.event_seq;
        event.seq = seq;

        // Deliver in-order to matching subs. A bounded, non-blocking `try_send`:
        // a subscriber whose buffer is full (fallen SUBSCRIBER_BUFFER behind) or
        // whose receiver is gone is shed here, so one slow consumer can never
        // grow the kernel's memory without bound. Dropped notifications remain
        // reconstructable from the ledger — the bus is only notification.
        state.subs.retain(|sub| {
            if sub.topics.iter().any(|t| t == &event.topic) {
                sub.tx.try_send(event.clone()).is_ok()
            } else {
                true
            }
        });
        let detail = event.encode_to_vec();
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

    // ── 6. REGISTRY ────────────────────────────────────────────────────────

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

    // ── 7. RUN / CONVERGENCE ─────────────────────────────────────────────

    /// Validate and start one finite feed-forward assembly. The graph, module
    /// versions, bindings, input artifacts, and terminal output are frozen into
    /// the returned Run and committed to the ledger.
    pub fn start_run(&self, req: RunRequest) -> Result<Run> {
        self.start_run_ctx(req, &RequestContext::default())
    }

    /// Like [`MemoryKernel::start_run`], honouring deadline and idempotency.
    pub fn start_run_ctx(&self, req: RunRequest, ctx: &RequestContext) -> Result<Run> {
        check_deadline(ctx)?;
        let mut state = self.state.lock().unwrap();
        if let Some(IdempotentResult::Run(r)) = idempotent_get(&state, "start_run", ctx) {
            return Ok(*r);
        }
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
        let assembly = materialize_assembly(&assembly, &req.include_nodes)?;
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
        let mut input_epochs = HashMap::new();
        for input in &req.inputs {
            input_epochs.insert(input.name.clone(), 0);
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
            policy: req.policy,
        };
        state.runs.insert(
            run.id.clone(),
            RunSlot {
                run: run.clone(),
                claimed: HashMap::new(),
                latest: HashMap::new(),
                done_units: HashSet::new(),
                input_epochs,
                node_commits: HashMap::new(),
            },
        );
        Self::append_locked(&mut state, "run.started", &run.id, run.encode_to_vec());
        idempotent_put(
            &mut state,
            "start_run",
            ctx,
            IdempotentResult::Run(Box::new(run.clone())),
        );
        Ok(run)
    }

    /// Admit or replace a named run input while RUNNING. Bumps delivery
    /// generation so `FIRING_ALWAYS` nodes can re-fire.
    pub fn inject_input(&self, req: InjectInputRequest) -> Result<Run> {
        self.inject_input_ctx(req, &RequestContext::default())
    }

    /// Like [`MemoryKernel::inject_input`], honouring deadline.
    pub fn inject_input_ctx(&self, req: InjectInputRequest, ctx: &RequestContext) -> Result<Run> {
        check_deadline(ctx)?;
        let mut state = self.state.lock().unwrap();
        let input = req
            .input
            .clone()
            .ok_or_else(|| KernelError::Invalid("input is required".into()))?;
        if input.name.is_empty() {
            return Err(KernelError::Invalid("input name is required".into()));
        }
        let art_id = input
            .artifact
            .as_ref()
            .ok_or_else(|| KernelError::Invalid(format!("input {} has no artifact", input.name)))?
            .id
            .clone();
        if !state.artifacts.contains_key(&art_id) {
            return Err(KernelError::NotFound(art_id));
        }
        let slot = state
            .runs
            .get_mut(&req.run_id)
            .ok_or_else(|| KernelError::NotFound(req.run_id.clone()))?;
        if slot.run.state() != RunState::Running {
            return Err(KernelError::RunClosed(slot.run.state()));
        }
        let used = slot
            .run
            .assembly
            .as_ref()
            .map(|a| a.bindings.iter().any(|b| b.input == input.name))
            .unwrap_or(false);
        if !used {
            return Err(KernelError::Invalid(format!(
                "run input {} is not bound in the assembly",
                input.name
            )));
        }
        if let Some(existing) = slot.run.inputs.iter_mut().find(|i| i.name == input.name) {
            *existing = input.clone();
        } else {
            slot.run.inputs.push(input.clone());
        }
        let epoch = slot.input_epochs.entry(input.name.clone()).or_insert(0);
        *epoch = epoch.saturating_add(1);
        let run = slot.run.clone();
        Self::append_locked(
            &mut state,
            "run.input_injected",
            &run.id,
            input.encode_to_vec(),
        );
        Ok(run)
    }

    /// Atomically claim the first ready work unit for a module. An empty
    /// WorkItem means that this module has no ready unit. Under default
    /// closure, if the whole graph has neither ready nor in-flight work, the
    /// run closes as STALLED.
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

        let assembly = slot.run.assembly.as_ref().unwrap().clone();
        let mut selected = None;
        let mut any_ready = false;
        for node in &assembly.nodes {
            let Some(inputs) = resolve_inputs(slot, node) else {
                continue;
            };
            let Ok(cap) = capability_for_node(&state, node) else {
                continue;
            };
            let firing = effective_firing(slot, node, cap);
            let Some(unit_key) = work_unit_key(slot, node, cap, firing, &inputs) else {
                continue;
            };
            if slot.done_units.contains(&unit_key) {
                continue;
            }
            any_ready = true;
            if selected.is_none() && node.module == req.module {
                selected = Some((node.clone(), inputs, unit_key));
            }
        }

        if let Some((node, inputs, unit_key)) = selected {
            // ONCE keeps the v1.0 work id shape so known-answer ledger fixtures
            // stay stable; multi-fire modes embed the unit key.
            let work_id = if unit_key.starts_with("once:") {
                format!("work:{}/{}", req.run_id, node.id)
            } else {
                format!("work:{}/{}", req.run_id, unit_key)
            };
            let work = WorkItem {
                id: work_id,
                run_id: req.run_id.clone(),
                node_id: node.id.clone(),
                module: node.module,
                module_version: node.module_version,
                capability: node.capability,
                inputs,
            };
            let slot = state.runs.get_mut(&req.run_id).unwrap();
            slot.done_units.insert(unit_key);
            slot.claimed.insert(work.id.clone(), work.clone());
            Self::append_locked(&mut state, "work.claimed", &work.id, work.encode_to_vec());
            return Ok(work);
        }

        let open = is_open_closure(slot);
        if !any_ready && slot.claimed.is_empty() && !open {
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
    /// work, and closes the run when its declared terminal output appears
    /// (unless `CLOSURE_OPEN`).
    pub fn commit(&self, submitted: Derivation) -> Result<Run> {
        self.commit_ctx(submitted, &RequestContext::default())
    }

    /// Like [`MemoryKernel::commit`], honouring deadline and idempotency.
    pub fn commit_ctx(&self, submitted: Derivation, ctx: &RequestContext) -> Result<Run> {
        check_deadline(ctx)?;
        let mut state = self.state.lock().unwrap();
        if let Some(IdempotentResult::Run(r)) = idempotent_get(&state, "commit", ctx) {
            return Ok(*r);
        }
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
            .get(&submitted.work_id)
            .cloned()
            .ok_or_else(|| KernelError::Conflict("work was not claimed".into()))?;
        if !submitted.node_id.is_empty() && submitted.node_id != work.node_id {
            return Err(KernelError::Invalid(
                "node_id does not match the claim".into(),
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
            slot.claimed.remove(&work.id);
            slot.latest
                .insert(work.node_id.clone(), derivation.clone());
            *slot.node_commits.entry(work.node_id.clone()).or_insert(0) += 1;
            slot.run.steps += 1;
            let terminal = slot
                .run
                .assembly
                .as_ref()
                .unwrap()
                .terminal
                .as_ref()
                .unwrap()
                .clone();
            let open = is_open_closure(slot);
            let mut closed = false;
            if terminal.node == work.node_id {
                if let Some(answer) = derivation
                    .outputs
                    .iter()
                    .find(|o| o.name == terminal.port)
                    .and_then(|o| o.artifact.clone())
                {
                    slot.run.answer = Some(answer);
                    if !open {
                        slot.run.state = RunState::Completed as i32;
                        closed = true;
                    }
                }
            }
            if !closed && slot.run.steps >= slot.run.max_steps {
                slot.run.state = RunState::Failed as i32;
                slot.run.reason = "max_steps exhausted before the terminal output".into();
                closed = true;
            }
            let should_check_stall = !closed && !open && slot.claimed.is_empty();
            let snap = if should_check_stall {
                Some(slot.clone())
            } else {
                None
            };
            let run_id = work.run_id.clone();
            let mut run_now = slot.run.clone();
            if let Some(snap) = snap {
                if !assembly_any_ready(&state, &snap) {
                    run_now.state = RunState::Stalled as i32;
                    run_now.reason = "no node is ready and no work is in flight".into();
                    let slot = state.runs.get_mut(&run_id).unwrap();
                    slot.run.state = run_now.state;
                    slot.run.reason = run_now.reason.clone();
                }
            }
            run_now
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
        idempotent_put(
            &mut state,
            "commit",
            ctx,
            IdempotentResult::Run(Box::new(run_now.clone())),
        );
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

// ─────────────────────────────────────────────────────────────────────────
// KernelApi — the portable ABI (unary RPCs of `service Kernel`).
// Streaming `Subscribe` stays inherent-only on [`MemoryKernel`].
// ─────────────────────────────────────────────────────────────────────────

/// The Kernel ABI: one method per unary `service Kernel` RPC, each taking a
/// [`RequestContext`] as call metadata. Context is **not** folded into ledger
/// detail (that would break the cross-SDK known-answer chain hashes); it rides
/// beside the call — gRPC headers on the wire, this parameter in-process.
///
/// Enforced context semantics:
/// - `deadline_unix_ms > 0` and wall clock past that instant →
///   [`KernelError::FailedPrecondition`]
/// - non-empty `request_key` on PutArtifact / StartRun / Commit: first success
///   for `(caller, request_key, operation)` is replayed without re-applying
///   side effects
///
/// [`MemoryKernel`] implements this trait. Other backends (durable store,
/// remote) implement the same trait.
pub trait KernelApi {
    fn register(&self, manifest: ModuleManifest, ctx: &RequestContext) -> RegisterAck;
    fn transition(&self, req: TransitionRequest, ctx: &RequestContext) -> Result<TransitionAck>;
    fn put_artifact(&self, artifact: Artifact, ctx: &RequestContext) -> Result<ArtifactRef>;
    fn get_artifact(&self, r#ref: &ArtifactRef, ctx: &RequestContext) -> Result<Artifact>;
    fn put_blob(&self, req: PutBlobRequest, ctx: &RequestContext) -> BlobRef;
    fn get_blob(&self, req: GetBlobRequest, ctx: &RequestContext) -> Result<BlobData>;
    fn has_blob(&self, req: HasBlobRequest, ctx: &RequestContext) -> HasBlobResponse;
    fn put_contract(&self, contract: Contract, ctx: &RequestContext) -> Result<Contract>;
    fn publish(&self, event: Event, ctx: &RequestContext) -> PublishAck;
    fn append(&self, req: AppendRequest, ctx: &RequestContext) -> LedgerEntry;
    fn snapshot(&self, req: SnapshotRequest, ctx: &RequestContext) -> RegistrySnapshot;
    fn start_run(&self, req: RunRequest, ctx: &RequestContext) -> Result<Run>;
    fn inject_input(&self, req: InjectInputRequest, ctx: &RequestContext) -> Result<Run>;
    fn claim_ready(&self, req: ClaimRequest, ctx: &RequestContext) -> Result<WorkItem>;
    fn commit(&self, submitted: Derivation, ctx: &RequestContext) -> Result<Run>;
    fn get_run(&self, r: &RunRef, ctx: &RequestContext) -> Result<Run>;
    fn cancel_run(&self, r: &RunRef, ctx: &RequestContext) -> Result<Run>;
    fn list_derivations(&self, r: &RunRef, ctx: &RequestContext) -> Result<DerivationList>;
}

impl KernelApi for MemoryKernel {
    fn register(&self, manifest: ModuleManifest, ctx: &RequestContext) -> RegisterAck {
        MemoryKernel::register_ctx(self, manifest, ctx)
    }
    fn transition(&self, req: TransitionRequest, ctx: &RequestContext) -> Result<TransitionAck> {
        MemoryKernel::transition_req(self, req, ctx)
    }
    fn put_artifact(&self, artifact: Artifact, ctx: &RequestContext) -> Result<ArtifactRef> {
        MemoryKernel::put_artifact_ctx(self, artifact, ctx)
    }
    fn get_artifact(&self, r#ref: &ArtifactRef, ctx: &RequestContext) -> Result<Artifact> {
        check_deadline(ctx)?;
        MemoryKernel::get_artifact(self, r#ref)
    }
    fn put_blob(&self, req: PutBlobRequest, _ctx: &RequestContext) -> BlobRef {
        MemoryKernel::put_blob(self, req)
    }
    fn get_blob(&self, req: GetBlobRequest, ctx: &RequestContext) -> Result<BlobData> {
        check_deadline(ctx)?;
        MemoryKernel::get_blob(self, req)
    }
    fn has_blob(&self, req: HasBlobRequest, _ctx: &RequestContext) -> HasBlobResponse {
        MemoryKernel::has_blob(self, req)
    }
    fn put_contract(&self, contract: Contract, ctx: &RequestContext) -> Result<Contract> {
        check_deadline(ctx)?;
        MemoryKernel::put_contract(self, contract)
    }
    fn publish(&self, event: Event, _ctx: &RequestContext) -> PublishAck {
        MemoryKernel::publish(self, event)
    }
    fn append(&self, req: AppendRequest, _ctx: &RequestContext) -> LedgerEntry {
        MemoryKernel::append(self, req)
    }
    fn snapshot(&self, _req: SnapshotRequest, _ctx: &RequestContext) -> RegistrySnapshot {
        MemoryKernel::snapshot(self)
    }
    fn start_run(&self, req: RunRequest, ctx: &RequestContext) -> Result<Run> {
        MemoryKernel::start_run_ctx(self, req, ctx)
    }
    fn inject_input(&self, req: InjectInputRequest, ctx: &RequestContext) -> Result<Run> {
        MemoryKernel::inject_input_ctx(self, req, ctx)
    }
    fn claim_ready(&self, req: ClaimRequest, ctx: &RequestContext) -> Result<WorkItem> {
        check_deadline(ctx)?;
        MemoryKernel::claim_ready(self, req)
    }
    fn commit(&self, submitted: Derivation, ctx: &RequestContext) -> Result<Run> {
        MemoryKernel::commit_ctx(self, submitted, ctx)
    }
    fn get_run(&self, r: &RunRef, ctx: &RequestContext) -> Result<Run> {
        check_deadline(ctx)?;
        MemoryKernel::get_run(self, r)
    }
    fn cancel_run(&self, r: &RunRef, ctx: &RequestContext) -> Result<Run> {
        check_deadline(ctx)?;
        MemoryKernel::cancel_run(self, r)
    }
    fn list_derivations(&self, r: &RunRef, ctx: &RequestContext) -> Result<DerivationList> {
        check_deadline(ctx)?;
        MemoryKernel::list_derivations(self, r)
    }
}

/// Reject calls whose absolute deadline has already passed.
fn check_deadline(ctx: &RequestContext) -> Result<()> {
    if ctx.deadline_unix_ms <= 0 {
        return Ok(());
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if now_ms > ctx.deadline_unix_ms {
        return Err(KernelError::FailedPrecondition("deadline exceeded".into()));
    }
    Ok(())
}

fn idempotency_key(op: &str, ctx: &RequestContext) -> Option<String> {
    if ctx.request_key.is_empty() {
        return None;
    }
    Some(format!("{op}\0{}\0{}", ctx.caller, ctx.request_key))
}

fn idempotent_get(state: &State, op: &str, ctx: &RequestContext) -> Option<IdempotentResult> {
    let key = idempotency_key(op, ctx)?;
    state.idempotency.get(&key).cloned()
}

fn idempotent_put(state: &mut State, op: &str, ctx: &RequestContext, value: IdempotentResult) {
    if let Some(key) = idempotency_key(op, ctx) {
        state.idempotency.entry(key).or_insert(value);
    }
}

fn is_sha256_digest(d: &str) -> bool {
    const PREFIX: &str = "sha256:";
    if d.len() != PREFIX.len() + 64 || !d.starts_with(PREFIX) {
        return false;
    }
    d[PREFIX.len()..]
        .bytes()
        .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn validate_artifact_content(artifact: &Artifact) -> Result<()> {
    if artifact.r#type.is_empty() {
        return Err(KernelError::Invalid("artifact type is required".into()));
    }
    let has_obj = has_external_object(artifact);
    let has_body = !artifact.body.is_empty();
    if has_obj && has_body {
        return Err(KernelError::Invalid(
            "artifact must not set both body and object".into(),
        ));
    }
    if let Some(obj) = artifact.object.as_ref() {
        if obj.digest.is_empty() && (obj.byte_count != 0 || !obj.namespace.is_empty()) {
            return Err(KernelError::Invalid(
                "object.digest is required when object is set".into(),
            ));
        }
        if has_obj && !is_sha256_digest(&obj.digest) {
            return Err(KernelError::Invalid(
                "object.digest must be sha256:<hex>".into(),
            ));
        }
    }
    Ok(())
}

fn verify_object_ref_locked(state: &State, object: &ObjectRef) -> Result<()> {
    let key = (object.namespace.clone(), object.digest.clone());
    let slot = state.blobs.get(&key).ok_or_else(|| {
        KernelError::NotFound(format!(
            "blob {} (namespace {:?})",
            object.digest, object.namespace
        ))
    })?;
    if slot.r#ref.byte_count != object.byte_count {
        return Err(KernelError::BlobIntegrity(format!(
            "object.byte_count {} != stored {}",
            object.byte_count, slot.r#ref.byte_count
        )));
    }
    if blob_id(&slot.data) != object.digest || slot.data.len() as u64 != object.byte_count {
        return Err(KernelError::BlobIntegrity(
            "blob does not match object ref".into(),
        ));
    }
    Ok(())
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
            slot.latest
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

fn materialize_assembly(assembly: &Assembly, include: &[String]) -> Result<Assembly> {
    if include.is_empty() {
        return Ok(assembly.clone());
    }
    let want: HashSet<&str> = include.iter().map(String::as_str).collect();
    if want.len() != include.len() {
        return Err(KernelError::Invalid(
            "include_nodes contains duplicates".into(),
        ));
    }
    let known: HashSet<&str> = assembly.nodes.iter().map(|n| n.id.as_str()).collect();
    for id in &want {
        if !known.contains(id) {
            return Err(KernelError::Invalid(format!(
                "include_nodes references unknown node {id}"
            )));
        }
    }
    let terminal = assembly
        .terminal
        .as_ref()
        .ok_or_else(|| KernelError::Invalid("assembly terminal is required".into()))?;
    if !want.contains(terminal.node.as_str()) {
        return Err(KernelError::Invalid(
            "include_nodes must retain the terminal node".into(),
        ));
    }
    let nodes: Vec<_> = assembly
        .nodes
        .iter()
        .filter(|n| want.contains(n.id.as_str()))
        .cloned()
        .collect();
    let bindings: Vec<_> = assembly
        .bindings
        .iter()
        .filter(|b| {
            want.contains(b.to_node.as_str())
                && (!b.input.is_empty() || want.contains(b.from_node.as_str()))
        })
        .cloned()
        .collect();
    Ok(Assembly {
        id: assembly.id.clone(),
        nodes,
        bindings,
        terminal: assembly.terminal.clone(),
    })
}

fn is_open_closure(slot: &RunSlot) -> bool {
    slot.run
        .policy
        .as_ref()
        .map(|p| p.closure() == Closure::Open)
        .unwrap_or(false)
}

fn effective_firing(slot: &RunSlot, node: &AssemblyNode, cap: &Capability) -> Firing {
    if let Some(policy) = slot.run.policy.as_ref() {
        if let Some(&f) = policy.by_node.get(&node.id) {
            if f != Firing::Unspecified as i32 {
                return Firing::try_from(f).unwrap_or(Firing::Once);
            }
        }
    }
    if cap.firing() != Firing::Unspecified {
        return cap.firing();
    }
    if let Some(policy) = slot.run.policy.as_ref() {
        if policy.default() != Firing::Unspecified {
            return policy.default();
        }
    }
    Firing::Once
}

fn work_unit_key(
    slot: &RunSlot,
    node: &AssemblyNode,
    cap: &Capability,
    firing: Firing,
    inputs: &[NamedArtifact],
) -> Option<String> {
    match firing {
        Firing::Always => {
            let fp = delivery_fingerprint(slot, node, inputs)?;
            Some(format!("always:{}:{}", node.id, fp))
        }
        Firing::OncePerKey => {
            let key = input_key(cap, inputs);
            Some(format!("key:{}:{}", node.id, key))
        }
        _ => Some(format!("once:{}", node.id)),
    }
}

fn input_key(cap: &Capability, inputs: &[NamedArtifact]) -> String {
    let marked: Vec<&str> = cap
        .inputs
        .iter()
        .filter(|p| p.key)
        .map(|p| p.name.as_str())
        .collect();
    let mut pairs: Vec<(&str, &str)> = inputs
        .iter()
        .filter_map(|i| {
            let id = i.artifact.as_ref()?.id.as_str();
            if marked.is_empty() || marked.iter().any(|n| *n == i.name) {
                Some((i.name.as_str(), id))
            } else {
                None
            }
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = Sha256::new();
    for (name, id) in pairs {
        h.update(name.as_bytes());
        h.update([SEP]);
        h.update(id.as_bytes());
        h.update([SEP]);
    }
    hex(&h.finalize())
}

fn delivery_fingerprint(
    slot: &RunSlot,
    node: &AssemblyNode,
    inputs: &[NamedArtifact],
) -> Option<String> {
    let assembly = slot.run.assembly.as_ref()?;
    let mut h = Sha256::new();
    let mut rows: Vec<(String, String, u64)> = Vec::new();
    for b in assembly.bindings.iter().filter(|b| b.to_node == node.id) {
        let art = inputs.iter().find(|i| i.name == b.to_port)?;
        let id = art.artifact.as_ref()?.id.clone();
        let epoch = if !b.input.is_empty() {
            slot.input_epochs.get(&b.input).copied().unwrap_or(0)
        } else {
            slot.node_commits.get(&b.from_node).copied().unwrap_or(0)
        };
        rows.push((b.to_port.clone(), id, epoch));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (port, id, epoch) in rows {
        h.update(port.as_bytes());
        h.update([SEP]);
        h.update(id.as_bytes());
        h.update([SEP]);
        h.update(epoch.to_be_bytes());
        h.update([SEP]);
    }
    Some(hex(&h.finalize()))
}

fn assembly_any_ready(state: &State, slot: &RunSlot) -> bool {
    let Some(assembly) = slot.run.assembly.as_ref() else {
        return false;
    };
    for node in &assembly.nodes {
        let Some(inputs) = resolve_inputs(slot, node) else {
            continue;
        };
        let Ok(cap) = capability_for_node(state, node) else {
            continue;
        };
        let firing = effective_firing(slot, node, cap);
        let Some(unit_key) = work_unit_key(slot, node, cap, firing, &inputs) else {
            continue;
        };
        if !slot.done_units.contains(&unit_key) {
            return true;
        }
    }
    false
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
