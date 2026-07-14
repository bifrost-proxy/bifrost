# HTML 页面注入 Bifrost 小圆点（Badge Injection）

## 背景

当用户通过 Bifrost 代理访问网页（尤其是 HTTPS MITM 后的页面）时，需要一个**低侵入**的视觉提示告诉用户「当前页面确实被 Bifrost 接管，命中了哪些规则」。同时该提示不能破坏任何客户端解析：

- `Content-Type: text/html` 但 body 是 JSON 的接口在真实站点大量存在（验证码、鉴权、心跳等），这些响应不得被注入 HTML 片段，否则会打断客户端解析。
- 用户规则可能包含含 `</script>`、`<iframe srcdoc>`、`<!--`、事件属性、`</textarea>` 等敏感 token 的 `htmlAppend://` 值，Badge 内联脚本必须彻底转义这些内容，否则规则文本会「逃逸」为真实页面脚本，导致 XSS。
- Group 规则通过 API 启停 / 创建 / 更新 / 删除后，Badge 面板显示的 active 规则必须立即刷新，用户不应通过「保存任意规则」这种旁路操作才能触发缓存补齐。
- 已经用于 Badge/规则命中链路的规则数据来自 `AdminState.badge_rules_cache`；缓存与 runtime resolver 之间必须共用一条通知路径，避免二者语义漂移。

主要实现位于 `crates/bifrost-proxy/src/transform/badge.rs`，通过 `crates/bifrost-proxy/src/proxy/http/handler.rs` 与 `tunnel/mod.rs`、`socks/tcp.rs` 三条转发路径接入；Admin 侧 `crates/bifrost-admin/src/handlers/{rules,group_rules,config}.rs` 负责 cache 刷新与开关；Web 侧 `web/src/pages/Settings/index.tsx` 与 `web/src/api/config.ts` 提供开关 UI。

## 用户目标验证清单

### 必须实现

- HTML 响应默认在左下角固定注入一个 Bifrost 小圆点（Badge），最高 `z-index` 常驻。
- Bifrost 自身生成的上游连接失败页面（例如直连目标端口失败返回的 502）在 Badge 开启时也必须注入同一小圆点与操作面板，不能因为错误响应提前返回而丢失入口。
- Badge hover 弹窗展示当前请求命中的 Merged Rules，右上角提供一键复制按钮。
- 全局开关 `traffic.inject_bifrost_badge` 可通过 CLI (`--enable-badge-injection` / `--disable-badge-injection`)、`UnifiedConfig` 持久化，以及 Admin API `PUT /api/config/performance` 修改，默认 `true`。
- Web UI Settings -> Proxy 提供开关，读写走 `GET/PUT /api/config/performance`。
- Group 规则通过 Admin API 启停 / 新增 / 更新 / 删除或从远端 sync 拉取后，`AdminState.badge_rules_cache` 立即被刷新，同时触发 runtime rules hot reload；不需要用户「随便保存一条规则」。
- 规则数据在注入前必须做统一 HTML/JS 转义：`<` -> `<`、`>` -> `>`、`&` -> `&`、U+2028/U+2029 -> ` `/` `；异常 JSON 回退为空对象，绝不把原始字符串拼进 `<script>`。
- Group 规则字段组合保持既有导航契约：Badge 点击可跳转到 `/_bifrost/rules?group=...` 的对应 Group。

### 必须不破坏

- 非 HTML、SSE、streaming、被识别为 JSON 的响应绝不注入。
- `--disable-badge-injection` 下的连接错误响应继续保持 `text/plain; charset=utf-8`；规则显式提供 `resBody` 时尊重用户响应体，不强行包装或注入 Badge。
- 连接错误字段必须先做 HTML 文本转义再放入 `<pre>`，host、URL 或上游错误文本中的标签不能逃逸为页面 DOM 或脚本。
- 已注入过 Badge 的响应不重复注入（同一 body 出现多次会被跳过）。
- 原响应带 `Content-Encoding: gzip/br/deflate/zstd` 时必须解压 -> 注入 -> 按原 encoding 重压，并通过 `normalize_res_headers` 修正 `Content-Length` / `Transfer-Encoding`。
- 存在高 `z-index` 浮层的业务页面上 Badge 仍能可见（使用 `z-index: 2147483647 !important`）。
- Merged Rules 复制按钮既支持 `navigator.clipboard.writeText`，也保留 `document.execCommand('copy')` fallback；fallback 必须真实触发 copy 事件才显示 `Copied`，否则显示 `Failed`。

