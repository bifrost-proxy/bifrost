# Header 规则 JSON Object 兼容

## 背景

`reqHeaders://` / `resHeaders://` / `reqCookies://` / `trailers://` 过去主要接受两类写法：

- 行式 header：`X-Env: ppe`、`X-Env=ppe`（多行按 `\n` 拆分，单行按 `,` 拆分）
- 值引用：`reqHeaders://{env_headers}` 通过 `@values` fenced block 注入

但文档和 human_tests 里长期同时存在两种混乱信号：

- 一些说明把 JSON object 描述成可用于内联参数。
- 一些旧 human_tests 写成 `resHeaders://{X-Test: value}` —— 既不是合法 JSON，也不是合法值引用，运行时的行为无法预测。

Coding Agent 在缺少精确约束时会天然选择 JSON object，因为它是表达 multi-key 的标准结构。真实用户提供的 `reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1"}` 就是这种风格。

**目标：把这种 JSON object 写法正式变成一等公民，同时不破坏值引用语义。**

## 用户目标验证清单

### 必须实现

- `reqHeaders://` / `resHeaders://` / `reqCookies://` / `trailers://` 直接接受 JSON object 字面量：
  - `reqHeaders://{"X-Env":"ppe","X-Flag":"1"}`
  - 支持数字 / bool / null（`null` → 空字符串）
- 括号包裹形式：`resHeaders://({"Cache-Control":"max-age=3600, public"})`，用于内含空格或逗号的值。
- 保留 `{name}` 值引用语义（例如 `{env_headers}`、`{cn_nextagent_ppe_headers}`）。
- 内容看起来是 JSON object 时（空对象、以双引号开头 key、或包含 `:`）走 JSON 分支；否则走值引用分支。
- Malformed JSON 时：
  - 运行时返回空 header 列表，绝不再回退到旧的“按 `:` 拆”逻辑（避免生成 `{"x"` 之类非法 header 名导致 502）。
  - 语法校验负责给用户明确的 `E021` 错误。
- JSON value 中 array / nested object 被忽略（跳过而不是报错），保持 header 生成安全。

### 必须不破坏

- 旧的多行 header（`X-Env: ppe\nX-Flag: 1`）继续生效。
- 单行逗号分隔（`X-Env=ppe, X-Flag=1`）继续生效。
- `{name}` 值引用继续正常解析。
- CORS `reqCors://` / `resCors://` 的 JSON 配置解析器不被误伤。
- Admin replay（`crates/bifrost-admin/src/request_rules.rs`）与 CLI 解析（`crates/bifrost-cli/src/parsing/headers.rs`）语义一致，不出现“admin 能解析、代理不能生效”的漂移。

### 必须真实验证

- 真实 9900 规则里粘一条 `reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1"}`，经代理发出的请求头包含这两个 header。
- 括号形式 `resHeaders://({"Cache-Control":"max-age=3600, public"})` 命中真实响应。
- Malformed `reqHeaders://{"x-tt-env":}` 不再触发 `Bad Gateway`，运行时表现为「无 header 写入」。

## 产品语义

**JSON object 是 header/cookie/trailer 的正式内联表达形式**。

分派规则（`crates/bifrost-cli/src/parsing/headers.rs::parse_header_value`）：

1. `trim()` 后：
   - 若形如 `(...)`，脱掉括号，`use_colon = true`。
   - 否则原样，`use_colon = 内容含 \n 或 :`。
2. 判定是否 `looks_like_json_header_object`：内容以 `{` 开始、`}` 结束，且内层为空 / 以 `"` 开头 / 包含 `:`。命中即走 `parse_json_header_object`。
3. JSON 解析：`serde_json::from_str::<Value>(content)`；失败返回空列表（不 fallback）。
4. Object 的每个 key/value：
   - Key 为空 → 跳过
   - Value String / Number / Bool / Null → 转字符串
   - Value Array / Object → `None`，被 filter 掉
5. 非 JSON 分支：按 `\n` 或 `,` 拆分，跳过空行和以 `#` 开头的注释，按 `:` / `=` 拆 key/value。

值引用（`{name}`）分派：语法解析器 (`crates/bifrost-core/src/rule/parser`) 已经在词法阶段区分 `{ident}` 与 JSON object；只有当内容满足上面 step 2 的“像 JSON”特征时才被视作 JSON。`{env_headers}` 因不满足（无 `:`、无 `"`），仍进值引用路径。

