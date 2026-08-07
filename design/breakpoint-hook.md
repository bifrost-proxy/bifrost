# Breakpoint Hook 设计方案

## 背景

Breakpoint 用于调试代理流量：用户可以在 request 发往 upstream 前，或 response 返回 client 前暂停请求，查看并修改 headers / body 后再继续。真实调试价值集中在“单条命中的 request 或 response 停下来修改”，而不是把所有流量都截停。因此第一版必须让默认路径快、命中路径准、超时可自愈。

PR #174 引入该能力后，性能风险主要集中在三类路径：

- 默认关闭时是否仍然发生 body clone、collect、WebSocket push 或 oneshot 等待。
- 启用 breakpoint 后，大 body、未知长度 streaming body、SSE 长连接是否被强制完整缓存。
- 用户忘记 resume 时，业务请求是否长期阻塞，造成高延迟或连接堆积。

本方案在既有 `BreakpointManager` 与 admin API 之上，明确 request/response 阶段由命中的 `breakpoint://request` / `breakpoint://response` 规则授权，并把默认关闭、body 上限、auto-resume timeout、header-only pause、trailer/SSE 保护固化为运行时行为。

## 用户目标验证清单

### 必须实现

- 支持普通 HTTP 与 HTTPS MITM 场景下的 request / response breakpoint。
- 全局开关 `enabled=false` 时零开销，业务流量不产生 body clone、body collect、oneshot 等待或 breakpoint push。
- 全局开关开启但当前请求未命中 `breakpoint://request` / `breakpoint://response` 规则时，不发生 pause，也不做 body collect。
- 命中 `breakpoint://request` 时暂停 request（editable 或 header-only）；命中 `breakpoint://response` 时暂停 response；`breakpoint://request,response` 顺序触发两次。
- 已在内存中的 body，或可在 `max_body_bytes` 内完整读取的未知长度 body，在解压后大小不超限且为 UTF-8 时允许 body 编辑；否则只允许 header-only pause。
- SSE response 仅在明确 `Content-Length` 且长度不超过 `max_body_bytes` 时允许缓存并进入 response breakpoint；未知长度或超过限制时保持原始 streaming。
- Request 阶段允许修改 method、absolute URL（含 query）、有序重复 headers 和 body；response 阶段允许修改 status、headers 和 body。
- `GET /api/breakpoint/pending` 暴露权威内存快照，WebUI 在首次连接、重连或刷新后恢复暂停状态，不依赖单次 push 是否送达。
- Breakpoint Auto-Resume Timeout 在 Settings -> Performance 中配置，默认 30s，服务端固定安全范围 `5000..=300000`；超时后自动继续，不应用编辑。
- `max_body_bytes` 默认 1 MiB，最大 10 MiB；超过限制或非 UTF-8 body 只允许 header-only pause。

### 必须不破坏

- 默认关闭时 HTTP handler、HTTPS MITM handler、tunnel、mock、immediate response 的原有 fast path 不引入 body clone / body collect / oneshot。
- WebSocket body 编辑不在本能力范围内；WebSocket 帧继续按现有链路 stream/push。
- 普通 response 只在有界读取能于 `max_body_bytes` 内完整结束时进入 body editable pause；超过上限、持续 streaming 或二进制性能模式命中的 response 保持 streaming 或走 header-only pause。
- 已存在的 UI 用例 TC-BP-01..06 保留，用于后续 UI 回归。
- Breakpoint 相关配置越界（`max_body_bytes`、`breakpoint_timeout_ms`）时返回 `400` / clamp 到安全范围，不导致 admin API 崩溃。
- 协议注册表 `tests/rules_test.rs::test_all_protocols` 必须覆盖 `breakpoint`，并与 `ALL_PROTOCOLS` 当前数量保持一致，避免 workspace 全量测试因漏同步失败。

### 必须真实验证

- 默认关闭下 2 MiB request body 通过代理正常完成，不产生 pause。
- `breakpoint://request` / `breakpoint://response` / `breakpoint://request,response` 分别触发正确阶段。
- header-only pause resume 时 body 未被覆盖。
- Auto-resume timeout 生效：未 resume 时约 timeout 时长自动放行。
- UI 与 CLI 真实操作能看到 paused 事件、编辑保存、resume 结果。

