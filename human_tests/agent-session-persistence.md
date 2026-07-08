# Agent Session 持久化测试

## 功能模块说明

验证 Agent Session 的 JSONL 持久化功能：
- 通过飞书机器人或 `/agent/chat` API 发送消息后，session 事件自动写入 `~/.bifrost/agent/sessions/` 目录的 JSONL 文件
- JSONL 文件包含完整执行过程（session_start、user_message、tool_call、tool_result、assistant_message、compaction、session_end 等）
- WebUI 可查看 session 历史文件列表、查看详细事件时间线、删除 session 文件
- WebUI Sessions 列表可通过点击 title 或整行进入 session 详情，不再依赖单独的查看 icon 按钮
- WebUI Session 详情页默认展示 Messages Tab，Settings Tab 承载 session metadata、AGENTS.md 和 Skills，长消息/事件列表在内容区域内真实滚动
- 跨 turn 复用同一 recorder（同一 session 多次对话写入同一文件）
- 受 `ephemeral` 和 `history.persistence` 配置控制

## 前置条件

1. 编译并启动 Bifrost 服务（临时数据目录）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保 Agent 功能已启用（`enabled: true`）
3. 确保 `ephemeral` 为 `false`，`history.persistence` 为 `SaveAll`（默认即可）
4. 清空 `~/.bifrost/agent/sessions/` 目录以便观察新生成文件

## 测试用例

### TC-ASP-01：通过 /agent/chat API 发送消息后生成 JSONL 文件

**操作步骤**：
1. 调用 `POST http://localhost:8800/_bifrost/agent/chat`，body: `{"message": "hello, what is 1+1?", "session_key": "test-persist-01"}`
2. 等待响应返回
3. 检查 `~/.bifrost/agent/sessions/` 目录是否生成了包含 `test-persist-01` 的 JSONL 文件

**预期结果**：
- API 返回 `{ "success": true, "response": "..." }`
- `~/.bifrost/agent/sessions/` 目录下生成 `session-test-persist-01-*.jsonl` 文件

### TC-ASP-02：JSONL 文件包含 session_start 事件

**操作步骤**：
1. 读取 TC-ASP-01 生成的 JSONL 文件
2. 解析第一行 JSON

**预期结果**：
- 第一行的 `event_type` 为 `session_start`
- `session_key` 为 `test-persist-01`
- `content` 包含 `model` 和 `provider` 字段

### TC-ASP-03：JSONL 文件包含 user_message 事件

**操作步骤**：
1. 读取 TC-ASP-01 生成的 JSONL 文件
2. 查找 `event_type` 为 `user_message` 的行

**预期结果**：
- 存在 `user_message` 事件
- `content` 包含用户发送的消息文本 `"hello, what is 1+1?"`

### TC-ASP-04：JSONL 文件包含 assistant_message 事件

**操作步骤**：
1. 读取 TC-ASP-01 生成的 JSONL 文件
2. 查找 `event_type` 为 `assistant_message` 的行

**预期结果**：
- 存在 `assistant_message` 事件
- `content` 包含 Agent 的回复文本

### TC-ASP-05：跨 turn 复用同一 JSONL 文件

**操作步骤**：
1. 使用相同 session_key 再次调用 `POST /agent/chat`，body: `{"message": "and what is 2+2?", "session_key": "test-persist-01"}`
2. 检查 `~/.bifrost/agent/sessions/` 目录

**预期结果**：
- 不会生成新的 JSONL 文件（仍然只有一个 `session-test-persist-01-*.jsonl`）
- 文件内容追加了第二轮对话的 user_message 和 assistant_message 事件

### TC-ASP-06：GET /agent/sessions/history 列表 API

**操作步骤**：
1. 调用 `GET http://localhost:8800/_bifrost/agent/sessions/history`

**预期结果**：
- 返回 `{ "history": [...], "total": N }`
- history 数组中包含至少一个条目，含 `path`、`filename`、`session_key`、`timestamp` 字段
- `session_key` 为 `test-persist-01`

### TC-ASP-07：GET /agent/sessions/history/{path} 详情 API 返回完整事件

