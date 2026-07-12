# srcport-substrate

**One canonical contract, many conforming implementations.**

This repo is a **specification, not a framework**. It defines a small,
domain-neutral microkernel as a language-neutral contract; SDKs in each language
you need (Rust first) conform to it. Production consumers may run their own
durable backends — what must not be re-derived is the contract and its
invariants, not necessarily the storage implementation.

**Durability lives in Modules, not the core.** Each SDK ships an in-memory
`MemoryKernel` that implements the portable `KernelApi` so the contract is
executable and conformance-testable. That type is *one* backend, not the
authority — persistence belongs in Modules or in another `KernelApi`
implementation over durable storage.

- **[`SPEC.md`](SPEC.md)** — the two-page human-owned specification. Read this.
- **[`substrate.proto`](contracts/proto/srcport/substrate/v1/substrate.proto)** — the canonical contract (protobuf-first).
- **[`buf.yaml`](buf.yaml)** — lint + breaking-change enforcement.

---

## The big picture

Every project — VR, games, content, growth — is just a set of **Modules**
sitting on top of one shared, versioned contract. Modules never see each other;
they see only the kernel. Read the stack top-down: domains rest on the contract;
SDKs and backends sit *under* the contract and implement it.

```mermaid
flowchart TB
    subgraph L1["① domains — horizontal peers, nothing privileged"]
        direction LR
        M1["VR<br/>module"] ~~~ M2["Game<br/>module"] ~~~ M3["Content<br/>module"] ~~~ M4["Growth<br/>module"]
    end

    ABI["↓ only the Kernel ABI · modules never import each other ↓"]

    subgraph L2["② contract — seven primitives + one Kernel service"]
        direction TB
        subgraph Nouns["primitives · the nouns"]
            direction LR
            P1["① Module"] ~~~ P2["② Artifact"] ~~~ P3["③ Contract"] ~~~ P4["④ Event"] ~~~ P5["⑤ Ledger"] ~~~ P6["⑥ Registry"] ~~~ P7["⑦ Run"]
        end
        Verbs["Kernel service · the verbs<br/>Register · PutArtifact · Publish · StartRun · Commit · …"]
        Nouns --- Verbs
    end

    IMPL["↓ same ABI · same conformance suite · per language ↓"]

    subgraph L3["③ SDKs — conforming implementations"]
        direction LR
        S1["Rust"] ~~~ S2["Go"] ~~~ S3["Python"]
    end

    subgraph L4["④ KernelApi backends — durability lives here, not in the core"]
        direction LR
        B1["MemoryKernel<br/>shipped · in-memory"] ~~~ B2["Your durable store<br/>Postgres · files · …"]
    end

    L1 --> ABI --> L2 --> IMPL --> L3 --> L4

    classDef dom fill:#eef2ff,stroke:#6366f1,stroke-width:1px,color:#1e1b4b;
    classDef ker fill:#ecfdf5,stroke:#10b981,stroke-width:1px,color:#064e3b;
    classDef abi fill:#f0fdf4,stroke:#059669,stroke-width:1px,color:#064e3b;
    classDef sdk fill:#fff7ed,stroke:#f59e0b,stroke-width:1px,color:#7c2d12;
    classDef back fill:#faf5ff,stroke:#a855f7,stroke-width:1px,color:#581c87;
    classDef seam fill:#f8fafc,stroke:#94a3b8,stroke-width:1px,stroke-dasharray: 4 3,color:#475569;
    class M1,M2,M3,M4 dom;
    class P1,P2,P3,P4,P5,P6,P7 ker;
    class Verbs abi;
    class S1,S2,S3 sdk;
    class B1,B2 back;
    class ABI,IMPL seam;
```

Nothing in the kernel knows about any domain. That shared, versioned contract is
the thing that finally becomes boring, trusted, and legible.

---

## See it run

[`example/`](example/) builds a tiny three-module domain on the Rust SDK, drives
a **real** convergent Run, then reconstructs the whole dataflow **solely by
decoding the append-only ledger** — proving, not merely illustrating, that
artifact refs are the data plane and the chain records exactly what happened.

