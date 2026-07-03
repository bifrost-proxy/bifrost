# UTF-8 安全 Preview 截断

## 背景

Bifrost 多个模块会把用户输入、工具参数、脚本输出、HTTP body 或 CLI 展示文本截断为 preview。此前部分代码直接使用 `&str[..N]` 或 `String::truncate(N)`，当 `N` 落在中文、emoji 等多字节 UTF-8 字符中间时会触发 Rust panic，例如：

```text
end byte index 500 is not a char boundary
```

Agent compaction、IM Gateway 任务输出、CLI/API 错误展示、E2E 断言预览以及 proxy body decode preview 都命中过这个 panic。本模块把散落在各 crate 的 preview 截断逻辑统一收敛到 `bifrost-core::text`，并禁止在业务代码里再直接写 `[..N]` / `String::truncate(N)`。

真实实现位于 `crates/bifrost-core/src/text.rs`，并由 21+ 个下游文件复用：`bifrost-agent`、`bifrost-admin`（`im_gateway::task_executor`、`replay_scripts`、`connection_monitor`、`im_gateway::feishu`、`im_gateway/agent_chat`）、`bifrost-cli`（`traffic`、`admin`、`im`）、`bifrost-proxy`（`http::handler::decode`、`utils::logging`）、`bifrost-sync::client`、`bifrost-e2e`（`assertions`、`curl`）、`agent`（`responses`、`session::turn_loop`、`tools::file_ops`、`client`）等。

## 用户目标验证清单

### 必须实现

- 提供 `bifrost-core::text` 公共 helper，覆盖字符预算和字节预算两类 preview。
- 支持字节边界 `floor_char_boundary` / `ceil_char_boundary`，永远回退到合法 UTF-8 字符边界。
- 支持字符预算 `truncate_chars` / `truncate_chars_with_suffix` / `truncate_chars_with_ellipsis`，按 Unicode scalar 计数。
- 支持字节预算 `truncate_bytes` / `truncate_bytes_with_suffix` / `truncate_middle_bytes`，先回退到合法字符边界。
- `truncate_middle_bytes` 保留头尾，中间替换为 `…N chars truncated…` 提示。
- Agent compaction、IM Gateway task preview、CLI list、proxy decode preview、E2E 断言 preview 全部改为公共 helper。
- 新增 crate 引入 preview 需求时优先复用 helper，不允许再引入 `[..N]` 用法。

### 必须不破坏

- 二进制 buffer、ASCII 协议分隔符、哈希/nonce 固定字节切片仍按原字节切片保留。
- 已有 preview 语义（`[truncated]`、`...`、`…chars truncated…`）保持外观稳定，避免测试断言破裂。
- 现有 API/JSON 响应字段类型不变，只把内部截断实现替换为 helper 调用。
- 大文件读取仍受 `MAX_READ_FILE_BYTES = 512 MiB` 限制（`check_file_size`）。

### 必须真实验证

- 直接构造多字节字符边界的输入，验证 helper 不 panic。
- 覆盖 Agent compaction、IM Gateway task executor、E2E 断言三条已知 panic 复现路径。
- CI 保留一条脚本 `test_utf8_safe_preview_e2e.sh` 作为回归触发入口。

## 产品语义

### 字符预算 vs 字节预算

Preview 需求分两大语义，助手/CLI 展示走字符预算，日志/请求体走字节预算：

- **字符预算**：UI 展示、CLI 列表、AI 对话历史、错误摘要。用户观感是「保留前 N 个字符」；使用 `truncate_chars_with_suffix(s, max_chars, "...")` 或 `truncate_chars_with_ellipsis(s, max_chars)`。
- **字节预算**：日志上限、HTTP body 预览、traffic decode preview、Feishu 消息片段。用户约束是「preview 不超过 N 字节」；使用 `truncate_bytes_with_suffix` 或 `truncate_middle_bytes`。

