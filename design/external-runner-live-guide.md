# External Runner 运行中引导

## 背景

Bifrost 已支持通过 Chat Gateway、IM Gateway 与 `bifrost agent run` 启动 Codex、Traex 等 external CLI runner。当前 Codex/Traex adapter 使用 `exec --json ... -`：Bifrost 只在进程启动时把首条 prompt 写入 stdin，随后关闭 stdin 并等待 JSONL 输出。相同 `session_key` 在任务运行期间收到的新消息会进入 FIFO queue，必须等当前 turn 结束后再通过 `exec resume` 启动下一轮。

内置 Bifrost Agent 已支持 guide channel：运行中消息可在工具调用 checkpoint 后进入同一 turn。Codex CLI 与 Trae CLI 的 app-server 协议也都提供 `turn/steer`，要求携带当前 `threadId`、`expectedTurnId` 与输入。当前差距不是产品入口，而是 external runner worker 只支持 `Run` / `Stop`，且底层 adapter 没有保留可双向控制的协议连接。

本方案让内置 Codex、Traex runner 默认使用每轮独立的 stdio app-server transport，并把 worker 控制协议扩展为 `Run` / `Guide` / `Stop`。WebUI 在会话忙碌时可选择 Guide/Queue，普通输入默认采用 Guide；IM 的普通后续消息统一 FIFO 排队，只有显式 `/g` 才尝试注入当前 turn。ChatGPT Web 因浏览器对话链路无法安全注入当前生成过程，继续只支持 Queue。Claude Code、自定义 CLI 和显式 exec transport 收到显式 `/g` 时会先尝试 worker guide capability，无法实时注入时明确降级到 FIFO queue，不能伪装成已注入。

## 用户目标验证清单

### 必须实现

- `bifrost agent run --runner Codex --session <key> ...` 启动的普通 turn 在运行中可通过第二个 CLI 进程向同一 session 发送 guide。
- Codex 与 Traex 使用各自 `app-server --stdio` 的 `turn/steer`，不得通过 PTY 键盘模拟或继续向一次性 `exec -` stdin 写数据。
- guide 必须带 `expectedTurnId`；只有 app-server 返回成功后才能向调用方报告 `steered`。
- guide 接口必须返回 `guideId`、`sessionKey`、`delivery`、`threadId`、`turnId` 与可选 `reason`，便于 CLI、IM 和 Web 做一致展示。
- app-server 拒绝 `no active turn`、turn id mismatch、review/compact non-steerable，或 guide 到达 turn-end 窗口时，消息必须原子降级到现有 FIFO queue，不能丢失。
- `clientUserMessageId` 使用 Bifrost 生成的 `guideId`，app-server response 与 worker ack 可关联同一输入。
- worker process isolation 保持不变：主进程只持有 worker 控制句柄，Codex/Traex app-server 仍在 `bifrost-runner` 子进程内启动。
- app-server notification 映射到现有 `ExternalCliProgressEvent`，保留 tool、plan、assistant、run status 与最终回复展示。
- 下一轮继续复用现有 `external_thread_id`，通过 `thread/resume` + `turn/start` 续聊。
- 新增 `bifrost agent guide --session <key> <message>`；session 必填，避免向错误的默认 runner session 发送控制消息。
- 新增 Chat Gateway session guide endpoint；IM 只有显式 `/g` 按 live capability 判断 steer 或 queue，普通 busy message 与 `/q` 都进入队列。

### 必须不破坏

- 自定义 `adapterConfig.args` 的 Codex/Traex runner 继续使用原始 exec transport。
- 配置 `adapterConfig.transport=exec` 时强制使用 exec；旧配置没有 transport 字段时，内置 Codex/Traex、没有 custom args，且 executable 未覆盖或 basename 是 `codex` / `traex` / `traecli` 时才默认 app-server。自定义 wrapper 继续 exec，除非显式声明 `transport=app_server`。
- ChatGPT Web 保持默认排队语义；Claude Code 与自定义 adapter 在缺少 live guide transport 时必须明确降级排队并保留原消息。
- `/stop`、run stop marker、超时、worker process group 清理与服务退出清理继续有效。
- model、reasoning effort、sandbox/approval、service tier、config override、feature flag、work dir、图片路径和 session resume 语义不因 transport 迁移丢失。
- 现有 `run_started`、progress、`run_finished` NDJSON 消费方兼容；只允许增加字段与 guide 专用响应。
- 同一 session 只允许一个 active turn；WebUI 可按运行中模式选择 guide/queue，IM 普通文本默认排队、显式 `/g` 才请求 guide。

