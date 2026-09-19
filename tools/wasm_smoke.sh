#!/usr/bin/env bash
# Builds the musicpack-wasm binding for Node and runs the smoke test.
#
# Requirements: Rust with the wasm32-unknown-unknown target and a
# wasm-bindgen CLI whose version matches the crate's `wasm-bindgen`
# dependency (currently 0.2.128):
#
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.128
#
# No browser and no wasm-pack are required.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

wasm-bindgen --version >/dev/null

cargo build -p musicpack-wasm --target wasm32-unknown-unknown --release
wasm-bindgen \
  --target nodejs \
  --out-dir target/wasm-node \
  target/wasm32-unknown-unknown/release/musicpack_wasm.wasm

node crates/musicpack-wasm/tests/node_smoke.mjs
