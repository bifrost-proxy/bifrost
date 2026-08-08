# Breakpoint Rule Gating 真实场景测试

## 功能模块说明
验证 Breakpoint 的真实用户链路：打开/关闭全局 breakpoint、通过 `breakpoint://request` / `breakpoint://response` 规则选择暂停阶段、暂停状态持久恢复、在 Network 详情中编辑请求或响应、严格按阶段恢复、HTTPS 规则按目标自动解密、规则编辑器智能提示、OpenAPI 描述，以及性能防护边界。

## 前置条件
- Bifrost 使用临时数据目录启动，并携带 `--no-system-proxy`。
- 启动命令必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，避免 Sync 自动登录弹窗干扰测试。
- WebUI 地址示例：`http://localhost:8800/_bifrost/`。
- 至少有一个本地 mock HTTP server 可用；测试禁止依赖外网域名。
- 性能防护用例可执行：`CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`。
- UI 用例可执行：`pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts`。
- OpenAPI 用例可执行：`cargo test -p bifrost-admin breakpoint_openapi --lib`。

## 本轮执行记录（2026-08-07）

- TC-BP-01 至 TC-BP-22 已在隔离数据目录与本地 HTTP/HTTPS mock server 上逐条执行。
- 浏览器链路：`pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts`，5/5 通过；覆盖真实 Rules 配置、pending 刷新恢复、浅色/深色等待背景、request method/URL/header/body 编辑、response status/header/body 编辑和 timeout 褪色。
- 代理与跨域链路：`SKIP_BUILD=true bash e2e-tests/tests/test_breakpoint_performance_guard.sh`，全部通过；覆盖 Web Origin、`tauri://localhost` 预检与带 CSRF 的跨站 POST、HTTP/HTTPS、双阶段、超限 body、关闭释放与超时清理。
- Windows 平台链路：在 Parallels Windows 11 的独立 worktree 使用原生 Schannel curl 执行同一代理 E2E，全部通过；额外覆盖 IP 地址 HTTPS 不协商 ALPN 时仍能进入 response Breakpoint 并应用 202/body 编辑。
- Windows CI 链路：Rules job 固定 Node 22，WebSocket 探针使用其内置 API，不依赖未安装的 `web/node_modules`；探针还使用统一的有界启动等待并保留进程状态与 stderr。
- 协议、OpenAPI 与编辑器补全由对应 Rust/前端定向测试复核，均通过。

## 测试用例

### TC-BP-01: 通过 Toolbar 打开/关闭 Breakpoint
**目标：** 验证 Breakpoint 开关状态与 Toolbar 控件显示正确。

**步骤：**
1. 打开 WebUI，进入 Traffic 页面。
2. 查看顶部 Toolbar，确认存在 Breakpoint 图标、Label 和 Switch。
3. Hover Breakpoint 图标，查看使用说明。
4. 打开 Breakpoint Switch。
5. 关闭 Breakpoint Switch。

**预期结果：**
- Switch 可以正常切换。
- Toolbar 只提供一个 Breakpoint Switch，不显示 "Req" / "Res" 阶段子开关。
- Hover 说明明确：开关打开表示允许命中的 `breakpoint://request` / `breakpoint://response` 规则暂停；开关关闭会阻止新暂停并释放 pending。
- Breakpoint 图标和控件状态与开关状态一致。

**实际结果：** 2026-06-04 执行 `SKIP_FRONTEND_BUILD=1 pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts`，通过。测试确认 Toolbar 只展示一个 Breakpoint Switch，前置暂停图标已移除，Hover `Breakpoint` 文案显示全局门禁、`breakpoint://request` / `breakpoint://response` 和“单开关不会单独暂停流量”的说明。

---

### TC-BP-02: Request breakpoint 暂停并恢复请求
**目标：** 验证命中 `breakpoint://request` 的请求可以在发往 upstream 前暂停，并在 resume 后继续。

**步骤：**
1. 打开 Breakpoint Switch。
2. 添加命中本地 mock server 的规则：`127.0.0.1:<mock_port>/request-edit breakpoint://request`。
3. 启动本地 mock server，或使用 `e2e-tests/tests/test_breakpoint_performance_guard.sh` 内置 mock server。
4. 通过代理发送 HTTP 请求，例如：`curl --noproxy "" -x http://localhost:8800 http://127.0.0.1:<mock_port>/request-edit`。
5. 查看 Traffic 列表，确认新请求进入 pending 状态。
6. 打开 TrafficDetail，确认显示暂停状态和 Resume 按钮。
7. 点击 Resume。
8. 确认请求继续发送并收到 response。