### 必须真实验证

- mock Codex app-server：运行慢工具期间发送 guide，断言收到 `turn/steer`，`expectedTurnId` 等于 active turn，最终回复包含 steer 后结果且没有第二个 turn/start。
- mock Traex app-server：验证同一协议路径和 adapter 可执行文件选择。
- mock app-server 在 steer 时返回 no-active/mismatch：Chat Gateway 响应为 `delivery=queued`，当前 turn 结束后消息作为下一 turn 执行。
- 显式 `transport=exec` 与 custom args：guide 返回 queue fallback，原 exec JSONL 仍成功。
- 真实本机 Codex 与 Traex CLI：使用独立 `BIFROST_DATA_DIR` 和临时端口启动最新二进制，分别启动长任务、从第二终端发送 guide、确认同 turn 接收并正常收尾，无残留 app-server/worker 进程。
- `/stop` 在 app-server turn 运行期间能结束 worker 与子进程，run/session 状态不残留 running。

### 必须交付

- 更新 external runner worker、app-server transport、Chat Gateway、IM busy capability、CLI 与相关文档。
- 单元测试、shell E2E、human_tests 用例与索引同步更新并真实执行。
- 完成两轮 Review/Fix/Test、项目最终校验、提交、PR 和远端 CI 看护。
- 远端 `bash scripts/ci/coverage-all.sh --json --gate` 90% 棘轮门禁通过。

## 产品语义

### Guide 与 Queue

- `guide`：追加到当前 active regular turn，由 runner 在当前工具调用/模型 checkpoint 后消费。
- `queue`：当前 turn 结束后作为新 turn 执行。
- 除 ChatGPT Web 外，WebUI 的普通 busy text 按界面当前模式处理且默认 Guide；IM 普通 busy text 默认 Queue，`/g` 才显式请求 Guide。
- Codex/Traex app-server 成功 ack 后展示已注入；Claude Code、自定义/exec transport 或 turn-end race 无法 ack 时展示降级原因并排队。
- ChatGPT Web 始终默认 Queue，WebUI 不展示 Guide 切换，IM 普通 busy message 直接排队。
- 图片暂不进入 `turn/steer` 文本协议；external runner 忙碌时收到图片必须保留附件并明确降级排队，不能只注入占位文本或丢失图片。
- 调用方收到 `delivery=steered` 才能展示“已注入”；`delivery=queued` 必须展示降级原因。

### 运行态真源

主进程 worker registry 与 worker 内 app-server registry 共同构成实时真源，均按 `session_key` 索引：

```text
main process: session_key -> worker_pid + control_tx
worker process: session_key -> thread_id + turn_id + guide_tx
```

持久化 `external_thread_id` 只用于下轮 resume；`turn_id` 是当前执行态 precondition，不作为持久化续聊 id。

### 可靠降级

1. CLI、IM 或 WebUI 生成/请求 `guideId`；IM/WebUI 对除 ChatGPT Web 外的普通 busy text 使用同一路径。
2. registry 解析当前 `threadId + turnId` 并把 `Guide` 发给 worker。
3. worker 调用 app-server `turn/steer`，使用 `guideId` 作为 `clientUserMessageId`。
4. 收到成功 response：返回 `delivery=steered`。
5. 收到 no-active、mismatch、non-steerable、worker closed 或 transport unavailable：把原消息写入 FIFO queue，再返回 `delivery=queued`。
6. guide request 超时不得直接声称成功；如果无法确认 app-server 是否接受，记录 reason 并降级排队。该极端路径保证消息不丢失（at-least-once），不能在协议未回 ack 时承诺 exactly-once。

`expectedTurnId` 防止 turn-end race 把 guide 注入刚启动的新 turn；FIFO fallback 防止消息在同一 race 中丢失。

## 技术方案

### App-server transport

新增 `external_cli/app_server.rs`：

- 启动 CLI 支持的 stdio app-server：Codex 使用 `<executable> app-server --stdio`，Traex 使用 `<executable> app-server --listen stdio://`；不开放 unix/ws listener。
- 发送 `initialize`，等待 response，再发送 `initialized` notification。
- 无 thread id：`thread/start`；已有 thread id：`thread/resume`。
- 发送 `turn/start` 并保存 response 的 `turn.id`。
- 注册 session control handle，循环处理 app-server stdout notification、guide control、stop marker 与 timeout。
- `Guide` 转为 `turn/steer`；`Stop` 转为 `turn/interrupt`，超时后沿用 process-group kill。
- `thread/started`、`turn/started`、`item/started`、`item/completed`、`turn/completed` 映射成现有 Codex-like normalized events。
- `agentMessage` 作为最终 response；command/file/MCP/dynamic tool item 映射 tool started/finished；plan/reasoning 保持现有展示。

