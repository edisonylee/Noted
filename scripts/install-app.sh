#!/bin/bash
# Build the standard local app and (re)install it into /Applications.
# This IS the update mechanism: noted's source of truth is this repo, so
# "updating the app" = rebuild + swap. Run after landing changes:
#
#   bun run app:update
#
# Notes:
# - Quit the installed noted, Noted Alpha, and any `tauri dev` instance first.
#   Multiple variants can compete for global app behavior and double the
#   detection prompts. Noted Alpha is only for explicit release validation.
# - The build is ad-hoc signed; macOS may re-ask the mic / System Audio
#   Recording permissions after an update. One click each.
set -euo pipefail
cd "$(dirname "$0")/.."

# A local build must include current master before replacing the installed app.
if git rev-parse --verify origin/master >/dev/null 2>&1 && ! git merge-base --is-ancestor origin/master HEAD; then
  echo "Refusing to replace Noted: this checkout does not contain origin/master. Integrate current master before installing." >&2
  exit 1
fi

echo "Building standard local noted with the native iPhone companion preview…"
VITE_NOTED_IPHONE_COMPANION=1 bun run tauri build --features sanitized-development-fixtures

APP="src-tauri/target/release/bundle/macos/noted.app"
if [ ! -d "$APP" ]; then
  echo "Build finished but $APP is missing — check the tauri build output." >&2
  exit 1
fi

if pgrep -x tauri-app > /dev/null 2>&1 \
  || pgrep -x noted > /dev/null 2>&1 \
  || pgrep -x Noted > /dev/null 2>&1; then
  echo "Quitting the running noted…"
  osascript -e 'tell application id "com.noted.desktop.alpha" to quit' 2>/dev/null || true
  osascript -e 'tell application "noted" to quit' 2>/dev/null \
    || pkill -x tauri-app \
    || pkill -x noted \
    || pkill -x Noted \
    || true
  sleep 1
fi

echo "Installing to /Applications…"
rm -rf /Applications/noted.app
ditto "$APP" /Applications/noted.app

echo "Done — noted updated. Launching…"
open /Applications/noted.app
