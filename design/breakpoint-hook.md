# Breakpoint: Rule-Gated Request/Response Debug

## 背景
Breakpoint 用于调试代理流量：用户可以在 request 发往 upstream 前，或 response 返回 client 前暂停请求，查看并修改 headers/body 后再继续。

PR #174 引入该能力后，性能风险主要集中在三类路径：
- 默认关闭时是否仍然发生 body clone、collect、WebSocket push 或 oneshot 等待。
- 启用 breakpoint 后，大 body、未知长度 streaming body、SSE 长连接是否被强制完整缓存。
- 用户忘记 resume 时，业务请求是否长期阻塞，造成高延迟或连接堆积。

## 范围
- 支持普通 HTTP request/response breakpoint。
- 不支持 WebSocket body 编辑。
- SSE response 仅在明确 `Content-Length` 且长度不超过 `max_body_bytes` 时允许缓存并进入 response breakpoint；未知长度或超过限制时保持原始 streaming。
- 不修改 response status code，只允许修改 headers 和 body。
- Breakpoint Auto-Resume Timeout 在 Settings -> Performance 中配置，默认 30s，超时后自动继续，不应用编辑。
- `max_body_bytes` 默认 1 MiB，超过限制或非 UTF-8 body 只允许 header-only pause。

## 后端设计

### BreakpointManager
`BreakpointManager` 维护全局开关、body 捕获上限、运行时超时时间和 pending 请求表。request/response 阶段由命中的 `breakpoint://request` / `breakpoint://response` 规则授权，不再通过 UI 或 settings 拆分两个阶段开关。

关键字段：
- `enabled: AtomicBool`
- `max_body_bytes: AtomicUsize`
- `timeout_ms: AtomicU64`
- `pending: DashMap<String, BreakpointHandle>`

`BreakpointHandle` 同时记录 request/response sender 以及 body 是否可编辑。header-only pause 的 resume body 会在服务端被忽略，避免 UI 或恶意客户端覆盖未被捕获的大 body。

### Admin API
| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/api/breakpoint/settings` | 获取当前 breakpoint 设置 |
| `POST` | `/api/breakpoint/settings` | 更新 `{enabled, max_body_bytes}`，返回有效值 |
| `POST` | `/api/breakpoint/resume` | 提交 edited headers/body 并继续 request 或 response |
| `GET` | `/api/config/performance` | 返回 Performance 配置中的 `breakpoint.timeout_ms`、`timeout_min_ms`、`timeout_max_ms` |
| `PUT` | `/api/config/performance` | 使用 `{breakpoint_timeout_ms}` 持久化并立即应用 auto-resume timeout |

设置边界：
- `max_body_bytes`: 默认 `1048576`，最大 `10485760`。
- `breakpoint_timeout_ms`: 默认 `30000`，固定安全范围为 `5000..=300000`，UI 展示后端常量，越界更新返回 `400`。

### Push Messages
| Type | Data |
| --- | --- |
| `breakpoint_request_paused` | `{request_id, method, url, headers, body, body_omitted, body_size, max_body_bytes}` |
| `breakpoint_response_paused` | `{request_id, status, headers, body, body_omitted, body_size, max_body_bytes}` |
| `breakpoint_resumed` | `{request_id}` |
| `breakpoint_settings_updated` | `{enabled, max_body_bytes}` |
| `settings_update(performance_config)` | 包含 `breakpoint.timeout_ms`、`timeout_min_ms`、`timeout_max_ms` |

### 规则协议
`breakpoint` 是控制类规则协议，只有全局 `enabled=true` 且当前请求命中对应规则时才会暂停：

```text
127.0.0.1:18080 breakpoint://request
127.0.0.1:18080 breakpoint://response
127.0.0.1:18080 breakpoint://request,response
```

空 value 或未知 value 不触发暂停，避免规则误写导致意外阻塞。

### Request Hook
处理位置：request body 进入 upstream 前。

行为：
1. 默认关闭、全局开关关闭或未命中 `breakpoint://request` 规则时，直接走原有快路径，不构造 breakpoint payload，不等待 oneshot。
2. 已在内存中的 body 在 `len <= max_body_bytes` 且为 UTF-8 时允许编辑。
3. streaming body 只有在 `Content-Length <= max_body_bytes` 时才 collect；未知长度或超过限制时保持 streaming，并发送 header-only pause。
4. header-only pause 的 `body_omitted=true`，`body_size` 尽量使用 `Content-Length` 或已知大小。
5. resume 时仅当 pause 阶段标记 body editable 且新 body 未超过上限，才替换 body。
6. 等待 resume 使用 `timeout_ms`，超时后 cancel pending 并继续原始请求。

### Response Hook
处理位置：response 返回 client 前。

