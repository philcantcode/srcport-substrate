# `kernel/example/` — a real Run, visualised from the ledger alone

A runnable example on the [Rust SDK](../sdk/rust). It builds a tiny domain on
the kernel, drives a **real** convergent Run, and then reconstructs the entire
dataflow **solely by decoding the append-only ledger** — proving the spec's
central claim rather than illustrating it.

```
cd kernel/example && cargo run          # standalone crate — depends on ../sdk/rust by path
```

It prints a live terminal trace and writes a self-contained
[`flow.html`](./flow.html) beside itself (no external assets, opens offline).

## What it shows

Three Modules with typed capability ports form a **diamond fan-in**:

```
                 ┌─▶ extract  (question ─▶ facts) ──┐
  question ──────┤                                  ├──▶ write ─▶ answer
                 └─▶ retrieve (question ─▶ sources)─┘
```

The example walks the seven primitives end to end:

1. **Module** — `extractor`, `retriever`, `writer` register with typed
   input/output ports; they couple only through **Contract** refs, never to each
   other.
2. **Assembly / Run** — a human-pinned, version-frozen, acyclic assembly names
   one terminal output. You can watch the `writer` node stay **unclaimable**
   until every one of its typed inputs exists — feed-forward convergence, made
   observable.
3. **Artifact** — each step produces an immutable, content-addressed artifact;
   its ref (never its bytes) is what flows along a binding.
4. **Ledger** — every action lands one hash-chained entry.

Then the payoff: the program throws away every in-memory handle, verifies
`k.ledger()`, and **rebuilds the whole graph by decoding the chain** — each
entry's `detail` back into the `substrate.proto` message named for its `kind`
(see [`SPEC.md`](../SPEC.md) § *Ledger detail*). The terminal diagram and
`flow.html` are drawn from that reconstruction, not from the code that produced
the run. That it reconstructs at all is invariant #7 in the conformance suite.

## Layout

| file | role |
|------|------|
| [`src/main.rs`](src/main.rs) | builds the domain, drives the Run, narrates the flow |
| [`src/reconstruct.rs`](src/reconstruct.rs) | decodes a ledger chain back into a dataflow graph |
| [`src/html.rs`](src/html.rs) | renders that graph as a self-contained SVG + ledger page |

The example depends on the SDK **by path, through its public ABI only** — it
imports nothing private. If it compiles, the published surface is enough to
build a domain on the kernel.
