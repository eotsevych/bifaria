#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
INSTALLERS_DIR="$ROOT_DIR/installers"
DESKTOP_DIR="$HOME/Desktop"

# shellcheck source=lib_codesign.sh
source "$SCRIPT_DIR/lib_codesign.sh"

echo "=========================================================="
echo "  Bifaria - Rebuild All Installer Packages"
echo "=========================================================="
bifaria_print_identity

# Ensure installers and desktop directories exist
mkdir -p "$INSTALLERS_DIR" "$DESKTOP_DIR"

# Detach any mounted Bifaria volumes to avoid hdiutil locks
for vol in "/Volumes/Bifaria"*; do
  if [ -d "$vol" ]; then
    hdiutil detach "$vol" -force 2>/dev/null || true
  fi
done

# Check or download ONNX runtime for x86_64 if missing
ONNX_DIR="$ROOT_DIR/build_deps/onnxruntime-osx-x86_64-1.18.0/lib"
if [ ! -d "$ONNX_DIR" ]; then
  echo "--> Downloading ONNX Runtime for x86_64..."
  mkdir -p "$ROOT_DIR/build_deps"
  cd "$ROOT_DIR/build_deps"
  curl -L -O https://github.com/microsoft/onnxruntime/releases/download/v1.18.0/onnxruntime-osx-x86_64-1.18.0.tgz
  tar -xzf onnxruntime-osx-x86_64-1.18.0.tgz
fi

# 1. Build frontend
echo "--> 1. Building frontend assets..."
cd "$ROOT_DIR"
bun run build

# 2. Build arm64 release bundle (Apple Silicon). DMGs are assembled below, so only the .app is needed.
echo "--> 2. Building Apple Silicon (arm64) release bundle..."
cd "$ROOT_DIR"
bun run tauri build --bundles app

BASE_APP="$ROOT_DIR/src-tauri/target/release/bundle/macos/Bifaria.app"
ARM64_BIN="$ROOT_DIR/src-tauri/target/release/bifaria"

if [ ! -f "$ARM64_BIN" ] || [ ! -d "$BASE_APP" ]; then
  echo "Error: Apple Silicon build did not produce expected output at $BASE_APP"
  exit 1
fi

# 3. Build x86_64 release binary (Intel)
echo "--> 3. Building Intel (x86_64) release binary..."
cd "$ROOT_DIR/src-tauri"
MACOSX_DEPLOYMENT_TARGET=11.0 \
ORT_LIB_LOCATION="$ONNX_DIR" \
ORT_PREFER_DYNAMIC_LINK=1 \
cargo build --release --target x86_64-apple-darwin

X86_64_BIN="$ROOT_DIR/src-tauri/target/x86_64-apple-darwin/release/bifaria"
if [ ! -f "$X86_64_BIN" ]; then
  echo "Error: Intel build did not produce expected output at $X86_64_BIN"
  exit 1
fi

sign_app_bundle() {
  bifaria_sign_app "$1"
}

# In-app updater archive (.app.tar.gz + .sig). Needs the updater private key, see DEPLOYMENT.md.
make_updater_archive() {
  local app_source="$1"
  local archive_name="$2"
  if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] && [ -z "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]; then
    echo "--> Skipping updater archive (TAURI_SIGNING_PRIVATE_KEY not set)."
    return
  fi
  echo "--> Packaging updater archive $archive_name..."
  local out="$INSTALLERS_DIR/$archive_name"
  rm -f "$out" "$out.sig"
  COPYFILE_DISABLE=1 tar -czf "$out" -C "$(dirname "$app_source")" "$(basename "$app_source")"
  # The password variable must exist (even empty), otherwise the CLI tries to prompt for it.
  (cd "$ROOT_DIR" && TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
    bun run tauri signer sign "$out" >/dev/null)
  echo "✓ Created $archive_name (+ .sig)"
}

