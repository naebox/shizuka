#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(mktemp -d /tmp/shizuka-hal.XXXXXX)"
BUNDLE_DIR="${BUILD_DIR}/ShizukaAudio.driver"

mkdir -p "${BUNDLE_DIR}/Contents/MacOS"
cp "${SCRIPT_DIR}/Info.plist" "${BUNDLE_DIR}/Contents/Info.plist"

clang -O3 -Wall -Wextra \
  -bundle \
  -arch arm64 -arch x86_64 \
  -mmacosx-version-min=12.0 \
  -framework CoreAudio \
  -framework CoreFoundation \
  "${SCRIPT_DIR}/ShizukaAudio.c" \
  -o "${BUNDLE_DIR}/Contents/MacOS/ShizukaAudio"

codesign --force --deep --sign - "${BUNDLE_DIR}"

if [ "$(id -u)" -eq 0 ]; then
  rm -rf "/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver"
  cp -R "${BUNDLE_DIR}" "/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver"
  chown -R root:wheel "/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver"
  launchctl kickstart -kp system/com.apple.audio.coreaudiod
else
  osascript -e "do shell script \"rm -rf '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && cp -R '${BUNDLE_DIR}' '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && chown -R root:wheel '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && launchctl kickstart -kp system/com.apple.audio.coreaudiod\" with administrator privileges"
fi

rm -rf "${BUILD_DIR}"
echo "Shizuka Virtual Mic installed successfully."