**操作步骤**：
1. 从 TC-ASP-06 的结果中取出 `path` 字段（URL encode）
2. 调用 `GET http://localhost:8800/_bifrost/agent/sessions/history/{encoded_path}`

**预期结果**：
- 返回 `{ "events": [...], "count": N }`
- events 数组包含 `session_start`、`user_message`、`assistant_message` 等事件类型
- 每个事件包含 `timestamp`、`event_type`、`session_key`、`content` 字段

### TC-ASP-08：DELETE /agent/sessions/history/{path} 删除 session 文件

**操作步骤**：
1. 从 TC-ASP-06 的结果中取出 `path` 字段
2. 调用 `DELETE http://localhost:8800/_bifrost/agent/sessions/history/{encoded_path}`
3. 确认文件已删除

**预期结果**：
- 返回 `{ "ok": true }`
- 再次调用 GET 列表 API，该 session 不再出现

### TC-ASP-09：WebUI Session History 列表展示

**操作步骤**：
1. 先通过 API 再创建一个 session 确保有数据
2. 在浏览器中打开 `http://localhost:8800/_bifrost/` 进入 Settings > Agent Tab
3. 找到 Session History 区域

**预期结果**：
- 表格展示持久化的 session 文件列表
- 每行显示 session key、时间戳等信息
- 点击 session title 或当前行可进入详情
- 列表不再展示单独的"查看"小按钮，仅保留删除操作

### TC-ASP-10：WebUI 查看 Session 详情事件时间线

**操作步骤**：
1. 在 Session History 列表中点击某条 session 的 title 或当前行
2. 进入 Session 详情页，保持 URL 中的 `session`、`view`、`historyPath` 参数

**预期结果**：
- Session 详情页默认在 Messages Tab 展示事件时间线
- 不同事件类型有不同的视觉样式（颜色、图标）
- session_start 显示 model/provider 信息
- user_message 显示用户消息内容
- assistant_message 显示 Agent 回复内容
- tool_call 显示工具名和参数（如有）
- tool_result 显示执行结果和成功/失败状态（如有）

### TC-ASP-11：WebUI 删除 Session 文件

**操作步骤**：
1. 在 Session History 列表中点击某条 session 的"删除"按钮
2. 确认删除

**预期结果**：
- session 从列表中消失
- 对应的 JSONL 文件被删除

### TC-ASP-12：暗色主题兼容性

**操作步骤**：
1. 切换到暗色主题
2. 查看 Session History 列表和详情模态框

**预期结果**：
- 所有文本、卡片、标签在暗色主题下清晰可辨
- 事件卡片颜色适配暗色主题

### TC-ASP-13：恢复持久化 session 后继续 tool loop 回归

**操作步骤**：
1. 执行持久化恢复回归：
   ```bash
   cargo run -p bifrost-e2e -- --test im_gateway_agent_tool_history_resume_regression --jobs 1 --timeout 240
   ```
2. 该用例会使用临时目录创建 `ConversationRecorder`，先触发一次 `list_directory` 工具调用并写入 JSONL。
3. 用 `load_conversation()` 从 JSONL 恢复 session history。
4. 恢复后再次发起需要工具调用的 turn。
5. 检查输出中 mock Chat Completions 服务没有拒绝任何请求。

**预期结果**：
- E2E 输出 `PASS im_gateway_agent_tool_history_resume_regression`
- 恢复出的 history 包含合法的 `assistant(tool_calls)` + `tool` 消息对
- 第二轮恢复后工具调用成功执行
- 不出现 `messages with role 'tool' must be a response to a preceeding message with 'tool_calls'`
- 不出现 orphan `tool` message 或不完整 tool-call suffix

### TC-ASP-14：WebUI Session 详情 Messages/Settings Tab 与右侧内容滚动回归

**操作步骤**：
1. 使用临时数据目录启动 Bifrost，端口为 `$MAIN_PORT`，启动参数必须包含 `--no-system-proxy`。
2. 准备包含 30 条以上事件的 session history JSONL 文件，或用 Playwright mock `GET /_bifrost/api/im-gateway/agent/sessions/history/{path}` 返回 30 条以上 `user_message`、`assistant_message`、`tool_call` 事件。
3. 在浏览器中打开：
   ```text
   http://localhost:$MAIN_PORT/_bifrost/ai?aiSection=agent-sessions&agentSection=sessions&session=test-scroll&view=history&historyPath=<url-encoded-path>
   ```
