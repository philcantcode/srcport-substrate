# srcport-substrate — Specification v1.0.0

One canonical contract for every future project. Domain-neutral. Small enough
to hold in your head. This document and [`substrate.proto`](contracts/proto/srcport/substrate/v1/substrate.proto)
are the **only** things a human owns and reads; SDKs and backends conform to
them, and coding agents fill the leaves behind them.

> **The one rule.** One canonical contract, many conforming implementations.
> A production consumer will often need its own durable backend. The thing that
> must not be re-derived is the **contract and invariants** — not necessarily
> the storage implementation. If the contract lacks a capability, widen *this*
> contract (by adding), tag a new version, and every project picks it up.
> Re-deriving the contract is the bug.

---

## What this is (and is not)

It **is** the seven primitives below plus one ABI (the `Kernel` service). That is
the entire surface. If it doesn't fit on these two pages, the design is wrong.

It **is not** any domain. There is deliberately no notion of a target, trial,
finding, entity, component, terrain tile, or content pack in here. Every one of
those is a **Module** built on top. VR is a set of modules. A game is a set of
modules. A content factory is a set of modules. They share *this*, and nothing
else — that shared, versioned contract is the thing that finally becomes boring,
trusted, and legible.

### Two durability homes (kernel vs domain)

This is a central design decision. The SDKs ship an in-memory realisation of the
ABI (`MemoryKernel` in Rust/Go/Python) so the contract is executable and
conformance-testable without a database. That type is **one** implementation of
`KernelApi`, not *the* authority.

