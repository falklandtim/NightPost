#!/usr/bin/env bash
# Create a self-signed code-signing certificate, "NightPost Local Signing", in
# your login keychain. scripts/bundle-macos.sh signs with it when it exists.
#
# Why: the Keychain only lets the app that saved a password read it back,
# and it recognises the app by its signature. Ad-hoc signatures change with
# every build, so each reinstall asked for Keychain access again. Signed
# with this certificate, every build counts as the same app.
#
# It's only trusted for code signing, on this Mac. macOS asks for your
# password once, to trust it. Run it once:
#
#   scripts/make-signing-cert.sh
set -euo pipefail

NAME="NightPost Local Signing"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

# A certificate made before the rename does the same job; bundle-macos.sh uses it.
if security find-identity -v -p codesigning 2>/dev/null | grep -q "OwlPost Local Signing"; then
  echo "\"OwlPost Local Signing\" is already in your login keychain, and bundle-macos.sh signs with it. Nothing to do."
  exit 0
fi

if security find-certificate -c "$NAME" "$KEYCHAIN" >/dev/null 2>&1; then
  echo "\"$NAME\" is already in your login keychain."
  security find-identity -v -p codesigning | grep "$NAME" || echo "…but it isn't trusted for code signing yet; delete it in Keychain Access and run this again."
  exit 0
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
# LibreSSL (macOS's own openssl) writes a .p12 that `security import` reads.
OPENSSL=/usr/bin/openssl

cat > "$WORK/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
CNF

"$OPENSSL" req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$WORK/cert.cnf" \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null
# A throwaway password: the .p12 only exists until it is imported.
PASS=$("$OPENSSL" rand -hex 16)
"$OPENSSL" pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" -name "$NAME" \
  -out "$WORK/nightpost.p12" -passout "pass:$PASS"

# Only codesign may use the private key without asking.
security import "$WORK/nightpost.p12" -k "$KEYCHAIN" -P "$PASS" -T /usr/bin/codesign >/dev/null
echo "Trusting the certificate for code signing (macOS asks for your password)…"
security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$WORK/cert.pem"

security find-identity -v -p codesigning | grep "$NAME"
echo "Done. scripts/bundle-macos.sh will now sign NightPost with \"$NAME\"."
