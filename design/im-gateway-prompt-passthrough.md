# IM Gateway 外部 Runner 指令透传

> 状态：已实现；真实 IM 通道的可信动态路由例外见 `design/im-agent-outbound-context.md`
> 实现入口：`crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`、`crates/bifrost-admin/src/handlers/im_gateway/event_loop/external_runner.rs`、`crates/bifrost-admin/src/handlers/im_gateway/chat_gateway.rs`。

## 背景与问题

历史版本中，外部 Runner 的 Agent Base / Developer / User Instructions 与 Runner Instructions 都为空时，Bifrost 仍会在每一条消息前自动拼接静态 `Bifrost Tool Context`。这段内容不是用户配置，而是旧的 `injectBifrostTools` 开关触发的硬编码 prompt；内置 Runner 又默认把该开关设为开启，因此空配置不等于空注入。

这个行为同时带来两个问题：

- IM 或 Chat Gateway 实际收到的用户消息被隐式改写，且同一会话的每一轮都会重复注入。
- Agent 的 Base / Developer / User 三层配置没有按会话与消息层级完整接入外部 Runner。

## 用户目标验证清单

### 必须实现

- Agent 与 Runner 指令全部为空时，非 IM Chat Gateway / Schedule Runner 收到的正文必须是通道实际传入的消息，不增加 Bifrost 自带说明。真实 IM turn 会按 `design/im-agent-outbound-context.md` 追加可信、动态、会话绑定的外发上下文，但不会恢复静态工具说明。
- Base Instructions 只在新建外部 Runner 会话的首条消息中传入；同一会话后续消息不重复传入。
- Developer Instructions、User Instructions 与 Runner Instructions 按消息传入。
- 指令按 `Base（仅首条） -> Developer -> User -> Runner -> 可信通道上下文（仅真实 IM） -> 通道消息` 的顺序组合；空白项被忽略，不生成标题或占位文本。
- IM event loop 与 Chat Gateway 使用同一套组合规则。
- 旧配置中的 `injectBifrostTools` 不再触发硬编码 prompt。

### 必须不破坏

- 保留旧配置字段的反序列化兼容，避免已有配置或调用方因未知字段/缺字段失败。
- 不改写原始会话历史；历史仍记录通道实际传入的用户消息。
- 不改变 Runner 选择、工作目录、附件、Skill Paths、delivery mode 与外部线程恢复逻辑。
- 显式配置的指令仍能传给 Runner；只移除 Bifrost 自动合成的工具说明。

### 必须真实验证

- 使用捕获 stdin 的 mock Runner 验证非 IM 全空配置原样到达；真实 IM 全空配置只增加会话绑定的可信动态路由上下文，不恢复静态工具说明。
- 使用同一会话连续发送两条消息，验证 Base 只出现一次，其他三层每条都出现。
- 验证旧配置即使保存了 `injectBifrostTools: true`，也不会出现 `Bifrost Tool Context`。
- 在 Settings 页面验证遗留开关不再展示，Base 与消息级指令的生命周期说明准确。

### 必须交付

- 单元测试、E2E 脚本与 `human_tests/` 回归用例同步更新并真实执行。
- 完成至少两轮 Review/Fix/Test。
- 提交、推送、创建或更新 PR，并由远端 CI 的 coverage gate 兜底覆盖率门禁。

## 产品语义

外部 Runner 的最终输入分为会话级与消息级两层：

```text
非 IM 新会话首条：
  Base Instructions
  Developer Instructions
  User Instructions
  Runner Instructions
  Channel Message

非 IM 同一会话后续消息：
  Developer Instructions
  User Instructions
  Runner Instructions
  Channel Message
```

真实 IM turn 在 Runner Instructions 之后、Channel Message 之前额外加入动态 `Bifrost IM Outbound Context`，每一轮按当前 event 重新生成。

每一项都执行 trim 后判空。所有指令为空时，非 IM 调用不创建额外 section，最终 prompt 只包含原消息及 Runner 输入文件所需的结尾换行。真实 IM turn 的动态路由上下文是安全与正确投递所需的可信通道 metadata，不受遗留 `injectBifrostTools` 控制，也不写回历史。

`injectBifrostTools` 作为旧配置字段保留兼容读取，但语义废弃：

- 后端默认值改为 `false`。
- 配置加载或保存时把该字段归一化为 `false`。
- prompt builder 不再读取该字段生成文字。
- WebUI 不再展示或保存该开关。

## 实现方案

### 统一指令组合

在 external CLI 模块提供纯函数，输入：

- 是否为新会话；
- Base / Developer / User；
- 当前 Runner 或请求级 Instructions。

函数只拼接非空文本，返回 `Option<String>`。调用方把结果放进现有 `ExternalCliRunRequest.instructions`，避免在 adapter 内引入第二套层级协议。

### IM event loop

Provider 的有效 Agent 配置继续通过全局配置与 Provider override 合并。恢复会话后，通过当前 session 的用户消息数量判断本轮是否为新会话首条，再把有效 Agent 指令与 Runner 指令组合进本轮 settings。

消息写入历史时继续使用原始 `request.message`，不能把组合后的 prompt 回写历史。

### Chat Gateway

Chat Gateway 在 run 入队前读取相同的有效 Agent 配置。持久 session 尚不存在或没有用户消息时包含 Base；已有用户消息时只组合 Developer / User / Runner 指令。状态记录仍保存原始 message。

### Settings

- Agent 全局 Base 文案明确“仅新会话首条；为空不添加内容”。
- Developer / User 文案明确“每条消息；为空不添加内容”。
- Provider override 文案保留继承语义，并补充相同生命周期。
- Runner Instructions 文案明确“每条消息；为空时通道消息原样传入”。
- 删除 `Inject Bifrost Tools` 控件。

## 验证方案

### 单元测试

- `build_prompt`：所有 Instructions 为空且遗留开关为 `true` 时，不包含遗留静态工具上下文。
- 指令组合函数：首条完整顺序、后续不含 Base、全空返回 `None`、空白项被忽略；真实 IM 的可信动态上下文位于 Runner Instructions 之后。
- 配置归一化：任意版本的 `injectBifrostTools: true` 保存后为 `false`。

### E2E

新增 IM Gateway prompt passthrough 测试，使用独立数据目录与捕获 stdin 的 mock Runner：

1. 空 Agent / Runner 指令、遗留开关为 `true`，断言捕获内容只增加当前 IM turn 的动态路由上下文，不出现遗留静态工具说明。
2. 配置四层指令并连续发送两条同会话消息，断言首条包含 Base，第二条不含 Base，其他层每条存在且顺序正确。
3. 新群会话重新包含 Base，并精确切换到群 chat ID；每条消息均验证 provider、App ID、part capabilities、canonical send 与 help/capabilities 诊断指引。

### human_tests

在 `human_tests/im-gateway-external-cli-chat-gateway.md` 更新动态 IM 外发上下文与新会话 Base 生命周期回归用例，并同步相关索引行。文档更新后立即按步骤执行。

## 覆盖率与交付

本地执行 `make coverage-changed` 的 changed-lines 门禁，不运行高成本的全 workspace coverage；绝对 crate/workspace 覆盖率由远端 CI 的 `bash scripts/ci/coverage-all.sh --json --gate` 门禁验证。
