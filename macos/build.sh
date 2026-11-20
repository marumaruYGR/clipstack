#!/bin/bash
set -euo pipefail

if [[ "${REQUIRE_STABLE_SIGNING:-0}" == "1" && "${SIGN_IDENTITY:--}" != "Developer ID Application:"* ]]; then
    echo "正式发布需要使用固定的 Developer ID Application 证书设置 SIGN_IDENTITY。" >&2
    exit 1
fi

DEST="${1:-$PWD/build}"
mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"
cd "$(dirname "$0")"
DEVELOPER="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
export DEVELOPER_DIR="$DEVELOPER"
TOOLS="$DEVELOPER/Toolchains/XcodeDefault.xctoolchain/usr/bin"
SDK="$DEVELOPER/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
export SDKROOT="$SDK"
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
APP="$TEMP/剪栈.app"
mkdir -p "$APP/Contents/"{MacOS,Resources}
cp Info.plist "$APP/Contents/"
cp Assets/{About.png,Greetings.txt} "$APP/Contents/Resources/"
cp -R Assets/Manual "$APP/Contents/Resources/"
cp CHANGELOG.md "$APP/Contents/Resources/"
"$DEVELOPER/usr/bin/actool" Assets/Clipstack.icon \
    --compile "$APP/Contents/Resources" --platform macosx --target-device mac \
    --minimum-deployment-target 26.0 --app-icon Clipstack --standalone-icon-behavior all \
    --output-partial-info-plist "$TEMP/AssetInfo.plist" \
    --output-format human-readable-text --warnings --errors
test -s "$APP/Contents/Resources/Assets.car"
/usr/libexec/PlistBuddy -c "Merge \"$TEMP/AssetInfo.plist\"" "$APP/Contents/Info.plist"

TARGETS=()
if [[ "${UNIVERSAL:-0}" == "1" ]]; then PLATFORM=(arm64 x86_64); else PLATFORM=("$(uname -m)"); fi
for ARCH in "${PLATFORM[@]}"; do
    TRIPLE="aarch64-apple-darwin"
    [[ "$ARCH" != x86_64 ]] || TRIPLE="x86_64-apple-darwin"
    MACOSX_DEPLOYMENT_TARGET=26.0 "${CARGO:-cargo}" build --locked --release --target "$TRIPLE" --target-dir "$TEMP/core"
    COMPILE_FLAGS=(-O -emit-executable -module-name JianZhan)
    if [[ "${KEEP_DEBUG_SYMBOLS:-0}" == "1" ]]; then
        mkdir -p "$DEST/Debug/$ARCH"
        COMPILE_FLAGS+=(-g -emit-module -emit-module-path "$DEST/Debug/$ARCH/JianZhan.swiftmodule")
    fi
    "$TOOLS/swiftc" "${COMPILE_FLAGS[@]}" -sdk "$SDK" -module-cache-path "$TEMP/module" -target "$ARCH-apple-macosx26.0" \
        Session.swift Store.swift SearchText.swift ClipboardFile.swift Hotkeys.swift Preview.swift Views.swift WorkspaceWindow.swift \
        PortableBackup.swift PasteAccess.swift Paste.swift Notices.swift Documents.swift Updates.swift BackupWindow.swift Main.swift \
        "$TEMP/core/$TRIPLE/release/libjianzhan_core.a" -o "$TEMP/app-$ARCH"
    TARGETS+=("$TEMP/app-$ARCH")
    if [[ "${KEEP_DEBUG_SYMBOLS:-0}" == "1" && -d "$TEMP/app-$ARCH.dSYM" ]]; then
        ditto "$TEMP/app-$ARCH.dSYM" "$DEST/Debug/$ARCH/JianZhan.dSYM"
    fi
done
"$TOOLS/lipo" -create "${TARGETS[@]}" -output "$APP/Contents/MacOS/JianZhan"
if [[ "${SIGN_IDENTITY:--}" == "Developer ID Application:"* ]]; then
    codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$APP"
else
    codesign --force --sign "${SIGN_IDENTITY:--}" "$APP"
fi
codesign --verify --deep --strict "$APP"
ditto "$APP" "$DEST/剪栈.app"