## 技术细节

### CLI 与代理侧

- `crates/bifrost-cli/src/parsing/headers.rs`
  - `parse_header_value(value) -> Option<Vec<(String, String)>>`：入口
  - `looks_like_json_header_object`：JSON object 判定
  - `parse_json_header_object`：`serde_json::from_str` + object 遍历
  - `json_scalar_to_header_value`：只放行 String / Number / Bool / Null
- 单元测试（同文件 `#[cfg(test)] mod tests`）：
  - `parse_header_value_supports_json_object` (215 行)
  - `parse_header_value_supports_parenthesized_json_object` (232 行)
  - `parse_header_value_ignores_nested_json_values` (252 行)
  - `parse_header_value_does_not_fallback_for_malformed_json_object` (261 行)
  - `parse_header_value_skips_hash_comment_lines` (200 行，与 markdown values fix 共用)

### Admin / Replay

- `crates/bifrost-admin/src/request_rules.rs`
  - `parse_header_values` (第 766 行)：与 CLI 逻辑同款；`looks_like_json_header_object` (第 833 行)、`parse_json_header_object` (第 813 行)、`json_scalar_to_header_value` (第 842 行)。
  - Replay 请求 (`req_headers`) / 响应 (`res_headers`) 都使用 `.extend(parse_header_values(&rule.resolved_value))`。
- 单元测试：
  - `test_parse_header_values_supports_json_object` (1447 行)
  - `test_parse_header_values_does_not_fallback_for_malformed_json_object` (1221 行)
  - `test_parse_header_values_multiline` (1421 行)
  - `test_parse_header_values_skips_hash_comment_lines` (1433 行)
  - 大 integration test (`crates/bifrost-admin/src/request_rules.rs` 1466 行) 覆盖 `reqHeaders://(X-Trace: old)` + `reqCookies://` + `delete://` + `urlParams://` + `headerReplace://` + `reqCors://` + `reqMerge://` 的完整链路。

### 语法校验

- `crates/bifrost-core/src/syntax.rs` / `crates/bifrost-core/src/rule/parser`
  - 对合法 JSON object 放行。
  - 对 malformed / nested / empty JSON object 报 `E021`（`header value must be a JSON object of scalars, header lines, or a value reference`）。
  - `Admin POST /api/rules/validate` 直接返回该错误码。

### Sync 边界

- JSON object 字面量作为规则内容随 sync 一起分发，远端无需额外处理。
- 值引用 `{name}` 与配套的 `@values` block 已经在 sync 层做过 normalize（见 `crates/bifrost-sync/src/normalize.rs`），本变更只多接受 JSON object 分支，未新增 sync 边界。

## CLI / Web / Admin API

### CLI 示例

```bash
# JSON object（首选）
bifrost rule update NextOncall --content \
  '*.nextoncall.byted.org reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1"}'

# 括号 + JSON（内含空格 / 逗号的值）
bifrost rule update Cache --content \
  '*.static.example.com resHeaders://({"Cache-Control":"max-age=3600, public"})'

# 值引用（继续可用）
bifrost rule update Env --content '*.corp.test reqHeaders://{env_headers}'
```

`bifrost rule validate <name>` 会返回 `E021` 当且仅当 JSON 语法错误。

### Web UI

- Rule editor 语法高亮 JSON object 部分。
- 保存前调用 `POST /api/rules/validate`；`E021` 弹出可读错误。
- 编辑器 hover tips 引用最新语法说明。

### Admin API

- `POST /api/rules/validate`：body `{ "content": "<rule>" }` → `{ "valid": bool, "errors": [{ "code": "E021", "message": "...", "line": N }] }`
- `PUT /api/rules/:name`：保存前先跑同一 validator，malformed JSON 返回 400。
- `POST /api/replay`：命中 `reqHeaders://{...}` / `resHeaders://` 的规则时，`request_rules.rs::parse_header_values` 与 proxy 保持一致。

## 实现切分

### Phase 1：CLI 解析器 + 单元测试（已完成）

- `parse_header_value` 增 JSON 分支。
- 覆盖 JSON scalar / null / nested / malformed 单元测试。
- `#` 注释行跳过（与 markdown values fix 合并）。

