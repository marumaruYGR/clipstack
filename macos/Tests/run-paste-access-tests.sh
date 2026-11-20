#!/bin/bash
set -euo pipefail
source_root="$(cd "$(dirname "$0")/.." && pwd)"
developer="${DEVELOPER_DIR:-$(xcode-select -p)}"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
"$developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc" \
    -sdk "$developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk" \
    -module-cache-path "$scratch/modules" \
    "$source_root/PasteAccess.swift" "$source_root/Tests/PasteAccessTests.swift" \
    -o "$scratch/paste-access-tests"
"$scratch/paste-access-tests"