4. 确认详情页默认选中 `Messages` Tab，右侧显示 `Event Timeline`。
5. 在 Messages 内容区域向下滚动到最底部，确认最后一条事件可见。
6. 点击 `Settings` Tab。

**预期结果**：
- 详情页默认展示 `Messages` Tab，而不是把 Messages 与 Settings 从上到下平铺。
- Messages 内容区域自身可滚动，`scrollHeight > clientHeight`，滚动后能看到最后一条事件；页面外层不需要靠整体下滚才能看完内容。
- `Settings` Tab 展示 Session Info、AGENTS.md Instructions 和 Skills。
- 亮色和暗色主题下两个 Tab、事件卡片和 Settings 内容均可读。

### TC-ASP-15：WebUI Sessions 列表 title/整行点击进入详情回归

**操作步骤**：
1. 使用临时数据目录启动 Bifrost，端口为 `$MAIN_PORT`，启动参数必须包含 `--no-system-proxy`。
2. 准备至少一条 ended session history 记录和一条 active session 记录，或用 Playwright mock `GET /_bifrost/api/im-gateway/agent/sessions/all` 返回两类记录。
3. 在浏览器中打开：
   ```text
   http://localhost:$MAIN_PORT/_bifrost/ai?aiSection=agent-sessions&agentSection=sessions
   ```
4. 确认 Sessions 列表没有单独的查看 icon 按钮。
5. 点击 ended session 的 title。
6. 返回 Sessions 列表后，点击 active session 的当前行。

**预期结果**：
- 点击 ended session title 后进入 history 详情页，URL 包含对应 `session`、`view=history`、`historyPath`，并默认选中 `Messages` Tab。
- 点击 active session 当前行后进入 active 详情页，URL 包含对应 `session`、`view=active`，并默认选中 `Messages` Tab。
- 删除按钮仍然只触发删除确认，不会因为事件冒泡而打开详情页。
- 亮色和暗色主题下行 hover、title 可点击状态和删除按钮均清晰可辨。

### TC-ASP-16：重启恢复后 Context 不使用累计 token 回归

**操作步骤**：
1. 执行持久化恢复回归：
   ```bash
   cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_restores_history_after_service_restart --jobs 1 --timeout 240
   ```
2. 该用例会使用临时数据目录创建 `/agent/chat` session，第一轮 mock 响应写入 `assistant_message.tokens = 15`。
3. 用同一 `BIFROST_DATA_DIR` 重建 `ImGatewayService` 模拟服务重启。
4. 重启恢复后立即向同一 session 发送 `/status`。
5. 继续发送第二条业务消息，确认恢复后的历史仍包含第一轮消息。

**预期结果**：
- `/status` 返回 `Context 用量: ~15 / ...`，说明恢复后使用最近响应 context 快照，而不是历史累计 token。
- 第二条业务消息发给 mock 模型时仍包含第一轮和第二轮 marker，说明对话保持未被破坏。
- `/reset` 后再次发送新消息不会携带旧 marker。

**执行记录（2026-05-25）**：PASS — 执行 `source ~/.zshrc && cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_restores_history_after_service_restart --jobs 1 --timeout 240`，用例通过。日志中存在测试环境 CA 缺失和 AGENTS.md 截断 warning，但 mock 模型链路未走 TLS 拦截，测试最终 `1 passed`。

### TC-ASP-17：Sessions 列表不把空闲恢复会话误报为 Running

**操作步骤**：
1. 准备 `/agent/sessions/all` 返回一条 `status:"active"`、`running:false`、`state:"idle"` 的 session 和一条 ended history session。
2. 在浏览器中打开：
   ```text
   http://localhost:$MAIN_PORT/_bifrost/ai?aiSection=agent-sessions&agentSection=sessions
   ```