**预期结果：**
- 请求在发送 upstream 前暂停。
- TrafficDetail 显示暂停提示和 Resume 按钮。
- Resume 后请求正常完成。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本使用本地 mock server 验证 `breakpoint://request` 命中后 request 在发送 upstream 前暂停，resume 后请求继续完成。

---

### TC-BP-03: Response breakpoint 暂停并恢复响应
**目标：** 验证命中 `breakpoint://response` 的响应可以在返回 client 前暂停，并在 resume 后继续。

**步骤：**
1. 打开 Breakpoint Switch。
2. 添加命中本地 mock server 的规则：`127.0.0.1:<mock_port>/response-edit breakpoint://response`。
3. 启动本地 mock server，或使用 `e2e-tests/tests/test_breakpoint_performance_guard.sh` 内置 mock server。
4. 通过代理向 `http://127.0.0.1:<mock_port>/response-edit` 发送 HTTP 请求。
5. 确认请求先正常发送到 upstream。
6. 确认 response 在返回 client 前暂停。
7. 打开 TrafficDetail，确认暂停时可以看到 response body 内容。
8. 点击 Resume。

**预期结果：**
- Request 正常发送 upstream，response 在返回 client 前暂停。
- TrafficDetail 显示 response body 和暂停状态。
- Resume 后 response 正常返回 client。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本验证 `breakpoint://response` 命中后 response 在返回 client 前暂停，resume 后 response 正常返回。

---

### TC-BP-04: 规则同时启用 request 和 response 阶段
**目标：** 验证同一请求可分别在 request 和 response 阶段暂停。

**步骤：**
1. 打开 Breakpoint Switch。
2. 添加命中本地 mock server 的规则：`127.0.0.1:<mock_port>/both-phase breakpoint://request,response`。
3. 通过代理向本地 mock server 发送 HTTP 请求。
4. Request 暂停后点击 Resume。
5. Request 发送到 upstream 并收到 response。
6. Response 暂停后点击 Resume。
7. 确认 response 返回 client。

**预期结果：**
- request 和 response 两个阶段都会暂停。
- 每个阶段需要分别 resume。
- 两次 resume 后请求正常完成。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本使用 `breakpoint://request,response` 验证同一请求先暂停 request，resume 后再暂停 response，两个阶段分别 resume 后请求完成。

---

### TC-BP-05: 暂停期间关闭 Breakpoint
**目标：** 验证关闭 breakpoint 会释放 pending 请求。

**步骤：**
1. 打开 Breakpoint Switch，并配置命中请求的 `breakpoint://request` 规则。
2. 通过代理向本地 mock server 发送请求，使请求进入暂停状态。
3. 关闭 Breakpoint Switch。
4. 查看 pending 请求是否被释放。

**预期结果：**
- 关闭 breakpoint 后 pending 请求被释放或取消。
- TrafficDetail 不再显示暂停状态。

**实际结果：** 2026-06-04 执行 `SKIP_FRONTEND_BUILD=1 pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts` 与 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。UI 测试覆盖单开关语义；E2E 单独覆盖 request pending 后关闭 Breakpoint settings，curl 自动继续完成且 mock server 收到原始 body。

---

### TC-BP-06: Breakpoint 关闭时保持原有行为
**目标：** 验证 breakpoint 关闭时不会改变代理转发和 Traffic 页面行为。

**步骤：**
1. 确认 Breakpoint Switch 处于关闭状态。
2. 通过代理向本地 mock server 发送多条 HTTP 请求。
3. 确认 Traffic 正常展示且没有 breakpoint delay。
4. 检查 filter、search、clear traffic 等基础功能仍可用。

**预期结果：**
- Breakpoint 关闭时无额外暂停或明显延迟。
- Traffic 基础功能保持正常。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本验证默认关闭时 2 MiB large POST 正常转发且不产生 `breakpoint_paused`，全局开关打开但无 breakpoint 规则时请求也不暂停。

---