### Phase 2：Admin replay 解析器（已完成）

- `crates/bifrost-admin/src/request_rules.rs::parse_header_values` 与 CLI 语义一致。
- 大 integration test 覆盖 `reqHeaders`/`reqCookies`/`urlParams`/`headerReplace`/`reqCors`/`reqMerge` 链路。

### Phase 3：语法校验（已完成）

- `E021` 覆盖 malformed / nested / empty JSON object。
- `POST /api/rules/validate` 直接暴露给 Web UI。

### Phase 4：文档 + human_tests（已完成 / 进行中）

- 修正 `human_tests/rule-merge-headers.md`：
  - `TC-RMH-07`（planned or already added, 需 grep 确认）：验证 NextOncall 风格 JSON object header rule。
- 更新 `docs/rule.md` header 章节：显式声明 JSON object 首选写法，并给出 `(json)` 括号形式示例。
- 清理旧例如 `resHeaders://{X: y}` → `resHeaders://(X: y)` 或合法 JSON object。

## 测试方案

### 单元测试

- `cargo test -p bifrost-cli parse_header_value_supports_json_object`
- `cargo test -p bifrost-cli parse_header_value_supports_parenthesized_json_object`
- `cargo test -p bifrost-cli parse_header_value_ignores_nested_json_values`
- `cargo test -p bifrost-cli parse_header_value_does_not_fallback_for_malformed_json_object`
- `cargo test -p bifrost-cli parse_header_value_skips_hash_comment_lines`
- `cargo test -p bifrost-admin test_parse_header_values_supports_json_object`
- `cargo test -p bifrost-admin test_parse_header_values_does_not_fallback_for_malformed_json_object`
- `cargo test -p bifrost-admin test_parse_header_values_multiline`
- `cargo test -p bifrost-admin test_parse_header_values_skips_hash_comment_lines`
- `cargo test -p bifrost-core rule::parser E021`

### E2E

- `bifrost-e2e` 中的 `req_headers_json_object`（planned, not yet shipped as of 2026-06-17；关键词搜索仍未命中已存在的 shell 脚本）
- `e2e-tests/rules/request_modify/headers.txt` 加 JSON object 夹具（planned）
- 现有的 `e2e-tests/test_rules.sh` header 场景保持通过

### 真实场景 human_tests

- `human_tests/rule-merge-headers.md` TC-RMH-07：真实规则 `reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1","x-tt-env-fe":"dev"}` → 代理请求头包含三个 header。
- 备注：`grep` 结果表明 `human_tests/rule-merge-headers.md` 已在仓库中，TC-RMH-07 需具体核对存在性；若未添加，为本次 doc-refresh 后跟进项。

### 项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 本机 no-local-coverage：不跑 `make coverage`；依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- Review：CLI / Admin 两条解析路径是否语义一致；`looks_like_json_header_object` 是否覆盖 `{}` / `{"…}` / `{a:b}` 三类；malformed 时确实返回空 list；nested value 被跳过。
- 复测：CLI + Admin 单元测试；真实 9900 规则粘 NextOncall JSON object。

### 第 2 轮

- Review：文档旧示例是否已改；human_tests TC-RMH-07 是否上线；`E021` 错误信息稳定；不误伤 CORS `reqCors://({...})` 的 JSON 配置分支。
- 复测：workspace 级 cargo test + 关键 E2E。

## 风险与决策

- **兼容 JSON object 让“过去非法但无效”的规则突然生效**：这是目标行为；文档中说明变更。
- **Malformed JSON 运行时 no-op**：避免代理 502；用户侧错误由语法校验和保存校验暴露。
- **值引用 vs JSON 判定**：只在满足“空对象 / 以 `"` 开头 / 含 `:`”时视作 JSON。`{env_headers}` 因不含 `:`，仍走值引用；已通过单元测试锁定该分支。
- **和 `reqCors://` 的 JSON 冲突**：CORS 分派器（`crates/bifrost-cli/src/parsing/headers.rs::parse_cors_config` 87 行起）单独入口，与 header parser 不共享判定，故不互相干扰。
- **人 review 门槛**：JSON object 里的 `:` 与 header 分隔符相同；启用 IDE 侧配色可显著降低误读。
