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

    subgraph Kernel["Kernel ABI — the only seam a module ever touches"]
        direction LR
        P1["① Module"]
        P2["② Artifact"]
        P3["③ Contract"]
        P4["④ Event"]
        P5["⑤ Ledger"]
        P6["⑥ Gate"]
        P7["⑦ Registry"]
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
    class P1,P2,P3,P4,P5,P6,P7 ker;
    class S1,S2,S3 sdk;
```

Nothing in the kernel knows about any domain. That shared frozen core is the
thing that finally becomes boring, trusted, and legible.

---

## The seven primitives

Seven primitives plus one `Kernel` ABI. Small enough to hold in your head.

| # | Primitive | Guarantees |
|---|-----------|------------|
| ① | **Module** | A self-contained vertical slice. Declares `provides`/`requires`; never imports another module. |
| ② | **Artifact** | Typed, content-addressed, **immutable**. Same content ⇒ same id. |
| ③ | **Contract** | The declarative schema — the **sole** coupling point between modules. |
| ④ | **Event** | Publish/subscribe topics with a kernel-assigned **total order** (`seq`). |
| ⑤ | **Ledger** | Append-only, **hash-chained**, tamper-evident record of every action. |
| ⑥ | **Gate** | A **human-held** checkpoint before anything irreversible. Non-bypassable. |
| ⑦ | **Registry** | Discovery — "what modules, capabilities, and contracts exist right now?" |

---

## How the primitives compose (the whole loop)

A module wakes on an event, produces an immutable artifact, publishes the next
event, and pauses at a human gate before anything irreversible — every step
landing in the ledger.

```mermaid
sequenceDiagram
    autonumber
    participant H as Human
    participant Mod as Module
    participant K as Kernel
    participant L as ⑤ Ledger

    Mod->>K: Register (provides / requires)
    K-->>L: append "module.activated"
    Note over Mod,K: ⑦ Registry now answers "what exists"

    K-->>Mod: ④ Event (subscribed topic, in seq order)
    Mod->>K: PutArtifact (② immutable, content-addressed)
    K-->>L: append "artifact.put"
    Mod->>K: Publish next ④ Event (coupled only via ③ contract ref)
    K-->>L: append "event.published"

    Mod->>K: RequestGate (irreversible act)
    K-->>L: append "gate.requested"
    Note over Mod,H: ⑥ blocked while PENDING / REJECTED
    H->>K: DecideGate → APPROVED
    K-->>L: append "gate.decided"
    K-->>Mod: proceed
```

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

The artifact id **is** a hash of its content. Same `(type, body)` always yields
the same id; flip a single byte and you get a brand-new id. Stored artifacts are
never mutated in place.

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

## The one rule

> **Do not create a new repo that re-implements this core.** If it lacks
> something, **widen this contract by adding to it**, tag a new version, and let
> every project pick it up. Re-derivation is the bug this repo exists to kill.

Evolution is by **addition, never mutation** — enforced mechanically, not by
good intentions:

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
└─ sdk/                                     # each conforms to SPEC.md
   ├─ rust/                                 # in-process Rust SDK (types generated by build.rs)
   ├─ go/                                   # in-process Go SDK (types generated via buf)
   └─ python/                               # in-process Python SDK (types generated via buf)
```

Every SDK's message types are **generated from `substrate.proto`** — Rust at
build time (`build.rs`), Go and Python via `buf generate` (committed). None
hand-writes the contract, so none can drift from it; CI fails if the committed
codegen falls out of sync. Every SDK realises the same `Kernel` ABI in-process
and ships the same **six-test conformance suite**:

| # | Test | Proves |
|---|------|--------|
| 1 | **Addressing** | same `(type, body)` ⇒ same id; one-byte change ⇒ different id |
| 2 | **Immutability** | a stored artifact reads back byte-identical |
| 3 | **Ordering & isolation** | events reach exactly their subscribers, in `seq` order |
| 4 | **Ledger integrity** | the chain verifies; tampering breaks it |
| 5 | **Gate non-bypass** | irreversible action blocked until `APPROVED` |
| 6 | **Discovery** | the registry reports every module, capability, and contract |

As a cross-check, all three SDKs produce **byte-identical artifact addresses**
for the same `(type, body)` — the spec's addressing rule, proven across
languages.

---

## Status

`v0.1` draft — **unfrozen, pending review**. The Rust SDK and the `v1.0.0`
freeze come after the schema is approved. Nothing depends on it yet.

```mermaid
flowchart LR
    A["v0.x<br/>unfrozen · may reshape"]:::now --> B["v0.1.0<br/>this draft · pending approval"] --> C["v1.0.0<br/>the freeze · rule is law"]:::frozen

    classDef now fill:#fff7ed,stroke:#f59e0b,color:#7c2d12;
    classDef frozen fill:#eff6ff,stroke:#3b82f6,color:#1e3a8a;
```

---

## License

Copyright (c) srcport.com. All rights reserved.
