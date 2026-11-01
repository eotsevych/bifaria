#!/usr/bin/env bash
# One-time setup: creates a free self-signed code-signing certificate for Bifaria.
#
# Why: ad-hoc signatures change on every build, so macOS forgets the Microphone and
# Accessibility permissions after each update. Signing every build with the same
# certificate keeps them. (This does not remove the Gatekeeper warning; see INSTALL.md.)
#
# Output (kept outside the repo, in ~/.bifaria-signing/):
#   bifaria-signing.p12      certificate + private key; back it up, every future build must use it
#   bifaria-signing.p12.b64  paste into the GitHub secret MACOS_SIGNING_P12
#   p12-password.txt        paste into the GitHub secret MACOS_SIGNING_P12_PASSWORD
#
# Set KEYCHAIN to import somewhere other than the login keychain.
set -euo pipefail

CERT_NAME="Bifaria Self-Signed"
OUT_DIR="${BIFARIA_SIGNING_DIR:-$HOME/.bifaria-signing}"
KEYCHAIN="${KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"

if security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "\"$CERT_NAME\""; then
  echo "\"$CERT_NAME\" is already in $KEYCHAIN. Nothing to do."
  echo "To start over, delete it in Keychain Access first."
  exit 0
fi

if [ -e "$OUT_DIR/bifaria-signing.p12" ]; then
  echo "Error: $OUT_DIR/bifaria-signing.p12 already exists but is not in the keychain."
  echo "Import it instead of creating a new one (a new certificate resets users' permissions once):"
  echo "  security import \"$OUT_DIR/bifaria-signing.p12\" -k \"$KEYCHAIN\" -P \"\$(cat \"$OUT_DIR/p12-password.txt\")\" -T /usr/bin/codesign"
  exit 1
fi

mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR"
WORK="$(mktemp -d)"
cleanup() {
  rm -f "$WORK/key.pem" "$WORK/cert.pem" "$WORK/cert.cnf"
  rmdir "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

cat > "$WORK/cert.cnf" <<EOF
[req]
distinguished_name = dn
prompt = no
x509_extensions = v3
[dn]
CN = $CERT_NAME
[v3]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

echo "--> Creating certificate \"$CERT_NAME\" (valid 20 years)..."
openssl req -x509 -newkey rsa:2048 -nodes -days 7300 \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/cert.cnf" 2>/dev/null

PASSWORD="$(openssl rand -hex 24)"
# -legacy: macOS `security` cannot read the OpenSSL 3 default PKCS#12 encryption.
# LibreSSL (the macOS system openssl) has no -legacy flag and already writes the old format.
if ! openssl pkcs12 -export -legacy -name "$CERT_NAME" \
  -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -out "$OUT_DIR/bifaria-signing.p12" -passout "pass:$PASSWORD" 2>/dev/null; then
  openssl pkcs12 -export -name "$CERT_NAME" \
    -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
    -out "$OUT_DIR/bifaria-signing.p12" -passout "pass:$PASSWORD"
fi

printf '%s' "$PASSWORD" > "$OUT_DIR/p12-password.txt"
base64 -i "$OUT_DIR/bifaria-signing.p12" > "$OUT_DIR/bifaria-signing.p12.b64"
chmod 600 "$OUT_DIR"/*

echo "--> Importing into $KEYCHAIN..."
security import "$OUT_DIR/bifaria-signing.p12" -k "$KEYCHAIN" -P "$PASSWORD" -T /usr/bin/codesign >/dev/null

SHA="$(security find-identity -p codesigning "$KEYCHAIN" | awk -v name="\"$CERT_NAME\"" 'index($0, name) {print $2; exit}')"

echo ""
echo "✓ Created and installed \"$CERT_NAME\" ($SHA)."
echo "  The build scripts now pick it up automatically."
echo "  The first build may show a keychain prompt for codesign: choose \"Always Allow\"."
echo ""
echo "Next steps:"
echo "  1. Back up $OUT_DIR (e.g. in a password manager). Losing it means users grant permissions again."
echo "  2. For GitHub Actions, add two repository secrets:"
echo "       MACOS_SIGNING_P12           = contents of $OUT_DIR/bifaria-signing.p12.b64"
echo "       MACOS_SIGNING_P12_PASSWORD  = contents of $OUT_DIR/p12-password.txt"
echo "     e.g.  gh secret set MACOS_SIGNING_P12 < \"$OUT_DIR/bifaria-signing.p12.b64\""
echo "           gh secret set MACOS_SIGNING_P12_PASSWORD < \"$OUT_DIR/p12-password.txt\""
