# Qwen3-ASR Runtime Fork Integration Plan

## Current Source Snapshot

- Local source mirror: `vendor/qwen3_asr_rs`
- Upstream repo: `https://github.com/second-state/qwen3_asr_rs`
- Checked commit: `3fa673441682350b12da5c21429fea71ce212023`
- MLX C submodule: `vendor/qwen3_asr_rs/mlx-c`
- MLX C commit: `a1290d221f92bd020af805b7d14207eee4ec973b`

This mirror is for source review and patch development only. It should not be added to the Bifrost Cargo workspace until we have a separate decision to vendor and build the runtime from source.

## Findings

`qwen3_asr_rs` already has the right process shape for Bifrost:

- `asr` is a short-lived CLI process.
- `asr-server` is an OpenAI-compatible HTTP server.
- Both macOS MLX entry points call `qwen3_asr_rs::backend::mlx::stream::init_mlx(true)` immediately before `AsrInference::load(...)`.
- `asr-server` stores one long-lived `AsrInference` behind `Arc<Mutex<_>>`, so requests are serialized against the same loaded model.

The missing piece is not MLX C support. The checked `mlx-c` submodule already exports:

- `mlx_set_memory_limit`
- `mlx_set_cache_limit`
- `mlx_set_wired_limit`
- `mlx_clear_cache`
- `mlx_get_active_memory`
- `mlx_get_cache_memory`
- `mlx_get_peak_memory`
- `mlx_reset_peak_memory`

The gap is in `qwen3_asr_rs/src/backend/mlx/ffi.rs`: it declares device, stream, array, op and safetensor APIs, but does not declare the memory/cache/wired/cache-clear APIs. As a result the current release can only rely on MLX defaults.

Inference also allocates several per-request temporary MLX tensors:

- mel spectrogram and audio encoder output
- prompt embeddings with audio injection
- MRoPE cos/sin tensors
- causal mask
- decoder KV cache
- logits and next-token tensors

Those temporaries should be dropped when `AsrInference::transcribe(...)` returns, but MLX may retain freed buffers in its cache. A long-lived `asr-server` therefore needs explicit cache/stat controls after each request, not only a startup memory limit.

## Recommended Integration Model

Do not make `vendor/qwen3_asr_rs` part of Bifrost's normal build. Building MLX from source pulls CMake plus `mlx-c`, is macOS/aarch64-specific, and would make normal Bifrost CI and developer builds slower and more fragile.

Use this model instead:

1. Maintain a small fork of `second-state/qwen3_asr_rs`.
2. Build and publish patched `asr` / `asr-server` release artifacts from that fork.
3. Point Bifrost's runtime installer at the fork release repo after validation.
4. Keep Bifrost's existing external process watchdog as the final safety net.

This preserves our current operational boundary: Bifrost downloads and supervises a runtime binary; it does not become responsible for compiling MLX in every build.

## Runtime Patch Shape

Patch the fork in small, upstreamable units.

### 1. MLX Memory Wrapper

Add FFI declarations in `src/backend/mlx/ffi.rs`:

- `mlx_clear_cache`
- `mlx_get_active_memory`
- `mlx_get_cache_memory`
- `mlx_get_memory_limit`
- `mlx_get_peak_memory`
- `mlx_reset_peak_memory`
- `mlx_set_cache_limit`
- `mlx_set_memory_limit`
- `mlx_set_wired_limit`

Add `src/backend/mlx/memory.rs` wrappers returning `Result<usize>`, with safe names like:

- `set_memory_limit(bytes)`
- `set_cache_limit(bytes)`
- `set_wired_limit(bytes)`
- `clear_cache()`
- `stats() -> MlxMemoryStats`
- `reset_peak_memory()`

### 2. Startup Configuration

Expose both CLI flags and env vars:

- `--mlx-memory-limit-mb` / `QWEN3_ASR_MLX_MEMORY_LIMIT_MB`
- `--mlx-cache-limit-mb` / `QWEN3_ASR_MLX_CACHE_LIMIT_MB`
- `--mlx-wired-limit-mb` / `QWEN3_ASR_MLX_WIRED_LIMIT_MB`