## 产品语义

### `breakpoint` 是控制类规则协议

`breakpoint` 与 Body / Header 类规则语义不同，它只表达“到达这个阶段时暂停”。全局开关和规则必须同时满足才会暂停：

```text
127.0.0.1:18080 breakpoint://request
127.0.0.1:18080 breakpoint://response
127.0.0.1:18080 breakpoint://request,response
```

空 value 或未知 value 不触发暂停，避免规则误写导致意外阻塞。

### header-only pause 与 body editable pause 是两个不同产品状态

- **body editable pause**：body 已完整拿到、大小在 `max_body_bytes` 内、且是有效 UTF-8。UI 允许编辑 body。
- **header-only pause**：body 大小超过上限 / 未知 / 非 UTF-8。UI 只允许查看和编辑 headers，body 编辑器 disabled，Body Tab 展示 `body_omitted=true` 说明。resume 时即使传入 body 也会被服务端丢弃，避免客户端或恶意脚本覆盖大 body。

### Auto-Resume Timeout 保证业务不长期阻塞

无人 resume 时，服务端按 `breakpoint_timeout_ms` 自动放行原始请求 / 响应，不应用任何编辑。这个超时是 UX 的一部分，避免调试窗口关闭后连接堆积。

## 技术细节

### BreakpointManager

`crates/bifrost-admin/src/breakpoint.rs`：`BreakpointManager` 维护全局开关、body 捕获上限、运行时超时时间和 pending 请求表。request/response 阶段由命中的规则授权，不再通过 UI 或 settings 拆分两个阶段开关。

关键字段：

- `enabled: AtomicBool`
- `max_body_bytes: AtomicUsize`
- `timeout_ms: AtomicU64`
- `pending: DashMap<String, BreakpointHandle>`

常量：

- `DEFAULT_BREAKPOINT_MAX_BODY_BYTES = 1 MiB`
- `MAX_BREAKPOINT_MAX_BODY_BYTES = 10 MiB`
- `DEFAULT_BREAKPOINT_TIMEOUT_MS = 30_000`
- `MIN_BREAKPOINT_TIMEOUT_MS = 5_000`
- `MAX_BREAKPOINT_TIMEOUT_MS = 300_000`

`BreakpointHandle` 记录当前阶段 sender、完整暂停快照以及 body 是否可编辑。一个 request id 在任一时刻只允许一个阶段 pending；resume 必须提交匹配的 phase，错误阶段返回 `409`。header-only pause 的 resume body 会在服务端被忽略。

### Request Hook

`crates/bifrost-proxy/src/proxy/http/breakpoint.rs` + `handler.rs` / `tunnel/mod.rs`：

1. 默认关闭、全局开关关闭或未命中 `breakpoint://request` 规则时，直接走原有快路径，不构造 breakpoint payload，不等待 oneshot。
2. 已在内存中的 body 在 `len <= max_body_bytes` 且为 UTF-8 时允许编辑。
3. streaming body 在有界读取不超过 `max_body_bytes` 时允许编辑；超过限制后使用可重放流保持原始 streaming，并发送 header-only pause。
4. header-only pause 的 `body_omitted=true`，`body_size` 尽量使用 `Content-Length` 或已知大小。
5. resume 时仅当 pause 阶段标记 body editable 且新 body 未超过上限，才替换 body。
6. 等待 resume 使用 `timeout_ms`，超时后 cancel pending 并继续原始请求。

### Response Hook

同样位于 `crates/bifrost-proxy/src/proxy/http/breakpoint.rs` 与 handler / tunnel 之间的钩子：

1. 默认关闭、全局开关关闭或未命中 `breakpoint://response` 规则时，保持原有 response streaming/tee 快路径。
2. 普通 response 不论是否声明 `Content-Length`，都只做 `max_body_bytes` 内的有界读取；完整读完则允许编辑，超过限制则进入 header-only pause 并重放原始流。
3. gzip / deflate / br 等受支持的压缩正文以解压后的文本呈现，Apply 后按最终 `Content-Encoding` 重新编码；解压失败、解压后超限或二进制正文进入 header-only。
4. SSE 只在明确长度且不超过上限时缓存；否则跳过 body breakpoint 继续 streaming。
5. response breakpoint 同样使用 `timeout_ms` 自动放行。
6. body 被替换后才重新计算 `Content-Length`，否则保留原路径行为。

