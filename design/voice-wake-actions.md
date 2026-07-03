# Voice Wake Actions

## 背景

Voice Wake Actions 是 Bifrost 本机语音触发全局键盘指令的能力。产品目标不是暴露 profile/binding 等内部对象，而是提供一条短流程：用户输入或确认一个短唤醒词、录一次样本、绑定已有 ASR Speaker Diarization 声纹 profile、按下要触发的快捷键，然后 Start Listening；由 Bifrost 后台独立 worker 进程用 sherpa-onnx `KeywordSpotter` 持续监听并触发真实 `key_press` 动作。

首版落地为可测试 MVP：

- 本机存储 `VoiceWakeProfile`：作为唤醒动作的轻量配置壳，必须关联已有 ASR Speaker Diarization 声纹 profile，记录显示名、`voiceprint_profile_id`、`speaker_threshold`。
- 本机存储 `VoiceWakeBinding`：记录触发词、profile、KWS score/threshold 元数据、cooldown 和 `key_press` 动作。
- Admin API 与 CLI 支持 profile/binding 创建、列表、dry-run trigger、显式执行 key press、后台 listener 启停 / 进度 / 事件查询。
- WebUI 在 ASR 页面提供 Voice Wake Actions 面板：可编辑唤醒词、录入试听样本、用输入框直接捕获全局快捷键、保存动作、启动/停止后台监听、查看事件。WebUI 不展示 dry-run 概念；后台监听命中后真实执行。
- 浏览器仅用于一次性配置和样本采集；Start Listening 后由 Bifrost 主服务拉起独立 worker 进程完成麦克风采集、KWS、声纹校验、按键触发。页面关闭不影响后台 listener；主服务 stop 后 worker 必须跟随退出（父进程 PID 守护）。
- 声纹录入不在 Voice Wake 内重复实现：用户先用现有 Speaker Diarization 声纹录入模块创建 speaker profile，Voice Wake 只选择并绑定。

真实实现分布在 `crates/bifrost-admin/src/handlers/voice/{mod.rs,wake.rs}`（后端 ~3.4k 行、90+ 单测）、`crates/bifrost-cli/src/commands/voice.rs`（CLI ~1.3k 行）、`crates/bifrost-cli/src/commands/voice_wake_worker.rs`（worker ~1k 行）、`crates/bifrost-asr/src/wake.rs`（sherpa-onnx KWS bootstrap ~530 行）、`web/src/pages/ASR/components/VoiceWakeActionsCard.tsx`（~870 行）、`web/tests/ui/voice-wake-actions.spec.ts`、`e2e-tests/tests/test_voice_wake_actions.sh`。

## 用户目标验证清单

### 必须实现

- WebUI 提供录音入口，用户可以直接输入/修正唤醒词、选择已有声纹、捕获全局快捷键、保存 binding。
- Start Listening 后 Bifrost 主服务拉起独立 worker（`bifrost ai voice wake worker`，`hide = true` 的 CLI 子命令），worker 用本机麦克风 + sherpa-onnx KWS + 声纹 embedding 双门禁触发真实按键。
- CLI 支持一条完整配置路径 `bifrost ai voice wake bind-audio`：一次样本 + 已有声纹 profile + 手动 `--phrase` + 快捷键捕获，直接建立 profile + binding。
- CLI `listener start` 有 dry-run trigger 供自动化验证；WebUI 不暴露该路径。
- WebUI Voice command 开关和 Start Listening 使用同一门禁：没有已录声纹 profile 或没有 binding 时禁用并展示提示；调用后端失败要收敛 `Failed to fetch` 成明确「Bifrost 后台不可达」提示。
- 后台监听命中后：`KeywordSpotter` 命中 binding phrase **且** `identified_profile_id == VoiceWakeProfile.voiceprint_profile_id` **且** `confidence >= binding.speaker_threshold` 才执行 macOS `osascript` + System Events 按键；其他平台明确返回 unsupported。
- 拉起 worker 时携带父进程 PID；父进程消失后 worker 自行退出。

### 必须不破坏

- Speaker Diarization 声纹录入模块行为、`/api/asr/speaker-profiles` API、`speaker-profiles/*.json` 数据格式不变。
- sherpa-onnx KWS 资产初始化路径与其他 ASR 页面共享（不允许重复下载）。
- 系统代理不受影响；启动测试使用 `--no-system-proxy`。
- 用户不允许配置任意 shell 脚本，只允许 `key_press`，避免副作用扩散。
- 非 macOS 平台：CLI/API 不 panic，返回 unsupported。

### 必须真实验证