不允许把两种语义混用。凡是配置里已经出现「N 字节」的既有语义，必须走字节 helper 才能保证向前兼容。

### 中间省略语义

`truncate_middle_bytes(s, max_bytes)` 保留头尾各 `max_bytes/2` 字节，中间替换为：

```text
…{removed_chars} chars truncated…
```

用于 replay/traffic body preview，方便同时看到请求前缀和响应尾部特征字段。当 `max_bytes == 0` 且字符串非空时返回 `…{total_chars} chars truncated…`，避免完全丢失长度信息。

### 与 Codex 保持一致

`text.rs` 头注释显式说明「Inspired by Codex's `codex_utils_string::truncate` module」；`MAX_READ_FILE_BYTES` 常量与 Codex 对齐（512 MiB），`truncate_middle_bytes` 输出形态和 Codex `truncate_middle_chars` 保持家族一致，便于 Codex parity 场景直接对比 preview。

## 技术细节

### crate 定位

`bifrost-core/src/text.rs` 是 pub 模块，其他 crate 通过 `use bifrost_core::text::...` 引入。它只依赖标准库 + `tempfile` (dev)；不引入运行时依赖，避免下沉 crate 变重。

`bifrost-agent` 只依赖 `bifrost-core`，反向不成立。

### 核心 API

```rust
pub fn floor_char_boundary(s: &str, index: usize) -> usize;
pub fn ceil_char_boundary(s: &str, index: usize) -> usize;
pub fn truncate_bytes(s: &str, max_bytes: usize) -> String;
pub fn truncate_bytes_with_suffix(s: &str, max_bytes: usize, suffix: &str) -> String;
pub fn truncate_chars(s: &str, max_chars: usize) -> String;
pub fn truncate_chars_with_suffix(s: &str, max_chars: usize, suffix: &str) -> String;
pub fn truncate_chars_with_ellipsis(s: &str, max_chars: usize) -> String;
pub fn truncate_middle_bytes(s: &str, max_bytes: usize) -> String;
pub const MAX_READ_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub fn check_file_size(path: &Path, max_bytes: u64) -> Result<u64, String>;
```

关键实现要点：

- `floor_char_boundary` 从 `index` 向前回退直到 `is_char_boundary` 命中；当 `index >= s.len()` 直接返回 `s.len()`，避免越界。
- `truncate_bytes_with_suffix` 短路：如果 `s.len() <= max_bytes` 返回原串，不追加 suffix，避免误插省略号。
- `truncate_chars_with_suffix` 用 `s.chars().count()` 判断，避免多字节被误算成多个字符。
- `truncate_middle_bytes` 在 `head_end..tail_start` 之间统计 `removed_chars`；使用 `tail_start.max(head_end)` 保护，避免预算重叠时下标穿越。

### 下游改造点

- `bifrost-agent`：`agent::responses`、`agent::session::turn_loop`、`agent::tools::file_ops`、`agent::client` 使用 `truncate_bytes_with_suffix` / `truncate_chars_with_suffix` 处理 compaction、tool call preview、file read preview、network body preview。
- `bifrost-admin`：`im_gateway::task_executor`（`test_truncate_preview_multibyte_boundary`）、`im_gateway::feishu`、`im_gateway/agent_chat`、`replay_scripts`、`connection_monitor` 使用 helper 生成任务 stdout preview、Feishu 消息片段、Replay 预览与连接监控消息。
- `bifrost-cli`：`commands::traffic`、`commands::admin`、`commands::im` 使用字符 helper 生成列表和错误提示。
- `bifrost-proxy`：`proxy::http::handler::decode`、`utils::logging` 使用字节 helper 生成响应 body / 日志 preview。
- `bifrost-e2e`：`assertions`（`test_assert_body_contains_multibyte_preview`）、`curl` 使用字符 helper 拼装失败预览。
- `bifrost-sync::client`：使用 helper 保护同步日志中的用户内容。
- `tests/http3_test.rs`：HTTP/3 集成测试使用 helper 生成 body assertion 预览。

