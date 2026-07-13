#!/bin/bash
# Build the release app and (re)install it into /Applications.
# This IS the update mechanism: noted's source of truth is this repo, so
# "updating the app" = rebuild + swap. Run after landing changes:
#
#   bun run app:update
#
# Notes:
# - Quit the installed noted (and any `tauri dev` instance) first — two
#   instances fight over the phone port (8787) and double the detection
#   prompts. The database is shared either way (same app-data dir).
# - The build is ad-hoc signed; macOS may re-ask the mic / System Audio
#   Recording permissions after an update. One click each.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "Building noted (release)…"
bun run tauri build

APP="src-tauri/target/release/bundle/macos/noted.app"
if [ ! -d "$APP" ]; then
  echo "Build finished but $APP is missing — check the tauri build output." >&2
  exit 1
fi

if pgrep -x noted > /dev/null 2>&1; then
  echo "Quitting the running noted…"
  osascript -e 'tell application "noted" to quit' 2>/dev/null || pkill -x noted || true
  sleep 1
fi

echo "Installing to /Applications…"
rm -rf /Applications/noted.app
ditto "$APP" /Applications/noted.app

echo "Done — noted updated. Launching…"
open /Applications/noted.app
