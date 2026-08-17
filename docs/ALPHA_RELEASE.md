# Noted Mac alpha profile

The alpha is a focused macOS product built from the private development
repository. It is not a second copy of the application.

## Included

- Notes, capture, Today, search, knowledge, and Google Calendar
- Meeting recording with microphone and macOS system audio
- Live transcription, notes, summaries, exports, and retained audio controls
- Themes
- Local inference and the full bring-your-own-key provider matrix

## Deferred

- Noted Hosted inference, billing, customer keys, quotas, and service operations
- Native iPhone companion; the retired LAN/PWA bridge stays disabled
- Remote-participant diarization and speaker naming
- Meeting window video capture
- Windows and other platforms

The alpha requires macOS 14.4 or newer because system-audio process taps are a
core product capability.

## Build and verify

```sh
bun run alpha:check
bun run tauri:alpha
```

`alpha:check` builds the frontend with its release gates and confirms the
deferred controls are absent. `tauri:alpha` applies the matching Rust profile,
uses the alpha bundle identifier, and produces a `Noted Alpha.app` bundle.

This profile does not make an artifact distributable by itself. Public builds
still require Developer ID signing, hardened runtime, notarization, stapling,
DMG packaging, and release testing on a clean Mac.
