# External Runner 运行中引导

## 背景

Bifrost 已支持通过 Chat Gateway、IM Gateway 与 `bifrost agent run` 启动 Codex、Traex 等 external CLI runner。当前 Codex/Traex adapter 使用 `exec --json ... -`：Bifrost 只在进程启动时把首条 prompt 写入 stdin，随后关闭 stdin 并等待 JSONL 输出。相同 `session_key` 在任务运行期间收到的新消息会进入 FIFO queue，必须等当前 turn 结束后再通过 `exec resume` 启动下一轮。


本方案让内置 Codex、Traex runner 默认使用每轮独立的 stdio app-server transport，让 Claude Code 默认使用长连接 stream-json transport，并把 worker 控制协议扩展为 `Run` / `Guide` / `Stop`。WebUI 在会话忙碌时可选择 Guide/Queue，普通输入默认采用 Guide；IM 的普通后续消息统一 FIFO 排队，只有显式 `/g` 才尝试注入当前 turn。ChatGPT Web 因浏览器对话链路无法安全注入当前生成过程，继续只支持 Queue。Claude Code、自定义 CLI 和显式 exec transport 收到显式 `/g` 时会先尝试 worker guide capability，无法实时注入时明确降级到 FIFO queue，不能伪装成已注入。

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
- busy external runner 必须先拦截 `/models`、`/model`、`/efforts`、`/effort` 等 Bifrost slash 命令，再进入默认 Guide/Queue 路由；控制命令不得作为 `turn/steer` 输入透传给 Runner。
- `BIFROST_EXTERNAL_CLI_WORKER` 只能标记显式 `external-runner-worker` 子进程，不能再作为生产运行时是否派生 worker 的分支条件。Desktop sidecar、Tray helper、Tray 拉起的服务和 detached daemon 等长期进程命令边界必须删除误继承的同名环境变量，避免内部角色标记继续向无关后代扩散。

### 必须不破坏

- 自定义 `adapterConfig.args` 的 Codex/Traex runner 继续使用原始 exec transport。
- 配置 `adapterConfig.transport=exec` 时强制使用 exec；Claude Code 在该路径保持 `--input-format text`，因为一次性 exec 会把原始 prompt 文本写入 stdin，不能沿用 stream-json 输入参数。旧配置没有 transport 字段时，内置 Codex/Traex、没有 custom args，且 executable 未覆盖或 basename 是 `codex` / `traex` / `traecli` 时才默认 app-server。自定义 wrapper 继续 exec，除非显式声明 `transport=app_server`。
- ChatGPT Web 保持默认排队语义；Claude Code 默认使用 stream-json 实时通道，显式 text/custom args/exec 与自定义 adapter 在缺少 live guide transport 时必须明确降级排队并保留原消息。
- `/stop`、run stop marker、超时、worker process group 清理与服务退出清理继续有效。
- model、reasoning effort、sandbox/approval、service tier、config override、feature flag、work dir、图片路径和 session resume 语义不因 transport 迁移丢失。
- 现有 `run_started`、progress、`run_finished` NDJSON 消费方兼容；只允许增加字段与 guide 专用响应。
- 同一 session 只允许一个 active turn；除 ChatGPT Web 外，新普通文本消息默认请求 guide，显式 `/q` 才排队。
- 真正的 external-runner worker 继续显式设置 `BIFROST_EXTERNAL_CLI_WORKER=1`；清理长期进程环境不得造成 worker 递归派生，也不得改变 worker process-group、Stop 或超时回收语义。

### 必须真实验证

- mock Codex app-server：运行慢工具期间发送 guide，断言收到 `turn/steer`，`expectedTurnId` 等于 active turn，最终回复包含 steer 后结果且没有第二个 turn/start。
- mock Traex app-server：验证同一协议路径和 adapter 可执行文件选择。
- mock app-server 在 steer 时返回 no-active/mismatch：Chat Gateway 响应为 `delivery=queued`，当前 turn 结束后消息作为下一 turn 执行。
- 显式 `transport=exec` 与 custom args：guide 返回 queue fallback，原 exec JSONL 仍成功；Claude exec 默认参数必须是 `--input-format text` 且不包含 `--replay-user-messages`。
- 真实本机 Codex 与 Traex CLI：使用独立 `BIFROST_DATA_DIR` 和临时端口启动最新二进制，分别启动长任务、从第二终端发送 guide、确认同 turn 接收并正常收尾，无残留 app-server/worker 进程。
- `/stop` 在 app-server turn 运行期间能结束 worker 与子进程，run/session 状态不残留 running。
- 从带有 `BIFROST_EXTERNAL_CLI_WORKER=1` 的父环境执行 `bifrost start --daemon`，daemon 必须清除该变量；真实 Chat Gateway/IM mock inbound 的第二条 busy 消息仍返回 `delivery=steered`，不能因主进程缺少 `ACTIVE_WORKER_SESSIONS` 而降级 Queue。