### Admin API

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/api/breakpoint/settings` | 获取当前 breakpoint 设置 |
| `POST` | `/api/breakpoint/settings` | 更新 `{enabled, max_body_bytes}`，返回有效值 |
| `GET` | `/api/breakpoint/pending` | 获取当前权威暂停快照，用于刷新/重连恢复 |
| `POST` | `/api/breakpoint/resume` | 提交 request method/URL 或 response status，以及 ordered headers/body，并继续严格匹配的 phase |
| `GET` | `/api/config/performance` | 返回 Performance 配置中的 `breakpoint.timeout_ms`、`timeout_min_ms`、`timeout_max_ms` |
| `PUT` | `/api/config/performance` | 使用 `{breakpoint_timeout_ms}` 持久化并立即应用 auto-resume timeout |

设置边界：

- `max_body_bytes`: 默认 `1048576`，最大 `10485760`，超出 clamp 或返回 `400`。
- `breakpoint_timeout_ms`: 默认 `30000`，固定安全范围为 `5000..=300000`，UI 展示后端常量，越界更新返回 `400`。

### Push Messages

| Type | Data |
| --- | --- |
| `breakpoint_paused` | `{phase, request_id, method, url, status, headers, body, body_omitted, body_size, max_body_bytes}`，`phase` 为 `"request"` 或 `"response"`；`method` / `url` 仅 request 阶段填充，`status` 仅 response 阶段填充 |
| `breakpoint_resumed` | `{request_id, phase, reason}`，`reason` 为 `resumed` 或 `timeout` |
| `breakpoint_settings_updated` | `{enabled, max_body_bytes}` |
| `settings_update(performance_config)` | 包含 `breakpoint.timeout_ms`、`timeout_min_ms`、`timeout_max_ms` |

## CLI / 自动化边界

当前没有独立的 `bifrost breakpoint ...` 子命令。脚本化使用统一走 Admin API：settings、pending、resume 与 Performance timeout 都有稳定 OpenAPI 契约，避免文档暴露不存在的 CLI。

## Web / Admin

### Web UI

- Settings -> Performance 提供 Breakpoint Auto-Resume Timeout，说明无人 resume 时自动放行的应用场景，并使用后端返回的 min/max 渲染滑块。
- `useBreakpointStore` 保存 `maxBodyBytes`、phase-specific paused snapshot、原始值/编辑值和 deadline；首次连接、重连及 resume 冲突后都会重新拉取 pending，并用 revision 避免旧 pending HTTP 响应覆盖更新的 push 状态。
- `pushService` 消费 `breakpoint_paused` payload：`body_omitted=true` 时不回退读取 TrafficDetail 里已有 body，避免把未捕获 body 误显示为可编辑。
- Network 行显示 request/response 阶段暂停标识并整行使用 `colorWarningBg`；主题切换时虚拟列表 memo 因 token 变化重绘，resume/disabled/timeout 移除 pending 后背景同步消失，选中态使用 primary inset 而不覆盖警示背景。
- TrafficDetail 顶部显示阶段、倒计时、压缩编码和明确的 `Resume unchanged` / `Apply & Resume`；headers、query、request method/URL、response status 与可编辑 body 均在原详情内编辑。
- TrafficDetail 在 header-only pause 时禁用 body 编辑，但保留 metadata 与 headers 编辑。
- 全局 Breakpoint 开启且 CONNECT 命中 Breakpoint 规则时，在标准 TLS 端口自动触发 scoped TLS interception；显式 `tlsIntercept://false` 仍优先，UI 同时提示客户端必须信任 Bifrost CA。
- `/api/breakpoint/pending` 与 `/api/breakpoint/resume` 继续经过 AdminRouter 统一 CORS、Origin guard 与 CSRF 层，兼容 localhost Web Origin 和 `tauri://localhost` Desktop WebView Origin。
- Monaco body editor 使用 lazy import，仅在可编辑 paused body 场景加载，避免默认 TrafficDetail 打开时引入重型 editor chunk。

### 后端保护

