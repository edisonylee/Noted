# Decision 008: Direct pairing and device enrollment

Status: accepted for sanitized-fixture implementation; external cryptographic review required before personal-data use

Date: 2026-08-16

Owners: mobile, data architecture, and security

Implementation status: the sanitized-fixture protocol core is implemented and
deterministically tested. It includes the bounded canonical parser, invitation
and enrollment state machine, scope/capability enforcement, replay and
idempotency behavior, confirmation state, authority rotation, revocation, and
the authorization seam used by the direct-sync logical router. The tests bind
TLS 1.3/no-0-RTT evidence and the exact SPKI pin, but there is no production
TLS/HTTP transport, Apple-backed cryptography or key storage, durable Mac
authority adapter, or personal-data enrollment. No pairing transport or
production key material has shipped.

## Context

The first synchronized iPhone milestone connects directly to the user's Mac. It
must authenticate a Mac discovered on an untrusted local network, enroll one
scoped device without a hosted account, and transfer bootstrap key material
without exposing the old broad Tauri command surface. Discovery is only a hint;
it is never evidence of endpoint identity.

TLS protects a connection but leaves endpoint naming and pinning to the
application. HPKE protects a message but is not, by itself, an enrollment
protocol. This decision therefore fixes the complete transcript, confirmation,
replay, role, and failure rules around those standard primitives.

## Decision

### Protocol and algorithm suite

The initial suite is `noted.direct-pairing.v1`:

- TLS 1.3 is required for every pairing and sync request. TLS 1.2, 0-RTT, and
  unauthenticated plaintext fallback are prohibited.
- The invitation pins the Mac's ephemeral pairing-server P-256 SPKI SHA-256
  digest. Hostnames, IP addresses, Bonjour names, and locally trusted roots do
  not override a pin mismatch.
- Each device has a hardware-backed P-256 ECDSA signing identity when Secure
  Enclave is available. Signatures use the fixed-width IEEE P1363
  representation and are rejected if non-canonical.
- Enrollment key wrapping uses RFC 9180 authenticated HPKE with
  DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, and AES-256-GCM. The HPKE recipient
  key is distinct from the Secure Enclave signing key and is stored as a
  non-synchronizable ThisDeviceOnly Keychain item. No claim is made that an
  X25519 HPKE private key executes inside Secure Enclave.
- Canonical JSON UTF-8 bytes are hashed with SHA-256. Each transcript component
  is domain-separated and length-prefixed before hashing. Free-form JSON,
  locale-sensitive numbers, and unordered maps are not signed directly.
- All identifiers, counters, expiry instants, requested scopes, capabilities,
  library authority generation, roles, and key fingerprints are covered by the
  transcript and final signatures.

Algorithm identifiers are carried explicitly. Unknown suites or a downgrade
from an offered supported suite fail closed; clients never negotiate by silently
choosing the weakest overlap.

### Key roles and local protection

Keys are separated by purpose:

- `device-signing`: signs enrollment receipts, mutations, checkpoints, and
  revocation acknowledgements. On iPhone this is a Secure Enclave P-256 key when
  available.
- `device-hpke`: receives enrollment and epoch wrappers. On iPhone it is an
  X25519 key stored with `kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly`.
- `transport-session`: ephemeral TLS traffic keys owned by the TLS stack.
- `library-data`: the existing-library encryption root delivered only after
  explicit verification. It is wrapped at rest and is not a signing key.
- `background-receive`: a future M10 key may use
  `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`, but it may decrypt only an
  opaque transport inbox. It cannot unlock the plaintext Notes database.

The iPhone plaintext database and decrypted media use complete file protection
and are unavailable while locked. Background delivery may durably store
ciphertext in a separately protected inbox; foreground unlock performs
validation and application. Reinstall creates a new device identity. Restored
database files do not resurrect ThisDeviceOnly keys and must re-enroll or follow
the recovery protocol.

### Invitation

The Mac creates one pending invitation containing:

- protocol and suite identifiers;
- a random 256-bit invitation nonce and UUIDv7 invitation ID;
- Mac ephemeral pairing-signing and HPKE public keys;
- the pairing TLS SPKI digest;
- library ID and authority generation;
- requested record-kind/scope ceiling;
- creation and expiry instants, with a maximum five-minute lifetime; and
- a nonce-bound invitation signature from the current Mac authority device.

