# Weixin Native Agent Experience 设计与实现测试

## 功能模块说明

本文先验证 `design/weixin-native-agent-experience.md` 是基于当前 `main` 和腾讯官方实现形成的可执行技术方案，再验证实施分支的 Typing、结构化进度、统一 `run_id`、原生附件与 long-poll/cursor 可靠性。真实微信账号协作验证仍由 `human_tests/weixin-provider.md` 驱动。

## 前置条件

- 位于从最新 `origin/main` 创建的独立 worktree。
- 调研附件可读：`/Users/eden/.codex/attachments/b729b60f-e8bc-4bed-b14c-41b36101f2cc/pasted-text.txt`。
- 可读取当前仓库的 Rust、design、E2E 和 human_tests 文件。
- 官方参考固定为 `Tencent/openclaw-weixin@cef0bfc390393f716903e16d50408118047f87e0`。

## 测试用例列表

### TC-WNAE-01：当前事实与目标能力没有混写

**操作步骤：**

1. 检查设计第 3 节记录的 `main` 基线事实。
2. 检查第 1.1 节实施分支状态、代码 capability、入站文件、poll/cursor 和 Progress 选择。
3. 确认文档没有把尚未合入的分支能力宣称为生产 `main` 已可用。

**预期结果：**

- 第 3 节明确把 file unsupported、空 files、30 秒普通 timeout、Feishu-only Progress 标为 `main` 基线。
- 第 1.1 节只把 Native 能力标为实施分支已完成，并明确 PR 合入前生产事实仍以 `main` capability 为准。

**实际执行结果：**

- 2026-08-12 PASS：固定字符串检查确认第 3 节保留 `main` 基线，第 1.1 节明确列出实施分支与 PR #483，且声明合入前生产 `main` 仍以其 capability 为准。

### TC-WNAE-02：关键设计锚点存在且实施清单可定位

**操作步骤：**

1. 逐项确认 `provider.rs`、`types.rs`、`progress_card.rs`、`weixin.rs`、`service.rs`、`external_runner.rs`、Agent reply 和现有 Weixin E2E 文件存在。
2. 搜索 `ImProvider`、`ImSendCapabilities`、`ImAgentProgressRegistry`、`AgentTurnProgressEvent`、`resolve_external_cli_delivery_mode`、`resolve_event_files`、`upload_file/send_file`。
3. 对照设计第 11 节文件级清单，确认没有引用不存在的现有模块；标注为“新”的文件除外。

**预期结果：**

- 所有现有锚点均可定位。
- 实施分支新增模块有明确职责和接入点；设计明确记录 trait-object 方案由类型化 session map 取代，不依赖不存在的 `progress.rs`。

**实际执行结果：**

- 2026-08-12 PASS：逐个检查 provider/types/progress_card/weixin/service/external_runner/Agent reply/E2E 以及新增 `weixin_progress.rs`、`weixin_sync_store.rs` 均存在；`ImAgentProgressRegistry`、`AgentTurnProgressEvent`、`upload_file` 锚点可定位，代码未引用未落地的 `progress.rs`。

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

### TC-WNAE-05：微信原生进度和 final 附件端到端发送

**操作步骤：**

1. 构建当前分支的 debug `bifrost`。
2. 执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_weixin_provider_e2e.sh`。
3. 核对 mock 请求记录中的 Typing、type 11/12、final TEXT、IMAGE、FILE、VIDEO、client ID 和 `run_id`。
4. 使用请求携带的 AES key 解密 CDN upload，逐字节比对图片、Markdown 文档和 MP4 fixture。

**预期结果：**

- Typing 至少出现 start、keepalive、CANCEL，CANCEL 早于本 turn 的 final。
- 工具 type 11 早于 type 12，随后是 final；全部携带同一 `run_id`，client ID 不重复。
- 图片发 type 2，文档发 type 4，MIME 与 `ftyp` 签名一致的 MP4 发 type 5。
- 三种 CDN 密文解密后分别等于原始 fixture。

**实际执行结果：**

- 2026-08-12 PASS：使用最新 debug 二进制执行专项 E2E，观察到 Typing start/keepalive/CANCEL、type 11/12/final TEXT/IMAGE/FILE/VIDEO；本 turn 全部消息 `run_id` 唯一一致、client ID 无重复，CANCEL 早于 final。图片、Markdown 文档和 MP4 三个 CDN 密文经 AES-128-ECB 解密后逐字节等于 fixture。

### TC-WNAE-06：本地视频、音频和补丁链接进入附件链路

**操作步骤：**

1. 执行 `cargo test -p bifrost-admin agent_reply_collects_local_video_audio_and_patch_attachments_with_mime --lib --no-fail-fast`。
2. 核对 MP4、M4A、PATCH 三个本地 Markdown 链接均被收集为附件，MP4 MIME 为 `video/mp4`。
3. 核对微信分类单测：只有 MIME 与签名一致的视频进入 VIDEO，冲突时退回 FILE。

**预期结果：**

- 本地 final 不再把 `.mp4`、音频或补丁链接仅作为文本留在正文。
- 音频和补丁交给通用 FILE；视频必须通过 MIME + 签名双重检查。

**实际执行结果：**

- 2026-08-12 PASS：专项单测 1/1 通过，本地 MP4、M4A、PATCH 均进入附件列表，MP4 MIME 为 `video/mp4`；微信 33 个专项测试中的 MIME/签名匹配、VIDEO 上传与冲突退回 FILE 均通过。

### TC-WNAE-07：入站限制、cursor 与认证失效回归

**操作步骤：**

1. 执行 `cargo test -p bifrost-admin inbound_file_size_helpers_enforce_single_and_message_limits --lib --no-fail-fast`。
2. 执行 `cargo test -p bifrost-admin weixin_tests:: --lib --no-fail-fast`。
3. 核对单文件/总量边界、AES key 兼容、cursor 加密隔离、动态 timeout、`ret/errcode` 和 `-14` 状态测试。

**预期结果：**

- 非法 Base64、单文件超限和单消息总量超限均被拒绝或跳过，不进入 Runner。
- cursor 按 provider/account 隔离加密，入队后持久化并可在下一轮恢复。
- `-14` 映射 `authentication_required` 并停止旧 poll，不形成忙循环。

**实际执行结果：**

- 2026-08-12 PASS：入站限制 helper 单测 1/1、微信专项 33/33 全部通过，覆盖非法 Base64、单文件/总量边界、FILE AES 解密、hex/base64 key、动态 timeout、cursor 入队后持久化/隔离加密与 `-14 -> authentication_required` 不忙循环。

## 清理步骤

- 删除用于核验官方仓库的 `/tmp/openclaw-weixin.*` 临时目录。
- 自动化 E2E 使用临时 `BIFROST_DATA_DIR`、`--no-system-proxy`，退出时清理进程和临时目录。
