# Breakpoint Rule Gating 真实场景测试

## 功能模块说明
验证 Breakpoint 的真实用户链路：打开/关闭全局 breakpoint、通过 `breakpoint://request` / `breakpoint://response` 规则选择暂停阶段、pause/resume、编辑 headers/body、规则编辑器智能提示、OpenAPI 描述，以及性能防护边界。

## 前置条件
- Bifrost 使用临时数据目录启动，并携带 `--no-system-proxy`。
- 启动命令必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，避免 Sync 自动登录弹窗干扰测试。
- WebUI 地址示例：`http://localhost:8800/_bifrost/`。
- 至少有一个本地 mock HTTP server 可用；测试禁止依赖外网域名。
- 性能防护用例可执行：`CARGO_INCREMENTAL=0 e2e-tests/tests/test_breakpoint_performance_guard.sh`。
- UI 用例可执行：`pnpm --dir web exec playwright test tests/ui/breakpoint-ui.spec.ts`。
- OpenAPI 用例可执行：`cargo test -p bifrost-admin breakpoint_openapi --lib`。

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

## 清理步骤
- 结束测试脚本后确认临时 Bifrost 进程、mock server、WebSocket probe 均已退出。
- 删除临时 `BIFROST_DATA_DIR`。
- 如手动启动过服务，使用对应 PID 或 `bifrost stop` 停止，并确认没有修改系统代理。
