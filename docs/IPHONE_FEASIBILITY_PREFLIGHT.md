# iPhone feasibility preflight

Status: blocked on full Xcode installation; source isolation work not started

Date checked: 2026-08-14

Related direction: [Decision 006](decisions/006-iphone-companion-direction.md)
and [the mobile implementation plan](MOBILE_COMPANION_IMPLEMENTATION_PLAN.md).

## Outcome

The repository has the basic Tauri mobile entry shape, and this Apple Silicon
Mac has ample disk space and current Rust, Swift, Bun, and Tauri tooling. It
cannot generate, compile, sign, or install an iPhone shell yet because only the
standalone Command Line Tools are installed. There is no iPhoneOS SDK, iOS Rust
target, CocoaPods installation, signing identity, or provisioning profile.

Do not run `tauri ios init` until full Xcode is installed and opened once. The
generated project would not be verifiable, and the current shared Rust startup
still includes macOS-only services.

## Verified environment

| Check | Result |
| --- | --- |
| Host | Apple Silicon (`arm64`), macOS 26.5.1 |
| Swift | 6.3.3 |
| Rust / Cargo | 1.97.0 |
| Bun | 1.3.14 |
| Tauri CLI / crate | 2.11.2 |
| Free disk | about 1.4 TiB |
| Active developer directory | `/Library/Developer/CommandLineTools` |
| Full Xcode / iPhoneOS SDK | missing |
| Installed Rust targets | `aarch64-apple-darwin` only |
| CocoaPods | missing |
| Valid code-signing identities | none |

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

## Source blockers to isolate before the first iOS build

The first iPhone shell must have a separate config and a deliberately small
command registry. Reusing the macOS startup verbatim would pull in unsupported or
inappropriate behavior:

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

The mobile shell should initially expose only startup diagnostics, mobile-local
database migration/smoke commands, Keychain/Data Protection probes, and the
first Notes vertical-slice commands. Desktop-only modules should be target-gated
or moved behind a desktop entry builder rather than stubbed at runtime.

## Owner prerequisite

1. Install the full Xcode application from Apple.
2. Launch Xcode once, accept its license, let it install platform components,
   and sign in with the Apple ID that will own development signing.
3. Select the development team when Xcode offers one. A paid Apple Developer
   Program membership is required later for TestFlight; a local device spike can
   begin with an available personal team, subject to Apple's signing limits.

After Xcode exists at `/Applications/Xcode.app`, run:

```sh
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -runFirstLaunch
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
brew install cocoapods
```

Re-run all evidence commands above. The preflight gate is open only when both
`iphoneos` and `iphonesimulator` SDKs resolve and Xcode can see an eligible
development team.

## First command after the gate opens

Once the separate iOS config, dependency guards, and minimal entry registry are
in place:

```sh
bun run tauri ios init --ci --skip-targets-install
```

Then compile the minimal shell for the simulator before attaching a physical
iPhone. No synchronized schema migration or real library data should be included
in this spike.
