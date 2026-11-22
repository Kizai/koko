#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT
sources=()
for source in apps/koko-macos/*.swift; do
    [[ "$source" == */KokoApp.swift ]] || sources+=("$source")
done
xcrun swiftc -parse-as-library -swift-version 6 "${sources[@]}" tests/macos/DesktopRegression.swift -o "$test_dir/check"
"$test_dir/check"