```
cd example && cargo run
```

It prints a live trace and writes a self-contained `flow.html` — every box and
arrow rebuilt from the tamper-evident chain, never from live kernel state.

---

## The seven primitives

Seven primitives (the nouns) plus one `Kernel` ABI — the verb set (`Register`,
`PutArtifact`, `Publish`, …) that operates on them. Small enough to hold in your
head. There is no kernel-level authorisation: systems built on this core are trusted.
In-process, the ABI is the `KernelApi` surface; `MemoryKernel` is the default
in-memory implementation.

They form three horizontal bands — structure, values, process — peer groups at
the same level of concern. Contracts type the structure and value bands; artifact
refs (not events) feed runs; every action lands on the ledger.

```mermaid
flowchart TB
    subgraph Band1["structure · who and what exists"]
        direction LR
        P1["① Module<br/>vertical slice · typed ports<br/>never imports another module"] ~~~ P6["⑥ Registry<br/>discovery snapshot<br/>modules · caps · contracts"] ~~~ P3["③ Contract<br/>sole coupling point<br/>ref pinned to content digest"]
    end

    subgraph Band2["values · what is said"]
        direction LR
        P2["② Artifact<br/>data plane · immutable<br/>content-addressed"] ~~~ P4["④ Event<br/>notification only<br/>total order via seq"]
    end

    subgraph Band3["process · what converges and what was recorded"]
        direction LR
        P7["⑦ Run<br/>bounded assembly<br/>completes · stalls · fails · cancels"] ~~~ P5["⑤ Ledger<br/>append-only hash chain<br/>tamper-evident history"]
    end

    Band1 -->|"contracts type ports and values"| Band2
    Band2 -->|"artifact refs feed runs · events only notify"| Band3

    classDef s fill:#eef2ff,stroke:#6366f1,color:#1e1b4b;
    classDef v fill:#ecfdf5,stroke:#10b981,color:#064e3b;
    classDef p fill:#fff7ed,stroke:#f59e0b,color:#7c2d12;
    class P1,P6,P3 s;
    class P2,P4 v;
    class P7,P5 p;
```

| # | Primitive | Guarantees |
|---|-----------|------------|
| ① | **Module** | A vertical slice with typed capability ports; never imports another module. |
| ② | **Artifact** | Typed, content-addressed, **immutable**. Small values inline; large values hold a verified external blob ref. Same typed value ⇒ same id. |
| ③ | **Contract** | The declarative schema — the **sole** coupling point. Immutable identity: `ref` pinned to a content `digest`. |
| ④ | **Event** | Publish/subscribe topics with a kernel-assigned **total order** (`seq`). |
| ⑤ | **Ledger** | Append-only, **hash-chained**, tamper-evident record of every action. |
| ⑥ | **Registry** | Discovery — "what modules, capabilities, and contracts exist right now?" |
| ⑦ | **Run** | Applies an immutable input set to a finite typed assembly; must close as completed, stalled, failed, or cancelled. |

---

## Module lifecycle

A module moves through exactly four states — no back-doors, no skipping.
States are a strict forward chain (horizontal), not a free graph:

```mermaid
stateDiagram-v2
    direction LR
    [*] --> REGISTERED: Register
    REGISTERED --> LOADED: dependencies resolved
    LOADED --> ACTIVE: activate
    ACTIVE --> DEACTIVATED: shut down
    DEACTIVATED --> [*]
```

---

## Content addressing

Typed **value** identity is sha256 over `type · 0x00 · content`, where `content`
is either the small inline `body` or the address-bytes of a verified `ObjectRef`
(digest · byte_count · namespace). Flipping a single byte yields a brand-new id.
**Blob** identity is separate: `digest = sha256(raw bytes)` only.

Two horizontal modes share one id formula; only the content source differs:

