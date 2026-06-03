# ChatGPT Web Adapter 真实场景测试

## 功能模块说明

验证 IM Gateway Runner 的内置 `chatgpt_web` adapter：WebUI 可配置 ChatGPT Web Runner，后端可弹出 Edge/Chrome 完成登录并保存本机登录态，执行阶段默认使用 headed 可见浏览器创建新对话、向既有对话追加消息、等待最终 assistant 结果，并通过 Chat Gateway 返回脱敏 artifacts。当前真实 IM 验证期默认保留可见浏览器，便于观察真实页面输入和发送动作；稳定后可再切回 headless。该 adapter 不依赖 Agent 控制浏览器，不使用 probe 脚本作为正式流程。

## 前置条件

1. 使用独立临时数据目录启动 Bifrost，必须带 `--no-system-proxy`：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-chatgpt-web-adapter-test cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy
   ```
2. 本机安装 Microsoft Edge 或 Google Chrome。
3. 当前设备可以访问 `https://chatgpt.com`。
4. 如果是首次验证，准备一个可登录 ChatGPT Web 的账号。

## 测试用例列表

### TC-CWA-01: WebUI 创建 ChatGPT Web Runner 并检查登录态

操作步骤：
1. 打开 `http://127.0.0.1:18880/_bifrost/`。
2. 进入 Settings -> IM Gateway -> Runners。
3. 点击 Add Runner，Runner ID 填 `chatgpt-web`，Adapter 选择 `ChatGPT Web`。
4. Browser 选择 Microsoft Edge，Execution Mode 保持默认 `Headless`，Base URL 保持 `https://chatgpt.com`。
5. Run Timeout Seconds 保持默认 `7200`，或按长任务需要调大。
6. 保存 Runner。
7. 再次编辑 `chatgpt-web` Runner，点击 `Check Login`。

预期结果：
1. Runner 列表中 `chatgpt-web` 的 Adapter 显示为 `chatgpt_web`。
2. 未登录或登录失效时，登录状态显示 `auth_required` 或 `logged_out`，不会误判为 logged in。
3. 状态详情只展示 cookie 数量、header 名称数量、profile 路径，不展示 cookie value、Authorization 或完整 email。
4. 长任务超时默认不低于 7200 秒。
5. Execution Mode 默认显示为 `Headless`；只有显式选择 `Headed` 时才展示执行浏览器用于诊断。

### TC-CWA-02: 登录失效时自动弹出浏览器并捕获登录态

操作步骤：
1. 在 `chatgpt-web` Runner 编辑弹窗中点击 `Open Login Browser`。
2. 在弹出的 Edge/Chrome 窗口里完成 ChatGPT Web 登录。
3. 等待 WebUI 按钮 loading 结束；登录等待期间不应因为固定登录超时或前端 30 秒 API 超时自动失败。
4. 点击 `Check Login` 复查。

预期结果：
1. Bifrost 后端自动打开浏览器，不需要 Agent 点击或输入。
2. 登录成功后状态为 `logged_in`。
3. `identityComplete=true`、`accountCheckOk=true`、`cookieCount > 0`。
4. 登录态文件保存在 Bifrost 数据目录下，权限为 `0600`。
5. 登录结束后，除非显式配置保留浏览器，否则本次 CDP target 不继续累积重复 tab。
6. 如果 native `accounts/check` 因本机网络、代理或 TLS 传输问题无法发出请求，但本次浏览器真实 `accounts/check` 响应已证明登录账号可用，登录状态仍为 `logged_in`，并在 message 中保留 native probe 失败原因。

### TC-CWA-03: Chat Gateway list 操作返回脱敏会话列表

操作步骤：
1. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"","operation":"list","adapter":"chatgpt_web"}'
   ```
2. 检查响应 JSON。

预期结果：
1. 响应 `status` 为 `succeeded`。
2. `response` 是会话列表 JSON 文本，包含 `items`。
3. `artifacts` 中存在 `auth_probe.json` 和 `normalized_events.jsonl`。
4. artifacts 不包含 cookie、Authorization、`x-oai-is` 等敏感值。

### TC-CWA-04: Chat Gateway 创建新对话并等待最终结果

操作步骤：
1. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"你好","operation":"ask","sessionKey":"human-chatgpt-web"}'
   ```
2. 检查响应 JSON 与 run artifacts。

预期结果：
1. 响应 `adapter` 为 `chatgpt_web`。
2. 响应 `status` 为 `succeeded`。
3. `response` 为 ChatGPT Web assistant 的最终文本，非空。
4. `conversation_handoff.json` 包含 `conversationId`。
5. `conversation_final.json` 包含 final assistant 摘要，状态为 `finished_successfully` 或 `endTurn=true`。

### TC-CWA-05: 同一 sessionKey 追加消息到原对话

操作步骤：
1. 在 TC-CWA-04 后继续调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"你是谁","operation":"ask","sessionKey":"human-chatgpt-web"}'
   ```
2. 再调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"你可以做什么","operation":"ask","sessionKey":"human-chatgpt-web"}'
   ```
3. 对比三次 run 的 `conversation_handoff.json`。

预期结果：
1. 三次 run 使用同一个 `conversationId`。
2. 后两次消息追加到原会话，不创建新的 ChatGPT 会话。
3. 后两次 run 的 `conversation_handoff.json.eventTypes` 包含 `browser_ui`，不包含 `browser_fetch` 或 `browser_fetch_failed:*`，证明追加消息通过浏览器 UI 进入会话页后发送。
4. 每次响应都有非空最终 assistant 文本。

### TC-CWA-06: 既有 conversationId 显式追加消息

操作步骤：
1. 从任意一次成功 run 的 `conversation_handoff.json` 取 `conversationId`。
2. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"继续补充一句","operation":"send","adapter":"chatgpt_web","params":{"conversationId":"<conversationId>"}}'
   ```

预期结果：
1. 响应 `status` 为 `succeeded`。
2. `conversation_handoff.json` 中的 `conversationId` 与请求一致。
3. `conversation_handoff.json.eventTypes` 包含 `browser_ui`，不包含 `browser_fetch` 或 `browser_fetch_failed:*`。
4. 最终结果来自该 conversation 的最新 assistant message。

### TC-CWA-07: SSE handoff 中断后继续等待最终结果

操作步骤：
1. 使用长任务 prompt 触发 `chatgpt_web` run。
2. 在 run 过程中观察网络环境或临时断开短连接，使 `POST /backend-api/f/conversation` 的 handoff SSE 可能提前结束。
3. 继续等待 run 返回。

预期结果：
1. adapter 不因短 SSE handoff 中断立即失败。
2. 如果页面 URL 已进入 `/c/<conversationId>`，adapter 从 URL 恢复 conversation id。
3. 如果请求本来携带 conversationId，adapter 使用既有 conversation id 继续轮询。
4. 最终结果仍来自 `GET /backend-api/conversation/{conversation_id}` 的 finished assistant message。

### TC-CWA-08: 长任务运行期间可以停止等待

操作步骤：
1. 使用预计耗时较长的 prompt 触发 `chatgpt_web` run，确认 Runner 的 Run Timeout Seconds 不低于 `7200`。
2. 在 run 返回前，调用：
   ```bash
   curl -sS -X POST http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/runs/<runId>/stop
   ```
3. 再查询该 run 详情。

预期结果：
1. stop 请求写入 `stop_requested` marker。
2. `chatgpt_web` 在 native final poll 或 handoff recovery 后的提交确认等待中感知 stop marker。
3. run 状态收敛为 `stopped`，响应文本为 `ChatGPT Web run was stopped by request.`。
4. 不需要等待 Run Timeout Seconds 自然耗尽。

### TC-CWA-09: 登录态失效返回可理解错误

操作步骤：
1. 将测试数据目录下 `agent/im_gateway/chatgpt_web/auth_state.json` 临时移走。
2. 调用 TC-CWA-04 的请求。

预期结果：
1. 如果 `openOnAuthRequired=true` 且设备有 GUI，Bifrost 自动打开登录浏览器等待登录。
2. 登录等待没有固定超时；只有用户关闭登录浏览器、WebUI 主动 Stop Login、或设备无法打开浏览器时，请求返回 400，错误文本包含 `auth_required`。
3. 响应或日志不包含 cookie、Authorization 或完整认证 header value。

### TC-CWA-10: get 操作返回完整脱敏消息列表

操作步骤：
1. 从一次成功 run 的 `conversation_handoff.json` 取 `conversationId`。
2. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"","operation":"get","adapter":"chatgpt_web","params":{"conversationId":"<conversationId>"}}'
   ```

