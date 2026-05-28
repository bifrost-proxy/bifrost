# HTML 页面注入 Bifrost 小圆点（Badge Injection）

## 背景与目标

当用户通过 Bifrost 代理访问网页时（尤其是 HTTPS MITM 后的页面），用户需要一个**低侵入**的视觉提示，确认“该页面已被 Bifrost 接管/代理”。

本方案在**明文可编辑且内容看起来确实是 HTML**的响应中，向页面左下角注入一个固定定位的小圆点（Bifrost badge）。不能只信任响应头；真实线上存在 `Content-Type: text/html` 但 body 是 JSON 数据接口的响应（例如验证码接口），此类响应不得注入 Badge，避免破坏客户端 JSON 解析。

## 需求范围

- 仅对 **HTTP 响应 body 可缓冲** 的场景生效（非 streaming）。
- 仅对 `Content-Type: text/html`（包含参数如 `charset=utf-8`）且 body 嗅探为 HTML-like 内容的响应生效；站点不完全遵守 HTML 文档结构但以标签内容返回时，仍允许注入。
- 支持开关配置：
  - 全局配置项：`traffic.inject_bifrost_badge`，默认 `true`，持久化到 `config.toml`。
  - Web UI：Settings -> Proxy 页提供开关，文案：**“注入 Bifrost 小圆点”**。
  - CLI：启动命令提供 flag（例如 `--disable-badge-injection`）用于覆盖并持久化该配置。
- Badge hover 弹窗的 `Merged Rules` 展开区域右上角提供复制按钮，一键复制当前合并规则文本。
- Badge 与 hover 弹窗使用浏览器允许的最高 `z-index`，并加 `!important`，降低被业务页面高层级浮层遮挡的概率。
- Group 规则通过 API 启用、禁用、创建、更新或删除后，后续代理页面中注入的 Badge 必须立即看到最新 active rules，不需要重启服务或再修改个人规则触发缓存刷新。

## 实现设计

### 1. 配置存储

- 配置位置：`UnifiedConfig.traffic.inject_bifrost_badge: bool`（默认 `true`）。
- 持久化链路：
  - `bifrost-storage`：扩展 `TrafficConfig` / `TrafficConfigUpdate` / `ConfigManager::update_traffic_config`。
  - `bifrost-proxy`：`ProxyConfig` 增加 `inject_bifrost_badge` 字段，运行时读取并在响应处理链路中生效。

### 2. 响应处理链路（Rust proxy）

注入发生在 HTTP handler 的响应 body 已经被 collect 成 bytes 后。

- 判定条件：
  - `config.inject_bifrost_badge == true`
  - `content-type` contains `text/html`
  - body 明文内容通过宽松 HTML 嗅探：忽略 BOM/前导空白后以 `<!doctype`、`<html` 或任意 HTML-like 标签开头，或内容中存在 `<body` / `</body>` 标记
  - body 明文内容像 JSON（忽略 BOM/前导空白后以 `{` 或 `[` 开头）时必须跳过，即使响应头声称是 `text/html`
  - 非 SSE / 非 streaming
- 注入策略：
  - 将 body 解压到明文（支持 `gzip` / `br` / `deflate` / `zstd`），在 `</body>` 前插入 badge 片段；如果找不到 `</body>`，则追加到末尾。
  - 若原响应存在 `Content-Encoding`，在注入完成后按原 encoding 重新压缩（保持 header 语义不变），并通过 `normalize_res_headers` 修正 `Content-Length` / `Transfer-Encoding`。

### 3. Badge 片段