## CLI + Web + Admin API

本模块是纯基础库，不新增 CLI 命令、不新增 Web UI、不新增 Admin API 路由。所有对外可见变化仅体现在既有命令/API 的 preview 文案中：

- CLI：`bifrost traffic list/get`、`bifrost admin log`、`bifrost im ...` 的截断预览不再 panic，字符尾部使用 `...`；输出格式与旧版一致。
- Web UI：Traffic Detail、Replay、Console、IM Gateway 面板显示的 preview 走后端已修复的路径。
- Admin API：`/api/traffic/*`、`/api/im_gateway/*`、`/api/agent/*`、`/api/replay/*` 响应字段中的 preview/summary/error/message 字符串保证是合法 UTF-8，且不会因 max_len 触发 500。

## Sync 边界

本模块不涉及 Sync：`text` helper 只处理本地字符串截断，`bifrost-sync::client` 只是消费方。规则/Group/端口 Sync 语义不变。

## Phase 1-4

### Phase 1：基础模块与单测

- 新增 `crates/bifrost-core/src/text.rs`，实现全部 helper 与内部单测（`floor_char_boundary_handles_multibyte_middle` 等）。
- 在 `bifrost-core` 的 `lib.rs` 暴露 `pub mod text`。
- 补齐 `check_file_size` + `MAX_READ_FILE_BYTES` 常量，方便下游文件读取共用。

### Phase 2：Agent / Admin 关键 panic 路径修复

- `bifrost-agent`：`compact::format_history_for_compaction` 改为 `truncate_chars_with_suffix`，新增 `test_format_history_handles_multibyte_tool_arguments` 与 `test_format_history_truncates_multibyte_user_messages`。
- `bifrost-admin::im_gateway::task_executor`：`truncate_preview` 内部改为 `truncate_bytes_with_suffix`，新增 `test_truncate_preview_multibyte_boundary`。

### Phase 3：CLI / Proxy / E2E 覆盖

- `bifrost-cli`：`commands::traffic`、`commands::admin`、`commands::im` 全面切换到 `truncate_chars_with_ellipsis`。
- `bifrost-proxy`：`http::handler::decode`、`utils::logging` 中的 body / header preview 切换到 `truncate_bytes_with_suffix`。
- `bifrost-e2e`：`assertions.rs`、`curl.rs` 使用字符 helper，新增 `test_assert_body_contains_multibyte_preview`。

### Phase 4：文档与回归入口

- 新增 `e2e-tests/tests/test_utf8_safe_preview_e2e.sh`，串起下面几条精准 `cargo test`。
- 新增 `human_tests/utf8-safe-preview.md`（TC-USP-01/02/03），同步 `human_tests/readme.md` 索引。
- 更新 `AGENTS.md` 或 codemate 提示：所有新增 preview 必须调用 helper，禁止 `[..N]`。

## 测试方案

### 单元测试

`bifrost-core::text::tests`：

- `floor_char_boundary_handles_multibyte_middle`
- `ceil_char_boundary_handles_multibyte_middle`
- `truncate_bytes_with_suffix_does_not_split_chinese`
- `truncate_bytes_with_suffix_keeps_ascii_boundary`
- `truncate_bytes_with_suffix_returns_original_when_short`
- `truncate_bytes_floors_to_boundary`
- `truncate_chars_takes_n_scalars`
- `truncate_chars_with_suffix_counts_multibyte_as_one_char`
- `truncate_chars_with_suffix_returns_original_when_short`
- `truncate_chars_with_ellipsis_appends_dots`
- `truncate_middle_bytes_returns_original_when_under_limit`
- `truncate_middle_bytes_removes_middle_ascii`
- `truncate_middle_bytes_respects_utf8_boundaries`
- `truncate_middle_bytes_zero_budget`
- `truncate_middle_bytes_zero_budget_empty_string`
- `check_file_size_returns_error_for_missing_file`
- `check_file_size_ok_and_oversized`

