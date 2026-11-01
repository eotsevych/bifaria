#!/usr/bin/env bash
# Shared macOS code-signing helpers. Source this file; do not execute it.
#
# Signing identity, in order of preference:
#   1. BIFARIA_SIGN_IDENTITY (certificate name or SHA-1), optionally with BIFARIA_SIGN_KEYCHAIN.
#   2. A "Developer ID Application" certificate, if installed: the only one Gatekeeper trusts.
#   3. The "Bifaria Self-Signed" certificate from scripts/create_macos_signing_cert.sh, if installed.
#   4. Ad-hoc ("-"): builds still work, but macOS asks for Microphone/Accessibility again after every update.
#
# A stable identity (1-3) keeps permissions across updates because macOS pins them to the certificate.
# Only a Developer ID build, notarized with bifaria_notarize, opens without the Gatekeeper warning.

BIFARIA_SELF_SIGNED_NAME="Bifaria Self-Signed"
BIFARIA_BUNDLE_ID="com.bifaria.desktop"
BIFARIA_ENTITLEMENTS="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/src-tauri/Entitlements.plist"

bifaria_resolve_identity() {
  if [ -n "${BIFARIA_SIGN_IDENTITY:-}" ]; then
    echo "$BIFARIA_SIGN_IDENTITY"
    return
  fi
  local identities sha
  # No -v: a self-signed certificate is "not trusted", and -v would hide it.
  identities="$(security find-identity -p codesigning ${BIFARIA_SIGN_KEYCHAIN:+"$BIFARIA_SIGN_KEYCHAIN"} 2>/dev/null)"
  sha="$(awk '/"Developer ID Application: / {print $2; exit}' <<<"$identities")"
  [ -n "$sha" ] || sha="$(awk -v name="\"$BIFARIA_SELF_SIGNED_NAME\"" 'index($0, name) {print $2; exit}' <<<"$identities")"
  echo "${sha:--}"
}

BIFARIA_IDENTITY="$(bifaria_resolve_identity)"
BIFARIA_DEVELOPER_ID=0
if [ "$BIFARIA_IDENTITY" != "-" ] && security find-identity -p codesigning ${BIFARIA_SIGN_KEYCHAIN:+"$BIFARIA_SIGN_KEYCHAIN"} 2>/dev/null \
  | grep -F "$BIFARIA_IDENTITY" | grep -q '"Developer ID Application: '; then
  BIFARIA_DEVELOPER_ID=1
fi

bifaria_codesign() {
  # Notarization requires the hardened runtime and a secure timestamp on every binary.
  local extra=()
  [ "$BIFARIA_DEVELOPER_ID" = 1 ] && extra=(--options runtime --timestamp)
  codesign --force --sign "$BIFARIA_IDENTITY" ${BIFARIA_SIGN_KEYCHAIN:+--keychain "$BIFARIA_SIGN_KEYCHAIN"} "${extra[@]}" "$@"
}

bifaria_print_identity() {
  if [ "$BIFARIA_IDENTITY" = "-" ]; then
    echo "--> Signing ad-hoc (no stable identity found; permissions will reset on update)."
    echo "    Run scripts/create_macos_signing_cert.sh once to fix this."
  elif [ "$BIFARIA_DEVELOPER_ID" = 1 ]; then
    echo "--> Signing with Developer ID: $BIFARIA_IDENTITY"
  else
    echo "--> Signing with stable identity: $BIFARIA_IDENTITY"
  fi
}

# Sends a signed .app or .dmg to Apple's notary service and staples the ticket to it, so it opens
# without a Gatekeeper warning, even offline. Needs a Developer ID identity plus an App Store
# Connect API key: APPLE_API_KEY (the .p8 file's contents), APPLE_API_KEY_ID and APPLE_API_ISSUER;
# skipped otherwise.
bifaria_notarize() {
  local target="$1"
  if [ "$BIFARIA_DEVELOPER_ID" != 1 ] || [ -z "${APPLE_API_KEY:-}" ] || [ -z "${APPLE_API_KEY_ID:-}" ] || [ -z "${APPLE_API_ISSUER:-}" ]; then
    echo "--> Skipping notarization of $(basename "$target") (needs Developer ID + APPLE_API_KEY, APPLE_API_KEY_ID, APPLE_API_ISSUER)."
    return 0
  fi
  local key_file
  key_file="$(mktemp)"
  printf '%s\n' "$APPLE_API_KEY" > "$key_file"
  local upload="$target"
  if [ -d "$target" ]; then
    # The notary service takes a zip, not a bare .app.
    upload="$(mktemp -d)/$(basename "$target").zip"
    ditto -c -k --keepParent "$target" "$upload"
  fi
  local auth=(--key "$key_file" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER")
  echo "--> Notarizing $(basename "$target") (usually a few minutes)..."
  local out
  out="$(xcrun notarytool submit "$upload" "${auth[@]}" --wait 2>&1)" || true
  echo "$out"
  if ! grep -q "status: Accepted" <<<"$out"; then
    local id
    id="$(awk '$1 == "id:" {print $2; exit}' <<<"$out")"
    [ -n "$id" ] && xcrun notarytool log "$id" "${auth[@]}"
    rm -f "$key_file"
    echo "❌ Notarization failed for $(basename "$target")" >&2
    return 1
  fi
  rm -f "$key_file"
  xcrun stapler staple "$target"
}

# Signs bundled dylibs first, then the app bundle itself.
bifaria_sign_app() {
  local app="$1"
  [ -d "$app" ] || return 0
  xattr -cr "$app" || true

  if [ -d "$app/Contents/Frameworks" ]; then
    for f in "$app/Contents/Frameworks"/*.dylib; do
      if [ -f "$f" ]; then
        bifaria_codesign "$f"
      fi
    done
  fi

  bifaria_sign_bundle --deep "$app"

  # The helpers (Apple Translation, Neural Engine speech recognition): --deep above gave them the
  # app's identifier and no hardened runtime. Sign each on its own, then seal the bundle again.
  local helper resigned=""
  for helper in translate asr; do
    if [ -f "$app/Contents/MacOS/bifaria-$helper" ]; then
      bifaria_codesign --options runtime --identifier "$BIFARIA_BUNDLE_ID.$helper" "$app/Contents/MacOS/bifaria-$helper"
      resigned=1
    fi
  done
  if [ -n "$resigned" ]; then
    bifaria_sign_bundle "$app"
  fi
}

# Signs the app bundle itself; extra arguments (e.g. --deep) go before the path.
bifaria_sign_bundle() {
  if [ "$BIFARIA_IDENTITY" = "-" ]; then
    # Ad-hoc signatures change every build; an identifier-only requirement is the best that is possible.
    bifaria_codesign "${@:1:$#-1}" --identifier "$BIFARIA_BUNDLE_ID" --entitlements "$BIFARIA_ENTITLEMENTS" \
      -r="designated => identifier \"$BIFARIA_BUNDLE_ID\"" "${@: -1}"
  else
    # Default designated requirement pins the certificate, so permissions survive updates.
    bifaria_codesign "${@:1:$#-1}" --identifier "$BIFARIA_BUNDLE_ID" --entitlements "$BIFARIA_ENTITLEMENTS" "${@: -1}"
  fi
}

bifaria_sign_dmg() {
  local dmg="$1"
  xattr -cr "$dmg" || true
  bifaria_codesign "$dmg"
}
