# ASR Daily Agent Reliability

## Status

Implementation design for the reliability follow-up to the MOSS 2.x rescue work.

## Problem

The ASR Daily Agent pipeline currently has several independent persistence
boundaries:

- external-device import progress;
- ASR scan and transcription state;
- per-agent processed-document state;
- per-question research outputs;
- IM provider connection state and message logs.

They are individually useful, but they do not form an end-to-end delivery
protocol. A restart or partial failure can therefore produce one of the
following outcomes:

1. a Weixin provider reports `connected` even though it cannot send until a
   fresh inbound message supplies a context token;
2. a failed send is retried with a new provider-side client ID, so the caller
   cannot distinguish a replay from a new delivery;
3. Daily Agent source state is marked processed without recording the report
   bytes or generator contract that produced it;
4. research fan-out starts every question again even if a matching question
   already has a successful result;
5. an unscoped run considers all historical daily inputs and backfills dates
   that predate the task's current processing watermark;
6. auto-run may be queued immediately after an import returns, without a
   durable completion token tying the discovered snapshot to the ASR run.

## Goals

- Make Weixin send readiness explicit and persist its context token encrypted
  at rest.
- Give IM sends a stable idempotency key and persist an outbox record before
  any provider call.
- Make Daily Agent delivery retryable without resending an acknowledged
  payload.
- Reuse successful research results at question granularity.
- Invalidate reports when source bytes, upstream artifacts, or the generation
  contract changes.
- Prevent accidental historical fan-out with a per-agent date watermark while
  preserving explicit single-date backfill.
- Tie automatic ASR follow-up to a completed, durable import snapshot.
- Add fault-injection coverage for failures before send, after provider
  acknowledgement, during token persistence, and between import completion and
  ASR scheduling.

## Non-goals

- Guaranteeing exactly-once delivery across a provider that does not honor a
  client request ID. Bifrost provides replay-safe at-least-once delivery and
  suppresses duplicate calls in its own durable state.
- Changing ASR model execution, diarization, or audio import semantics.
- Automatically researching questions that do not pass the exact recording
  anchor rules.

## Design

### 1. Encrypted Weixin context store and send readiness

`WeixinProvider` receives the Bifrost data directory at construction and owns a
small `WeixinContextStore`.

The store:

- is keyed by `(provider account ID, user ID)`;
- encrypts every token with the local AES-GCM secret key;
- is written atomically with owner-only permissions;
- never exposes decrypted tokens through status or API responses;
- tolerates a missing/corrupt entry by reporting `send_ready=false`.

Inbound polling updates both memory and the encrypted store before the message
is exposed as a successful inbound event. Provider startup loads valid entries
back into memory.

Provider status adds:

- `send_ready`;
- `send_ready_reason` when false.

`connected` continues to describe the long-lived provider connection;
`send_ready` describes whether the configured owner target can currently be
addressed.

### 2. Transactional IM outbox

The messages API accepts an optional `idempotency_key`. Daily Agent always
supplies one derived from:

`task ID + agent ID + date + destination + report SHA-256 + chunk index`.

Before calling a provider, the handler persists an outbox row containing the
key, destination, payload hash, status `pending`, attempt count, and timestamps.
The following transitions are allowed:

`pending -> sending -> sent`

`sending -> pending` on a confirmed provider error

`sending -> uncertain` when the provider may have acknowledged but the local
commit failed

An existing `sent` row returns its stored message ID without a provider call.
Reusing a key with different payload or destination is rejected.

Provider client IDs are derived from the idempotency key instead of generated
randomly. Weixin text is split before the first call, and each chunk receives a
stable child key. A send-ready preflight happens before creating attempts.

The existing message log remains an observability surface; the outbox is the
authoritative replay ledger.

### 3. Daily Agent delivery state

Daily Agent creates one outbox record per text chunk and only sets the existing
`sent` marker after every chunk reaches `sent`. A retry resumes pending chunks
and never calls the provider for acknowledged chunks.

The report itself remains the source of the outbound plain text. Card delivery
is not introduced.

### 4. Question-level research reuse

Each research question gets a deterministic fingerprint over:

- normalized original question;
- source excerpt and background;
- selected runner and context profile;
- research prompt;
- relevant fan-out contract version.

Successful child metadata persists the fingerprint and result SHA-256.
Fan-out reuses a child only when:

- metadata status is successful;
- fingerprint matches;
- the result file exists;
- the stored result hash matches the file.

Changed or corrupt children run again; unchanged successful children are
reused. Failed children remain independently retryable. Rendering the date
index is deterministic from reused and new child results.

### 5. Artifact versioning and invalidation

Processed Daily Agent documents move to state version 2 and record:

- source SHA-256 and length;
- report SHA-256 and length;
- generator contract version;
- normalized agent configuration SHA-256;
- included upstream report hashes;
- run ID and completion time.

A report is `unchanged` only if all recorded fields still match. Old version 1
entries migrate conservatively: their source fields remain readable, but the
missing report/contract fields force one regeneration.

### 6. Date watermark and explicit backfill

Processed state stores a per-agent `date_watermark`.

For an automatic or unscoped run:

- dates newer than the watermark are eligible;
- a date at or below the watermark is eligible only when an already-tracked
  source or dependency fingerprint changed;
- unrelated historical files that were never tracked are ignored.

An explicit date request bypasses the watermark for that date only. Successful
completion advances the watermark monotonically. This keeps intended backfill
possible while preventing a new downstream agent from sweeping the entire
archive accidentally.

### 7. Import completion barrier

External import progress records a `completion_token` computed from the import
run ID and the durable imported/skipped/failed counters after the final progress
write succeeds.

Automatic ASR follow-up is scheduled only after:

1. the import status is durably `completed`;
2. the completion token can be read back and matches;
3. the task is not paused;
4. no ASR run is already active;
5. the token has not already been consumed.

The consumed token is persisted before scheduling. If scheduling fails, the
token returns to retryable state. This creates an observable barrier without
changing manual ASR run behavior.

## Failure handling

- Missing Weixin context: return a send-not-ready conflict without a provider
  call or blind split retry.
- Provider error: leave outbox retryable with the same stable client ID.
- Local failure after provider acknowledgement: mark `uncertain`; reconciliation
  may query the message log or retry with the same provider client ID.
- Corrupt encrypted token: discard only that entry and require a fresh inbound
  message.
- Corrupt research result: rerun only that question.
- Corrupt report: invalidate only the affected agent/date.
- Crash after import completion: the persisted unconsumed completion token is
  eligible for one follow-up scheduling attempt.

## Verification

Unit tests cover:

- encrypted token round-trip, permissions, corrupt entry, and restart reload;
- send-ready status and preflight behavior;
- idempotent replay, payload mismatch, stable chunk IDs, provider error, and
  acknowledgement/local-commit fault injection;
- question fingerprint reuse and single-child invalidation;
- report and upstream hash invalidation plus v1 migration;
- automatic watermark filtering and explicit backfill;
- import barrier read-back, duplicate consumption, and scheduling failure.

E2E tests run with an isolated data directory, disabled tray, disabled sync
login prompt, no system proxy, and a non-production port. They verify that:

- a successful inbound Weixin event makes a restarted provider send-ready;
- a Daily Agent delivery repeated with the same report produces one provider
  send;
- an import completion token causes at most one ASR scheduling request.

The human test document exercises the same visible status and replay cases
through the Admin API.
