# Rule Share Query Protocol

## 0. 已确认产品决策

本功能第一版只做本地 My Rules 范围内的无损切换，不静默修改 Group 规则或远端同步规则状态。

已确认决策：

1. 分享 Query 参数名固定为 `__bifrost_rule`。
2. 分享 payload 必须同时携带规则名称和规则内容。
3. 导入成功后必须启用目标规则，并禁用其它 My Rules。
4. `exclusive_scope` 第一版固定为 `my_rules`；`all` 不进入第一版实现。
5. 同名同内容必须复用，避免每次刷新产生重复规则。
6. 同名不同内容必须创建递增名称，例如 `name 2`、`name 3`。
7. `GET` / `HEAD` 访问分享链接后必须重定向到 clean URL。
8. clean URL 必须移除 `__bifrost_rule`，并保留其它业务 query 和 fragment。
9. 导入成功后必须给用户可见提示，不能只写日志或只刷新规则列表。
10. Web UI 右键 Share 和 CLI 生成链接必须使用同一套协议编码。

第一版非目标：

- 不支持签名、加密、信任白名单或远端授权。
- 不修改 Group 规则 enabled 状态。
- 不把分享规则导入到 Group。
- 不支持 `exclusive_scope = all` 的行为，只保留 wire schema 扩展位。
- 不支持 POST/PUT 等非幂等方法的重定向；这些方法只剥离私有 Query 后继续转发。

## 1. 用户目标验证清单

必须实现：

- 代理能识别目标 URL 上的 `__bifrost_rule` Query。
- payload 能还原出规则名称、规则内容、协议版本和内容 hash。
- 首次访问分享链接后自动创建或复用 My Rule。
- 导入成功后立即启用目标规则。
- 导入成功后禁用其它 My Rules。
- 同名同内容不创建新规则。
- 同名不同内容创建递增名称。
- Web UI 规则列表右键支持 Share。
- CLI 支持 Agent 直接生成分享链接。
- 成功导入后 Web UI 或 notification 展示用户可见提示。

必须不破坏：

- 普通业务 Query 不受影响。
- 目标网站不接收 `__bifrost_rule`。
- 已有 `rule add/update/enable/disable/reorder` 行为不变。
- Group 规则默认不被静默写脏。
- 运行中 resolver 通过 `RulesChanged` 重载。

必须真实验证：

- 使用真实 Bifrost 代理和临时数据目录访问分享链接。
- 使用 CLI 生成链接，再由代理导入。
- 使用 Web UI 右键 Share 生成并复制链接。
- `GET` / `HEAD` 分享链接返回 clean URL 重定向。
- 导入提示包含最终规则名和导入动作。
- 重复访问同一分享链接不会增加规则数量。
- 同名不同内容分享链接会创建递增规则名。

## 2. 当前工程约束

规则文件模型在 `crates/bifrost-storage/src/rules.rs`：

- `RuleFile::new(name, content)` 默认 enabled。
- `RulesStorage::save(...)` 写入 `.bifrost`。
- `RulesStorage::set_enabled(...)` 修改 enabled。
- `RulesStorage::load_all()` 读取 My Rules。
- `RulesStorage::load_enabled_with_subdirs_filtered(...)` 会读取 My Rules 和有效 Group 子目录规则。

Admin 规则 API 在 `crates/bifrost-admin/src/handlers/rules.rs`：

- `/api/rules` 创建规则时会设置最高优先级 sort order。
- `notify_rules_changed(...)` 会刷新 badge cache 并发送 `ConfigChangeEvent::RulesChanged`。
- 任何运行中导入都必须复用该通知路径，不能只依赖文件 watcher。

运行时热加载在 `crates/bifrost-cli/src/commands/start.rs`：

- `spawn_rules_watcher_task(...)` 收到 `RulesChanged` 后重新加载规则并更新 resolver。
- `spawn_rules_filesystem_watcher_task(...)` 只作为兜底扫描。

代理入口在 `crates/bifrost-proxy/src/proxy/http/handler.rs`：

- `handle_http_request(...)` 在调用 resolver 前已经拿到 URL、method、headers 和 cookies。
- 分享 Query 必须在 resolver 匹配前处理。
- `GET` / `HEAD` 通过重定向让第二次 clean 请求命中新 resolver。

