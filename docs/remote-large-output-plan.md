# Bifrost Remote — Large Output / Long-running Stable Channel Plan

Branch: `fix/remote-large-output`  Working worktree: `~/work/github/bifrost-large-output/`

## Goal

Byte-exact, reconnect-safe streaming of arbitrarily large stdout/stderr from
`bifrost remote exec` — including **≥1 GiB binary outputs** and
**multi-hour tasks** — without being killed by:

- the executor's legacy 30s default timeout,
- the relay's 30-minute single-connection hard wall-clock,
- in-memory truncation / corruption (UTF-8 lossy conversion, 64 KiB cap),
- slow consumers causing executor OOM.

## Delivery status (as of this commit)

| PR | Scope | Status | Commit |
|----|-------|--------|--------|
| 1 | Protocol types (`OutputTransport`, `StreamFrame`, `ObjectRef`, `ProtocolFeatures`) | ✅ landed, pushed | `3711a42` |
| 2 | Executor streaming SHA-256 + 6 additive fields on `RemoteInvokeResponse`; inline cap 64 KiB → 4 MiB; read buffer 4 KiB → 64 KiB | ✅ landed, pushed | `38436d6` |
| 3a | Split single `timeout_ms` into `wall_clock_timeout_ms` + `idle_timeout_ms`; add 10 s heartbeat tick branch; `max_idle_ms` on policy | ✅ landed, pushed | `f99480e` |
| 3b-types | `StreamFrame::Stdout/Stderr` gain `offset: u64`; `Heartbeat` gains offsets; new `Reconnect` variant | ✅ landed, pushed | `b2823e3` |
| 3b-executor | Streaming `execute_shell_exec_streaming(cmd, tx: mpsc::Sender<StreamFrame>)` method with backpressure + 27 min reconnect-hint | ⏭ pending |  |
| 4 | worker.rs SessionRing + call_id routing + `Resume` entry | ⏭ pending |  |
| 5 | CLI frame consumption + reconnect + SHA verification | ⏭ pending |  |
| 6 | bifrost-server-v4 relay protocol rework + 28 min `ConnectionExpiring` + DB schema | ⏭ pending (separate repo) |  |
| 7 | Shell Access policy: `max_wall_clock_ms`, `max_output_bytes` hardening, per-caller rate limit | ⏭ pending |  |
| 8 | e2e matrix: 1 GiB binary, 2 h task with ≥4 reconnects, 5× forced kill + resume, idle/wall-clock assertions, OOM-guard | ⏭ pending |  |

## Architectural summary

```
┌──────────┐     shell.exec(call_id=C)      ┌──────────┐        ┌──────────┐
│  CLI /   │ ─────────────────────────────▶│  Relay   │◀──────▶│  Worker  │
│ caller   │                                │ (30 min  │        │ (target  │
│ (tokio)  │◀────── StreamFrame* ───────────│  conn.   │────────│  device) │
└──────────┘        Ack(offset) ───────────▶│  hard    │        └──────────┘
     │                                      │  cap)    │              │
     │                                      └──────────┘              │
     │            Resume(call_id, from_offset)                        │
     └────────────────────────────────────────────────────────────────┘
                on relay disconnect or at ~27 min
```

### Core invariants

1. **Executor never blocks on the network.** It writes to an `mpsc::Sender<StreamFrame>` with bounded capacity (16 × 64 KiB = 1 MiB). `send().await` naturally back-pressures the child read loop when downstream is slow.
2. **Relay never stores more than N MiB per call.** Default 64 MiB `SessionRing`. Older bytes are dropped once acked.
3. **Resume is the receiver's job.** Executor does not retry; it keeps producing frames to its local ring. If the relay drops, the receiver reconnects and issues `Resume(call_id, local_file_offset)`.
4. **27 min < 30 min.** Executor emits `Reconnect{reason: "relay-wall-clock"}` at 27 min so the receiver has time to tear down and re-establish before the relay hard-cap hits.
5. **Byte-exact verification.** CLI streams stdout to disk with `tokio::fs::File`; on `Done` frame it computes SHA-256 of the local file and compares with `stdout_sha256` in the `Done` frame. Mismatch → `exit 2`.