预期结果：
1. 响应 `status` 为 `succeeded`。
2. `response` JSON 包含 `messages` 数组。
3. `messages` 中至少包含 user 和 assistant 两类消息，每条消息包含 `id`、`role`、`createTime`、`text`。
4. 返回内容不包含 cookie、Authorization 或完整认证 header value。

### TC-CWA-11: WebUI 登录等待可主动停止

操作步骤：
1. 在 `chatgpt-web` Runner 编辑弹窗中点击 `Open Login Browser`，但不要完成 ChatGPT 登录。
2. 确认按钮处于 loading 状态，并出现 `Stop Login` 按钮。
3. 点击 `Stop Login`。
4. 再点击 `Check Login`。

预期结果：
1. WebUI 不因为固定超时自动关闭登录浏览器。
2. 点击 `Stop Login` 后，后端停止当前登录等待并关闭本次受控浏览器 target / 进程。
3. 原登录请求以 `auth_required: ChatGPT browser login was stopped by request` 结束，WebUI 不再保持 loading。
4. `Check Login` 显示 `auth_required` 或 `logged_out`，不会误判为 `logged_in`。

### TC-CWA-12: 回归 - ChatGPT Web 发送与等待性能优化

操作步骤：
1. 使用已登录的 `chatgpt_web` Runner，连续向同一个 `sessionKey` 发送两个短 prompt：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"Reply exactly CWA_PERF_ONE","operation":"ask","sessionKey":"human-chatgpt-web-perf"}'
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"Reply exactly CWA_PERF_TWO","operation":"ask","sessionKey":"human-chatgpt-web-perf"}'
   ```
2. 检查两次响应 JSON 与 run artifacts。
3. 查看 Bifrost 日志中 `chatgpt_web ask: send phase complete` 与 `chatgpt_web ask: complete (send + wait_final)` 记录。

预期结果：
1. 两次响应 `status` 均为 `succeeded`，且 `response` 非空。
2. 第二次 run 复用同一 `conversationId`，追加消息仍通过浏览器 UI composer 发送，不出现 `browser_fetch_failed:*`。
3. 日志包含 `send_ms`、`wait_ms`、`total_ms`，便于确认发送阶段与最终等待阶段耗时。
4. 短 SSE 中断但已经捕获 POST 时，`conversation_handoff.json.eventTypes` 包含 `browser_post_captured` 或其他 SSE 事件证据；adapter 不再额外等待用户消息可见后才进入最终轮询。
5. 短文本最终回答（例如 `OK`、`好`、`收到`）不能因为字符数太少被 DOM fallback 当作无内容，IM 通道必须收到对应 assistant 回复。
6. artifacts 与日志不包含 cookie、Authorization、`x-oai-is` 等敏感值。

### TC-CWA-13: headed 诊断模式便于观察真实页面操作

操作步骤：
1. 使用已登录的 `chatgpt_web` Runner，将 Runner 编辑弹窗中的 Execution Mode 显式切换为 `Headed`。
2. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"Reply exactly CWA_HEADED_VISIBLE","operation":"ask","sessionKey":"human-chatgpt-web-headed"}'
   ```
3. 在本机观察 Edge/Chrome 是否弹出真实 ChatGPT 页面，并观察页面输入、发送和等待动作。
4. 查看 Bifrost 日志中 `chatgpt_web send: launching browser`。

预期结果：
1. 仅在显式选择 `Headed` 时，执行浏览器是可见窗口。
2. 日志中 `headless=false`。
3. 用户可以观察到 ChatGPT 页面被打开、`#prompt-textarea` / ProseMirror composer 写入消息、右侧按钮从“启动语音功能”切换为 `send-button` / “发送提示”后被触发。
4. run 成功时返回最终 assistant 文本；如果页面侧出现 challenge 或异常，run 返回明确错误，不静默等待。

### TC-CWA-14: handoff 心跳失败会释放 active run

操作步骤：
1. 使用已登录的 `chatgpt_web` Runner 发起一个 `ask` 请求。
2. 在日志出现 `chatgpt_web send: waiting for f/conversation handoff` 后，关闭本次受控浏览器窗口或结束该浏览器进程。
3. 继续向同一个 IM 会话发送一条普通消息，使其进入 active-run 并发路径。
4. 观察 Bifrost 日志与 IM 回复。

预期结果：
1. 日志出现 `chatgpt_web handoff heartbeat failed` 或 `chatgpt_web handoff page heartbeat failed`。
2. run 错误包含 `browser_unavailable`，不会继续等待 `7200` 秒。
3. IM active session 被释放；后续消息不会永久停留在 `消息已收到，将在当前任务完成后处理`。
4. 如果队列中已有消息，日志应继续出现 `processing next queued message` 或返回明确失败状态，而不是无日志卡住。

### TC-CWA-15: mock IM 入站连续注入验证新建、追加与队列消费

操作步骤：
1. 使用默认数据目录启动 Bifrost，确保 `acc` provider 的 Agent runner 指向 `chatgpt_web`，并确认 auth status 为 `logged_in`。
2. 调用 mock inbound 接口重置会话：
   ```bash
   curl -sS -X POST http://127.0.0.1:9900/_bifrost/api/im-gateway/debug/mock-inbound \
     -H 'content-type: application/json' \
     -d '{"providerId":"acc","userId":"o9cq80wqfvOh3cJ69ywGqu9cGdqM@im.wechat","chatId":"o9cq80wqfvOh3cJ69ywGqu9cGdqM@im.wechat","text":"/clear"}'
   ```
3. 继续注入第一条普通消息，观察日志确认浏览器打开新建会话页；如需可视化诊断，可临时切换 headed。
4. 再注入第二条普通消息，观察日志确认浏览器进入上一步产生的 `/c/{conversationId}` 目标会话并追加消息。
5. 连续快速注入 3 条普通消息，观察 active run 期间的并发 inbound event 与后续队列消费。

预期结果：
1. mock inbound 事件日志包含 `mock inbound IM event injected`，并随后进入 `received inbound event from owner`。
2. 第一条普通消息的日志包含 `pageKind:"new_conversation"`、`visibleComposerCount:1`、`chatgpt_web send: target page stable`、`captured f/conversation POST` 和 `wait_final: found completed assistant message`。
3. 第二条普通消息的日志包含目标 `conversationId`，`pageKind:"conversation"`，URL 为同一个 `/c/{conversationId}`。
4. 连续快速注入时，后续消息先出现 `received concurrent inbound event during active agent chat`，当前 run 结束后继续出现新的 `chatgpt_web send: launching browser`，并逐条完成 handoff 与 final wait。
5. 如果 IM provider 回发失败，错误应明确体现供应商发送失败；这不应影响 runner 的输入、提交、等待和队列释放。

### TC-CWA-16: ChatGPT Web runner Session 记录可在 WebUI 查看

操作步骤：
1. 完成 TC-CWA-15 中任意一轮 mock inbound 消息注入。
2. 打开 WebUI，进入 AI -> Agent -> Sessions。
3. 找到对应 session key（形如 `{providerId}:{userId}`），打开详情。
4. 在 active detail 中查看 Messages；如果该 session 已清理或过期，打开 History detail 的 Event Timeline。
5. 将 runner 配置临时改为一个无法启动的 executable，或制造浏览器不可用错误，再注入一条普通消息，打开同一 session 详情查看异常记录；验证后恢复 runner 配置。

预期结果：
1. active detail 的 Messages 至少包含本轮用户输入和 ChatGPT Web assistant 最终输出。
2. History Event Timeline 包含 `session_start`、`user_message`、runner `tool_call`、`tool_result`、`assistant_message`。
3. `tool_result` 中包含 run id、adapter、status、duration、artifacts 路径和事件类型，且不包含 cookie、Authorization、`x-oai-is` 等敏感值。
4. runner 或浏览器失败时，active detail 显示 `Runner failed:*` assistant 消息；History 中对应 `tool_result.success=false`，并包含脱敏异常信息。

### TC-CWA-17: 生成图片原图通过 IM 通道独立发送

操作步骤：
1. 使用默认数据目录或临时数据目录启动 Bifrost，确保目标 provider 的 Agent runner 指向 `chatgpt_web`。
2. 如果使用 Weixin provider，确保 provider 已扫码授权并可发送 outbound 消息。
3. 通过 IM 通道发送：`帮我生成4张可爱的小猫咪`。
4. 等待 runner 完成，查看 IM 侧消息、Bifrost message history、run artifacts 和日志。
5. 如无真实 Weixin 外部环境，使用 Weixin mock 服务执行 provider 图片发送子路径，并用 ChatGPT Web 生成图片 tool 结果验证解析、下载、缓存与剥离逻辑。