| Concern | Owner |
|---------|--------|
| **Kernel state** — registry, ledger, runs, blob index | A **`KernelApi` backend** (`MemoryKernel` today; Postgres/files/… later) |
| **Domain state** — findings, game world, content packs, evidence | **Modules** (artifacts, blobs, or a module's own store) |

The core stays small, bounded, and domain-neutral either way.

- The event bus is **notification, not the data plane**. Artifact refs carry
  values; the ledger is the durable record. Subscriber queues are therefore
  **bounded** — a slow consumer is shed rather than allowed to OOM the kernel.
  Dropped notifications remain reconstructable from the chain.
- `RequestContext` (caller, idempotency key, deadline, correlation id) rides as
  **call metadata** — gRPC headers on the wire, an optional parameter
  in-process. It is deliberately **not** folded into hash-chained ledger
  `detail`: the chain records *what happened to state*, not who asked. Folding
  caller/trace ids into the hash would break the cross-SDK known-answer chain.
- **Deadline:** when `deadline_unix_ms > 0` and wall clock is past that instant,
  Result-returning RPCs fail with `FAILED_PRECONDITION` (`deadline exceeded`).
- **Idempotency:** a non-empty `request_key` on `PutArtifact`, `StartRun`, and
  `Commit` de-duplicates by `(caller, request_key, operation)` — the first
  success is cached; later calls with the same key return the same response
  without re-applying side effects.

---

## The seven primitives

Each primitive names one message (or small group) in `substrate.proto` and one
**invariant** the kernel guarantees. The invariants are the contract; the wire
shapes just carry them.

There is **no kernel-level authorisation**. Systems built on this core are
trusted; modules and host processes are not principals the kernel polices.
`RequestContext.caller` is observational only.

| # | Primitive | Message(s) | Invariant the kernel guarantees |
|---|-----------|------------|---------------------------------|
| 1 | **Module** | `ModuleManifest`, `Capability`, `Port`, `Lifecycle`, `TransitionRequest` | A module is a self-contained vertical slice. Capabilities declare typed input/output **ports** (the only I/O typing). `requires` is a boot/load **availability hint only** — never run dataflow; the kernel does not gate `LOADED` or `ClaimReady` on it. Lifecycle moves through `REGISTERED → LOADED → ACTIVE → DEACTIVATED` via `Transition` (one forward step at a time) and the module never imports another module. |
| 2 | **Artifact** | `Artifact`, `ArtifactRef`, `ObjectRef`, `BlobRef` | Typed, content-addressed, **immutable**. Value identity and blob identity are distinct. **Inline (small):** `id = H(type ‖ 0x00 ‖ body)`. **External (large):** `id = H(type ‖ 0x00 ‖ object_ref_bytes(object))` where `object_ref_bytes = digest ‖ 0x00 ‖ uint64_be(byte_count) ‖ 0x00 ‖ namespace`. **Blob identity** is pure content: `digest = "sha256:" + hex(sha256(data))`. Same value ⇒ same artifact id; any change ⇒ a new id. Large blobs live in the content-addressed blob store; artifacts hold a verified `ObjectRef` without copying bytes. Production provenance lives **only** in separate `Derivation` records — not on the Artifact. |
| 3 | **Contract** | `Contract`, `Port.contract`, `Artifact.type`, `Event.type` | The declarative schema is the **sole** coupling point. Modules couple to a contract **identity** (`ref` pinned to a content `digest`), never to each other's code. Ports (and artifact/event types) name the ref. Registration is **immutable**: same ref + different content ⇒ conflict. |
| 4 | **Event** | `Event`, `Subscription` | Modules publish to topics and subscribe to topics; they never call each other directly. Every event gets a monotonic `seq` — a **total order** within the kernel. **Artifact refs are the data plane** (`Event.artifacts`); the bus never carries domain value bytes. |
| 5 | **Ledger** | `LedgerEntry` | Append-only and **hash-chained**. Each entry commits to the previous entry's hash, so the whole history is tamper-evident and fully agent-observable. Every meaningful kernel action writes one entry. |
| 6 | **Registry** | `RegistrySnapshot` | Discovery. At any moment you can ask the kernel what modules, capabilities, and contracts exist. This is the "what systems are here?" answer, always available. |
| 7 | **Run** | `Assembly`, `Run`, `WorkItem`, `Derivation` | Convergence. A finite, acyclic, version-pinned assembly connects typed capability ports and names exactly one terminal output. A run executes each node at most once and terminates as `COMPLETED`, `STALLED`, `FAILED`, or `CANCELLED`. |

### The Kernel ABI

`substrate.proto` also defines one `service Kernel` — the seam every SDK
implements. It is the union of the operations above (`Register`, `Transition`,
`PutArtifact`, `GetArtifact`, `PutBlob`, `GetBlob`, `HasBlob`, `PutContract`,
`Publish`, `Subscribe`, `Append`, `Snapshot`, `StartRun`, `ClaimReady`,
`Commit`, `GetRun`, `CancelRun`, `ListDerivations`). An SDK MAY realise it
in-process (methods mirroring these RPCs) or over the wire (gRPC) — the
*invariants* are identical either way. Modules see only the kernel; they never
see each other.

In-process SDKs surface the same ABI as a language-native `KernelApi`
(trait / interface / Protocol). The shipped `MemoryKernel` implements it; other
backends may too. Streaming `Subscribe` stays inherent-only on the in-memory
type (channel / queue). Every unary call also accepts an optional
`RequestContext` and maps native failures onto the portable `Error` message
(`ErrorCode` + retryability).

### Contract identity (immutable registration)

A capability or port names a contract by **`ref`** (a stable human string). That
ref is an identity handle: the kernel pins it to one content digest and never
lets the meaning change under the name.

| Field | Role |
|-------|------|
| `ref` | Registry key and the string ports / artifacts / events carry |
| `media_type` | Schema language (e.g. `text/x-protobuf`, `application/schema+json`) |
| `schema` | Schema text; may be empty only for name-only placeholders |
| `version` | Optional explicit version metadata (advisory; digests are authoritative) |
| `digest` | Kernel-assigned content address |
| `compatible_with` | Optional other refs this contract claims compatibility with (advisory) |

**Digest rule.**

```
digest = "sha256:" + hex(sha256(
  media_type ‖ 0x00 ‖ schema ‖ 0x00 ‖ version ‖ 0x00 ‖
  compatible_with…   // sorted ascending as raw UTF-8; each entry followed by 0x00
))
```

**Registration rules the kernel enforces.**

1. `PutContract` requires a non-empty `ref`. It normalizes `compatible_with` to
   UTF-8 ascending order, assigns `digest`, and stores the contract.
2. First write of a `ref` wins. An identical re-put (same digest) is a no-op and
   returns the stored contract.
3. A later put of the same `ref` with a **different** digest is `CONFLICT`
   (`conflict_subject = ref`). Content cannot be redefined under a name.
4. `Register` may create an empty name-only placeholder when a capability/port
   names a ref that is not yet registered. A placeholder (empty `media_type`,
   `schema`, `version`, and `compatible_with`) may be **filled once** by
   `PutContract` with real content; after that, rule 3 applies.
5. If the caller supplies `digest`, it must match the kernel recomputation or
   the put is `INVALID`.
6. Ports and capabilities bind to the **ref**. Because the registry freezes
   content under that ref, binding to the ref *is* binding to contract identity.
   Assemblies still match ports by ref equality; they never parse schema bodies.

### Production artifact boundary (inline vs external)

The reference kernel must stay honest for small typed values **and** usable with
large existing content (PCAP, APK, evidence bundles) without forcing those bytes
through the artifact store.

| Mode | When | How identity works | Where bytes live |
|------|------|--------------------|------------------|
| **Inline** | Small typed values | `id = H(type ‖ 0x00 ‖ body)` | `Artifact.body` |
| **External** | Large / shared blobs | `id = H(type ‖ 0x00 ‖ object_ref_bytes(object))` | Blob store; artifact holds `ObjectRef` |

Rules the kernel enforces:

1. Exactly one of `body` or `object` (non-empty digest) carries the value.
2. `PutBlob` content-addresses raw bytes (`digest = H(data)`), stores immutably
   under `(namespace, digest)`, and never interprets domain content. First write
   wins for a given key.
3. `PutArtifact` with `object` set requires the blob to already exist in
   `object.namespace`, with matching digest and `byte_count`. `body` must be empty.
4. `GetBlob` re-hashes stored bytes and rejects digest or size mismatches
   (verified external refs).
5. Typed value equality is independent of blob location policy beyond what is
   encoded in `ObjectRef`: same `(type, object_ref_bytes)` converges; same blob
   bytes under different namespaces are different object refs and may be
   different artifact ids.
6. The ledger never records raw blob data or inline artifact bodies; it records
   digests / `ObjectRef` / `BlobRef` metadata only.

---

## How the primitives compose (the whole loop)

1. A **Module** registers, declaring typed input/output ports on its
   **Capabilities**.
2. A human-owned **Assembly** pins module versions, binds compatible ports, and
   names exactly one terminal output. The kernel rejects missing bindings,
   incompatible contracts, ambiguous scalar fan-in, and cycles.
3. A **Run** freezes that assembly with named immutable input **Artifacts**.
   Modules atomically claim ready nodes; a node becomes ready only after all of
   its bound inputs exist.
4. A module produces immutable output Artifacts and commits a **Derivation**.
   The kernel validates their contracts, releases downstream nodes, and closes
   the run when the terminal output appears. Events may notify workers, but
   artifact refs — never event payload bytes — are the run's data plane.
5. Every step lands in the append-only **Ledger**, so a human or an agent can
   reconstruct exactly what happened and verify it wasn't altered.
6. The **Registry** always answers "what exists right now."

That is the complete substrate. Six re-derivations across Go, Rust, and Python
collapse to this.

---

## Ledger detail — what each entry carries

The Ledger is only as good as what it commits to. `LedgerEntry.detail` is not
free-form: for every state-bearing `kind` it holds the **canonical protobuf
encoding of exactly one `substrate.proto` message**, so the chain records not
merely *that* something happened but *what* — reconstructable, and tamper-evident
because `detail` is folded into the entry hash.

| `kind` | `detail` message |
|--------|------------------|
| `module.registered` | `ModuleManifest` |
| `module.{loaded,activated,deactivated}` | *(empty — `kind` + `subject` suffice)* |
| `contract.registered` | `Contract` (full, including kernel-assigned `digest`) |
| `artifact.put` | `Artifact`, `body` cleared (`object` retained — it is small and part of value identity) |
| `blob.put` | `BlobRef` (no data bytes) |
| `event.published` | `Event` (artifact refs are the data plane; no domain value body) |
| `run.started`, `run.{progressed,completed,stalled,failed,cancelled}` | `Run` |
| `work.claimed` | `WorkItem` |
| `derivation.committed` | `Derivation` |
| module `Append` | opaque, module-owned bytes (the kernel never interprets them) |

Where `subject` already commits to large content — an inline `artifact.put`
body is addressed by its id; a `blob.put` is addressed by its digest — the large
field is cleared and the log leans on that reference. Verified `ObjectRef` /
`BlobRef` metadata stays in `detail` so external values reconstruct without
re-inlining blob bytes. Events carry artifact refs, not value bytes. The chain
never duplicates multi-megabyte content into a record it can never prune.

**Canonical form.** Two conformant SDKs must hash identical bytes for the same
history, or their chains would not cross-verify. So the encoding is pinned:

1. fields in ascending field-number order (proto3 default);
2. `map<>` entries in ascending key order, compared as raw UTF-8 bytes;
3. proto3 default-omission, plus the explicit clears above;
4. no unknown fields; standard varint / fixed-width encodings only.

This is a rule SDKs uphold, **not** a wire change — `detail` was always `bytes`.
Pin it now, while one SDK exists; discovered after several exist, it is a breaking
change to chain verification.

> **Status (v1.0.0).** All three SDKs — Rust, Go, and Python — enforce this for
> `module.registered` and `artifact.put`, so the registry and the artifact store
> both reconstruct from the chain alone. A shared known-answer fixture pins the
> exact final chain hash and every suite asserts the same constant, so the three
> chains are proven to cross-verify byte-for-byte. Run state, claimed work, and
> derivations use the same canonical encoding; a second shared known-answer
> fixture pins a complete convergent run's derivation id and final ledger hash
> across all three SDKs.

### Run closure rules

- An assembly contains a finite set of nodes and exactly one terminal output.
- Every node pins `(module, module_version, capability)`; resolution is never
  "latest" and duplicate providers at that exact coordinate are rejected as
  ambiguous.
- Every required input port has a binding. Multiple bindings require a port
  explicitly marked `multiple`; unbound ports must be explicitly `optional`.
- Bindings must preserve contract refs. The kernel never parses domain bodies.
- Assemblies are acyclic. Iteration requires a future bounded primitive with an
  explicit fixed-point rule, never an accidental event loop.
- A node is claimed and committed at most once per run. A commit records exact
  input and output artifact refs in a separate immutable `Derivation`.
- Artifact identity remains the typed value: `(type, body)` when inline, or
  `(type, object_ref_bytes(object))` when external. Equal values share one
  address while distinct derivations remain separately observable. Blob identity
  (`digest` of raw bytes) is separate and lives in the blob store.
- The terminal output yields `COMPLETED`; no ready or in-flight node yields
  `STALLED`; exhausting `max_steps` yields `FAILED`. Terminal runs accept no
  further claims or commits.

---

## Evolution policy (why this can be "the last one")

The contract stays trustworthy because it is **versioned and governed**, not
because it is locked forever:

| Promise | What it means |
|---------|----------------|
| **Versioned** | Package path (`srcport.substrate.v1`) and semver tags. Projects pin deliberately. |
| **Mechanically compatibility-checked** | Every change runs `buf breaking` before it can land. The check — not good intentions — enforces the rule. |
| **Deprecations documented** | Fields and RPCs are reserved or marked deprecated with a documented replacement; never silently removed. |
| **Security fixes permitted** | Always, including within a major line, without waiting for a redesign. |
| **v2 for genuine corrections** | An incompatible redesign becomes `…v2` living beside `v1`, never a silent break of v1 consumers. |
| **Support windows published** | Each major line publishes how long it is supported and when it reaches end-of-life. |

Within a major version, evolution is by **addition**, never by mutation:

- **Never** change or reuse a field number. **Never** renumber. **Never** repurpose
  a field's meaning.
- To add capability: add a new field (proto3 optional/defaulted), a new message, or
  a new RPC. Old SDKs keep working; they ignore what they don't know.
- Removing a field: reserve its number and name; do not delete-and-reuse. Document
  the deprecation and the replacement path.
- Every change bumps the version and is mechanically checked (`buf breaking`, below)
  before it can land.

### Versioning

- `v0.x` — pre-stable drafts (`v0.1.0` Rust-only; `v0.1.1` three SDKs + runs).
- **`v1.0.0`** — the first **stable** line (this document). The seven primitives,
  blob store, immutable contract identity, `RequestContext` / portable `Error`,
  and the `KernelApi` / `MemoryKernel` split. From this tag on, the evolution
  rules above and the breaking-change check are law. Projects pin an exact
  version and upgrade deliberately.

---

## Conformance (what "an SDK" must prove)

An SDK for any language is conformant iff it upholds every invariant in the table.
The minimal conformance suite (each SDK ships it) is:

1. **Addressing** — same typed value yields the same `id` (inline: same
   `(type, body)`; external: same `(type, object_ref_bytes)`); a one-byte change
   yields a different `id`. (Metamorphic: content change *must* change the address.)
2. **Immutability** — a stored artifact reads back byte-identical and is never
   altered by a later put of the same id.
3. **Ordering & isolation** — published events reach exactly their subscribers, in
   `seq` order, and never reach non-subscribers.
4. **Ledger integrity** — the chain verifies; tampering with any committed entry
   breaks verification.
5. **Discovery** — the registry reports every registered module, capability, and
   contract.
5b. **Contract identity** — `PutContract` content-addresses
   `(media_type, schema, version, compatible_with)` under a `ref`; identical
   re-puts are no-ops; a different content under the same ref is `CONFLICT`; a
   name-only placeholder may be filled once. Ports bind to that pinned identity.
6. **Ledger reconstruction & canonical detail** — a state-bearing entry's `detail`
   decodes to the message named for its `kind` and reproduces the original value,
   and re-encoding it canonically is byte-identical, so chains cross-verify. All
   three SDKs enforce this for `module.registered` and `artifact.put` — the
   registry and the artifact store both round-trip from the tamper-evident chain
   alone. A shared known-answer fixture pins the exact chain hash identically
   across Rust, Go, and Python, so cross-verification is proven, not assumed.
7. **Address invariance** — `meta` and `produced_by` are not part of the address;
   an identity-preserving change to them must *not* move the `id`. (Metamorphic:
   the mirror of #1 — a change that preserves the typed value must preserve the
   address.) Provenance is a `Derivation`, never part of artifact identity.
8. **Feed-forward convergence** — downstream work is unavailable until every
   typed binding resolves; fan-in supplies the complete input set; the declared
   terminal artifact closes the run and a closed run cannot reopen.
9. **Structural termination** — cycles and invalid bindings are rejected;
    exhausted work becomes `STALLED`, and `max_steps` bounds committed work.
10. **Derivation preservation** — value-equal artifacts share an address while
    distinct production paths remain separately committed and observable.
11. **Production artifact boundary** — small values inline in `Artifact.body`;
    large values use a verified `ObjectRef` (digest, byte_count, namespace)
    after `PutBlob`. Blob identity is `H(data)` only. `GetBlob` verifies digest
    and size. Typed value identity does not hash blob bytes. Exactly one of
    body or object is set. The ledger never stores raw blob data.

An SDK is "done" when these pass and the human-owned contract above is unchanged.
Everything else in the SDK is a leaf you never have to read.
