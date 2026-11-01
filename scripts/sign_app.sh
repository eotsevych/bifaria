#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# shellcheck source=lib_codesign.sh
source "$SCRIPT_DIR/lib_codesign.sh"

echo "==> Preparing and signing Bifaria macOS Universal bundle..."
bifaria_print_identity

SRC_APP="$ROOT_DIR/src-tauri/target/release/bundle/macos/Bifaria.app"
ROOT_APP="$ROOT_DIR/Bifaria.app"
VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$ROOT_DIR/src-tauri/tauri.conf.json")"
UNIVERSAL_DMG="$ROOT_DIR/Bifaria_${VERSION}_universal.dmg"
AARCH64_DMG="$ROOT_DIR/Bifaria_${VERSION}_aarch64.dmg"

ARM64_BIN="$ROOT_DIR/src-tauri/target/release/bifaria"
X86_64_BIN="$ROOT_DIR/src-tauri/target/x86_64-apple-darwin/release/bifaria"
ONNX_DIR="$ROOT_DIR/build_deps/onnxruntime-osx-x86_64-1.18.0/lib"

if [ -f "$ARM64_BIN" ] && [ -f "$X86_64_BIN" ]; then
  echo "--> Combining arm64 and x86_64 binaries into Universal Mach-O executable..."
  UNIV_BIN="/tmp/bifaria_univ_$$"
  lipo -create "$ARM64_BIN" "$X86_64_BIN" -output "$UNIV_BIN"
  chmod +x "$UNIV_BIN"
  
  # Ensure @executable_path/../Frameworks is in the rpath so x86_64 dyld can find libonnxruntime
  install_name_tool -add_rpath "@executable_path/../Frameworks" "$UNIV_BIN" 2>/dev/null || true
  
  # Install into SRC_APP
  cp "$UNIV_BIN" "$SRC_APP/Contents/MacOS/bifaria"
  rm -f "$UNIV_BIN"
  
  # Copy Frameworks
  mkdir -p "$SRC_APP/Contents/Frameworks"
  if [ -d "$ONNX_DIR" ]; then
    cp "$ONNX_DIR"/libonnxruntime*.dylib "$SRC_APP/Contents/Frameworks/"
  fi
  echo "✓ Universal binary and Frameworks installed into bundle."
fi

sign_app() {
  local target="$1"
  if [ -d "$target" ]; then
    echo "Signing: $target"
    bifaria_sign_app "$target"
    codesign -d -r- "$target" 2>&1 | grep "designated =>"
    echo "✓ Signed: $target"
  fi
}

sign_app "$SRC_APP"

# Synchronize root Bifaria.app
if [ -d "$SRC_APP" ]; then
  echo "Updating root Bifaria.app..."
  rm -rf "$ROOT_APP"
  cp -R "$SRC_APP" "$ROOT_APP"
  sign_app "$ROOT_APP"
fi

# Package fresh signed Universal DMG
if [ -d "$ROOT_APP" ]; then
  echo "Packaging signed Universal DMG..."
  STAGE_DIR="/tmp/bifaria_dmg_pack_$$"
  rm -rf "$STAGE_DIR"
  mkdir -p "$STAGE_DIR"
  cp -R "$ROOT_APP" "$STAGE_DIR/"
  ln -s /Applications "$STAGE_DIR/Applications"
  
  rm -f "$UNIVERSAL_DMG" "$AARCH64_DMG"
  hdiutil create -volname "Bifaria" -srcfolder "$STAGE_DIR" -ov -format UDZO "$UNIVERSAL_DMG" > /dev/null
  rm -rf "$STAGE_DIR"
  bifaria_sign_dmg "$UNIVERSAL_DMG"
  
  # Also copy to AARCH64_DMG for backwards compatibility
  cp "$UNIVERSAL_DMG" "$AARCH64_DMG"
  echo "✓ Signed Universal DMG ready at: $UNIVERSAL_DMG"
  echo "✓ Signed DMG ready at: $AARCH64_DMG"
fi

# Installing is left to the user (or scripts/clean_and_deploy_desktop.sh for a full reset):
# resetting permissions here would undo the point of signing with a stable identity.

echo "==> All bundles signed and verified successfully."