下游模块：

- `bifrost-agent`：`compact::tests::test_format_history_handles_multibyte_tool_arguments`、`compact::tests::test_format_history_truncates_multibyte_user_messages`
- `bifrost-admin`：`im_gateway::task_executor::tests::test_truncate_preview_multibyte_boundary`
- `bifrost-e2e`：`assertions::tests::test_assert_body_contains_multibyte_preview`

### E2E 回归脚本

`e2e-tests/tests/test_utf8_safe_preview_e2e.sh`：串行触发下列精准命令，全部 `ok` 即视为回归通过：

```bash
cargo test -p bifrost-core text::tests::
cargo test -p bifrost-agent compact::tests::test_format_history_handles_multibyte_tool_arguments
cargo test -p bifrost-agent compact::tests::test_format_history_truncates_multibyte_user_messages
cargo test -p bifrost-admin im_gateway::task_executor::tests::test_truncate_preview_multibyte_boundary
cargo test -p bifrost-e2e assertions::tests::test_assert_body_contains_multibyte_preview
```

### 真实场景测试 human_tests

`human_tests/utf8-safe-preview.md`（已落地）：

- **TC-USP-01**：回归 Agent compaction tool arguments 中文边界不 panic。执行 `cargo test -p bifrost-agent compact::tests::test_format_history_handles_multibyte_tool_arguments -- --nocapture`。
- **TC-USP-02**：IM Gateway 任务输出中文长文本 preview 不 panic 且保留 `[truncated]` 或 `...` 标记。执行 `cargo test -p bifrost-admin im_gateway::task_executor::tests::test_truncate_preview_multibyte_boundary -- --nocapture`。
- **TC-USP-03**：CLI/API 错误 preview 含中文长文本不 panic。触发 `cargo test -p bifrost-e2e assertions::tests::test_assert_body_contains_multibyte_preview -- --nocapture`，并肉眼确认 preview 前缀含中文且尾部有 `...`。

所有测试都不需要启动 Bifrost 服务，也不修改系统代理。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：所有 preview 路径都能承接中文/emoji，且保留字符/字节两种语义。
- Diff review：`git status --short`、`git diff` 确认没有遗漏的 `[..N]` / `String::truncate(...)`。可通过 `rg -n "&\w+\[\.\.[0-9]+\]" crates/` 反向巡检。
- 单元测试：`cargo test -p bifrost-core text::tests::` + 下游三条精准测试。
- 回归脚本：`bash e2e-tests/tests/test_utf8_safe_preview_e2e.sh`。

### 第 2 轮

- 复核第 1 轮修复。若新增了业务代码里 truncate 逻辑，追加对应 helper 覆盖。
- 复核 `human_tests/readme.md` 索引是否新增 TC-USP 行，且不维护「总用例数」。
- 复跑：`cargo fmt --all -- --check`、相关 clippy、`cargo test --workspace --all-features`。
- 校验：`bash scripts/ci/local-ci.sh --skip-e2e` 至少通过 fmt/clippy/build，本机 no-local-coverage 豁免仍适用。

## 风险与决策

- 字符 vs 字节语义混用是最大隐性风险；review 时必须核对被替换处的原语义是「按字节」还是「按字符」，不要一律替换成 `truncate_chars_*`。
- `truncate_middle_bytes` 会破坏尾部签名固定长度的场景（例如 hash 尾），此类场景仍应保留原字节切片，不引入 helper。
- 未来若引入 grapheme cluster 需求（emoji + skin tone），可在 `text` 模块继续增补 `truncate_graphemes_*` 系列 API，保持公共入口只在 `bifrost_core::text` 中演进，不允许在业务 crate 里自建 preview 工具。
- helper 目前不做转义；如果 preview 会被写入 JSON/HTML，调用方仍需负责 escape，避免 preview 内嵌控制字符影响 UI。