行为：
1. 默认关闭、全局开关关闭或未命中 `breakpoint://response` 规则时，保持原有 response streaming/tee 快路径。
2. 普通 response 只有在明确 `Content-Length <= max_body_bytes` 时，为 breakpoint 读取完整 body 并允许编辑。
3. 未知长度、超过限制或二进制性能模式命中的 response 不强制缓存；进入 header-only pause 或保持 streaming，避免高内存和高延迟。
4. SSE 只在明确长度且不超过上限时缓存；否则跳过 body breakpoint，继续 streaming。
5. response breakpoint 同样使用 `timeout_ms` 自动放行。
6. body 被替换后才重新计算 `Content-Length`，否则保留原路径行为。

## 前端设计
- `useBreakpointStore` 保存 `maxBodyBytes`、paused request/response，以及 `bodyOmitted`。
- Settings -> Performance 提供 Breakpoint Auto-Resume Timeout，说明无人 resume 时自动放行的应用场景，并使用后端返回的 min/max 渲染滑块。
- `pushService` 消费 `breakpoint_paused` payload，`body_omitted=true` 时不回退读取 TrafficDetail 里已有 body，避免把未捕获 body 误显示为可编辑。
- TrafficDetail 在 header-only pause 时禁用 body 编辑和 body tab。
- Monaco body editor 使用 lazy import，仅在可编辑 paused body 场景加载，避免默认 TrafficDetail 打开时引入重型 editor chunk。

## 性能默认值与开销控制
- 默认 `enabled=false`；仅打开全局开关但没有命中 `breakpoint://request` / `breakpoint://response` 规则时也不会暂停任何流量。
- 默认热路径不 clone 大 body、不 collect streaming body、不创建 pending pause。
- 大 body 或未知长度 body 不进入 UI 编辑器。
- 超时自动放行，避免业务请求无限挂起。
- SSE 和二进制响应优先保持 streaming。
- Breakpoint timeout 配置越界时返回错误；min/max 是服务端固定安全边界，UI 只展示同一份后端常量，避免前端重复维护边界。

## 变更文件
| File | Action |
| --- | --- |
| `crates/bifrost-admin/src/breakpoint.rs` | 扩展 runtime timeout、pending editable 标记、resume body 上限保护 |
| `crates/bifrost-admin/src/handlers/breakpoint.rs` | settings 返回和 push 使用有效值 |
| `crates/bifrost-admin/src/handlers/config.rs` | Performance 配置返回/更新 breakpoint timeout |
| `crates/bifrost-storage/src/unified_config.rs` | 持久化 breakpoint timeout 与 bounds |
| `crates/bifrost-admin/src/push.rs` | 扩展 paused/settings/performance push payload |
| `crates/bifrost-admin/src/openapi.rs` | 更新 breakpoint 和 performance API 描述 |
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
| `human_tests/breakpoint-hook.md` | 新增性能防护真实场景用例 |
| `docs/breakpoint.md` | 新增用户使用手册 |
| `README.md` / `docs/README.md` | 增加使用手册入口 |

## 测试方案

### 单元测试
- `BreakpointManager` 默认值：默认关闭，body 上限和 runtime timeout 默认值正确。
- settings clamp：超过最大 body 上限后读取到有效上限。
- Performance config：`breakpoint_timeout_ms` 持久化、越界校验、更新后立即影响 runtime timeout。
- header-only pause：resume body 被丢弃。

### E2E
- 默认关闭：2 MiB request body 通过代理正常完成，不产生 breakpoint pause。
- 规则应用：全局开关开启但没有 `breakpoint://...` 规则时不暂停；`breakpoint://request` 触发 request 阶段；`breakpoint://response` 触发 response 阶段；`breakpoint://request,response` 按 request 后 response 的顺序触发两次。
- request 小 body 与 headers 编辑：内置 mock server 收到 resume 后的 edited body/header。
- request breakpoint body 超限：4 KiB body 在 `max_body_bytes=1024` 时只 header-only pause，resume 伪造 body 不覆盖原始 body。
- response 小 body 与 headers 编辑：curl 收到 resume 后的 edited body/header。
- response breakpoint timeout：通过 `/api/config/performance` 设置 5s 后，未 resume 时约 5s 自动放行并返回正确响应。
- 所有新增 E2E 均使用脚本内置 `127.0.0.1` mock server，禁止依赖外网域名。

### Human Tests
- `human_tests/breakpoint-hook.md` 中 TC-BP-07/08/09 覆盖性能防护真实场景。
- `human_tests/breakpoint-hook.md` 中 TC-BP-10/11 覆盖 request/response 小 body 与 headers 编辑。
- 既有 UI 用例 TC-BP-01..06 保留，用于后续 UI 回归。

## Review/Fix/Test 闭环
- 第 1 轮：复核默认关闭快路径、body 上限、timeout、SSE/streaming 跳过策略、前端 lazy import；运行 focused unit tests、性能 E2E、human_tests 执行记录核对。
- 第 2 轮：复查 diff 是否仍有默认路径 body clone/collect、`body_omitted` 是否误回填 body、settings push 是否返回有效值；复跑 focused tests、fmt、clippy/工作区测试。