3. 查看 active session 行的状态标签。
4. 点击 active session 行进入详情。

**预期结果**：
- active idle session 行显示 `Active`，不显示 `Running`。
- 点击该行仍能进入 active session 详情，URL 包含 `view=active`。
- ended session 的 history 详情入口不受影响。

**执行记录（2026-05-25）**：PASS — 执行 `source ~/.zshrc && pnpm --dir web exec playwright test admin-settings.spec.ts -g "AI Agent Sessions 列表支持"`，用例通过；mock idle active session 带 `running:false`，断言该行不包含 `Running` 且包含 `Active`。

### TC-ASP-18：Agent Chat 已完成 Loop 默认折叠与工具摘要耗时回归

**操作步骤**：
1. 使用 Playwright mock 或真实 history JSONL 准备一条已完成 Agent Chat timeline，事件至少包含 `user_message`、两段 `assistant_delta`、一组 `tool_call/tool_result` 和最终 `assistant_message`，其中工具调用耗时大于 1 秒。
2. 在浏览器中打开：
   ```text
   http://localhost:$MAIN_PORT/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=completed-loop&view=history&historyPath=<url-encoded-path>
   ```
3. 不点击展开，查看消息区。
4. 点击 `已处理 <duration>` 摘要行展开该 turn。
5. 切换暗色主题后重复查看摘要行、process block 和最终输出。
6. 准备一条仍在运行的 timeline（最后一个 turn 只有 `tool_call`，尚无 `tool_result`），打开同一 Agent Chat history 深链并观察 `Running n commands` 摘要至少 2 秒。
7. 将浏览器宽度缩小到 640px 左右，查看消息列表、完成态摘要行、最终输出和底部输入框。
8. 在消息区向上滚动，观察底部输入框正上方是否出现滚动到底部按钮；点击该按钮。
9. 打开一个 `Running` 状态的历史线程，查看右上角 `New Chat` 按钮并点击创建新对话。

**预期结果**：
- 默认收起状态只显示用户消息、`已处理 <分钟/秒>` 摘要和最终 assistant 结论，不显示中间 assistant delta、工具参数或工具结果。
- 点击摘要后，中间 assistant delta、`Ran n commands · <分钟/秒>` process block、工具名、参数和结果按原始顺序恢复可见。
- 已完成工具摘要耗时来自 `tool_call` 到 `tool_result`，格式为 `Xs`、`Mm Ss` 或 `Hh Mm Ss`。
- 未完成工具摘要显示 `Running n commands (m active) · <分钟/秒>`，已执行时长每秒更新。
- 运行中的最后一个 turn 不会被默认折叠；该 turn 输出顶部显示 `已处理 <分钟/秒>` 并每秒更新，更早的已完成 turn 仍保持可折叠。
- 亮色和暗色主题下摘要行、箭头、耗时和最终输出均清晰可读。
- 窄屏下右侧线程列表不挤压消息列，消息列表随视口收窄并保留左右 padding；长路径、内联代码、代码块和表格不会撑出横向溢出。
- 当消息区不在底部时，输入框正上方居中淡入一个圆形向下箭头按钮；回到底部或点击按钮后按钮淡出隐藏，点击后消息区直接滚动到底部。
- 当前线程处于 `Running` 时仍可点击 `New Chat` 并创建新的独立 session；该动作不停止、不复用、也不修改原 running 线程。

**执行记录（2026-05-29）**：PASS — 执行 `source ~/.zshrc && pnpm --dir web exec playwright test agent-chat.spec.ts -g "deep link|restores active timeline process steps|keeps polling running history timeline|can start a new chat"`，5 个相关 UI 用例通过；另用 Vite dev server 代理 `BACKEND_PORT=9900` 打开真实 history 深链 `/Users/eden/.bifrost/agent/sessions/2026/05/29/session-feishu-main_ou_82c9bc36c12abfaed40c2c52ef4b7fea-1780017943.jsonl`，刷新后默认显示 `已处理 4m 26s` 和最终结论，展开后旧 turn 的过程摘要显示 `Ran ...`，不再误显示 `Running ...`，且可从持久化 timeline 恢复工具耗时；运行中 turn 输出顶部显示实时更新的 `已处理 <duration>`；将浏览器缩到约 640px 后消息列随视口收窄并保留左右 padding，没有横向溢出；消息区离开底部时滚动到底部按钮在输入框正上方居中淡入，点击后直接回到底部并淡出；`Running` 线程页面的 `New Chat` 可点击并能创建新 session。

