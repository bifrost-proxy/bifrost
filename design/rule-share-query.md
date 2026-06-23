# Rule Share Query Protocol

## 0. 已确认产品决策

本功能第一版只做本地 My Rules 范围内的无损切换，不静默修改 Group 规则或远端同步规则状态。2026-06-23 安全修订后，代理入口只负责跳转到本机确认页；写规则必须由用户在确认页点击 Apply Rule 后触发。

已确认决策：

1. 分享 Query 参数名固定为 `__bifrost_rule`。
2. 分享 payload 必须同时携带规则名称和规则内容。
3. 访问分享链接后必须先展示本机确认页，确认页必须展示规则名称、内容 hash、独占范围、返回目标和完整规则内容。
4. 用户确认导入成功后必须启用目标规则，并禁用其它 My Rules。
5. `exclusive_scope` 第一版固定为 `my_rules`；`all` 不进入第一版实现。
6. 同名同内容必须复用，避免每次刷新产生重复规则。
7. 同名不同内容必须创建递增名称，例如 `name 2`、`name 3`。
8. 确认完成后必须重定向到 clean URL。
9. clean URL 必须移除 `__bifrost_rule`，并保留其它业务 query 和 fragment。
10. 导入成功后必须给用户可见提示，不能只写日志或只刷新规则列表。
11. Web UI 右键 Share 和 CLI 生成链接必须使用同一套协议编码。

第一版非目标：

- 不支持签名、加密、信任白名单或远端授权。
- 不修改 Group 规则 enabled 状态。
- 不把分享规则导入到 Group。
- 不支持 `exclusive_scope = all` 的行为，只保留 wire schema 扩展位。
- 不保留 POST/PUT 等非幂等原始方法；访问分享链接统一跳转确认页，确认后以普通页面跳转回 clean URL。

## 1. 用户目标验证清单

必须实现：

- 代理能识别目标 URL 上的 `__bifrost_rule` Query。
- payload 能还原出规则名称、规则内容、协议版本和内容 hash。
- 首次访问分享链接后只展示确认页，不创建或启用规则。
- 用户确认后创建或复用 My Rule。
- 确认导入成功后立即启用目标规则。
- 确认导入成功后禁用其它 My Rules。
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
- 分享链接返回本机确认页重定向，确认成功后返回 clean URL。
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

第一版 payload（与 `crates/bifrost-core/src/rule_share.rs` 实际实现一致）：

```json
{
  "version": 1,
  "name": "local-api",
  "content": "api.example.com host://127.0.0.1:3000",
  "mode": "enable_exclusive",
  "exclusive_scope": "my_rules",
  "content_hash_algorithm": "sha256",
  "content_hash": "<lower-hex sha256 of content>"
}
```

字段契约：

| 字段 | 必填 | Rust 类型 | 约束 |
| --- | --- | --- | --- |
| `version` | 是 | `u8` | 第一版只接受 `1`（常量 `RULE_SHARE_PROTOCOL_VERSION`） |
| `name` | 是 | `String` | trim 后不能为空，不能包含 `/` 或 `\` 路径分隔符 |
| `content` | 是 | `String` | trim 后不能为空；长度不超过 `MAX_RULE_FILE_BYTES`；导入侧再用 `validate_rules` 做语义校验 |
| `mode` | 否 | enum | 默认 `enable_exclusive`；第一版只接受该值 |
| `exclusive_scope` | 否 | enum | 默认 `my_rules`；第一版只接受该值 |
| `content_hash_algorithm` | 是 | `String` | 必须等于 `"sha256"`（常量 `RULE_SHARE_CONTENT_HASH_ALGORITHM`） |
| `content_hash` | 是 | `String` | 必须等于 `sha256(content)` 的 lower-hex；不再使用 `sha256:` 前缀 |

CLI `--exclusive-scope` 当前直接接受 `my_rules`（下划线），与 wire JSON 一致；没有引入 `my-rules` 别名。第一版没有顶层 `type: "bifrost.rule.share"` 字段——payload 通过 `__bifrost_rule` query 参数携带，已经天然在 Bifrost 协议下，本字段视为未来扩展位（planned, not yet shipped as of 2026-06-17）。

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

实际文件：`crates/bifrost-core/src/rule_share.rs`。

对外常量（全部带 `RULE_SHARE_` 前缀，便于按命名空间检索）：

```rust
pub const RULE_SHARE_QUERY_PARAM: &str = "__bifrost_rule";
pub const RULE_SHARE_PROTOCOL_VERSION: u8 = 1;
pub const RULE_SHARE_CONTENT_HASH_ALGORITHM: &str = "sha256";
pub const RULE_SHARE_IMPORTED_RULE_PREFIX: &str = "share/";
pub const RULE_SHARE_IMPORTED_DESCRIPTION_TITLE: &str = "Imported from a Bifrost rule share link";
pub const RULE_SHARE_IMPORTED_NAME_MARKER: &str = "bifrost-rule-share-name=";
pub const RULE_SHARE_IMPORTED_HASH_MARKER: &str = "bifrost-rule-share-sha256=";
```

对外类型：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareMode {
    #[default]
    EnableExclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareExclusiveScope {
    #[default]
    MyRules,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSharePayload {
    pub version: u8,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub mode: RuleShareMode,
    #[serde(default)]
    pub exclusive_scope: RuleShareExclusiveScope,
    pub content_hash_algorithm: String,
    pub content_hash: String,
}

pub struct RuleShareUrlParts {
    pub payload: Option<RuleSharePayload>,
    pub clean_url: String,
}
```

