# IM Agent 动态外发上下文

> 状态：已实现并完成本地验证
> 实现入口：`crates/bifrost-admin/src/handlers/im_gateway/agent_outbound_context.rs`、`crates/bifrost-admin/src/handlers/im_gateway/event_loop/external_runner.rs`

## 背景

来自 IM Gateway 的 Codex、Trae X、Claude Code、Chains 或自定义 Runner，能正常把最终答复交回 Bifrost 自动回复，但模型并不知道“额外主动发送”必须通过哪一个 Bifrost provider、发向哪一个会话。模型因此可能误用 Lark IM、微信 connector 或平台 OpenAPI；这些工具既不一定有权限，也会绕过 Bifrost 的 provider、能力预检、发送回执与审计链路。

解决方案不是恢复全局静态 `Bifrost Tool Context`，而是为每一个真实 IM Runner turn 生成一段可信、短生命周期的动态外发上下文。Web Chat、Schedules 和其他非 IM 调用不注入。

## 目标

- 对所有 IM provider 使用同一套上下文协议，差异由 `ImChannelCapabilities` 和 readiness 数据表达。
- 明确当前 Bifrost Provider ID、平台机器人身份、会话类型、精确目的地与主动发送 readiness。
- 列出 CLI 支持的全部内容输入形式，并逐项列出当前 provider 的 native、degraded、unsupported、`delivered_as` 和字节上限。
- 强制使用 `bifrost im send`；禁止改走 Lark IM、飞书 OpenAPI、微信 connector 等旁路。
- 明确普通最终回复会由 Gateway 自动投递，避免主动 send 造成重复消息。
- 发生疑问时引导 Runner 执行 help、capabilities 与 provider list，以当前 CLI 和运行时能力为准。
- 不把 secret、token、cookie、微信 context token 写进 prompt、历史或运行快照。

## 可信输入与路由

上下文只从服务端可信状态生成：

1. 当前 `ImEvent`；
2. 当前 `ImProviderConfig`；
3. `agent_reply_target_ref(provider, event)` 解析出的精确回信目的地；
4. `ImProviderClient::channel_capabilities` 的实时能力；
5. 对 `requires_context` provider 的发送 readiness。

路由规则与自动回复保持一致：

- 飞书优先使用事件 `chat_id`，类型为 `chat_id`；没有 chat 时回退 sender/owner `open_id`。
- Weixin、WeChat 和 Webhook 优先 chat，再 sender，再 owner，类型为 `open_id`。
- 群和 thread 仍绑定原 chat；上下文同时标明 conversation kind，不能让 Runner 自行替换目的地。

Provider ID、receive ID type、receive ID 和平台机器人 ID 必须通过严格的单行安全字符校验且长度不超过 256 字节。缺失或不安全时 fail-closed：不渲染可执行 send 命令，只保留不含危险值的诊断指引。

## 注入生命周期与顺序

动态上下文只放进当前 `ExternalCliRunRequest.instructions`，顺序为：

```text
Base（新会话首条）
Developer
User
Runner
Bifrost IM Outbound Context（每个真实 IM turn）
Channel Message
```

排队消息切换 `current_event` 后重新生成上下文，因此群聊、sender 或 thread 改变时不会复用旧路由。上下文不写回用户消息历史，也不影响 Chat Gateway 和 Schedule 的原样透传语义。

## 模板契约

模板必须包含：

- Provider ID、provider type、平台 bot identity；
- direct/group/thread、精确 `receive_id_type` 与 `receive_id`；
- conversation support 与每种内容 part 的实时能力；
- `ready`、`missing context` 或 `unsupported`；
- 文本、Markdown、图片、文件、原生卡片、快速卡片和视频映射；
- `--owner`、`--target`、飞书 `--chat-id`、generic direct 四类目的地说明；
- ready 时的 canonical 命令：

```bash
bifrost im send --provider '<provider_id>' \
  --receive-id-type '<type>' --receive-id '<id>' \
  <CONTENT_ARGS> --format json
```

- 诊断顺序：

```bash
bifrost im --help
bifrost im send --help
bifrost im provider capabilities '<provider_id>' --format json-pretty
bifrost im provider list
```

发送后必须检查 bundle 状态、每条 receipt、warning/error 与 `partial_success` 的失败项；失败时保留原 provider/target 并报告原始错误，不能声称成功。

## Provider 行为

- Feishu：当前支持 direct/group/thread；文本、Markdown、图片、文件和 native card 为 native。
- Weixin / WeChat：当前只支持 direct，主动发送依赖从入站消息安全持久化的 context token。token 只用于 provider 内部 readiness 和发送，绝不进入模板；文本、图片、文件和视频为 native，Markdown degraded 为文本，native card unsupported。
- Webhook：当前 client unsupported，不生成主动发送命令。
- 新 provider：只要实现 `channel_capabilities` 与发送 client，即可进入同一模板；不在模板代码中硬编码新的通道分支。

## 验证

- 单元测试覆盖 Feishu P2P、群、thread，Weixin/WeChat ready 与 missing-context，Webhook unsupported，危险标识 fail-closed，以及 secret 不泄漏。
- 组合函数测试保证动态上下文在 Runner Instructions 之后、消息之前，并保证旧的非 IM 调用保持不变。
- 隔离 E2E 使用 mock Feishu provider 与捕获 stdin 的 mock Runner，验证首轮/后续/群聊的精确 provider、bot、target、能力、help 指引和生命周期。
- `human_tests/im-gateway-external-cli-chat-gateway.md` 记录并执行同一真实二进制链路。