预期结果：
1. ChatGPT Web 只有 `image_gen` tool 结果、尚未产生最终 assistant 文本时，runner 也能识别为已完成图片结果，不会持续等待到超时；如果 conversation/SSE 中仍存在 `in_progress` 消息，则不得把 partial 图片资产提前当作完成。
2. runner 优先从 ChatGPT conversation JSON 的 `image_asset_pointer: sediment://file_...` 解析 `fileId`，调用 `/backend-api/files/{fileId}/download` 获取签名原图 URL；如果该结构缺失，再从目标 conversation 页面解析 `/backend-api/estuary/content` URL。多图场景必须全部下载，少一张即返回明确 `image_download_failed`。
3. 原图默认缓存到数据目录附件存储：`$BIFROST_DATA_DIR/agent/im_gateway/attachments/chatgpt_web/{conversationId}/` 或默认 `~/.bifrost/agent/im_gateway/attachments/chatgpt_web/{conversationId}/`，并写入同名 metadata JSON。
4. 最终文本回复不包含本机图片路径、`file://` 路径或无法点击的 Markdown 本地图片语法；本地缓存路径仅作为内部投递引用。
5. IM 发送必须走各 provider 自己的图片模式：Weixin 先把原图字节加密后通过 `POST` 上传到 Weixin CDN 并发送 `image_item`；Feishu 使用 Feishu image upload 得到 `image_key` 后发送图片消息；不同 provider 不共享发送 payload。
6. Weixin 通道对 4 张生成图片分别发送 4 条 `image` 类型消息，发送内容为原图字节的 CDN 加密上传结果，而不是压缩后的文本摘要或 base64 文本。
7. Message History 中有最终文本 outbound 记录和 4 条 `msg_type=image` outbound 记录；失败时记录 provider 错误，active run 仍释放并继续处理队列。
8. 下载后的本地图片 Markdown 必须进入最终 IM 投递内容；当 runner 返回多个 `responses` 时，不能用下载前的 `all_texts` 覆盖已经追加图片路径的最终 `response`。

### TC-CWA-18: 失败时写入页面 DOM 与 conversation 响应 JSON

操作步骤：
1. 使用默认数据目录或临时数据目录启动 Bifrost，确保目标 runner 指向 `chatgpt_web`，并已有可用 ChatGPT Web 登录态。
2. 通过 Chat Gateway 或 IM mock inbound 触发一轮会失败的 `chatgpt_web` run，例如传入不支持的 operation，或传入已知失效的 `conversationId` 触发页面/会话校验失败。
3. 找到本轮 `$BIFROST_DATA_DIR/im_gateway/chat_runs/<run_id>/` 目录。
4. 检查 `failure_diagnostics.json`、`page_dom.json`、`page_dom.html`；如果本轮已知 conversation id，同时检查 `conversation_response.json` 或 `failure_diagnostics.json` 中 conversation 捕获失败原因。

预期结果：
1. 失败 run 不会只留下 `stderr` 或最终错误文本；`failure_diagnostics.json` 必须存在，并包含 operation、conversationId（如果可得）、原始 error、capturedAt 和 artifacts 状态列表。
2. 页面 DOM 诊断必须尽力写入 `page_dom.html` 与 `page_dom.json`；`page_dom.json` 包含 URL、title、readyState、composer 摘要、landmarks 摘要和截断 bodyText，完整 HTML 只在 `page_dom.html` 中。
3. 如果有 conversation id，runner 必须尝试保存 ChatGPT conversation API 原始响应到 `conversation_response.json`；如果 ChatGPT 返回 401/403/404、网络错误或超时，必须在 `failure_diagnostics.json` 中记录 `capture_failed` 或 `capture_timeout`，不能静默丢失。
4. failure diagnostics 不包含 cookie、Authorization、sentinel token、`x-oai-is`、`x-conduit-token` 等认证 header value。

## 最近执行记录

