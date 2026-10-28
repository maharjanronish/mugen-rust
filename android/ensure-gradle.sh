#!/usr/bin/env bash
# Ensures a Gradle 8.7 binary is available; prints its path.
# Used by build.sh and CI so host installations don't matter.
set -euo pipefail

DIST="$(cd "$(dirname "$0")" && pwd)/.gradle-dist"
WANT="$DIST/gradle-8.7/bin/gradle"

if [[ -x "$WANT" ]]; then
    echo "$WANT"
    exit 0
fi

if command -v gradle > /dev/null 2>&1; then
    if gradle --version 2> /dev/null | grep -q "Gradle 8"; then
        command -v gradle
        exit 0
    fi
fi

mkdir -p "$DIST"
if [[ ! -f /tmp/gradle-8.7.zip ]]; then
    wget -q https://services.gradle.org/distributions/gradle-8.7-bin.zip -O /tmp/gradle-8.7.zip
fi
unzip -q -o /tmp/gradle-8.7.zip -d "$DIST"
echo "$WANT"
