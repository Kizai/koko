#!/bin/bash
# Print the local path of the pinned Sparkle binary distribution.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)/target/sparkle"
version=2.10.1
distribution="$root/$version"
if [[ ! -d "$distribution" ]]; then
    mkdir -p "$root"
    stage="$(mktemp -d "$root/download.XXXXXXXX")"
    trap 'rm -rf "$stage"' EXIT
    curl --fail --location --silent --show-error --retry 2 \
        "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-for-Swift-Package-Manager.zip" \
        -o "$stage/Sparkle.zip"
    (cd "$stage" && echo '4c2e48a3e50ebcc2c88a58a4c7264a1a99c3c31857ce754357afb9cdb323022a  Sparkle.zip' | shasum -a 256 -c - >&2)
    ditto -x -k "$stage/Sparkle.zip" "$stage/$version"
    # Both desktop tests and the app build can request the dependency at the same time.
    /bin/mv -n "$stage/$version" "$root/"
fi
printf '%s\n' "$distribution"
