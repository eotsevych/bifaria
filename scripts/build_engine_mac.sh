#!/usr/bin/env bash
# Builds bifaria-engine (src-tauri/engine: transcribe-cpp) for Macs with Apple silicon: GGUF models
# on the GPU (Metal) or CPU. Run by src-tauri/build.rs on macOS; can also be run by hand:
#   scripts/build_engine_mac.sh [output-path]
#
# Needs CMake and network access on the first build (crates.io). arm64 only, like bifaria-asr:
# Bifaria uses it only on Apple silicon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ENGINE="$ROOT_DIR/src-tauri/engine"
OUT="${1:-$ROOT_DIR/src-tauri/binaries/bifaria-engine-universal-apple-darwin}"
TARGET_DIR="$ENGINE/target"

# Started from Bifaria's own build script: drop the outer cargo's settings so this is a build of its own.
unset CARGO_TARGET_DIR RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_MAKEFLAGS MAKEFLAGS MFLAGS
MACOSX_DEPLOYMENT_TARGET=11.0 "${CARGO:-cargo}" build --release --manifest-path "$ENGINE/Cargo.toml" \
  --target aarch64-apple-darwin --target-dir "$TARGET_DIR"

mkdir -p "$(dirname "$OUT")"
cp "$TARGET_DIR/aarch64-apple-darwin/release/bifaria-engine" "$OUT.tmp"
# Signed again with the app bundle; ad-hoc here so the binary runs straight from the build.
codesign --force --sign - --options runtime "$OUT.tmp"
mv "$OUT.tmp" "$OUT"
echo "Built $OUT ($(lipo -archs "$OUT"))"