The QR is a data payload, not a URL. It contains no reusable bearer credential,
vault key, database path, account token, or sync session. The Mac stores only a
hash of the invitation nonce after rendering it. An invitation is single-use,
expires durably, and is invalidated on success, cancellation, five failed
attempts, authority-generation change, or app restart unless the user explicitly
keeps the pairing sheet open.

### Enrollment transcript

1. The iPhone scans the invitation, validates its structure, time window,
   authority signature, and supported suite, then connects using TLS 1.3 with
   the exact SPKI pin.
2. The iPhone sends `ClientHello`, containing the invitation ID and nonce proof,
   a fresh 256-bit client nonce, proposed device ID and display name, signing
   and HPKE public keys, requested scopes, per-kind reader/writer capabilities,
   app/build version, and a proof-of-possession signature.
3. The Mac atomically consumes the invitation attempt, validates every bound
   field and capability, chooses scopes no broader than the invitation ceiling,
   and returns `ServerHello` with a fresh server nonce, an authenticated-HPKE
   encapsulated key and challenge, the full proposed enrollment receipt, and a
   Mac proof-of-possession signature. The iPhone authenticates the HPKE sender
   with the invitation's Mac HPKE public key.
4. Both sides derive an eight-digit verification string from the authenticated
   HPKE exporter secret produced by that same sender context and the complete
   transcript digest. RFC 5869 HKDF-SHA256 uses the exporter as IKM, the
   transcript digest as salt, and the domain-separated
   `noted.direct-pairing.v1/sas-hkdf-info` counter as expand info. An unbiased
   64-bit rejection sampler maps the output to eight decimal digits. The code
   is displayed as `1234 5678`; it is never accepted automatically.
5. The user confirms that both devices show the same code and intended scopes.
   Either rejection cancels the invitation. Codes are compared by the user, not
   sent back as authentication values.
6. The Mac creates a pending device-registry row and HPKE-wraps the scoped
   bootstrap key package. The package is authenticated to the transcript,
   receipt ID, library ID, device ID, scope IDs, and authority generation.
7. The iPhone validates and stores the key package, then signs `ClientFinish`
   over the receipt and a canonical digest binding both the HPKE encapsulated
   key and ciphertext.
8. The Mac verifies `ClientFinish` and atomically marks the device active. Its
   signed `ServerFinish` is the enrollment receipt. The iPhone becomes paired
   only after verifying it; an interrupted pending enrollment is safe to retry
   with the same exact bytes or cancel, never to reinterpret.

The receipt includes a protocol version, receipt ID, library/device IDs, both
key fingerprints, granted scopes, per-kind capabilities, authority generation,
created/expiry times, invitation ID, transcript digest, and both role labels.
Role labels prevent reflection. Library and environment bindings prevent a
receipt from being replayed into another library or debug/production boundary.

### Replay, idempotency, and concurrency

- Invitation nonces and receipt IDs have durable state: pending, consumed,
  active, cancelled, expired, or revoked.
- Reusing an ID with byte-identical signed content returns the prior result.
  Reusing it with different content is a security error and quarantines the
  attempt.
- At most one enrollment transition may consume an invitation. Two simultaneous
  scans race through one immediate SQLite transaction; one succeeds and the
  other receives `invitation_consumed`.
- Device transaction counters begin only after activation and are reserved in
  the same transaction as the durable outbox. Restored data with missing device
  keys receives a new identity rather than reusing a rolled-back counter.
- Pairing requests and responses have strict byte, nesting, string, member, and
  decompression limits. Unknown required fields and malformed signatures fail
  closed without advancing enrollment state.

### Direct sync boundary

An enrolled device receives a scoped, short-lived session over pinned TLS. The
server exposes only versioned `/sync/v1` negotiation, bootstrap, push, pull,
checkpoint, acknowledgement, and bounded blob-chunk operations. It cannot route
Tauri commands, read arbitrary paths, configure providers, invoke models, or
permanently delete data. Credentials never appear in query strings, QR URLs,
logs, analytics, clipboard content, or browser storage.

Bonjour or Network.framework discovery may advertise a nonsecret service ID.
The client still requires its enrolled pin and signed device challenge. Manual
IP entry follows the same authenticated path.

### Revocation and limitations

The Mac authority can revoke an enrolled device while direct mode is active.
Revocation immediately rejects new sessions and mutations from that identity.
Because a previously enrolled device may retain historical plaintext and keys,
revocation does not claim retroactive erasure. Future-confidentiality requires
the key-epoch and per-record rekey rules in the managed-relay milestone.

There is no account recovery in direct-only M4. Losing both the Mac authority
and an unexported phone branch can lose that branch. The UI must state this and
offer pending-branch export before device reset or revocation.

