# Remote Invoke Recent Calls 参数预览回退

> **状态**（2026-07-03）：已交付。本节列出的所有 client 侧回退、落盘、清理、120 字符截断、Grants 时间字段稳定性、Web UI 三段布局与详情弹窗均已在 `bifrost-remote-fixes-on-tray` HEAD 上落地并被单元测试 + E2E 覆盖。参见文末「实现状态对齐」。

## 背景

Remote Invoke 的 `openCall` 已升级到密文链路，relay 不再持久化明文 `command_summary`。当前 client 侧 `Recent Calls` 标题区域仍优先依赖 `command_summary.masked_args_json` 渲染参数预览，导致加密链路下即使本地已经解密并保存了 `command.args_json`，Web UI 依然显示不出命令参数详情。

用户可见现象：

- `Recent Calls` 只显示命令名与状态，不显示参数预览。
- Hover Tooltip 也没有完整参数 JSON。
- `bytes_out` 仍能显示，说明并非整条调用记录缺失，而是参数摘要字段为空。

## 用户目标验证清单

### 必须实现

- `Recent Calls` 在加密链路下继续展示可读参数预览。
- 优先复用已有 `command_summary.masked_args_json`。
- 若 relay 未下发该字段，则 client 本地使用已解密的 `command.args_json` 补齐。
- `Recent Calls` 本地落盘，Bifrost 重启后仍能恢复最近记录。
- 支持一键清理当前客户端的全部 Recent Calls。
- 本地记录默认保留 90 天，单个 relay/client 最多保留 1000 条（`CALL_HISTORY_HARD_MAX_RECORDS` 与 `RemoteInvokeConfig.max_records` 双端硬上限均为 1000）。
- 命令相关文本超过 120 字符时直接截断，只保留前 120 字符用于展示和落盘。
- Grants API `first_connected_at` 严格稳定，不会因命令执行、SSE 重连、`grant_created` 补偿事件而波动。

### 必须不破坏

- 不改变 connect 等无参数命令的展示行为。
- 现有 `remote-invoke/status` / `remote-invoke/grants` / `remote-invoke/policies` API 契约。
- Web UI 已存在的 Recent Calls hover、状态标签、caller/policy/exec mode 显示。
- 现有单测：`build_call_command_summary` / `preserve_existing_grant_runtime_state` / `CallHistoryStore` 测试族。

### 必须真实验证

- `cargo test -p bifrost-admin` 中 `test_build_call_command_summary_*` / `test_preserve_existing_grant_runtime_state_*` / `test_call_history_store_*` 全绿。
- E2E `test_remote_invoke_recent_calls_args_preview_e2e.sh` 与 `test_remote_invoke_recent_calls_persistence_e2e.sh` 全绿。
- Human tests 记录真实浏览器验证：加密链路下 Recent Calls 展示参数、重启后不丢失、清理有效。

## 产品语义

### 参数预览的三级回退

1. `call.command_summary.masked_args_json`（relay 下发，加密链路下通常为空）。
2. `call.command.args_json`（client 本地解密后已保存）。
3. Web UI 显示占位（如无参数命令）。

前两级由 client `build_call_command_summary` 保证，第三级由 Web `getCallArgsPreviewSource` 保证。

### 120 字符截断策略

写入本地 JSONL 前统一执行 120 字符截断：

- `command_summary.command_preview`
- `command_summary.masked_args_json`
- `command.command`
- `command.args_json`
- `command.query` 中的字符串字段
- `command.argv`、`command.shell`、`command.command_text`、`command.cwd`、`command.env`
- `policy_id`

截断逻辑保留可识别前缀并附带原始长度后缀；API、Web UI、落盘文件看到同一份截断后的内容。对 `args_json` / `masked_args_json` 这类 JSON 字符串，截断发生在 JSON 内部的字符串值上，序列化后的 JSON 仍保持合法。

### `first_connected_at` 的稳定性

`first_connected_at` 来自 `GrantInfo.first_authorized_at`，语义是 grant 生命周期内的首次授权/连接时间。以下任一路径都必须保留 existing 值：

- `approve_pairing` 本地授权成功。
- `grant_created` SSE 补偿事件（早于 crypto 落盘时短暂等待后仍要保留 local 值）。
- Relay payload 重建 `GrantInfo` 时如果发现本地已有同一 `grant_id`。

