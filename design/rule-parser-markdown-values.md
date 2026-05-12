# Rule Parser Markdown Values

## 功能模块详细描述

规则支持通过 Markdown fenced block 定义多行 value，并用 `reqHeaders://{name}` 等协议引用。多行 header value 中以 `#` 开头的整行内容应视为注释，不能被转换成请求头名，也不能影响 fenced block 前后的规则解析。

## 问题与根因

用户规则中 `env_block` value 包含：

```text
#comment_marker
## X-Ignored-Env: ignored comment
```

core rule parser 与 sync normalize 都能保留 fenced block 和前后 4 条规则；实际风险在 header value 消费侧：运行时和 Admin/Replay 将 `## X-Ignored-Env: ...` 解析成 header 名 `## X-Ignored-Env`。该 header 名非法，HTTPS tunnel 应用请求头时会返回 `Bad Gateway`，表现为前置 API 规则不可用，只有后面的普通转发规则看起来生效。

## 实现逻辑

- `crates/bifrost-cli/src/parsing/headers.rs`：多行 header value 拆分后跳过 `trim().starts_with('#')` 的整行注释，避免运行时生成非法 request/response header。
- `crates/bifrost-admin/src/request_rules.rs`：Admin/Replay 请求规则使用同样语义，避免 replay/API 路径与真实代理路径不一致。
- `crates/bifrost-core/src/rule/parser/mod.rs`：补充用户规则原文回归，证明 fenced value 中的 `#` 不会导致前后规则丢失。
- `crates/bifrost-sync/src/normalize.rs`：补充同步归一化回归，证明远端规则 normalize 不会删除 fenced block 或前后规则。

## 依赖项

- `RuleParser::parse_rules_with_inline_values`
- `ValueSource::ValueRef`
- `parse_header_value` / `parse_header_values`
- `RulesResolver::with_values`

## 测试方案

### 单元测试

- `cargo test -p bifrost-core test_parse_reqheaders_markdown_value_with_hash_lines -- --nocapture`
- `cargo test -p bifrost-sync preserves_markdown_value_blocks_with_hash_lines_during_normalization -- --nocapture`
- `cargo test -p bifrost-cli parse_header_value_skips_hash_comment_lines -- --nocapture`
- `cargo test -p bifrost-cli test_reqheaders_markdown_value_skips_hash_comment_lines -- --nocapture`
- `cargo test -p bifrost-admin test_parse_header_values_skips_hash_comment_lines -- --nocapture`

### E2E 测试

- 执行 `cargo test -p bifrost-e2e rule_validation -- --nocapture`，覆盖真实 Admin validate API 对 Markdown value block 与 active `reqHeaders` 的解析。
- 本次 bug 的最小失效点是 header value 消费侧，已用 `bifrost-cli` resolver 单元测试覆盖真实运行时解析结果。

### 真实场景测试

- 更新 `human_tests/proxy-rules-advanced.md`，新增 `TC-PRA-62`。
- 按用例执行：解析中性示例规则文本，断言 4 条规则保留、`env_block` value 保留 `#` 行、最终运行时 request headers 只包含 `X-Test-Env` 与 `X-Test-Flag`。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户规则原文、core/sync/CLI/Admin 四条路径；检查是否误删合法 header 值中的 `#`；运行最小相关单元测试。
- 第 2 轮：复查 diff、human_tests 索引、设计文档和解析语义一致性；复跑受影响测试并执行格式/工作区校验。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 按需执行 `scripts/ci/local-ci.sh`；本次改动为 parser/request-rule 小范围修复，若未执行 local-ci，最终说明风险。

## 文档更新要求

- 更新 `human_tests/proxy-rules-advanced.md`
- 更新 `human_tests/readme.md`
