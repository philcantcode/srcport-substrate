# Artifacts

Short guide to how **artifacts** and **raw bytes** work in srcport-substrate.

## What an artifact is

An **artifact** is a content-addressed, **immutable trait bag**: a map of
contract ref → `Trait`. Modules exchange artifacts (via refs); events only
notify. Provenance is recorded separately as a `Derivation`, not on the
artifact.

Value identity:

```text
id = H( canonical trait bag )
     for each contract ref (UTF-8 order):
         contract_ref ‖ 0x00 ‖ trait_content ‖ 0x00
```

`meta`, `produced_by`, `entity_id`, and `supersedes` are **not** part of the id.

## Where the data lives

Each trait holds **exactly one** of:

| Mode | Field | Bytes live in | Hashed into artifact id |
|------|--------|---------------|-------------------------|
| **Inline** (small) | `Trait.body` | the artifact record | the body bytes |
| **External** (large) | `Trait.object` (`ObjectRef`) | the **blob store** | `object_ref_bytes` (digest · size · namespace) — **not** the raw blob |

It is **not** “on disk vs in memory.” Producers choose inline vs external by
payload size and sharing needs. The default hard max for inline is **1 MiB**
(see `ArtifactStorePolicy` below).

## External refs: copy, not original path

`PutBlob` **copies** bytes into the kernel blob store under
`(namespace, digest)`. The artifact’s `ObjectRef` points at that store — never
at the caller’s original file path or buffer.

```text
caller bytes  ──copy──►  PutBlob  ──►  blob store[(namespace, digest)]
                                              ▲
artifact.trait.object  ────── points here ────┘
```

- First write wins (same namespace + digest is a no-op).
- `GetBlob` re-hashes and verifies size.
- Physical backend (process memory today, files/Postgres later) is a
  **`KernelApi` implementation detail**, not part of value identity.

## ArtifactStorePolicy (core store law)

Frozen at kernel construction. Enforced on every `PutArtifact` / `PutBlob`.
Does **not** change identity formulas. Exposed on `RegistrySnapshot.store_policy`.

| Field | Default | Guarantee |
|-------|---------|-----------|
| `max_inline_bytes` | 1 MiB (if input `0`) | Larger `Trait.body` → `RESOURCE_EXHAUSTED` |
| `max_blob_bytes` | `0` = unlimited | Larger `PutBlob` → `RESOURCE_EXHAUSTED` |
| `ingest_mode` | `COPY_VERIFY` | Only strong mode in v2; always copy + content-address |
| `durability` | `EPHEMERAL` (`MemoryKernel`) | Declared class; durable backends report `DURABLE` |

**Not in policy:** disk paths, mount options, or “reference file in place.”
Those would weaken verification. A future weak locator would be a **new**
payload kind, not an `ObjectRef` mode.

### Construction (examples)

```rust
// Defaults
let k = MemoryKernel::new();

// Custom limits
let k = MemoryKernel::with_store_policy(ArtifactStorePolicy {
    max_inline_bytes: 64 * 1024,
    max_blob_bytes: 100 * 1024 * 1024,
    ingest_mode: BlobIngestMode::CopyVerify as i32,
    durability: StoreDurability::Ephemeral as i32,
})?;
```

```go
k := NewMemoryKernel()
k, err := NewMemoryKernelWithStorePolicy(&ArtifactStorePolicy{
    MaxInlineBytes: 64 * 1024,
    MaxBlobBytes:   100 * 1024 * 1024,
    IngestMode:     BlobIngestModeCopyVerify,
    Durability:     StoreDurabilityEphemeral,
})
```

```python
k = MemoryKernel()
k = MemoryKernel(ArtifactStorePolicy(
    max_inline_bytes=64 * 1024,
    max_blob_bytes=100 * 1024 * 1024,
    ingest_mode=BlobIngestMode.BLOB_INGEST_MODE_COPY_VERIFY,
    durability=StoreDurability.STORE_DURABILITY_EPHEMERAL,
))
```

## Production path for large evidence

```text
1. PutBlob(namespace, data)           → BlobRef (digest, byte_count, namespace)
2. PutArtifact(trait with ObjectRef)  → ArtifactRef
   (or put_artifact_with_blob helper)
```

Small typed values (hosts, ports, view payloads) stay **inline**.

## Ledger

- `artifact.put` — full artifact detail; **external** trait bodies cleared;
  `ObjectRef` retained.
- `blob.put` — `BlobRef` only (no raw bytes).
- Chain never duplicates multi‑MB content it cannot prune.

## Further reading

- Kernel contract: [`kernel/SPEC.md`](../kernel/SPEC.md) — production artifact
  boundary + ArtifactStorePolicy
- Wire types: [`kernel/contracts/proto/.../substrate.proto`](../kernel/contracts/proto/srcport/substrate/v1/substrate.proto)
- SDKs: `kernel/sdk/{rust,go,python}/`
