#!/usr/bin/env bash
# Builds the Windows/Linux speech helper (src-tauri/engine: transcribe-cpp, Vulkan + CPU) and puts
# it with its libraries in src-tauri/engine-bundle, which tauri.{windows,linux}.conf.json ship as
# resources/engine. Needs the Vulkan SDK (Windows) or libvulkan-dev and glslc (Linux).
# Usage: scripts/build_engine.sh   (ENGINE_TARGET_DIR overrides cargo's build folder)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
engine="$root/src-tauri/engine"
out="$root/src-tauri/engine-bundle"
target="${ENGINE_TARGET_DIR:-$engine/target}"

cargo test --release --manifest-path "$engine/Cargo.toml" --target-dir "$target"
cargo build --release --manifest-path "$engine/Cargo.toml" --target-dir "$target"

exe=bifaria-asr
[[ "${OS:-}" == Windows_NT ]] && exe=bifaria-asr.exe
rm -rf "$out"
mkdir -p "$out"
cp "$target/release/$exe" "$out/"
cp "$engine/runtime-libs/"* "$out/"
ls "$out" | grep -q 'ggml-vulkan' || { echo "no ggml-vulkan module was built" >&2; exit 1; }
ls "$out" | grep -q 'ggml-cpu' || { echo "no ggml-cpu modules were built" >&2; exit 1; }
ls -la "$out"
