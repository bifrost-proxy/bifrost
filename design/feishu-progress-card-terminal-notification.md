# Feishu Progress Card Terminal Notification

## 背景

飞书外部 Runner 的 `progress_card` 投递会持续更新同一张 CardKit 卡片，并在任务结束时通过 `ImAgentProgressRegistry::finish` 把该卡片收敛为成功或失败状态。这个行为保留了完整执行过程，但任务结束后没有新消息进入会话，长任务在用户离开当前聊天后缺少可见的终态提醒。

`ExternalCliRunResult.response` 已由 external CLI runtime 从最后一个非空 `AssistantFinal`（或协议终态回退）选出，因此它是成功通知所需的“最后一次最终总结”。非成功 run status 会先由 `external_cli_non_success_reply` 归一化为包含超时、停止、退出码和 run id 的失败原因；runtime `Err` 路径则会清理诊断截图标记并生成可读错误摘要。

## 目标

- `progress_card` 任务成功后先更新原进度卡，再另发一条终态消息。
- 成功消息通过 CardKit 2.0 组件级 i18n 标题显示本地化的“任务执行结束 / Task completed”等文案，正文包含 `ExternalCliRunResult.response` 的最后一次最终总结。
- 非成功 run status 与 runtime 异常都先更新原卡为失败，再另发一条失败消息并说明原因。
- 新消息必须直接回复刚刚结束的任务进度卡，形成“用户原消息 → 任务进度卡 → 终态卡”的引用链，并进入既有 outbound message log。
- 终态通知发送失败不得改变 Runner 已确定的成功/失败状态，也不得阻止 session、队列和 group turn 收尾。
- 非 progress-card 投递维持现有 final reply / no-IM 行为，避免普通通道收到重复回复。

## 方案

在 IM external runner event loop 中新增统一的终态收束函数：

1. 调用 `progress_registry.finish(session_key, final_text, failed)`，保留原卡最终状态和正文。
2. 从 `finish()` 返回的 `ProgressCardMessageInfo.message_id` 取得最终任务卡 message id，并把它作为终态卡 `reply` API 的父消息；若飞书异常地未返回 message id，则只能向同一会话主动发送终态卡，不能误引用原触发消息。
3. 根据 `failed` 生成独立 CardKit 2.0 通知：成功使用绿色 header，失败使用红色 header；默认标题为英文，并通过标题组件 `i18n_content` 覆盖飞书支持的 16 个 locale，正文保留 Runner 自己输出的语言。
4. 普通 agent reply 继续在 provider 边界移除根级 header；终态卡通过专用的 header-preserving reply/send 路径发送，只有该卡保留多语言状态标题，不改变其它飞书卡片的紧凑样式。
5. 终态正文继续复用 agent reply 的 Markdown、图片、附件、工作目录解析和消息日志能力。
6. 两个发送动作均为 best effort；原卡更新失败不阻止新通知，新通知失败也不回滚任务终态。

## 边界

- 成功结果为空时使用语言中性的破折号占位，不能写死某一种语言的兜底文案，也不能发送空卡片。
- 失败结果沿用已经清洗/截断的用户可见原因，不能把诊断截图内部标记暴露到正文。
- 每个完成的队列 turn 各发送一次终态通知；不在 coalescer、progress snapshot 或 CardKit retry 内发送，避免刷新重试造成重复通知。
- progress card 启动失败时 `progress_enabled=false`，继续走既有普通 final reply；不会再额外发送第二条终态消息。

## 验证计划

- 单元/HTTP 集成测试：成功场景断言原卡被 finish、新消息只发送一次、绿色 header 包含默认英文及 `zh_cn`/`ja_jp` 等本地化标题、正文包含最后总结。
- 单元/HTTP 集成测试：失败场景断言原卡为失败、新消息只发送一次、红色 header 包含本地化失败标题、正文包含异常原因。
- 单元/HTTP 集成测试：断言终态请求路径直接引用上一张任务卡的 message id，而不是再次引用用户原消息。
- 单元/HTTP 集成测试：终态消息发送失败时原卡仍完成收敛，message log 记录失败且调用正常返回。
- shell E2E：隔离启动 Bifrost、mock external runner 与 loopback Feishu OpenAPI，分别注入成功和异常任务，断言 CardKit update 与独立 reply payload。
- human_tests：按成功、非成功状态、runtime 异常和既有单卡持续更新回归逐条执行。

## 风险

- 新消息会增加一次飞书 API 调用；失败仅记录日志，用户仍可从原卡看到终态。
- 最终总结可能较长，沿用普通 agent reply 的卡片和附件处理能力；若飞书拒绝该消息，原 progress card 中仍保留同一终态正文。