### TC-BP-07: 性能回归 - Breakpoint 默认关闭时 large request body 不暂停
**目标：** 确保 breakpoint 默认关闭时不会让大 request body 进入 breakpoint pause 或复制到 breakpoint UI。

**步骤：**
1. 使用新 binary 和临时数据目录启动 Bifrost，设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并携带 `--no-system-proxy`。
2. 调用 `GET /_bifrost/api/breakpoint/settings`。
3. 确认 breakpoint settings 中 `enabled=false`、`max_body_bytes=1048576`。
4. 调用 `GET /_bifrost/api/config/performance`，确认 `breakpoint.timeout_ms=30000`，固定安全边界为 `breakpoint.timeout_min_ms=5000` 与 `breakpoint.timeout_max_ms=300000`。
5. 通过代理向 mock HTTP echo server 发送 2 MiB POST body。
6. 检查 curl 正常完成，response 中 body size 为 2097152，且没有 `breakpoint_paused` push。

**预期结果：**
- Breakpoint 默认关闭。
- Large POST 不被暂停。
- 不产生 `breakpoint_paused`。
- 请求在 mock server 的正常耗时范围内完成。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本验证默认 settings、Performance timeout 默认值 `30000`、固定安全边界 `timeout_min_ms=5000` / `timeout_max_ms=300000`，2 MiB POST 通过代理完成。

---

### TC-BP-08: 性能防护 - Request breakpoint 跳过 oversized body 编辑
**目标：** 命中 `breakpoint://request` 后，超过 `max_body_bytes` 的 body 只暂停 headers，不把大 body 放入 UI，也不允许 resume 覆盖 body。

**步骤：**
1. 添加命中本地 mock server 的规则：`127.0.0.1:<mock_port>/oversized breakpoint://request`。
2. POST `/api/breakpoint/settings`，body 为 `{ "enabled": true, "max_body_bytes": 1024 }`。
3. 通过 WebSocket push 监听 `breakpoint_paused`。
4. 通过代理发送 4 KiB POST body。
4. 检查 push event 中 `body_omitted=true`、`body_size=4096`、`max_body_bytes=1024`。
5. 调用 `/api/breakpoint/resume`，携带原 headers 和一个伪造 body。
6. 检查 upstream/mock server 收到的仍是原始 4 KiB body，而不是伪造 body。

**预期结果：**
- Headers 仍可暂停和 resume。
- Oversized body 不进入 WebUI editor。
- Resume body 被服务端忽略，原请求体保持不变。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本收到 `body_omitted=true`、`body_size=4096`、`max_body_bytes=1024`，resume 传入伪造 body 后 mock server 仍收到原始 4 KiB body。

---

### TC-BP-09: 性能防护 - Response breakpoint timeout 自动放行
**目标：** 用户不 resume 时，breakpoint timeout 自动放行，避免业务请求无限挂起。

**步骤：**
1. 添加命中本地 mock server 的规则：`127.0.0.1:<mock_port>/timeout breakpoint://response`。
2. POST `/api/breakpoint/settings`，body 为 `{ "enabled": true, "max_body_bytes": 1048576 }`。
3. PUT `/api/config/performance`，body 为 `{ "breakpoint_timeout_ms": 5000 }`。
4. 通过 WebSocket push 监听 response 阶段的 `breakpoint_paused`。
5. 通过代理发送 HTTP GET 请求，但不调用 resume。
6. 测量 curl 完成耗时，并检查响应内容。

**预期结果：**
- Response 被暂停后约 5s 自动继续。
- Auto-resume timeout 由 Settings -> Performance / `/api/config/performance` 配置生效。
- curl 不会无限挂起。
- 响应内容正确。
- pending breakpoint 在超时后被清理。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本把 `breakpoint_timeout_ms` 设置为固定下限 5000ms，未调用 resume，curl 在自动放行窗口内返回 `hello-breakpoint-timeout`。

---

### TC-BP-10: 功能正确性 - Request breakpoint 可编辑小 body 和 headers
**目标：** 命中 `breakpoint://request` 后，小 body 和 request headers 可以被编辑，并真实影响 upstream 收到的请求。