### TC-ASP-19：陈旧 Running 状态与 Stop 一致性回归

**操作步骤**：
1. 使用临时 `BIFROST_DATA_DIR` 创建一条内置 Bifrost Agent session：canonical JSONL 中最后一个 `run_state_changed` 为 `completed`，但 `agent/im_gateway/session_state.json` 中同一 `sessionKey`、`adapter:"bifrost_agent"` 的 `status` 仍为 `running`。
2. 同时创建一条外部 runner session：`adapter` 使用非 `bifrost_agent` 值（例如 `codex`、`chatgpt_web` 或自定义 CLI adapter），`status:"running"`，且没有 completed/failed/stopped/timed_out 的 canonical timeline 终态。
3. 用临时端口启动 Bifrost，启动参数必须包含：
   ```bash
   --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
4. 请求：
   ```bash
   curl -fsS --noproxy '*' "http://127.0.0.1:$MAIN_PORT/_bifrost/api/im-gateway/agent/sessions/all"
   ```
5. 对第 1 步的内置 Agent session 发送：
   ```bash
   curl -fsS --noproxy '*' -X POST "http://127.0.0.1:$MAIN_PORT/_bifrost/api/im-gateway/agent/chat" \
     -H 'Content-Type: application/json' \
     -d '{"session_key":"<stale-session-key>","message":"/stop"}'
   ```
6. 再次请求 `/sessions/all`，并读取临时数据目录下的 `agent/im_gateway/session_state.json`。

**预期结果**：
- 第 1 步内置 Agent session 在 `/sessions/all` 中显示 `running:false`、`status:"ended"`、`state:"ended"`、`run_state:"completed"`，不会再显示为 `Running`。
- 第 1 步内置 Agent session 的 `last_active_time` 来自 canonical JSONL 终态时间，而不是陈旧 `session_state.updatedAt`。
- `/stop` 返回 `stopped:false`，并返回 `repaired_stale_running >= 1`，响应文案说明当前没有正在执行的 Agent loop 且已修复残留运行状态。
- `session_state.json` 中该内置 Agent session 的 `status` 被修复为 `ended`。
- 第 2 步外部 runner session 仍显示 `running:true`、`status:"active"`、`state:"running"`、`run_state:"running"`；该修复不会误停止或误结束不同装置、不同 adapter 的外部 runner。
- 如果任意 runner adapter 后续已有 canonical timeline 终态，则 `/sessions/all` 按该终态投影为 completed/failed/stopped/timed_out，不再受 persisted `status:"running"` 覆盖。
- 终态投影只使用最新模型显式写入的 `history_path`；未绑定到 `session_state.history_path` 的旧 history 文件不会被按 `sessionKey` 额外扫描或用于修正状态。

**执行记录（2026-06-02）**：PASS — 执行 `source ~/.zshrc && e2e-tests/tests/test_agent_session_stale_running_reconciliation.sh`，脚本使用临时数据目录和随机端口启动 Bifrost（包含 `--no-system-proxy`），先验证 completed JSONL + stale `bifrost_agent` running state 在 `/sessions/all` 投影为 ended/completed，再验证 `/stop` 返回 `stopped:false` 且 `repaired_stale_running >= 1` 并把 persisted status 修复为 `ended`，最后确认无终态 timeline 的外部 `codex`、`chatgpt_web` 和 `custom_cli` runner 都仍保持 running。follow-up 复核补充执行 `source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_running -- --nocapture --test-threads=1`，确认未绑定到显式 `history_path` 的 terminal history 不会被按 `sessionKey` 扫描用于状态投影；并执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_agent_session_stale_running_reconciliation.sh` 复跑真实 API 场景，结果 PASS。

### TC-ASP-20：Active Idle 会话不继承历史 Running run_state

