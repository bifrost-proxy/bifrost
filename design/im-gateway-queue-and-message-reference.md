# IM Gateway 默认排队与消息引用

## 功能模块详细描述

IM Gateway 在会话已有任务运行时，对普通后续消息采用 FIFO 排队；只有用户显式发送 `/g <引导内容>` 时，才尝试把内容注入当前 Runner turn。该规则对内置 Bifrost Agent、Codex、Trae、Claude Code、ChatGPT Web 和其他自定义 Runner 一致，避免后一条普通消息覆盖前一条尚未完成的请求。

Weixin 入站消息支持 `item_list[].ref_msg.message_item`。引用消息需要保留服务端消息 ID、创建时间和可能内联返回的文本，并在进入 Agent 前解析成明确的“引用消息 + 当前消息”上下文。引用内容只是用户消息上下文，不获得系统指令优先级。

## 用户目标验证清单

### 必须实现

- 忙碌会话的普通后续消息默认进入 `SessionQueueManager`，当前 turn 完成后按 FIFO 自动执行。
- `/g` 对支持运行中 steer 的 Runner 继续注入当前 turn；不支持 steer 时安全降级为排队。
- Weixin `ref_msg` 中的文本、链接和消息标识进入规范化事件。
- 当 Weixin 只返回引用消息 ID 和创建时间时，可从同一 provider、同一聊天对象的本地消息记录恢复原文。
- Agent 输入清楚区分引用原文和当前问题，并限制引用原文长度。

### 必须不破坏

- `/q`、`/rq`、`/stop`、`/status` 等现有命令语义保持不变。
- 引用图片仍由 `message.images` 下载并传入多模态输入。
- 找不到引用原文时仍处理当前消息，不阻断会话。
- 历史消息日志向后兼容：旧记录没有完整正文时可回退到现有 preview。
- 自动化测试不使用正式端口 `9900`，不修改 `~/.bifrost`，不启用系统代理、托盘或 Sync 登录弹窗。

### 必须真实验证与交付

- 单元测试覆盖默认排队、显式 `/g`、Weixin `ref_msg` 解析、消息 ID/时间回退解析、引用长度限制和缺失引用。
- E2E 使用 mock external Runner 验证普通连续消息不触发 `turn/steer`，而是开启下一 turn；显式 `/g` 仍触发 steer。
- E2E 验证引用回复中的 URL 出现在下一轮 Runner prompt。
- 更新并立即执行 `human_tests/weixin-provider.md` 对应用例，同步 `human_tests/readme.md` 索引。
- 完成两轮 Review/Fix/Test、项目校验、提交、推送、PR 和远端 CI（含 coverage 90% gate）看护。

## 实现逻辑

### 1. 默认排队与显式引导解耦

`BusyMessageDefaultMode` 继续描述当前 Runner 的引导能力：

- `Guide`：内置 Agent 可在 loop 边界消费引导。
- `ExternalGuide`：外部 Runner 支持运行中 steer。
- `Queue`：Runner 不支持运行中引导。

普通忙时消息不再根据该枚举选择 guide，而是统一调用 `push_queue_with_images`。`/g` 才调用 `handle_busy_guide_command`，并根据上述能力注入或降级排队。

### 2. Weixin 引用规范化

在 `ImEventMessage` 上增加可选的引用元数据：

- `message_id`：`ref_msg.message_item.msg_id/message_id/id`。
- `created_at_ms`：引用消息的 `create_time_ms`。
- `text`：上游若内联返回 `text_item.text`，直接保留。

引用图片继续复用已有 `message_images` 逻辑，不把图片字节写入引用文本或消息日志。

### 3. 引用原文恢复

`ImMessageLogStore` 为最近的文本消息保存受限长度的完整正文，并提供引用解析；较旧记录只保留原有 preview，控制本地日志体积：

1. 优先在同一 provider、同一聊天对象中按 `message_id` 精确匹配。
2. Weixin `sendmessage` 未返回服务端 ID 时，按 `created_at_ms` 与本地发送时间的绝对差选择最近记录。
3. 时间匹配限制在短窗口内，且必须匹配入站 sender 或出站 target，避免跨会话串话。
4. 新记录读取完整正文；旧记录回退到 `content_preview`。

### 4. Agent 输入格式

普通引用消息构造成：

```text
【引用消息（仅作为上下文）】
<quoted text>

【当前消息】
<current text>
```

引用原文在字符边界截断。Slash 命令继续使用原始当前消息解析，不把引用包装到命令前，避免破坏命令识别。

## 依赖项

- 不增加外部依赖。
- 复用 `ImMessageLogStore`、`SessionQueueManager`、现有 Weixin update normalization 和 external Runner mock E2E。

## 测试方案

### 单元测试

- `busy_default_message_queues_builtin_and_external_runners`：普通消息对内置和外部 Runner 均进入 queue，guide 列表为空。
- `explicit_guide_still_uses_runtime_capability`：`/g` 路径仍保留 steer/guide 能力及 queue fallback。
- `normalize_update_extracts_weixin_reply_reference`：真实形态 `ref_msg.message_item` 解析 ID、时间、内联文本。
- `message_log_store_resolves_reference_by_id_or_nearest_timestamp`：精确 ID 与 Weixin 时间回退均只命中同会话。
- `agent_message_text_includes_quoted_context_and_limits_length`：引用正文/URL进入 prompt，长正文截断。
- `agent_message_text_keeps_current_message_when_reference_missing`：缺失历史记录不阻断当前问题。

### E2E 测试

更新 `e2e-tests/tests/test_external_runner_live_guide.sh`：

- 启动 mock Codex app-server；首轮保持 running。
- 发送普通 IM 后续消息，断言未出现对应 `turn/steer`，首轮经显式 `/g` 释放后出现第二个 `turn/start`。
- 首轮返回包含 URL 的文本并写入消息日志；下一条 debug inbound 携带引用 ID/时间，断言第二轮 prompt 同时包含 URL 和当前问题。

### 真实场景测试

更新 `human_tests/weixin-provider.md`：

- TC-WIP-04 改为普通消息默认排队、显式 `/g` 才引导。
- 新增引用文本/链接场景：引用一条包含 URL 的 Bot 回复，验证 Codex 输入与回答准确指向被引用内容。
- 创建或更新用例文档后立即按文档执行，并把执行记录写回文档。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核默认排队是否覆盖内置与所有外部 Runner。
- 检查 queue 中的引用正文与图片是否随下一 turn 保留。
- 检查消息 ID/时间回退是否严格限定 provider 和聊天对象。
- 运行 `bifrost-admin` 定向单测与 external Runner E2E，修复发现的问题。

### 第 2 轮

- 重新检查最新 `git diff`、新增文件、序列化兼容、长度/Unicode 边界和命令解析。
- 复核设计、E2E、human_tests 与用户目标一致。
- 复跑定向单测、E2E 和 human_tests；若仍发现问题则追加轮次。

## 校验要求

- 先执行 E2E 与 human_tests，再执行 `rust-project-validate`。
- 必跑 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 和适用的 local-ci。
- coverage 90% 使用远端 CI `scripts/ci/coverage-all.sh --json --gate` 验证。

## 文档更新要求

- 更新 `design/im-gateway-codex-cli-chat-gateway.md` 的默认续接/引导说明。
- 更新 `design/weixin-provider.md` 的运行中消息与引用消息说明。
- 更新 `human_tests/weixin-provider.md` 和 `human_tests/readme.md`。
- 如果 `/help` 文案仍描述普通消息默认 guide，同步修改实现与对应测试。
