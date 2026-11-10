#!/usr/bin/env bash
# Make the one self-signed code-signing identity that every Seance build is
# signed with, so macOS sees the same app after every update and keeps its
# permission grants (Documents, Photos, ...). An ad-hoc signature changes with
# every build, which is why a privacy prompt came back after each update.
#
#   ./scripts/create-signing-identity.sh [NAME]      (default: Seance Local Signing)
#
# It makes a DEDICATED keychain (~/Library/Keychains/seance-signing.keychain-db,
# not on the search list, so nothing else on this Mac changes), keeps its
# password in ~/.config/seance-signing/keychain-password (mode 600; move it
# into the login keychain if you prefer), and writes a password-protected
# .p12 backup beside it. Nothing it makes belongs in a git repository. Run once
# per signing Mac; keep the backup somewhere safe.
set -euo pipefail

NAME="${1:-Seance Local Signing}"
KC="$HOME/Library/Keychains/seance-signing.keychain-db"
DIR="$HOME/.config/seance-signing"
P12="$DIR/seance-local-signing.p12"

if [[ -e "$KC" ]]; then
  echo "$KC already exists; not making another identity" >&2
  exit 1
fi

pw()  { /usr/bin/openssl rand -base64 24 | tr -d '/+=\n'; }
KC_PW="$(pw)"
P12_PW="$(pw)"
umask 077
mkdir -p "$DIR"; chmod 700 "$DIR"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

cat > "$T/openssl.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
CNF
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -sha256 \
  -config "$T/openssl.cnf" -keyout "$T/key.pem" -out "$T/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$T/key.pem" -in "$T/cert.pem" -name "$NAME" \
  -passout "pass:$P12_PW" -out "$T/id.p12" 2>/dev/null

printf '%s' "$KC_PW"  > "$DIR/keychain-password"
printf '%s' "$P12_PW" > "$DIR/p12-password"
chmod 600 "$DIR/keychain-password" "$DIR/p12-password"

security create-keychain -p "$KC_PW" "$KC"
security set-keychain-settings "$KC"          # never lock, never time out
security unlock-keychain -p "$KC_PW" "$KC"
security import "$T/id.p12" -k "$KC" -P "$P12_PW" -T /usr/bin/codesign -T /usr/bin/security >/dev/null
# Let codesign use the key without a prompt.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PW" "$KC" >/dev/null

cp "$T/id.p12" "$P12"; chmod 600 "$P12"
echo "identity: $NAME"
echo "keychain: $KC"
echo "backup:   $P12   (its password: $DIR/p12-password)"
security find-certificate -c "$NAME" -Z "$KC" | awk '/SHA-1 hash:/ {print "leaf sha1: " $3}'