- 2026-05-14：代码级真实场景回归已执行 TC-CWA-12 的可自动验证子集：`cargo test -p bifrost-admin chatgpt_web -- --nocapture` 通过，覆盖 handoff SSE 解析、`browser_post_captured` 提交证据判定、conversation 最终消息摘要、transient 读错误分类与登录状态回退逻辑；未执行真实 ChatGPT Gateway 登录链路，原因是当前环境没有可复用 `auth_state.json`，此前同日记录显示 authenticated Chat Gateway adapter 路径仍被 `auth_required` 阻塞。
- 2026-05-14：本地真实 Edge UI 路径已执行，临时服务端口 `18886`，`BIFROST_DATA_DIR=/tmp/bifrost-chatgpt-web-edge-real`，启动命令为 `cargo run --bin bifrost -- start -p 18886 --unsafe-ssl --no-system-proxy`，CA 安装选择跳过，系统代理保持 disabled。通过 `chatgpt_web` runner 配置启动 Microsoft Edge，实际进程包含 `--remote-debugging-port=57215` 与独立 profile `/tmp/bifrost-chatgpt-web-edge-real/agent/im_gateway/chatgpt_web/browser_profile`。在该 Edge 页面中直接点击 `#prompt-textarea`，发送 `Please reply exactly EDGE_LOCAL_FIRST_OK and nothing else.`，页面返回 `EDGE_LOCAL_FIRST_OK`；随后在同一页面再次点击输入框追加 `Now reply exactly EDGE_LOCAL_APPEND_OK and nothing else.`，页面返回 `EDGE_LOCAL_APPEND_OK`，页面快照同时包含第一轮与追加轮结果。
- 2026-05-14：同一次验证中，authenticated Chat Gateway adapter 路径未完成。`Open Login Browser` 返回 `400 auth_required: no ChatGPT accounts/check auth headers captured from browser traffic`，`GET /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/status?runnerId=chatgpt-web-edge-real` 返回 `state:"logged_out"`、`loggedIn:false`、`cookieCount:0`，且未生成 `auth_state.json`。在该状态下直接调用 `/_bifrost/api/im-gateway/chat` 的 `chatgpt_web` adapter 返回 `400 auth_required: read auth state failed: No such file or directory (os error 2)`。因此本次只能证明真实 Edge UI 发起和追加消息链路通过，不能证明需要登录态的 Chat Gateway `chatgpt_web` adapter 已完整通过 TC-CWA-04/05/06。
- 2026-05-14：本地真实 Stop Login 路径已执行，临时服务端口 `18887`，`BIFROST_DATA_DIR=/tmp/bifrost-chatgpt-web-login-stop-test`，启动命令为 `cargo run --bin bifrost -- start -p 18887 --unsafe-ssl --no-system-proxy`。通过 Chat Gateway config API 创建 `chatgpt-web-stop` runner 后，请求 `POST /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/open` 保持等待，未被前端 30 秒超时中断；随后调用 `POST /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/stop`，原 open 请求返回 `400 auth_required: ChatGPT browser login was stopped by request`，`auth/status` 仍为 `logged_out` 且未生成 `auth_state.json`。清理时确认测试浏览器进程退出，并删除临时数据目录。
- 2026-05-15：执行代码级回归子集，`cargo test -p bifrost-admin chatgpt_web -- --nocapture` 通过 15 项，覆盖默认 headed 配置、TC-CWA-13 的可见诊断配置、TC-CWA-14 的 heartbeat 错误文案、TC-CWA-15 的目标页状态判断与 native 403 时复用已捕获 browser accounts/check proof。
- 2026-05-15：默认数据目录真实链路已执行 TC-CWA-15。启动命令为 `cargo run --bin bifrost -- start -p 9900 --host 127.0.0.1 --unsafe-ssl --no-system-proxy`，通过 `POST /_bifrost/api/im-gateway/debug/mock-inbound` 注入 `acc` provider 消息。`/clear` 后第一条普通消息进入 `https://chatgpt.com/`，日志确认 `pageKind:"new_conversation"`、`visibleComposerCount:1`、`captured f/conversation POST`、conversation id `6a0605b1-f63c-83ec-afcb-717f9077a239`、`wait_final: found completed assistant message`。第二条普通消息进入同一 `/c/6a0605b1-f63c-83ec-afcb-717f9077a239`，日志确认 `pageKind:"conversation"` 并完成 handoff/final wait。三连发队列压测中后两条先出现 `received concurrent inbound event during active agent chat`，随后逐条继续执行 `chatgpt_web send`、捕获 `f/conversation` POST 并完成 final wait。Weixin 回发出现 `ret=-2`，归类为 provider outbound failure，不影响 WebChatGPT runner 的输入、提交、等待与 active run 释放验证。
- 2026-05-15：headed 诊断模式真实链路连续执行 3 轮通过。每轮均使用 mock inbound 注入 `/clear` 后连续发送 `你好`、`你会做什么`、`写一个500字的故事`，日志确认 `headless=false`、新建会话进入 `pageKind:"new_conversation"`、后续消息进入目标 `/c/{conversationId}`、`captured f/conversation POST`、`wait_final: found completed assistant message`，队列提示后均返回 3 条最终 assistant 回复。该轮用于观察真实页面输入与发送动作。
- 2026-05-15：按最终默认设置恢复为 headless 后再次执行 3 轮回归通过，run id 为 `clean-1778782210`。三轮分别使用独立 mock user，每轮均新建会话并连续发送 `你好`、`你会做什么`、`写一个500字的故事`；脚本输出 `===== CLEAN HEADLESS ROUND 1 PASS =====`、`ROUND 2 PASS`、`ROUND 3 PASS` 与 `ALL_CLEAN_HEADLESS_ROUNDS_PASS run_id=clean-1778782210`。每轮均观察到 2 条排队提示、0 条 `runner_failed`，最终 3 条 assistant 回复依次返回，证明 active run 释放与队列消费稳定。fake Weixin user 回发出现 `ret=-3`，归类为供应商投递失败；message log 已记录最终内容，runner 输入、提交、等待和队列释放链路通过。
- 2026-05-15：代码级执行 TC-CWA-16 回归子集，`cargo test -p bifrost-admin im_event_loop_ -- --nocapture` 通过 4 项，覆盖 external runner 成功路径的 active session Messages 与持久化 JSONL 事件；同时覆盖 runner spawn 失败时 active detail 中 `Runner failed:*` 与 History `tool_result.success=false` 异常记录。
- 2026-05-15：默认数据目录 headless 真实链路执行 TC-CWA-16 通过，run id 为 `sessionrec4-1778783977`。连续 3 轮 mock inbound 注入 `/clear` 后发送 `你好`、`你会做什么`、`写一个500字的故事`；三轮均断言 active session detail 中存在 6 条 Messages（3 条 user 输入 + 3 条 assistant 最终输出），History Event Timeline 中存在 3 个 `user_message`、3 个 `assistant_message`、3 个 `chatgpt_web` `tool_call` 和 3 个 `success=true` 的 `tool_result`。脚本输出 `ROUND_1_PASS`、`ROUND_2_PASS`、`ROUND_3_PASS` 与 `ALL_SESSION_RECORD_HEADLESS_ROUNDS_PASS run_id=sessionrec4-1778783977`。
- 2026-05-15：代码级执行 TC-CWA-17 回归子集，`cargo test -p bifrost-admin agent_reply_ -- --nocapture` 通过 5 项，覆盖本地 Markdown 图片路径解析、非代码块提取、剥离最终文本中的本地路径、跳过远程图片/Feishu image_key、重复图片去重；`cargo test -p bifrost-admin weixin::weixin_tests::send_image_uploads_original_bytes_to_cdn_and_sends_image_item -- --nocapture` 通过，覆盖 Weixin 原图字节加密后以 `POST` 上传 CDN 并独立发送 `image_item.media`；`cargo test -p bifrost-admin weixin::weixin_tests::send_image_returns_config_error_for_unknown_image_key -- --nocapture` 通过，覆盖缺失图片 key 不发送空图片消息。
- 2026-05-15：默认数据目录真实 Weixin 链路执行 TC-CWA-17 通过。启动命令为 `cargo run --bin bifrost -- start -p 9900 --host 127.0.0.1 --unsafe-ssl --no-system-proxy`，真实 IM 提问 `帮我生成四张不同风格的小丑鱼的图片`；日志确认 headed 浏览器进入目标会话、composer 清理后捕获 `backend-api/f/conversation` POST，`wait_final: found completed generated image tool result image_count=4`，4 张原图分别缓存到 `~/.bifrost/agent/im_gateway/attachments/chatgpt_web/6a066cb1-8144-83ec-9e05-26583cc836bc/`，随后 Weixin provider 对 4 张图均完成 `outbound image uploaded to CDN` 与 `weixin image message sent`。
- 2026-05-15：补充回归发现仅依赖二次渲染 DOM 抓 estuary URL 会在懒加载/限流场景下漏图，随后补强为结构化 `image_asset_pointer -> fileId -> /backend-api/files/{fileId}/download` 优先路径；`cargo test -p bifrost-admin chatgpt_web -- --nocapture` 通过 17 项，覆盖 `fileId` 被带入生成图下载描述。
- 2026-05-15：补充真实链路观察发现仅凭 partial `image_asset_pointer` 作为完成信号会在 4 图生成过程中提前返回 2 张图片，随后移除 asset-only 完成 fallback；`image_asset_pointer/fileId` 仅用于在 `Generated images...` 完成清单出现后补齐原图下载地址，避免多图场景漏发。补充用例 `generated_image_assets_without_finished_path_list_do_not_complete` 覆盖 partial assets 不得结束等待。
- 2026-05-15：默认数据目录真实 Weixin 链路复测 TC-CWA-17 通过。真实 IM 提问 `再来四张中国元素风格的美女`，日志确认 headed 浏览器进入目标会话并捕获 `backend-api/f/conversation` POST；等待期间出现 partial estuary / asset 请求但 runner 未提前结束，直到 conversation detail 出现完整 `Generated images...` 清单后才记录 `wait_final: found completed generated image tool result image_count=4`。随后 4 张原图全部缓存到 `~/.bifrost/agent/im_gateway/attachments/chatgpt_web/6a0674d6-a73c-83ec-9727-5e047b46b184/`，Weixin provider 对 4 张图逐张完成 `outbound image uploaded to CDN` 与 `weixin image message sent`，确认 Weixin 使用自身 `POST CDN + image_item` 图片模式独立发送原图。
- 2026-05-15：代码级执行 TC-CWA-18 回归子集，`cargo test -p bifrost-admin chatgpt_web -- --nocapture` 通过 21 项，新增覆盖失败诊断 conversation id 提取、`failure_diagnostics.json` 写入 run 目录、完整 DOM HTML 与 `page_dom.json` 元数据分离。真实 ChatGPT 错误现场抓取将在当前默认数据目录服务重启后，通过后续真实 IM/Chat Gateway 失败样本持续观察。
- 2026-05-18：代码级回归执行 TC-CWA-12/17 子集，`cargo test -p bifrost-admin chatgpt_web --lib` 通过 46 项；覆盖 DOM fallback 移除短文本字符阈值，短回答只要非空就可投递；asset-only `image_asset_pointer` 在无 `in_progress` 时可作为最终图片结果，仍有 `in_progress` 时继续等待；图片下载后强制使用带本地图片 Markdown 的最终 `response` 进入 IM 投递路径，避免多 `responses` 覆盖图片。真实 ChatGPT / Weixin 外部链路本轮未执行，原因是本次修复点可通过代码级 runner 判定和 IM 投递数据结构回归覆盖，且不需要重新触发外部登录/图片生成。
- 2026-05-19：CLI 真实链路执行 TC-CWA-12/17 组合回归通过。使用当前 worktree 编译产物重启 9900 服务：`./target/debug/bifrost start -p 9900 --no-system-proxy --daemon`，runner id 为 `web`，登录态 `logged_in`。第一次命令 `./target/debug/bifrost -p 9900 agent run --runner web --session cli-chatgpt-ai-news-20260519-000234 --output-dir /tmp/bifrost-chatgpt-cli-20260519-000234 --json '查询今天的AI 主要新闻'` 成功，run id `1779120174041-9842be87-ddb9-42b2-9b03-6c66b88811cc`，返回今日 AI 新闻摘要。第二次命令在同 session 发送 `请把上一轮“今天的 AI 主要新闻”整理成一张中文信息图图片...只生成图片。` 成功，run id `1779120351182-c0920ab4-a79b-46e4-8ee4-1e49301a0e33`，最终 response 为 `已生成图片，正在发送原图。` 并包含本地 Markdown 图片路径；`conversation_final.json` 记录 `imageCount=1`，`generated_images.json` 记录 PNG 原图缓存到 `~/.bifrost/agent/im_gateway/attachments/chatgpt_web/6a0b3839-675c-83ec-acef-2b16bbf7fe81/chatgpt-web-image-1-5434cbebb34c8b47413e2bb4247d997718314c21.png`，文件校验为 `PNG image data, 1055 x 1491`，大小约 1.3MB。该轮先复现到旧逻辑会把 `ChatGPT 说：正在创建图片` 当作 `dom_fallback_outcome` 成功返回，随后修复为基于 streaming signal、图片生成 busy、stop button、composer/send button 恢复状态和内容状态共同判断输出完成；DOM 状态检查间隔从 3 秒降到 1 秒，减少最终输出发现延迟；多段文本投递保留页面自然批次：过程/思考按出现批次分别投递，最终结论作为最后一批。代码级 `cargo test -p bifrost-admin chatgpt_web --lib` 通过 47 项，`cargo clippy -p bifrost-admin --lib --all-features -- -D warnings` 通过。

