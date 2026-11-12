#!/usr/bin/env bash
# Package mingw-cross Windows build: rrss.exe + WebView2Loader.dll -> zip.
# 目标机仍需自行安装 WebView2 Evergreen Runtime；此包只带 Loader DLL。
set -euo pipefail
TARGET="${1:?target}"
BIN="${2:?bin}"
ARTIFACT="${3:?artifact}"
WEBVIEW2_VERSION="${WEBVIEW2_VERSION:-1.0.2903.40}"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
EXE="${TARGET_DIR}/${TARGET}/release/${BIN}"
test -f "$EXE" || { echo "missing $EXE"; exit 1; }

mkdir -p dist/pkg .local/webview2
STAGE=dist/pkg
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$EXE" "$STAGE/"

NUPKG=".local/webview2/Microsoft.Web.WebView2.${WEBVIEW2_VERSION}.nupkg"
if [[ ! -f "$NUPKG" ]]; then
  # v3 flat container 是直链，避免 v2 OData 再跳到 www.nuget.org。
  id_lower="$(printf '%s' 'Microsoft.Web.WebView2' | tr '[:upper:]' '[:lower:]')"
  curl -fsSL -o "$NUPKG" \
    "https://api.nuget.org/v3-flatcontainer/${id_lower}/${WEBVIEW2_VERSION}/${id_lower}.${WEBVIEW2_VERSION}.nupkg"
fi
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
unzip -q -o "$NUPKG" -d "$TMP"
DLL=$(find "$TMP" -type f -path '*/runtimes/win-x64/native/WebView2Loader.dll' | head -n1)
test -n "$DLL" || { echo "runtimes/win-x64/native/WebView2Loader.dll not found in nupkg"; exit 1; }
cp "$DLL" "$STAGE/WebView2Loader.dll"

(cd "$STAGE" && zip -9 "../${ARTIFACT}.zip" "$BIN" WebView2Loader.dll)
ls -la "dist/${ARTIFACT}.zip"
echo "Packed $BIN + WebView2Loader.dll -> dist/${ARTIFACT}.zip"
echo "Note: target PC still needs the WebView2 Evergreen Runtime installed for the UI."