- CLI 子命令解析（`ai voice wake profile/binding/listener/trigger/bind-audio/worker`）由 `cli_commands.rs` clap 解析测试守护。
- 后端 `voice::wake` 90+ 单测覆盖 phrase 规范化、AppleScript 生成、profile-bound trigger、speaker 阈值、events 持久化。
- Playwright + E2E 覆盖 WebUI 门禁与 CLI listener 启动全链路。

## 产品语义

### Profile / Binding 数据模型

`VoiceWakeProfile`：轻量壳，字段包括 `id`（默认从 UUID 派生，也可用户指定）、`display_name`（缺省即 voiceprint id）、`voiceprint_profile_id`（必填，指向已存在的 Speaker Diarization profile）、`speaker_threshold`。

`VoiceWakeBinding`：`id`、`profile_id`、`phrase`（sherpa-onnx KWS keywords 归一化后的短词）、`kws_score / kws_threshold` 元数据、`cooldown_ms`、`action`（第一版只支持 `key_press`：`key` 或 `keycode` + `modifiers`）。

### 双门禁语义

listener 触发条件三选三 AND：

1. `KeywordSpotter` 命中 `binding.phrase`，`score >= kws_threshold`。
2. `identified_profile_id == VoiceWakeProfile.voiceprint_profile_id`（对触发音频计算 sherpa-onnx speaker embedding 后查已注册 profiles，`cosine similarity` 最大值命中）。
3. `confidence >= binding.speaker_threshold`。

任一失败：只记录事件（`RejectedKws` / `RejectedSpeaker` / `RejectedConfidence`），不执行按键。cooldown 内的重复命中同样只记录事件不重复触发。

### Engine 白名单

listener 默认且唯一使用 `lightweight_kws_listener`（sherpa-onnx KWS）。显式请求 `backend_asr_phrase_match` 等大模型作常驻关键词的选项被后端硬拒绝，避免长时间占用 GPU / 造成断流。CLI `listener start` 的 `--engine` 参数解析后校验白名单。

### 音频源

worker 默认 `--source mic`，macOS 使用 `ffmpeg -f avfoundation -i :0`（默认设备 0）。可通过 `--device` 覆盖。worker 内部维护一个滑动 ring buffer，KWS 命中后从中截取「wake window」用于声纹校验。

### 事件语义

listener 每次决策落一条事件到 `voice/wake/events.json`（截断到最新 N 条）：`Triggered / Executed / RejectedKws / RejectedSpeaker / RejectedConfidence / Cooldown / Error`。前端事件表和 `GET /api/voice/wake/events` 均读取同一事件流。

## 技术细节

### 数据落盘

```
BIFROST_DATA_DIR/voice/wake/actions.json     # profiles + bindings
BIFROST_DATA_DIR/voice/wake/events.json      # 事件流（滚动截断）
BIFROST_DATA_DIR/voice/wake/kws_state.json   # KWS 资产状态（首次 init 后写入）
```

### sherpa-onnx 对接边界

- KWS 文本侧：listener 只使用 sherpa-onnx `KeywordSpotter` 流式检测 binding phrase；不允许用 Qwen/大模型 ASR 做常驻关键词。
- Speaker Embedding：复用 ASR Speaker Diarization 已持久化的 `speaker-profiles/*.json`，直接调用 sherpa-onnx speaker embedding extractor。
- 资产初始化：`bifrost-asr::wake::ensure_kws_assets`（`crates/bifrost-asr/src/wake.rs`）负责下载/校验 KWS 模型文件；API `POST /api/voice/wake/kws/init` 与 `GET /api/voice/wake/kws/status` 暴露状态。

### Worker 进程守护

`voice_wake_worker::run_wake_worker` 是 `bifrost ai voice wake worker` CLI 子命令的实现体（`cli.rs:954` `#[command(hide = true)]`）：

- 启动时 `--parent-pid`（由主服务传入）；周期性检测父进程存活，父进程消失即退出。
- 通过 unix domain socket 或临时 HTTP 通道向 `POST /api/voice/wake/listener/progress` 汇报事件；主服务把事件持久化到 events.json。
- 采集链路：`ffmpeg -f avfoundation -i <device>` → PCM stream → sherpa-onnx `KeywordSpotter` → 触发时截 window → speaker embedding → 阈值判定 → `osascript` key press。

### 快捷键执行

macOS：`osascript -e 'tell application "System Events" to key code <keycode>'`，或 `keystroke "x" using {command down, shift down}`。需要 Accessibility 权限；无权限时 event `Error`。

其他平台：`unsupported` 明确返回，不做 stub 按键。

## CLI + Web + Admin API

### CLI

