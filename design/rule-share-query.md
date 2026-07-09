# Rule Share Query Protocol

## 背景

Bifrost 用户经常需要把一条本地规则临时分发给其它同事、AI Agent 或另一台设备。传统做法是导出 `.bifrost` 文件后再手工导入，中间还要指导对方“打开哪个页面才能命中这条规则”，很难做到“打开一个链接就能立刻生效”。

本方案在 URL Query 上定义一个专用参数 `__bifrost_rule`，让 CLI、Web UI 和 Agent 可以把规则内容打包附加到目标业务 URL 上。当 Bifrost 代理看到这个 Query 时，会强制先跳转到本机 Admin 的确认页，让用户在浏览器里显式点击 Apply Rule，才真正把规则写入 My Rules 并启用；确认成功后再跳转回移除了 `__bifrost_rule` 的 clean URL。

2026-06-23 安全修订后，代理入口只负责跳转到本机确认页；写规则必须由用户在确认页点击 Apply Rule 后触发。第一版严格限定在 My Rules 范围内，不会静默修改 Group 规则或触发远端同步。

## 用户目标验证清单

### 必须实现

- 代理能识别目标 URL 上的 `__bifrost_rule` Query。
- payload 能还原出规则名称、规则内容、协议版本、`content_hash_algorithm` 和 lower-hex `content_hash`。
- 首次访问分享链接后只展示确认页，不创建或启用规则。
- 确认页响应必须携带 `Content-Security-Policy: frame-ancestors 'none'` 与 `X-Frame-Options: DENY`，防止 clickjacking。
- 确认页必须展示完整内容 hash，但 Apply 按钮不要求用户手工输入 hash。
- `POST /_bifrost/api/rules/share-confirm` 必须校验 payload、target_url 与浏览器上下文 CSRF；请求体不要求 `confirmation` 字段。
- 用户确认后创建或复用 My Rule。
- 确认导入成功后立即启用目标规则，并禁用其它 My Rules（Default 全局规则不受影响）。
- 同名同内容不创建新规则；同名不同内容创建 `share/<name> 2` 递增名称。
- Web UI 规则列表右键支持 Share，CLI `bifrost rule share <name> <target_url>` 也能生成同一套链接。
- 成功导入后 Web UI 或 notification 展示用户可见提示（`notification_type = rule_share_imported`）。
- 退出分享 env 时通过 `exit_rule_share_env` 恢复用户此前的 enabled 集合。

### 必须不破坏

- 普通业务 Query 不受影响；`__bifrost_rule` 在转发给上游前必须移除。
- 已有 `rule add/update/enable/disable/reorder` 行为不变。
- Group 规则默认不被静默写脏，也不接受分享导入。
- 运行中 resolver 只通过 `ConfigChangeEvent::RulesChanged` 重载，避免绕过热更新链路。
- Default 全局规则的 enabled 状态在 exclusive 期间保持不变。

### 必须真实验证

- 使用真实 Bifrost 代理和临时数据目录访问 HTTP / HTTPS 分享链接。
- 使用 CLI 生成链接再由代理导入。
- 使用 Web UI 右键 Share 生成并复制链接。
- 分享链接返回 302 跳到本机确认页；用户点击 Apply Rule 后再回到 clean URL。
- 确认页安全头真实存在，跨站/缺 CSRF 的确认请求均不能导入规则。
- 重复访问同一分享链接不会增加规则数量。
- 同名不同内容分享链接会创建递增规则名。

## 产品语义

### 分享链接的独立命名空间

导入落盘的规则统一放在 `share/` 命名空间下，例如 `share/local-api`、`share/local-api 2`。这样保证：

- 用户已有的普通 My Rule `local-api` 不会被静默覆盖。
- CLI/Web/Agent 再次分享 `share/...` 规则时，能通过 description 里的 `bifrost-rule-share-name=` / `bifrost-rule-share-sha256=` 标记还原原始分享名和内容 hash（详见 `bifrost_core::rule_share::share_payload_name_from_rule`）。

### Exclusive My Rules，但保留 Default

`mode = enable_exclusive` + `exclusive_scope = my_rules` 表示：导入完成后目标规则 enabled，且其它 My Rules 全部 disabled。Group 规则和 Default 全局规则保持原样，不参与 exclusive 计算——对应单元测试 `import_exclusive_share_keeps_global_default_enabled` (`crates/bifrost-admin/src/rule_share_import.rs:524`)。