### 必须真实验证

- 通过临时 `BIFROST_DATA_DIR` 启动的 CLI 实例，浏览器访问真实 HTML 页面时看到 Badge，hover 展开 Merged Rules，复制到系统剪贴板后内容匹配。
- 通过代理请求一个真实未监听的远端端口，确认 502 响应为 HTML、保留 Status/Error/Host/URL 诊断文本，并包含可展开的 Bifrost Badge；关闭开关后同一路径恢复纯文本且不含 Badge。
- `Content-Type: text/html` 但 body 为 JSON 时客户端仍收到原始 JSON，无任何 Badge 片段。
- 含 `</script>` / `<iframe srcdoc>` / `<!--` 等敏感 token 的 `htmlAppend://` 规则文本仅出现在转义后的 Merged Rules 中，不会以原始标签形态出现在页面中。
- Group 规则 API 启停后立即请求代理页面，Badge 面板 active 数量与 Rules 一致。

## 产品语义

Badge 由三部分组成：

1. 固定定位的圆点元素 `id=__bifrost_badge__`，默认可见，点击可隐藏。
2. Hover 弹窗：显示 Bifrost 分组、Merged Rules、Share Env、Exit 等入口。
3. 内联脚本 + 内联 JSON（含当前请求命中规则），支撑弹窗渲染与跳转。

`transform::badge::maybe_inject_bifrost_badge_html(body, rules_json) -> (Bytes, bool)` 是唯一的注入入口；返回布尔 `injected` 供调用方决定是否需要重压 body 并修正 headers。

### 何时注入

- `config.inject_bifrost_badge == true`
- `content-type` 包含 `text/html`
- body 明文（忽略 BOM / 前导空白）以 `<!doctype`、`<html`、任意 HTML-like 标签开头，或存在 `<body` / `</body>` 标记
- body 明文不以 `{` / `[` 开头（避免误标 JSON）
- 非 SSE、非 streaming
- body 未包含 `id="__bifrost_badge__"`（避免重复注入）

普通上游响应在响应 body 收集完成后执行上述判定。连接建立失败没有上游响应，会在转发函数中提前返回；该路径必须先把 Bifrost 生成的纯文本诊断安全包装为 `<!doctype html><html><body><pre>...</pre></body></html>`，再调用同一个 `maybe_inject_bifrost_badge_html`。Badge 关闭或规则显式覆盖 `resBody` 时不做包装。

### 转义与安全

浏览器 HTML 解析器会在脚本文本中识别标签结束序列 `</script>`、`<!--`、`</style>`、`<!--<script>` 等，即使这些序列位于合法 JS 字符串内部。因此规则 JSON 必须先 `serde_json::from_str` 解析再重新序列化，然后统一转义敏感字符：

- `<` -> `<`
- `>` -> `>`
- `&` -> `&`
- U+2028 / U+2029 -> ` ` / ` `

若 JSON 解析失败，回退为空规则对象 `{}`，避免任何原始文本进入 `<script>` 上下文。

## 技术细节

### 存储层

- `crates/bifrost-storage/src/unified_config.rs`：`TrafficConfig` 新增 `inject_bifrost_badge: bool`（默认 `true`）；`TrafficConfigUpdate` 增补对应可选字段。
- `crates/bifrost-storage/src/config_manager.rs`：`ConfigManager::update_traffic_config` 支持部分更新，落盘 `config.toml`。

### Proxy 层

