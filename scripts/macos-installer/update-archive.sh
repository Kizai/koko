#!/bin/bash
set -euo pipefail
app="$1"
output="$2"
tools="$3/bin"
version="$(plutil -extract CFBundleVersion raw "$app/Contents/Info.plist")"
account="io.github.kizai.koko.updates"
expected="$(plutil -extract SUPublicEDKey raw "$app/Contents/Info.plist")"
if [[ "$("$tools/generate_keys" --account "$account" -p)" != "$expected" ]]; then
    echo "The update signing key must match the public key embedded in Koko.app." >&2
    exit 1
fi
mkdir -p "$output"
stage="$(mktemp -d "$output/staging.XXXXXXXX")"
trap 'rm -rf "$stage"' EXIT
archive="Koko-$version.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$stage/$archive"
"$tools/generate_appcast" --account "$account" --maximum-deltas 0 \
    --download-url-prefix "https://github.com/Kizai/koko/releases/download/v$version/" \
    -o "$stage/appcast.xml" "$stage"
test -s "$stage/appcast.xml"
mv "$stage/$archive" "$stage/appcast.xml" "$output/"