对外函数（实际签名）：

```rust
pub fn new_rule_share_payload(
    name: impl Into<String>,
    content: impl Into<String>,
) -> Result<RuleSharePayload>;

pub fn encode_rule_share_payload(payload: &RuleSharePayload) -> Result<String>;
pub fn decode_rule_share_payload(encoded: &str) -> Result<RuleSharePayload>;
pub fn append_rule_share_query(target_url: &str, payload: &RuleSharePayload) -> Result<String>;
pub fn extract_rule_share_query(input_url: &str) -> Result<RuleShareUrlParts>;

pub fn content_sha256(content: &str) -> String;
pub fn imported_rule_name(source_name: &str) -> String;
pub fn imported_rule_description(source_name: &str, content_hash: &str) -> String;
pub fn imported_rule_source_name(description: Option<&str>) -> Option<String>;
pub fn imported_rule_content_hash(description: Option<&str>) -> Option<String>;
pub fn share_payload_name_from_rule(name: &str, description: Option<&str>) -> String;
pub fn validate_payload(payload: &RuleSharePayload) -> Result<()>;
```

该模块只负责协议、hash、URL 以及 `share/` 命名空间的元数据辅助函数，不读写规则存储。

错误必须能区分（当前实现统一通过 `BifrostError::Config(msg)` 返回，message 中包含下列语义；细粒度错误码尚未拆分，planned, not yet shipped as of 2026-06-17）：

- `invalid_target_url`
- `unsupported_url_scheme`
- `payload_too_large`（编码后或解码后超出 `MAX_RULE_FILE_BYTES`）
- `invalid_base64`
- `invalid_json`
- `unsupported_version`
- `unsupported_content_hash_algorithm`
- `invalid_mode` / `invalid_scope`（由 serde rename_all 自动拒绝未知 variant）
- `hash_mismatch`
- `empty_name` / 名称含 `/` 或 `\`
- `empty_content`

### 4.2 `bifrost-admin::rule_share_import`

实际文件：`crates/bifrost-admin/src/rule_share_import.rs`。

对外类型（实际形态）：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareImportAction {
    Created,
    Reused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleShareImportOutcome {
    pub action: RuleShareImportAction,
    pub rule_name: String,
    pub requested_name: String,
    pub content_hash: String,
    pub disabled_rules: Vec<String>,
}
```

对外函数（实际签名，直接吃 `RuleSharePayload`，不再包一层 wrapper struct）：

```rust
pub async fn import_rule_share_payload(
    state: SharedAdminState,
    push_manager: Option<&SharedPushManager>,
    payload: RuleSharePayload,
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

第一版通知复用 `create_notification` + `push_manager.broadcast_notification(NotificationPushData{...})`，前端通过通用 notification 通道收到。

实际事件载荷（来自 `notify_after_import`）：

```json
{
  "notification_type": "rule_share_imported",
  "title": "Rule imported | Shared rule reused",
  "message": "Imported and enabled rule '<rule_name>'. Other My Rules were disabled.",
  "metadata": {
    "rule_name": "share/local-api",
    "created": true
  },
  "unread_count": 0
}
```

注意与早期设计的差异：

- 当前 metadata 只携带 `rule_name` 和 `created`（布尔），没有 `action`/`disabled_count` 字段。如果未来要让前端展示禁用了几条规则，需要扩展 metadata（planned, not yet shipped as of 2026-06-17）。
- `rule_name` 字段对应最终落盘的 `share/...` 名（可能带递增后缀），不是 payload 里的原始 `name`。
- 同时还会触发 `broadcast_settings_scope(SETTINGS_SCOPE_NOTIFICATIONS)`，让通知中心 badge 即时刷新。

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

裸域名输入统一规范成 `http://...` 后再附加 `__bifrost_rule`，确保默认分享链接在普通 HTTP 代理路径中可被 Bifrost 看到并导入；显式输入 `https://...` 时保持 HTTPS，由调用方自行确保目标域名已走 TLS 拦截。显式非 HTTP(S) scheme，例如 `ftp://...`，必须返回错误。

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