## Remaining PR specifications

### PR #3b-executor (executor.rs, +~200 LOC)

**New method** (keep existing `execute` intact for legacy callers):

```rust
impl RemoteInvokeExecutor {
    pub async fn execute_shell_exec_streaming(
        &self,
        command: &RemoteCommand,
        frame_tx: tokio::sync::mpsc::Sender<StreamFrame>,
    ) -> Result<(), BifrostError>;
}
```

Behavior:
- Resolves policy as in `execute`.
- Spawns child; per 64 KiB read, sends `StreamFrame::Stdout { seq, offset, data_b64 }` through `frame_tx.send().await` (back-pressure point).
- Per 10 s heartbeat tick: `StreamFrame::Heartbeat { ts, stdout_offset, stderr_offset }`.
- At wall-clock = 27 min: emit one `StreamFrame::Reconnect { reason: "relay-wall-clock", stdout_offset, stderr_offset }`; do NOT kill child.
- On child exit: one `StreamFrame::Done { exit_code, total_stdout, total_stderr, stdout_sha256, stderr_sha256, duration_ms, stdout_object: None, stderr_object: None }`.
- On error: `StreamFrame::Error { code, message }` then close sender.

Unit tests to ship with this PR:
- `stream_emits_ordered_frames_with_correct_offsets` — stub child producing 3 × 64 KiB chunks, assert offsets 0, 65536, 131072.
- `stream_done_sha256_matches_concatenation` — verify `Done.stdout_sha256` equals streaming hash of all emitted bytes.
- `stream_backpressure_blocks_on_slow_consumer` — use a capacity-1 channel, start a child producing 10 MiB, assert executor is blocked when receiver is slow (memory flat).
- `stream_reconnect_frame_after_27min` — use `tokio::time::pause()` to fast-forward time, assert exactly one `Reconnect` frame emitted.

### PR #4 (worker.rs, +~400 LOC, behind feature flag)

**Additive types:**

```rust
struct SessionRing {
    call_id: Uuid,
    started_at: Instant,
    stdout_buf: RingBuf,    // 64 MiB default, configurable
    stderr_buf: RingBuf,
    stdout_offset_head: u64,
    stdout_offset_tail: u64,
    // ...
    last_ack_stdout_offset: u64,
    last_ack_stderr_offset: u64,
}
```

**New relay endpoints** (handled in `worker.rs` / routed by admin server):
- `POST /remote/shell/exec/stream` — initiate streaming; returns `call_id`.
- `POST /remote/shell/exec/resume?call_id=C&from_stdout=N1&from_stderr=N2` — replay from ring buffer, then continue live.
- `POST /remote/shell/exec/ack?call_id=C&stdout_offset=N&stderr_offset=M` — frees ring capacity.

**Protocol negotiation:**
- If `ProtocolFeatures::negotiates_large_output()` returns true, worker calls `execute_shell_exec_streaming`; otherwise falls back to legacy `execute`.

**Parallel-dev conflict mitigation:**
- Wrap all new code in `#[cfg(feature = "large_output_v1")]` OR gate by runtime flag `BIFROST_ENABLE_LARGE_OUTPUT=1`. Default OFF until PR #8 e2e passes.
- Do not modify existing `call_history_store` call sites; only read.

### PR #5 (bifrost-cli, +~500 LOC across remote_shell.rs)

- Add `--stream` flag to `bifrost remote exec` (auto-enabled when negotiation says `large_output_v1`).
- Frame consumption loop:
  ```
  while let Some(frame) = rx.recv().await {
      match frame {
          Stdout { offset, data_b64, .. } => {
              // write to tokio::fs::File at end (append);
              // assert file.len() == offset (detect gaps);
              // update streaming SHA-256;
              // every 4 MiB: send Ack.
          }
          Reconnect { stdout_offset, stderr_offset, .. } => {
              // close current connection; open new one with Resume.
          }
          Done { stdout_sha256, .. } => {
              // finalize local SHA-256; compare; exit 0 or 2.
          }
          Error { .. } => exit 3,
          Heartbeat { .. } => reset idle timer,
      }
  }
  ```
