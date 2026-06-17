# Remote Invoke Recent Calls 参数预览回退

## 背景

Remote Invoke 的 `openCall` 已升级到密文链路，relay 不再持久化明文 `command_summary`。当前 client 侧 `Recent Calls` 标题区域仍优先依赖 `command_summary.masked_args_json` 渲染参数预览，导致加密链路下即使本地已经解密并保存了 `command.args_json`，Web UI 依然显示不出命令参数详情。

用户可见现象：

- `Recent Calls` 只显示命令名与状态，不显示参数预览
- hover Tooltip 也没有完整参数 JSON
- `bytes_out` 仍能显示，说明并非整条调用记录缺失，而是参数摘要字段为空

## 目标

- `Recent Calls` 在加密链路下继续展示可读参数预览
- 优先复用已有 `command_summary.masked_args_json`
- 若 relay 未下发该字段，则 client 本地使用已解密的 `command.args_json` 补齐
- 不改变 connect 等无参数命令的展示行为
- `Recent Calls` 本地落盘，Bifrost 重启后仍能恢复最近记录
- 支持一键清理当前客户端的全部 Recent Calls
- 本地记录默认保留 90 天，单个 relay/client 最多保留 1000 条（`CALL_HISTORY_HARD_MAX_RECORDS` 在 `call_history_store.rs` 中硬上限同样为 1000；用户配置 `max_records` 超过 1000 时也会被截到 1000）
- 命令相关文本超过 120 字符时直接截断，只保留前 120 字符用于展示和落盘，避免超长命令撑爆本地历史文件

## 实现方案

### 1. Client Worker 本地补齐参数摘要

文件：`crates/bifrost-admin/src/remote_invoke/worker.rs`

- 在 `build_call_command_summary()` 中保留现有 `command_preview` 回退逻辑
- 新增 `masked_args_json` 回退：
  - 若 relay 下发的 `masked_args_json` 非空，则保持原值
  - 若为空，则使用本地解密后的 `RemoteCommand.args_json`
  - 空字符串按缺失处理，避免写入无意义占位

这样 `GET /api/remote-invoke/calls` 返回的本地调用历史可以稳定包含参数摘要，不再依赖 relay 明文字段。

### 1.1 Recent Calls 本地落盘与清理

文件：`crates/bifrost-admin/src/remote_invoke/worker.rs`、`crates/bifrost-admin/src/remote_invoke/call_history_store.rs`

- `CallHistoryStore` 已抽到独立模块 `call_history_store.rs`，由 `worker.rs` 通过 `Arc<CallHistoryStore>` 持有；落盘目录为 `BIFROST_DATA_DIR/admin/remote_invoke_call_history/`
- 存储维度为 `relay_url + client_instance_id + call_id`
- 收到 `call_open` 并完成本地解密后 append 一行 `streaming` 快照
- completed / failed / cancelled 终态再 append 一行同 `call_id` 的最新快照
- worker 启动和 relay_url 切换时不恢复历史；Recent Calls 列表和详情 API 按需读取 JSONL
- worker 内存不保留历史列表；正在执行的 call 只保留临时快照，执行结束后释放
- 旧 `BIFROST_DATA_DIR/admin/remote_invoke_call_history.json` 不兼容读取，发现后直接删除
- `remote_invoke.retention_days` 默认 90 天；`remote_invoke.max_records` 默认 1000 条
- `DELETE /_bifrost/api/remote-invoke/calls` 清理当前 relay/client 的全部本地 Recent Calls
- 写入本地 JSONL 前统一执行 120 字符截断：
  - `command_summary.command_preview`
  - `command_summary.masked_args_json`
  - `command.command`
  - `command.args_json`
  - `command.query` 中的字符串字段
  - `command.argv`、`command.shell`、`command.command_text`、`command.cwd`、`command.env`
  - `policy_id`
- 截断逻辑保留可识别前缀并附带原始长度后缀；API、Web UI、落盘文件看到同一份截断后的内容。
- 对 `args_json` / `masked_args_json` 这类 JSON 字符串，截断发生在 JSON 内部的字符串值上，序列化后的 JSON 仍保持合法，避免参数预览因为硬截断变成不可解析文本。

### 1.2 Grants 时间字段稳定性

文件：`crates/bifrost-admin/src/remote_invoke/worker.rs`