- 默认 `enabled=false`；仅打开全局开关但没有命中规则时也不会暂停任何流量。
- 默认热路径不 clone 大 body、不 collect streaming body、不创建 pending pause。
- 大 body、无法在上限内完整读取的 streaming body 或非文本 body 不进入 UI 编辑器。
- 超时自动放行，避免业务请求无限挂起。
- SSE 和二进制响应优先保持 streaming。
- Breakpoint timeout 配置越界时返回错误；min/max 是服务端固定安全边界，UI 只展示同一份后端常量。

## Sync 边界

- Breakpoint 全局设置（`enabled` / `max_body_bytes`）与 Performance timeout 属于本机调试状态，不参与规则或配置的跨设备 sync。
- Pending / paused 状态是内存态，不落盘、不同步。
- `breakpoint://request/response` 规则是普通规则的一部分，参与规则 sync；但接收方在同步这类规则前应清楚“对方设备启用了断点会影响自己”，UI 上需要有明显标记。

## 变更文件

| File | Action |
| --- | --- |
| `crates/bifrost-admin/src/breakpoint.rs` | 扩展 runtime timeout、pending editable 标记、resume body 上限保护 |
| `crates/bifrost-admin/src/handlers/breakpoint.rs` | settings 返回和 push 使用有效值 |
| `crates/bifrost-admin/src/handlers/config.rs` | Performance 配置返回 / 更新 breakpoint timeout |
| `crates/bifrost-storage/src/unified_config.rs` | 持久化 breakpoint timeout 与 bounds |
| `crates/bifrost-admin/src/push.rs` | 扩展 paused/settings/performance push payload |
| `crates/bifrost-admin/src/openapi.rs` | 更新 breakpoint 与 performance API 描述 |
| `crates/bifrost-proxy/src/proxy/http/breakpoint.rs` | body omission、timeout、header-only pause、hook outcome |
| `crates/bifrost-proxy/src/proxy/http/handler.rs` | 普通 HTTP request/response breakpoint 性能保护 |
| `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` | tunnel request/response breakpoint 性能保护 |
| `web/src/api/breakpoint.ts` | 同步 settings 和 paused payload 类型 |
| `web/src/services/pushService.ts` | 同步 push message 字段 |
| `web/src/stores/useBreakpointStore.ts` | 处理 body omitted 与 resume payload |
| `web/src/pages/Settings/tabs/PerformanceTab.tsx` | 增加 Breakpoint Auto-Resume Timeout 配置 |
| `web/src/components/TrafficDetail/index.tsx` | header-only pause 禁用 body 编辑 |
| `web/src/components/TrafficDetail/panes/Body/HighLightBody.tsx` | lazy-load Monaco editor |
| `e2e-tests/tests/test_breakpoint_performance_guard.sh` | 新增性能防护 E2E |
| `tests/rules_test.rs` | 同步全协议 registry 断言 |
| `human_tests/breakpoint-hook.md` | 新增性能防护真实场景用例 |
| `docs/breakpoint.md` | 新增用户使用手册 |
| `README.md` / `docs/README.md` | 增加使用手册入口 |

## 实现切分

### Phase 1：BreakpointManager 与 admin API

- 扩展 `BreakpointManager` 常量、runtime timeout、editable flag、pending body 上限保护。
- `handlers/breakpoint.rs`：settings 读写、resume 校验、越界返回 400。
- `handlers/config.rs`：Performance 配置返回 / 更新 `breakpoint_timeout_ms`。
- `unified_config.rs` 持久化 timeout。
- 单元测试覆盖默认值、clamp、resume 忽略被 omit 的 body。

### Phase 2：Proxy Hook 与性能保护

- `proxy/http/breakpoint.rs`：body omission、timeout、header-only pause、hook outcome。
- `handler.rs` / `tunnel/mod.rs`：默认关闭快路径、命中路径 body collect 保护、SSE / 二进制保护。
- `push.rs`：扩展 paused / settings / performance payload。
- 全 protocol registry 同步：`tests/rules_test.rs::test_all_protocols` 覆盖 `breakpoint`。

### Phase 3：Web UI

- `useBreakpointStore` / `pushService` / TrafficDetail / PerformanceTab / Monaco lazy import。
- Playwright 覆盖：paused overlay 展示 request/response、body_omitted 禁用编辑、resume 应用编辑、settings 界面。