### 必须交付

- 更新 external runner worker、app-server transport、Chat Gateway、IM busy capability、CLI 与相关文档。
- 单元测试、shell E2E、human_tests 用例与索引同步更新并真实执行。
- 完成两轮 Review/Fix/Test、项目最终校验、提交、PR 和远端 CI 看护。
- 远端 `bash scripts/ci/coverage-all.sh --json --gate` 90% 棘轮门禁通过。

## 产品语义

### Guide 与 Queue

- `guide`：要求当前 active runner 立即改变执行方向。Codex/Traex 通过 `turn/steer` 修改当前 turn；Claude Code 通过官方 interrupt control request 中断当前响应，再在同一进程与同一 session 中接续处理 guide。
- `queue`：当前 turn 结束后作为新 turn 执行。
- 除 ChatGPT Web 外，WebUI 的普通 busy text 按界面当前模式处理且默认 Guide；IM 普通 busy text 默认 Queue，`/g` 才显式请求 Guide。
- busy 状态下 `/efforts` 与 `/effort` 继续走 Bifrost session 命令处理：查询即时返回，设置只影响下一轮，不改变已运行中的 turn。
- Codex/Traex app-server RPC 成功 ack 后展示当前 turn 已引导。Claude Code 必须先收到 interrupt control response，再发送 guide user frame，并在 `--replay-user-messages` 回显确认后展示 session 已重定向；单独的 user-frame 回显只代表排队确认，不得伪装成当前响应已被引导。自定义/exec transport 或 turn-end race 无法 ack 时展示降级原因并排队。
- ChatGPT Web 始终默认 Queue，WebUI 不展示 Guide 切换，IM 普通 busy message 直接排队。
- 图片暂不进入 `turn/steer` 文本协议；external runner 忙碌时收到图片必须保留附件并明确降级排队，不能只注入占位文本或丢失图片。
- 调用方收到 `delivery=steered` 才能展示实时引导已生效；有 `turnId` 表示当前 turn steer，无 `turnId` 表示同一 runner session interrupt-and-continue。`delivery=queued` 必须展示降级原因。

### 运行态真源

主进程 worker registry 与 worker 内 runner 无关的 `ACTIVE_GUIDE_SESSIONS` 共同构成实时真源，均按 `session_key` 索引：

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

### Claude Code stream-json transport

- Claude Code 无 custom args 时默认参数为 `-p --verbose --output-format stream-json --input-format stream-json --replay-user-messages`；显式 custom args（包括 `--input-format text`）继续走 exec。
- 首条 user JSONL 帧启动响应，stdin 在 run 内保持打开。普通追加 user 帧只会排队成后续响应，因此 guide 必须先发送 `control_request(request.subtype=interrupt)`；CLI 返回匹配的成功 `control_response` 后，才在同一 stdin 发送 guide user 帧，不启动第二个进程。
- 收到 `system/init` 后以 Claude `session_id` 注册通用 live guide handle；`result`、stop、timeout 或 session ownership 变化时按 run id 条件注销，避免旧 turn 清理新 handle。
- interrupt 或 guide 写入成功后都不立即声称 steered；只有匹配 request id 的 control response 成功且 `--replay-user-messages` 回显匹配 guide user frame，才返回 accepted。interrupt 拒绝、通道关闭或回显超时返回 rejected，由上层原子降级排队；同一时刻只允许一个待确认的 Claude redirect，额外 guide 明确拒绝并走 queue fallback。
- interrupt 后 Claude 会先输出旧响应的 `result(error_during_execution)`，随后为 guide 输出新的 init/assistant/result。transport 只抑制已确认 interrupt 对应的中间失败 result，继续等待 guide 的最终 result，避免把旧响应的中断误报成整个 run 失败。
- stdout 继续复用 Claude Code 现有 `ExternalCliProgressEvent` 解析，assistant/tool/最终 result 展示与原 exec transport 兼容；持久化 `session_id` 仍用于下一轮 `--resume`。
- 收到终态 result 后先关闭 stdin 并等待进程自然退出；超过 grace window 必须终止进程组并再次有界等待，之后才 join stderr reader，避免 CLI 已给结果但仍持有 stderr 导致 worker 永久挂起。

### Transport 选择