实际入口分为两处（具体见 `crates/bifrost-proxy/src/proxy/http/handler.rs` 与 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`）：

- 普通 HTTP absolute-form：`handle_http_request` 在 resolver 匹配前调用 `handle_rule_share_query(req, ctx, admin_state, push_manager)` 返回 `RuleShareProxyAction::{None, Redirect(url)}`。
- HTTPS TLS 解包后的 intercepted request：tunnel 模块调用 `handle_intercepted_rule_share_query(...)`，返回相同语义的 `InterceptedRuleShareAction`。

伪代码（与实现一致）：

```text
raw_url = ctx.url or get_request_url(req)
if !raw_url.contains("__bifrost_rule"):
    return None
parts = extract_rule_share_query(raw_url)        # 失败 -> warn + None
if parts.payload is None:
    return None
if admin_state is Some:
    confirm_url = "http://127.0.0.1:<admin_port>/_bifrost/share/rule?payload=...&target=<clean_url>"
    return Redirect(confirm_url)                # 上层映射到 302
return Redirect(parts.clean_url)                # admin 不可用时不写规则，只移除私有 query
```

实现要求：

- 处理发生在 `rules.resolve_with_context(...)` 前。
- 需要同时覆盖普通 HTTP absolute-form 请求路径和 HTTPS TLS 解包后的 intercepted request 路径；HTTPS 场景中先基于 `https://<original_host><path_and_query>` 还原完整 URL，再提取 `__bifrost_rule`，避免分享 query 被后续普通规则命中并消费。
- 代理层不直接调用 `import_rule_share_payload`，避免任意网页通过代理流量静默写入规则。
- 确认页 GET 只解码并展示 payload；真正写入发生在 `POST /api/rules/share-confirm`。
- `POST /api/rules/share-confirm` 复用 Admin API 浏览器写请求防护，浏览器上下文必须携带同源 CSRF token。
- admin 不可用时，代理只重定向到 clean URL，不创建规则。

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

响应（实际实现包含两项额外字段）：

```json
{
  "url": "https://example.com/path?a=1&__bifrost_rule=...",
  "payload_version": 1,
  "query_param": "__bifrost_rule",
  "rule_name": "local-api",
  "content_hash": "<lower-hex sha256>"
}
```

请求体支持可选 `exclusive_scope`（缺省 `my_rules`），即使传入也只参与校验，不影响生成的 payload；当前 handler 读完后会丢弃（实际上只生成 `my_rules` 一种 scope）。

错误：

| 状态码 | 场景 |
| --- | --- |
| 400 | 空 name、空 target_url、非法 URL、非 HTTP/HTTPS URL |
| 404 | name 对应的 My Rule 不存在 |
| 500 | 读取规则或编码失败 |

该 API 只读取 My Rules。第一版不做 Group 直接分享 API，也不在 Group 模式展示 Share 入口。

### 7.2 OpenAPI

`crates/bifrost-admin/src/openapi.rs` 必须补充 `/api/rules/share-link`，字段与错误码保持一致（planned, not yet shipped as of 2026-06-17 —— 当前 OpenAPI spec 中没有该路径定义，前端 / 第三方调用方需要直接参考本文档的请求与响应结构）。

## 8. CLI 设计

在 `RuleCommands` 下新增 `Share`（实际定义见 `crates/bifrost-cli/src/cli.rs`）：

```rust
Share {
    name: String,
    target_url: String,                       // 位置参数，不是 --url
    #[arg(short, long)]
    content: Option<String>,
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    file: Option<PathBuf>,
    #[arg(long, default_value = "my_rules", value_parser = ["my_rules"])]
    exclusive_scope: String,                  // wire 与 CLI 都用 my_rules（下划线）
}
```

行为：

- `--content` 和 `--file` 互斥；`--content` 优先于 `--file`。
- 都不传时，从 `RulesStorage::load(name)` 读取已有 My Rule 内容。
- 已存在 `share/<name>` 等导入规则时，CLI 通过 `share_payload_name_from_rule` 自动剥掉 `share/` 前缀，再从 description 里恢复原始分享名作为 payload 的 `name`。
- `--file` 第一版只读取纯规则内容文件，不接受 `.bifrost` 元数据文件作为输入。
- stdout 只输出最终 URL，方便 Agent 捕获。
- `--exclusive-scope` 当前只接受 `my_rules`；未引入 `my-rules` 别名（planned, not yet shipped as of 2026-06-17）。

