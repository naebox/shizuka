#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_BUNDLE="${1:-${SCRIPT_DIR}/build/ShizukaAudio.driver}"

rm -rf "${OUT_BUNDLE}"
mkdir -p "${OUT_BUNDLE}/Contents/MacOS"
cp "${SCRIPT_DIR}/Info.plist" "${OUT_BUNDLE}/Contents/Info.plist"

clang -O3 -Wall -Wextra \
  -bundle \
  -arch arm64 -arch x86_64 \
  -mmacosx-version-min=12.0 \
  -framework CoreAudio \
  -framework CoreFoundation \
  "${SCRIPT_DIR}/ShizukaAudio.c" \
  -o "${OUT_BUNDLE}/Contents/MacOS/ShizukaAudio"

codesign --force --deep --sign - "${OUT_BUNDLE}"
lipo -info "${OUT_BUNDLE}/Contents/MacOS/ShizukaAudio"
echo "Built Universal ShizukaAudio.driver at ${OUT_BUNDLE}"
