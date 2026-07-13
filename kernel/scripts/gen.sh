#!/usr/bin/env bash
# Regenerate all kernel SDK types from contracts/proto/**/*.proto.
#
# Run from anywhere; this script cds to kernel/ (parent of scripts/).
# The Rust SDK generates at build time (build.rs); this script covers Go and
# Python, whose gencode is committed. CI runs this and fails if the working tree
# changes — that check, not good intentions, is what keeps the SDKs in lockstep
# with the contract.
#
# Requires: buf (https://buf.build). Uses PINNED remote plugins, so no local
# protoc / protoc-gen-* is needed — only network access to buf.build.
set -euo pipefail

kernel_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$kernel_root"

echo "buf lint..."
buf lint

echo "buf generate..."
buf generate

# Python protobuf output is a plain package tree with no __init__.py; add them
# so `srcport_substrate._gen.*` imports as a regular package.
gen_root="sdk/python/src/srcport_substrate/_gen"
find "$gen_root" -type d -print0 | while IFS= read -r -d '' dir; do
  touch "$dir/__init__.py"
done

echo "done. Generated code is committed; review 'git status'."