```bash
bifrost ai voice wake status
bifrost ai voice wake profile list
bifrost ai voice wake profile add --id wake_profile_eden --name Eden --voiceprint-profile-id speaker_eden
bifrost ai voice wake binding list
bifrost ai voice wake binding add --profile wake_profile_eden --phrase "打开录音" --key space --modifiers cmd
bifrost ai voice wake bind-audio ./wake.wav --voiceprint-profile-id speaker_eden --phrase "打开录音"
bifrost ai voice wake listener start [--source mic] [--device :0] [--engine lightweight_kws_listener] [--execute]
bifrost ai voice wake listener stop
bifrost ai voice wake trigger --profile wake_profile_eden --phrase "打开录音"
bifrost ai voice wake trigger --profile wake_profile_eden --phrase "打开录音" --execute
```

- `bind-audio` 若省略 `--key`，终端进入快捷键捕获模式（TTY 环境）；`--key` + `--modifiers` 明确传入时跳过捕获，适配 CI/无 TTY。
- `bind-audio` 不再从音频样本猜唤醒词，保存后的 `--phrase` 直接生成 sherpa-onnx KWS keywords。
- `worker` 为 hidden 子命令，用户不应手动执行。

### WebUI（`web/src/pages/ASR/components/VoiceWakeActionsCard.tsx`）

- 入口：`AI -> ASR -> Voice Wake Actions`（`web/src/pages/ASR/index.tsx` 挂载）。
- 组件包含：`Wake Audio`（浏览器 mic 录一次样本，仅试听，不做 ASR）→ `Wake phrase`（紧跟在录音下方，可编辑/修正）→ `Voiceprint`（选择现有声纹 profile；无声纹禁用保存）→ `Global shortcut`（只读输入框，聚焦后按键盘捕获组合键）→ `Voice command` 开关（Start/Stop backend listener）。
- 门禁：缺少声纹或未保存 binding 时开关禁用并展示提示。
- 事件表：调用 `GET /api/voice/wake/events`，展示 `Executed / Rejected*` 记录。
- Error 收敛：`fetch` 失败时把浏览器原始 `Failed to fetch` 转成 `Bifrost 后台不可达，请重启 Bifrost 后再录音`。

### Admin API

| Method | Path | 用途 |
| ------ | ---- | ---- |
| GET  | `/api/voice/wake/status` | listener enabled、profile/binding/events 数量 |
| GET  | `/api/voice/wake/kws/status` | KWS 资产状态 |
| POST | `/api/voice/wake/kws/init` | 触发 KWS 资产下载/校验 |
| GET  | `/api/voice/wake/profiles` | Profile 列表 |
| POST | `/api/voice/wake/profiles` | 创建 Profile |
| GET  | `/api/voice/wake/bindings` | Binding 列表 |
| POST | `/api/voice/wake/bindings` | 创建 Binding |
| POST | `/api/voice/wake/trigger` | dry-run 或 `--execute` 触发 |
| POST | `/api/voice/wake/listener/start` | 拉起 worker |
| POST | `/api/voice/wake/listener/progress` | worker 上报事件（内部） |
| POST | `/api/voice/wake/listener/stop` | 停止 worker |
| GET  | `/api/voice/wake/events` | 事件流 |

路由匹配集中在 `handlers/voice/wake.rs::305-320`。

## Sync 边界

Voice Wake profile/binding/events 属于本机 AI 能力，不参与规则/Group Sync，不上传到 Bifrost 云端，不通过分享链路传播。声纹本身也严格本地。

## Phase 1-4

### Phase 1：后端 profile / binding / trigger

- 定义 `VoiceWakeProfile / Binding`、落盘 `actions.json`、handler CRUD、dry-run trigger、AppleScript key press 生成。
- 单测覆盖 phrase 规范化、AppleScript 生成、profile-bound trigger 参数合法性。

### Phase 2：sherpa-onnx KWS 资产 + 双门禁

- `bifrost-asr::wake::ensure_kws_assets`、`kws/status`、`kws/init`。
- listener 引擎白名单、`KeywordSpotter` + speaker embedding 双门禁、events 落盘。

### Phase 3：Worker 独立进程 + CLI

- `voice_wake_worker` 隐藏子命令；父进程 PID 守护；ffmpeg 采集链路；`listener/progress` 汇报。
- CLI：`profile/binding/listener/trigger/bind-audio` 命令，clap 解析。
- CLI 解析测试 `cli_commands.rs` 保护 CLI 表面。

### Phase 4：WebUI 面板 + E2E + 文档