## TC-CWA-19 ChatGPT Web Conversation Tab 长驻池（2026-05-16 新增）

### 用例背景

旧实现中 [send.rs](crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs) 在每次 send 完成后都会立刻调用 `cdp.close()` 与 `browser.close_target(tid)` 关闭隔离 tab，导致：

1. **同一对话连续多轮时 tab 被反复销毁重建**，每轮都要走完整的 navigate + cookie seed + composer 等待流程，CDP 抖动可能导致中间轮失败；
2. **ChatGPT 流式生成图片时 tab 被早早关掉**，DOM fallback 抓图失败，下游 IM 无法收到完整图片。

新版改为：

- 按 `(profile_dir, conversation_id)` 维护 `ConversationTab` 长驻池，容量 `CONVERSATION_TAB_POOL_SIZE = 16`；
- 入口若命中池子直接复用 CDP 与 target_id，跳过 navigate；不命中再走 `create_tab_with_cdp_retry`；
- 若服务重启导致内存池为空，但受控浏览器仍有同一个 `/c/{conversation_id}` 标签页，发送前先通过 `/json/list` 扫描现有 targets，命中后 attach CDP、重新注册入池并复用该 tab，避免重复打开相同 conversation；
- 成功时 `register_conversation_tab` 把 tab 入池；满了 LRU 淘汰，淘汰条目通过 `EvictedTab::shutdown` 关闭；
- 失败时仅关闭"刚新建未入池"的 tab；池中 tab 保持开启等待下一次复用；
- 浏览器进程被 kill / 重启时（mode mismatch 或不响应）调用 `clear_conversation_tabs_for_profile` 同步丢弃池中条目，避免悬挂引用。

### TC-CWA-19-01：单对话连续 3 轮真实流量复用 tab

**前置条件**：
- 服务从默认数据目录启动：`cargo run --bin bifrost -- start -p 8800 --host 127.0.0.1 --unsafe-ssl --no-system-proxy`
- 已配置 chatgpt-web runner，登录态有效，浏览器执行模式 `headed` 便于人工观察。

**操作步骤**：
1. 通过 CLI 在同一 sessionKey 下连续发起 3 轮：
   ```bash
   bifrost -H 127.0.0.1 -p 8800 agent run --runner web --session NOVEL_SINGLE_CONV \
     --json '{"prompt": "用一句话给我一个原创科幻短篇的设定。"}'
   bifrost -H 127.0.0.1 -p 8800 agent run --runner web --session NOVEL_SINGLE_CONV \
     --json '{"prompt": "在刚才的设定下，写出第一章 500 字。"}'
   bifrost -H 127.0.0.1 -p 8800 agent run --runner web --session NOVEL_SINGLE_CONV \
     --json '{"prompt": "在刚才设定下，给主角配一张赛博朋克头像。"}'
2. 观察服务日志中 `chatgpt_web send: ` 行。

**预期结果**：
- 第 1 轮日志包含 `chatgpt_web send: isolated tab created for this run`；
- 第 2、3 轮日志包含 `chatgpt_web send: reusing pooled conversation tab` 且 `target_id=` 与第 1 轮相同；
- 整个 3 轮过程中没有 `chatgpt_web send: closing isolated tab after run`（旧版每轮都会出现）；
- 三轮均成功返回 assistant 回复（第 3 轮 response 中包含 markdown 图片链接 `![...](...)`，且本地缓存路径下能找到对应原图）；
- headed 模式下，能在浏览器中观察到一个 `/c/<conversation_id>` 标签页，从第 1 轮起持续存在不被关闭。

### TC-CWA-19-02：图片生成完整后才返回（不提前关 tab）

**前置条件**：同 TC-CWA-19-01。

**操作步骤**：
1. 单轮请求生成 4 张图片：
   ```bash
   bifrost -H 127.0.0.1 -p 8800 agent run --runner web --session NOVEL_SINGLE_CONV \
     --json '{"prompt": "生成四张赛博朋克小丑鱼图片，风格各不相同。"}'
2. 观察日志 `wait_final: ` 行与最终 response 中的图片数量。

**预期结果**：
- 日志出现 `wait_final: found completed generated image tool result image_count=4`；
- response 中包含 4 个 `![ChatGPT 生成图片 N](本地路径)`；
- 4 个本地路径全部存在且非空（每个 PNG/JPEG > 1KB）；
- 整个等待过程未出现 `chatgpt_web send: closing isolated tab after run`。

### TC-CWA-19-03：池容量 LRU 淘汰

**前置条件**：服务以默认数据目录启动；当前 chatgpt_web 池为空（重启服务即可）。

**操作步骤**：
1. 用 17 个不同 sessionKey 连续发起 17 轮（每个 sessionKey 触发一个新对话，因此池中将有 17 条候选项）。
2. 观察日志 `chatgpt_web tab pool: ` 行。

**预期结果**：
- 前 16 轮成功 `register` 入池，无 `evicting LRU tab`；
- 第 17 轮触发 `chatgpt_web tab pool: evicting LRU tab to make room`，紧跟一条 `chatgpt_web tab pool: closing evicted tab`，对应被淘汰的是第 1 轮的 tab；
- 此后第 1 轮的 conversation_id 再次发起请求时，会命中 `chatgpt_web send: isolated tab created for this run`（不再命中池），其余的 conversation_id 仍命中 `reusing pooled conversation tab`。

### TC-CWA-19-04：浏览器执行模式切换清空池

**前置条件**：服务运行中，`chatgpt_web` 池中至少有 1 条 tab；准备从 headed 切换到 headless（或反之）。

**操作步骤**：
1. 通过 WebUI 编辑 chatgpt-web runner 把 `executionMode` 从 `headed` 改为 `headless`，保存。
2. 用同 sessionKey 再发起一轮请求。

**预期结果**：
- 日志出现 `chatgpt_web browser: execution mode changed, killing old browser to relaunch` 后紧跟一条来自 `clear_conversation_tabs_for_profile` 的清空动作；
- 新一轮请求走 `chatgpt_web send: isolated tab created for this run`（池被清空，无法复用）；
- 之后再发同一 conversation_id 时命中 `reusing pooled conversation tab`。

### TC-CWA-19-05：服务重启后复用浏览器中已有 conversation tab

**前置条件**：headed 模式下浏览器里已经打开目标 `/c/<conversationId>`，并且 Bifrost 服务刚重启，内存 `ConversationTab` 池为空；浏览器进程仍在，CDP `/json/list` 能列出该 tab。

**操作步骤**：
1. 使用同一 sessionKey 或显式 `params.conversationId` 向该 conversation 追加一轮短消息。
2. 观察服务日志中 `chatgpt_web browser: found existing conversation tab` 与 `chatgpt_web send: reusing existing browser conversation tab`。
3. 观察浏览器标签页数量。

**预期结果**：
- 本轮不新建同 conversation 的重复 tab；
- 命中的 target_id 被重新注册到 `ConversationTab` 池；
- 后续同 conversation 请求继续命中 `chatgpt_web send: reusing pooled conversation tab`。

### TC-CWA-19-06：单元测试回归

```bash
cargo test -p bifrost-admin --lib chatgpt_web -- --nocapture
```

**预期结果**：22 项均通过；clippy 全 workspace 零警告：

```bash
cargo clippy -p bifrost-admin --all-targets --all-features -- -D warnings
```

### TC-CWA-20：CLI JSON 与 IM 分批投递结构化输出

**前置条件**：已登录的 `chatgpt_web` runner 可用，服务运行在 `127.0.0.1:9900` 或测试端口；CLI 使用 `--json`。

**操作步骤**：
1. 调用：
   ```bash
   bifrost -H 127.0.0.1 -p 9900 agent run --runner web --session CWA_JSON_STREAM \
     --json '查询今天的AI主要新闻'
   ```
2. 观察 stdout 每一行都是单独 JSON 对象。
3. 对同一 session 发起图片整理请求：
   ```bash
   bifrost -H 127.0.0.1 -p 9900 agent run --runner web --session CWA_JSON_STREAM \
     --json '把上一轮内容整理成一张中文信息图图片'
   ```
4. 如果通过 IM 入站触发同类请求，观察 IM 消息。

**预期结果**：
- CLI stdout 不混入 spinner、pretty JSON 或非 JSON 文本；每个事件一行，可被 NDJSON 消费。
- 如果页面产生多条 assistant message，前面的过程批次输出为 `assistant_delta` 且 `raw.phase="thinking"`，最后答案输出为 `assistant_final` 且 `raw.phase="final"`。
- 如果 DOM 可识别到搜索/浏览等工具状态，CLI 输出独立的 `tool_finished` 事件且 `raw.phase="tool"`。
- IM 通道只按自然批次发送过程消息和最终答案，不发送工具调用事件。
- 多张本地图片会从正文卡片剥离，并按出现顺序逐张发送为 IM 图片消息；正文卡片不包含本地路径。