```mermaid
flowchart TB
    subgraph Modes["two modes · pick one content source"]
        direction LR
        BI["inline · small<br/>content = body<br/>bytes live in the artifact record"] ~~~ BE["external · large<br/>content = object_ref_bytes<br/>bytes live in the blob store"]
    end

    subgraph Formula["one formula · both modes"]
        direction LR
        T["type<br/>contract ref"] --> C(("type · 0x00 · content"))
        C --> S["sha256"] --> ID["artifact id = sha256: + hex"]
    end

    BI --> C
    BE --> C

    classDef n fill:#ecfdf5,stroke:#10b981,color:#064e3b;
    classDef m fill:#eef2ff,stroke:#6366f1,color:#1e1b4b;
    class T,C,S,ID n;
    class BI,BE m;
```

| Mode | Content hashed into artifact id | Bytes live in |
|------|----------------------------------|---------------|
| **Inline** (small) | `body` | the artifact record |
| **External** (large) | `object_ref_bytes(ObjectRef)` | blob store (`PutBlob` / `GetBlob`) |

Modules place large evidence (PCAP, APK, bundles) by putting the blob once and
committing an artifact that holds only the verified ref — no copy into the
typed value store.

---

## The ledger is a hash chain

Every meaningful kernel action appends one entry, and each entry commits to the
previous entry's hash. Tamper with any entry and every later hash stops
verifying — the whole history is agent-observable and tamper-evident.

```mermaid
flowchart LR
    subgraph Chain["append-only · each link seals the previous"]
        direction LR
        G["entry 0<br/>genesis<br/>prev = ''"] --> E1["entry 1<br/>prev = H0"]
        E1 --> E2["entry 2<br/>prev = H1"]
        E2 --> E3["entry 3<br/>prev = H2"]
        E3 --> More["…"]
    end

    classDef l fill:#eef2ff,stroke:#6366f1,color:#1e1b4b;
    class G,E1,E2,E3,More l;
```

Each `hash = sha256(seq, kind, subject, detail, prev_hash)`.

---

## How the primitives converge (the bounded, feed-forward run)

A **Run** freezes a finite acyclic assembly plus immutable input **Artifact**
refs. Modules never call each other: each talks only to the kernel. A node
becomes claimable only when **every** bound input artifact already exists
(fan-in waits; it does not race). Events may wake workers, but **artifact refs
are the data plane** — not event payloads. Every step is appended to the
**Ledger**.

Feed-forward shape (left → right):

```mermaid
flowchart LR
    IN["input<br/>ArtifactRefs"] --> UP["upstream node<br/>claim → put → commit"]
    UP --> MID["intermediate<br/>ArtifactRefs"]
    MID --> DN["downstream node<br/>claim → put → commit"]
    DN --> OUT["Answer<br/>ArtifactRef"]

    classDef a fill:#ecfdf5,stroke:#10b981,color:#064e3b;
    classDef m fill:#eef2ff,stroke:#6366f1,color:#1e1b4b;
    class IN,MID,OUT a;
    class UP,DN m;
```

Same story as a timeline. Read top → bottom; each phase is one closed beat.
Ledger writes are notes on the kernel (side effects), not a separate actor.

```mermaid
sequenceDiagram
    autonumber
    actor Owner as Owner
    participant Kernel as Kernel
    participant Up as Upstream module
    participant Down as Downstream module

    rect rgb(248, 250, 252)
        Note over Owner,Kernel: Phase 1 — start the run
        Owner->>Kernel: StartRun(assembly + input ArtifactRefs)
        Note right of Kernel: ledger ← run.started<br/>upstream is ready (inputs exist)<br/>downstream is NOT ready yet
    end

    rect rgb(238, 242, 255)
        Note over Kernel,Up: Phase 2 — upstream produces intermediate artifacts
        Up->>Kernel: ClaimReady
        Kernel-->>Up: typed input ArtifactRefs
        Up->>Kernel: PutArtifact + Commit(Derivation)
        Note right of Kernel: ledger ← derivation.committed<br/>downstream fan-in is now complete
    end

    rect rgb(236, 253, 245)
        Note over Kernel,Down: Phase 3 — downstream runs only after fan-in
        Down->>Kernel: ClaimReady
        Kernel-->>Down: full input ArtifactRefs
        Down->>Kernel: Put terminal Artifact + Commit
        Note right of Kernel: ledger ← run.completed
    end

    rect rgb(255, 247, 237)
        Note over Owner,Kernel: Phase 4 — answer
        Kernel-->>Owner: Answer ArtifactRef
    end
```