**步骤：**
1. 添加命中本地 mock server 的规则：`127.0.0.1:<mock_port>/request-edit breakpoint://request`。
2. POST `/api/breakpoint/settings`，body 为 `{ "enabled": true, "max_body_bytes": 1048576 }`。
3. 通过 WebSocket push 监听 `breakpoint_paused`。
4. 通过代理向本地 mock server 发送 `original-request-body`。
4. 检查 push event 中 `phase=request`、`body_omitted=false`、`body=original-request-body`。
5. 调用 `/api/breakpoint/resume`，把 body 改为 `edited-request-body`，并追加 header `X-Breakpoint-Request: edited`。
6. 检查 mock server response，确认 upstream 收到 edited body 和 edited header。

**预期结果：**
- Request breakpoint 可以编辑小 body。
- Request breakpoint 可以编辑 headers。
- Mock server 只依赖本地端口，不访问外网。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本收到 request 阶段 `body=original-request-body`，resume 后 mock server 返回 `len=19`、`prefix=edited-request-b`、`breakpoint_header=edited`。

---

### TC-BP-11: 功能正确性 - Response breakpoint 可编辑小 body 和 headers
**目标：** 命中 `breakpoint://response` 后，小 body 和 response headers 可以被编辑，并真实影响 client 收到的响应。

**步骤：**
1. 添加命中本地 mock server 的规则：`127.0.0.1:<mock_port>/response-edit breakpoint://response`。
2. POST `/api/breakpoint/settings`，body 为 `{ "enabled": true, "max_body_bytes": 1048576 }`。
3. 通过 WebSocket push 监听 `breakpoint_paused`。
4. 通过代理向本地 mock server 发送 GET 请求。
4. 检查 push event 中 `phase=response`、`body_omitted=false`、`body=hello-breakpoint-timeout`。
5. 调用 `/api/breakpoint/resume`，把 body 改为 `edited-response-body`，并追加 header `X-Breakpoint-Response: edited`。
6. 检查 curl 输出和响应头。

**预期结果：**
- Response breakpoint 可以编辑小 body。
- Response breakpoint 可以编辑 headers。
- curl 收到编辑后的响应内容和响应头。
- Mock server 只依赖本地端口，不访问外网。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本收到 response 阶段 `body=hello-breakpoint-timeout`，resume 后 curl body 为 `edited-response-body`，响应头包含 `X-Breakpoint-Response: edited`。

---

### TC-BP-12: 规则应用 - 全局开关打开但无 breakpoint 规则不暂停
**目标：** 验证仅打开 Toolbar/API 的全局 Breakpoint 开关不会暂停任何流量，必须命中 `breakpoint://request` 或 `breakpoint://response` 规则才生效。

**步骤：**
1. POST `/api/breakpoint/settings`，body 为 `{ "enabled": true, "max_body_bytes": 1048576 }`。
2. 不创建任何 `breakpoint://...` 规则。
3. 通过 WebSocket push 监听 `breakpoint_paused`。
4. 通过代理向本地 mock server 发送 POST 请求。
5. 等待短时间，确认 curl 已完成且没有 `breakpoint_paused` event。

**预期结果：**
- 请求不会暂停。
- 不产生 `breakpoint_paused` push。
- Mock server 正常收到请求并返回响应。

**实际结果：** 2026-06-04 执行 `CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。脚本仅开启全局 settings，不创建 `breakpoint://...` 规则，POST 到本地 mock server 正常返回且等待窗口内没有 `breakpoint_paused` event。

---

### TC-BP-13: 规则编辑器智能提示 - breakpoint 协议与 value 提示
**目标：** 验证规则编辑器中 `breakpoint` 协议的补全、value 补全和 hover 文档没有遗漏。

**步骤：**
1. 打开 Rules 编辑器。
2. 在规则右侧触发协议补全，输入 `break`。
3. 选择 `breakpoint`。
4. 在 `breakpoint://` 后触发 value 补全。
5. Hover `breakpoint://` 协议名查看说明。
6. 执行前端单元测试：`cd web && npm test -- --run src/components/BifrostEditor/snippet/protocol-docs.test.ts`。

**预期结果：**
- 协议补全包含 `breakpoint://request`、`breakpoint://response`、`breakpoint://request,response`。
- value 补全包含 `request`、`req`、`response`、`res`、`request,response`、`both`、`all`。
- hover 文档明确说明：Toolbar Switch 是全局门禁，规则 value 决定 request/response 阶段，`req`/`res`/`both`/`all` 是支持的别名，空值或未知值不触发暂停。
- 单元测试通过。

