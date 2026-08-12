# Weixin Native Agent Experience 设计可执行性测试

## 功能模块说明

本文验证 `design/weixin-native-agent-experience.md` 是基于当前 `main` 和腾讯官方实现形成的可执行技术方案，而不是把目标能力误写成当前已落地能力。该文档任务不修改运行时代码，因此本轮验证设计事实、代码锚点、阶段依赖、测试闭环和 current/target 边界；真实微信功能用例将在对应实现 PR 中追加到 `human_tests/weixin-provider.md` 并逐条执行。

## 前置条件

- 位于从最新 `origin/main` 创建的独立 worktree。
- 调研附件可读：`/Users/eden/.codex/attachments/b729b60f-e8bc-4bed-b14c-41b36101f2cc/pasted-text.txt`。
- 可读取当前仓库的 Rust、design、E2E 和 human_tests 文件。
- 官方参考固定为 `Tencent/openclaw-weixin@cef0bfc390393f716903e16d50408118047f87e0`。

## 测试用例列表

### TC-WNAE-01：当前事实与目标能力没有混写

**操作步骤：**

1. 检查当前微信 capability、入站文件、poll timeout/cursor 和 Progress 选择代码。
2. 检查新设计的“已核验的当前事实”“目标与非目标”“发布与回滚”。
3. 确认设计未提前把 file/structured progress 宣称为当前可用。

**预期结果：**

- 当前代码仍返回微信 file unsupported、normalize 使用空 files、30 秒普通 HTTP timeout、Feishu-only Progress。
- 新设计明确区分 current 与 target，并要求实现合入后才切 capability。

**实际执行结果：**

- 2026-08-12 PASS：`weixin.rs:1269` 仍声明 generic file unsupported，`weixin.rs:482` 仍构造空 `files`，`weixin.rs:28` 仍是 30 秒普通 timeout，`external_runner.rs:64` 仍以 Feishu 类型选择 Progress。新设计第 1/3/4/15 节明确区分实施方案与当前能力，并要求代码合入后才切 capability。

### TC-WNAE-02：关键设计锚点存在且实施清单可定位

**操作步骤：**

1. 逐项确认 `provider.rs`、`types.rs`、`progress_card.rs`、`weixin.rs`、`service.rs`、`external_runner.rs`、Agent reply 和现有 Weixin E2E 文件存在。
2. 搜索 `ImProvider`、`ImSendCapabilities`、`ImAgentProgressRegistry`、`AgentTurnProgressEvent`、`resolve_external_cli_delivery_mode`、`resolve_event_files`、`upload_file/send_file`。
3. 对照设计第 11 节文件级清单，确认没有引用不存在的现有模块；标注为“新”的文件除外。

**预期结果：**

- 所有现有锚点均可定位。
- 新模块有明确职责和接入点，不依赖不存在的上层附件入口。

**实际执行结果：**

- 2026-08-12 PASS：所有 8 个现有文件均存在；`ImProvider`、`ImSendCapabilities`、`ImAgentProgressRegistry`、`AgentTurnProgressEvent`、delivery mode、`resolve_event_files`、`upload_file/send_file` 均通过 `rg` 定位。设计第 11 节只把 `progress.rs`、`weixin_progress.rs`、`weixin_sync_store.rs` 标为新文件。

### TC-WNAE-03：阶段依赖和验证门禁闭环

**操作步骤：**

1. 检查 PR1–PR6 是否给出依赖顺序和每阶段完成标准。
2. 检查单元测试、E2E、human_tests、`make coverage-changed`、远端 coverage gate 是否都有具体条目。
3. 检查 failure semantics、回滚和 capability 降级是否覆盖 Typing、Progress、媒体和 cursor。

**预期结果：**

- Reliability 在依赖其稳定性的 Progress/媒体之前。
- 每阶段可以独立 review、测试和回滚。
- 生产 Rust 实现明确受 changed-lines 95% 与 CI 90% 门禁约束。

**实际执行结果：**

- 2026-08-12 PASS：PR1–PR6 标题、`PR1 -> PR2 -> PR3 -> PR4 -> PR5` 依赖、单元/E2E/human_tests、`make coverage-changed` changed-lines 95%、`coverage-all.sh --json --gate`、`best_effort` 和 `TextOnly` 回退均通过固定字符串检查。

### TC-WNAE-04：调研纠偏与平台边界可核验

**操作步骤：**

1. 检查当前 `send_text_with_client_id` 是 full-first 还是预切分。
2. 检查本地 ASR 平台限制。
3. 对照设计确认长文本未被误列为 P0 bug，且无 transcript 语音未被无条件绑定本地 ASR。

**预期结果：**

- 当前实现是整段文本首发失败后才分片。
- 本地 ASR 明确仅支持 Apple Silicon macOS。
- 设计选择 transcript-first，无 transcript 先作为附件，符合跨平台落地边界。

**实际执行结果：**

- 2026-08-12 PASS（一次测试措辞修正后复跑）：`send_text_with_client_id` 先执行 `send_text_once(..., text, ...)`，错误分支才遍历 `split_text_messages_for_retry`；`SUPPORTED_ASR_TARGET = \"macos-aarch64\"` 且平台测试只接受 macOS aarch64。首次命令误查“完整文本首发”，而设计原文为“整段首发”，归类为测试断言措辞不一致；改用原文后复跑通过。设计保持 transcript-first，无 transcript 作为附件。

## 清理步骤

- 删除用于核验官方仓库的 `/tmp/openclaw-weixin.*` 临时目录。
- 本测试不启动 Bifrost、不写默认数据目录、不改变系统代理。
