# iPhone feasibility preflight

Status: native shell and first local Notes slice verified on physical hardware

Date checked: 2026-08-16

Related direction: [Decision 006](decisions/006-iphone-companion-direction.md),
[Decision 007](decisions/007-mobile-sync-sequencing-and-provider-gate.md), and
[the mobile implementation plan](MOBILE_COMPANION_IMPLEMENTATION_PLAN.md).

## Outcome

The native app compiles, installs, launches, and completes its Rust-to-webview
startup health check in an iPhone 17 simulator. The signed app was installed on
an iPhone 15 Pro and launched through CoreDevice. Its first real product slice
provides local note creation, editing, tombstoned deletion, and search backed by
an isolated WAL-mode SQLite database in the iOS application-data directory.
File-backed store tests prove that saved notes survive a database close and
reopen.

The iOS build has its own frontend entry, Tauri config, capability manifest,
Rust entry point, and command registry. macOS-only services and native
dependencies are excluded at compile time instead of being hidden at runtime.

## Verified environment

| Check | Result |
| --- | --- |
| Host | Apple Silicon (`arm64`), macOS 26.5.1 |
| Swift | 6.3.3 |
| Rust / Cargo | 1.97.0 |
| Bun | 1.3.14 |
| Tauri CLI / crate | 2.11.2 |
| Free disk | about 1.4 TiB |
| Active developer directory | `/Applications/Xcode.app/Contents/Developer` |
| Full Xcode / iPhoneOS SDK | Xcode 26.6 / iOS 26.5 |
| Installed Rust targets | macOS, iOS device, Apple Silicon simulator, Intel simulator |
| CocoaPods | 1.17.0 |
| Valid code-signing identity | Apple Development certificate for the configured personal team |
| Simulator proof | iPhone 17: local Notes UI rendered; signed-boundary and store tests passed |
| Physical-device proof | iPhone 15 Pro: signed Notes build installed and launched |

Evidence commands:

```sh
xcode-select -p
xcodebuild -version
xcrun --sdk iphoneos --show-sdk-path
rustup target list --installed
pod --version
security find-identity -v -p codesigning
```

## Repository facts that help

- `src-tauri/src/lib.rs` already uses `#[cfg_attr(mobile,
  tauri::mobile_entry_point)]`.
- The crate emits `staticlib`, `cdylib`, and `rlib`, which is the expected Tauri
  mobile library shape.
- Tauri CLI and the Rust Tauri dependency are on the same version.
- The Vite configuration already honors `TAURI_DEV_HOST`.
- Existing icon source assets are sufficient for the first shell spike.

## Source isolation proven by the first iOS build

The first iPhone shell now has a separate config and a deliberately small
command registry. The following desktop behavior is excluded from the iOS
compile and bundle:

- global-shortcut imports, plugin construction, and registration;
- the Unix agent broker and its desktop peer-credential assumptions;
- Brain/git discovery, local agent access, Ollama/provider administration, and
  macOS `security` command-based credentials;
- meeting detection, microphone/system-audio recording, diarization, video, and
  long-running desktop workers;
- desktop reminder scheduling and background loops;
- sqlite-vec registration in the database bootstrap;
- the desktop build hook that compiles the macOS diarizer helper;
- the dormant LAN phone server and its broad historical dispatcher;
- the legacy mobile shell, which is gated by `phoneLan` and includes Ask.

The current iPhone registry exposes `mobile_health` plus five local Notes
commands: list/search, create, update, and delete. The Notes database is isolated
from the desktop database and intentionally excludes sqlite-vec. Cross-device
record identity and synchronization are not implied by this local slice.

Keychain/Data Protection probes, a shared portable record schema, migration
smoke tests against that schema, and synchronization remain unimplemented.

## Completed signing setup

Xcode is signed in, an Apple Development certificate is available, and the
personal development team is recorded in `tauri.ios.conf.json` for the
`com.noted.iphone` bundle identifier. A paid Apple Developer Program membership
is still required later for TestFlight; personal-team installations remain
subject to Apple's signing and reprovisioning limits.

The completed toolchain setup was:

```sh
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -runFirstLaunch
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
brew install cocoapods
```

The simulator, first physical-device, and basic local-persistence gates are open.
The remaining M2 work is the Keychain/Data Protection, lifecycle, accessibility,
and capability-isolation probe set described in the implementation plan.

## Reproducing the simulator proof

```sh
bun run ios:check
bun run tauri ios init --config src-tauri/tauri.ios.conf.json --ci --skip-targets-install
bun run tauri ios build --debug --target aarch64-sim --no-sign --ci \
  --config src-tauri/tauri.ios.conf.json
```

## Reproducing the physical-device proof

With the configured development team and a trusted iPhone connected:

```sh
bun run tauri ios build --debug --target aarch64 --ci \
  --export-method debugging --config src-tauri/tauri.ios.conf.json
xcrun devicectl device install app --device <device-id> \
  src-tauri/gen/apple/build/tauri-app_iOS.xcarchive/Products/Applications/Noted.app
xcrun devicectl device process launch --device <device-id> com.noted.iphone
```

No synchronized schema migration or real library data is included in this spike.
