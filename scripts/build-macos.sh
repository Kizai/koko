#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

allow_ad_hoc=false
if [[ "$#" -eq 1 && "$1" == "--allow-ad-hoc" ]]; then
    allow_ad_hoc=true
elif [[ "$#" -eq 1 && "$1" == "--require-apple-signing" ]]; then
    :
elif [[ "$#" -ne 0 ]]; then
    printf 'Usage: %s [--allow-ad-hoc | --require-apple-signing]\n' "$0" >&2
    exit 2
fi

signing_identity="${KOKO_SIGN_IDENTITY:--}"
build_dir="$repo_root/target/koko-macos"
ad_hoc=false
if [[ "$signing_identity" == "-" ]]; then
    if [[ "$allow_ad_hoc" != true ]]; then
        printf 'Set KOKO_SIGN_IDENTITY to an Apple code-signing identity in the build Mac keychain.\nFor a local test build, explicitly pass --allow-ad-hoc.\n' >&2
        exit 1
    fi
    ad_hoc=true
    build_dir="$build_dir/local"
    printf 'Ad hoc test build. macOS permissions may need to be granted again after upgrading.\n' >&2
fi
signing_options=(--force --sign "$signing_identity")

export MACOSX_DEPLOYMENT_TARGET=13.0
version="$(cargo metadata --locked --no-deps --format-version 1 | plutil -extract packages.0.version raw -)"
disk_image="$build_dir/Koko-$version.dmg"
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
    xcrun swiftc -parse-as-library -swift-version 6 -O \
        -target "$architecture-apple-macosx$MACOSX_DEPLOYMENT_TARGET" \
        apps/koko-macos/ScreenCapture.swift apps/koko-macos/DesktopInput.swift apps/koko-macos/desktop/KokoDesktop.swift \
        -o "$build_dir/koko-desktop-$architecture"
done

xcrun lipo -create "$build_dir/Koko-arm64" "$build_dir/Koko-x86_64" \
    -output "$app/Contents/MacOS/Koko"
for executable in koko-daemon koko-agent; do
    xcrun lipo -create \
        "$repo_root/target/aarch64-apple-darwin/release/$executable" \
        "$repo_root/target/x86_64-apple-darwin/release/$executable" \
        -output "$app/Contents/Helpers/$executable"
    codesign "${signing_options[@]}" --identifier "io.github.kizai.koko.$executable" \
        "$app/Contents/Helpers/$executable"
done

xcrun lipo -create "$build_dir/koko-desktop-arm64" "$build_dir/koko-desktop-x86_64" \
    -output "$app/Contents/Helpers/koko-desktop"
codesign "${signing_options[@]}" --identifier io.github.kizai.koko.koko-desktop \
    "$app/Contents/Helpers/koko-desktop"

ditto apps/koko-macos/Info.plist "$app/Contents/Info.plist"
plutil -insert CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -insert CFBundleVersion -string "$version" "$app/Contents/Info.plist"
plutil -insert KokoAdHocSigned -bool "$ad_hoc" "$app/Contents/Info.plist"
logo="$repo_root/docs/assets/koko-logo.png"
iconset="$build_dir/KoKo.iconset"
mkdir -p "$iconset"
for points in 16 32 128 256 512; do
    sips -z "$points" "$points" "$logo" --out "$iconset/icon_${points}x${points}.png" >/dev/null
    pixels=$((points * 2))
    sips -z "$pixels" "$pixels" "$logo" --out "$iconset/icon_${points}x${points}@2x.png" >/dev/null
done
iconutil --convert icns --output "$app/Contents/Resources/KoKo.icns" "$iconset"
ditto LICENSE "$app/Contents/Resources/LICENSE"
codesign "${signing_options[@]}" "$app"
codesign --verify --deep --strict "$app"
if [[ "$ad_hoc" != true ]]; then
    codesign --verify -R '=anchor apple generic' "$app"
fi

ln -sfn /Applications "$image_dir/Applications"
hdiutil create -quiet -volname "Koko $version" -srcfolder "$image_dir" \
    -format UDZO -ov "$disk_image"
hdiutil verify -quiet "$disk_image"

for older in "$build_dir"/Koko*.dmg; do
    [[ "$older" == "$disk_image" || ! -f "$older" ]] && continue
    filename="$(basename "$older")"
    if [[ "$filename" == "Koko.dmg" ]] ||
       [[ "$filename" =~ ^Koko-[0-9]+\.[0-9]+\.[0-9]+\.dmg$ &&
          "$(printf '%s\n' "$filename" "Koko-$version.dmg" | LC_ALL=C sort -V | tail -n 1)" == "Koko-$version.dmg" ]]; then
        rm -- "$older"
    fi
done
printf 'App: %s\nDMG: %s\n' "$app" "$disk_image"