```text
transport=exec                         -> exec
transport=app_server + unsupported     -> clear startup error
transport=stream_json + non-Claude      -> clear startup error
custom adapterConfig.args              -> exec
custom executable basename             -> exec（显式 app_server 除外）
Codex/Traex + official executable      -> app_server
Claude Code + no custom args            -> stream_json
other adapter                          -> existing transport
```

不在 app-server 启动失败后静默重跑 exec，因为第一进程可能已经创建 thread/产生副作用；错误必须可见。兼容 fallback 只由明确 transport 选择与能力判断决定。Unix 在 `spawn` 尚未创建子进程时若返回瞬态 `ETXTBSY`（例如 CLI 刚完成原子升级或 CI 刚落盘 mock executable），允许在 200ms 内有界重试同一命令；其他错误立即返回，且一旦成功创建子进程就不再做启动重放。

### Worker 协议

```json
{"type":"guide","guideId":"...","message":"..."}
{"type":"guide_result","guideId":"...","accepted":true,"threadId":"...","turnId":"..."}
```

主进程 registry 的 ack 只有在收到 `guide_result` 后完成。worker 退出时所有 pending guide 返回 rejected，调用方随后进入 queue fallback。
主进程 worker control channel 与单 run 的 pending guide map 都限制为 32 条；饱和时新 Guide 必须快速返回明确错误并走现有 FIFO queue fallback，不能继续无界占用内存。Stop 在 control channel 饱和时直接终止已确认归属该 session 的 worker，关闭/陈旧 channel 则保留 PID reuse 防护，不盲目 kill。

### Worker 标记生命周期

`BIFROST_EXTERNAL_CLI_WORKER` 是进程角色标记，不是用户配置或全局运行模式。唯一合法的注入点是主进程创建隐藏子命令 `agent external-runner-worker` 时；worker 隐藏子命令直接调用私有的 in-process runner transport，避免再次派生 worker。生产 `run_with_progress` 不读取该环境变量，因此污染的前台进程或 Linux fork daemon 也不会绕过 worker registry。

长期进程会成为后续所有 Agent turn 的环境根节点，因此必须在以下命令边界显式 `env_remove`：

- Desktop 创建 backend sidecar；
- 服务创建 Tray helper；
- Tray helper 重新启动 detached service；
- `start --daemon` 创建 detached daemon child。

这项清理只作用于新子进程的环境，不修改当前进程环境，也不清理真实 worker 的标记。旧版本已经启动且受污染的长期进程不会被热修复；安装新版本后需要由用户选择合适窗口完整重启 Desktop、Tray 和 core，避免打断正在进行的连接。

### Worker 内发起自升级

外部 CLI worker、Codex 和它启动的 shell 都是运行中 Bifrost daemon 的后代。如果这条进程树直接执行普通 `bifrost upgrade`，updater 在停止旧 daemon 时会触发 active run 回收，连同 worker、Codex 和 updater 自身一起被终止，导致二进制尚未替换而服务已经退出。

因此 `bifrost upgrade` 在检测到 `BIFROST_EXTERNAL_CLI_WORKER=1` 时必须在获取本地 upgrade lock 或执行任何安装动作之前改变编排方式：

1. 从当前 data dir 的 `runtime.json` 读取 owner PID 和 Admin 端口，并确认 owner 仍存活；
2. 通过 loopback 直连请求 `POST /_bifrost/api/system/upgrade?channel=cli`；
3. Admin 使用既有 detached `bifrost self-update --source admin` 编排器启动升级；该进程不属于 external run 的 process group，能在 daemon 回收 Codex 后继续完成安装和重启；
4. CLI 收到 `202 Accepted` 后只报告“已安排升级”并退出，不等待重启完成；`409 No update available` 和 `409 An upgrade is already in progress` 作为幂等成功返回。

这一委托必须 fail closed：runtime 缺失、owner 已退出、Admin 不可达或返回未知错误时，CLI 返回明确错误并保留当前服务，禁止回退到进程树内 inline upgrade。隐藏的 `self-update` 子命令继续直接进入 background handler，不读取该分支，避免 Admin 派生的 updater 再次委托形成循环。

Linux 的传统 daemon 路径使用 `fork` 而不是 `Command`，因此在 fork child 进入长期运行前直接 `remove_var` 清除该角色标记；生产 dispatch 同时不再读取 ambient marker，形成进程环境与调度语义两层隔离。由 Tray 创建 Linux 服务时，Tray 的 command 边界仍会清除该变量。

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

