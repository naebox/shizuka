#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(mktemp -d /tmp/shizuka-hal.XXXXXX)"
BUNDLE_DIR="${BUILD_DIR}/ShizukaAudio.driver"

bash "${SCRIPT_DIR}/build_driver.sh" "${BUNDLE_DIR}"

INSTALL_CMD="mkdir -p '/Library/Audio/Plug-Ins/HAL' && rm -rf '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && cp -R '${BUNDLE_DIR}' '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && (xattr -dr com.apple.quarantine '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' 2>/dev/null || true) && (xattr -cr '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' 2>/dev/null || true) && chown -R root:wheel '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && chmod -R u+rwX,go+rX '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && (killall -9 coreaudiod 2>/dev/null || killall coreaudiod 2>/dev/null || launchctl kickstart -kp system/com.apple.audio.coreaudiod 2>/dev/null || true)"

if [ "$(id -u)" -eq 0 ]; then
  bash -c "${INSTALL_CMD}"
else
  osascript -e "do shell script \"${INSTALL_CMD}\" with administrator privileges"
fi

rm -rf "${BUILD_DIR}"
echo "Shizuka Virtual Mic installed successfully."
