#!/usr/bin/env bash
# Builds latest.json for the in-app updater from the signed assets of a GitHub release
# and uploads it to that release. Run by CI after all platform builds; can also be run by hand:
#   bash scripts/make_update_manifest.sh v0.1.0
#
# The app reads https://github.com/<repo>/releases/latest/download/latest.json, so users are
# only offered an update once the (draft) release is published.
set -euo pipefail

TAG="${1:?usage: make_update_manifest.sh <tag, e.g. v0.1.0>}"
REPO="${GITHUB_REPOSITORY:-eotsevych/bifaria}"
VERSION="${TAG#v}"
WORK="$(mktemp -d)"
cleanup() {
  rm -f "$WORK"/*.sig "$WORK/latest.json"
  rmdir "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

ASSETS="$(gh release view "$TAG" --repo "$REPO" --json assets --jq '.assets[].name')"

# Prints the first release asset matching a pattern that also has a .sig next to it.
find_signed_asset() {
  local pattern="$1" name
  while IFS= read -r name; do
    # shellcheck disable=SC2053
    if [[ "$name" == $pattern ]] && grep -qxF "$name.sig" <<<"$ASSETS"; then
      echo "$name"
      return 0
    fi
  done <<<"$ASSETS"
  return 0
}

PLATFORMS='{}'
add_platform() {
  local platform="$1" asset="$2"
  gh release download "$TAG" --repo "$REPO" --pattern "$asset.sig" --dir "$WORK" --clobber
  PLATFORMS="$(jq -c \
    --arg p "$platform" \
    --arg url "https://github.com/$REPO/releases/download/$TAG/$asset" \
    --rawfile sig "$WORK/$asset.sig" \
    '. + {($p): {url: $url, signature: $sig}}' <<<"$PLATFORMS")"
  echo "  $platform -> $asset"
}

echo "--> Collecting signed updater assets for $TAG..."
MAC="$(find_signed_asset 'Bifaria_Universal.app.tar.gz')"
WIN="$(find_signed_asset '*_x64-setup.exe')"
LINUX="$(find_signed_asset '*_amd64.AppImage')"

# The universal archive serves both Apple Silicon and Intel.
if [ -n "$MAC" ]; then
  add_platform darwin-aarch64 "$MAC"
  add_platform darwin-x86_64 "$MAC"
fi
if [ -n "$WIN" ]; then
  add_platform windows-x86_64 "$WIN"
fi
if [ -n "$LINUX" ]; then
  add_platform linux-x86_64 "$LINUX"
fi

if [ "$PLATFORMS" = '{}' ]; then
  echo "No signed updater assets found (is the TAURI_SIGNING_PRIVATE_KEY secret set?). Skipping latest.json."
  exit 0
fi

jq -n \
  --arg version "$VERSION" \
  --arg notes "See https://github.com/$REPO/releases/tag/$TAG" \
  --arg pub_date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson platforms "$PLATFORMS" \
  '{version: $version, notes: $notes, pub_date: $pub_date, platforms: $platforms}' > "$WORK/latest.json"

gh release upload "$TAG" --repo "$REPO" "$WORK/latest.json" --clobber
echo "✓ Uploaded latest.json to $TAG"
