#!/usr/bin/env bash
# Bifaria - 1-Line Installer for macOS
# Downloads the latest notarized release and installs it to /Applications.

set -e

REPO="${BIFARIA_REPO:-eotsevych/bifaria}"
APP_NAME="Bifaria.app"
INSTALL_PATH="/Applications/$APP_NAME"
TEMP_DIR="/tmp/bifaria_install_$$"

echo "=========================================================="
echo "  Bifaria - macOS Quick Installer"
echo "=========================================================="

# Check if running on macOS
if [ "$(uname -s)" != "Darwin" ]; then
  echo "Error: This installer is only for macOS."
  exit 1
fi

# Detect CPU architecture
ARCH="$(uname -m)"
echo "--> Detected hardware: $ARCH"

mkdir -p "$TEMP_DIR"

cleanup() {
  if [ -n "$MOUNT_POINT" ] && [ -d "$MOUNT_POINT" ]; then
    hdiutil detach "$MOUNT_POINT" -force >/dev/null 2>&1 || true
  fi
  rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

# Check if local DMG exists in current directory or installers/
DMG_PATH=""
if [ -f "installers/Bifaria_Universal.dmg" ]; then
  DMG_PATH="installers/Bifaria_Universal.dmg"
elif [ -f "Bifaria_Universal.dmg" ]; then
  DMG_PATH="Bifaria_Universal.dmg"
elif [ -f "$HOME/Desktop/Bifaria_Universal.dmg" ]; then
  DMG_PATH="$HOME/Desktop/Bifaria_Universal.dmg"
fi

if [ -n "$DMG_PATH" ] && [ -f "$DMG_PATH" ]; then
  echo "--> Using local installer: $DMG_PATH"
  LOCAL_DMG="$DMG_PATH"
else
  # Download from GitHub Releases
  DMG_NAME="Bifaria_Universal.dmg"
  DOWNLOAD_URL="https://github.com/$REPO/releases/latest/download/$DMG_NAME"
  LOCAL_DMG="$TEMP_DIR/$DMG_NAME"
  
  echo "--> Downloading latest release from GitHub ($REPO)..."
  curl -fL --progress-bar "$DOWNLOAD_URL" -o "$LOCAL_DMG" || {
    echo ""
    echo "Warning: Direct release download not found at $DOWNLOAD_URL"
    echo "If you have not created a GitHub Release yet, run scripts/build_installers.sh first."
    exit 1
  }
fi

# Close existing running instance
echo "--> Closing any running instances of Bifaria..."
pkill -x bifaria >/dev/null 2>&1 || true

# Mount the disk image
echo "--> Mounting disk image..."
MOUNT_OUTPUT="$(hdiutil attach "$LOCAL_DMG" -nobrowse -readonly)"
MOUNT_POINT="$(echo "$MOUNT_OUTPUT" | grep "/Volumes/" | sed -E 's/.*(\/Volumes\/.*)/\1/' | head -n 1)"

if [ -z "$MOUNT_POINT" ] || [ ! -d "$MOUNT_POINT" ]; then
  echo "Error: Failed to mount disk image."
  exit 1
fi

# Check for app inside mounted DMG
SOURCE_APP="$MOUNT_POINT/$APP_NAME"
if [ ! -d "$SOURCE_APP" ]; then
  SOURCE_APP="$(find "$MOUNT_POINT" -maxdepth 2 -name "*.app" | head -n 1)"
fi

if [ -z "$SOURCE_APP" ] || [ ! -d "$SOURCE_APP" ]; then
  echo "Error: Application bundle not found inside disk image."
  exit 1
fi

# Copy app to /Applications
echo "--> Installing to /Applications..."
rm -rf "$INSTALL_PATH"
cp -R "$SOURCE_APP" "/Applications/"

# Unmount disk image
hdiutil detach "$MOUNT_POINT" -force >/dev/null 2>&1 || true
MOUNT_POINT=""

# Register with macOS LaunchServices
echo "--> Registering application with system..."
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$INSTALL_PATH" >/dev/null 2>&1 || true

echo ""
echo "=========================================================="
echo "✓ Bifaria installed successfully into /Applications!"
echo "=========================================================="
echo ""
echo "To start the app:"
echo "1. Open Finder -> Applications -> Bifaria"
echo "2. Grant Microphone and Accessibility permissions when asked."
echo ""