Apply them after MLX device initialization and before `AsrInference::load(...)`.

The order should be:

1. initialize MLX device and default stream
2. apply memory/cache/wired limits
3. log effective MLX stats and limits
4. load model weights

### 3. Per-request Cleanup

For `asr-server`, add request cleanup around `model.transcribe(...)`:

1. run transcription while holding the current model mutex
2. drop all request-local tensors by leaving `transcribe`
3. call `synchronize()`
4. call `clear_cache()` if enabled
5. collect active/cache/peak memory stats
6. reset peak memory for the next request

Make cache clear configurable:

- `--mlx-clear-cache-after-request=true` by default for Bifrost-managed server
- allow `false` for benchmark comparison, because cache reuse may improve speed but can hurt long-run memory stability

### 4. Worker Lifecycle Control

Add optional server lifecycle knobs:

- `--max-requests <N>`: after N successful or failed requests, finish the current response and shut down.
- `--max-peak-memory-mb <N>`: after a request, if MLX peak exceeds N, log and shut down after response.
- `/health` should include optional memory stats when built with MLX.

Bifrost can then restart the managed service cleanly between batches or after memory pressure without losing the current request.

## Bifrost-side Integration

Current Bifrost directory-task integration has an experiment switch so the fastest current path and the conservative isolation path can be compared without code changes:

- `reuse_per_file`: default production path after the 2026-05-18 same-file benchmark, one managed `asr-server` per source file, stopped after the file when Bifrost started it.
- `fork_per_chunk`: conservative isolation baseline, native `asr` CLI per chunk.
- `reuse_server`: one managed `asr-server` per task run.
- `auto`: start with server reuse; startup failure or RTF drift can switch the remaining chunks to fork-per-chunk, while a mid-run server request failure retries the current chunk through fork-per-chunk and schedules a managed server restart for later chunks.
- `compare`: run fork output as canonical and server output as a shadow attempt for the same chunk; persist both metrics and log text hash differences.

Every strategy must emit `ASR chunk metric` logs and persist `chunk_metrics` in `files.json` and metadata so a crash or watchdog kill still leaves enough evidence to attribute the bad run to a strategy, file, chunk, runner, RTF, text hash, server URL, and error.

### 2026-05-19 reuse_per_file server-death recovery

The live `a911c68b0f7a43afa29d1863cc02229a` directory run showed a failure mode specific to `reuse_per_file`: the Bifrost watchdog killed a file-scoped `asr-server` after physical footprint exceeded the configured limit, then the remaining chunks in that file kept calling the same dead port. That turned one server death into dozens of `connect_failed` chunks.

The Bifrost-side mitigation is:

1. keep `reuse_per_file` as the default fast path;
2. when a `reuse_per_file` or `reuse_server` chunk returns a server error, persist the failed server metric with `server_url` and error;
3. retry the current chunk immediately through `fork_per_chunk`;
4. mark the server runner state as `restart_required` so the next server-eligible chunk first stops the stale managed service state and starts a fresh `asr-server` on a new loopback port;
5. if that restart fails, run only the current chunk through `fork_per_chunk`, keep `restart_required=true`, and retry the server restart before the next server-eligible chunk;
6. persist `fallback_reason` as `<strategy> strategy <transport|server> failure; retrying current chunk via fork_per_chunk and scheduling managed ASR server restart for later chunks: <error>`.

This does not raise the memory ceiling. The watchdog remains the protection against MLX/Metal footprint runaway, while the recovery prevents one watchdog kill from becoming a large partial-success hole. Restarts are deliberately serialized at chunk boundaries: Bifrost never starts a replacement `asr-server` while the native/fork fallback for the failed chunk is still running, avoiding a forked `asr` process and server initialization competing for unified memory.

### 2026-05-22 task-scoped server recovery state