- 以一个 `div` + 内联样式注入，不依赖外部资源。
- 样式目标：左下角、固定定位、小圆点，`z-index: 2147483647 !important`。
- Hover 弹窗同样固定定位，显示在 Badge 上方；面板、标题与 Merged Rules 内容均由内联脚本渲染。
- Merged Rules 展开后，代码框右上角显示 `Copy` 按钮。点击后优先使用安全上下文下的 `navigator.clipboard.writeText`，失败或不可用时回退到隐藏 `textarea` + `copy` 事件 `clipboardData.setData('text/plain', text)` + `document.execCommand('copy')`；只有 fallback 真实触发 copy 事件并写入数据后才显示 `Copied`，否则显示 `Failed`，避免假阳性成功提示。
- 规则数据以内联 JSON 形式进入 `<script>` 前，必须先解析为 JSON，再重新序列化，并统一转义 HTML/JS 敏感字符：`<` -> `\u003C`、`>` -> `\u003E`、`&` -> `\u0026`、U+2028/U+2029 -> `\u2028`/`\u2029`。原因是浏览器 HTML 解析器会在脚本文本中识别标签和脚本结束序列，即使它们位于合法 JS 字符串内部；例如 `htmlAppend://{vconsole-inject}` 的 value 中包含 `</script><script>new VConsole()</script>` 时，未转义会提前闭合 Badge 脚本并把规则文本提升为真实页面脚本。统一转义后，`<script>`、`</script>`、`<!--`、`<iframe srcdoc>`、`</textarea>` 等标签片段都不会以原始 HTML token 形态进入注入脚本。
- Badge Group 数据保留既有导航契约：内联脚本使用当前字段组合来同时满足面板分组展示和 `/_bifrost/rules?group=...` 跳转定位，不按字段名做单纯重命名式改造。后续如要改字段语义，必须同步迁移注入脚本和跳转逻辑。
- Group 规则变更时，`handlers/group_rules.rs` 的规则变更通知路径必须刷新 `AdminState.badge_rules_cache`，与个人规则变更路径保持一致。
- Group 规则启用、禁用或更新已启用规则后，通知路径必须同时触发代理规则 hot reload；hot reload 的 Group 目录解析以本地规则目录为准，远端 group cache 不得成为 Badge 或代理运行时生效的前置条件。
- Group 规则列表接口从远端 env 同步到本地时，如果同步导致已启用规则的内容变化，或删除了本地已启用规则，必须触发同一条规则变更通知链路，刷新 `AdminState.badge_rules_cache` 并 hot reload runtime rules。否则 Rules 页面会先显示同步后的 enabled 状态，而注入网页里的 Badge 仍使用旧 cache，表现为 Badge active 数量少一到两个，直到用户手动保存任意规则才恢复。

### 4. Web UI / Admin API

- Admin API：复用 `/_bifrost/api/config`，扩展 response 增加 `inject_bifrost_badge` 字段，并新增 `PUT /_bifrost/api/config` 支持更新该字段（内部调用 `update_traffic_config` 持久化）。
- Web：Settings -> Proxy tab 新增 `Switch`，默认值来自 `GET /config`，切换后调用 `PUT /config`。

## 验证计划（强制三层）

### 单元测试

- `bifrost-proxy`：
  - `test_inject_badge_before_body_end`：HTML 含 `</body>` 时插入位置正确。
  - `test_inject_badge_append_when_no_body_end`：无 `</body>` 时回退到末尾追加。
  - `test_inject_badge_gzip_roundtrip`：gzip body 解压->注入->再压缩后，解压结果包含 badge。
  - `test_badge_panel_uses_top_z_index`：Badge 与 hover 弹窗都使用最高 z-index。
  - `test_badge_merged_rules_copy_button_present`：注入片段包含 Merged Rules 复制按钮、剪贴板 API 与 fallback。
  - `test_badge_merged_rules_copy_button_present` 同时断言 fallback 使用 `clipboardData.setData` 且必须有 copy 事件写入标记才成功。
  - `test_badge_inline_rules_data_escapes_script_close_tag`：验证包含 vConsole 风格 `</script><script>new VConsole()</script>` 的合并规则文本在 Badge 内联数据中转义为 `\u003C/script\u003E`，整段 Badge 只保留自身的一个结束脚本标签。
  - `test_badge_inline_rules_data_escapes_html_tag_syntax_generally`：验证 `<img onerror>`、`<!--`、`<svg onload>`、`</textarea>`、`<iframe srcdoc>` 等标签注入形态都不会以原始 HTML 片段出现在内联脚本里。
  - `test_badge_inline_rules_data_falls_back_for_invalid_json`：验证异常数据不会原样拼入脚本，而是回退为空规则数据。
  - `test_skip_badge_for_mislabeled_json_response`：验证 `Content-Type: text/html` 场景下传入 JSON object body 时不会注入 Badge。
  - `test_skip_badge_for_mislabeled_json_array_response`：验证 JSON array body 不会注入 Badge。
  - `test_inject_badge_for_html_like_fragment`：验证 `<main>...</main>` 这类非完整 HTML 文档但明显是标签内容的响应仍会注入 Badge。
- `bifrost-admin`：
  - `badge_rules_cache_preserves_group_navigation_mapping`：验证 Badge Group 字段组合仍保持现有跳转契约，避免修复缓存刷新时破坏 Group 规则定位。

### E2E 测试

- 新增 e2e 用例：`badge_injection_html_response`
  - 启动本地 http server 返回 `Content-Type: text/html` 的页面。
  - 通过 Bifrost 代理请求该页面。
  - 断言响应 body 包含注入标识（例如 `__bifrost_badge__`）。
  - 断言响应 body 包含 `__bb_copy`、`Copy merged rules`、`navigator.clipboard` 与 `z-index:2147483647!important`。