- `first_connected_at` 来自 `GrantInfo.first_authorized_at`，其语义是 grant 生命周期内的首次授权/连接时间，不能在后续命令执行、SSE 重连或 relay `grant_created` 补偿事件中变化。
- `approve_pairing` 已经把本地授权成功时刻写入 `local_grants` 和持久化 grant info；若稍早到达的 `grant_created` SSE 因本地 crypto 尚未落盘而短暂等待，等待结束后必须保留 `local_grants` 中已有 grant 的运行态字段。
- 所有会用 relay payload 重建 `GrantInfo` 的 client 侧同步路径，在发现本地已有同一 `grant_id` 时必须保留：
  - `first_authorized_at`：严格沿用 existing，避免 1ms 级别的 relay / 本地时间差回退
  - `last_command_at` / `last_used_at`：取更大的时间戳，保持单调
  - `max_calls` / `remaining_calls` / `use_count` / 非 active 状态：避免重建 grant 时重置一次性授权或命令计数

### 2. Web UI 再做一层展示回退

文件：`web/src/api/remoteInvoke.ts`
文件：`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`

- 抽出 `Recent Calls` 参数预览来源函数
- 展示顺序：
  1. `call.command_summary.masked_args_json`
  2. `call.command.args_json`
- `RemoteInvokeTab` 的标题预览与 Tooltip 共用同一个来源，避免标题和 hover 内容不一致
- Recent Calls 卡片右上角增加清理按钮，调用 `DELETE /remote-invoke/calls` 后清空页面列表

### 2.1 Recent Calls 长内容布局与详情弹窗

文件：`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`

- 每条 Recent Calls 记录改为稳定的三段布局：
  1. 左侧为命令摘要和参数预览，占据剩余宽度并允许收缩
  2. 中间为状态、caller、policy、exec mode 等短标签，不被长命令挤压
  3. 右侧保留详情按钮，点击后打开完整记录弹窗
