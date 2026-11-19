#!/bin/bash
set -euo pipefail

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
    "$TOOLS/swiftc" -O -sdk "$SDK" -module-cache-path "$TEMP/module" -target "$ARCH-apple-macosx26.0" \
        Session.swift Store.swift SearchText.swift ClipboardFile.swift Hotkeys.swift Preview.swift Views.swift WorkspaceWindow.swift \
        PortableBackup.swift Paste.swift Notices.swift Documents.swift Updates.swift BackupWindow.swift Main.swift \
        "$TEMP/core/$TRIPLE/release/libjianzhan_core.a" -o "$TEMP/app-$ARCH"
    TARGETS+=("$TEMP/app-$ARCH")
done
"$TOOLS/lipo" -create "${TARGETS[@]}" -output "$APP/Contents/MacOS/JianZhan"
codesign --force --sign "${SIGN_IDENTITY:--}" "$APP"
ditto "$APP" "$DEST/剪栈.app"
