#!/bin/bash
# Keep the installed standard app current without rebuilding on every automated
# stop. The release binary is a reliable stamp: app source newer than it means
# /Applications/noted.app cannot contain the latest checkout.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_BINARY="/Applications/noted.app/Contents/MacOS/tauri-app"
WATCHED_PATHS=(
  src
  src-tauri
  public
  package.json
  bun.lock
  index.html
  vite.config.ts
  tsconfig.json
  tsconfig.node.json
)

# A live Tauri development session already hot-reloads frontend changes and
# rebuilds Rust changes. Do not interrupt it by replacing/relaunching the app.
if pgrep -f '[t]auri dev|src-tauri/target/debug/tauri-app' >/dev/null 2>&1; then
  echo "Tauri dev is running; the installed app update is deferred."
  exit 0
fi

if [ -x "$APP_BINARY" ]; then
  NEWER_SOURCE="$({
    find "${WATCHED_PATHS[@]}" \
      -path 'src-tauri/target' -prune -o \
      -type f -newer "$APP_BINARY" -print -quit
  } 2>/dev/null || true)"
  if [ -z "$NEWER_SOURCE" ]; then
    exit 0
  fi
  echo "Installed noted is stale (${NEWER_SOURCE} changed); updating…"
else
  echo "Installed noted is missing; building and installing it…"
fi

bun run app:update