- `crates/bifrost-proxy/src/transform/badge.rs`：核心注入逻辑与常量 `BIFROST_BADGE_ELEMENT_ID = "__bifrost_badge__"`。
- `crates/bifrost-proxy/src/transform/mod.rs`：暴露 `maybe_inject_bifrost_badge_html`；与 `compress` 模块协作完成解压 / 重压。
- `crates/bifrost-proxy/src/proxy/http/handler.rs`、`http/tunnel/mod.rs`、`socks/tcp.rs`：普通 HTTP、HTTPS MITM、SOCKS 三条路径统一在 body 已 collect 成 bytes 后调用注入。
- `crates/bifrost-proxy/src/proxy/http/handler.rs`、`http/tunnel/mod.rs`：HTTP 直连与 HTTPS MITM 的连接错误提前返回路径按开关生成纯文本或带 Badge 的 HTML；错误页规则摘要按实际 listener port 构建，临时端口仍展示自己的 active rules。
- `crates/bifrost-proxy/src/server.rs`：`ProxyConfig` 携带 `inject_bifrost_badge` 到 runtime，热更新配置后新连接立即生效。

### Admin / Rules 缓存

- `crates/bifrost-admin/src/state.rs`：`AdminState.badge_rules_cache` 保存最新 active rules JSON。
- `crates/bifrost-admin/src/handlers/rules.rs`：个人规则任何写路径（create/update/delete/enable/disable/reorder/import）后统一刷新 `badge_rules_cache` 并广播 runtime hot reload。
- `crates/bifrost-admin/src/handlers/group_rules.rs`：Group 规则的启停 / 创建 / 更新 / 删除 / list-sync 路径共用与个人规则相同的通知函数，确保远端同步覆盖本地 enabled Group 时也刷新 cache 与 runtime；不再存在「Rules 页面显示新版而 Badge 面板仍是旧版」的漂移。
- `crates/bifrost-admin/src/handlers/config.rs`：`GET /api/config/performance` 与 `PUT /api/config/performance` 读写 `traffic.inject_bifrost_badge` 字段；`crates/bifrost-admin/src/openapi.rs` 同步 schema。

### CLI

- `crates/bifrost-cli/src/commands/start.rs`：`--enable-badge-injection` / `--disable-badge-injection` 覆盖配置并持久化，通过 `--no-system-proxy` 与临时 `BIFROST_DATA_DIR` 组合可反复回归。

### Web UI

- `web/src/pages/Settings/index.tsx`：`SystemProxySection` 新增 Switch，默认取自 `GET /api/config/performance`；切换后 `PUT` 持久化。
- `web/src/api/config.ts`：类型定义扩展 `inject_bifrost_badge`。

### Admin API

```
GET  /_bifrost/api/config/performance
PUT  /_bifrost/api/config/performance   { "traffic": { "inject_bifrost_badge": bool } }
GET  /_bifrost/api/rules                # 触发个人规则通知路径
POST /_bifrost/api/group-rules/:name/enable
POST /_bifrost/api/group-rules/:name/disable
POST /_bifrost/api/group-rules/sync     # 从远端 sync 刷新本地 Group 规则
```

上述任何写入路径都会：

1. 更新 `RulesStorage` / `GroupRulesStorage`。
2. 通过统一通知回调刷新 `AdminState.badge_rules_cache`。
3. 广播到 `DynamicRulesResolver`，让所有主端口 / 临时端口 resolver 重建。

## Sync 边界

- 个人规则和 Group 规则的**任何**变更路径都必须走同一通知回调。禁止在 handler 里就地 `RulesStorage.save` 后忘记刷新 Badge cache。
- Group 规则的远端 list-sync（拉取远端最新目录并覆盖本地 enabled 规则）必须触发同一回调；如果同步导致本地 enabled 规则被删除，也走 disable 路径。
- Group 目录解析基于本地目录：远端 group cache 不能成为运行时 hot reload 的前置条件，避免远端不可达时 Badge 静默停摆。

## Phase 1-4

### Phase 1：注入基础能力

- 完成 `transform/badge.rs` 的注入、解压、转义、跳过逻辑与 20+ 单元测试。
- `traffic.inject_bifrost_badge` 配置字段落地 storage / proxy runtime。
- CLI flag、Admin API、Web UI 开关端到端打通。