- `VoiceWakeActionsCard.tsx` + ASR 页面挂载 + 收敛 fetch 失败提示。
- `web/tests/ui/voice-wake-actions.spec.ts` 覆盖门禁、录音入口、快捷键捕获、事件表。
- `e2e-tests/tests/test_voice_wake_actions.sh` 覆盖 CLI 全链路 + engine 白名单 + 声纹拒绝。
- `human_tests/voice-wake-actions.md` + `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin voice::wake` 覆盖 90+ 用例：phrase 规范化、AppleScript 生成、profile-bound trigger、speaker 阈值、events 持久化、engine 白名单、cooldown。
- `cargo test -p bifrost-cli ai_voice_wake_commands_parse --test cli_commands` 覆盖 CLI 表面（`profile add/list`、`binding add/list`、`listener start/stop`、`trigger --execute`、`bind-audio`、`worker`）。
- `cargo test -p bifrost-cli voice_wake_worker --lib` 覆盖 worker 父进程守护、KWS 配置、`--device` / `--engine` 解析。
- `cargo test -p bifrost-asr wake::` 覆盖 KWS 资产 ensure/校验、断点续传、失败重试。

### E2E 测试

- `e2e-tests/tests/test_voice_wake_actions.sh`：
  - 临时 `BIFROST_DATA_DIR`；编译当前 worktree；`--unsafe-ssl --skip-cert-check --no-system-proxy` 启动。
  - 准备一份 ASR speaker voiceprint profile。
  - 未保存 voice command 前 `listener start` 被拒绝。
  - CLI `bind-audio --phrase` 从音频样本 + 已有声纹 profile + phrase 创建 profile & binding。
  - `listener start` 验证后台监听入口。
  - CLI dry-run trigger 命中事件。
  - 强制 `--engine backend_asr_phrase_match` 被拒绝。
  - 不同 speaker profile 触发被拒绝，不新增触发事件。
  - `GET /api/voice/wake/events` 查询事件断言。
- `web/tests/ui/voice-wake-actions.spec.ts`：
  - Voice Wake Actions 入口显示。
  - 录音入口、可编辑唤醒词、快捷键输入框捕获、Voice command 开关。
  - 无声纹/无 binding 时开关禁用并展示提示。
  - 开关 On 调用 listener start API。
  - 事件表显示 `Executed`。

### 真实场景测试 human_tests

`human_tests/voice-wake-actions.md`：

- 覆盖服务启动、已有声纹准备、CLI profile 创建、binding 创建、API events 查询。
- 覆盖 WebUI 录音入口、手动输入或修正唤醒词、选择已有声纹、快捷键绑定、Start Listening、后台监听触发真实执行。
- 覆盖 WebUI Voice command 开关启动门禁 + worker 独立进程状态。
- 覆盖主服务 stop 后 worker 跟随退出。
- 覆盖他人声纹或低 confidence 不触发动作。
- 可选真实 macOS key press：显式标注 Accessibility 权限与 `--execute` 风险。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：唤醒词录入、快捷键绑定、后台启动监听、说话触发真实按键是否可见且可测。
- 复核变更范围：`git status --short`、`git diff`；确认没有修改主 worktree。
- Review 后端 API、CLI 参数、安全边界（Accessibility 提示、engine 白名单、`key_press only`）、E2E、human_tests。
- 运行：`cargo test -p bifrost-admin voice::wake`、`cargo test -p bifrost-cli ai_voice_wake_commands_parse --test cli_commands`、`cargo test -p bifrost-cli voice_wake_worker --lib`、E2E 脚本。

### 第 2 轮

- 复查第 1 轮修复后的 diff。
- 复查 human_tests 索引、API/CLI 输出、WebUI 不暴露 dry-run、允许手动确认唤醒词、监听请求进入后台 KWS listener。
- 复跑受影响测试和最终启动验证。
- 校验：`cargo fmt --all -- --check`、相关 clippy、必要时 `cargo test --workspace --all-features`。

## 风险与决策

- **Accessibility 权限强依赖**：macOS 首次触发按键会弹权限对话；用户拒绝后无回退。事件流必须明确记录 `Error(permission denied)`，前端应展示指引。
- **worker 孤儿**：父进程崩溃或被 kill -9 时，worker PID 守护 poll 间隔期间可能短暂存活；已通过 poll 秒级间隔 + `Drop` 兜底缓解，但不能 100% 消除。生产环境建议依赖 systemd/launchd 兜底。
- **麦克风占用**：worker 常驻会占默认麦克风；其他 App 抢占时 ffmpeg 会失败。事件流应记录 `Error(device busy)`，UI 建议提示用户切换 `--device`。
- **KWS 误触**：单纯 KWS 阈值可能被路人声音触发，声纹校验是关键防线；若声纹 embedding 提取失败（音频过短/噪声），必须走 `RejectedSpeaker` 而不是 fallback 触发。
- **非 macOS 平台**：Windows/Linux 只支持 profile/binding 编辑与 dry-run，`--execute` 与 listener 硬失败。后续接入 Windows `SendInput` / Linux `xdotool` 需要重设 `key_press` 序列化格式。
- **未来扩展**：`action` 目前只 `key_press`；如引入 shell 或 URL scheme 触发，需要严格审计和显式确认，避免变成 RCE。
