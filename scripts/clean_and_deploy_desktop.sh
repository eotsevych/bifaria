#!/usr/bin/env bash
# Developer reset: removes the installed app, its permissions and local data,
# then rebuilds all macOS installers and installs a fresh copy to /Applications.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=========================================================="
echo "  Bifaria - Clean Rebuild & Desktop Deploy Script"
echo "=========================================================="

# 1. Stop running instances (match the process name exactly so this script is not killed)
echo "--> 1. Stopping any running Bifaria instances..."
pkill -9 -x bifaria 2>/dev/null || true
sleep 1

# 2. Delete old installed app
echo "--> 2. Deleting old app from /Applications..."
rm -rf "/Applications/Bifaria.app"

# 3. Reset macOS permissions (Accessibility & Microphone)
echo "--> 3. Resetting macOS permissions..."
tccutil reset All com.bifaria.desktop 2>/dev/null || true

# 4. Clear application data, caches, and preferences for clean install
echo "--> 4. Clearing previous app settings and databases..."
rm -rf "$HOME/Library/Application Support/bifaria"
rm -rf "$HOME/Library/Application Support/com.bifaria.desktop"
rm -rf "$HOME/Library/Caches/com.bifaria.desktop"
rm -rf "$HOME/Library/WebKit/com.bifaria.desktop"
rm -rf "$HOME/Library/Preferences/com.bifaria.desktop.plist"
rm -rf "$HOME/Library/Saved Application State/com.bifaria.desktop.savedState"

# 5. Rebuild installers and install to /Applications
exec "$SCRIPT_DIR/build_installers.sh" --install
