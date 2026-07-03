# 规则解析器：Markdown 值 `#` 行注释兼容

## 背景

Bifrost 规则语言允许通过 Markdown fenced block 定义多行 value，并通过 `reqHeaders://{name}` / `resHeaders://{name}` / `reqCookies://{name}` 等协议引用。例如：

```
`https://api.corp.test/*` reqHeaders://{env_block}

```env_block
X-Test-Env: test_env
#comment_marker
## X-Ignored-Env: ignored comment
X-Test-Flag: 1
```
```

用户希望 `env_block` 中：

- `X-Test-Env: test_env` → 变成合法请求头
- `#comment_marker` → 视作注释，被忽略
- `## X-Ignored-Env: ignored comment` → 视作注释（不是合法 header 名），被忽略
- `X-Test-Flag: 1` → 变成合法请求头

## 问题与根因

Core rule parser (`crates/bifrost-core/src/rule/parser`) 与 sync normalize (`crates/bifrost-sync/src/normalize.rs`) 能正确保留 fenced value block、value 内容以及前后 4 条普通规则的完整性 —— 这一层没有问题。**真正的失效点在 header value 消费侧**：

- CLI 解析 (`crates/bifrost-cli/src/parsing/headers.rs::parse_header_value`)
- Admin/Replay (`crates/bifrost-admin/src/request_rules.rs::parse_header_values`)

它们把 value 按 `\n` 拆行后，对以 `#` 开头的行直接调用 `part.find(':')` 或 `find('=')`，得到 `## X-Ignored-Env` 这种非法 header name。当代理构造 HTTPS tunnel 请求头时 `HeaderName` 校验失败，返回 `Bad Gateway`。表现出来的现象：

- 前置 API 规则完全不工作（所有走该规则的请求都 502）
- 后面的“普通转发规则”看起来生效（因为不走该 header 分支）
- 用户以为是规则解析错误，实际是消费错误

## 用户目标验证清单

### 必须实现

- 多行 header value 拆分后，跳过任何 `trim().starts_with('#')` 的整行（同时兼顾 `#` 与 `##` 两种注释深度）。
- CLI 与 Admin/Replay 使用**同一**语义，避免代理链路 vs replay 链路结果不一致。
- Core parser 保证 fenced block 前后规则完整（用户示例里 4 条规则一条都不能少）。
- Sync normalize 不删除 fenced block、不修改 value 内容、不影响前后规则。
- Header value 中包含 `#` 时不应被误当成 hash-comment；只有整行以 `#` 开头才跳过。

### 必须不破坏

- 合法 `#` 字符出现在 value 中间（e.g. `X-Trace-Id: abc#def`）仍然保留，因为这时 `part.trim()` 不以 `#` 开头。
- Value 引用 `{env_block}` 语义不变。
- 已有的行式 header（`X-Env: ppe\nX-Flag: 1`）、单行逗号分隔（`X-Env=ppe, X-Flag=1`）继续工作。
- JSON object header（见 `rule-header-json-compat.md`）不受影响，走独立分支。
- `@values`, `@disabled`, `@important` 语法不受影响。
- CORS `reqCors://({...})` 的多行 JSON 配置解析器（`crates/bifrost-cli/src/parsing/headers.rs::parse_cors_config`）不被误伤。

### 必须真实验证

- 真实用户规则原文（Markdown fenced block + `#comment_marker` + `## X-Ignored-Env`）走一次代理，请求头只包含 `X-Test-Env` + `X-Test-Flag`。
- 同一规则走一次 replay（Admin `POST /api/replay`）结果一致。
- 通过 `bifrost rule validate` 无告警；rule list / show 显示 fenced block 原文。

## 产品语义

Header value（Markdown 多行文本或行式）按下面顺序解析：

1. `trim()` 后判断括号包裹形式（`(...)`）与 `use_colon` 分隔符。
2. 判定 `looks_like_json_header_object`（见 rule-header-json-compat），命中走 JSON 分支。
3. 按 `\n` 或 `,` 拆分 part。
4. 对每个 part：`trim()` 后若为空 或 以 `#` 开头 → 跳过。
5. 否则按 `:` 或 `=` 拆 key/value。
6. Key 非空则加入 headers。

关键实现（`crates/bifrost-cli/src/parsing/headers.rs` 第 22 行）：