- transport selection：Codex/Traex default app-server、Claude Code default stream-json、explicit exec、text/custom args fallback、unsupported adapter。
- app-server request：initialize/initialized、thread start/resume、turn start、turn steer 字段完整。
- app-server spawn：Unix `ETXTBSY` 有界重试后成功，非瞬态错误不重试。
- notification normalization：agent message、command execution、reasoning、plan、turn completed/failed。
- worker protocol：Guide serialize/parse、ack correlation、worker exit rejects pending、control channel 饱和快速拒绝与 32 条 pending 上限。
- worker marker：生产 dispatch 忽略 ambient marker；Desktop backend、Tray helper、Tray service 与 detached daemon command 均删除继承标记；真实 external-runner worker command 仍显式设置标记。
- guide result：accepted、no active、mismatch、non-steerable、timeout -> queue fallback。
- CLI：Guide 参数、URL encoding、JSON/人类输出、空 message/session 拒绝。

### E2E

新增 `e2e-tests/tests/test_external_runner_live_guide.sh`，用 mock app-server/stream-json/exec 可执行文件与独立数据目录启动真实 Bifrost 二进制，覆盖 Codex、Traex、Claude interrupt-and-continue、Codex/Claude reject-to-queue、explicit exec 与 inactive-session reject；启动链路显式污染父环境并通过 detached daemon 验证长期进程会清除 worker 标记；既有 worker stop 聚焦测试继续覆盖 stop cleanup。脚本由 CI full-shell 的 `test_*.sh` 自动收录。

所有模拟 Claude Code stream-json 的测试夹具必须读取一条初始 user JSONL frame 后开始输出，不能通过 `cat` 等待 stdin EOF。需要验证 guide 的夹具还必须依次校验 interrupt control request、返回匹配 request id 的 control response、读取 guide user frame，并模拟旧响应的 interrupted result 与 guide 的最终 success result。真实 transport 会保持 stdin 打开；等待 EOF 会让夹具在正确的产品行为下永久阻塞并最终误报 30 秒超时。

Web Playwright 同时覆盖 Codex/Traex/Claude Code 默认 Guide、显式 Queue、ChatGPT Web 只展示 Queue，以及 Guide 降级后刷新队列状态。IM mock inbound 覆盖普通 busy text 默认排队、显式 `/g` steer、`/q` 显式排队、busy Codex/Traex 的 `/efforts` 与 `/effort` 不透传、微信引用上下文，以及 ChatGPT Web 默认排队。

`e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh` 以真实本地 release 归档启动独立 daemon，再执行 `BIFROST_EXTERNAL_CLI_WORKER=1 bifrost upgrade`：断言 worker CLI 在 10 秒内返回委托成功、后台进度持续到 completed、旧 PID 被新 PID 替换且新 daemon 使用升级后的安装路径。该链路同时证明 updater 不依赖发起升级的 worker/Codex 调用者继续存活。

### Human tests

更新 `human_tests/im-guide-queue-mode.md` 新增污染父环境下 detached daemon 仍可实时 Guide 的回归用例，并同步 `human_tests/readme.md` 对应模块索引；创建/更新后立即逐条执行并记录结果。

### Review/Fix/Test

- 第 1 轮：复核 Desktop、Tray helper、Tray service、detached daemon 与真实 worker 五个边界，检查是否存在误删 worker 标记或遗漏长期进程；运行各命令构造器单元测试和 external runner live-guide E2E。
- 第 2 轮：基于修复后的最新 diff 复查设计、E2E cleanup、human_tests 索引与跨平台命令行为，复跑受影响测试；随后按 `rust-project-validate` 执行项目校验，并由远端 `bash scripts/ci/coverage-all.sh --json --gate` 兜底 90% 棘轮门禁。

## 风险与回滚

- app-server 是版本化协议：启动握手与方法不存在必须返回明确版本错误；不做盲目 exec 重试。
- 配置映射与 exec flag 不完全等价：所有已支持 model/effort/sandbox/config 字段必须有单测；无法等价的 custom args 自动保留 exec。
- turn-end race：必须依赖 expected turn id + ack + queue fallback，不允许 fire-and-forget。
- app-server stdout 可能包含未知 notification：保留 raw frame，忽略未知展示事件但不能中断 turn。
- Linux 并行测试或 CLI 更新窗口中，`fork/exec` 可能因其它线程短暂持有可执行文件的可写句柄而返回 `ETXTBSY`。app-server 启动层只对该 OS 错误执行最多 8 次、总计不超过 140ms 的线性退避重试；其它 spawn 错误立即返回，且不得在进程已成功启动后重跑，以免重复创建 thread 或产生副作用。
- 回滚：用户可设置 `adapterConfig.transport=exec`，或给 Claude Code 配置 `--input-format text` custom args 恢复原路径；其他自定义 args 天然继续 exec。
