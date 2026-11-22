#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT
sources=()
for source in apps/koko-macos/*.swift; do
    [[ "$source" == */KokoApp.swift ]] || sources+=("$source")
done
sparkle="$(scripts/macos-installer/sparkle.sh)/Sparkle.xcframework/macos-arm64_x86_64"
xcrun swiftc -F "$sparkle" -framework Sparkle -Xlinker -rpath -Xlinker "$sparkle" -parse-as-library -swift-version 6 "${sources[@]}" tests/macos/*.swift -o "$test_dir/check"
"$test_dir/check"
