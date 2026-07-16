# IM 主动推送会话上下文与 Runner 终态可靠性

## 背景

Daily Agent、研究摘要和工作日文章通过 `POST /im-gateway/messages/send` 主动发给 Bot owner。旧链路只写统一消息日志，不写 Agent 会话时间线，也不把消息带入外部 Runner 的下一轮请求。因此用户在微信里已经收到内容，Bot 随后的回答却会声称不知道该内容。

真实日报运行还暴露了另一个终态问题：隔离 external runner 完成后，worker 等待 progress channel 自行关闭；app-server session 可能继续持有 sender，导致 worker 无法发出 `Finished`，Daily Agent 卡在 Runner 已生成文件但尚未投递的阶段。

## 设计目标

- 成功发给 owner 的纯文本 API 消息同时进入该 provider/owner 的持久会话时间线。
- 该消息作为一次性待导入上下文进入下一轮外部 Runner instructions；消费后不重复注入。
- 服务启动时从统一消息日志幂等补录已有的日报概要、研究摘要和文章文字稿，修复升级前的上下文缺口，但不重新发送消息。
- 在线通知、失败发送、非 owner 目标、图片和卡片不进入主动推送上下文。
- external runner 本身终态后显式停止 best-effort progress forwarder，不能因 sender 生命周期较长而阻塞最终结果。

## 数据模型与幂等

`ImAgentSessionState.imported_outbound_message_ids` 保存最近 256 个已补录的统一消息日志 ID。`push_outbound_context_if_unseen` 在同一 session-state 文件锁内完成去重、seen marker 和 pending context 写入。pending context 使用 `target_adapter=proactive_outbound` 区分于 slash Runner call。

成功 owner text 的正文写入 canonical `ConversationRecorder` assistant message，session state 保存 `history_path`，因此 Agent Chat 详情与 Bot 会话时间线能读取同一份内容。若升级前日志只保留 preview，补录 preview；新消息从发送时起保留完整正文。

## Runner 注入

IM event loop 在建立当前 external CLI request、恢复 thread/conversation metadata 后，调用 `take_imported_contexts`。主动推送渲染为 `Proactive Messages Sent Through This Bot`，明确这些内容已经通过同一 Bot 发给用户，要求 Runner 将其视为当前会话上下文。take 操作保证一次性注入；seen marker 独立保留，服务重启不会再次补录。

## 启动补录边界

每个 provider 最多扫描 32 条最新成功 owner API text。为避免把历史测试、上线通知和旧版完整长日报污染上下文，启动补录只识别首行为“日报概要”“研究摘要”或中文书名号文章标题的 durable proactive message。发送时的新消息不受标题规则限制，所有成功 owner text 都会记录。

## Worker 终态

external runner 的 final result 已包含完整事件历史，实时 progress 仅为 best-effort。run future 进入终态后立即 abort 并 join progress forwarder，再写 `Finished`/`Failed`/`Stopped`；不等待所有 progress sender drop。

## 验证

- retained progress sender 下 forwarder stop 在 100ms 内返回。
- 主动消息相同日志 ID 只入队一次，消费后 pending 清空但 seen marker 保留。
- IM external runner instructions 同时包含原 instructions 与主动推送正文，第二次不再注入。
- 启动补录写 canonical timeline、pending context，并在第二次补录时保持单条。
- 正式 9900 上验证日报概要单条纯文本发送成功、worker 正常退出、历史日报/研究/文章只补录不重发；随后验证研究摘要发送后直接出现在同一 Bot session timeline。
