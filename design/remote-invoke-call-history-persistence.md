# Remote Invoke Recent Calls 本地落盘

## 功能模块说明

Remote Invoke 的 `Recent Calls` 是目标客户端本地 Settings 页面展示的审计历史。它必须保留本地解密后的命令摘要、参数预览、执行状态、退出码和 digest，同时不能拖慢服务启动、不能在写入时整文件读写、不能在 worker 内存中常驻历史列表。

## 实现逻辑

`CallHistoryStore` 使用 JSONL 滚动存储，目录位于 `BIFROST_DATA_DIR/admin/remote_invoke_call_history/`。旧整文件 `BIFROST_DATA_DIR/admin/remote_invoke_call_history.json` 不再兼容，发现后直接删除，不迁移、不读取。

- 存储 key 维度：`relay_url + client_instance_id + call_id`
- 每个 `relay_url + client_instance_id` 对应一个 `<client-key>.jsonl`
- 收到 `call_open` 并本地解密命令后，立即 append 一行 `streaming` 快照
- 命令完成、失败或取消时，再 append 一行同 `call_id` 的终态快照
- 写入路径不先读取全量历史；只 append 当前快照并更新轻量 meta
- `RemoteInvokeWorker::new()` 不读取 call history；列表/详情 API 请求到来时才按需读取 JSONL
- worker 内存不保留历史列表；正在执行的 call 仅在 `ActiveCallControl` 中保存临时快照，结束或取消后释放
- 单个 `relay_url + client_instance_id` 最终落盘最多保留 `remote_invoke.max_records`，默认 1000 条；超出或发现坏 JSONL 行时触发 compaction，按 `call_id` 只保留最新快照，再按 `started_at` 删除最旧记录
- `/api/remote-invoke/calls` 支持 `limit` 与 `before` 游标分页；前端 Recent Calls 默认只读取一页

## 依赖项

- `crates/bifrost-admin/src/remote_invoke/types.rs` 中的 `CallInfo`
- 现有 `bifrost_storage::data_dir()` 数据目录约定
- 现有 Remote Invoke worker 的 call lifecycle 更新点

## 测试方案

### 单元测试

- `test_call_history_store_prunes_by_retention_and_max_records`：验证 `max_records` 与保留时间裁剪最旧记录
- `test_call_history_store_clear_for_client_removes_only_current_client`：验证清理只删除当前 relay/client 的记录
- `test_call_history_store_truncates_command_fields_before_persisting`：验证命令相关长文本和 JSON 字符串值落盘前最多保留 120 字符，且原始完整长文本不会写入存储文件
- `test_call_history_store_removes_legacy_json_without_migrating`：验证旧整 JSON 文件直接删除，不迁移
- `test_call_history_store_compacts_bad_jsonl_lines_and_caps_records`：验证坏 JSONL 行会清理，最终只保留上限内有效记录
- `test_call_history_store_hard_caps_configured_max_records_at_1000`：验证旧配置传入超过 1000 的 `max_records` 时仍只落盘最新 1000 条
- `remote_invoke_worker_reads_call_history_only_on_demand`：验证 worker 构造不加载历史，API 请求时才按需读取

### E2E 测试

- 新增 `e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`
- 使用临时 `BIFROST_DATA_DIR`、动态 admin 端口、本地 relay
- 建立 remote connect 后执行 `remote status`
- 读取 `/_bifrost/api/remote-invoke/calls` 确认 call 存在
- 停止并用同一个数据目录重启 Bifrost
- 再次读取 Recent Calls，断言同一个 `call_id` 和 `command_summary.command_preview=status` 仍存在
- 扩展 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- 生成长参数 `remote search` 调用后，断言 `masked_args_json.keyword` 最多 120 字符且保留前缀
- 检查旧 `remote_invoke_call_history.json` 不存在，JSONL 文件不包含完整长参数
- 用同一个数据目录重启 Bifrost 后，按长参数调用的同一个 `call_id` 再次读取 Recent Calls，并断言长字段仍最多 120 字符
- 读取 `/_bifrost/api/remote-invoke/calls?limit=25`，断言最多返回 25 条并在有更多记录时返回 `next_cursor`

### 真实场景测试

- 更新 `human_tests/remote-invoke.md`
- 新增 `TC-RI-回归-136`
- 按文档真实执行：生成调用记录、重启 Bifrost、刷新 Settings Remote Invoke，确认 Recent Calls 未清空

## 校验要求

- `cargo test -p bifrost-admin call_history_store -- --nocapture`
- `cargo test -p bifrost-admin remote_invoke_worker_reads_call_history_only_on_demand -- --nocapture`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/readme.md`
- 不涉及外部 README/API 参数变更