示例（注意 target URL 是位置参数）：

```bash
bifrost rule share local-api https://example.com/path
bifrost rule share local-api https://example.com/path --content "api.example.com bp://127.0.0.1:3000"
bifrost rule share local-api https://example.com/path --file ./rules/local-api.txt
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

前端收到 `notification_type == "rule_share_imported"` 的通知后，可以直接展示后端 `message` 字段；实际文案与早期设计不同：

创建路径（metadata.created == true，title = "Rule imported"）：

```text
Imported and enabled rule '<final-name>'. Other My Rules were disabled.
```

复用路径（metadata.created == false，title = "Shared rule reused"）：

```text
Enabled existing rule '<final-name>'. Other My Rules were disabled.
```

当前 message 不包含被禁用规则的数量（与 §4.3 一致）。若未来要在前端展示 "Disabled N other My Rules"，需要在 admin import 侧扩展 metadata 把 disabled_rules 长度透出（planned, not yet shipped as of 2026-06-17）。

如果最终规则名是递增名（如 `share/local-api 2`），后端给出的 `metadata.rule_name` 就是该最终名；前端必须直接使用它，不要再剥掉 `share/` 前缀。

## 10. 文档更新

实际更新状态：

- `docs/rule.md`：已增加分享 Query 协议说明和示例。
- `docs/cli.md`：已增加 `bifrost rule share` 用法（目标 URL 是位置参数，不是 `--url`）。
- CLI help：`bifrost rule share -h` 能看到 target URL、content/file、scope 说明。
- `human_tests/readme.md`：已在表格中新增 `rule-share-query.md` 行（8 个用例）。

第一版不要求更新（仍未更新，符合预期）：

- `docs/cli-quick-start.md`：未加入 Agent 生成分享链接的短例子。

## 11. 测试方案

### 11.1 单元测试

`crates/bifrost-core/src/rule_share.rs`（实际命名）：

- `encode_decode_round_trip`
- `append_and_extract_preserves_site_query_and_fragment`
- `extract_removes_only_share_query_without_empty_query_suffix`
- `append_replaces_existing_share_query`
- `append_accepts_schemeless_domain_targets`
- `append_accepts_schemeless_localhost_port_targets`
- `append_rejects_explicit_non_http_scheme`
- `imported_rule_description_round_trips_source_name`
- `share_payload_name_strips_import_namespace_and_prefers_source_metadata`
- `decode_rejects_hash_mismatch`

下列原计划的细分用例仍未补齐（planned, not yet shipped as of 2026-06-17）：`decode_rejects_invalid_base64` / `decode_rejects_invalid_json` / `decode_rejects_wrong_type` / `decode_rejects_unsupported_version`。

`crates/bifrost-admin/src/rule_share_import.rs`（实际命名）：

- `import_reuses_same_name_same_content`
- `import_suffixes_same_name_different_content_and_disables_others`
- `import_does_not_overwrite_user_rule_with_original_name`
- `import_accepts_rule_reference_lines`
- `import_reopens_same_link_over_existing_share_rule`

下列原计划用例尚未拆出独立测试（部分语义由上述测试覆盖）：`import_creates_rule_when_name_unused` / `import_creates_numbered_name_when_same_name_differs` / `import_reuses_numbered_name_when_content_matches` / `import_exclusive_disables_other_my_rules_only` / `import_rejects_invalid_rule_content`（planned, not yet shipped as of 2026-06-17）。

`crates/bifrost-cli/src/commands/rule.rs`：当前没有针对 `rule share` 子命令的 Rust 单元测试（planned, not yet shipped as of 2026-06-17）；行为由 `e2e-tests/tests/test_rule_share_query.sh` 覆盖。

### 11.2 Web 测试

Vitest 或 Playwright：

- 右键 My Rule 显示 Share 菜单。
- Group 模式不显示 Share 菜单。
- 输入 URL 后调用 `/rules/share-link`。
- 展示返回 URL。
- Copy 按钮调用 clipboard helper。
- 收到 `rule_share_imported` push 后展示 toast/notification。

### 11.3 E2E 测试

实际 E2E 用例落在仓库根 shell 测试目录，而不是 Rust `bifrost-e2e`：

```text
e2e-tests/tests/test_rule_share_query.sh
```

Rust `crates/bifrost-e2e/src/tests/rule_share_query.rs` 仍未创建（planned, not yet shipped as of 2026-06-17）。当前 shell 脚本已经覆盖：裸域名经 HTTP 代理导入、HTTPS TLS 解包路径导入、带 `@规则引用` 的内容、重复访问不创建副本、同名不同内容创建递增 `share/<name> 2`、对其它 My Rules 独占启用等关键路径。

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
