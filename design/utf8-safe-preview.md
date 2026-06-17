# UTF-8 安全 Preview 截断

## 功能模块说明

Bifrost 多个模块会把用户输入、工具参数、脚本输出、HTTP body 或 CLI 展示文本截断为 preview。此前部分代码直接使用 `&str[..N]` 或 `String::truncate(N)`，当 `N` 落在中文、emoji 等多字节 UTF-8 字符中间时会触发 Rust panic，例如：

```text
end byte index 500 is not a char boundary
```

本模块统一治理所有面向字符串 preview 的截断逻辑，避免 Agent compaction、IM Gateway 任务输出、CLI/API 错误展示和测试断言再次因为多字节字符崩溃。

## 实现逻辑

- 在 `bifrost-core` 新增 `text` 工具模块（`crates/bifrost-core/src/text.rs`），提供两类明确语义：
  - `truncate_chars*`：按 Unicode scalar 字符数量截断，适合 UI/CLI/历史记录 preview。包含 `truncate_chars`、`truncate_chars_with_suffix`、`truncate_chars_with_ellipsis`。
  - `truncate_bytes*` / `floor_char_boundary` / `ceil_char_boundary` / `truncate_middle_bytes`：保留字节预算语义，但先回退到合法 UTF-8 字符边界，适合日志和请求体预览；`truncate_middle_bytes` 用于保留头尾、中间替换为省略提示。
- 禁止新增面向 `&str` 的 `[..N]`、`[..max_len]` 或 `String::truncate(max_len)` preview 截断；必须调用公共 helper。
- 对确认为二进制 buffer、ASCII 协议分隔符、哈希/nonce 固定字节数组等场景保留原字节切片。
- Agent compaction 中 `content` 和 tool call arguments 的截断统一改为字符安全 helper，直接覆盖本次 panic。
- IM Gateway task stdout/stderr preview、CLI 列表展示、proxy decode preview、E2E 失败预览统一切换到公共 helper。

## 依赖项

- `bifrost-agent` 新增对 `bifrost-core` 的依赖，仅使用 `bifrost_core::text`，不引入反向依赖。
- 其他已依赖 `bifrost-core` 的 crate 直接复用公共 helper。

## 测试方案

### 单元测试

- `bifrost-core::text`：
  - `truncate_chars_with_suffix` 验证中文/emoji 截断不会 panic，且不切断字符。
  - `truncate_bytes_with_suffix` 验证字节边界落在中文内部时会回退到上一个合法字符边界。
  - `floor_char_boundary` 验证 0、完整长度、中间字节等边界。
- `bifrost-agent::compact`：
  - 构造 tool call arguments，使第 500 byte 落在中文字符内部，验证 `format_history_for_compaction` 不 panic 且输出包含省略号。
  - 构造超长中文 user message，验证 compaction input 截断不 panic。
- `bifrost-admin::im_gateway::task_executor`：
  - 构造 stdout preview，使 `max_len` 落在中文字符内部，验证不 panic 且带 `[truncated]` 标记。
- CLI/E2E assertion 辅助模块：
  - 构造失败预览含中文多字节内容，验证错误消息可生成。

### E2E 测试

新增 `e2e-tests/tests/test_utf8_safe_preview_e2e.sh`，作为回归脚本执行相关 crate 的精准测试：

- `cargo test -p bifrost-core text::tests::`
- `cargo test -p bifrost-agent compact::tests::test_format_history_handles_multibyte_tool_arguments`
- `cargo test -p bifrost-agent compact::tests::test_format_history_truncates_multibyte_user_messages`
- `cargo test -p bifrost-admin im_gateway::task_executor::tests::test_truncate_preview_multibyte_boundary`
- `cargo test -p bifrost-e2e assertions::tests::test_assert_body_contains_multibyte_preview`

### 真实场景测试

新增 `human_tests/utf8-safe-preview.md`：

- TC-USP-01：Agent compaction tool arguments 含中文多字节边界，不触发 panic。
- TC-USP-02：IM Gateway 任务输出中文长文本 preview，不触发 panic 且保留截断标记。
- TC-USP-03：CLI/API 错误预览含中文长文本，不触发 panic。

## 校验要求

- `bash e2e-tests/tests/test_utf8_safe_preview_e2e.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 修改范围适配时执行 `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 新增 `human_tests/utf8-safe-preview.md`。
- 更新 `human_tests/readme.md` 索引表。
- 本次不涉及用户命令、协议、配置或 README 对外说明变更。