**实际结果：** 2026-06-04 执行 `pnpm --dir web test:unit src/components/BifrostEditor/snippet/protocol-docs.test.ts` 与 `SKIP_FRONTEND_BUILD=1 pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts`，通过。单元测试覆盖完整 value completion 别名集合；Playwright 覆盖真实 Rules 编辑器可触发 Breakpoint value suggestions。

---

### TC-BP-14: OpenAPI 描述 Breakpoint API 与 UI 分工
**目标：** 验证 OpenAPI 明确暴露 Breakpoint API、Performance timeout schema、规则 API 的相位配置说明，并且不再暴露旧的 request/response 子开关或 settings timeout。

**步骤：**
1. 启动 Bifrost 并打开 `http://localhost:8800/_bifrost/swagger`，确认 Swagger 页面可用于外部管理 API 调试。
2. 调用 `GET /_bifrost/api/openapi.json`。
3. 检查 schema `BreakpointSettings` 只包含 `enabled` 和 `max_body_bytes`，不包含 `hook_request`、`hook_response` 或 `timeout_ms`。
4. 检查 schema `PerformanceBreakpointConfig` 包含 `timeout_ms`、`timeout_min_ms`、`timeout_max_ms`。
5. 检查 `UpdatePerformanceConfigRequest.breakpoint_timeout_ms` 包含固定安全边界 `minimum=5000` 与 `maximum=300000`。
6. 检查 `/api/config/performance` GET/PUT 带有 request/response schema。
7. 执行单元测试：`cargo test -p bifrost-admin breakpoint_openapi --lib`。

**预期结果：**
- OpenAPI 用于外部管理客户端、CLI/自动化和 Swagger 调试页；产品 UI 已分别由 Toolbar、Rules、Settings Performance 和 TrafficDetail 承载。
- OpenAPI 中 Breakpoint settings 不包含旧的阶段子开关，也不包含 timeout。
- Breakpoint timeout 只通过 Performance 配置暴露，并携带固定安全 bounds。
- Breakpoint 阶段配置通过 Rules API / 规则 DSL 暴露。

**实际结果：** 2026-06-04 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin openapi::tests --lib`，通过。2 个测试确认 OpenAPI 包含 Breakpoint settings/resume/rules/performance 路径，settings 不包含旧 hook/timeout 字段，`breakpoint_timeout_ms` 暴露固定 `minimum=5000` 与 `maximum=300000`。

---

### TC-BP-15: 侧边栏 OpenAPI 入口
**目标：** 验证 WebUI 为 Swagger/OpenAPI 提供明确入口，位置在侧边栏底部主题切换按钮上方，并打开独立 OpenAPI 页面。

**步骤：**
1. 打开 WebUI 任意主页面，例如 Traffic。
2. 查看左侧侧边栏底部。
3. 确认主题切换按钮上方显示小号 `OpenAPI` 入口。
4. 点击 `OpenAPI`。
5. 确认打开的页面 URL 包含 `/_bifrost/swagger`。

**预期结果：**
- `OpenAPI` 入口不占用主导航项，不影响侧边栏滚动。
- 入口位于主题切换上方。
- 点击后打开独立 Swagger/OpenAPI 页面。

**实际结果：** 2026-06-04 执行 `SKIP_FRONTEND_BUILD=1 pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts`，通过。测试确认 `OpenAPI` 文案可见，位于 `theme-toggle` 上方，点击后弹出页面 URL 匹配 `/_bifrost/swagger`。

---

### TC-BP-16: 协议 registry 覆盖 breakpoint
**目标：** 验证 `breakpoint` 已包含在核心规则协议 registry 测试清单中，避免新增协议后 workspace 全量测试因测试常量漏同步失败。

**步骤：**
1. 执行全协议清单单测：
   ```bash
   cargo test -p bifrost-tests --test rules_test test_all_protocols -- --nocapture
   ```
2. 执行完整规则测试文件：
   ```bash
   cargo test -p bifrost-tests --test rules_test
   ```

**预期结果：**
- `test_all_protocols` 通过，`ALL_PROTOCOLS.len()` 与测试清单均为 76。
- 测试清单包含 `breakpoint`，且 `Protocol::parse("breakpoint")` 成功。
- 完整 `rules_test` 通过。

**实际结果：** 2026-06-04 执行 `cargo test -p bifrost-tests --test rules_test test_all_protocols -- --nocapture`，单用例通过；执行 `cargo test -p bifrost-tests --test rules_test`，35 个规则协议测试全部通过。

---

### TC-BP-17: Network 刷新后恢复 pending 状态并直接定位编辑区
**目标：** 验证请求暂停后，刷新 WebUI 或重连 push 不会丢失断点，Network 列表和详情仍能恢复正确阶段及剩余时间。

**步骤：**
1. 打开 Breakpoint 并配置 request breakpoint 规则。
2. 通过代理发起请求，等待 Network 列表出现 request 暂停标识和整行淡黄色背景。
3. 选择该请求并刷新页面。
4. 确认页面通过 `GET /api/breakpoint/pending` 恢复暂停状态。
5. 检查详情自动打开 Request 编辑区，并显示阶段、倒计时和 `Resume unchanged` / `Apply & Resume`。
6. 分别切换浅色、深色主题，确认警示背景可辨识；恢复请求后确认背景立即消失。

**预期结果：**
- 刷新与 WebSocket 重连不释放也不隐藏 pending 请求。
- Network 行展示当前暂停阶段，详情自动定位到对应 request/response 编辑区。
- 暂停行在浅色和深色主题下均使用主题 token 生成的淡黄色背景；恢复、关闭或 timeout 后背景消失。
- 倒计时来自服务端 `deadline_at_ms`，不会因页面刷新重新计时。

**实际结果：** 2026-08-07 执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts`，5 个用例全部通过；补充后聚焦真实 Network 用例再次 `1/1` 通过。request/response 暂停刷新后状态、详情 tab 和倒计时均恢复；浅色/深色 warning 背景颜色不同且均非透明，resume 与 timeout 后 `data-breakpoint-phase` 和警示背景均消失。

