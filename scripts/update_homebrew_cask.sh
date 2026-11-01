#!/usr/bin/env bash
# Refreshes version and sha256 in packaging/homebrew/Casks/bifaria.rb from a published GitHub release.
#   bash scripts/update_homebrew_cask.sh 0.1.0                   # update the cask in this repo
#   bash scripts/update_homebrew_cask.sh 0.1.0 ../homebrew-tap   # ...and copy it into a tap checkout
set -euo pipefail

VERSION="${1:?usage: update_homebrew_cask.sh <version, e.g. 0.1.0> [path to homebrew-tap checkout]}"
VERSION="${VERSION#v}"
TAP_DIR="${2:-}"
REPO="${BIFARIA_REPO:-eotsevych/bifaria}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CASK="$SCRIPT_DIR/../packaging/homebrew/Casks/bifaria.rb"
URL="https://github.com/$REPO/releases/download/v$VERSION/Bifaria_Universal.dmg"
DMG="$(mktemp -t bifaria-cask)"
trap 'rm -f "$DMG"' EXIT

echo "--> Downloading $URL..."
curl -fL --progress-bar "$URL" -o "$DMG"
SHA="$(shasum -a 256 "$DMG" | awk '{print $1}')"

sed -i '' -E \
  -e "s/^  version \".*\"/  version \"$VERSION\"/" \
  -e "s/^  sha256 \".*\"/  sha256 \"$SHA\"/" \
  "$CASK"
echo "✓ Cask updated: version $VERSION, sha256 $SHA"

if [ -n "$TAP_DIR" ]; then
  mkdir -p "$TAP_DIR/Casks"
  cp "$CASK" "$TAP_DIR/Casks/bifaria.rb"
  echo "✓ Copied to $TAP_DIR/Casks/bifaria.rb. Commit and push the tap to publish."
fi
