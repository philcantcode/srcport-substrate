//! Compile the canonical contract into Rust types at build time.
//!
//! We do NOT hand-write the wire messages: `substrate.proto` is the single
//! source of truth, so the SDK's types are generated from it directly. We use
//! `protox` (a pure-Rust protobuf compiler) instead of `prost-build`'s default
//! path so that building this crate does not require a `protoc` binary on the
//! machine.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Repo layout: kernel/sdk/rust/build.rs  ->  ../../contracts/proto
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest_dir.join("../../contracts/proto");
    let proto_file = proto_root.join("srcport/substrate/v1/substrate.proto");

    println!("cargo:rerun-if-changed={}", proto_file.display());

    // Parse + resolve the proto entirely in Rust into a FileDescriptorSet...
    let fds = protox::compile([&proto_file], [&proto_root])?;

    // ...then let prost generate the Rust types from the descriptors. No protoc.
    let mut config = prost_build::Config::new();
    // Generate every `map<>` field as a BTreeMap, not a HashMap, so protobuf
    // encoding is deterministic (entries in sorted-key order). Ledger `detail`
    // is folded into the entry hash, so its encoding MUST be canonical across
    // SDKs — see SPEC.md "Ledger detail". `.` matches all paths, now and future.
    config.btree_map(["."]);
    config.compile_fds(fds)?;

    Ok(())
}