### 用户 exit 分享 env 时恢复现场

进入分享 env 时，`import_rule_share_payload` 会把当前 enabled 的 My Rules 记录到 `state.rules_storage` 的 pre-share stash（对应 `import_stashes_pre_share_enabled_rules_once_and_exit_restores` 用例 `:565`），退出时 `exit_rule_share_env` 可以一次性恢复。这一版 CLI 尚未暴露 exit 子命令，但 Admin 已经提供接口，供 Web/Agent 调用。

## 技术细节

### 协议契约

Query 参数固定 `__bifrost_rule`。payload 结构（`crates/bifrost-core/src/rule_share.rs`）：

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

- `version` 必须等于常量 `RULE_SHARE_PROTOCOL_VERSION = 1`。
- `content_hash_algorithm` 必须等于 `RULE_SHARE_CONTENT_HASH_ALGORITHM = "sha256"`。
- `content_hash` 必须等于 `sha256(content)` 的 lower-hex，不再使用 `sha256:` 前缀（旧文档已废弃）。
- `mode` / `exclusive_scope` 默认值分别是 `enable_exclusive` / `my_rules`，第一版只接受这一组。
- payload 编码方式为 UTF-8 JSON → URL-safe base64 no padding。

### 模块划分

- `crates/bifrost-core/src/rule_share.rs`：常量、类型、编解码、URL 拼接与清理，以及 `share/` 命名空间元数据 helper（`imported_rule_name` / `imported_rule_description` / `imported_rule_source_name` / `imported_rule_content_hash` / `share_payload_name_from_rule`）。
- `crates/bifrost-admin/src/rule_share_import.rs`：`import_rule_share_payload`、`exit_rule_share_env`、`apply_exclusive_enable`、`notify_after_import`、`notify_after_share_exit`。
- `crates/bifrost-admin/src/handlers/rule_share_confirm.rs`：`handle_rule_share_confirm_page`（GET 渲染确认页）与 `handle_rule_share_confirm_api`（POST 校验后调用 import）。
- `crates/bifrost-proxy/src/proxy/http/handler.rs` + `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`：`handle_rule_share_query` / `handle_intercepted_rule_share_query`，把命中 `__bifrost_rule` 的请求变成 302 重定向到本机 `http://127.0.0.1:<admin_port>/_bifrost/share/rule`。

### 代理入口伪代码

```text
raw_url = ctx.url or reconstruct https://<original_host><path_and_query>
if !raw_url.contains("__bifrost_rule"):
    return None
parts = extract_rule_share_query(raw_url)          # 失败 -> warn + None
if parts.payload is None:
    return None
if admin_state is Some:
    confirm_url = format!("http://127.0.0.1:{admin_port}/_bifrost/share/rule?payload={base64}&target={clean_url}");
    return Redirect(confirm_url)
return Redirect(parts.clean_url)                   # admin 不可用时只清除私有 query
```

### 命名选择算法（`resolve_final_name`）

1. 基础名 `share/<source_name>`；索引 1 使用基础名。
2. 若基础名不存在则直接创建。
3. 若已存在，比较 description 中的 `bifrost-rule-share-name` 与 `bifrost-rule-share-sha256`：完全一致则复用；否则 index 递增到 `share/<source_name> 2`。
4. 上限 1000 作为防御性阈值，避免异常目录导致无限循环。

## CLI + Web + Admin API

### CLI

`bifrost rule share <name> <target_url> [--content <content> | --file <path>] [--exclusive-scope my_rules]`

- `target_url` 是位置参数（不是 `--url`），可以是完整 URL 或裸域名 / `host:port`。
- `--content` / `--file` 互斥；都不传时读取 `RulesStorage::load(name)`。
- `--file` 只接受纯规则内容文件，不接受 `.bifrost` 元数据文件。
- 输入是已导入的 `share/xxx` 规则时，通过 `share_payload_name_from_rule` 自动剥前缀并从 description 恢复原始分享名。
- `--exclusive-scope` 第一版只接受 `my_rules`；未来若引入 `all` 会同时扩展 wire schema。
- stdout 只输出最终 URL，便于 Agent 捕获。

### Admin API

