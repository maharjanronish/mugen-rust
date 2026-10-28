#!/usr/bin/env bash
# Bundle the official Ikemen GO base data (data/, font/, chars/, stages/,
# external/, sound/, video/) into the APK assets so the app ships playable
# out of the box — the same content set the official Android APK carries.
#
# The big files are NOT committed to git; they are pulled from the Ikemen GO
# release zip (cached in android/.cache/) at build time.
#
# Usage: ./android/bundle-basedata.sh [/path/to/Ikemen_GO-linux.zip]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
ASSETS="$ROOT/android/app/src/main/assets/basedata"
URL="https://github.com/ikemen-engine/Ikemen-GO/releases/download/v1.0.0/Ikemen_GO-v1.0.0-linux.zip"
CACHE_DIR="$ROOT/android/.cache"
ZIP="${1:-$CACHE_DIR/Ikemen_GO-v1.0.0-linux.zip}"

mkdir -p "$CACHE_DIR"
if [[ ! -f "$ZIP" ]]; then
    echo "==> downloading $URL"
    curl -sL --fail -o "$ZIP" "$URL"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
unzip -q -o "$ZIP" -d "$TMP"

rm -rf "$ASSETS"
mkdir -p "$ASSETS"
for d in data font external sound chars stages video; do
    if [[ -d "$TMP/$d" ]]; then
        cp -r "$TMP/$d" "$ASSETS/$d"
    fi
done

echo "==> basedata bundled: $(du -sh "$ASSETS" | cut -f1) ($(find "$ASSETS" -type f | wc -l) files)"