### TC-CWA-21：回归 - 图片生成状态文案不能提前结束

**前置条件**：已登录的 `chatgpt_web` runner 可用；同一 conversation 已经能生成图片。

**操作步骤**：
1. 发送明确图片数量的请求：
   ```bash
   bifrost -H 127.0.0.1 -p 9900 agent run --runner web --session CWA_IMAGE_STATUS \
     --json '生成4张独立的照片'
   ```
2. 在 ChatGPT 页面出现 `ChatGPT 说：正在打草稿`、`正在创建图片` 或类似状态时继续等待，不手动干预。
3. 检查 run 的 `result.json`、`generated_images.json` 和 IM 出站消息。

**预期结果**：
- run 不会以 `ChatGPT 说：正在打草稿`、`正在创建图片`、`Drafting`、`Thinking` 等状态文案作为成功最终 response。
- 最终态不由文本非空决定；必须观察到 stop/cancel 按钮消失、composer 可见且可用后，才重新提取并返回最新 assistant 消息。若 composer 仍有待发送文本，提交按钮必须可见；若 composer 已为空，ChatGPT 可能把发送位切回语音/提交控制，此时不能继续等待传统 send button 或 disabled 状态。
- 如果生成图渲染为最后一个 user turn 后的 `conversation-turn-*` section，且 section 内没有 `data-message-author-role="assistant"`，但包含 `ChatGPT 说：` 和 `estuary/content` 图片，也必须作为最终图片结果返回；如果该 section 暂时只有空壳 `ChatGPT 说：` 或 `最后微调一下...` 且图片数为 0，必须继续等待。
- 若 prompt 明确要求 4 张图片，首次 DOM 只渲染出少于 4 张时，adapter 继续滚动/监听网络请求补齐图片 URL。
- `generated_images.json.images.length` 与最终 response 图片 Markdown 数量不低于 ChatGPT 页面实际完成数量；明确请求 4 张且页面完成 4 张时，IM 图片消息按顺序发出 4 条。
- 如果最终仍未达到期望数量，run artifact 必须保留实际下载数量和来源 URL，便于定位是 ChatGPT 未完成、页面懒加载还是 IM 上传失败。

### TC-CWA-22：回归 - 长输入使用浏览器原生粘贴路径避免 CDP 卡死

**前置条件**：已登录的 `chatgpt_web` runner 可用；服务使用默认数据目录或包含可复用登录态的数据目录启动。

**操作步骤**：
1. 执行代码级阈值回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin chatgpt_web::tests::composer_text_injection --lib
   ```
2. 发送超过 120 字符的 prompt，或通过 ASR Daily Agent 的 `web` runner 触发包含 `AGENTS.md` 与变更文件内容的长 prompt。
3. 查看服务日志中的 `chatgpt_web send: injected composer text via native clipboard paste path`，并确认长文本没有走 `Input.insertText`。
4. 等待 `f/conversation` handoff、final wait 与 report / run result 完成。

**预期结果**：
- 120 个字符以内继续使用 `Input.insertText`；超过 120 个字符进入浏览器原生剪贴板 + 原生粘贴快捷键路径。
- 长输入路径不再调用 `Input.insertText`，避免 CDP 在大 prompt 时卡死。
- paste 路径先通过 CDP 写入当前浏览器上下文的 `navigator.clipboard`，再通过 macOS `Meta+V` 或其它平台 `Ctrl+V` 触发真实浏览器粘贴；不依赖系统剪贴板，也不需要用户处理浏览器授权弹窗。
- 粘贴后不采样 composer 文本；ChatGPT 可能把长文本上传成文件，输入框为空也不能判定为失败。
- 粘贴后持续轮询发送按钮状态，按钮变为可发送后立即继续；短文本不被固定 sleep 拖慢，长文档上传/解析较慢时在最大上限内继续等待。
- 发送按钮可用性检查、点击发送、handoff 捕获和最终结果等待仍然完成。

### TC-CWA-23：回归 - 超长输入文件化后仍能等待发送按钮并提交

**前置条件**：已登录的 `chatgpt_web` runner 可用；服务使用默认数据目录或包含可复用登录态的数据目录启动。

**操作步骤**：
1. 执行 ChatGPT Web targeted 单测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin 'chatgpt_web::' --lib
   ```
2. 使用 ASR Daily Agent 默认任务强制触发多份大 Markdown：
   ```bash
   curl -sS -X POST 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/run?force=true'
   ```
3. 检查每个 run 目录的 `prompt.md`、`result.json` 和 ChatGPT 页面状态。

**预期结果**：
- 超过 120 字符仍不使用 CDP `Input.insertText`。
- 超过超长阈值的输入不再优先派发 synthetic paste，也不使用页面内 `execCommand("insertText")` 或 DOM fallback 注入正文。
- 超长正文通过浏览器原生剪贴板 + 原生粘贴快捷键进入 ChatGPT；当页面把正文转换为粘贴文本文件/附件时，adapter 不读取 composer 正文，只通过轮询等待发送按钮可用。
- ASR Daily Agent 的 `web` runner 按单个 daily 文件顺序发送 prompt；每个 run 成功返回并写 report。

### TC-CWA-24：回归 - 过期登录态不能把登录页误判为已登录

**前置条件**：存在 ChatGPT Web runner 的历史 `auth_state.json`，其中 `capturedAuthIdentity.expiresAt` 已过期，且历史 `capturedAccountCheck` 可能仍记录 `status=200/loggedIn=true`；native `accounts/check` 当前返回 401/403 或页面已经跳到 ChatGPT 登录页。

**操作步骤**：
1. 执行登录态判定回归单测：
   ```bash
   cargo test -p bifrost-admin chatgpt_web --lib
   ```
2. 重点确认测试输出包含并通过：
   - `auth_status_rejects_expired_authorization_identity`
   - `auth_status_rejects_stale_browser_account_check_when_native_forbidden`
   - `auth_status_accepts_browser_account_check_when_native_probe_is_forbidden`
3. 查看最近一次真实失败样本的 `auth_probe.json`，若出现 `accountStatus=403`，修复后同类状态不能再同时出现 `loggedIn=true`；应进入 `auth_required`，并在 message 中说明 Authorization 过期或 browser proof 已陈旧。
4. 通过 IM 通道触发 ChatGPT Web runner 时，如页面是登录页，应返回登录失效/需要重新登录的可理解反馈，不应继续执行 composer 发送逻辑，也不应把登录页截图作为普通 runner failure 发送给用户。

**预期结果**：
- 过期 `expiresAt` 使 `identityComplete=false`、`loggedIn=false`、`state=auth_required`。
- 超过短期兜底窗口的 `capturedAccountCheck` 不能覆盖当前 native 401/403。
- 刚完成登录时的短期 browser proof fallback 仍保留，避免 native probe 被本机传输问题误杀。
- IM 用户看到的是登录失效/需要重新登录，而不是 `browser_ui: composer not ready` 或登录页截图。

### TC-CWA-25：服务启动时 ChatGPT Web Runner 登录态预检

**前置条件**：准备隔离 `BIFROST_DATA_DIR`，在 `admin/im_gateway_external_cli_agent.json` 中配置自定义 runner `web`，其 `adapter` 为 `chatgpt_web`，且该 runner 的 `auth.statePath` 不存在或登录态失效。

**操作步骤**：
1. 执行代码级 runner 收集和 dry-run 登录提示回归：
   ```bash
   cargo test -p bifrost-admin chatgpt_web_startup_auth --lib
   ```
2. 执行隔离服务启动 E2E：
   ```bash
   bash e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh
   ```
3. 真实人工场景中，去掉 E2E 的 `BIFROST_CHATGPT_WEB_STARTUP_AUTH_DRY_RUN=1`，用同样配置启动：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-chatgpt-web-startup-live \
     target/debug/bifrost start -p 18947 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
4. 观察启动日志和浏览器窗口。

**预期结果**：
- 服务启动后立即在后台扫描 Runners 中所有 `adapter=chatgpt_web` 的 runner，不依赖 channel 是否已启用。
- 已登录 runner 只记录 `chatgpt_web startup auth: login ready`。
- 未登录或登录失效 runner 复用强 `auth_status` 判定，自动打开 ChatGPT 登录浏览器；用户完成登录后记录 ready。
- E2E dry-run 不弹真实浏览器，但日志必须出现 `would open login browser` 和 `chatgpt_web startup auth: login still required`，auth status 仍返回 `loggedOut/loggedIn=false`，证明启动预检确实执行到缺登录分支。
- `bifrost start` 前台和 daemon 模式都会触发同一预检方法，主服务 HTTP ready 不被等待用户登录阻塞。

