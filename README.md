# srcport-substrate

**Monorepo: kernel + optional framework.**

Two top-level products with a hard dependency boundary — the framework may
depend on the kernel; the kernel never depends on the framework.

| Product | Path | Role |
|---------|------|------|
| **Kernel** | [`kernel/`](kernel/) | Domain-neutral microkernel: contract, SDKs, conformance |
| **Framework** | [`framework/`](framework/) | Opinionated host, module plugins, UI profiles (`v0.x`) |

```text
srcport-substrate/
├─ kernel/          ← substrate SPEC · proto · SDKs · example
├─ framework/       ← host · plugins · UI profiles
├─ README.md        ← you are here
└─ LICENSE*
```

```mermaid
flowchart TB
    subgraph apps["your products"]
        P["apps / UI shells"]
    end
    subgraph fw["framework/"]
        H["Host · ModulePlugin · UI profiles"]
    end
    subgraph ker["kernel/"]
        S["SPEC + substrate.proto"]
        SDK["sdk/{rust,go,python}"]
        S --- SDK
    end
    P --> H
    P --> SDK
    H --> SDK
```

## Start here

| Goal | Go to |
|------|--------|
| Read the substrate contract | [`kernel/SPEC.md`](kernel/SPEC.md) |
| Kernel overview & diagrams | [`kernel/README.md`](kernel/README.md) |
| Wire format | [`kernel/contracts/proto/.../substrate.proto`](kernel/contracts/proto/srcport/substrate/v1/substrate.proto) |
| Framework charter | [`framework/SPEC.md`](framework/SPEC.md) |
| Framework usage | [`framework/README.md`](framework/README.md) |
| Run the kernel demo | `cd kernel/example && cargo run` |
| Test the framework | `cargo test --manifest-path framework/rust/Cargo.toml` |

## Kernel (summary)

Seven primitives (Module · Artifact · Contract · Event · Ledger · Registry ·
Run) plus one `KernelApi`. Domain-neutral. Immutable artifacts are the data
plane; assemblies are human-owned; the ledger is tamper-evident.

```bash
# regenerate Go/Python types from the proto (from monorepo root)
bash kernel/scripts/gen.sh

cargo test --manifest-path kernel/sdk/rust/Cargo.toml
cd kernel/sdk/go && go test ./...
pip install ./kernel/sdk/python && python -m unittest discover -s kernel/sdk/python/tests -v
```

## Framework (summary)

Optional application layer: `Host` drives claim → optional UI hooks → execute →
commit. Plugins implement domain work; UI is opt-in via `srcport.ui.v1` JSON
artifacts. Does **not** change `substrate.proto`.

```bash
cargo test --manifest-path framework/rust/Cargo.toml
```

## Rule

> **One canonical kernel contract, many conforming implementations.**  
> Widen the kernel by *adding* to the contract. Put product opinions in
> `framework/`, never reverse-depend into `kernel/`.

## License

Dual-licensed under MIT OR Apache-2.0. See [`LICENSE`](LICENSE),
[`LICENSE-MIT`](LICENSE-MIT), and [`LICENSE-APACHE`](LICENSE-APACHE).
