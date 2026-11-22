#!/bin/bash
set -euo pipefail

app="$1"
version="$2"
disk_image="$3"
release="${4:-false}"
resources="$(cd "$(dirname "$0")" && pwd)"
stage="$(mktemp -d "$(dirname "$disk_image")/installer.XXXXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/payload" "$stage/image"
ditto "$app" "$stage/payload/Koko.app"

package="$stage/image/双击安装 KoKo.pkg"
options=(--root "$stage/payload" --component-plist "$resources/component.plist"
    --install-location /Applications --identifier io.github.kizai.koko.installer --version "$version")
if [[ -n "${KOKO_INSTALLER_SIGN_IDENTITY:-}" ]]; then
    options+=(--sign "$KOKO_INSTALLER_SIGN_IDENTITY")
fi
if [[ "$release" == true ]]; then
    options+=(--timestamp)
fi
pkgbuild "${options[@]}" "$package"
xcrun swift "$resources/InstallerIcon.swift" "$app/Contents/Resources/KoKoMark.icns" "$package"
if [[ -n "${KOKO_INSTALLER_SIGN_IDENTITY:-}" ]]; then
    pkgutil --check-signature "$package"
fi

ditto "$resources/FinderLayout.dsstore" "$stage/image/.DS_Store"
hdiutil create -quiet -volname "Koko $version" -fs HFS+ -srcfolder "$stage/image" \
    -format UDZO "$stage/Koko.dmg"
if [[ "$release" == true ]]; then
    codesign --force --sign "$KOKO_SIGN_IDENTITY" --timestamp "$stage/Koko.dmg"
    xcrun notarytool submit "$stage/Koko.dmg" --keychain-profile "$KOKO_NOTARY_PROFILE" \
        --wait --output-format plist > "$stage/notary.plist" || {
            cat "$stage/notary.plist" >&2
            exit 1
        }
    if [[ "$(plutil -extract status raw "$stage/notary.plist")" != Accepted ]]; then
        plutil -p "$stage/notary.plist" >&2
        exit 1
    fi
    xcrun stapler staple "$stage/Koko.dmg"
    xcrun stapler validate "$stage/Koko.dmg"
    codesign --verify --strict "$stage/Koko.dmg"
    spctl --assess --type open --context context:primary-signature "$stage/Koko.dmg"
fi
hdiutil verify -quiet "$stage/Koko.dmg"
mv -f "$stage/Koko.dmg" "$disk_image"