---

### TC-BP-18: Request 阶段编辑 method、URL/query、重复 headers 与 body
**目标：** 验证 request breakpoint 的编辑内容真实作用于 upstream，而非只改页面展示。

**步骤：**
1. 发起一个没有 `Content-Length` 的 chunked POST，使其命中 request breakpoint。
2. 在 Network 详情把 method 改为 `PUT`，URL/query 改到同一 mock server 的新路径。
3. 新增 `X-Breakpoint-Request: edited`，保留 headers 的顺序与重复项能力，并把 body 改为 `edited-request-body`。
4. 点击 `Apply & Resume`，等待请求完成。
5. 检查 mock server 实际收到的 method、path/query、header 与 body。

**预期结果：**
- 未知长度但小于捕获上限的 request body 可以完整编辑。
- upstream 收到 `PUT`、新 path/query、编辑后的 header/body。
- `Resume unchanged` 不提交编辑；`Apply & Resume` 才应用当前编辑。

**实际结果：** 2026-08-07 执行完整 Breakpoint Playwright 与 `SKIP_BUILD=true bash e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。Playwright 使用真实 chunked POST，经页面把 method/URL/query/header/body 编辑后恢复，mock server 实际收到 `PUT`、新路径与编辑内容；API E2E 复核相同 upstream 结果。

---

### TC-BP-19: Response 阶段编辑 status、重复 headers 与 body
**目标：** 验证 response breakpoint 可直接修改最终返回给 client 的状态码、重复响应头和 body。

**步骤：**
1. 发起命中 response breakpoint 的请求。
2. 在 Network 详情刷新页面后重新选中该请求。
3. 把 status 改为 `418`，新增响应 header 和两个同名 `Set-Cookie`，并编辑响应 body。
4. 点击 `Apply & Resume`。
5. 检查真实 client 收到的状态码、两个 `Set-Cookie` 与新 body。

**预期结果：**
- client 收到 HTTP 418 和编辑后的 body。
- 两个 `Set-Cookie` 均保留，不因 map 化而丢失重复 header。
- TrafficDetail 刷新为最终应用后的响应内容。

**实际结果：** 2026-08-07 执行完整 Breakpoint Playwright 与代理 E2E，通过。UI 真实 client 收到 HTTP 418、`X-UI-Breakpoint-Response: response-edited` 与 `edited-response-body`；API E2E 另验证两个独立 `Set-Cookie` 未丢失；刷新后的 Network 详情仍能编辑并恢复该 response。

---

### TC-BP-20: phase 冲突、超时与 pending 清理
**目标：** 验证恢复 API 不会误放行另一个阶段，并且关闭/超时不会遗留僵尸 pending。

**步骤：**
1. 创建 `breakpoint://request,response` 规则并发起请求。
2. request 暂停时，使用同一 request id 但 `phase=response` 调用 resume。
3. 确认 API 返回 409，随后查询 pending 仍是 request。
4. 使用正确 request phase 恢复，等待 response 再次暂停并正确恢复。
5. 另发一条 response breakpoint 请求，不调用 resume，等待 timeout。
6. 查询 pending 列表并关闭 Breakpoint。

