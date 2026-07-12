# srcport-substrate

**The one pluggable core for every future project — frozen once, reused forever.**

This repo is a **specification, not a framework**. It defines a small,
domain-neutral microkernel as a language-neutral contract; SDKs in each language
you need (Rust first) conform to it. It exists to end the pattern of re-deriving
the same substrate every time a new project starts.

- **[`SPEC.md`](SPEC.md)** — the two-page human-owned specification. Read this.
- **[`substrate.proto`](contracts/proto/srcport/substrate/v1/substrate.proto)** — the canonical contract (protobuf-first).
- **[`buf.yaml`](buf.yaml)** — lint + breaking-change enforcement.

---

## The big picture

Every project — VR, games, content, growth — is just a set of **Modules**
sitting on top of one shared, unchanging core. Modules never see each other;
they see only the kernel.

```mermaid
flowchart TB
    subgraph Domain["Your domains — just Modules, nothing privileged"]
        direction LR
        M1["VR module"]
        M2["Game module"]
        M3["Content module"]
        M4["Growth module"]
    end

    subgraph Kernel["The kernel — eight frozen primitives"]
        direction LR
        P1["① Module"]
        P2["② Artifact"]
        P3["③ Contract"]
        P4["④ Event"]
        P5["⑤ Ledger"]
        P6["⑥ Gate"]
        P7["⑦ Registry"]
        P8["⑧ Run"]
    end

    subgraph SDKs["SDKs — same ABI, same conformance suite, per language"]
        direction LR
        S1["Rust"]
        S2["Go"]
        S3["Python"]
    end

    Domain -->|"only through the Kernel ABI"| Kernel
    Kernel --- SDKs

    classDef dom fill:#eef2ff,stroke:#6366f1,stroke-width:1px,color:#1e1b4b;
    classDef ker fill:#ecfdf5,stroke:#10b981,stroke-width:1px,color:#064e3b;
    classDef sdk fill:#fff7ed,stroke:#f59e0b,stroke-width:1px,color:#7c2d12;
    class M1,M2,M3,M4 dom;
    class P1,P2,P3,P4,P5,P6,P7,P8 ker;
    class S1,S2,S3 sdk;
```

Nothing in the kernel knows about any domain. That shared frozen core is the
thing that finally becomes boring, trusted, and legible.

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

## The eight primitives

Eight primitives (the nouns) plus one `Kernel` ABI — the verb set (`Register`,
`PutArtifact`, `Publish`, …) that operates on them. Small enough to hold in your head.

| # | Primitive | Guarantees |
|---|-----------|------------|
| ① | **Module** | A vertical slice with typed capability ports; never imports another module. |
| ② | **Artifact** | Typed, content-addressed, **immutable**. Same content ⇒ same id. |
| ③ | **Contract** | The declarative schema — the **sole** coupling point between modules. |
| ④ | **Event** | Publish/subscribe topics with a kernel-assigned **total order** (`seq`). |
| ⑤ | **Ledger** | Append-only, **hash-chained**, tamper-evident record of every action. |
| ⑥ | **Gate** | A **human-held** checkpoint before anything irreversible. Non-bypassable. |
| ⑦ | **Registry** | Discovery — "what modules, capabilities, and contracts exist right now?" |
| ⑧ | **Run** | Applies an immutable input set to a finite typed assembly; must close as completed, stalled, failed, or cancelled. |

---

## Module lifecycle

A module moves through exactly four states — no back-doors, no skipping.

```mermaid
stateDiagram-v2
    [*] --> REGISTERED: Register
    REGISTERED --> LOADED: dependencies resolved
    LOADED --> ACTIVE: activate
    ACTIVE --> DEACTIVATED: shut down
    DEACTIVATED --> [*]
```

---

## Content addressing

The id **is** sha256 over `type · 0x00 · body`, so flipping a single byte yields
a brand-new id:

