# External macOS beta releases

Noted's public beta is a direct-download macOS app. It does not use the Mac
App Store. Official builds use the permanent `com.noted.app` bundle identifier
so app data and macOS privacy grants carry forward from beta to stable releases.

The target tester experience is:

1. Download the universal `Noted.dmg` from a GitHub Release.
2. Drag Noted into Applications and open it.
3. Approve microphone and Screen & System Audio Recording once.
4. Install future signed builds without repeating those permissions.

## One-time Apple setup

The Account Holder for an Apple Developer Program membership must create a
**Developer ID Application** certificate. Export the certificate and private
key from Keychain Access as a password-protected `.p12` file.

Create an app-specific password for notarization and record the Apple Team ID.
The release workflow needs these GitHub Actions repository secrets:

| Secret | Value |
| --- | --- |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_PASSWORD` | App-specific password, not the normal Apple ID password |
| `APPLE_TEAM_ID` | Apple Developer Team ID |
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12` |
| `KEYCHAIN_PASSWORD` | A random password used only for the temporary CI keychain |
| `TAURI_SIGNING_PRIVATE_KEY` | Private updater key stored in Keychain as `com.noted.app.updater-private-key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Updater key password stored in Keychain as `com.noted.app.updater-key-password` |

Encode the certificate without adding line breaks:

```sh
openssl base64 -A -in DeveloperIDApplication.p12 -out certificate-base64.txt
```

Copy the contents of `certificate-base64.txt` into the
`APPLE_CERTIFICATE` secret. Never commit the `.p12`, its password, an Apple
password, or an updater private key.

## Create a beta

1. Update `version` in `src-tauri/tauri.conf.json`.
2. Run `bun run alpha:check` and the Rust test suite locally.
3. Push the release commit.
4. In GitHub Actions, run **macOS beta** against that commit.
5. Download the DMG from the draft release and test it on a clean Mac user.
6. Publish the draft release when capture, permissions, local data, and the
   generated `latest.json` updater manifest have passed the release smoke test.

The workflow builds one universal Apple Silicon + Intel app, signs it with the
Developer ID identity, submits it to Apple's automated notarization service,
and creates a draft beta release. It cannot run successfully until all eight
secrets exist.

## Open-source builds

The signing certificate is not part of the source repository. Contributors
can run `bun install` and `bun run tauri dev` without access to release secrets.
Forks can create ad-hoc local builds; only artifacts produced by the official
release workflow are presented as verified Noted downloads.

## In-app updates

Public beta builds check the latest published GitHub Release on launch and every
six hours while Noted is running. When a newer semantic version is available,
the sidebar and Settings show **Update Noted**. The update is downloaded,
signature-verified, installed, and then Noted restarts. Local development and
the isolated `Noted Alpha.app` build never contact the release feed.

GitHub's `releases/latest` endpoint ignores releases marked as prereleases.
Accordingly, the workflow creates a draft normal release whose name and copy
identify it as beta. Publishing the tested draft advances the beta update
channel; leaving it as a draft keeps it invisible to installed apps.

The updater private key and its password must remain recoverable. They are
stored locally in the macOS Keychain and copied into the two GitHub Actions
secrets above. The public key is intentionally committed in
`tauri.beta.conf.json`. Losing the private key prevents existing installations
from trusting future updates.
