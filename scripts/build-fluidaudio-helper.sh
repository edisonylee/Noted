#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PACKAGE_DIR="$PROJECT_DIR/native/noted-fluid-diarizer"
RESOURCE_DIR="$PROJECT_DIR/src-tauri/resources"
SCRATCH_DIR="$PROJECT_DIR/.swift-build/noted-fluid-diarizer"
MODULE_CACHE_DIR="$SCRATCH_DIR/module-cache"

# Some Command Line Tools updates leave the newest SDK one Swift patch behind
# the compiler. The installed 15.4 SDK is fully sufficient for our macOS 14
# deployment target and avoids mutating the user's system toolchain.
SDK_ARGS=()
SELECTED_SDK=""
COMPATIBLE_SDK="/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk"
if [[ -d "$COMPATIBLE_SDK" ]]; then
  SDK_ARGS=(--sdk "$COMPATIBLE_SDK")
  SELECTED_SDK="$COMPATIBLE_SDK"
else
  SELECTED_SDK="$(xcrun --sdk macosx --show-sdk-path)"
  SDK_ARGS=(--sdk "$SELECTED_SDK")
fi

mkdir -p "$RESOURCE_DIR" "$MODULE_CACHE_DIR"
SDKROOT="$SELECTED_SDK" \
CLANG_MODULE_CACHE_PATH="$MODULE_CACHE_DIR" \
SWIFTPM_MODULECACHE_OVERRIDE="$MODULE_CACHE_DIR" \
swift build \
  --package-path "$PACKAGE_DIR" \
  --scratch-path "$SCRATCH_DIR" \
  "${SDK_ARGS[@]}" \
  --configuration release \
  --product noted-fluid-diarizer
cp "$SCRATCH_DIR/release/noted-fluid-diarizer" "$RESOURCE_DIR/noted-fluid-diarizer"
chmod 755 "$RESOURCE_DIR/noted-fluid-diarizer"