```mermaid
flowchart LR
    T["type<br/>(contract ref)"] --> C(("concat<br/>type · 0x00 · body"))
    B["body<br/>(opaque bytes)"] --> C
    C --> S["sha256"]
    S --> ID["id = 'sha256:' + hex(…)"]

    classDef n fill:#ecfdf5,stroke:#10b981,color:#064e3b;
    class T,B,C,S,ID n;
```

---

## The ledger is a hash chain

Every meaningful kernel action appends one entry, and each entry commits to the
previous entry's hash. Tamper with any entry and every later hash stops
verifying — the whole history is agent-observable and tamper-evident.

```mermaid
flowchart LR
    G["entry 0<br/>(genesis)<br/>prev_hash = ''"] --> E1["entry 1<br/>prev_hash = H0"]
    E1 --> E2["entry 2<br/>prev_hash = H1"]
    E2 --> E3["entry 3<br/>prev_hash = H2"]
    E3 --> More["…"]

    classDef l fill:#eef2ff,stroke:#6366f1,color:#1e1b4b;
    class G,E1,E2,E3,More l;
```

Each `hash = sha256(seq, kind, subject, detail, prev_hash)`.

---

## How the primitives converge (the bounded, feed-forward run)

A node is released only after every bound input artifact exists, so fan-in waits
rather than races; events may wake workers, but artifact refs — not event
payloads — are the run's data plane. The diagram below traces one such bounded run.

```mermaid
sequenceDiagram
    autonumber
    participant H as Human / assembly owner
    participant A as Upstream module
    participant B as Downstream module
    participant K as Kernel
    participant L as ⑤ Ledger

    H->>K: StartRun(assembly + input artifact refs)
    K-->>L: run.started
    A->>K: ClaimReady
    K-->>A: exact typed inputs
    A->>K: PutArtifact + Commit(Derivation)
    K-->>L: derivation.committed
    Note over B,K: downstream is now ready
    B->>K: ClaimReady
    K-->>B: complete fan-in input set
    B->>K: Put terminal Artifact + Commit
    K-->>L: run.completed
    K-->>H: Answer ArtifactRef
```

---

## The one rule

> **Do not create a new repo that re-implements this core.** If it lacks
> something, **widen this contract by adding to it**, tag a new version, and let
> every project pick it up. Re-derivation is the bug this repo exists to kill.

Evolution is by **addition, never mutation**:

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

A genuinely incompatible redesign becomes `…v2` living beside `v1`, never a
silent break.

---

## Layout

```
srcport-substrate/
├─ SPEC.md                                  # the human-owned specification
├─ buf.yaml                                 # lint + breaking-change enforcement
├─ buf.gen.yaml                             # codegen: contract → Go & Python types
├─ scripts/gen.sh                           # regenerate the SDK types
├─ contracts/proto/srcport/substrate/v1/
│  └─ substrate.proto                       # THE contract
├─ example/                                 # a runnable domain on the Rust SDK (see below)
└─ sdk/                                     # each conforms to SPEC.md
   ├─ rust/                                 # in-process Rust SDK (types generated by build.rs)
   ├─ go/                                   # in-process Go SDK (types generated via buf)
   └─ python/                               # in-process Python SDK (types generated via buf)
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
| 1 | **Addressing** | | 7 | **Ledger reconstruction & canonical detail** |
| 2 | **Immutability** | | 8 | **Address invariance** |
| 3 | **Ordering & isolation** | | 9 | **Feed-forward convergence** |
| 4 | **Ledger integrity** | | 10 | **Structural termination** |
| 5 | **Gate non-bypass** | | 11 | **Derivation preservation** |
| 6 | **Discovery** | | | |

As a cross-check, all three SDKs produce **byte-identical artifact addresses**
for the same `(type, body)`.

---

## Status

`v0.1` draft — **unfrozen, pending review**. Rust, Go, and Python implement the
same ABI; the `v1.0.0` freeze comes after the schema is approved.

---

## License

Copyright (c) srcport.com. All rights reserved.