- `POST /api/rules/share-link`：把已有 My Rule 的内容打包成分享 URL。响应字段：`url`、`payload_version`、`query_param`、`rule_name`、`content_hash`。错误：400 空/非法/非 HTTP(S) URL、404 规则不存在、500 内部错误。
- `GET /_bifrost/share/rule?payload=...&target=...`：渲染确认页，携带 CSP `frame-ancestors 'none'` + `connect-src 'self'` 等安全头。
- `POST /_bifrost/api/rules/share-confirm`：body 含 `payload`、`target_url`，服务端重新解码并校验 content hash / target URL / CSRF 后调用 `import_rule_share_payload`。
- OpenAPI（`crates/bifrost-admin/src/openapi.rs`）：`/api/rules/share-link` 尚未补齐，视为已知 gap（planned as of 2026-07-02）。

### Web UI

- `web/src/pages/Rules/RuleList/index.tsx` 右键菜单在 My Rules 模式下追加 `Share`；Group 模式不展示。
- Share Modal：`target URL` input、`generated URL` readonly、Generate/Copy 按钮、loading/error 状态；Generate 调 `POST /rules/share-link`。
- Notification：收到 `rule_share_imported` push 后直接展示后端 `message`：
  - `metadata.created = true` → title `Rule imported`，message `Imported and enabled rule '<final-name>'. Other My Rules were disabled.`
  - `metadata.created = false` → title `Shared rule reused`，message `Enabled existing rule '<final-name>'. Other My Rules were disabled.`
- 后端 metadata 只带 `rule_name` 与 `created`，未来若要展示 `Disabled N other rules` 需扩展 metadata。

## Sync 边界

- 分享链接只在本机写入 My Rules 命名空间的 `share/...` 规则，不进 Group，不进远端同步。
- 导入完成后由 `notify_rules_changed(...)` 统一发送 `ConfigChangeEvent::RulesChanged`，触发 filesystem watcher/hot reload 链路重建 resolver。
- 若用户之后手工把 `share/xxx` 规则改名成普通规则，会失去 description 中的元数据，下次分享同一链接会新建一条 `share/<name>`。

## Phase 划分

### Phase 1：协议与 CLI

- 落地 `bifrost-core::rule_share` 常量、类型、编解码与 URL 处理。
- 实现 CLI `bifrost rule share <name> <target_url>`，让 Agent 能立刻生成链接。
- 覆盖 encode/decode/append/extract 单元测试。

### Phase 2：导入服务与代理接入

- 实现 `bifrost-admin::rule_share_import::{import_rule_share_payload, exit_rule_share_env}`。
- proxy 层接入 `handle_rule_share_query` / `handle_intercepted_rule_share_query`，包含 HTTPS TLS 解包后的 intercepted request。
- 302 重定向到本机确认页，禁止代理直接写规则。

### Phase 3：确认页与 API

- Admin `GET /_bifrost/share/rule` 渲染确认页，`POST /api/rules/share-confirm` 完成写入。
- 所有 HTML 管理端页面统一追加 `X-Frame-Options: DENY` + CSP `frame-ancestors 'none'`。
- 通知系统广播 `rule_share_imported`，同时刷新通知中心 badge。

### Phase 4：Web UI、docs 与 human_tests

- Web UI 右键 Share Modal、复制按钮、导入通知消费。
- `docs/rule.md`、`docs/cli.md`、`human_tests/rule-share-query.md`、`human_tests/readme.md` 同步更新。
- 补齐 E2E 与 human_tests 覆盖，形成 Review/Fix/Test 闭环。

## 测试方案

### 单元测试（实际已落地）

- `crates/bifrost-core/src/rule_share.rs`：`encode_decode_round_trip`、`append_and_extract_preserves_site_query_and_fragment`、`extract_removes_only_share_query_without_empty_query_suffix`、`append_replaces_existing_share_query`、`append_accepts_schemeless_domain_targets`、`append_accepts_schemeless_localhost_port_targets`、`append_rejects_explicit_non_http_scheme`、`imported_rule_description_round_trips_source_name`、`share_payload_name_strips_import_namespace_and_prefers_source_metadata`、`decode_rejects_hash_mismatch`。
- `crates/bifrost-admin/src/rule_share_import.rs`：`import_reuses_same_name_same_content`、`import_suffixes_same_name_different_content_and_disables_others`、`import_exclusive_share_keeps_global_default_enabled`、`import_stashes_pre_share_enabled_rules_once_and_exit_restores`、`exit_share_env_restores_empty_pre_share_enabled_set`、`exit_share_env_restores_multiple_pre_share_enabled_rules`、`exit_share_env_without_state_is_noop`、`import_does_not_overwrite_user_rule_with_original_name`、`import_accepts_rule_reference_lines`、`import_reopens_same_link_over_existing_share_rule`。