Web UI 规则列表在 `web/src/pages/Rules/RuleList/index.tsx`：

- 每个规则项已经使用 Ant Design `Dropdown`，`trigger={['contextMenu']}`。
- Share 是现有右键菜单的新增动作，不需要重做列表交互模型。

## 3. 协议契约

### 3.1 Query 参数

固定参数名：

```text
__bifrost_rule
```

参数值：

- UTF-8 JSON。
- URL-safe base64 no padding 编码。
- JSON 反序列化前先做大小限制。

### 3.2 Payload JSON

第一版 payload：

```json
{
  "v": 1,
  "type": "bifrost.rule.share",
  "name": "local-api",
  "content": "api.example.com host://127.0.0.1:3000",
  "mode": "exclusive",
  "exclusive_scope": "my_rules",
  "content_sha256": "sha256:..."
}
```

字段契约：

| 字段 | 必填 | Rust 类型 | 约束 |
| --- | --- | --- | --- |
| `v` | 是 | `u32` | 第一版只接受 `1` |
| `type` | 是 | `String` | 必须等于 `bifrost.rule.share` |
| `name` | 是 | `String` | trim 后不能为空，不能包含路径穿越语义 |
| `content` | 是 | `String` | trim 后不能为空，必须通过规则 parser 校验 |
| `mode` | 否 | enum | 第一版只接受缺省或 `exclusive` |
| `exclusive_scope` | 否 | enum | 缺省为 `my_rules`；第一版只接受 `my_rules` |
| `content_sha256` | 否 | `String` | 存在时必须等于原始 content 的 `sha256:<hex>` |

CLI 参数使用 `--exclusive-scope my-rules`，wire JSON 使用 `my_rules`。CLI 层负责映射，避免 URL payload 混用两种拼写。

### 3.3 URL 构造与清理

`append_share_query(target_url, payload)` 必须满足：

- 只接受 `http://` 和 `https://`。
- 保留原 URL 的其它 query。
- 如果已有 `__bifrost_rule`，替换旧值。
- 保留 fragment。
- 返回完整 URL 字符串。

`remove_share_query(url)` 必须满足：

- 只移除 `__bifrost_rule`。
- 保留其它 query 的 key/value。
- query 为空时移除 `?`。
- 保留 fragment。

## 4. Rust 模块设计

### 4.1 `bifrost-core::rule_share`

新增文件：`crates/bifrost-core/src/rule_share.rs`。

对外常量：

```rust
pub const SHARE_QUERY_PARAM: &str = "__bifrost_rule";
pub const SHARE_PAYLOAD_TYPE: &str = "bifrost.rule.share";
pub const SHARE_PAYLOAD_VERSION: u32 = 1;
```