**预期结果：**
- 错误 phase 返回 409，不消费当前 pending，也不误恢复请求。
- 两阶段按 request -> response 顺序分别恢复。
- timeout 或关闭全局开关后 pending 列表为空，业务请求被安全放行。

**实际结果：** 2026-08-07 执行代理 E2E 与聚焦真实 Network Playwright，均通过。错误 phase 返回 409 且 pending 未被消费；正确恢复后按 request -> response 顺序暂停；5 秒 timeout 与关闭开关均放行业务请求并把 pending 清为 0，UI timeout 行警示背景同步消失。

---

### TC-BP-21: HTTPS Breakpoint 按目标自动启用 TLS 解密
**目标：** 验证全局 TLS interception 关闭时，具体 HTTPS breakpoint 规则仍能暂停并编辑目标请求，同时不扩大为全局解密。

**步骤：**
1. 保持全局 TLS interception 关闭并确认临时 CA 已生成。
2. 在本地 `8443` 启动自签名 HTTPS mock server。
3. 配置 `127.0.0.1:8443 breakpoint://response` 并打开 Breakpoint。
4. 使用信任测试 CA/`curl -k` 的 client 通过 Bifrost 访问 `https://127.0.0.1:8443/https-breakpoint`。
5. 确认 response 暂停，把 status/body 改为 `202` / `edited-https-response` 后恢复。
6. 检查 client 实际响应，并确认宽泛 `* breakpoint://response` 不作为自动 TLS 解密触发器。

**预期结果：**
- 仅具体 host 范围的 breakpoint 规则触发该 CONNECT 的 TLS 解密。
- HTTPS response 在 client 收到前暂停，恢复后 client 收到 202 和新 body。
- 显式 `tlsPassthrough://` 仍优先；未信任 CA 的 client 会按 TLS 安全语义失败，不会静默绕过。

**实际结果：** 2026-08-07 在 macOS 与 Parallels Windows 11 的全局 TLS interception 关闭隔离实例中执行代理 E2E，均通过。`127.0.0.1:8443 breakpoint://response` 自动触发该 CONNECT 的 TLS 解密，response 暂停后 client 实际收到 202 与 `edited-https-response`；Windows 原生 Schannel curl 对 IP 地址未协商 ALPN，代理通过首包嗅探识别 HTTP/1.1 后仍进入 Breakpoint。具体 host/port、非默认端口 URL 保留、无 ALPN 非 HTTP 透传和宽泛规则拒绝路径另有 Rust 单测覆盖。

---

### TC-BP-22: Web 与 Desktop Origin 的 Breakpoint API CORS
**目标：** 验证新增 pending/resume 接口沿用管理端统一 CORS、CSRF 和桌面 WebView Origin 策略，不出现 Web 可用但 Desktop 不可用。

**步骤：**
1. 对 `GET /api/breakpoint/pending` 分别使用 `Origin: http://localhost:3000` 和 `Origin: tauri://localhost` 发起 OPTIONS 预检与实际 GET。
2. 对 `POST /api/breakpoint/resume` 使用上述两个 Origin 发起 OPTIONS，声明 `Content-Type, X-Client-Id, X-Bifrost-CSRF`。
3. 请求进入 request pause 后，获取 `/api/security/csrf` token。
4. 使用 `Origin: tauri://localhost`、`Sec-Fetch-Site: cross-site`、有效 CSRF 调用 resume 的错误 phase 路径。
5. 检查 409 业务响应仍带正确 `Access-Control-Allow-Origin`，pending 未被消费。