尚未落地（planned as of 2026-07-02）：`decode_rejects_invalid_base64` / `decode_rejects_invalid_json` / `decode_rejects_wrong_type` / `decode_rejects_unsupported_version`，以及 `crates/bifrost-cli/src/commands/rule.rs` 里针对 `rule share` 子命令的独立 Rust 单元测试。

### E2E 测试

- `e2e-tests/tests/test_rule_share_query.sh`：真实启动 Bifrost + curl 代理，覆盖裸域名经 HTTP 代理导入、HTTPS TLS 解包路径导入、带 `@规则引用` 的内容、重复访问不创建副本、同名不同内容创建递增 `share/<name> 2`、对其它 My Rules 独占启用。
- `e2e-tests/tests/test_rule_share_confirm_browser.sh`：真实 Chromium-family DevTools 打开确认页，无需填写 hash 点击 Apply Rule，跳回 clean URL；脚本优先使用 `CHROME_BIN`，否则使用 Playwright `@playwright/test` 的 Chromium executable，CI 仅安装 `chromium-headless-shell` 时从 Playwright browser cache 中查找 headless shell，再 fallback 到系统 Chrome/Edge/Chromium；测试结束时 Chrome profile 临时目录被 helper 短暂占用不能把已通过业务断言误报为 E2E 失败。

Rust `crates/bifrost-e2e/src/tests/rule_share_query.rs` 未落地（planned as of 2026-07-02），当前由上述 shell 脚本覆盖等价语义。

### human_tests

`human_tests/rule-share-query.md`：

- TC-RSQ-01：CLI 用已有规则生成分享链接。
- TC-RSQ-02：CLI 用 inline content 生成分享链接。
- TC-RSQ-03：HTTP 访问分享链接后展示确认页，Apply 后规则被创建并启用。
- TC-RSQ-04：GET/HEAD 访问分享链接后重定向到本机确认页，再回到 clean URL。
- TC-RSQ-05：导入成功后 Web UI 或 notification 展示最终规则名提示。
- TC-RSQ-06：重复刷新同一链接不创建重复规则。
- TC-RSQ-07：同名不同内容创建递增规则名 `share/<name> 2`。
- TC-RSQ-08：Web UI 右键 Share 生成并复制链接。
- TC-RSQ-09：HTTPS TLS 解包路径分享链接同样命中确认页。
- TC-RSQ-10：Exit share env 后 My Rules 恢复到进入分享前的 enabled 集合。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 payload 协议、URL 清理、导入算法、exclusive/exit 语义。
- 执行 `git status --short`、`git diff`。
- Review `rule_share.rs`、`rule_share_import.rs`、proxy 入口、CLI、`rule_share_confirm.rs`。
- 跑 core/admin/cli focused tests：`cargo test -p bifrost-core rule_share`、`cargo test -p bifrost-admin rule_share_import`。

### 第 2 轮

- 复核 Web/API/docs/human_tests 是否与实现一致。
- 再次执行 `git status --short`、`git diff`。
- 复跑 E2E：`BIFROST_BIN=./target/debug/bifrost bash e2e-tests/tests/test_rule_share_query.sh` + `test_rule_share_confirm_browser.sh`。
- 跑 `cargo test --workspace --all-features` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。

若任一轮仍发现代理绕过确认页写入规则、hash 校验失败仍成功导入、Default 被 exclusive 关掉、`share/<name> N` 覆盖用户原有规则、退出分享 env 未恢复现场，必须追加第 3 轮。

## 风险与决策

- **是否允许 Group 分享**：第一版禁止，避免把远端协作规则静默改脏。若后续开放 Group 分享，需要单独 wire schema 位并明确 Group 权限模型。
- **exclusive_scope = all**：暂只在 wire schema 保留扩展位；打开会牵扯 Group 与 Default 边界，第一版不做。
- **分享链接被恶意投放**：确认页是唯一写入门槛，必须严格校验 CSRF token、target URL、payload content hash；`connect-src 'self'` 是同源确认 API 的必要条件。
- **OpenAPI 未补 `/api/rules/share-link`**：目前调用方需直接参考本文档；补齐后应同步补 UI 类型和 CLI help 引用。
- **CLI 无 `bifrost rule share exit` 子命令**：目前只能通过 Admin API 触发 `exit_rule_share_env`；如果后续在 CLI 暴露，必须复用同一函数，避免和 Web 走出两条恢复路径。
