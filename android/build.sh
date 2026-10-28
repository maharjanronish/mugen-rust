#!/usr/bin/env bash
# Kurogane Android build — compiles the Rust workspace for Android ABIs and
# packages the APK. Run inside the kurogane-android Docker image, or on a
# host with: Rust, cargo-ndk, Android SDK (platform 34, build-tools 34,
# NDK 26.3), JDK 17+.
#
# Usage: ./android/build.sh [--debug] [--abis "arm64-v8a,armeabi-v7a"]
# Output: android/app/build/outputs/apk/debug/app-debug.apk
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$ROOT"

ABIS="${ABIS:-arm64-v8a,armeabi-v7a,x86_64,i686}"
GRADLE_TASK="assembleDebug"
PROFILE_FLAG=("--release")

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug) PROFILE_FLAG=(); shift ;;
        --abis) ABIS="$2"; shift 2 ;;
        *) echo "unknown flag: $1" >&2; exit 1 ;;
    esac
done

IFS=',' read -ra TARGETS <<< "$ABIS"
NDK_ARGS=()
for t in "${TARGETS[@]}"; do NDK_ARGS+=(-t "$t"); done

echo "==> cargo ndk: ${TARGETS[*]}"
cargo ndk "${NDK_ARGS[@]}" \
    -o android/app/src/main/jniLibs \
    build "${PROFILE_FLAG[@]}" -p krg-android

echo "==> native libraries:"
ls -la android/app/src/main/jniLibs/*/

echo "==> gradle $GRADLE_TASK"
GRADLE="$(./android/ensure-gradle.sh)"
(cd "$ROOT/android" && "$GRADLE" --no-daemon "$GRADLE_TASK")

echo
echo "APK output:"
ls -la "$ROOT"/android/app/build/outputs/apk/debug/*.apk