- Reconnect-on-disconnect: 2 s exponential backoff, max 5 attempts, then `exit 4`.
- New CLI flags:
  - `--output-file PATH` (default stdout)
  - `--stderr-file PATH`
  - `--max-reconnects N` (default 5)
  - `--cli-idle-timeout-ms N` (default 5 min)
  - `--cli-wall-clock-timeout-ms N` (default none)

### PR #6 (bifrost-server-v4 — **separate repo**)

- Raise `jsonLimit` for `/remote/*/stream` to `Infinity`; stream raw body, not buffered JSON.
- Add `SessionRing` entries keyed by `call_id`, 64 MiB default, TTL 15 min after last activity.
- At 28 min of a single connection: emit `ConnectionExpiring` advisory frame, close SSE cleanly.
- DB migration on `remote_calls` table:
  ```sql
  ALTER TABLE remote_calls ADD COLUMN call_id TEXT UNIQUE;
  ALTER TABLE remote_calls ADD COLUMN last_ack_stdout_offset INTEGER DEFAULT 0;
  ALTER TABLE remote_calls ADD COLUMN last_ack_stderr_offset INTEGER DEFAULT 0;
  ALTER TABLE remote_calls ADD COLUMN total_stdout_bytes INTEGER;
  ALTER TABLE remote_calls ADD COLUMN total_stderr_bytes INTEGER;
  ALTER TABLE remote_calls ADD COLUMN stdout_sha256 TEXT;
  ALTER TABLE remote_calls ADD COLUMN stderr_sha256 TEXT;
  ALTER TABLE remote_calls ADD COLUMN status TEXT; -- running/done/failed/abandoned
  CREATE INDEX idx_remote_calls_call_id ON remote_calls(call_id);
  ```

### PR #7 (policy/grant)

- `ShellPolicy.max_wall_clock_ms: Option<u64>` (hard upper bound; None → unlimited)
- `ShellPolicy.max_output_bytes_total: Option<u64>` (hard byte cap; refuse beyond)
- `Grant.allow_resume: bool` (default true)
- Per-caller byte rate limit (tokio-governor or custom): default 100 MiB/s.
- Grant keep-alive: each `Ack` extends grant TTL by 5 min, cap at 8 h.

### PR #8 (bifrost-e2e)

| Scenario | Setup | Expectation |
|----------|-------|-------------|
| 1 GiB binary stdout | `dd if=/dev/urandom bs=1M count=1024` | CLI file SHA-256 == Done.stdout_sha256 == local source SHA-256 |
| 2 h task, 4 reconnects | `yes` + forced 25-min connection recycle | Exit code 0 (killed after 2 h by wall-clock cap), stdout monotonic |
| 5× TCP kill | Toxiproxy, kill relay conn every 30 s | All bytes present, SHA match, no duplicates |
| idle-timeout | `sleep 6min && echo ok`, `max_idle_ms=5min` | CLI exits with "idle timeout" error in ~5 min |
| idle-timeout passes | same with `max_idle_ms=10min` | Exit 0, stdout "ok\n" |
| executor OOM guard | 1 GiB stdout, consumer paused for 10 s | Executor RSS stays < 32 MiB (back-pressure works) |
| legacy client | CLI without `--stream` hitting new executor | Falls back to legacy 4 MiB inline path |

## Operational notes

- Every `bifrost remote exec` call currently invalidates the SSH grant; PR #7 fixes this. Until then, caller workflow:
  ```
  bifrost remote conn down --all
  bifrost remote conn up --ssh-key ~/.bifrost/remote-device.key
  bifrost remote exec ...
  ```
- The main worktree `~/work/github/bifrost/` is reserved for the call_history_store developer. All large-output work lives in `~/work/github/bifrost-large-output/` on branch `fix/remote-large-output`.
- `SKIP_FRONTEND_BUILD=1` bypasses the TypeScript check during pre-commit when working on Rust-only patches.