```rust
for part in content.split(delimiter) {
    let part = part.trim();
    if part.is_empty() || part.starts_with('#') {
        continue;
    }
    ...
}
```

同款逻辑存在于 `crates/bifrost-admin/src/request_rules.rs::parse_header_value`（第 791 行）。

## 技术细节

### 涉及的四条路径

1. `crates/bifrost-cli/src/parsing/headers.rs`
   - `parse_header_value(value: &str) -> Option<Vec<(String, String)>>`
   - 单元测试 `parse_header_value_skips_hash_comment_lines`（第 200 行）：
     - Input: `"X-Test-Env:test_env\n#comment_marker\nX-Test-Flag:1\n## X-Ignored-Env: ignored comment"`
     - Output: `[("X-Test-Env", "test_env"), ("X-Test-Flag", "1")]`
2. `crates/bifrost-admin/src/request_rules.rs`
   - `parse_header_values(value: &str) -> Vec<(String, String)>`（第 766 行）
   - `parse_header_value(value: &str) -> Option<(String, String)>`（第 791 行）
   - 单元测试 `test_parse_header_values_skips_hash_comment_lines`（第 1433 行）覆盖同样的输入。
3. `crates/bifrost-core/src/rule/parser/mod.rs`
   - 单元测试 `test_parse_reqheaders_markdown_value_with_hash_lines`：证明 fenced block 中的 `#` / `##` 行不会导致前后 4 条规则丢失，也不会破坏 value 原文。
4. `crates/bifrost-sync/src/normalize.rs`
   - 单元测试 `preserves_markdown_value_blocks_with_hash_lines_during_normalization`：sync normalize 不删除 fenced block 或修改内容。

### 与 JSON object header 的互补关系

- 走 JSON 分支（`{"X-Env":"1"}`）时，`#` 字符如果出现在 JSON 里必须遵守 JSON 语法（e.g. 只能在字符串内）。
- 只有走「行式 header value」分支时，`#` 才作为整行注释被跳过。
- 两条分支的判定完全解耦；`looks_like_json_header_object` 命中就不进入行式分支。

### 依赖项

- `RuleParser::parse_rules_with_inline_values`
- `ValueSource::ValueRef`
- `parse_header_value` / `parse_header_values`
- `RulesResolver::with_values`

## CLI / Web / Admin API

### 用户写法示例

```
`https://api.corp.test/*` reqHeaders://{env_block}
`https://api.corp.test/*` reqCookies://{cookie_block}
`https://api.corp.test/*` reqBody://{body_block}
`https://api.corp.test/*` host://api-ppe.corp.test

```env_block
X-Test-Env: test_env
#comment_marker
## X-Ignored-Env: ignored comment
X-Test-Flag: 1
```

```cookie_block
sid: abc
#skip=this
```
```

### CLI

- `bifrost rule validate <name>` 无告警。
- `bifrost rule show <name>` 显示 fenced block 原文，`#` 行保留在展示中，只在运行时消费时被忽略。
- `bifrost traffic get <id>` 展示实际发出的请求头，只有 2 个 header。

### Web UI

- Monaco editor 语法高亮：`#` 开头行显示为注释色。
- 保存前 validate 通过。
- Traffic detail 展示实际 header 集合。

### Admin API

- `POST /api/rules/validate`：`#` 行不再触发 `E021`。
- `POST /api/replay`：命中 fenced header value 的规则时，`request_rules.rs::parse_header_values` 与 proxy 保持一致，`#` 行都被跳过。
- `GET /api/rules/:name`：返回原文，`#` 行保留。

## Sync 边界

- Value fenced block 内容随 rule content 一起同步；sync normalize 不删 `#` 行。
- 消费侧 (CLI/proxy/replay) 在本地跳过 `#` 行，不影响 sync payload。
- 老客户端读到新写法（含 `#` 行）时，如果它的消费侧没跟上这次修复，会退回旧行为（`## X-Ignored-Env` 变成非法 header 名）。因此升级需同步 CLI/desktop 版本；服务端 sync 层无需变更。

## 实现切分

### Phase 1：CLI 消费侧修复（已完成）

- `parse_header_value` 里加 `part.starts_with('#') → continue`。
- 单元测试 `parse_header_value_skips_hash_comment_lines`。

### Phase 2：Admin/Replay 消费侧同步（已完成）