---

## The one rule

> **One canonical contract, many conforming implementations.** A production
> consumer will often need its own durable backend. The thing that must not be
> re-derived is the **contract and invariants** — not necessarily the storage
> implementation. If the contract lacks something, **widen this contract by
> adding to it**, tag a new version, and let every project pick it up.
> Re-deriving the contract is the bug this repo exists to kill.

The contract is **versioned**, not frozen forever:

| Promise | What it means |
|---------|----------------|
| **Versioned** | Package path (`srcport.substrate.v1`) and semver tags |
| **Mechanically compatibility-checked** | `buf breaking` blocks renumbers, repurposes, and silent removals |
| **Deprecations documented** | Reserved / marked with a replacement path; never silently dropped |
| **Security fixes permitted** | Always, within a major line |
| **v2 for genuine corrections** | Incompatible redesigns live beside `v1`, never break it silently |
| **Support windows published** | Each major line states how long it is supported |

Within a major version, evolution is by **addition, never mutation**:

```mermaid
flowchart LR
    Change["proposed change"] --> Q{"adds only?<br/>(new field / message / RPC)"}
    Q -->|yes| Bump["bump version →<br/>buf breaking passes"] --> Land["lands"]
    Q -->|"no — renumber / repurpose / remove"| Block["buf breaking blocks it"]

    classDef ok fill:#ecfdf5,stroke:#10b981,color:#064e3b;
    classDef bad fill:#fef2f2,stroke:#ef4444,color:#7f1d1d;
    class Bump,Land ok;
    class Block bad;
```

---

## Layout

Repo layout mirrors the stack: human-owned contract at the top, generated
conforming SDKs beneath, example domain on the side.

```
srcport-substrate/
│
├─ SPEC.md                                  # human-owned specification
├─ buf.yaml / buf.gen.yaml                  # lint · breaking · codegen
├─ scripts/gen.sh                           # regenerate SDK types
│
├─ contracts/                               # ── the canonical contract ──
│  └─ proto/srcport/substrate/v1/
│     └─ substrate.proto                    # THE contract
│
├─ sdk/                                     # ── conforming implementations ──
│  ├─ rust/                                 # in-process (types via build.rs)
│  ├─ go/                                   # in-process (types via buf)
│  └─ python/                               # in-process (types via buf)
│
└─ example/                                 # ── a domain on the Rust SDK ──
                                            # three modules · real Run · ledger HTML
```

---

## Conformance

Every SDK's message types are **generated from `substrate.proto`** — Rust at
build time (`build.rs`), Go and Python via `buf generate` (committed). None
hand-writes the contract, so none can drift from it; CI fails if the committed
codegen falls out of sync. Every SDK realises the same `Kernel` ABI in-process
and ships the same convergence-aware conformance suite.
[`SPEC.md` §Conformance](SPEC.md) states each invariant in full; the eleven it proves:

| # | Invariant | | # | Invariant |
|---|-----------|---|---|-----------|
| 1 | **Addressing** | | 6 | **Ledger reconstruction & canonical detail** |
| 2 | **Immutability** | | 7 | **Address invariance** |
| 3 | **Ordering & isolation** | | 8 | **Feed-forward convergence** |
| 4 | **Ledger integrity** | | 9 | **Structural termination** |
| 5 | **Discovery** | | 10 | **Derivation preservation** |
| 11 | **Production artifact boundary** | | | |

As a cross-check, all three SDKs produce **byte-identical artifact addresses**
for the same `(type, body)`.

---

## Status

**`v1.0.0` — stable.** Rust, Go, and Python implement the same `KernelApi` ABI
(with `MemoryKernel` as the in-process backend). The contract is versioned and
compatibility-checked (`buf breaking`); pin an exact tag and upgrade deliberately.

---

## License

Dual-licensed under MIT OR Apache-2.0. See [`LICENSE`](LICENSE),
[`LICENSE-MIT`](LICENSE-MIT), and [`LICENSE-APACHE`](LICENSE-APACHE).