### Transport 选择

```text
transport=exec                         -> exec
transport=app_server + unsupported     -> clear startup error
custom adapterConfig.args              -> exec
custom executable basename             -> exec（显式 app_server 除外）
Codex/Traex + official executable      -> app_server
other adapter                          -> existing transport
```

不在 app-server 启动失败后静默重跑 exec，因为第一进程可能已经创建 thread/产生副作用；错误必须可见。兼容 fallback 只由明确 transport 选择与能力判断决定。

### Worker 协议

```json
{"type":"guide","guideId":"...","message":"..."}
{"type":"guide_result","guideId":"...","accepted":true,"threadId":"...","turnId":"..."}
```

主进程 registry 的 ack 只有在收到 `guide_result` 后完成。worker 退出时所有 pending guide 返回 rejected，调用方随后进入 queue fallback。
主进程 worker control channel 与单 run 的 pending guide map 都限制为 32 条；饱和时新 Guide 必须快速返回明确错误并走现有 FIFO queue fallback，不能继续无界占用内存。Stop 在 control channel 饱和时直接终止已确认归属该 session 的 worker，关闭/陈旧 channel 则保留 PID reuse 防护，不盲目 kill。

### Admin API 与 CLI

```http
POST /_bifrost/api/im-gateway/chat/sessions/{sessionKey}/guide
Content-Type: application/json

{"message":"先检查失败日志","guideId":"optional-idempotency-key"}
```

```json
{
  "guideId": "guide-...",
  "sessionKey": "cli-Codex",
  "delivery": "steered",
  "threadId": "...",
  "turnId": "..."
}
```

CLI：

```bash
bifrost agent guide --session cli-Codex "先检查失败日志"
```

默认输出明确区分 `Steered active turn` 与 `Queued for next turn`；`--json` 返回原始响应。

## 测试设计

### 单元测试

- transport selection：Codex/Traex default、explicit exec、custom args、unsupported adapter。
- app-server request：initialize/initialized、thread start/resume、turn start、turn steer 字段完整。
- notification normalization：agent message、command execution、reasoning、plan、turn completed/failed。
- worker protocol：Guide serialize/parse、ack correlation、worker exit rejects pending、control channel 饱和快速拒绝与 32 条 pending 上限。
- guide result：accepted、no active、mismatch、non-steerable、timeout -> queue fallback。
- CLI：Guide 参数、URL encoding、JSON/人类输出、空 message/session 拒绝。

### E2E

新增 `e2e-tests/tests/test_external_runner_live_guide.sh`，用 mock app-server/exec 可执行文件与独立数据目录启动真实 Bifrost 二进制，覆盖 Codex、Traex、reject-to-queue、explicit exec 与 inactive-session reject；既有 worker stop 聚焦测试继续覆盖 stop cleanup。脚本由 CI full-shell 的 `test_*.sh` 自动收录。

Web Playwright 同时覆盖 Codex/Traex/Claude Code 默认 Guide、显式 Queue、ChatGPT Web 只展示 Queue，以及 Guide 降级后刷新队列状态。IM mock inbound 覆盖普通 busy text 默认排队、显式 `/g` steer、`/q` 显式排队和 ChatGPT Web 默认排队。

### Human tests

更新 `human_tests/im-gateway-external-cli-chat-gateway.md` 新增 Codex/Traex 运行中 guide 用例，并同步 `human_tests/readme.md` 对应模块索引；创建/更新后立即逐条执行并记录结果。

## 风险与回滚

- app-server 是版本化协议：启动握手与方法不存在必须返回明确版本错误；不做盲目 exec 重试。
- 配置映射与 exec flag 不完全等价：所有已支持 model/effort/sandbox/config 字段必须有单测；无法等价的 custom args 自动保留 exec。
- turn-end race：必须依赖 expected turn id + ack + queue fallback，不允许 fire-and-forget。
- app-server stdout 可能包含未知 notification：保留 raw frame，忽略未知展示事件但不能中断 turn。
- 回滚：用户可设置 `adapterConfig.transport=exec` 恢复原路径；自定义 args 天然继续 exec。
