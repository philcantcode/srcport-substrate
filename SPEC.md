# srcport-substrate — Specification v0.1 (draft, unfrozen)

The one pluggable core for every future project. Domain-neutral. Small enough
to hold in your head. This document and [`substrate.proto`](contracts/proto/srcport/substrate/v1/substrate.proto)
are the **only** things a human owns and reads; SDKs conform to them, and coding
agents fill the leaves behind them.

> **The one rule.** You are not allowed to create a new repo that re-implements
> this core. If it lacks a capability, you widen *this* contract (by adding),
> tag a new version, and every project picks it up. Re-derivation is the bug.

---

## What this is (and is not)

It **is** the seven primitives below plus one ABI (the `Kernel` service). That is
the entire surface. If it doesn't fit on these two pages, the design is wrong.

It **is not** any domain. There is deliberately no notion of a target, trial,
finding, entity, component, terrain tile, or content pack in here. Every one of
those is a **Module** built on top. VR is a set of modules. A game is a set of
modules. A content factory is a set of modules. They share *this*, and nothing
else — that shared frozen core is the thing that finally becomes boring, trusted,
and legible.

---

## The seven primitives

Each primitive names one message (or small group) in `substrate.proto` and one
**invariant** the kernel guarantees. The invariants are the contract; the wire
shapes just carry them.

| # | Primitive | Message(s) | Invariant the kernel guarantees |
|---|-----------|------------|---------------------------------|
| 1 | **Module** | `ModuleManifest`, `Capability`, `Lifecycle` | A module is a self-contained vertical slice. It declares what it `provides`/`requires` and moves through `REGISTERED → LOADED → ACTIVE → DEACTIVATED`. It never imports another module. |
| 2 | **Artifact** | `Artifact`, `ArtifactRef` | Typed, content-addressed, **immutable**. `id = "sha256:" + hex(sha256(type + 0x00 + body))`. Same content ⇒ same id; any change ⇒ a new id. Stored artifacts are never mutated in place. `derived_from` records the parent ids it was built from — provenance that makes the immutable chain a lineage graph, and is deliberately **not** part of the address. |
| 3 | **Contract** | `Contract`, `Capability.contract`, `Artifact.type`, `Event.type` | The declarative schema is the **sole** coupling point. Modules couple to a contract *ref* (a string name), never to each other's code. |
| 4 | **Event** | `Event`, `Subscription` | Modules publish to topics and subscribe to topics; they never call each other directly. Every event gets a monotonic `seq` — a **total order** within the kernel. |
| 5 | **Ledger** | `LedgerEntry` | Append-only and **hash-chained**. Each entry commits to the previous entry's hash, so the whole history is tamper-evident and fully agent-observable. Every meaningful kernel action writes one entry. |
| 6 | **Gate** | `GateRequest`, `GateDecision`, `Decision` | A **human-held** checkpoint. Before anything irreversible, a module requests a gate and must not proceed until a human decides `APPROVED`. `PENDING` and `REJECTED` both block. Non-bypassable by design. |
| 7 | **Registry** | `RegistrySnapshot` | Discovery. At any moment you can ask the kernel what modules, capabilities, and contracts exist. This is the "what systems are here?" answer, always available. |

### The Kernel ABI

`substrate.proto` also defines one `service Kernel` — the seam every SDK
implements. It is the union of the operations above (`Register`, `PutArtifact`,
`GetArtifact`, `Publish`, `Subscribe`, `Append`, `RequestGate`, `DecideGate`,
`AwaitGate`, `Snapshot`). An SDK MAY realise it in-process (Rust methods) or over
the wire (gRPC) — the *invariants* are identical either way. Modules see only the
kernel; they never see each other.

---

## How the primitives compose (the whole loop)

1. A **Module** registers, declaring the **Contracts** it speaks via its
   **Capabilities**, and activates.
2. On an **Event** it is subscribed to, it produces an immutable **Artifact**
   and publishes a new **Event** — coupling only through contract refs.
3. Every step lands in the append-only **Ledger**, so a human or an agent can
   reconstruct exactly what happened and verify it wasn't altered.
4. Before any irreversible act, the module opens a **Gate** and waits for a human.
5. The **Registry** always answers "what exists right now."

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
| `artifact.put` | `Artifact`, `body` cleared |
| `event.published` | `Event`, `payload` cleared |
| `gate.requested` | `GateRequest` |
| `gate.decided` | `GateDecision` |
| module `Append` | opaque, module-owned bytes (the kernel never interprets them) |