`last_command_at` / `last_used_at` 取更大的时间戳，保持单调；`max_calls` / `remaining_calls` / `use_count` / 非 active 状态不重置。

## 技术细节

### Client Worker（`crates/bifrost-admin/src/remote_invoke/worker.rs`）

- `build_call_command_summary`（第 3760 行）：保留现有 `command_preview` 回退；新增 `masked_args_json` 回退——若 relay 下发的 `masked_args_json` 非空则保持原值，否则使用本地解密后的 `RemoteCommand.args_json`，空字符串按缺失处理。
- `preserve_existing_grant_runtime_state`（第 4549 行）：所有会用 relay payload 重建 `GrantInfo` 的 client 侧同步路径（`worker.rs:1366 / 1928 / 2201 / 2209`）都会调用它，保留 first_authorized_at 严格不变、last_command_at 单调、grant_mode 不被重置。
- Recent Calls 由 `handle_call_open`（`worker.rs:2906`）触发写入。

### CallHistoryStore（`crates/bifrost-admin/src/remote_invoke/call_history_store.rs`）

- 落盘目录：`BIFROST_DATA_DIR/admin/remote_invoke_call_history/`。
- 存储维度：`relay_url + client_instance_id + call_id`。
- 收到 `call_open` 并完成本地解密后 append 一行 `streaming` 快照。
- Completed / failed / cancelled 终态再 append 一行同 `call_id` 的最新快照。
- Worker 启动和 relay_url 切换时不恢复历史；Recent Calls API 按需读取 JSONL。
- Worker 内存不保留历史列表；正在执行的 call 只保留临时快照，执行结束后释放。
- 旧 `BIFROST_DATA_DIR/admin/remote_invoke_call_history.json` 不兼容读取，发现后直接删除。
- `RemoteInvokeConfig.retention_days` 默认 90 天；`max_records` 默认 1000 条（`types.rs:289-294`）；`CALL_HISTORY_HARD_MAX_RECORDS` 硬上限 1000（`call_history_store.rs:25`）。
- 坏 JSONL 行由 compaction 清理。

### Web UI

- `web/src/api/remoteInvoke.ts:210` `getCallArgsPreviewSource` 抽取参数预览来源。
- `web/src/pages/Settings/tabs/RemoteInvokeTab.tsx:2848-3088` 标题预览、Tooltip、详情弹窗共用同一来源。
- Recent Calls 卡片右上角清理按钮调用 `DELETE /_bifrost/api/remote-invoke/calls`。
- 每条记录改为稳定的三段布局：
  1. 左侧：命令摘要 + 参数预览，占据剩余宽度并允许收缩。
  2. 中间：状态、caller、policy、exec mode 短标签，不被长命令挤压。
  3. 右侧：详情按钮打开完整记录弹窗。
- 命令摘要、参数预览、caller、policy、exec mode 均单行 `ellipsis` 截断。
- 点击详情调用 `GET /_bifrost/api/remote-invoke/calls/{call_id}`；失败时回退当前列表记录。
- 详情弹窗展示命令、参数 JSON、调用 ID、grant/client/caller、状态、耗时、流量、policy/exec mode、命令详情 JSON；长文本最多 120 字符。

## CLI

Recent Calls 数据由 Admin API 读取，CLI 无独立子命令直接访问该视图；`bifrost status --tui` 的 Remote Invoke tab 会通过 API 拉取展示 Recent Calls。

## Web

见「技术细节 → Web UI」。

## Admin API

- `GET /_bifrost/api/remote-invoke/calls?limit=N&before=<ts>`（`handlers/remote_invoke.rs:90`）：
  - `limit` 默认 100，clamp 到 1..=200。
  - `before` 是毫秒 `started_at` 时间戳。
  - 返回体：`{ calls, next_cursor, limit }`；`next_cursor` 是当前页最早一条的 `started_at` 毫秒。
- `GET /_bifrost/api/remote-invoke/calls/{call_id}`：单条详情。
- `DELETE /_bifrost/api/remote-invoke/calls`（`handlers/remote_invoke.rs:635` 起 `handle_calls_list`）：清空当前 relay/client 的全部本地记录，返回 `{success, removed}`。

## Sync 边界

- Recent Calls 属本地历史，绝不参与账号 sync。
- Grant runtime 字段（`first_authorized_at` / `last_command_at` / `last_used_at`）在 client 内保持稳定，不因 relay 补偿事件被覆盖。

## Phase 1：Client 侧参数摘要回退（已完成）

