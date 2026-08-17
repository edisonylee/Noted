# iPhone feasibility preflight

Status: simulator feasibility shell built and launched; physical-device signing pending

Date checked: 2026-08-16

Related direction: [Decision 006](decisions/006-iphone-companion-direction.md)
and [the mobile implementation plan](MOBILE_COMPANION_IMPLEMENTATION_PLAN.md).

## Outcome

The native feasibility shell now compiles, installs, launches, and completes its
Rust-to-webview startup health check in an iPhone 17 simulator. The iOS build has
its own frontend entry, Tauri config, capability manifest, Rust entry point, and
command registry. macOS-only services and native dependencies are excluded at
compile time instead of being hidden at runtime.

Physical-device installation remains pending because Xcode does not yet expose
a valid Apple Development signing identity or development team on this Mac.

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
| Valid code-signing identities | none |
| Simulator proof | iPhone 17: build, install, launch, and `mobile_health` passed |

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

The current mobile shell exposes only `mobile_health`. Mobile-local database
migration/smoke commands, Keychain/Data Protection probes, and the first Notes
vertical-slice commands remain intentionally unimplemented.

## Owner prerequisite

1. In Xcode Settings, sign in with the Apple ID that will own development signing.
2. Create or download an Apple Development certificate and select the team for
   the `com.noted.iphone` bundle identifier. A paid Apple Developer
   Program membership is required later for TestFlight; a local device spike can
   begin with an available personal team, subject to Apple's signing limits.

The completed toolchain setup was:

```sh
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -runFirstLaunch
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
brew install cocoapods
```

The simulator gate is open. The physical-device gate opens when Xcode can see an
eligible development team and a connected iPhone.

## Reproducing the simulator proof

```sh
bun run ios:check
bun run tauri ios init --config src-tauri/tauri.ios.conf.json --ci --skip-targets-install
bun run tauri ios build --debug --target aarch64-sim --no-sign --ci \
  --config src-tauri/tauri.ios.conf.json
```

No synchronized schema migration or real library data is included in this spike.
