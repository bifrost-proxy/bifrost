# Bifrost CI 修复报告 Round 2

## 根因定位

- 失败用例：`e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh` / `TC-RI-GRANTS-TIME-01`
- 失败现象：执行命令后同一 grant 的 `first_connected_at` 从 `1777557672411` 回退到 `1777557672410`。
- 根因：`crates/bifrost-admin/src/remote_invoke/worker.rs` 的 `handle_grant_created()` 会用 `grant_created` SSE 事件重建 `GrantInfo`。当 SSE 先于 `approve_pairing` 的 HTTP 响应到达时，代码会因为本地 grant crypto 尚未写入而 sleep 500ms；sleep 期间 `approve_pairing` 已把本地 `first_authorized_at` 写成较新的授权成功时间，但 `handle_grant_created()` 恢复后仍把早先构造的 `grant_info.first_authorized_at` 写回 `local_grants` / 持久化文件，覆盖本地稳定值。
- 该路径符合 1ms 回退特征：不是命令执行时写入新的 `now_millis()`，而是较早到达的 relay/SSE 重建对象在延迟后覆盖了本地授权时间。

## 修改清单

- `crates/bifrost-admin/src/remote_invoke/worker.rs`
  - `handle_grant_created()` 在写回 grant 前，如果本地已有同 `grant_id`，保留 existing 运行态字段；在 500ms crypto 等待后再次检查并保留，覆盖 race 窗口。
  - SSE reconnect 的 active grants reconciliation 改为复用同一 helper，避免只保留部分 timestamp。
  - 新增 `preserve_existing_grant_runtime_state()`：
    - `first_authorized_at` 严格沿用 existing。
    - `last_command_at` / `last_used_at` 取最大值，保持单调。
    - `max_calls`、`remaining_calls`、`use_count` 和非 active 状态不被重建对象重置。
  - 新增单元测试 `test_preserve_existing_grant_runtime_state_keeps_first_authorized_at_stable`，覆盖 `1777557672411` vs `1777557672410` 的回退场景。
- `design/remote-invoke-call-args-preview.md`
  - 增补 Grants 时间字段稳定性设计、单元测试/E2E/human_tests 计划。
- `human_tests/remote-invoke.md`
  - 新增并执行 `TC-RI-回归-144`，验证命令执行后 `first_connected_at` 严格不变。
- `human_tests/readme.md`
  - Remote Invoke 用例数更新到 180，并在索引说明中加入首次连接时间严格稳定回归。

## 本地执行摘要

- `cargo test -p bifrost-admin preserve_existing_grant_runtime_state -- --nocapture`
  - PASS：1 passed。
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
  - PASS：`TC-RI-GRANTS-TIME-01`、`TC-RI-GRANTS-TIME-02`、`TC-RI-ARGS-01` 至 `TC-RI-ARGS-05` 全部通过。
  - 关键验证：执行命令前后同一 grant 的 `first_connected_at` 严格相等，`last_command_at` 非空且不早于首次连接时间。
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
  - PASS：`All assertions: total=71 passed=71 failed=0`。
- `cargo fmt --all -- --check`
  - PASS。
- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
  - PASS。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - PASS。
- `cargo test --workspace --all-features`
  - PASS。
- `bash scripts/ci/local-ci.sh --skip-e2e`
  - PASS：fmt workspace、fmt desktop、clippy、workspace test 全部通过。

## 新 commit sha

- 1a9af9d7