- `build_call_command_summary` 新增 `masked_args_json` 回退。
- `preserve_existing_grant_runtime_state` 覆盖所有 grant 重建路径。

## Phase 2：本地落盘 + 清理 + 截断（已完成）

- `CallHistoryStore` 独立模块。
- 120 字符截断策略统一应用于 API / Web UI / JSONL。
- `DELETE /_bifrost/api/remote-invoke/calls` 清理。
- 旧 `remote_invoke_call_history.json` 启动时删除。

## Phase 3：Web UI 三段布局 + 详情弹窗（已完成）

- `RemoteInvokeTab.tsx` 三段布局。
- 详情弹窗调 `calls/{call_id}`。

## Phase 4：观测扩展（规划中）

- Cross-relay Recent Calls 汇总视图。
- 状态：(planned, not yet shipped as of 2026-07-03)。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/remote_invoke/worker.rs`：
  - `test_build_call_command_summary_falls_back_to_decrypted_command_when_preview_blank`（第 5571 行）。
  - `test_build_call_command_summary_keeps_relay_masked_args_json_when_present`。
  - `test_preserve_existing_grant_runtime_state_keeps_first_authorized_at_stable`（第 5172 行）。
  - `test_preserve_existing_grant_runtime_state_keeps_transport_identity_when_sync_omits_it`（第 5200 行）。
  - `test_preserve_existing_grant_runtime_state_keeps_ssh_key_grant_mode`（第 5225 行）。
- `crates/bifrost-admin/src/remote_invoke/call_history_store.rs`：
  - `test_call_history_store_truncates_long_command_fields`。
  - `test_call_history_store_clear_for_client`。
  - `test_call_history_store_drops_legacy_json_file`。
  - `test_call_history_store_compaction_removes_bad_jsonl_lines`。
  - `test_call_history_store_respects_retention_and_max_records`。
- `web/src/api/remoteInvoke.test.ts`：
  - `getCallArgsPreviewSource` 优先使用 `masked_args_json`。
  - 缺失时回退到 `command.args_json`。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_e2e.sh`：`remote search` 后 `/api/remote-invoke/calls` 中对应记录的 `command_summary.masked_args_json` 非空，含 `query`/`max_results`/`max_scan`。
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`：加密链路下参数预览回退可见。
- `e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`：
  - Recent Calls 写入 `admin/remote_invoke_call_history/*.jsonl`。
  - 超长参数在 API 中只返回前 120 字符，且 `masked_args_json` 仍是合法 JSON。
  - 旧 `remote_invoke_call_history.json` 不存在。
  - 重启后同一 `call_id` 仍可读取。
  - `limit` 1..=200 clamp 与 `before=<ts>` 分页正确。
  - DELETE 后 API 返回空列表。
  - 命令执行前后 Grants API 中同一 grant 的 `first_connected_at` 严格相等，`last_command_at` 不早于 `first_connected_at`。

### Web UI 回归

- 构造包含超长 `shell.exec` 文本的 Recent Calls 记录。
- 浏览器打开 Remote Invoke Tab，确认列表行保持单行摘要，右侧标签不换成竖排。
- 点击记录确认详情弹窗展示完整命令和参数（截至 120 字符）。

### human_tests

- `human_tests/remote-invoke.md`：加密链路下 `Recent Calls` 参数预览与 Tooltip 完整 JSON，重启后不丢失，并支持清理全部记录。
- `human_tests/remote-invoke.md`：超长命令不撑乱 Recent Calls 布局，API/详情/落盘文件都只保留前 120 字符。
- `human_tests/remote-invoke.md`：执行命令后 Grants API 的 `first_connected_at` 严格不变，`last_command_at` 单调更新。
- `human_tests/readme.md` 索引与用例数同步更新。

## Review / Fix / Test 闭环

- 改动 `build_call_command_summary`：必须补齐对应 `test_build_call_command_summary_*` 分支单测。
- 改动 `preserve_existing_grant_runtime_state`：必须补齐对应 `test_preserve_existing_grant_runtime_state_*` 分支单测，覆盖新增字段。
- 改动 `CallHistoryStore`：必须补齐 truncate / clear_for_client / retention / bad-jsonl / legacy-file 五族单测。
- 改动 Web `getCallArgsPreviewSource`：`web/src/api/remoteInvoke.test.ts` 必须更新。
- 改动 API 返回体：本文档「Admin API」表格必须同步更新，并给出对应 e2e 断言修改。

## 校验要求

- `pnpm --dir web test:unit -- src/api/remoteInvoke.test.ts`
- `cargo test -p bifrost-admin preserve_existing_grant_runtime_state -- --nocapture`
- `cargo test -p bifrost-admin build_call_command_summary -- --nocapture`
- `cargo test -p bifrost-admin call_history_store -- --nocapture`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `bash scripts/ci/local-ci.sh --e2e-only platform`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## 风险与决策

### 1. Relay 补偿事件覆盖 grant 时间

- 风险：`grant_created` SSE 在 crypto 尚未落盘时到达，本地已 `approve_pairing` 的 `first_authorized_at` 被覆盖导致时间回退。
- 决策：所有重建 `GrantInfo` 路径必须走 `preserve_existing_grant_runtime_state`，保留 existing 时间戳与非 active 状态。

### 2. 超长命令撑爆 JSONL

- 风险：`shell.exec` 长命令历史被完整落盘，导致 JSONL 体积无边界增长；Web UI 布局被撑乱。
- 决策：写入前 120 字符截断，落盘、API、UI 三处同源；`max_records` 硬上限 1000 + `retention_days` 90 天双重兜底。

### 3. JSON 字符串截断破坏合法性

- 风险：直接对 `args_json` 硬截断会产生非法 JSON。
- 决策：截断发生在 JSON 内部字符串值上，序列化后的 JSON 仍合法；API 返回体保证可被 Web 直接解析。

### 4. 旧整 JSON 文件残留

- 风险：老版本用户升级后 `remote_invoke_call_history.json` 与新 JSONL 并存，读取不一致。
- 决策：启动时直接删除旧文件，不做迁移。

### 5. 分页游标漂移

- 风险：`limit` 未 clamp 时可能被恶意调用者请求超大值造成内存放大。
- 决策：`limit` clamp 1..=200，默认 100；`before` 为毫秒时间戳而非序号，避免记录增减导致游标失效。

## 实现状态对齐（2026-07-03 复核）

- 已交付：`build_call_command_summary`（`worker.rs:3760`）、`preserve_existing_grant_runtime_state`（`worker.rs:4549`）、`CallHistoryStore`（`call_history_store.rs`）及 `clear_for_client`、120 字符截断、坏 JSONL 行 compaction、旧整 JSON 文件删除（`call_history_store.rs:18` 起常量及对应单测）。
- 已交付：`RemoteInvokeConfig.retention_days` 默认 90、`max_records` 默认 1000，并在 `effective_max_records()` / `CALL_HISTORY_HARD_MAX_RECORDS` 处硬上限 1000（`types.rs:289-294`、`call_history_store.rs:25`）。
- 已交付：Web 侧 `getCallArgsPreviewSource` 抽取（`web/src/api/remoteInvoke.ts:210`）与 `RemoteInvokeTab` 标题/Tooltip/详情弹窗共用同一来源（`RemoteInvokeTab.tsx:2848-3088`）。
- 已交付：`GET /_bifrost/api/remote-invoke/calls`、`GET /_bifrost/api/remote-invoke/calls/{call_id}`、`DELETE /_bifrost/api/remote-invoke/calls` 路由（`handlers/remote_invoke.rs:90`、`handlers/remote_invoke.rs:635` 起的 `handle_calls_list`）。
- 已交付：本节「测试方案」列举的 5 个 e2e 脚本均已存在（`test_remote_invoke_e2e.sh`、`..._recent_calls_args_preview_e2e.sh`、`..._recent_calls_persistence_e2e.sh` 等），相关单测函数 `test_call_history_store_*` / `test_build_call_command_summary_*` / `test_preserve_existing_grant_runtime_state_*` 均已落地。
- 文档遗留偏差（保留原始描述，仅在此标注）：原文档曾写过「默认保留 7 天 / 100000 条」，已按现网实现修正为「90 天 / 1000 条」。
- (planned, not yet shipped as of 2026-07-03) 暂无本设计内独立未交付项；后续若新增 cross-relay 历史合并视图等扩展能力，应在新章节追加。

## 文档更新要求

- 本次改动仅修复 Remote Invoke Recent Calls 展示回退逻辑，不涉及 README / 外部 API 文档变更。
- 必须更新 `human_tests/remote-invoke.md`。
- 必须更新 `human_tests/readme.md`。