`reuse_server`, `auto`, and `compare` acquire a managed server at task-run scope. A server failure in `reuse_server` or `auto` must therefore update a task-scoped `ServerRunnerState`, not a per-file temporary state. Otherwise the current file can fall back, but the next file recreates a fresh state for the same dead `server_url` and repeats connection failures.

The Bifrost-side mitigation is:

1. create one `ServerRunnerState` next to the task-scoped managed server URL for strategies that use a task-lifetime server;
2. pass that mutable state through each file transcription and chunk loop;
3. when a server chunk fails, set `restart_required` and `fallback_reason` on that shared state;
4. make later files and chunks in the same task run restart the managed server before the next server attempt instead of reusing the dead URL;
5. continue using a file-scoped state for `reuse_per_file`, because that strategy intentionally starts and stops a server at file boundaries.

This keeps the attribution accurate: the first server failure remains visible as a shadow metric, the immediate recovery chunk is visible as fork fallback, and later chunks show the new `server_url` if restart succeeds. If restart fails, only that chunk falls back and the next server-eligible chunk tries to restart again.

### 2026-05-22 watchdog unreliable footprint handling

`asr-server` watchdog sampling must distinguish reliable physical footprint samples from RSS-only fallbacks or sampler failures. When macOS cannot provide `physical footprint`, the watchdog may still see RSS, but RSS is not equivalent to Metal/MLX physical footprint and can be unavailable or noisy while the model is otherwise healthy.

The Bifrost-side mitigation is:

1. kill the managed `asr-server` only when a reliable physical footprint sample exceeds the model-aware limit;
2. treat RSS-only fallback samples as advisory evidence and log repeated unavailable physical-footprint samples without killing the server;
3. treat repeated sampler errors as warnings while the process is still alive, rather than killing a service whose peak reliable footprint has not exceeded the limit;
4. keep clearing managed service state only when the process is already gone or when a reliable sample proves the service exceeded the footprint limit.

This avoids turning a transient `physical footprint unavailable` condition into an unnecessary server death, which in turn reduces avoidable chunk fallback and repeated `error sending request for url (.../v1/audio/transcriptions)` metrics.

### 2026-05-24 streaming timeout and server breaker guard

Realtime streaming windows use the plain-text `/v1/audio/transcriptions` path through `call_asr_text_endpoint`. This path must fail quickly enough that a stuck or overloaded `asr-server` cannot stall websocket or microphone consumers for minutes. The request timeout is therefore bounded by `BIFROST_ASR_TEXT_REQUEST_TIMEOUT_SECS`, defaulting to 45 seconds. Whole-file `verbose_json` / text fallback keeps the duration-aware `BIFROST_ASR_SERVER_REQUEST_TIMEOUT_SECS` path so long offline files still have a bounded but larger budget.

Native fork-per-chunk retries also use a tighter default budget for short chunks. `BIFROST_ASR_CHUNK_TIMEOUT_SECS` remains an explicit override, but without it the timeout is `chunk_duration_secs * BIFROST_ASR_TIMEOUT_MULTIPLIER`, clamped by `BIFROST_ASR_MIN_CHUNK_TIMEOUT_SECS` and 120 seconds. Defaults are multiplier `3` and minimum `45` seconds, so 10 second sub-chunks time out in 45 seconds and 30 second chunks time out in 90 seconds. This keeps timeout-triggered bisection responsive without reducing the 30 second chunk size or masking slow-but-healthy long-file server requests.

Managed server recovery also now has a clear circuit breaker:

1. every failed managed-server chunk increments `ServerRunnerState.server_failures` and persists `fallback_reason` on the metric/state;
2. restartable failures still stop the failed process and may retry the current chunk on a fresh managed server;
3. if consecutive failures reach `BIFROST_ASR_MAX_SERVER_FAILURES_PER_FILE` for `reuse_per_file` or `BIFROST_ASR_MAX_SERVER_FAILURES_PER_TASK` for task-scoped server strategies, `force_fork_for_remaining=true` and `restart_required=false`;
4. subsequent chunks use `fork_per_chunk` isolation with a fallback reason that explicitly says `switching remaining chunks to fork_per_chunk isolation`, rather than silently attempting endless server restarts;
5. successful managed-server chunks reset `server_failures=0`, preserving the fast path when the server recovers before the threshold.