## Failure behavior

- Pin, signature, transcript, role, environment, scope, expiry, or capability
  mismatch: abort and retain no active enrollment.
- User code mismatch or rejection: cancel and destroy ephemeral secrets.
- Crash before Mac activation: retain a bounded pending receipt that can only be
  completed by the same signed transcript.
- Crash after Mac activation but before iPhone receipt storage: replaying the
  identical finish returns the signed receipt.
- Clock disagreement: invitation validity uses the Mac authority clock plus a
  bounded skew allowance; nonce consumption and monotonic attempt counts still
  prevent replay.
- Keychain/Secure Enclave loss: mark local enrollment unusable and require
  revocation plus new enrollment; never silently generate a key under an old
  device ID.

## Required verification before personal data

- Cross-language golden vectors for canonical transcript bytes, digest,
  signatures, HPKE authenticated mode, associated data, SAS, and receipt.
- Wrong pin, MITM discovery, reflected roles, changed scope, downgraded suite,
  expired/replayed invitation, simultaneous scans, and byte-different ID reuse.
- Crash/restart at every pending/active boundary and an idempotent finish retry.
- Reinstall with surviving app data, restored backup without ThisDeviceOnly
  keys, device-lock behavior, and explicit export/revoke recovery.
- Fuzzing and parser/resource limits for every pairing and sync envelope.
- External review of this design and the implementation. Until that review is
  closed, only generated or sanitized fixture libraries may cross the channel.

### Current checkpoint

The implemented state machine intentionally accepts only the
`sanitized_fixture` library data class. Its HPKE provider contract now returns
the encapsulated key, ciphertext, and exporter from one sender operation;
challenge signatures and bootstrap digests bind the complete envelope. Sync
transactions are finalized before canonical member signing bytes are exposed,
so a native signer never signs a placeholder manifest or an ambiguous textual
hash. Deterministic tests cover stable transcript/SAS fixtures, complete HPKE
envelope binding, two-stage transaction signing, restart and idempotent finish,
changed-content replay,
simultaneous scans, expiry and attempt limits, scope/role/environment/library/
authority/pin binding, revocation, malformed inputs, and parser resource limits.
The direct-sync logical core separately verifies the exact route set, request and
response signatures, enrolled-device scopes, TLS/pin evidence, bounded bodies,
checkpoint-bound paged bootstrap, bootstrap/checkpoint consistency, replay,
stale heads, and response limits. Transaction admission proves that every
accepted transaction can later fit in a pull response, and revocation is
serialized with in-flight requests so a completed write linearizes either
before revocation or is rejected after it. The pairing coordinator is not
cloneable around that revocation gate.

This checkpoint does not relax the fixture-only gate. Before personal data is
eligible, the production implementation still needs:

- CryptoKit/Secure Enclave and ThisDeviceOnly Keychain adapters for the roles
  defined above, plus the required cross-language signature/HPKE/SAS vectors;
- a real pinned TLS 1.3 client/server adapter with 0-RTT disabled and no
  plaintext fallback;
- durable Mac storage and atomic transitions for invitations, receipts, device
  registry, counters, revocation, authority generation, and restart recovery;
- physical-device Data Protection, locked-device, reinstall, backup, and key-loss
  probes; and
- external cryptographic and implementation review followed by an end-to-end
  sanitized physical-device run.

## Consequences

This protocol adds deliberate confirmation and a narrow device registry rather
than treating local-network reachability as trust. Separate signing, wrapping,
transport, and data keys make their lifecycles auditable. The cost is more state
and explicit recovery UX; that complexity is required to keep direct pairing
compatible with the later relay without making the old broad phone bridge part
of the product.

## Standards and platform references

- [RFC 9180: Hybrid Public Key Encryption](https://www.rfc-editor.org/info/rfc9180/)
- [RFC 9846: TLS 1.3](https://www.rfc-editor.org/info/rfc9846/)
- [RFC 5869: HKDF](https://www.rfc-editor.org/info/rfc5869/)
- [Apple CryptoKit HPKE](https://developer.apple.com/documentation/cryptokit/hpke)
- [Apple Secure Enclave P-256 key agreement](https://developer.apple.com/documentation/cryptokit/secureenclave/p256/keyagreement)
- [Apple Keychain accessibility guidance](https://developer.apple.com/documentation/security/restricting-keychain-item-accessibility)
- [Apple complete file protection](https://developer.apple.com/documentation/foundation/fileprotectiontype/complete)