**操作步骤**：
1. 准备一条已恢复到内存的 active session，当前 live 状态为 `running:false`、`state:"idle"`、`run_state:"idle"`。
2. 让同一 `session_key` 的 canonical JSONL 最后一条 `run_state_changed` 仍为 `running`，模拟历史文件缺失最终 completed/stopped 事件。
3. 请求：
   ```bash
   curl -fsS --noproxy '*' "http://127.0.0.1:$MAIN_PORT/_bifrost/api/im-gateway/agent/sessions/all"
   ```
4. 在 WebUI 打开该 active session，并加载对应 history timeline。
5. 不输入内容时点击输入框右侧 Stop 按钮。

**预期结果**：
- `/sessions/all` 返回该 active session 的 `running:false`、`state:"idle"`、`run_state:"idle"`，不会被 history summary 中的 `run_state:"running"` 覆盖。
- 如果该 session 只作为 history row 返回，`status:"ended"`、`state:"ended"` 时也不能继续暴露非终态 `run_state:"running"`；非终态 history summary 应投影为 `run_state:"completed"`。
- WebUI 不显示 Running 状态，不展示 Stop 作为当前 turn 操作；history timeline 中陈旧的 running event 不会把 telemetry phase 恢复为 `running`。
- 如果仍手动调用 `/stop`，响应可以是 `stopped:false` 和“当前没有正在执行的 Agent loop”，但页面状态必须与该响应一致。

**执行记录（2026-06-02）**：PASS — 在默认数据目录的真实会话 `feishu-main:ou_82c9bc36c12abfaed40c2c52ef4b7fea` 上复现 `/sessions/all` 返回 `running:false`、`state:"idle"` 但 `run_state:"running"`，同时 `/stop` 返回“当前没有正在执行的 Agent loop”。修复后执行 `source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin sessions_all_idle_active_session_does_not_inherit_running_history_state -- --nocapture` 和 `source ~/.zshrc && pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts`，分别验证后端 active session run_state 投影与前端 history telemetry 覆盖逻辑，结果 PASS。

### TC-ASP-21：Agent Chat 摘要字段不一致时运行态展示稳定

**操作步骤**：
1. 使用 Playwright mock 或真实数据准备一条 Agent Chat session summary，字段同时包含 `status:"active"`、`running:true`、`state:"completed"`、`run_state:"completed"`，并绑定一条 canonical timeline。
2. timeline 中包含 `run_state_changed: running`、`user_message`、`assistant_message`、`run_state_changed: completed`，模拟刷新后 summary 与 timeline 轮询存在短暂字段不一致。
3. 在浏览器中打开：
   ```text
   http://localhost:$MAIN_PORT/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=active-completed-stale-running&view=history&historyPath=<url-encoded-path>
   ```
4. 查看右上角状态 Tag、消息区最后一轮展示形态、左侧线程列表 running 点。
5. 停留至少 1 秒，等待一次 UI timer / poll 窗口后再次观察同样区域。

**预期结果**：
- 右上角状态保持 `Ready`，不会因 `status:"active"` 或陈旧 `running:true` 切回 `Running`。
- 消息区最后一轮展示为完成态 `已处理 <duration>` 折叠摘要，不出现 `agent-chat-turn-running-group` 或 `Agent is running...` 占位。
- 左侧线程列表不展示 `aria-label="running"` 的运行点。
- 等待后上述状态保持稳定，不出现 running/ready、运行态/完成态之间的抖动。

**执行记录（2026-07-08）**：PASS — 执行 `source ~/.zshrc && pnpm --dir web exec playwright test agent-chat.spec.ts -g "summary fields disagree"`，Playwright mock 复现 `status:"active" + running:true + run_state:"completed"` 的不一致摘要；断言右上角保持 `Ready`、消息区只显示完成态 `已处理` 摘要、没有 `agent-chat-turn-running-group`，左侧没有 running 点，并等待 1.2 秒后状态仍稳定。

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-test`
3. 清理测试生成的 session 文件（如果在默认目录）：`rm -f ~/.bifrost/agent/sessions/session-test-persist-*`