- `crates/bifrost-admin/src/request_rules.rs::parse_header_value` 同款语义。
- 单元测试 `test_parse_header_values_skips_hash_comment_lines`。

### Phase 3：Parser / Sync 回归（已完成）

- `crates/bifrost-core/src/rule/parser/mod.rs`：`test_parse_reqheaders_markdown_value_with_hash_lines`。
- `crates/bifrost-sync/src/normalize.rs`：`preserves_markdown_value_blocks_with_hash_lines_during_normalization`。

### Phase 4：human_tests + 索引（已完成）

- `human_tests/proxy-rules-advanced.md`：新增 `TC-PRA-62`：粘用户中性示例规则文本，断言 4 条规则保留 + `env_block` value 原文保留 `#` 行 + 运行时 request headers 只包含 `X-Test-Env` 与 `X-Test-Flag`。
- `human_tests/readme.md`：把 TC-PRA-62 加入索引。

## 测试方案

### 单元测试

- `cargo test -p bifrost-core test_parse_reqheaders_markdown_value_with_hash_lines -- --nocapture`
- `cargo test -p bifrost-sync preserves_markdown_value_blocks_with_hash_lines_during_normalization -- --nocapture`
- `cargo test -p bifrost-cli parse_header_value_skips_hash_comment_lines -- --nocapture`
- `cargo test -p bifrost-cli test_reqheaders_markdown_value_skips_hash_comment_lines -- --nocapture`
- `cargo test -p bifrost-admin test_parse_header_values_skips_hash_comment_lines -- --nocapture`

### E2E

- `cargo test -p bifrost-e2e rule_validation -- --nocapture` 覆盖 Admin validate API 对 Markdown value block 与 active `reqHeaders` 的解析。
- 本次 bug 最小失效点是 header value 消费侧，已用 `bifrost-cli` resolver 单元测试覆盖真实运行时解析结果；未额外新增 E2E 脚本。

### 真实场景 human_tests

- `human_tests/proxy-rules-advanced.md` TC-PRA-62：
  - 前置：临时数据目录，非 9900 端口，`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`, `BIFROST_DISABLE_TRAY=1`, `--no-system-proxy`。
  - 步骤：粘规则文本 → CLI validate → 主端口发起请求 → traffic detail 检查 request headers。
  - 断言：只有 `X-Test-Env: test_env` 与 `X-Test-Flag: 1` 出现在真实请求；`## X-Ignored-Env` 完全不出现在任何 header 中。

### 项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 按需 `scripts/ci/local-ci.sh`；本次是 parser/request-rule 小范围修复，若未跑 local-ci 需在交付备注中说明风险。
- 本机 no-local-coverage 生效。

## Review/Fix/Test 闭环方案

### 第 1 轮

- Review：CLI 与 Admin/Replay 消费路径是否 100% 对齐；`#` 判定是否用 `trim().starts_with('#')` 避免 leading whitespace 逃逸；value 中间的合法 `#` 未被误删。
- 复测：最小单元测试 5 个 + `bifrost-e2e rule_validation`。

### 第 2 轮

- Review：human_tests 索引；design 文档和解析语义一致；`rule-header-json-compat` 中的 JSON 分支未被本次改动影响。
- 复测：workspace `cargo test`；`TC-PRA-62` 全量走一次。

## 风险与决策

- **`#` 作为整行注释可能覆盖用户合法内容**：例如 `X-Comment: #note-1`。因为判定用 `part.trim().starts_with('#')`，`X-Comment: #note-1` 的 part 是 `X-Comment: #note-1`，`trim()` 后以 `X` 开头，仍会按 header 处理。已通过 JSON / 括号形式测试锁定。
- **`##` 深注释也被识别**：`starts_with('#')` 同时命中 `#` 与 `##`；符合 markdown 语义。
- **老客户端行为回退**：升级 desktop 时需同步版本；sync 服务器不变。
- **JSON 分支冲突**：JSON 字符串里的 `#` 不会走行式分支，因为 JSON 已被 `looks_like_json_header_object` 提前分派。
- **性能**：每个 part 多一次 `starts_with('#')` 判定，`O(1)`，可忽略。

## 文档更新要求

- 更新 `human_tests/proxy-rules-advanced.md`（TC-PRA-62）
- 更新 `human_tests/readme.md` 索引
- Design 文档：本文件保持与实现同步；相关设计 `rule-header-json-compat.md` 中的 JSON 分支互补描述。
