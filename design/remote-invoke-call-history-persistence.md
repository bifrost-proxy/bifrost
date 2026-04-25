# Remote Invoke Recent Calls 本地落盘

## 功能模块说明

Remote Invoke 的 `Recent Calls` 是目标客户端本地 Settings 页面展示的审计历史。它必须保留本地解密后的命令摘要、参数预览、执行状态、退出码和 digest。此前这部分只存放在 `RemoteInvokeWorker.call_history` 内存队列中，Bifrost 进程重启后队列重新初始化，导致页面显示 `No recent calls`。

## 实现逻辑

新增 `CallHistoryStore`，文件位于 `BIFROST_DATA_DIR/admin/remote_invoke_call_history.json`，沿用 grant store 的版本化 JSON 文件模式：

- 存储 key 维度：`relay_url + client_instance_id + call_id`
- 收到 `call_open` 并本地解密命令后，立即写入 `streaming` 记录
- 命令完成、失败或取消时，更新同一条记录的终态字段
- `RemoteInvokeWorker::new()` 启动时按当前 relay 和 client instance 恢复
- `relay_url` 变更时重新加载对应 relay 的历史，避免跨 relay 混用
- 单个 `relay_url + client_instance_id` 最多保留 `remote_invoke.max_records`，超出时按 `started_at` 删除最旧记录
- 重启时非终态记录恢复为 `failed`，补齐 `exit_code=-1`、`ended_at` 和 `duration_ms`，避免 UI 永久显示执行中

## 依赖项

- `crates/bifrost-admin/src/remote_invoke/types.rs` 中的 `CallInfo`
- 现有 `bifrost_storage::data_dir()` 数据目录约定
- 现有 Remote Invoke worker 的 call lifecycle 更新点

## 测试方案

### 单元测试

- `test_call_history_store_prunes_by_retention_and_max_records`：验证 `max_records` 与保留时间裁剪最旧记录
- `test_call_history_store_clear_for_client_removes_only_current_client`：验证清理只删除当前 relay/client 的记录
- `test_call_history_store_truncates_command_fields_before_persisting`：验证命令相关长文本和 JSON 字符串值落盘前最多保留 120 字符，且原始完整长文本不会写入存储文件
- `test_finalize_non_terminal_restored_calls_marks_streaming_failed`：验证重启恢复时 streaming 记录收敛为 failed

### E2E 测试

- 新增 `e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`
- 使用临时 `BIFROST_DATA_DIR`、动态 admin 端口、本地 relay
- 建立 remote connect 后执行 `remote status`
- 读取 `/_bifrost/api/remote-invoke/calls` 确认 call 存在
- 停止并用同一个数据目录重启 Bifrost
- 再次读取 Recent Calls，断言同一个 `call_id` 和 `command_summary.command_preview=status` 仍存在
- 扩展 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- 生成长参数 `remote search` 调用后，断言 `masked_args_json.keyword` 最多 120 字符且保留前缀
- 检查 `remote_invoke_call_history.json` 不包含完整长参数
- 用同一个数据目录重启 Bifrost 后，按长参数调用的同一个 `call_id` 再次读取 Recent Calls，并断言长字段仍最多 120 字符

### 真实场景测试

- 更新 `human_tests/remote-invoke.md`
- 新增 `TC-RI-回归-136`
- 按文档真实执行：生成调用记录、重启 Bifrost、刷新 Settings Remote Invoke，确认 Recent Calls 未清空

## 校验要求

- `cargo test -p bifrost-admin call_history_store -- --nocapture`
- `cargo test -p bifrost-admin finalize_non_terminal_restored_calls -- --nocapture`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/readme.md`
- 不涉及外部 README/API 参数变更