make_dmg() {
  local app_source="$1"
  local dmg_name="$2"
  local vol_name="$3"

  echo "--> Packaging $dmg_name..."
  local stage_dir="/tmp/stage_dmg_$$"
  rm -rf "$stage_dir"
  mkdir -p "$stage_dir"
  cp -R "$app_source" "$stage_dir/"

  if [ ! -d "$stage_dir/Bifaria.app" ]; then
    for d in "$stage_dir"/*; do
      if [ -d "$d" ] && [ "$(basename "$d")" != "Applications" ]; then
        mv "$d" "$stage_dir/Bifaria.app"
        break
      fi
    done
  fi

  ln -s /Applications "$stage_dir/Applications"

  local out_installers="$INSTALLERS_DIR/$dmg_name"
  local out_desktop="$DESKTOP_DIR/$dmg_name"

  rm -f "$out_installers" "$out_desktop"
  # hdiutil intermittently fails with "Resource busy" (e.g. on CI runners), so retry a few times.
  local attempt
  for attempt in 1 2 3 4 5; do
    if hdiutil create -volname "$vol_name" -srcfolder "$stage_dir" -ov -format UDZO "$out_installers" > /dev/null; then
      break
    fi
    if [ "$attempt" -eq 5 ]; then
      echo "❌ hdiutil create failed for $dmg_name after $attempt attempts" >&2
      exit 1
    fi
    echo "⚠️  hdiutil create failed for $dmg_name (attempt $attempt), retrying..." >&2
    sleep $((attempt * 5))
  done
  rm -rf "$stage_dir"

  bifaria_sign_dmg "$out_installers"
  bifaria_notarize "$out_installers"

  # Copy to Desktop for immediate access
  cp "$out_installers" "$out_desktop"
  xattr -cr "$out_desktop" || true
  echo "✓ Created $dmg_name"
}

copy_resources() {
  local target_app="$1"
  mkdir -p "$target_app/Contents/Resources"
  if [ -f "$ROOT_DIR/src-tauri/icons/Assets.car" ]; then
    cp "$ROOT_DIR/src-tauri/icons/Assets.car" "$target_app/Contents/Resources/"
  fi
  if [ -f "$ROOT_DIR/src-tauri/icons/AppIcon.icns" ]; then
    cp "$ROOT_DIR/src-tauri/icons/AppIcon.icns" "$target_app/Contents/Resources/"
  fi
  if [ -f "$ROOT_DIR/src-tauri/icons/icon.icns" ]; then
    cp "$ROOT_DIR/src-tauri/icons/icon.icns" "$target_app/Contents/Resources/"
  fi
  if [ -f "$target_app/Contents/Info.plist" ]; then
    plutil -remove LSRequiresCarbon "$target_app/Contents/Info.plist" 2>/dev/null || true
    plutil -remove CFBundleIconName "$target_app/Contents/Info.plist" 2>/dev/null || true
    plutil -replace CFBundleIconFile -string "icon.icns" "$target_app/Contents/Info.plist" 2>/dev/null || true
  fi
  rm -f "$target_app/Contents/Resources/Assets.car"
}

# A. Universal Installer (arm64 + x86_64)
echo "--> 4. Creating Universal 2 (Intel + Apple Silicon) installer..."
UNIV_DIR="/tmp/bifaria_univ_$$"
UNIV_APP="$UNIV_DIR/Bifaria.app"
rm -rf "$UNIV_DIR"
mkdir -p "$UNIV_DIR"
cp -R "$BASE_APP" "$UNIV_APP"
mkdir -p "$UNIV_APP/Contents/Frameworks"
if [ -d "$ONNX_DIR" ]; then
  cp "$ONNX_DIR"/libonnxruntime*.dylib "$UNIV_APP/Contents/Frameworks/"
fi
copy_resources "$UNIV_APP"
UNIV_BIN="/tmp/bifaria_univ_bin_$$"
lipo -create "$ARM64_BIN" "$X86_64_BIN" -output "$UNIV_BIN"
chmod +x "$UNIV_BIN"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$UNIV_BIN" 2>/dev/null || true
cp "$UNIV_BIN" "$UNIV_APP/Contents/MacOS/bifaria"
rm -f "$UNIV_BIN"
sign_app_bundle "$UNIV_APP"
bifaria_notarize "$UNIV_APP"

# Update root app bundle
rm -rf "$ROOT_DIR/Bifaria.app"
cp -R "$UNIV_APP" "$ROOT_DIR/Bifaria.app"

# If --install passed, install to /Applications
if [ "$1" = "--install" ]; then
  echo "--> Installing to /Applications/Bifaria.app..."
  rm -rf "/Applications/Bifaria.app"
  cp -R "$UNIV_APP" "/Applications/Bifaria.app"
  sign_app_bundle "/Applications/Bifaria.app"
  touch "/Applications/Bifaria.app"
  touch "/Applications/Bifaria.app/Contents/Info.plist"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f -r "/Applications/Bifaria.app" 2>/dev/null || true
  rm -rf /private/var/folders/*/*/*/com.apple.iconservices* 2>/dev/null || true
  rm -rf /private/var/folders/*/*/*/com.apple.dock.iconcache* 2>/dev/null || true
  killall Finder Dock 2>/dev/null || true
  echo "✓ Installed to /Applications"
fi

make_dmg "$UNIV_APP" "Bifaria_Universal.dmg" "Bifaria Universal"
make_updater_archive "$UNIV_APP" "Bifaria_Universal.app.tar.gz"
rm -rf "$UNIV_DIR"

# B. Intel Only Installer (x86_64)
echo "--> 5. Creating Intel (x86_64) installer..."
INTEL_DIR="/tmp/bifaria_intel_$$"
INTEL_APP="$INTEL_DIR/Bifaria.app"
rm -rf "$INTEL_DIR"
mkdir -p "$INTEL_DIR"
cp -R "$BASE_APP" "$INTEL_APP"
mkdir -p "$INTEL_APP/Contents/Frameworks"
if [ -d "$ONNX_DIR" ]; then
  cp "$ONNX_DIR"/libonnxruntime*.dylib "$INTEL_APP/Contents/Frameworks/"
fi
cp "$X86_64_BIN" "$INTEL_APP/Contents/MacOS/bifaria"
chmod +x "$INTEL_APP/Contents/MacOS/bifaria"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$INTEL_APP/Contents/MacOS/bifaria" 2>/dev/null || true
copy_resources "$INTEL_APP"
sign_app_bundle "$INTEL_APP"
bifaria_notarize "$INTEL_APP"
make_dmg "$INTEL_APP" "Bifaria_Intel_x86_64.dmg" "Bifaria Intel"
rm -rf "$INTEL_DIR"

# C. Apple Silicon Only Installer (arm64)
echo "--> 6. Creating Apple Silicon (arm64) installer..."
ARM_DIR="/tmp/bifaria_arm_$$"
ARM_APP="$ARM_DIR/Bifaria.app"
rm -rf "$ARM_DIR"
mkdir -p "$ARM_DIR"
cp -R "$BASE_APP" "$ARM_APP"
rm -rf "$ARM_APP/Contents/Frameworks"
cp "$ARM64_BIN" "$ARM_APP/Contents/MacOS/bifaria"
chmod +x "$ARM_APP/Contents/MacOS/bifaria"
copy_resources "$ARM_APP"
sign_app_bundle "$ARM_APP"
bifaria_notarize "$ARM_APP"
make_dmg "$ARM_APP" "Bifaria_Apple_Silicon_arm64.dmg" "Bifaria Apple Silicon"
rm -rf "$ARM_DIR"

echo ""
echo "=========================================================="
echo "✓ All installer apps rebuilt successfully!"
echo "✓ Location: $INSTALLERS_DIR"
echo "   - Bifaria_Universal.dmg (Intel + Apple Silicon)"
echo "   - Bifaria_Apple_Silicon_arm64.dmg"
echo "   - Bifaria_Intel_x86_64.dmg"
echo "✓ Also copied to: $DESKTOP_DIR"
echo "=========================================================="
