# Noted Apple security boundary

This isolated Tauri 2 plugin owns the iPhone companion's private key material
and protected-store lifecycle. The iOS app registers it for the
sanitized-fixture checkpoint, while personal-data enrollment remains disabled
until the full M4 security gate is complete.

## Native guarantees

- Device signing uses a Secure Enclave P-256 key on physical iPhones.
- The X25519 HPKE private key, Secure Enclave persistent representation, and
  decrypted library bootstrap are held in one Keychain generic-password item.
- The item uses `kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly`, explicitly
  disables synchronization, and opts into the Data Protection Keychain.
- Rust receives opaque UUID handles, public keys, signatures, and the ephemeral
  challenge plaintext/exporter required by the pairing transcript. Private keys
  and the decrypted library bootstrap never cross the bridge.
- Bootstrap staging, activation, and discard are single-record `SecItemUpdate`
  transitions. Discard writes a tombstone and removes all secret fields in the
  same logical commit.
- Authenticated HPKE is CryptoKit's X25519/HKDF-SHA256/AES-256-GCM suite on
  iOS 17, including transcript AAD and the RFC 9180 exporter.
- SQLite, WAL, SHM, and recovery files have pre-open directory and post-open
  file-hardening APIs for `NSFileProtectionComplete` and backup exclusion.

Software P-256 exists only for sanitized simulator fixtures. Calling it needs
the Rust `sanitized-development-fixtures` feature, the exact native fixture
gate, a DEBUG Swift build, and an iOS simulator. Any missing gate fails closed.

## Required app integration order

1. Register `noted_apple_security::init()` before mobile setup.
2. Query `protected_data_state()` before opening SQLite. If unavailable, create
   the mobile store in its closed/path-only state and wait for availability.
3. Call `prepare_store_directory()` before the first SQLite open.
4. Open the database, force WAL initialization, then call
   `harden_store_files()` and require `StoreProtectionReport::is_compliant()`.
5. Subscribe to protected-data events. Close SQLite synchronously on
   `Unavailable`; on `Available`, repeat steps 3–4 before accepting reads.
   Re-query state whenever the app enters foreground so event delivery is not
   the sole source of truth.
6. Use `identity_inventory()` to reconcile pending and active Keychain handles
   after a crash or reinstall. Never choose between multiple active identities
   implicitly.
7. Adapt the pairing client's crypto trait to these native operations. Do not
   add JavaScript commands for this plugin.

## Verification

```sh
cargo test --all-features --manifest-path Cargo.toml
cargo check --all-features --manifest-path Cargo.toml --target aarch64-apple-ios
NOTED_APPLE_SECURITY_CORE_TESTS_ONLY=1 swift test --package-path ios
cd ios && xcodebuild -scheme noted-apple-security \
  -destination generic/platform=iOS build CODE_SIGNING_ALLOWED=NO \
  IPHONEOS_DEPLOYMENT_TARGET=17.0
```

The host tests cover the exact fixture gate, authenticated HPKE round trips,
UUIDv7 layout, lifecycle replay/idempotency, discard wiping, and path planning.
They cannot validate Secure Enclave behavior, passcode removal, locked-device
Keychain access, actual Data Protection transitions, or backup contents. Those
remain mandatory physical-device tests, followed by external cryptographic and
implementation review, before personal-data pairing can be enabled.