Where `subject` already commits to large content — an `artifact.put` body is
addressed by its id; an `event.published` payload travels as an artifact — the
field is cleared and the log leans on that reference. The chain never duplicates
blob content into a record it can never prune.

**Canonical form.** Two conformant SDKs must hash identical bytes for the same
history, or their chains would not cross-verify. So the encoding is pinned:

1. fields in ascending field-number order (proto3 default);
2. `map<>` entries in ascending key order, compared as raw UTF-8 bytes;
3. proto3 default-omission, plus the explicit clears above;
4. no unknown fields; standard varint / fixed-width encodings only.

This is a rule SDKs uphold, **not** a wire change — `detail` was always `bytes`.
Pin it now, while one SDK exists; discovered after several exist, it is a breaking
change to chain verification.

> **Status (v0.1).** All three SDKs — Rust, Go, and Python — enforce this for
> `module.registered`, `artifact.put`, `gate.requested`, and `gate.decided`, so
> the registry, the artifact store, and the approval record all reconstruct from
> the chain alone. A shared known-answer fixture pins the exact final chain hash
> and every suite asserts the same constant, so the three chains are proven to
> cross-verify byte-for-byte. `event.published` waits on how a payload is
> addressed (as an artifact ref); the encoding rule above is fixed regardless.

---

## Evolution policy (why this can be "the last one")

The core stays trustworthy only if it changes by **addition**, never by mutation.

- **Never** change or reuse a field number. **Never** renumber. **Never** repurpose
  a field's meaning.
- To add capability: add a new field (proto3 optional/defaulted), a new message, or
  a new RPC. Old SDKs keep working; they ignore what they don't know.
- Removing a field: reserve its number and name forever; do not delete-and-reuse.
- Every change bumps the version and is mechanically checked (`buf breaking`, below)
  before it can land. The check — not good intentions — is what enforces the rule.
- The package is versioned in its path (`srcport.substrate.v1`). A genuinely
  incompatible redesign becomes `…v2` living beside `v1`, never a silent break.

### Versioning

- `v0.x` — this spec is **unfrozen** and may change shape. Nothing depends on it yet.
- `v0.1.0` (this draft, pending your approval) — the seven primitives and the ABI as
  written here.
- `v1.0.0` — the **freeze**. From that tag on, the rule above is absolute and the
  breaking-change check is law. Projects pin an exact version and upgrade deliberately.

---

## Conformance (what "an SDK" must prove)

An SDK for any language is conformant iff it upholds every invariant in the table.
The minimal conformance suite (each SDK ships it) is:

1. **Addressing** — same `(type, body)` yields the same `id`; a one-byte change
   yields a different `id`. (Metamorphic: content change *must* change the address.)
2. **Immutability** — a stored artifact reads back byte-identical and is never
   altered by a later put of the same id.
3. **Ordering & isolation** — published events reach exactly their subscribers, in
   `seq` order, and never reach non-subscribers.
4. **Ledger integrity** — the chain verifies; tampering with any committed entry
   breaks verification.
5. **Gate non-bypass** — an irreversible action is blocked while `PENDING`/`REJECTED`
   and permitted only after `APPROVED`.
6. **Discovery** — the registry reports every registered module, capability, and
   contract.
7. **Ledger reconstruction & canonical detail** — a state-bearing entry's `detail`
   decodes to the message named for its `kind` and reproduces the original value,
   and re-encoding it canonically is byte-identical, so chains cross-verify. All
   three SDKs enforce this for `module.registered`, `artifact.put`, and both gate
   kinds — the registry, the artifact store, and the approval record all round-trip
   from the tamper-evident chain alone. A shared known-answer fixture pins the exact
   chain hash identically across Rust, Go, and Python, so cross-verification is
   proven, not assumed.
8. **Address invariance** — `meta`, `produced_by`, and `derived_from` are not part
   of the address; an identity-preserving change to them must *not* move the `id`.
   (Metamorphic: the mirror of #1 — a change that preserves `(type, body)` must
   preserve the address.)

An SDK is "done" when these pass and the human-owned contract above is unchanged.
Everything else in the SDK is a leaf you never have to read.