### Phase 2：Merged Rules 面板

- Badge hover 弹窗渲染 Bifrost 分组 / Merged Rules / Share Env / Exit。
- 复制按钮支持 `navigator.clipboard.writeText` + fallback；fallback 只在真实写入后显示 `Copied`。
- `z-index: 2147483647 !important` 应用到 Badge 和面板。

### Phase 3：Group 缓存一致性

- `AdminState.badge_rules_cache` 抽出为共享通知路径。
- 个人规则 / Group 规则 / list-sync / import 全部走同一路径。
- 断言 Rules 页面 active 数量与 Badge 面板 active 数量始终一致。

### Phase 4：安全与文档

- 覆盖 `htmlAppend://` 值中 `</script>`、`<iframe srcdoc>`、`<!--`、`</textarea>`、事件属性等所有敏感 token 的转义。
- README、`docs/getting-started`、site 安装页说明开关；`human_tests/badge-hover-panel.md` 补齐 TC-BHP-10 ~ TC-BHP-14。

## 测试方案

### 单元测试（`crates/bifrost-proxy/src/transform/badge.rs`）

真实存在的测试（可用 `cargo test -p bifrost-proxy badge::` 全部运行）：

- `test_inject_badge_before_body_end`
- `test_inject_badge_append_when_no_body_end`
- `test_inject_badge_with_doctype`
- `test_inject_badge_for_html_like_fragment`
- `test_skip_badge_for_mislabeled_json_response`
- `test_skip_badge_for_mislabeled_json_array_response`
- `test_badge_contains_b_character_and_click_hide`
- `test_inject_badge_case_insensitive_body_end`
- `test_badge_snippet_contains_inline_rules_data`
- `test_badge_inline_rules_data_escapes_script_close_tag`
- `test_badge_inline_rules_data_escapes_html_tag_syntax_generally`
- `test_badge_inline_rules_data_falls_back_for_invalid_json`
- `test_badge_panel_html_present`
- `test_badge_panel_uses_top_z_index`
- `test_badge_merged_rules_copy_button_present`
- `test_badge_share_env_badge_and_exit_button_present`
- `test_skip_duplicate_injection`
- `test_badge_rule_row_links_to_admin_ui`
- `build_connection_error_response_with_badge_*`：覆盖默认纯文本、Badge HTML、诊断字段 HTML 转义、显式 `resBody` 不注入。

编解码链路复用 `transform::compress::test_brotli_roundtrip` / `test_zstd_roundtrip`；Admin 侧新增 `badge_rules_cache_preserves_group_navigation_mapping`。

### E2E 测试

- `e2e-tests/tests/test_badge_injection_e2e.sh`
  - 启动本地 HTTP server 返回 `Content-Type: text/html`。
  - 断言注入片段包含 `__bifrost_badge__`、`__bb_copy`、`Copy merged rules`、`navigator.clipboard`、`z-index:2147483647!important`。
  - 配置 `htmlAppend://{vconsole-inject}` 值含 `</script><script>new VConsole()</script>`，断言响应中只有 `</script>`。
  - 上游返回 `Content-Type: text/html; charset=utf-8` 但 body 为 `{"code":200,...}`，断言响应不包含任何 Badge 片段。
- 请求本地未监听端口触发真实 502，断言开启时 `Content-Type: text/html`、诊断文本和 Badge/操作脚本同时存在；关闭时仍为 `text/plain` 且无 Badge。
- HTML 错误页采用水平/垂直居中的响应式状态卡片，展示 502 状态、错误摘要、Host、时间、请求 URL、分步排查引导、Rules 跳转、重试入口和折叠原始诊断；覆盖深浅色与窄屏样式。
- 错误页按原始请求 `Accept` 做保守内容协商：只有明确接受 `text/html`、`application/xhtml+xml` 或 `text/*` 且质量值非零时生成 HTML + Badge；缺失 `Accept`、只有 `*/*`、仅接受 JSON/纯文本或显式 `text/html;q=0` 时保持原纯文本诊断。请求规则对 `Accept` 的改写不改变该判定。
- `crates/bifrost-e2e/src/tests/group_rules.rs`
  - `group_rules_enable_refreshes_badge_cache`
  - `group_rules_rapid_toggle_keeps_active_summary_and_badge_consistent`
  - `group_rules_remote_sync_refreshes_badge_cache_for_enabled_rules`
  - `resolve_valid_group_dirs_uses_local_dirs_without_sync_session`