### Phase 4：文档与 human_tests

- `docs/breakpoint.md` 用户手册；README 入口。
- `human_tests/breakpoint-hook.md` 新增 / 更新用例。
- `human_tests/readme.md` 索引更新。

## 测试方案

### 单元测试

- `breakpoint::manager_defaults`：默认关闭，body 上限和 runtime timeout 默认值正确。
- `breakpoint::settings_clamp`：超过最大 body 上限后读取到有效上限。
- `config::performance_breakpoint_timeout_persist_and_apply`：`breakpoint_timeout_ms` 持久化、越界校验、更新后立即影响 runtime timeout。
- `breakpoint::header_only_pause_ignores_resume_body`：header-only pause 时 resume body 被丢弃。
- `rules::test_all_protocols_includes_breakpoint`：`tests/rules_test.rs::test_all_protocols` 覆盖 `breakpoint`，避免 workspace 全量测试因漏同步失败。

### E2E 测试

- `test_breakpoint_default_off_large_body`：默认关闭下 2 MiB request body 通过代理正常完成，不产生 pause。
- `test_breakpoint_rule_gating`：全局开关开启但没有 `breakpoint://...` 规则时不暂停；`breakpoint://request` 触发 request 阶段；`breakpoint://response` 触发 response 阶段；`breakpoint://request,response` 按 request 后 response 顺序触发两次。
- `test_breakpoint_request_edit`：request 小 body 与 headers 编辑，内置 mock server 收到 resume 后的 edited body / header。
- `test_breakpoint_request_body_omitted`：4 KiB body 在 `max_body_bytes=1024` 时只 header-only pause，resume 伪造 body 不覆盖原始 body。
- `test_breakpoint_response_edit`：response 小 body 与 headers 编辑，curl 收到 resume 后的 edited body / header。
- `test_breakpoint_response_timeout`：通过 `/api/config/performance` 设置 5s 后，未 resume 时约 5s 自动放行并返回正确响应。
- 所有新增 E2E 均使用脚本内置 `127.0.0.1` mock server，禁止依赖外网域名。

### 真实场景测试 human_tests

- `human_tests/breakpoint-hook.md` TC-BP-07/08/09 覆盖性能防护真实场景（默认关闭大 body、命中 header-only、SSE 保护）。
- TC-BP-10/11 覆盖 request/response 小 body 与 headers 编辑。
- TC-BP-15 覆盖 `breakpoint` 协议 registry 同步回归。
- 既有 UI 用例 TC-BP-01..06 保留，用于后续 UI 回归。
- 服务启动统一使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin breakpoint`
- `cargo test -p bifrost-proxy proxy::http::breakpoint`
- `cargo test rules_test`
- `bash e2e-tests/tests/test_breakpoint_performance_guard.sh`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认关闭快路径、命中路径 body / header 编辑、header-only pause 保护、auto-resume timeout、SSE / 二进制 streaming 保持。
- 复核前端 lazy import、`body_omitted` 是否误回填 body。
- 运行 focused unit tests、性能 E2E、human_tests 执行记录核对。

### 第 2 轮

- 复查 diff 是否仍有默认路径 body clone/collect。
- Settings push 是否返回有效值；越界更新是否返回 400。
- Protocol registry 是否同步 `breakpoint`。
- 复跑 focused tests、fmt、clippy / 工作区测试。

## 风险与决策点

- Body editable 边界：非 UTF-8 body 一律 header-only pause，不做二进制编辑器，避免 UI 与后端解析口径不一致。
- Timeout 范围：`5s..=300s` 覆盖常见调试节奏，过小容易“还没看清就放行”，过大风险与不开断点一致。若后续有场景需要更长，通过后端常量升级，UI 自动同步。
- SSE / streaming：优先保持 streaming，避免为了断点把长连接强制缓存导致内存爆炸；仅在明确长度且小于上限时才允许 body editable。
- 规则误配：`breakpoint://<空>` 或未知 value 不触发暂停，避免规则误写导致业务全线阻塞。
- Sync 侧：`breakpoint://...` 规则参与规则同步，若一台设备开启断点会影响自己（接收方设备），UI 需要在启用同步时提示“接收到的规则包含断点”。