对外类型：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSharePayload {
    pub v: u32,
    #[serde(rename = "type")]
    pub payload_type: String,
    pub name: String,
    pub content: String,
    #[serde(default = "default_share_mode")]
    pub mode: RuleShareMode,
    #[serde(default = "default_exclusive_scope")]
    pub exclusive_scope: RuleShareExclusiveScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareMode {
    Exclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareExclusiveScope {
    MyRules,
}

pub struct RuleShareUrlParts {
    pub payload: Option<RuleSharePayload>,
    pub clean_url: String,
}
```

对外函数：

```rust
pub fn new_rule_share_payload(
    name: impl Into<String>,
    content: impl Into<String>,
) -> Result<RuleSharePayload>;

pub fn encode_rule_share_payload(payload: &RuleSharePayload) -> Result<String>;

pub fn decode_rule_share_payload(encoded: &str) -> Result<RuleSharePayload>;

pub fn append_rule_share_query(target_url: &str, payload: &RuleSharePayload) -> Result<String>;

pub fn extract_rule_share_query(request_url: &str) -> Result<RuleShareUrlParts>;

pub fn content_sha256(content: &str) -> String;
```

该模块只负责协议、hash、URL，不读写规则存储。

错误必须能区分：

- `invalid_target_url`
- `unsupported_url_scheme`
- `payload_too_large`
- `invalid_base64`
- `invalid_json`
- `unsupported_version`
- `invalid_payload_type`
- `invalid_mode`
- `invalid_scope`
- `hash_mismatch`
- `empty_name`
- `empty_content`

### 4.2 `bifrost-admin::rule_share_import`

新增文件：`crates/bifrost-admin/src/rule_share_import.rs`。

对外类型：

```rust
pub struct RuleShareImportRequest {
    pub payload: RuleSharePayload,
}

pub struct RuleShareImportOutcome {
    pub final_name: String,
    pub action: RuleShareImportAction,
    pub disabled_rule_names: Vec<String>,
}

pub enum RuleShareImportAction {
    Created,
    Reused,
}
```

对外函数：

```rust
pub async fn import_rule_share(
    state: &SharedAdminState,
    push_manager: Option<&SharedPushManager>,
    request: RuleShareImportRequest,
) -> Result<RuleShareImportOutcome>;
```

职责：

1. 校验 payload 中的规则内容能被 parser 接受。
2. 使用 normalized content 计算同名复用或递增新名称。
3. 创建或复用 My Rule。
4. 启用最终规则。
5. 禁用其它 My Rules。
6. 调用统一 `notify_rules_changed` 路径。
7. 触发用户可见提示。

该模块只操作 `state.rules_storage` 的 My Rules 根目录，不遍历 Group 子目录，不修改 Group 规则 enabled。

### 4.3 `RuleShareImportOutcome` 通知

第一版通知使用现有 Admin push 或 notification 能力，要求前端可见。

事件内容必须至少包含：

```json
{
  "type": "rule_share_imported",
  "rule_name": "local-api",
  "action": "created",
  "disabled_count": 3
}
```

如果当前 push/notification 结构无法直接承载该事件，新增最小 notification 类型，不要把提示耦合到 traffic 记录或 badge JSON。

## 5. 导入算法

### 5.1 名称选择

伪代码：

```rust
fn resolve_final_name(storage: &RulesStorage, source_name: &str, content_hash: &str) -> Result<NameDecision> {
    let base_name = format!("share/{source_name}");
    let mut index = 1;

    // 先通过导入元数据查找同一条分享链接，即使用户后来编辑过 share 规则内容，也要覆盖回同一条规则。
    loop {
        let candidate = if index == 1 {
            base_name.to_string()
        } else {
            format!("{base_name} {index}")
        };

        if !storage.exists(&candidate) {
            return Ok(NameDecision::Create(candidate));
        }

        let existing = storage.load(&candidate)?;
        if existing.description_has("bifrost-rule-share-name", source_name)
            && existing.description_has("bifrost-rule-share-sha256", content_hash)
        {
            return Ok(NameDecision::Reuse(candidate));
        }

        index += 1;
        if index > 1000 {
            return Err("too many conflicting rule names");
        }
    }
}
```

注意：

- 导入本地规则名固定落到 `share/` 命名空间，普通用户规则 `name` 不会被覆盖。
- `index == 1` 使用 `share/name`。
- 第一个冲突新名称是 `share/name 2`。
- 每条导入规则的 description 必须记录原始分享名和内容 hash：`bifrost-rule-share-name=<urlencoded name>`、`bifrost-rule-share-sha256=<hash>`。
- 同一分享链接重复打开时，通过元数据复用并覆盖同一条 `share/...` 规则，避免用户编辑该 share 规则后下次打开又产生新规则。
- 同名但不同内容的分享链接不能覆盖既有 `share/...` 规则，必须创建递增后缀。
- CLI / Web UI / Admin API 再次分享 `share/...` 规则时，payload 名称必须剥掉 `share/` 前缀，并优先使用 description 中的原始分享名。
- 上限 1000 是防御性限制，避免异常目录导致无限循环。

### 5.2 目标 URL 规范化

生成分享链接的目标网站支持：

- 完整 `http://` / `https://` URL。
- 裸域名或 host:port，例如 `a.com`、`example.com/path`、`localhost:3000`。

裸域名输入统一规范成 `https://...` 后再附加 `__bifrost_rule`。显式非 HTTP(S) scheme，例如 `ftp://...`，必须返回错误。

### 5.3 Exclusive My Rules

伪代码：

```rust
fn apply_exclusive_my_rules(storage: &RulesStorage, final_name: &str) -> Result<Vec<String>> {
    let mut disabled = Vec::new();

    for name in storage.list()? {
        let mut rule = storage.load(&name)?;
        let should_enable = name == final_name;

        if rule.enabled != should_enable {
            rule.enabled = should_enable;
            rule.touch_local_change();
            storage.save(&rule)?;

            if !should_enable {
                disabled.push(name);
            }
        }
    }

    Ok(disabled)
}
```

注意：

- 只使用 `RulesStorage::list()` 读取 My Rules 根目录。
- 不调用 `load_all_with_subdirs`，避免影响 Group。
- 如果最终规则是新建规则，先 save，再执行 exclusive。
- 如果最终规则是复用规则，也必须确保它 enabled。

### 5.3 事务边界

当前规则存储是文件系统，没有跨文件事务。第一版采用可恢复顺序：

1. 校验 payload 和规则 parser。
2. 选定最终名称。
3. 如果需要创建，先写目标规则。
4. 逐个启停 My Rules。
5. 最后发送 `RulesChanged`。

失败处理：

- 步骤 1 或 2 失败：不写文件。
- 步骤 3 失败：不修改 enabled。
- 步骤 4 中途失败：返回错误并记录已经修改的规则；不做自动回滚，因为现有规则 API 也不是事务化。最终交付测试必须覆盖正常路径。
- 步骤 5 失败：返回导入失败，但文件状态已经改变；日志必须明确 reload notification failed。

## 6. 代理接入设计

在 `handle_http_request(...)` 中，resolver 匹配前加入：

```text
raw_url = ctx.url or req.uri().to_string()
parts = extract_rule_share_query(raw_url)
if parts.payload exists:
    outcome = import_rule_share(...)
    emit notification(outcome)
    if method is GET or HEAD:
        return redirect(parts.clean_url)
    else:
        rewrite req.uri to clean URL path/query
        use parts.clean_url for resolver match URL
```

实现要求：

- 处理发生在 `rules.resolve_with_context(...)` 前。
- 成功导入后的 `GET` / `HEAD` 不继续转发原请求，固定返回 302 到 clean URL，保持浏览器普通导航行为。
- 非 `GET` / `HEAD` 不重定向，避免改变请求方法和 body 语义。
- 非 `GET` / `HEAD` 必须剥离上游请求中的 `__bifrost_rule`。
- 导入失败时不修改规则；`GET` / `HEAD` 仍可重定向到 clean URL，并通过 notification/log 说明失败。
- proxy 层不直接拼写规则存储逻辑，只调用 admin import service。

## 7. Admin API 设计

### 7.1 生成分享链接

新增：

```text
POST /api/rules/share-link
```

请求：

```json
{
  "name": "local-api",
  "target_url": "https://example.com/path?a=1",
  "exclusive_scope": "my_rules"
}
```

响应：

```json
{
  "url": "https://example.com/path?a=1&__bifrost_rule=...",
  "payload_version": 1,
  "query_param": "__bifrost_rule"
}
```

错误：

| 状态码 | 场景 |
| --- | --- |
| 400 | 空 name、空 target_url、非法 URL、非 HTTP/HTTPS URL |
| 404 | name 对应的 My Rule 不存在 |
| 500 | 读取规则或编码失败 |

该 API 只读取 My Rules。第一版不做 Group 直接分享 API，也不在 Group 模式展示 Share 入口。

### 7.2 OpenAPI

`crates/bifrost-admin/src/openapi.rs` 必须补充 `/api/rules/share-link`，字段与错误码保持一致。

## 8. CLI 设计

在 `RuleCommands` 下新增 `Share`：

```rust
Share {
    name: String,
    #[arg(long)]
    url: String,
    #[arg(short, long)]
    content: Option<String>,
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    file: Option<PathBuf>,
    #[arg(long, value_parser = ["my-rules"], default_value = "my-rules")]
    exclusive_scope: String,
}
```

行为：

- `--content` 和 `--file` 互斥。
- 都不传时，从 `RulesStorage::load(name)` 读取已有 My Rule 内容。
- `--file` 第一版只读取纯规则内容文件，不接受 `.bifrost` 元数据文件作为输入。
- stdout 只输出最终 URL，方便 Agent 捕获。
- warning 输出到 stderr，例如 URL 过长。

示例：

```bash
bifrost rule share local-api --url https://example.com/path
bifrost rule share local-api -c "api.example.com host://127.0.0.1:3000" --url https://example.com/path
bifrost rule share local-api -f ./rules/local-api.txt --url https://example.com/path
```

## 9. Web UI 设计

### 9.1 右键菜单

在 `web/src/pages/Rules/RuleList/index.tsx` 的 My Rules 右键菜单中新增：

- key: `share`
- label: `Share`
- icon: 使用现有 icon 库中的分享/链接图标。

第一版只在 My Rules 模式展示 Share。Group 模式不展示 Share，避免让用户误以为导入后会写回 Group。

### 9.2 Share Modal

Modal 状态：

- target URL input。
- generated URL readonly textarea/input。
- Generate 按钮。
- Copy 按钮。
- loading 和 error 状态。

交互：

1. 用户右键规则，点击 Share。
2. 输入目标 URL。
3. 点击 Generate。
4. 前端调用 `POST /api/rules/share-link`。
5. 展示返回 URL。
6. Copy 使用 `copyToClipboard(...)`。

### 9.3 导入成功提示

前端收到 `rule_share_imported` 事件后展示：

```text
Imported and enabled rule "local-api". Disabled 3 other My Rules.
```

如果 action 是 `reused`：

```text
Reused and enabled rule "local-api". Disabled 3 other My Rules.
```

如果最终规则名是递增名，提示必须使用最终规则名。

## 10. 文档更新

必须更新：

- `docs/rule.md`：增加分享 Query 协议说明和安全提示。
- `docs/cli.md`：增加 `bifrost rule share` 用法。
- CLI help：`bifrost rule share -h` 能看到目标 URL、content/file、scope 说明。
- `human_tests/readme.md`：新增 rule share query 用例索引。

第一版不要求更新：

- `docs/cli-quick-start.md`：加入 Agent 生成分享链接的短例子。

## 11. 测试方案

### 11.1 单元测试

`crates/bifrost-core/src/rule_share.rs`：

- `encode_decode_round_trip_preserves_payload`
- `decode_rejects_invalid_base64`
- `decode_rejects_invalid_json`
- `decode_rejects_wrong_type`
- `decode_rejects_unsupported_version`
- `decode_rejects_hash_mismatch`
- `append_share_query_preserves_existing_query_and_fragment`
- `append_share_query_replaces_existing_share_query`
- `append_share_query_accepts_schemeless_domain`
- `append_share_query_accepts_schemeless_host_port`
- `append_share_query_rejects_explicit_non_http_scheme`
- `extract_share_query_returns_payload_and_clean_url`
- `extract_share_query_preserves_business_query`
- `share_payload_name_strips_import_namespace_and_prefers_source_metadata`

`crates/bifrost-admin/src/rule_share_import.rs`：

- `import_creates_rule_when_name_unused`
- `import_reuses_same_name_when_content_matches`
- `import_creates_numbered_name_when_same_name_differs`
- `import_reuses_numbered_name_when_content_matches`
- `import_does_not_overwrite_user_rule_with_original_name`
- `import_reopens_same_link_over_existing_share_rule`
- `import_exclusive_disables_other_my_rules_only`
- `import_rejects_invalid_rule_content`

`crates/bifrost-cli/src/commands/rule.rs`：

- `rule_share_from_inline_content_outputs_url`
- `rule_share_from_existing_rule_outputs_url`
- `rule_share_rejects_non_http_url`

### 11.2 Web 测试

Vitest 或 Playwright：

- 右键 My Rule 显示 Share 菜单。
- Group 模式不显示 Share 菜单。
- 输入 URL 后调用 `/rules/share-link`。
- 展示返回 URL。
- Copy 按钮调用 clipboard helper。
- 收到 `rule_share_imported` push 后展示 toast/notification。

### 11.3 E2E 测试

新增 Bifrost E2E 用例：

```text
crates/bifrost-e2e/src/tests/rule_share_query.rs
```

真实流程：

1. 创建临时 `BIFROST_DATA_DIR`。
2. 启动 Bifrost：

```bash
BIFROST_DATA_DIR=<tmp> \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
cargo run --bin bifrost -- start -p <port> --unsafe-ssl --no-system-proxy
```

3. 创建并启用旧规则 `old-rule`。
4. 生成分享链接 `local-api`。
5. `curl -I -x http://127.0.0.1:<port> "<share-url>"`。
6. 断言返回 302，Location 为 clean URL，不含 `__bifrost_rule`。
7. 断言 `bifrost rule list` 中 `local-api` enabled、`old-rule` disabled。
8. 再次访问相同分享链接。
9. 断言规则数量不增加。
10. 生成同名不同内容分享链接并访问。
11. 断言创建 `local-api 2`。

### 11.4 human_tests

新增 `human_tests/rule-share-query.md`，并立即执行。

用例：

- TC-RSQ-01：CLI 用已有规则生成分享链接。
- TC-RSQ-02：CLI 用 inline content 生成分享链接。
- TC-RSQ-03：访问分享链接后自动导入并启用规则。
- TC-RSQ-04：GET/HEAD 访问分享链接后重定向到 clean URL。
- TC-RSQ-05：导入成功后 Web UI 或 notification 展示最终规则名提示。
- TC-RSQ-06：重复刷新同一链接不创建重复规则。
- TC-RSQ-07：同名不同内容创建递增规则名。
- TC-RSQ-08：Web UI 右键 Share 生成并复制链接。

### 11.5 收尾验证命令

开发完成后至少执行：

```bash
cargo test -p bifrost-core rule_share
cargo test -p bifrost-admin rule_share_import
cargo test -p bifrost-cli rule_share
cargo test -p bifrost-e2e rule_share_query -- --nocapture
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

如果改动包含 Web UI：

```bash
pnpm --dir web test -- --run
pnpm --dir web lint
pnpm --dir web build
```

最终按仓库规则再执行 `rust-project-validate` 和必要的 local-ci。

## 12. 实施拆分

按以下顺序推进，避免 UI 先行但协议不可测：

1. `bifrost-core::rule_share` 协议模块和单元测试。
2. CLI `bifrost rule share`，先证明 Agent 可生成链接。
3. `bifrost-admin::rule_share_import` 导入服务和单元测试。
4. proxy 请求入口接入，完成 GET/HEAD clean URL 重定向。
5. notification/push 事件。
6. Admin share-link API 和 OpenAPI。
7. Web UI 右键 Share Modal。
8. docs、human_tests、E2E。
9. 两轮 Review/Fix/Test。
10. 提交、推送、PR、远端 CI 看护。

每一步完成后必须能独立验证，不允许等最后一次性补测试。

## 13. Review/Fix/Test 闭环

第 1 轮：

- 复核 payload 协议、URL 清理、导入算法和 exclusive 范围。
- 执行 `git status --short`、`git diff`。
- Review `rule_share.rs`、`rule_share_import.rs`、proxy 入口和 CLI。
- 跑 core/admin/cli focused tests。
- 修复发现的问题并复跑失败命令。

第 2 轮：

- 复核 Web/API/docs/human_tests 是否和实现一致。
- 再次执行 `git status --short`、`git diff`。
- Review E2E、Web UI、notification 文案和 clean URL 行为。
- 跑 E2E、human_tests、`cargo test --workspace --all-features`。
- 如果仍发现缺口，追加第 3 轮。

## 14. 完成定义

只有同时满足以下条件才算完成：

- CLI 能生成分享链接。
- Web UI 能生成并复制分享链接。
- 代理能导入分享 Query。
- GET/HEAD 返回 clean URL 重定向。
- My Rules exclusive 行为正确。
- 同名同内容复用。
- 同名不同内容递增。
- 导入成功提示用户可见。
- docs 和 human_tests 同步。
- focused tests、E2E、human_tests、workspace all-features、clippy 通过。
- 远端 CI 通过或有明确外部阻塞证据。