### TC-CWA-26：回归 - ChatGPT behavior-btn 图片与 ZIP 自动下载

**前置条件**：已登录的 `chatgpt_web` runner 可用；准备一个会生成多张信息卡并提供 ZIP 打包下载的 ChatGPT Web 对话。

**操作步骤**：
1. 通过 ChatGPT Web Runner 发送会生成多张 PNG 信息卡并要求 ZIP 打包下载的 prompt，或打开历史 run `1779784377984-e0206a71-0333-4ac6-8dd5-9dd090ce24ab` 对应的 ChatGPT 对话 `6a155ad4-1d74-83ec-bc1d-bc1faccf276b`。
2. 使用默认 Runner CDP 检查最终 assistant turn 中的 DOM，确认图片与 ZIP 下载项以 `button.behavior-btn` / `entity-underline` 形式存在，而不是 `<img>` 或 `<a href>`。
3. 使用默认 Runner CDP 点击 `大模型竞争格局` 等图片行为按钮，确认图片 dialog 出现原图 `<img>`，且可解析到 `backend-api/estuary/content?...fn=*.png`。
4. 使用默认 Runner CDP 点击 `5 张图片 ZIP` 行为按钮，确认浏览器触发下载完成事件且下载文件非空。
5. 等待 run 完成后检查 `conversation_final.json`、`behavior_artifacts.json` 和 `result.json`。
6. 通过 Agent Chat 或 IM 出站消息检查最终投递文本。

**预期结果**：
- DOM fallback 识别最终 assistant turn 内的行为按钮，不把复制、分享、来源、模型切换或粘贴的 markdown 文件 pill 当作生成产物。
- `conversation_final.json` 包含 `artifactCount >= 1` 和 `artifacts[]`，图片按钮为 `kind=image`，`5 张图片 ZIP` 或同类打包下载按钮为 `kind=archive`。
- `result.json.raw.artifacts` 与 final summary 中的 artifacts 保持一致。
- Runner 自动点击 `kind=image` 图片行为按钮，解析 dialog / `estuary/content` 原图 URL，`behavior_artifacts.json` 和 `result.json.raw.downloadedArtifacts` 记录本地 PNG 路径、文件名、字节数、来源 URL 和 content type。
- Runner 自动点击 `kind=archive` / ZIP / 压缩包行为按钮，`behavior_artifacts.json` 和 `result.json.raw.downloadedArtifacts` 记录本地归档路径、文件名、字节数、来源 URL 和下载 guid。
- 最终投递文本在原回答后追加本地 Markdown 图片 `![图片标题](本地路径)`，Agent Chat 能渲染图片；IM 通道由本地 Markdown 图片解析层拆出并单独发图，同时 ZIP 以“已自动下载 ChatGPT Web 附件”本地链接呈现；不能只剩 `打包下载：`。
- 任一图片或 ZIP 下载失败都不能导致整个 Runner 失败；已成功下载的图片/ZIP 继续输出，失败项在最终回复中以“部分 ChatGPT Web 附件未能自动下载”汇总，并在日志中记录 `kind`、`label`、错误原因。
- 既有 `<img>` / `generatedImages` 下载链路仍按原逻辑缓存原图并按 IM 图片模式发送。

### TC-CWA-27：回归 - 授权流不进入 Conversation 重导航循环

**前置条件**：存在 ChatGPT Web runner 的真实失败样本或等价页面状态，其中 `auth_probe.json` 仍显示 `loggedIn=true/accountStatus=403`，但 `failure_diagnostics.json` 的页面 URL 为 `https://chatgpt.com/auth/login`，正文包含“登录即可开始聊天”；或浏览器跳转到 Google / Apple / OpenAI 等第三方授权页面。

**操作步骤**：
1. 执行代码级登录页终止态回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin auth_flow_pages_are_not_retryable_conversation_navigation --lib -- --nocapture
   ```
2. 执行 ChatGPT Web adapter 定向回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin chatgpt_web --lib
   ```
3. 对照真实失败样本：
   - `/Users/eden/.bifrost/im_gateway/runs/1780447535923-fb3febe2-74d3-4db4-ae80-f66b21f85523/auth_probe.json`
   - `/Users/eden/.bifrost/im_gateway/runs/1780447535923-fb3febe2-74d3-4db4-ae80-f66b21f85523/failure_diagnostics.json`
4. 如果后续用真实 runner 复测，应先停止当前卡住的 run 或换隔离数据目录，避免继续污染默认 ChatGPT 会话。

**预期结果**：
- ChatGPT 登录页、OpenAI 授权页、Google / Apple 等第三方授权页都被识别为授权流页面。
- 授权流期间 runner 不再把页面误判为 stale conversation、429 或普通 `composer not ready`，也不会继续反复 `Page.navigate` 到原 conversation。
- 如果用户在授权页面完成登录，runner 等待授权流结束；若页面没有自动回到原 conversation，才主动跳回目标 URL 一次并继续任务。
- 如果授权流一直未完成，用户可见反馈应指向“需要重新登录 ChatGPT Web”，而不是长期刷新浏览器页面。

## 真实执行记录