Timeout and guardrail failures in `memory_bisect.rs` now share the same bisection semantics: memory-limit kills and chunk timeouts skip same-size retries and split the current chunk into smaller subsegments. Child subsegment timeouts no longer abort the parent chunk immediately; the sibling/result merge path continues and only records a failed chunk when the minimum split boundary still cannot complete. This prevents a single 30 second timeout from becoming a permanent `partial_success` hole when smaller windows can still be transcribed.

The managed `asr-server` fallback path intentionally no longer restarts the server before retrying the current failed chunk. On a server failure, Bifrost stops/marks the failed service, immediately retries the same chunk through `fork_per_chunk`, and only restarts a managed server before a later server-eligible chunk. That ordering avoids concurrent native ASR fallback and server model initialization competing for unified memory.

The service watchdog warning path is rate-limited. Repeated RSS-only advisory samples or sampler errors while the process is still alive are written at most once per minute per warning class, with log lines explicitly containing `process_alive=true`. Reliable physical-footprint samples above the model-aware limit still kill the managed service immediately.

Test coverage:

- unit: `asr_runtime_timeouts_are_bounded_for_short_chunks` covers the 45s streaming text timeout default alongside whole-file bounds;
- unit: `server_failure_recovery_reason_uses_fork_for_current_chunk` covers the current-chunk fork fallback wording and delayed server restart semantics;
- unit: `server_failure_breaker_switches_remaining_chunks_to_fork` covers the breaker state transition;
- unit: `service_watchdog_warning_log_is_rate_limited` covers repeated watchdog warning throttling;
- unit: `reuse_server_failure_threshold_forces_remaining_fork_isolation` exercises `run_chunk_with_strategy` against the simulated `test-error:` managed-server path;
- E2E: `e2e-tests/tests/test_qwen3_asr_runtime_guards.sh` runs the targeted Rust assertions without downloading or starting Qwen3-ASR.

When an existing `partial_success` file is repaired through `POST /api/asr/tasks/<task_id>/files/<file_key>/retry-chunks`, recovered chunks must be merged back into every user-visible artifact:

- replace the placeholder in the transcript `.txt`;
- append and sort recovered timeline segments in `.timeline.json`;
- update file status, `failed_chunks`, and `text_chars` in `files.json`;
- persist retry evidence in the single-file metadata JSON;
- refresh task daily Markdown summaries so CLI/WebUI daily document views do not show stale placeholder text.

For WebUI operations, task detail also exposes `POST /api/asr/tasks/<task_id>/retry-failed-chunks`.
This endpoint does not retry every chunk in a file. It snapshots files that currently have
non-empty `failed_chunks`, creates an in-memory bulk retry state, and spawns a background worker.
The worker waits for the global ASR processing lock, recomputes retryable files after the lock is
acquired, then processes files one by one through the same single-file retry path above. The task
detail response includes `bulk_retry` while the job is queued, running, completed, or failed:

- `status`: `queued`, `running`, `completed`, or `failed`;
- file progress: `queued_files`, `processed_files`, `current_file_key`, `current_source_path`;
- chunk totals: `total_failed_chunks`, `recovered_chunks`, `still_failed_chunks`;
- per-file results with elapsed time, recovered/still-failed counts, persistence warnings, and
  refreshed daily document paths.

The WebUI task detail page shows a `Retry all failed chunks` button whenever
`summary.failed_chunk_count > 0`. The button queues the bulk job and then relies on task-detail
polling to show the queue state. Logs are emitted when the bulk retry is queued, when it starts,
before and after each file, and when it completes, so a process crash still leaves enough evidence
to identify the file and strategy that was active.

### 2026-05-19 ASR jobs module split