### 真实场景测试（`human_tests/badge-hover-panel.md`）

- TC-BHP-01 ~ 09：Badge 基本可见 / hover 弹窗 / 复制按钮 / 高 z-index 覆盖测试。
- TC-BHP-10：`htmlAppend://{vconsole-inject}` 规则不匹配当前页面时，规则文本仅出现在 Merged Rules，不逃逸为页面脚本或原始 HTML token。
- TC-BHP-11：`Content-Type: text/html` 但 body 为 JSON 的响应下，Badge 不注入，客户端收到原始 JSON。
- TC-BHP-12：通过 Group Rule API 启用规则后，不重启服务直接请求代理 HTML，Badge active 数量与规则列表立即更新，链接可正确定位。
- TC-BHP-13：连续快速启用 / 停用同一 Group 规则，active summary 与 Badge cache 在限定时间内收敛一致。
- TC-BHP-14：本地已 enabled Group 规则旧内容 + 远端同步同名新内容时，只触发 Group list-sync（不手动保存），Badge active 数量与 Merged Rules 立即更新。
- TC-BHP-15：真实请求未监听远端端口，验证居中的美化 502 状态页、诊断信息、排查引导、Rules/重试操作，以及左下角 Badge、hover 操作面板和关闭开关后的纯文本回退。

启动命令示例：

```bash
BIFROST_DATA_DIR=./.bifrost-test \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
BIFROST_DISABLE_TRAY=1 \
cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy --enable-badge-injection
```

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：注入 / 转义 / 开关 / Group 缓存一致性 / 复制交互全部覆盖。
- 复核 diff：`git status --short` 覆盖 `bifrost-proxy/transform/badge.rs`、`bifrost-storage/unified_config.rs`、`bifrost-admin/handlers/{rules,group_rules,config}.rs`、`bifrost-admin/state.rs`、`web/src/pages/Settings/index.tsx`、`web/src/api/config.ts`。
- 重点检查：Group 规则任一路径是否遗漏 cache 刷新；`Content-Type: text/html` 但 body 为 JSON / SSE / streaming 的响应是否绝不注入；`</script>` 转义是否覆盖所有敏感 token。
- 复测：`cargo test -p bifrost-proxy badge::`、`cargo test -p bifrost-admin badge_rules_cache`、`bash e2e-tests/tests/test_badge_injection_e2e.sh`、`cargo test -p bifrost-e2e group_rules::`。

### 第 2 轮

- 再次核对 diff、human_tests 索引、README / site 文档更新。
- 重点：`z-index` 与复制 fallback 保护、Group list-sync 通知路径、runtime hot reload 是否会漏掉已启用 Group 规则。
- 失败路径重跑：连续启停 Group、模拟远端不可达、`htmlAppend://` 含极端 token。

## 风险与决策

- Merged Rules 复制按钮 fallback 依赖 `document.execCommand('copy')`，Chromium 已 deprecate；必须显式检测事件回调，避免出现「按钮变绿但剪贴板为空」的假阳性。
- Group 规则的远端 sync 与本地 enable 使用同一通知回调；若产品未来允许「远端规则不同步到本地目录」的模式，需要显式扩展通知语义。
- Badge 在存在 CSP `script-src` 严格策略的站点上仍可能被浏览器拦截；这是设计权衡，不做绕过。
- `htmlAppend://` 值中的 `<script>` 属于用户已授权自注入，只在 Badge 面板展示时才需要转义；真实站点被规则命中时 append 语义不变。
- SSE 与 streaming 响应不注入是安全默认；若后续需要为 streaming HTML 提供弱化版 Badge，需要新的设计分支。
