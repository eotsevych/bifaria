#!/usr/bin/env bash
# Builds bifaria-asr, the helper that runs Parakeet on the Neural Engine (Core ML through FluidAudio),
# for Apple Silicon. Run by src-tauri/build.rs on macOS; can also be run by hand:
#   scripts/build_apple_asr.sh [output-path]
#
# Needs Xcode with Swift 6, and network access on the first build to fetch FluidAudio (pinned in
# src-tauri/apple-asr/Package.swift). arm64 only: Intel Macs have no Neural Engine, and Bifaria uses
# the helper only on Apple Silicon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PKG="$ROOT_DIR/src-tauri/apple-asr"
OUT="${1:-$ROOT_DIR/src-tauri/binaries/bifaria-asr-universal-apple-darwin}"

swift build -c release --arch arm64 --package-path "$PKG"
BIN="$(swift build -c release --arch arm64 --package-path "$PKG" --show-bin-path)/bifaria-asr"

mkdir -p "$(dirname "$OUT")"
cp "$BIN" "$OUT.tmp"
# Signed again with the app bundle; ad-hoc here so the binary runs straight from the build.
codesign --force --sign - --options runtime "$OUT.tmp"
mv "$OUT.tmp" "$OUT"
echo "Built $OUT ($(lipo -archs "$OUT"))"