- 回归用例：在 `test_badge_injection_e2e.sh` 中创建未匹配当前页面的启用规则：
  - `not-current-test.local htmlAppend://{vconsole-inject}`
  - value 内容包含 `<script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script><script>new VConsole();</script>`
  - 通过代理请求普通 HTML 页面，断言 Merged Rules 仍显示该规则文本，但响应中只存在 `\u003C/script\u003E` 这类转义形式，不存在未转义的 `<script src=...>` 或 `</script>\n<script>new VConsole();</script>` 序列。
- 误标 JSON 回归用例：本地上游返回 `Content-Type: text/html; charset=utf-8` 但 body 为 `{"code":200,"data":...}`。
  - 通过启用 Badge 的 Bifrost 代理请求该接口。
  - 断言响应 body 保持 JSON 内容，不包含 `__bifrost_badge__`、`__bb_copy` 或任何 Badge 片段。
- `crates/bifrost-e2e/src/tests/group_rules.rs`
  - `group_rules_enable_refreshes_badge_cache`：通过 Group Rule enable API 开启规则后，直接检查 `badge_rules_json()` 已包含该规则，并保留既有 Group 跳转字段组合。
  - `group_rules_rapid_toggle_keeps_active_summary_and_badge_consistent`：连续多次启用/停用同一 Group 规则，每次都轮询 active summary 与 Badge cache 到一致状态，覆盖本地刷新延迟和系统短暂卡顿下的最终一致性。
  - `group_rules_remote_sync_refreshes_badge_cache_for_enabled_rules`：本地已有 enabled Group 规则且 Badge cache 仍是旧内容时，通过 Group list-sync 从远端同步同名新规则内容，断言 active summary 与 Badge cache 都刷新到新内容，不需要用户再保存任意规则触发补刷新。
  - `resolve_valid_group_dirs_uses_local_dirs_without_sync_session`：没有 active sync session 时，代理热重载仍加载本地 Group 规则目录。

### 真实场景测试

- 按临时数据目录启动：
  - `BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy --enable-badge-injection`
- 通过代理访问测试 HTML，打开页面后 hover Badge，展开 Merged Rules，确认代码框右上角有复制按钮。
- 点击复制按钮后，将系统剪贴板内容粘贴到可编辑区域、或读取系统剪贴板/浏览器剪贴板，确认等于当前合并规则文本；不能只看按钮是否显示 `Copied`。
- 在页面中放置高 z-index 覆盖层，确认 Badge 弹窗仍可显示在其上方。
- 新增 `human_tests/badge-hover-panel.md` 的 `TC-BHP-10`：创建包含 vConsole 脚本片段、HTML 注释、事件属性、`srcdoc`、闭合标签等片段的 `htmlAppend://{vconsole-inject}` 规则，但请求不匹配该规则的普通 HTML 页面，确认规则文本只出现在 Badge 的 Merged Rules 展示中，不会逃逸为页面真实脚本或原始 HTML token。
- 新增 `human_tests/badge-hover-panel.md` 的 `TC-BHP-11`：通过本地上游返回 `Content-Type: text/html` 但 body 为 JSON 的响应，确认启用 Badge 时客户端收到的仍是原始 JSON，不包含 Badge 注入片段。
- 新增 `human_tests/badge-hover-panel.md` 的 `TC-BHP-12`：通过 Group Rule API 启用规则后，不重启服务直接请求代理 HTML，确认 Badge hover 面板 active 数量和规则列表立即更新，并确认 Group 规则链接仍能定位到对应 Rules 页面。
- 新增 `human_tests/badge-hover-panel.md` 的 `TC-BHP-13`：连续快速启用/停用同一 Group 规则，允许短暂刷新延迟但要求 active summary 与 Badge cache 在限定时间内收敛到一致状态，防止系统卡顿或本地写入延迟导致长期显示旧规则。
- 新增 `human_tests/badge-hover-panel.md` 的 `TC-BHP-14`：本地已有 enabled Group 规则旧内容、远端同步同名新内容时，只打开/刷新 Rules 页面触发 Group list-sync，不手动保存规则，确认后续代理 HTML 的 Badge active 数量与 Merged Rules 内容立即更新。

## 校验要求

- 必须执行：`cargo test --workspace --all-features`
- 提交前必须通过：`cargo fmt`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`

## 文档更新

- `README.md`：补充配置项与 CLI 参数说明。
