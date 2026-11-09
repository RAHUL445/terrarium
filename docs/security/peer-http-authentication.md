# Peer HTTP authentication — 2026-11-05

Status: implementation and local regression milestone. Not a protocol certification or final release approval. No dependency/Loro change, persisted-key migration, live pairing, credential access, app rebuild/relaunch or device installation occurred.

## Behavior and rollout

Peer HTTP reads/writes now require a signed device request, checked against the relay's current roster. Devices, roster, config, batches and snapshots are covered. Only exact `POST /v1/pair` (existing one-time-code proof) and `GET /v1/challenge` (random challenge, no roster data) are exempt. Google OAuth's separate loopback callback is unchanged by peer authentication.

The client uses existing device signing keys and the space key from Secrets. There are no new saved bearer tokens or membership fields. Old paired records, batches and offline edits are preserved. **Update the host and every HTTP peer together**: old clients cannot read from a new server, and a new client rejects old/unsigned responses. No compatibility fallback opens unauthenticated access. Re-pairing is normally unnecessary when the original keys/space records remain available. Drive/local-folder formats and transport behavior are unchanged; they are not covered by this HTTP request authentication.

Keep reviewed older builds available for rollback. Rolling code back also restores its previous unauthenticated HTTP behavior; do not treat a downgrade as a security-preserving fallback. Do not delete or regenerate identities/space keys to address a protocol-version mismatch.

## Wire contract

1. Client fetches `/v1/challenge`, capped at 1 KiB. Relay issues a base64 32-byte OS-random nonce, stores it for 60 seconds using a monotonic clock, and retains at most 1,024 outstanding challenges. Expired entries are reclaimed; a full cache or RNG failure returns 503 without evicting live challenges. Restart invalidates outstanding challenges. Challenge issuance adds one HTTP round trip per authenticated request.
2. Client calculates SHA-256 over the exact request body (empty for reads). Ed25519 signs the UTF-8 JSON array `["pp-http-request-v1", device_id, space, method, target, nonce, base64(body_digest)]`. `target` includes the `/v1` path and exact query string. Header `x-pp-auth` carries base64 JSON with `device`, `nonce`, `digest`, `signature`; decoded fields have fixed-length checks and the whole encoded header is capped at 1 KiB.
3. Relay checks the nonce is live, the device is in the **current** roster, and the signature is valid. It consumes the nonce only after signature verification and before collecting the bounded body. Wrong space/device/method/path/query, altered header, expired/reused nonce or unknown membership fails. Actual body digest must match before dispatch; a failed body check consumes that challenge and performs no storage action. Retry obtains a new challenge. Existing signed/encrypted envelope verification remains required for uploads.
4. Relay authenticates the dispatched response, including authenticated errors/404s. HKDF-SHA256 with the space key, no salt and info `terrarium peer HTTP response v1` derives a distinct 32-byte response key. HMAC-SHA256 covers UTF-8 JSON `["pp-http-response-v1", base64(canonical_request_message), HTTP_status, base64(SHA256(response_body))]`. Header `x-pp-response` carries its base64 tag. The client bounds the body then verifies the tag in constant time before returning metadata, success, or an optional not-found result. Missing/invalid authentication is an error, never an accepted empty roster/snapshot/config.

The proof header is not a reusable bearer credential. Private signing keys never leave the client. Errors from unauthenticated/capacity/framing failures may be unsigned; the client rejects them as unverified responses. Such errors can affect availability but cannot update the client's roster or establish success. Existing transport policy still disables redirects/environment proxies, restricts cleartext origins to the configured private-address policy, rejects browser Origin requests, and bounds headers/connections/body size/deadlines.

## Pairing-key checks

Both wrapping and unwrapping reject non-contributory X25519 shared secrets. Previously a low-order public key could yield a predictable shared secret; preventing this matters because pairing responses are visible on the private transport. New pairing also validates the device ID/signing key and refuses changes to either signing or exchange key under an existing device ID. The tentative roster is installed only after response/key wrapping succeeds. The joiner verifies that its own roster entry contains its original signing and exchange keys, even when the response's pairing-code MAC is valid.

No existing membership was silently rewritten. The existing roster-save callback still has no durable error result; making pairing/revocation persistence transactional across restarts remains a separate requirement.

## Scope and remaining risks

- This adds peer request authentication and replay rejection, not end-to-end revocation. A removed member fails the relay's next authorization check, including for a previously issued challenge; a request already authorized can finish. The later [host-local blocking milestone](device-revocation.md) adds persisted device blocking. Full key rotation and provider-token revocation remain unfinished.
- Response MACs prove **space-key possession**, not a uniquely pinned host identity or exclusive authority to change the roster. A malicious current/former member with the old space key can forge responses if it controls the network path. Signed authoritative roster epochs, host identity pinning, key rotation/distribution, and Drive access revocation remain required for compromised-device containment.
- Removing a roster entry cannot erase a device's previously downloaded data or old space key. New-data confidentiality after removal needs actual key rotation and transport credential revocation, with offline/recovery compatibility tests.
- Bounded unauthenticated challenges can still be exhausted, denying new requests for up to the TTL. Existing connection limits are not a complete rate-limit/abuse policy. Private-overlay ACLs, storage quotas and target-specific TLS/identity review remain open. Metadata remains plaintext on permitted HTTP; authentication does not encrypt it.
- The protocol and canonical encodings need independent security review before publication. Tests cover intended behavior, not formal cryptographic proof. Physical-device upgrades, full paired desktop/Android flows, large/slow transfer profiling and final signed release artifacts remain untested.

## Verification

Regression coverage includes unsigned metadata routes, real paired-client encrypted sync, unknown and removed roster members, request replay/retargeting/body changes, expired/full challenge state, device-ID binding even with a shared public key, response status/body/path/space/key binding, invalid header sizes, low-order exchange keys, failed pairing without roster mutation, and a code-authenticated roster substituting the joiner's keys. Existing oversized/chunked-body, redirect, shutdown, snapshot, restart, rollback and Drive fallback tests remain enabled.

Recheck from the host root:

```sh
TERRA_PLUGIN_SOURCE=../terrarium-plugins/packages cargo test --locked --workspace --features terrarium-runtime/plugin-integration
cargo clippy --locked --workspace --all-targets --features terrarium-runtime/plugin-integration -- -D warnings
python3 scripts/dependency_exposure.py
python3 -m unittest discover -s scripts/tests -v
```

Use an absolute TERRA_PLUGIN_SOURCE if the test environment requires it. Android arm64 Clippy uses the installed NDK as documented in the main security plan. Final counts/results are recorded there after checks finish. The three existing RustSec findings remain release blockers; no advisory ignores were added.
