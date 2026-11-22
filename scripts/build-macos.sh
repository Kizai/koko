#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

allow_ad_hoc=false
release=false
if [[ "$#" -eq 1 && "$1" == "--allow-ad-hoc" ]]; then
    allow_ad_hoc=true
elif [[ "$#" -eq 1 && "$1" == "--release" ]]; then
    release=true
elif [[ "$#" -eq 1 && "$1" == "--require-apple-signing" ]]; then
    :
elif [[ "$#" -ne 0 ]]; then
    printf 'Usage: %s [--allow-ad-hoc | --require-apple-signing | --release]\n' "$0" >&2
    exit 2
fi

signing_identity="${KOKO_SIGN_IDENTITY:--}"
build_dir="$repo_root/target/koko-macos"
ad_hoc=false
if [[ "$release" == true ]]; then
    app_identity_pattern='^Developer ID Application: .+ \(([A-Z0-9]+)\)$'
    installer_identity_pattern='^Developer ID Installer: .+ \(([A-Z0-9]+)\)$'
    if [[ ! "$signing_identity" =~ $app_identity_pattern ]]; then
        printf 'Set KOKO_SIGN_IDENTITY to the full Developer ID Application identity.\n' >&2
        exit 1
    fi
    team_id="${BASH_REMATCH[1]}"
    if [[ ! "${KOKO_INSTALLER_SIGN_IDENTITY:-}" =~ $installer_identity_pattern || "${BASH_REMATCH[1]:-}" != "$team_id" ]]; then
        printf 'Set KOKO_INSTALLER_SIGN_IDENTITY to the full Developer ID Installer identity for team %s.\n' "$team_id" >&2
        exit 1
    fi
    : "${KOKO_NOTARY_PROFILE:?Set KOKO_NOTARY_PROFILE to an existing notarytool keychain profile.}"
    build_dir="$build_dir/release"
elif [[ "$signing_identity" == "-" ]]; then
    if [[ "$allow_ad_hoc" != true ]]; then
        printf 'Set KOKO_SIGN_IDENTITY to an Apple code-signing identity in the build Mac keychain.\nFor a local test build, explicitly pass --allow-ad-hoc.\n' >&2
        exit 1
    fi
    ad_hoc=true
    build_dir="$build_dir/local"
    printf 'Ad hoc test build. macOS permissions may need to be granted again after upgrading.\n' >&2
fi
signing_options=(--force --sign "$signing_identity")
if [[ "$release" == true ]]; then
    signing_options+=(--options runtime --timestamp)
fi

export MACOSX_DEPLOYMENT_TARGET=13.0
version="$(cargo metadata --locked --no-deps --format-version 1 | plutil -extract packages.0.version raw -)"
disk_image="$build_dir/Koko-$version.dmg"
if [[ "$release" == true ]]; then
    # Only the verified, stapled DMG is retained in the release directory.
    mkdir -p "$build_dir"
    build_dir="$(mktemp -d "$build_dir/build.XXXXXXXX")"
    trap 'rm -rf "$build_dir"' EXIT
fi
app="$build_dir/Koko.app"
sparkle="$("$repo_root/scripts/macos-installer/sparkle.sh")"
frameworks="$sparkle/Sparkle.xcframework/macos-arm64_x86_64"
launcher_sources=("$repo_root"/apps/koko-macos/*.swift)

previous_app="${KOKO_PREVIOUS_APP:-}"
if [[ -n "$previous_app" ]]; then
    if [[ ! -d "$previous_app" || "$previous_app" -ef "$app" ]]; then
        printf 'KOKO_PREVIOUS_APP must point to a separate copy of the signed previous app.\n' >&2
        exit 1
    fi
    codesign --verify --deep --strict "$previous_app"
fi

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
        -F "$frameworks" -framework Sparkle -Xlinker -rpath -Xlinker @executable_path/../Frameworks \
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
rm -f "$app/Contents/Resources/KoKo.icns"
iconutil --convert icns --output "$app/Contents/Resources/KoKoMark.icns" "$iconset"
ditto LICENSE "$app/Contents/Resources/LICENSE"
ditto "$sparkle/LICENSE" "$app/Contents/Resources/Sparkle-LICENSE"
framework="$app/Contents/Frameworks/Sparkle.framework"
ditto "$frameworks/Sparkle.framework" "$framework"
if [[ "$ad_hoc" != true ]]; then
    components="$framework/Versions/B"
    for component in "$components"/XPCServices/*.xpc "$components"/Autoupdate "$components"/Updater.app "$framework"; do
        codesign "${signing_options[@]}" --preserve-metadata=entitlements "$component"
    done
fi
codesign "${signing_options[@]}" "$app"
codesign --verify --deep --strict "$app"
if [[ "$release" == true ]]; then
    codesign --verify -R "=identifier \"io.github.kizai.koko\" and anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"$team_id\"" "$app"
elif [[ "$ad_hoc" != true ]]; then
    codesign --verify -R '=anchor apple generic' "$app"
fi

if [[ -n "$previous_app" ]]; then
    verify_continuity() {
        local requirement
        # codesign prefixes an implicit cdhash requirement with "# ".
        requirement="$(codesign --display -r - "$1" | sed -n 's/^# //; s/^designated => //p')"
        if [[ -z "$requirement" ]]; then
            printf 'Missing designated requirement for %s\n' "$1" >&2
            return 1
        fi
        codesign --verify -R "=$requirement" "$2"
    }
    verify_continuity "$previous_app" "$app"
    verify_continuity "$app" "$previous_app"
fi

"$repo_root/scripts/macos-installer/package.sh" "$app" "$version" "$disk_image" "$release"
if [[ "$release" == true || "${KOKO_UPDATE_ASSETS:-0}" == 1 ]]; then
    "$repo_root/scripts/macos-installer/update-archive.sh" "$app" "$(dirname "$disk_image")/updates/$version" "$sparkle"
fi
if [[ "$release" == true ]]; then
    printf 'Notarized DMG: %s\n' "$disk_image"
    exit 0
fi

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
