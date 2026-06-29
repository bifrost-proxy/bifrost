# Remote Invoke v5 PoP — P0 Hardening Smoke Tests

Branch: feat/remote-invoke-v5-pop-hardening

These checks verify the four P0 security fixes in the v5 Proof-of-Possession
authentication flow:

- P0-1: SSH route is bound to the registering user. No cross-user device_code
  hijack is possible.
- P0-2: Once a grant has frozen a caller_ephemeral_pub, any later lookup that
  presents a different ephemeral pubkey is rejected with HTTP 401
  ephemeral_pub_rotation_not_allowed.
- P0-3: SSH approval no longer leaks the long-lived grant_session_token. The
  server mints a single-use claim_token; the CLI must redeem it via PoP at
  POST /v5/remote-invoke/grants/ssh-claim to obtain the real session token.
- P0-4: Pairing caller_fingerprint is derived server-side from caller_pubkey
  with ed25519FingerprintFromBase64. Attacker-supplied caller_info.fingerprint
  is ignored. start_pairing without caller_pubkey is rejected.

## Setup

1. Start a fresh sync-server (sqlite is fine):

   pnpm --filter @bifrost/sync-server dev

2. Register two users (alpha, beta) via /v4/sso/register and capture their
   x-bifrost-token values.

3. Generate two ed25519 long-term client keypairs (alpha-client,
   beta-client) and one caller PoP ed25519 keypair.

## P0-1: SSH route user binding

1. As alpha, register alpha-client with an ssh_device_route whose device_code
   is derived from alphaSshPubPemA via deriveSshDeviceCode.
   Expect: HTTP 200, code 0.
2. As beta, register beta-client with the SAME device_code/public_key_pem from
   step 1.
   Expect: non-200, message contains device_code_owned_by_other_user.
3. Confirm via the routes DAO that the device_code is still owned by alpha.

## P0-2: ephemeral_pub freeze

1. Seed a grant for client-instance C with caller_ephemeral_pub = E1 (32-byte
   base64) via the grants DAO directly OR by completing a pairing.
2. Send POST /v5/remote-invoke/grants/lookup with a PoP envelope whose body
   carries caller_ephemeral_pub = E2 (different 32-byte base64) for the same
   client_instance_id and caller fingerprint.
   Expect: HTTP 401, message ephemeral_pub_rotation_not_allowed.
3. Re-read the grant row; caller_ephemeral_pub must still be E1.
4. Sending another lookup with E1 should succeed and mint a session token.

## P0-3: SSH claim_token redemption

1. Drive a full SSH connect on the CLI:
   bifrost remote connect --client-id <id> --ssh-key <pubkey>
2. Server side: on approval, the SSE ssh_connect_result event must contain
   claim_token + claim_expires_at + grant_id, NOT grant_session_token.
3. Server DB: bifrost_remote_invoke_ssh_claims should contain a row with the
   sha256(claim_token), grant_id, client_instance_id, caller_pubkey_fp,
   expires_at and empty claimed_at.
4. CLI must POST /v5/remote-invoke/grants/ssh-claim with a PoP envelope
   { client_instance_id, claim_token, caller_ephemeral_pub } and receive a
   normal GrantInfo back (grant_session_token encrypted with the caller's
   shared secret). After redemption, claimed_at on the SshClaim row must be
   non-empty.
5. Replaying the same claim_token: expect HTTP 401 claim_token_already_used.
6. After grant_session_expires_at the claim_token must also be refused.

## P0-4: server-derived caller fingerprint

1. POST /v5/remote-invoke/pairings/start without caller_pubkey.
   Expect: non-200, error mentions caller_pubkey or invalid_pair_code.
2. POST start_pairing with attacker-controlled caller_info.fingerprint = 'fake'
   AND a real caller_pubkey. Server must store
   caller_fingerprint = ed25519FingerprintFromBase64(caller_pubkey).
3. Inspect the pairings DAO row and the SSE pairing_offer event — both must
   show the derived fingerprint, never 'fake'.
4. claim flow: a POST /v5/remote-invoke/pairings/claim whose PoP-derived
   fingerprint differs from the stored caller_fingerprint is rejected
   (caller_pubkey_mismatch / caller_fingerprint_mismatch).