- 2026-06-03：执行 TC-CWA-27 通过。现场确认默认任务 `76612de33e9740bc92440ce64a98a4cb` 的 Daily Agent 当前 run `1780447855777-aa643349-f391-4050-8e48-fd166b2786e7` 使用 `runner=web` 且仍为 running；前一条真实失败样本 `1780447535923-fb3febe2-74d3-4db4-ae80-f66b21f85523` 中 `auth_probe.json` 为 `loggedIn=true/accountStatus=403`，但 `failure_diagnostics.json` 的实际页面为 `https://chatgpt.com/auth/login`，正文包含“登录即可开始聊天”，错误为 `browser_ui: composer not ready`。修复后执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin auth_flow_pages_are_not_retryable_conversation_navigation --lib -- --nocapture` 通过，验证 ChatGPT 登录页与 Google OAuth 页面都被识别为授权流页面，且不会进入 conversation 重导航分支。
- 2026-05-26：执行 TC-CWA-26 代码级与默认 Runner CDP 真实点击回归通过。使用用户提供的真实 ChatGPT DOM 结构确认问题形态：最终 assistant message `4cd623b5-3870-49a3-bb64-ffd3b06c769b` 中 5 张信息卡和 `5 张图片 ZIP` 都是 `button.behavior-btn` / `entity-underline`，不是 `<img>` 或 `<a href>`；历史 run `1779784377984-e0206a71-0333-4ac6-8dd5-9dd090ce24ab` 的 `conversation_final.json` 为 `imageCount=0` 且最终 response 停在 `打包下载：`。默认 Runner CDP 使用已有登录态打开 `https://chatgpt.com/c/6a155ad4-1d74-83ec-bc1d-bc1faccf276b`，点击 `大模型竞争格局` 后 dialog 出现 `alt=大模型竞争格局` 的原图 `<img>`，图片 URL 为 `backend-api/estuary/content?id=file_00000000c37c7230a857de6a3fd1c397&fn=ai_infographic_01_overview.png`，同时页面预取其余 4 张 `ai_infographic_02_timeline.png`、`ai_infographic_03_anthropic_profit.png`、`ai_infographic_04_llm_intelligence.png`、`ai_infographic_05_today_news.png`；点击 `5 张图片 ZIP` 后收到 `downloadWillBegin` / `downloadProgress completed`，下载 URL 为 `backend-api/estuary/content?id=file_000000007e9871fda9b7c4b602594f18&fn=ai_infographics_5_images.zip`，文件 `/tmp/bifrost-chatgpt-web-live-download-20260526180404/ai_infographics_5_images.zip` 大小 `1499429` 字节。修复后 targeted 单测确认 DOM 提取源码扫描 `button.behavior-btn`、输出 `artifactCount` / `chatgpt_behavior_button`，Runner 会下载 behavior 图片为本地 PNG Markdown、写入 `downloadedArtifacts`，并把 ZIP 本地链接追加到最终投递文本。
- 2026-05-26：执行 TC-CWA-25 通过。代码级命令 `cargo test -p bifrost-admin chatgpt_web_startup_auth --lib` 通过 2 项，确认服务层会收集所有 `adapter=chatgpt_web` 的 runner（包括未启用但可被显式选择的自定义 runner），且 dry-run 模式下缺失登录态会返回“would open login browser”。E2E 命令 `bash e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh` 使用隔离数据目录和自定义 runner `web` 启动 Bifrost，设置 `BIFROST_CHATGPT_WEB_STARTUP_AUTH_DRY_RUN=1` 避免 CI 弹真实浏览器；日志出现 `would open login browser` 与 `chatgpt_web startup auth: login still required`，随后 Auth Status API 返回 `state=logged_out`、`loggedIn=false`、`statePath` 指向测试配置的 `startup-auth-e2e.json`，证明启动预检已在首次使用 runner 前执行。
- 2026-05-25：执行 TC-CWA-24 通过。先用最新真实失败样本确认问题形态：微信入站 `你好` 后，IM 出站返回 `Runner failed: browser_ui: composer not ready`，正文包含 ChatGPT 登录页文案，并额外发送 `Diagnostic Screenshot`；对应历史 `auth_state.json` 的 `capturedAuthIdentity.expiresAt=2026-05-24T16:47:44+00:00` 已过期、`capturedAccountCheck.capturedAt=2026-05-14T17:22:53.680183+00:00` 陈旧，而失败 run 的 `auth_probe.json` 仍为 `accountStatus=403/loggedIn=true/state=logged_in`。修复后执行 `cargo test -p bifrost-admin chatgpt_web --lib`，62 项通过，包含 `auth_status_rejects_expired_authorization_identity`、`auth_status_rejects_stale_browser_account_check_when_native_forbidden`、`browser_account_check_proof_rejects_far_future_timestamp` 和 `auth_status_accepts_browser_account_check_when_native_probe_is_forbidden`。随后启动隔离数据目录临时服务：`BIFROST_DATA_DIR=$(mktemp -d) target/debug/bifrost start -p 18898 --unsafe-ssl --skip-cert-check --no-system-proxy --daemon`，注入同一过期 `auth_state.json` 与 mock `accounts/check=403`，调用 `GET /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/status?runnerId=web`，返回 `state=auth_required`、`loggedIn=false`、`identityComplete=false`、`accountCheckOk=false`、`accountStatus=403`，message 同时包含 `captured browser accounts/check proof is stale` 与 `authorization token expired at 2026-05-24 16:47:44 UTC`。测试结束已停止临时 Bifrost 进程并删除临时数据目录。
- 2026-05-26：执行 TC-CWA-22 / TC-CWA-23 原生剪贴板回归通过。先用当前源码 `cargo build --bin bifrost` 编译，再重启正式默认目录服务 `BIFROST_DATA_DIR=$HOME/.bifrost ./target/debug/bifrost start -p 9900 --host 0.0.0.0 --no-system-proxy --daemon`；随后通过 CLI Runner `./target/debug/bifrost -p 9900 agent run --runner web --session chatgpt-web-native-clipboard-no-sample-20260526 --json "$(cat /Users/eden/.bifrost/asr/data/text/76612de33e9740bc92440ce64a98a4cb/.daily/2026-05-19.md)"` 发送 457840 字节 prompt。run `1779727870753-c7feafc8-3173-43c8-8462-014e2b7409b1` 成功，日志 `/tmp/bifrost-chatgpt-web-no-sample-20260526005110.log` 返回 `收到文件《粘贴的文本 (1)(3).txt》`，证明 ChatGPT 将长文本文件化后 adapter 没有再采样 composer 文本，而是等待发送按钮可用并完成点击、handoff 和最终回复。代码级 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin composer_text_injection --lib` 通过 3 项，确认 120 字符以内走 `Input.insertText`，121 字符及以上走 `NativeClipboardPaste`，中文按字符数判断；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin native_clipboard_paste_uses_platform_modifier --lib` 通过 1 项，确认 macOS 使用 Meta 粘贴 modifier。
- 2026-05-26：补充执行 TC-CWA-22 浏览器剪贴板真实回归通过。使用已登录的隔离临时目录 `/private/tmp/bifrost-clipboard-auth-live-data.lFIcBF` 和独立 Edge profile `/tmp/bifrost-clipboard-auth-live-profile.0k7hs4`，先用当前源码 `cargo build --bin bifrost` 编译，再启动临时服务：`BIFROST_DATA_DIR=/private/tmp/bifrost-clipboard-auth-live-data.lFIcBF ./target/debug/bifrost start -p 19911 --host 127.0.0.1 --unsafe-ssl --skip-cert-check --no-system-proxy --daemon`。执行 `./target/debug/bifrost -p 19911 agent run --runner web --session chatgpt-web-browser-clipboard-smoke-20260526 --json '请只回复 OK_BROWSER_CLIPBOARD_SMOKE...abcdefghijklmnopqrstuvwxyz0123456789'`，该 prompt 超过 120 字符，触发浏览器 `navigator.clipboard.writeText` + `Meta+V` 原生粘贴路径。run `1779733277111-8e92518d-5606-4854-a3ec-7e8d63813796` 成功返回 `OK_BROWSER_CLIPBOARD_SMOKE`，conversationId 为 `6a149332-9e4c-83ec-93a4-2b3d2da12853`；验证 headless Edge 不依赖系统剪贴板，也不需要用户授权弹窗，粘贴后可持续等待发送按钮并完成发送。
- 2026-05-19：执行 TC-CWA-19-05 服务重启后复用已有 conversation tab 通过。测试 session 为 `cli-chatgpt-tab-reuse-restart-20260519`。第一轮命令通过 9900 发送 `请只回复 TAB_REUSE_FIRST_OK，不要解释。`，run id `1779124077160-9e8739f3-3cb2-4846-847b-01cf6395624d`，返回 `TAB_REUSE_FIRST_OK`，conversationId 为 `6a0b4778-3224-83ec-8086-bdfcbd525047`。第一轮结束后 CDP `/json/list` 中该 conversation 的 tab 数为 `count=1`，target id 为 `58CB389DB8D5665FB1D017B8E8412025`。随后执行 `./target/debug/bifrost -p 9900 stop` 停止服务，再用 `./target/debug/bifrost start -p 9900 --no-system-proxy --daemon` 重启服务，PID `21351`，System Proxy 保持 disabled；重启后、第二轮发送前，CDP `/json/list` 中同一 conversation 仍为 `count=1` 且 target id 仍为 `58CB389DB8D5665FB1D017B8E8412025`。第二轮同 session 发送 `请只回复 TAB_REUSE_SECOND_OK，不要解释。`，run id `1779124121796-044389de-d24b-4933-975c-38111f3d2f50`，返回 `TAB_REUSE_SECOND_OK`，conversationId 仍为 `6a0b4778-3224-83ec-8086-bdfcbd525047`。第二轮结束后 CDP `/json/list` 中该 conversation 仍为 `count=1`，target id 仍为 `58CB389DB8D5665FB1D017B8E8412025`，确认服务重启后复用原有 conversation tab，没有新开重复 tab。
- 2026-05-20：执行 TC-CWA-22 通过。代码级命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin chatgpt_web::tests::composer_text_injection --lib` 通过 2 项，确认 120 字符以内走 `Input.insertText`、121 字符及以上走 paste，并按字符数而非字节数判断中文。默认目录真实 ASR Daily Agent live E2E 使用当前源码服务 `18896` 触发 `runner=web`，日志显示 `injected composer text via paste path`、`expectedLength=767`、`pasteDispatched=true`、`pasteDefaultPrevented=true`、`ok=true`，随后成功点击发送、捕获 `f/conversation`、等待 ChatGPT 最终输出并写入 `/Users/eden/.bifrost/asr/data/text/159f0fa758334ab1b3f1191c7921b322/daily/report/2026-05-20-report.md`；报告包含 `ASR Daily Agent Live Runner Result`、`runner validation passed` 和 marker `ASR_LIVE_RUNNER_web_1779212771614`。同一 live E2E 还验证 IM send 使用 `mode=full_report`，outbound `sentPreview` 以 `# ASR Daily Agent Live Runner Result` 开头，确认发送的是报告原文而不是任务摘要。
- 2026-05-20：执行 TC-CWA-23 通过。默认目录任务 `76612de33e9740bc92440ce64a98a4cb` 使用 `runner=web` 强制运行后生成四个 ChatGPT Web run：`1779216503662-*`、`1779216632038-*`、`1779216764881-*`、`1779216872746-*`。四个 prompt 分别只包含 `2026-05-14.md`、`2026-05-15.md`、`2026-05-16.md`、`2026-05-17.md`，大小约 217KB、262KB、267KB、149KB；四个 `result.json.status=succeeded`，没有复现 CDP 输入卡死，也没有把正文变成 ChatGPT paste attachment；最终写入四个 report。代码级 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin 'chatgpt_web::' --lib` 55 项通过。

## 清理步骤

1. 停止测试 Bifrost 服务。
2. 删除临时数据目录：
   ```bash
   rm -rf /tmp/bifrost-chatgpt-web-adapter-test
   ```
3. 如需保留登录态用于后续验证，不执行第 2 步；登录态仅保留在本机测试数据目录。