- 命令摘要、参数预览、caller、policy、exec mode 均单行显示并自动 `ellipsis` 截断，避免长 shell 文本把列表撑高或把右侧列压成竖排。
- 点击记录或详情按钮时调用 `GET /remote-invoke/calls/{call_id}` 拉取最新详情；失败时回退当前列表记录，保持详情可读。
- 详情弹窗展示服务端已保存的命令、参数 JSON、调用 ID、grant/client/caller、状态、耗时、流量、policy/exec mode 与命令详情 JSON；其中命令相关长文本最多为前 120 字符，不再暴露或保存完整超长原文。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/remote_invoke/worker.rs`
  - 验证 `command_summary.masked_args_json` 缺失时，会回退到 `command.args_json`
  - 验证已有 `masked_args_json` 时不会被本地 `args_json` 覆盖
  - 验证 `preserve_existing_grant_runtime_state` 保留已有 `first_authorized_at`，且 `last_command_at` / `last_used_at` 单调、不重置一次性授权计数
  - 验证 call history store 按保留期和 max_records=1000 裁剪
  - 验证 clear_for_client 只清理当前 relay/client
  - 验证旧整 JSON 文件直接删除，不迁移
  - 验证坏 JSONL 行会被 compaction 清理
  - 验证 worker 构造不加载历史，Recent Calls API 请求时才读取 JSONL
  - 验证 call history store 写入前会把命令相关字符串截断到 120 字符，原始长片段不会出现在落盘 JSONL 中
- `web/src/api/remoteInvoke.test.ts`
  - 验证 Recent Calls 参数预览来源优先使用 `masked_args_json`
  - 验证缺失时回退到 `command.args_json`

### E2E 测试

- 更新 `e2e-tests/tests/test_remote_invoke_e2e.sh`
- 新增断言：
  - `remote search` 执行后，`/api/remote-invoke/calls` 中对应记录的 `command_summary.masked_args_json` 非空
  - 其中包含 `query`、`max_results`、`max_scan`
- 更新 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- 新增断言：
  - 执行命令前后读取 Grants API，同一 grant 的 `first_connected_at` 必须严格相等，`last_command_at` 必须在执行后出现且不早于 `first_connected_at`
  - Recent Calls 写入 `admin/remote_invoke_call_history/*.jsonl`
  - 超长搜索参数在 Recent Calls API 中只返回前 120 字符，且 `masked_args_json` 仍是合法 JSON
  - 旧 `remote_invoke_call_history.json` 不存在，JSONL 不包含完整超长参数原文
  - 保留同一 `BIFROST_DATA_DIR` 重启 Bifrost 后，同一 `call_id` 仍可从 Recent Calls API 读取
  - `GET /_bifrost/api/remote-invoke/calls?limit=25&before=<ts>` 路径下：`limit` 接受 1..=200（默认 100，超出会被 clamp），下一页通过 response 中的 `next_cursor`（上一页最早一条的 `started_at` 毫秒时间戳）配合 `before=<ts>` 查询参数滚动获取；DELETE 同一路径会清空当前 relay/client 的全部本地记录并返回 `{success, removed}`
  - DELETE Recent Calls 后 API 返回空列表
- Web UI 回归验证：
  - 构造包含超长 `shell.exec` 文本的 Recent Calls 记录
  - 在浏览器中打开 Remote Invoke Tab，确认列表行保持单行摘要、右侧标签不换成竖排
  - 点击该记录后确认详情弹窗展示完整命令和参数

### 真实场景测试（human_tests）

- 更新 `human_tests/remote-invoke.md`
- 新增回归用例：加密链路下 `Recent Calls` 必须展示参数预览与 Tooltip 完整 JSON，重启后不丢失，并支持清理全部记录
- 新增回归用例：超长命令不会撑乱 Recent Calls 布局，且 API / 详情 / 落盘文件都只保留前 120 字符
- 新增回归用例：执行命令后 Grants API 的 `first_connected_at` 严格保持不变，`last_command_at` 单调更新
- 同步更新 `human_tests/readme.md` 索引与用例数

## 校验要求

- `pnpm --dir web test:unit -- src/api/remoteInvoke.test.ts`
- `cargo test -p bifrost-admin preserve_existing_grant_runtime_state -- --nocapture`
- `cargo test -p bifrost-admin build_call_command_summary -- --nocapture`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `bash scripts/ci/local-ci.sh --e2e-only platform`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## 文档更新要求

- 本次改动仅修复 Remote Invoke Recent Calls 展示回退逻辑，不涉及 README / 外部 API 文档变更
- 必须更新 `human_tests/remote-invoke.md`
- 必须更新 `human_tests/readme.md`

## 实现状态对齐（2026-06-16 复核）

本节记录 2026-06-16 对照仓库实际代码的复核结论，便于后续阅读时分辨「设计意图」与「现网行为」。

- 已交付：`build_call_command_summary`（`worker.rs:3461`）、`preserve_existing_grant_runtime_state`（`worker.rs:4175`）、`CallHistoryStore`（`call_history_store.rs`）及 `clear_for_client`、120 字符截断、坏 JSONL 行 compaction、旧整 JSON 文件删除（`call_history_store.rs:18` 起常量及对应单测）。
- 已交付：`RemoteInvokeConfig.retention_days` 默认 90、`max_records` 默认 1000，并在 `effective_max_records()` / `CALL_HISTORY_HARD_MAX_RECORDS` 处硬上限 1000（`types.rs:289-294`、`call_history_store.rs:25`）。
- 已交付：Web 侧 `getCallArgsPreviewSource` 抽取（`web/src/api/remoteInvoke.ts:210`）与 `RemoteInvokeTab` 标题/Tooltip/详情弹窗共用同一来源（`RemoteInvokeTab.tsx:2848-3088`）。
- 已交付：`GET /_bifrost/api/remote-invoke/calls` 与 `DELETE /_bifrost/api/remote-invoke/calls` 路由（`handlers/remote_invoke.rs:90`、`handlers/remote_invoke.rs:635` 起的 `handle_calls_list`）。注意：list 接口的下一页游标参数名是 `before`（毫秒 `started_at`），默认 `limit=100`，clamp 到 1..=200；返回体含 `calls`、`next_cursor`、`limit`。`DELETE` 返回 `{success, removed}`。
- 已交付：本节「测试方案」列举的 5 个 e2e 脚本均已存在（`e2e-tests/tests/test_remote_invoke_e2e.sh`、`..._recent_calls_args_preview_e2e.sh`、`..._recent_calls_persistence_e2e.sh` 等），相关单测函数 `test_call_history_store_*` / `test_build_call_command_summary_*` / `test_preserve_existing_grant_runtime_state_*` 均已落地。
- 文档遗留偏差（保留原始描述，仅在此标注）：原文档曾写过「默认保留 7 天 / 100000 条」，已按现网实现修正为「90 天 / 1000 条」。
- (planned, not yet shipped as of 2026-06-16) 暂无本设计内独立未交付项；后续若新增 cross-relay 历史合并视图等扩展能力，应在新章节追加。
