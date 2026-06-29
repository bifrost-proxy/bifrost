---
title: "Remote Invoke v5 PoP Hardening"
description: "Automatically synced English Remote Invoke v5 PoP Hardening documentation from docs-en/design/remote-invoke-v5-pop-hardening.md."
editUrl: false
sidebar:
  label: "Remote Invoke v5 PoP Hardening"
  order: 936
---

> This page is automatically synced from `docs-en/design/remote-invoke-v5-pop-hardening.md`.
# Remote Invoke v5 PoP Hardening

> Status: implemented on the `feat/remote-invoke-v5-pop` branch.
> Scope: P0-1, P0-2, P0-3, P0-4, and their regression tests.

This document records the English counterpart for the Remote Invoke v5
Proof-of-Possession hardening plan. The implementation moves caller-sensitive
Remote Invoke operations away from caller-supplied identifiers and onto
Ed25519 PoP-bound sessions.

## Goals

1. Caller-sensitive relay operations must be bound to a caller Ed25519 public
   key fingerprint, not to self-reported caller identifiers alone.
2. SSH public-key authorization must not bypass v5 PoP. SSH approval creates a
   one-time claim that must be redeemed through the v5 claim flow.
3. Pairing and grant events must not leak reusable long-lived grant identifiers.
4. Multiple pairing watchers must be supported without exposing reusable grant
   credentials over SSE.

## P0-1: SSH Route Ownership

SSH device-code routes are scoped to the registered client user. Route updates
from a different user are rejected instead of replacing an existing device-code
route. This prevents one account from hijacking another account's SSH pairing
route.

Validation coverage:

- route registration carries the server-side user id;
- heartbeat route refresh uses the persisted client record owner;
- attempts to reuse a device code across users are rejected;
- affected SSH grants are revoked when a route changes.

## P0-2: Ephemeral Transport Binding

Grant lookup freezes the caller ephemeral X25519 public key for the lifetime of
the grant. A later lookup with a different caller ephemeral key is rejected.

This prevents a caller from rotating transport context under an existing grant
and confusing encrypted open-call payload handling.

Validation coverage:

- first lookup may materialize the caller ephemeral key;
- later lookups must use the same caller ephemeral key;
- lookup returns the client ephemeral key required for open-call encryption;
- nonce replay and canonical JSON checks remain enforced.

## P0-3: Grant Session Tokens

Remote Invoke v5 uses short-lived `grant_session_token` bearer tokens for
caller operations. Pairing and SSH approval paths issue one-time claim tokens;
the caller redeems those claims with PoP and receives the short-lived session
token.

Open-call requests must include:

- `Authorization: Bearer <grant_session_token>`;
- a PoP-signed canonical JSON request body;
- encrypted command payloads bound to the saved transport context.

Validation coverage:

- claim tokens are one-time-use;
- session tokens are hashed at rest;
- revoked, expired, or mismatched session tokens are rejected;
- CLI callers refresh an expired or invalid session token through v5 lookup.

## P0-4: SSE Grant Leakage

SSE approval events no longer include reusable grant identifiers or transport
keys. Pairing watchers receive only the data required to claim authorization.
The caller must then complete the v5 claim flow with PoP.

Validation coverage:

- multiple watchers can observe the same pending pairing;
- approval events omit long-lived grant ids and caller fingerprints;
- watch tokens gate the pairing watch stream;
- legacy v4 caller-sensitive endpoints return `410 protocol_version_not_supported`.

## Rollback Notes

This is an intentional v5 security break. A rollback must restore the previous
Remote Invoke router and schema together; mixing v4 caller behavior with v5
token semantics is not supported.

Recommended rollback sequence:

1. Stop the sync server.
2. Drop or restore the `bifrost_remote_invoke_*` tables to the previous schema.
3. Restore the previous Remote Invoke router and CLI caller implementation.
4. Restart the sync server and verify only the matching caller protocol version.
