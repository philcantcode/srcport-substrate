# srcport-substrate

The one pluggable core for every future project — frozen once, reused forever.

This repo is a **specification, not a framework**. It defines a small,
domain-neutral microkernel as a language-neutral contract; SDKs in each language
you need (Rust first) conform to it. It exists to end the pattern of re-deriving
the same substrate every time a new project starts.

- **[`SPEC.md`](SPEC.md)** — the two-page human-owned specification. Read this.
- **[`contracts/proto/srcport/substrate/v1/substrate.proto`](contracts/proto/srcport/substrate/v1/substrate.proto)** — the canonical contract (protobuf-first).
- **[`buf.yaml`](buf.yaml)** — lint + breaking-change enforcement.

## The core, in one breath

Seven primitives — **Module · Artifact · Contract · Event · Ledger · Gate ·
Registry** — plus one `Kernel` ABI. Nothing here knows about any domain. VR,
games, content, and growth are all just **Modules** built on top of this shared,
unchanging core.

## The one rule

> Do not create a new repo that re-implements this core. If it lacks something,
> **widen this contract by adding to it**, tag a new version, and let every
> project pick it up. Re-derivation is the bug this repo exists to kill.

## Status

`v0.1` draft — **unfrozen, pending review**. The Rust SDK and the `v1.0.0` freeze
come after the schema is approved. Nothing depends on it yet.

## Layout

```
srcport-substrate/
  SPEC.md                                  # the human-owned specification
  buf.yaml                                 # lint + breaking-change CI
  contracts/proto/srcport/substrate/v1/
    substrate.proto                        # THE contract
  sdk/                                     # each conforms to SPEC.md
    rust/                                  # in-process Rust SDK (types generated from the proto)
    go/                                    # in-process Go SDK (stdlib only)
    python/                                # in-process Python SDK (stdlib only)
```

Every SDK realises the same `Kernel` ABI in-process and ships the same
six-test conformance suite. As a cross-check, all three produce byte-identical
artifact addresses for the same `(type, body)` — the spec's addressing rule.

## License

Copyright (c) srcport.com. All rights reserved.