**预期结果：**
- Web 本地 Origin 与 Tauri Origin 的预检均为 204，并回显对应 allow-origin。
- allow-methods 包含 GET/POST/OPTIONS，allow-headers 包含桌面 client id 与 CSRF header。
- Desktop 实际 POST 通过跨站 Origin guard 和 CSRF 校验，进入 Breakpoint 自身的 409 phase 校验，而不是被 CORS/安全层拦截。
- 非信任外部 Origin 仍不获得 CORS 许可。

**实际结果：** 2026-08-07 执行 `SKIP_BUILD=true bash e2e-tests/tests/test_breakpoint_performance_guard.sh`，通过。localhost Web Origin 与 `tauri://localhost` 对 pending GET、resume POST 的 OPTIONS 均返回 204 和正确 allow-origin/allow-headers；带 Tauri Origin、cross-site fetch metadata 与有效 CSRF 的实际 resume 请求进入业务 409 phase mismatch，响应保留 Tauri allow-origin 且 pending 未被消费。

---

### TC-BP-23: Review 回归 - 搜索态、远端时钟、流式响应、规则上限、URL 路由与无 body 状态
**目标：** 验证 PR review 暴露的六类边界已关闭，正常/搜索列表和 HTTP/HTTPS 代理行为保持一致。

**步骤：**
1. 执行 Breakpoint 决策、URL 路由和状态 framing 回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy response_breakpoint_buffers_only_known_safe_lengths -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy no_body_status_edits_remove_response_framing -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy plaintext_breakpoint_edited_url_ignores_stale_host_rule -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy plaintext_unknown_length_response_pauses_before_stream_eof -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy breakpoint_capture_limit_does_not_reduce_response_rule_buffer_limit -- --nocapture
   ```
2. 执行真实 Network UI 回归：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts --grep "Network detail edits"
   ```
3. request 暂停后进入 Fuzzy Search，搜索该 URL，确认搜索结果行继续显示淡黄色背景、阶段标签；退出搜索后状态不丢失。
4. 在浏览器时钟比代理主机快 1 小时的模拟环境中刷新页面，打开断点详情并检查倒计时。
5. 对未知 Content-Length 且不结束的响应确认先在 header 阶段进入 pending；将 status 改成 204 后恢复。
6. 配置旧 host/path rewrite 规则，在 request breakpoint 中把 URL 改到另一个 upstream；另配置 response body replace，并把 Breakpoint capture limit 调小到响应体以下。

**预期结果：**
- SearchMode 与普通 Network 表都显示相同的 warning token 背景和 request/response 阶段；resume/timeout 后同步清除。
- 倒计时使用 `deadline_at_ms - server_now_ms` 得到服务端剩余时间，再按浏览器本地经过时间递减；客户端与代理时钟偏差不会显示提前超时。
- 未知长度响应不会为了捕获 body 等待 EOF；Breakpoint 先以 header-only 状态暂停，普通流仍可在恢复后继续。
- `max_body_bytes` 只限制 Breakpoint 可编辑捕获，不降低独立的响应规则/script body-processing 上限。
- 编辑后的绝对 URL 决定 host、port、scheme 和 path，不重新套用请求暂停前的 host/path rewrite。
- status 改为 1xx、204 或 304 时清空 payload，并移除 `Content-Length` / `Transfer-Encoding`；Traffic 记录同步为最终状态且无响应 body。

**实际结果：** 2026-08-07 按步骤执行 5 个聚焦 proxy 回归测试与真实 Network Playwright，全部通过。Fuzzy Search 中暂停行保留 warning token 背景和阶段标签；浏览器 `Date.now()` 人为快 1 小时后倒计时仍显示服务端剩余时间而非 `0.0s`。未知长度响应以 header-only 状态及时暂停；定长响应在 Network 详情中完成 response status/header/body 编辑。编辑 URL 后请求到达新 upstream 且不再套用旧 host/path rule；小 Breakpoint capture limit 不降低 response rule buffer limit；无 body 状态会清空 payload 和 framing headers。

## 清理步骤
- 结束测试脚本后确认临时 Bifrost 进程、mock server、WebSocket probe 均已退出。
- 删除临时 `BIFROST_DATA_DIR`。
- 如手动启动过服务，使用对应 PID 或 `bifrost stop` 停止，并确认没有修改系统代理。
