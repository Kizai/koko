#!/bin/bash
# Create the self-signed code-signing identity KoKo uses until it ships with a Developer ID.
# macOS remembers Screen Recording and Accessibility grants by the app's designated requirement.
# Ad-hoc builds pin that requirement to each build's hash; this certificate keeps it stable.
set -euo pipefail

name="KoKo Self-Signed Code Signing"
keychain="$HOME/Library/Keychains/login.keychain-db"
backup="${1:-}"
if [[ -z "$backup" ]]; then
    printf 'Usage: %s <backup.p12>\nThe backup holds the private key. Keep it outside the repository.\n' "$0" >&2
    exit 2
fi
if [[ -e "$backup" ]]; then
    printf '%s already exists. Choose a new backup path.\n' "$backup" >&2
    exit 1
fi
if security find-certificate -c "$name" "$keychain" >/dev/null 2>&1; then
    printf '"%s" is already in the login keychain. Reuse it, or import its backup on a new build Mac:\n' "$name" >&2
    printf '  security import <backup.p12> -k %s -T /usr/bin/codesign\n' "$keychain" >&2
    exit 1
fi

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
chmod 700 "$stage"
cat > "$stage/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $name
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF
# The system LibreSSL writes PKCS#12 files that the macOS keychain can import.
/usr/bin/openssl req -x509 -newkey rsa:3072 -nodes -days 3650 \
    -keyout "$stage/key.pem" -out "$stage/cert.pem" -config "$stage/cert.cnf" 2>/dev/null

read -r -s -p "Password for the backup file: " KOKO_P12_PASSWORD
printf '\n'
read -r -s -p "Repeat the password: " confirm
printf '\n'
if [[ -z "$KOKO_P12_PASSWORD" || "$KOKO_P12_PASSWORD" != "$confirm" ]]; then
    printf 'The passwords are empty or do not match.\n' >&2
    exit 1
fi
export KOKO_P12_PASSWORD
/usr/bin/openssl pkcs12 -export -name "$name" -inkey "$stage/key.pem" -in "$stage/cert.pem" \
    -passout env:KOKO_P12_PASSWORD -out "$stage/identity.p12"
unset KOKO_P12_PASSWORD

(umask 077 && cp "$stage/identity.p12" "$backup")
# macOS asks for the backup password, then lets codesign use the key without further prompts.
security import "$backup" -k "$keychain" -f pkcs12 -T /usr/bin/codesign

hash="$(security find-certificate -c "$name" -Z "$keychain" | sed -n 's/^SHA-1 hash: //p')"
cat <<EOF
Created "$name" (SHA-1 $hash).
Backup: $backup
Store the backup and its password somewhere safe. Losing them forces every user to grant permissions again.

Build with:
  KOKO_SIGN_IDENTITY="$name" ./scripts/build-macos.sh
EOF