`crates/bifrost-admin/src/handlers/asr_jobs.rs` had grown into a single 5k+ line file covering API
routing, scheduling, retry orchestration, runtime strategy selection, chunk transcription,
audio normalization, persistence, and unit tests. The refactor keeps `handle_asr_tasks` and
`ensure_scheduler_started` in the same public module path, but splits the implementation into
small files under `crates/bifrost-admin/src/handlers/asr_jobs/`:

- `state.rs`: persisted task/file/chunk structs, runtime strategy enum, schedule validation, and request/response types.
- `api.rs`: HTTP routing and task-level response handlers.
- `retry.rs`: single-file failed chunk retry plus queued task-level bulk retry state.
- `runner.rs`: scheduler startup, directory task execution, file processing, and per-file runtime lifecycle.
- `chunk_runtime.rs`: chunk planning, runtime strategy dispatch, chunk metrics, server/fork execution, and timeline normalization.
- `memory_bisect.rs`: memory-limit hint merging and recursive chunk bisection.
- `audio_processing.rs`: ffmpeg split/normalize helpers, abortable process execution, and WAV RMS inspection.
- `store.rs`: task/file store persistence, summaries, discovery, output paths, and task run lock handling.
- `tests.rs`: existing ASR jobs regression tests.

This first split intentionally uses `include!` from the original module instead of changing every
helper into cross-module `pub(super)` APIs. That preserves the pre-refactor visibility model and
keeps the behavioral diff mechanical. A later cleanup can replace the includes with regular
submodules once the internal boundaries have settled.

After the fork has release artifacts:

1. Change the runtime release repo from `second-state/qwen3_asr_rs` to our fork, behind a single constant or config field.
2. Start `asr-server` with conservative defaults:
   - 1.7B on 32GB/64GB: memory limit 18 GiB, cache limit 512 MiB, wired limit min(18 GiB, host memory * 0.8)
   - 1.7B on 16GB: host safety cap should reduce wired/memory limit to about 14.4 GiB
   - 0.6B: memory limit 8 GiB, cache limit 512 MiB
3. Keep the current Bifrost physical-footprint watchdog at 18 GiB for 1.7B until the runtime proves stable.
4. Keep directory tasks on fork-per-chunk CLI initially. Do not switch batch workloads back to long-lived server until the fork passes long-run validation.
5. Use the patched long-lived server first for WebUI file upload and service-mode workflows, where the server lifecycle already exists.

## Validation Plan

The patched runtime is not acceptable until it passes these checks with real audio:

1. Build validation:
   - `cargo build --release --no-default-features --features mlx`
   - confirm `asr` and `asr-server` start on Apple Silicon.
2. Memory-limit validation:
   - start with explicit MLX memory/cache/wired values
   - confirm logs report effective limits
   - confirm too-low values fail cleanly instead of hanging.
3. Cache cleanup validation:
   - run repeated 30s chunk requests through `asr-server`
   - compare `clear_cache=true` and `false`
   - confirm active/cache/peak stats are reset and reported.
4. Long-run stability:
   - process `~/Downloads/we` through the patched server path
   - no failed files
   - no physical footprint above the Bifrost guard
   - no progressive RTF degradation across at least 100 chunks.
5. Performance target:
   - 1801s audio must remain under 5 minutes wall time
   - target RTF should stay near the current CLI baseline, about 0.12 on this machine.
6. Lifecycle validation:
   - `--max-requests` exits only after response completion
   - Bifrost detects exit and can restart the server
   - pause/force-pause still kills the whole process group promptly.

## Recommendation

The best path is a forked runtime release plus Bifrost supervisor integration, not vendoring the runtime into Bifrost's main build.

The first implementation milestone should be tiny:

1. Patch `qwen3_asr_rs` to expose MLX memory/cache/wired limits and memory stats.
2. Add cache clear after request in `asr-server`.
3. Build a local patched release.
4. Run the same 30-minute and directory-task benchmarks before changing Bifrost defaults.

Only after that should we decide whether the long-lived server can replace fork-per-chunk for directory tasks. Until the benchmark proves otherwise, fork-per-chunk remains the safer batch path.
