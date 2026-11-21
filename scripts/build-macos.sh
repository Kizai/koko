#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

export MACOSX_DEPLOYMENT_TARGET=13.0
build_dir="$repo_root/target/koko-macos"
image_dir="$build_dir/image"
app="$image_dir/Koko.app"
launcher_sources=("$repo_root"/apps/koko-macos/*.swift)

mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Resources"
rustup target add aarch64-apple-darwin x86_64-apple-darwin

for architecture in arm64 x86_64; do
    rust_architecture="$architecture"
    if [[ "$architecture" == arm64 ]]; then
        rust_architecture=aarch64
    fi

    cargo build --release --locked --target-dir "$repo_root/target" \
        --target "$rust_architecture-apple-darwin" -p koko-daemon -p koko-agent
    xcrun swiftc -parse-as-library -swift-version 6 -O \
        -target "$architecture-apple-macosx$MACOSX_DEPLOYMENT_TARGET" \
        "${launcher_sources[@]}" -o "$build_dir/Koko-$architecture"
done

xcrun lipo -create "$build_dir/Koko-arm64" "$build_dir/Koko-x86_64" \
    -output "$app/Contents/MacOS/Koko"
for executable in koko-daemon koko-agent; do
    xcrun lipo -create \
        "$repo_root/target/aarch64-apple-darwin/release/$executable" \
        "$repo_root/target/x86_64-apple-darwin/release/$executable" \
        -output "$app/Contents/Helpers/$executable"
    codesign --force --sign - "$app/Contents/Helpers/$executable"
done

ditto apps/koko-macos/Info.plist "$app/Contents/Info.plist"
ditto LICENSE "$app/Contents/Resources/LICENSE"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"

ln -sfn /Applications "$image_dir/Applications"
hdiutil create -quiet -volname Koko -srcfolder "$image_dir" \
    -format UDZO -ov "$build_dir/Koko.dmg"
printf 'App: %s\nDMG: %s\n' "$app" "$build_dir/Koko.dmg"
