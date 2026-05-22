# ASR 实时语音输入与本地 Voice Input Runtime 方案

## 功能模块说明

本方案把现有 Qwen3-ASR 本地能力升级为 Bifrost 的统一语音输入底座。目标不是只在 ASR 工具页里增加一个录音按钮，而是建设一个可被 WebUI、CLI、后续输入法和 Agent 输入框复用的 `Voice Input Runtime`。

核心目标：

- 高性能：实时输入直接进入本机 stateful streaming ASR session，避免整段录完后再一次性识别；离线长音频继续用 ASR server 的批处理/分段转写链路。
- 低延迟：实时语音输入目标 first partial 小于 1 秒，静音后 final 小于 1.5 秒。
- 稳定可控：每个 session 有明确状态、背压、VAD、去重、取消、资源释放和错误事件。
- 本地原生部署：ASR 和默认后置处理都在本机运行，音频默认不出设备。
- 多输入通道：Web 录音和 CLI 本机音频监听都进入同一套后端管线。
- 后置大模型优化：ASR 原始输出和用户最终想输入的文本分层，支持词汇表、上下文和本地 LLM 改写。

非目标：

- 不接 DashScope、OpenAI、Gemini 或其它云端 ASR API。
- 不把原始音频上传到任何第三方服务。
- 不把系统音频捕获做成无权限提示的隐式行为。
- 不在 V1 中重写现有目录任务、长音频 30 秒窗口和已验证的 Qwen3-ASR 安装流程。

## 外部资料与可行性结论

### 开源 macOS 语音输入工具的实现共性

本轮重点调研了 TypeWhisper、FluidVoice、VocaMac、Pindrop、Whispur、OpenWhispr、OpenFlow 以及底层 WhisperKit / whisper.cpp。它们的主流实现并不是传统输入法优先，而是一个更容易落地的四段式架构：

```text
menu bar / helper app
  global hotkey
  microphone or system audio capture
  local or BYOK ASR provider
  paste / accessibility text insertion into focused app
```

共性结论：

- 常驻形态：绝大多数是菜单栏 App 或后台 helper，而不是纯 CLI。helper 负责生命周期、权限引导、热键和录音状态 UI。
- 热键形态：支持 push-to-talk、toggle 或 modifier-key hotkey。社区工具通常不依赖 macOS 当前输入法是否选中，热键在系统级生效。
- 录音形态：Mac 原生项目多用 AVAudioEngine / ScreenCaptureKit；跨平台项目常用 Tauri/Electron/Rust 录音层。Bifrost 当前 CLI 用 `ffmpeg avfoundation` 可做验证，但产品化 helper 应收敛到原生 Swift 音频采集。
- ASR 形态：本地优先工具通常使用 WhisperKit、whisper.cpp、Parakeet 或 Apple Speech；更重的工具抽象成多 engine/plugin，允许本地和云端 provider 切换。Bifrost 已有 Qwen3-ASR 服务，应把它接成 provider，而不是重写独立识别引擎。
- 文本注入形态：主流方案是剪贴板临时替换 + paste，或 Accessibility/CGEvent 模拟输入。真正使用 InputMethodKit 做输入源的项目较少，因为安装、TCC、签名和调试成本高，而且只有用户切到该输入法后才能自然接收文本事件。
- 后处理形态：成熟工具会保留 raw transcript，再做可配置 cleanup/workflow；TypeWhisper 这类工具还支持按应用/网站选择 profile。Bifrost 应保留 raw/refined 双轨，避免 LLM 改写覆盖证据。
- 集成面：TypeWhisper 和 OpenWhispr 都暴露 CLI/API/插件或 MCP 入口，说明语音输入能力不应只绑定 UI；Bifrost 的 Admin API + CLI + WebUI 正好适合沉淀成统一 Voice Runtime。

参考项目和资料：

- TypeWhisper: https://github.com/TypeWhisper/typewhisper-mac
- FluidVoice: https://github.com/altic-dev/FluidVoice
- VocaMac: https://github.com/jatinkrmalik/vocamac
- Pindrop: https://github.com/watzon/pindrop
- Whispur: https://github.com/sophiie-ai/whispur
- OpenWhispr: https://github.com/OpenWhispr/openwhispr
- OpenFlow: https://github.com/siamekanto19/openflow
- WhisperKit / Argmax OSS: https://github.com/argmaxinc/argmax-oss-swift
- whisper.cpp: https://github.com/ggml-org/whisper.cpp

对 Bifrost 的设计取舍：

- 用户当前目标要求一步到位做真正语音输入法，因此 V1 主路径就是 `Bifrost Voice.inputmethod`，不是“像输入法一样粘贴文本”的 helper-only 方案。
- Homebrew / 脚本负责安装输入法 bundle、helper 和 LaunchAgent；helper 只做输入法运行所需的热键、录音、权限和诊断支撑，不替代输入法。
- 当前光标位置写入必须优先走 InputMethodKit 的 marked text / commit text；剪贴板、Accessibility paste 或 CGEvent typing 只能作为非输入法 fallback，并且 UI/API 必须明确展示降级原因。
- ASR 服务由 Bifrost daemon 托管，helper 不直接管理模型二进制，避免多个入口重复拉起 Qwen3-ASR。

### Qwen3-ASR 模型与流式能力

Qwen 官方资料确认：

- Qwen3-ASR-1.7B 和 0.6B 支持 52 种语言和方言，支持 offline / streaming，同一模型可处理长音频、语音、歌声和带 BGM 歌曲。
- 官方 `qwen-asr` 包提供 transformers 与 vLLM 两个 backend；官方明确说明 streaming inference 目前只在 vLLM backend 可用，不支持 batch 和 timestamps。
- 官方 streaming example 使用 `init_streaming_state(...)`、循环 `streaming_transcribe(seg, state)`、最后 `finish_streaming_transcribe(state)` 的 stateful session 形态；参数包含 `chunk_size_sec`、`unfixed_chunk_num`、`unfixed_token_num` 和 `max_new_tokens`。
- Hugging Face 模型卡给出 streaming 与 offline 的公开 WER 对比：1.7B offline 平均 2.69，streaming 平均 3.33。说明 streaming 是官方支持路径，但质量略低于 offline。
- 技术报告和模型说明提到 context tokens / system prompt 可作为背景知识，帮助定制化 ASR 结果。这支持用户自定义词汇的方案，但不同 runtime 暴露程度不一致，必须通过 provider capability 管控。

参考：

- https://github.com/QwenLM/Qwen3-ASR
- https://huggingface.co/Qwen/Qwen3-ASR-1.7B
- https://github.com/QwenLM/Qwen3-ASR/blob/main/examples/example_qwen3_asr_vllm_streaming.py
- https://arxiv.org/abs/2601.21337

结论：

- 真流式、高性能路线可行，实时 Web/CLI 输入只保留 `qwen3_stateful_streaming` provider。
- 当前 Bifrost 默认不能把官方 vLLM 当作云服务使用；只能在本机部署 vLLM 或本机 Rust/MLX streaming runtime。
- 现有 `qwen3_asr_rs` 是更稳的 V1 底座，官方真流式是 V2 高性能目标。

### qwen3_asr_rs 本地运行能力

`second-state/qwen3_asr_rs` 已提供：

- `asr` 本地 CLI。
- `asr-server` 本地 OpenAI-compatible HTTP API server。
- 支持 Qwen3-ASR-0.6B 和 Qwen3-ASR-1.7B。
- macOS Apple Silicon 可用 MLX/Metal backend。
- `/v1/audio/transcriptions` 支持 multipart `file/language/response_format`。
- `/health` 和 `/v1/models` 可用于健康检查。
- 当前安装的 `asr-server --help` 和上游 README 只暴露 `--model-dir`、`--host`、`--port`、`--language`、`-v/-vv`。没有 `--stream`、session、chunk size 或 WebSocket/SSE 启动参数。
- Rust `qwen3_asr` crate 已出现 `StreamingOptions`，但这不是当前 `qwen3_asr_rs/asr-server` HTTP binary 暴露的运行时参数。若要本地真 stateful streaming，需要新 provider 直接集成该 crate，或 fork/扩展 `asr-server` 增加流式 endpoint。

参考：

- https://github.com/second-state/qwen3_asr_rs

结论：

- `qwen3_asr_rs/asr-server` 继续作为离线文件、目录任务和长音频处理的本地部署方案。
- 它的 HTTP server 是无状态转写接口，不是官方 stateful streaming session，因此不再作为 Web/CLI 实时语音输入 provider。
- 实时语音输入必须直接使用 Rust `qwen3-asr` crate 的 `StreamingState`，持续 feed PCM chunk，而不是在 Bifrost 上层用短窗口/overlap 模拟流式体验。
- `language` 应作为每次 `/v1/audio/transcriptions` 的请求参数透传；服务进程默认 language 只影响缺省值，不应阻止已启动的同模型服务被 Voice Runtime 复用。

### macOS 本机音频捕获

Apple 官方资料显示有两类原生能力：

- ScreenCaptureKit 的 `SCStreamConfiguration` 支持音频 capture 配置，包括 `capturesAudio`、`sampleRate`、`channelCount` 和 microphone capture device。
- Core Audio process taps 可捕获某个进程或一组进程的输出音频。Apple 文档要求 macOS 14.2+，并需要 `NSAudioCaptureUsageDescription` 权限说明。
- InputMethodKit 是 macOS 官方输入法框架，负责输入法 server、input controller、candidate window 和 client 应用通信；真实输入法模式需要按该框架注册输入源并实现 `IMKInputController`。
- Apple Speech `SFSpeechRecognizer` 可作为 Apple 原生识别 fallback，但官方文档明确识别服务可用性按语言变化，部分语言可能需要网络；因此它不能作为 Bifrost 的默认离线 ASR 承诺。

参考：

- https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration
- https://developer.apple.com/documentation/screencapturekit/capturing_screen_content_in_macos
- https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps
- https://developer.apple.com/documentation/InputMethodKit
- https://developer.apple.com/documentation/speech/sfspeechrecognizer

结论：

- CLI 监听麦克风、系统音频、应用音频在 macOS 上有原生可行路径。
- 系统音频和单应用音频的实现风险集中在权限、签名、entitlement、macOS 版本和 TCC 行为。方案必须先实现 source discovery 与能力探测，而不是假设所有机器都可捕获。
- V1 先支持 `mic` 和文件源回归；`system` 与 `app` 进入 Phase 2，并且必须在 source discovery 返回明确 capability 后再允许录制。
- 真正的 macOS 输入法集成可行，但应拆成独立 helper / input source 包，而不是由 `bifrost start` 主进程直接承担 TCC、AppKit runloop、热键和输入法 server。

## 当前 Bifrost 基线

现有能力：

- `~/.bifrost/asr` 固定安装目录。
- `crates/bifrost-admin/src/handlers/asr.rs` 提供 `/api/asr/status`、`/api/asr/init-stream`、`/api/asr/service/start`、`/api/asr/service/stop`、`/api/asr/transcribe-stream`、`/api/asr/transcribe-ws`。
- WebUI 文件上传路径按 30 秒窗口、2 秒 overlap 顺序送本地模型。
- CLI `bifrost ai asr stream-file` 对长音频按 30 秒窗口、2 秒 overlap 输出 JSON Lines。
- 浏览器麦克风 WebSocket 已存在，默认 1 秒窗口、300ms overlap；但当前实现会累积 MediaRecorder session bytes，每次 flush 重新转码整个会话，再截取新增窗口送模型。
- 现有设计 `design/asr-multi-model-local-service.md` 已提出统一 ASR 应用服务层和 provider adapter，不能在业务层为每个模型写旁路。

现有主要缺口：

- 麦克风实时链路不够高性能：重复转码完整会话，session 越长成本越高。
- 缺少 CLI 级音频监听命令。
- 缺少系统音频 / 应用音频来源选择。
- ASR 输出没有和“用户最终想输入的文本”分层。
- 用户词汇只停留在未来可能的 prompt/context，没有配置、注入、纠错和验证闭环。
- 隐私边界没有被作为 Voice Input Runtime 的硬约束写入。

## 目标架构

```text
Input Adapters
  WebMicInput
  CliMicInput
  CliSystemAudioInput
  CliAppAudioInput
        |
        v
Local Audio Pipeline
  source discovery
  permission/status probe
  16kHz mono PCM normalize
  stateful session buffer
  VAD / endpointing
  backpressure
        |
        v
ASR Provider Layer
  qwen3_stateful_streaming
  future local providers
        |
        v
Transcript Engine
  partial
  stable_delta
  final_utterance
  dedupe
  timestamps
        |
        v
Rewrite Engine
  local vocabulary
  local LLM preferred
  faithful rewrite policy
        |
        v
Output Adapters
  WebUI
  CLI stdout/jsonl/sse
  InputMethodKit input source
  macOS fallback text injection
  clipboard fallback
  Agent composer
```

关键原则：

- 所有入口只进入 `VoiceInputSession`，不直接调用模型 binary。
- 所有音频在进入模型前统一转成 16kHz mono PCM。
- 实时语音输入只使用 stateful streaming provider；`chunk_ms` / `stateful_chunk_sec` 仅作为模型 streaming chunk size，不触发 HTTP whole-file 窗口转写。
- 离线文件、目录任务和长录音继续使用 ASR server 的批处理/分段转写能力。
- ASR 原始文本和大模型优化文本同时保留，不能用优化文本覆盖调试证据。
- 本地 provider 绑定 `127.0.0.1` 或 Unix domain socket。禁止默认绑定 `0.0.0.0`。

## macOS 系统集成方案（真正输入法一步到位）

### 分发约束与结论

Bifrost 当前主要通过 Homebrew formula 或安装脚本分发，而不是 Mac App Store 或 signed DMG。但用户目标要求“真正的语音输入法”，所以 V1 必须交付 InputMethodKit 输入源，而不是只做全局热键 + paste。

可执行结论：

- CLI、daemon、ASR runtime、WebUI、Voice Runtime 可以通过 Homebrew formula / 脚本稳定交付。
- V1 语音输入法体验使用 `Bifrost Voice.inputmethod` 作为主入口，用户在 macOS 输入源里选择该输入法后使用。
- `bifrost-voice-helper` 作为输入法的伴随用户态进程，负责麦克风采集、热键、LaunchAgent、权限探测和 WebUI 诊断。
- 首次启用时仍必须引导用户授予 macOS 权限：Microphone、Accessibility、Input Monitoring。没有签名不能绕过这些授权；签名也不能静默授权。
- Homebrew / 脚本可以安装 `.inputmethod` bundle，但未签名 bundle 的系统加载和升级后权限稳定性弱。V1 安装器必须将该风险转化为可诊断状态，而不是静默失败。
- 如果团队要对外发布“安装后直接能用”的正式体验，应把 Homebrew 安装产物做 Developer ID 签名与 notarization；不上 Mac App Store 也可以签名公证。

### 安装产物

Homebrew formula / install script 安装以下文件：

```text
/opt/homebrew/bin/bifrost
/opt/homebrew/bin/bifrost-voice-helper
/opt/homebrew/opt/bifrost/share/bifrost/input-method/Bifrost Voice.inputmethod
/opt/homebrew/opt/bifrost/share/bifrost/launch-agents/com.bifrost.voice-helper.plist
~/.bifrost/voice/config.json
~/.bifrost/voice/runtime.json
```

脚本安装使用同样的逻辑路径，但可执行文件位于用户安装目录，例如：

```text
~/.bifrost/bin/bifrost
~/.bifrost/bin/bifrost-voice-helper
~/.bifrost/share/input-method/Bifrost Voice.inputmethod
~/.bifrost/share/launch-agents/com.bifrost.voice-helper.plist
```

稳定身份策略：

- Homebrew 下 helper 启动路径优先使用稳定 symlink：`/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper` 或 `/usr/local/opt/bifrost/bin/bifrost-voice-helper`。
- 如果 macOS TCC 对 `Cellar/<version>` 路径升级敏感，`bifrost ai voice ime setup` 可把 helper 复制到 `~/Library/Application Support/Bifrost/bin/bifrost-voice-helper`，由 LaunchAgent 固定从该路径启动。
- 输入法 bundle 安装到 `~/Library/Input Methods/Bifrost Voice.inputmethod`。setup 必须复制 bundle，而不是让系统从版本化 Cellar 路径直接加载。
- WebUI 必须显示当前 inputmethod bundle 路径、helper 实际路径、是否 signed/notarized、权限是否仍有效，以及最近一次权限探测时间。

### 进程架构

```text
bifrost daemon
  Admin API
  Voice Runtime
  ASR managed service
  config persistence
        ^
        | loopback WebSocket / HTTP + local admin token
        v
Bifrost Voice.inputmethod
  IMKServer
  IMKInputController per client session
  marked text / candidate / commit text
  edit feedback tracker
        ^
        | local IPC / loopback
        v
bifrost-voice-helper
  user LaunchAgent or foreground child process
  global hotkey
  permission probe
  mic capture
  selected input device
  lightweight status IPC
```

职责边界：

- `bifrost daemon`：负责模型、Voice session、ASR provider、词汇表、rewrite、学习数据、状态和资源策略。它不注册全局热键，不直接申请桌面权限，不操作当前焦点 App。
- `Bifrost Voice.inputmethod`：负责真正输入法生命周期。它通过 `IMKServer` 为每个 App 输入会话创建 controller，通过 input client 提交 marked/final text，并在输入法会话内跟踪用户对已提交文本的编辑反馈。
- `bifrost-voice-helper`：负责用户会话内的 macOS 桌面集成。它可以由 `launchctl bootstrap gui/<uid>` 常驻，也可以由 `bifrost ai voice ime helper start --foreground` 前台运行。
- `bifrost-voice-helper` 不直接启动模型二进制。它只向 Bifrost daemon 发起 Voice session，daemon 按资源策略启动或复用 ASR 服务。
- `Bifrost Voice.app` 不是 V1 必需产物；`Bifrost Voice.inputmethod` 是 V1 必需产物。

### 启动与配置

新增命令建议：

```bash
brew install bifrost
bifrost ai voice ime setup
bifrost ai voice ime status
bifrost ai voice ime repair
bifrost ai voice ime helper start
bifrost ai voice ime helper stop
bifrost start --voice-input --no-system-proxy
```

`bifrost ai voice ime setup` 必须是幂等流程：

1. 检查平台和 macOS 版本。
2. 检查 `bifrost-voice-helper` 是否存在。
3. 复制 `Bifrost Voice.inputmethod` 到 `~/Library/Input Methods/`。
4. 写入 `~/.bifrost/voice/config.json` 默认配置。
5. 安装或更新用户级 LaunchAgent。
6. 启动 helper 或提示用户前台运行。
7. 执行输入法 bundle probe、helper probe 和权限 probe。
8. 打开 WebUI Voice Input 页面或打印本机 URL。
9. 不自动启动 1.7B ASR，不自动下载大模型，CI 环境直接返回 guardrail 状态。

配置项落在 runtime settings 和 `~/.bifrost/voice/config.json`：

```json
{
  "voice_input": {
    "enabled": true,
    "model": "Qwen3-ASR-0.6B",
    "language": "chinese",
    "hotkey": "right-option",
    "hotkey_mode": "hold_to_dictate_revert_on_release",
    "toggle_cancel_behavior": "second_press_cancel_and_revert",
    "input_method_bundle": "~/Library/Input Methods/Bifrost Voice.inputmethod",
    "install_mode": "homebrew_formula",
    "helper_path": "/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper",
    "helper_launch_mode": "user_launch_agent",
    "startup_policy": "start_daemon_and_helper",
    "asr_start_policy": "on_first_use",
    "install_input_method": true,
    "output_strategy": "imk_marked_text_then_commit",
    "microphone_device": "system_default",
    "edit_feedback": {
      "enabled": true,
      "capture_window_seconds": 120,
      "min_changed_chars": 1,
      "store_audio_hash_only": true
    },
    "debug_audio_dump": false,
    "unsigned_permission_notice_ack": false
  }
}
```

策略说明：

- `startup_policy=start_daemon_and_helper`：启动 Bifrost 时只启动 daemon 和 helper，不立即 warm up ASR，避免用户开代理时无感知占用大量内存。
- `asr_start_policy=on_first_use`：用户首次按热键时启动或复用 ASR 服务。WebUI 可提供 `Start now`，但必须展示模型、预估资源和当前内存状态。
- 默认模型应使用 `Qwen3-ASR-0.6B`。`1.7B` 必须作为高精度选项，由用户显式选择；真实测试中已经出现 1.7B 内存压力事故，不能作为语音输入法默认自动拉起模型。
- `install_input_method=true`：V1 必须安装真正输入法。未签名环境下如果系统拒绝加载，setup/status 必须返回 `input_method_load_failed_unsigned_or_quarantined`、`input_method_not_enabled` 或等价可行动状态。
- `hotkey_mode=hold_to_dictate_revert_on_release`：按住热键开始语音输入；释放热键取消本次语音输入并撤销 marked text，不提交最终文本。另一个可选模式是 `toggle_to_dictate_second_press_cancel`：按一下开始，第二次按下取消并撤销。
- `microphone_device=system_default`：默认跟随系统输入设备；也可选择具体 USB/蓝牙/内置麦克风设备 ID。

### WebUI 入口

在 Settings -> Speech Converter 卡片下新增 `Voice Input` 区块：

- 开关：Enable Voice Input。
- 安装状态：formula/script、inputmethod bundle installed/enabled/loaded、helper path、LaunchAgent loaded、helper pid、signed/notarized/unsigned mode。
- 权限状态：Microphone、Accessibility、Input Monitoring。
- 运行状态：Bifrost daemon reachable、ASR service ready、Voice session active、last hotkey session。
- 热键设置：hold-to-dictate / toggle-to-dictate / long dictation；每个模式必须明确 cancel/revert 行为。
- 麦克风设备：System Default、Built-in、USB、Bluetooth；展示设备名称、UID、sample rate、channel count、connected/disconnected。
- 模型选择：0.6B 默认，1.7B 标注高内存。
- 学习设置：启用编辑反馈学习、保留时长、是否只保存音频 hash、是否允许本地词汇自动更新。
- 按钮：
  - Install / Repair Input Method。
  - Start / Stop Helper。
  - Repair LaunchAgent。
  - Re-check Permissions。
  - Open Microphone Settings。
  - Open Accessibility Settings。
  - Open Input Monitoring Settings。
  - Start ASR Service。
- 诊断：
  - 最近一次 hotkey session。
  - first partial latency。
  - insertion result。
  - ASR auto-start 是否被资源闸门拒绝。
  - 最近一次编辑反馈：raw/refined/committed/user_final diff 摘要。
  - helper stderr 摘要。
  - 升级后权限是否需要重新确认。

WebUI 开关不直接在浏览器里监听全局热键；它只写配置并调用 Admin API 管理 inputmethod/helper。全局热键可以由输入法 controller 持有，也可以由 helper 持有后转发给当前活动输入法 session。V1 必须以 IMK commit/marked text 为主路径，clipboard paste 只作为非输入法 fallback。

状态模型：

```json
{
  "enabled": true,
  "install_mode": "homebrew_formula",
  "signed": false,
  "input_method": {
    "status": "enabled",
    "bundle_path": "/Users/eden/Library/Input Methods/Bifrost Voice.inputmethod",
    "loaded": true,
    "active_input_source": true,
    "last_client_bundle_id": "com.apple.TextEdit"
  },
  "helper": {
    "status": "running",
    "path": "/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper",
    "pid": 12345,
    "launch_agent_loaded": true,
    "version": "0.9.0"
  },
  "permissions": {
    "microphone": "ready",
    "accessibility": "needs_permission",
    "input_monitoring": "unknown"
  },
  "asr": {
    "model": "Qwen3-ASR-0.6B",
    "start_policy": "on_first_use",
    "ready": false,
    "resource_guard": "not_evaluated"
  },
  "microphones": {
    "selected": "system_default",
    "devices": [
      {
        "id": "system_default",
        "name": "System Default",
        "kind": "system_default",
        "status": "ready"
      },
      {
        "id": "coreaudio:BuiltInMicrophoneDevice",
        "name": "MacBook Pro Microphone",
        "kind": "built_in",
        "status": "ready"
      }
    ]
  },
  "last_error": {
    "code": "needs_accessibility_permission",
    "message": "Grant Accessibility permission to bifrost-voice-helper."
  }
}
```

### Helper 运行时链路

快捷键输入链路：

```text
user selects Bifrost Voice input source
  IMKServer creates IMKInputController for focused text client
  user presses configured hotkey
  inputmethod/helper validates permissions and daemon reachability
  helper captures PCM frames from configured microphone
  helper opens /api/voice/listen-ws
  daemon starts/reuses ASR service according to policy
  daemon emits partial/stable/final events
  inputmethod sets marked text for draft
  inputmethod commits final/refined text or reverts on cancel
  inputmethod tracks post-commit edits while session remains observable
```

文本注入优先级：

1. InputMethodKit selected mode：通过 input controller client 提交 marked/final text，保留候选窗能力。
2. Accessibility paste mode：非输入法 fallback。保存当前剪贴板，写入目标文本，模拟 paste，短延迟后恢复剪贴板。
3. CGEvent typing mode：用于剪贴板受限 App 的 fallback；速度较慢，需明确记录。
4. Clipboard-only：权限不足时至少把 final text 放入剪贴板并提示用户。

失败状态必须可行动：

- `helper_not_installed`
- `helper_not_running`
- `needs_microphone_permission`
- `needs_accessibility_permission`
- `needs_input_monitoring_permission`
- `input_method_not_installed`
- `input_method_not_enabled`
- `input_method_client_unobservable`
- `asr_service_not_ready`
- `asr_resource_guard_rejected`
- `target_app_insertion_failed`
- `unsigned_helper_permission_reset`
- `launch_agent_not_loaded`
- `helper_path_changed_after_upgrade`

### LaunchAgent 策略

`bifrost ai voice ime setup` 安装输入法 bundle，并为配套 helper 安装用户级 LaunchAgent，不使用 root daemon：

```xml
<key>Label</key>
<string>com.bifrost.voice-helper</string>
<key>ProgramArguments</key>
<array>
  <string>/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper</string>
  <string>run</string>
  <string>--config</string>
  <string>/Users/<user>/.bifrost/voice/config.json</string>
</array>
<key>RunAtLoad</key>
<true/>
<key>KeepAlive</key>
<false/>
```

关键约束：

- 只安装到当前用户的 `~/Library/LaunchAgents/com.bifrost.voice-helper.plist`。
- 不修改系统级 LaunchDaemons。
- plist 写入前必须把 `~` 展开为绝对路径，`ProgramArguments` 中不得依赖 shell 展开。
- `setup` / `repair` 必须先 unload 旧 plist，再 load 新 plist。
- `status` 必须能区分 plist 存在、LaunchAgent loaded、helper process running、helper API reachable 四个状态。
- 升级后必须触发 `helper_path_changed_after_upgrade` 检查，并提示用户重新确认权限。

### InputMethodKit 输入法策略

V1 输入法 bundle 是必须交付项：

- bundle id：`com.bifrost.voice.inputmethod`。
- 安装路径：`~/Library/Input Methods/Bifrost Voice.inputmethod`。
- 主进程创建 `IMKServer`。
- 每个文本 client session 创建 `BifrostVoiceInputController`。
- controller 维护当前 `client_bundle_id`、`selectedRange`、`markedRange`、`committedRange`、`session_id`。
- partial transcript 使用 marked text 展示；final/refined text commit 到 client。
- cancel/revert 必须删除本次 marked text 或回滚本次 commit 范围内的文本；如果 client 不支持可靠替换，则返回 `input_method_client_unobservable` 并降级为“需要手动撤销”提示。

编辑反馈学习只在可观察边界内执行：

```text
onCommit:
  snapshot committed text, range, selectedRange, surrounding text before/after
  store raw_asr_text, refined_text, committed_text, audio_hash, app bundle id

onKeyEvent / selectedRangeChanged / nextHotkey:
  query surrounding text through input client when available
  diff committed_text vs current text around committedRange
  produce edit operations: delete / insert / replace / move-cursor
  store user_final_text and feedback confidence

onLearn:
  if replacement is stable and local-only policy allows:
    add or update voice vocabulary alias
    add rewrite correction example
    never retrain acoustic model in V1
```

学习数据 schema：

```json
{
  "id": "voice-feedback-...",
  "created_at_ms": 1760000000000,
  "app_bundle_id": "com.apple.TextEdit",
  "input_source": "Bifrost Voice",
  "session_id": "voice-session-...",
  "audio_hash": "sha256:...",
  "microphone_device_id": "coreaudio:BuiltInMicrophoneDevice",
  "raw_asr_text": "白 frost 代理",
  "refined_text": "Bifrost 代理",
  "committed_text": "Bifrost 代理",
  "user_final_text": "Bifrost 代理服务",
  "edit_ops": [
    {"type": "insert", "offset": 10, "text": "服务"}
  ],
  "learned_aliases": [
    {"heard": "白 frost", "canonical": "Bifrost", "confidence": 0.92}
  ],
  "privacy": {
    "audio_persisted": false,
    "audio_hash_only": true,
    "local_only": true
  }
}
```

边界：

- 如果目标 App 不支持周边文本读取，Bifrost 只能记录 commit 结果，不能声称完整监控用户修改。
- 如果用户切走输入法或焦点离开文本输入框，反馈 tracker 进入 `lost_focus`，只在下一次激活时尝试采样上下文。
- 反馈学习默认只更新本地 vocabulary/rewrite examples，不训练 ASR 声学模型。

### 热键与取消语义

必须支持三种模式：

| 模式 | 触发 | 提交 | 取消/撤销 |
| --- | --- | --- | --- |
| `hold_to_dictate_revert_on_release` | 按住热键开始 | 只显示 marked text，不自动 commit | 松开热键取消并撤销本次 marked text |
| `hold_to_dictate_commit_on_release` | 按住热键开始 | 松开热键提交 final/refined text | `Esc` 或二级取消热键撤销 |
| `toggle_to_dictate_second_press_cancel` | 按一下开始 | 说完后由静音/VAD 或 Enter 提交 | 再按一次热键取消并撤销本次 marked/commit |

实现要求：

- 所有模式都必须把本次输入绑定到 `voice_input_session_id`。
- cancel 必须先停止音频采集，再 cancel Voice WebSocket，然后回滚 marked/committed range。
- 若已 commit 且 client 不支持可靠替换，必须返回 `cancel_revert_unavailable` 并提示用户使用系统 Undo。

### 麦克风设备选择

设备枚举由 helper 提供，daemon 只保存配置和展示状态：

- `system_default`：默认，跟随 macOS 当前 Sound Input。
- `coreaudio:<uid>`：具体 Core Audio 设备 UID，适用于 USB/蓝牙/内置麦克风。
- 每个设备展示 name、uid、transport、sample rates、channel count、connected、is_default。
- 设备断开时自动回退 `system_default`，同时产生 `microphone_device_disconnected` 事件。
- 采集启动前必须验证设备仍存在；不存在时不能悄悄切换到其它具体设备，除非配置为 `fallback_to_system_default=true`。

API：

```text
GET  /api/voice/microphones
PUT  /api/voice/microphones/selection
POST /api/voice/microphones/recheck
```

示例：

```json
{
  "selected": "system_default",
  "devices": [
    {"id":"system_default","name":"System Default","kind":"system_default","status":"ready"},
    {"id":"coreaudio:AppleUSBAudioEngine:...","name":"USB Microphone","kind":"usb","status":"ready"},
    {"id":"coreaudio:Bluetooth:...","name":"AirPods Pro Microphone","kind":"bluetooth","status":"ready"}
  ]
}
```

### 权限策略

V1 不承诺无权限自动可用。权限处理必须显式、可诊断：

- Microphone：用于录音。CLI 前台模式可能归属 Terminal/iTerm；LaunchAgent/helper 模式归属 helper binary。状态必须显示实际被授权的执行主体。
- Accessibility：用于 paste/CGEvent 注入。没有该权限时不得伪装成功；可以降级为 clipboard-only。
- Input Monitoring：用于全局热键和部分按键监听。没有该权限时 helper 可退化为手动命令触发或 WebUI `Start Listening`。
- Screen Recording / system audio：仅 Phase 2 系统音频或应用音频捕获需要。

WebUI 必须提供每项权限的 `Open Settings` 和 `Re-check`，并展示 unsigned helper 说明：

```text
Unsigned Homebrew helper: macOS may ask you to re-approve permissions after upgrades.
```

### 签名与加载策略

真正输入法要求系统加载 `.inputmethod` bundle。Homebrew / 脚本可以复制 bundle，但不能绕过 macOS 的 Gatekeeper、TCC 和输入源启用流程。

V1 要求：

- `bifrost ai voice ime setup` 必须清理 quarantine 或返回明确 `input_method_quarantined` 指引。
- `status` 必须区分 `installed`、`not_enabled_in_system_settings`、`enabled_but_not_active`、`loaded`、`client_attached`。
- 未签名可作为 developer/internal distribution 跑通，但正式公开分发要优先做 Developer ID 签名与 notarization；不上 Mac App Store 也可以做 Developer ID。
- 如果系统拒绝加载，不能回退成“假输入法”；只能提示用户修复签名/公证/权限或进入 fallback paste mode。

## 服务协议

### Admin API

新增 Voice Input API，不替代现有 `/api/asr/*`，而是在其上方提供统一入口：

```text
GET  /api/voice/sources
GET  /api/voice/status
GET  /api/voice/ime/status
POST /api/voice/ime/setup
POST /api/voice/ime/repair
POST /api/voice/ime/activate-guide
GET  /api/voice/helper/status
POST /api/voice/helper/start
POST /api/voice/helper/stop
POST /api/voice/helper/recheck-permissions
GET  /api/voice/microphones
PUT  /api/voice/microphones/selection
POST /api/voice/microphones/recheck
GET  /api/voice/feedback
POST /api/voice/feedback
POST /api/voice/feedback/<feedback_id>/apply
POST /api/voice/sessions
GET  /api/voice/sessions/<session_id>/events
POST /api/voice/sessions/<session_id>/audio
POST /api/voice/sessions/<session_id>/finish
POST /api/voice/sessions/<session_id>/cancel
GET  /api/voice/vocabulary
PUT  /api/voice/vocabulary
```

IME status response:

```json
{
  "enabled": true,
  "install_mode": "homebrew_formula",
  "signed": false,
  "input_method": {
    "status": "loaded",
    "bundle_path": "/Users/eden/Library/Input Methods/Bifrost Voice.inputmethod",
    "bundle_id": "com.bifrost.voice.inputmethod",
    "enabled_in_system_settings": true,
    "active_input_source": true,
    "client_attached": true,
    "last_client_bundle_id": "com.apple.TextEdit"
  },
  "helper": {
    "status": "running",
    "path": "/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper",
    "pid": 12345,
    "launch_agent_installed": true,
    "launch_agent_loaded": true,
    "api_reachable": true,
    "version": "0.9.0"
  },
  "permissions": {
    "microphone": {
      "status": "ready",
      "subject": "/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper"
    },
    "accessibility": {
      "status": "needs_permission",
      "subject": "/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper"
    },
    "input_monitoring": {
      "status": "unknown",
      "subject": "/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper"
    }
  },
  "actions": [
    "open_microphone_settings",
    "open_accessibility_settings",
    "recheck_permissions",
    "repair_launch_agent"
  ],
  "warnings": [
    "unsigned_helper_permissions_may_reset_after_upgrade"
  ]
}
```

API 行为要求：

- `ime/setup` 幂等：重复调用不得创建重复 LaunchAgent，不得重置用户热键配置，不得覆盖用户已选择的麦克风。
- `ime/setup` 负责复制 inputmethod bundle、安装/修复 LaunchAgent、执行权限和输入源状态 probe。
- `start` 只启动 helper，不启动 1.7B ASR。
- `repair` 修复 helper path 与 LaunchAgent；如果 helper 二进制路径变化，返回 `helper_path_changed_after_upgrade` 并要求 re-check 权限。
- `recheck-permissions` 不弹出录音任务，不发送音频，只执行权限探测和可行动状态刷新。
- 所有 helper API 在非 macOS 返回 `unsupported`，但仍允许 CLI/file source 的 Voice Runtime 测试。
- `feedback/apply` 只能更新本地 vocabulary/rewrite examples；不得自动上传音频或云端训练。

Web 实时输入优先使用 WebSocket：

```text
GET /api/voice/listen-ws?source=web_mic&model=Qwen3-ASR-0.6B&provider=qwen3_stateful_streaming&language=chinese&chunk_ms=1000
GET /api/voice/listen-ws?source=web_mic&model=Qwen3-ASR-1.7B&provider=qwen3_stateful_streaming&language=chinese&chunk_ms=1000&allow_stateful_17b=1
```

WebSocket 客户端消息：

```json
{"type":"start","source":"web_mic","sample_rate":16000,"channels":1,"format":"pcm_s16le"}
<binary pcm_s16le audio frames>
{"type":"flush"}
{"type":"finish"}
{"type":"cancel"}
```

实现约束：

- WebUI ASR 工具页的 `Start Mic` 不再连接 `/api/asr/transcribe-ws`，只连接 `/api/voice/listen-ws`。
- 浏览器端使用 WebAudio 采集 16kHz mono PCM16 chunk，优先用 `AudioWorklet` 在音频线程编码，旧浏览器才 fallback 到 `ScriptProcessorNode`；不能用 `MediaRecorder` 发送 `audio/webm` 给实时 ASR。
- Web realtime 默认模型为 `Qwen3-ASR-0.6B`；用户在 Speech 设置中显式选择 `Qwen3-ASR-1.7B` 时，WebSocket URL 必须携带 `allow_stateful_17b=1`。
- 文件上传和目录任务仍走 `/api/asr/transcribe-stream` / ASR server，不复用 Voice realtime session。

服务端事件：

```json
{"type":"connected","session_id":"...","source":"web_mic"}
{"type":"source_ready","session_id":"...","source":"web_mic"}
{"type":"asr_partial","text":"...","delta":"...","window_index":0,"captured_at_ms":1010,"emitted_at_ms":1480,"inference_ms":470,"detail":"provider=qwen3_stateful_streaming; language=chinese"}
{"type":"asr_stable_delta","delta":"...","committed":"...","window_index":0}
{"type":"asr_final_utterance","text":"...","start_ms":0,"end_ms":3200}
{"type":"rewrite_partial","text":"..."}
{"type":"rewrite_final","text":"...","raw_text":"..."}
{"type":"error","message":"...","detail":"..."}
{"type":"done"}
```

### CLI

新增 `bifrost ai voice` 命令族：

```bash
bifrost ai voice sources
bifrost ai voice ime setup
bifrost ai voice ime status
bifrost ai voice ime repair
bifrost ai voice ime microphones
bifrost ai voice ime microphones set --device system_default
bifrost ai voice ime microphones set --device coreaudio:<uid>
bifrost ai voice listen --source mic --model Qwen3-ASR-0.6B --chunk-ms 1000 --format jsonl
bifrost ai voice listen --source file --input-file ./sample.wav --duration 7 --model Qwen3-ASR-0.6B --chunk-ms 1000 --format jsonl
bifrost ai voice listen --source file --input-file ./sample.wav --duration 7 --model Qwen3-ASR-1.7B --allow-stateful-large-model --format jsonl
bifrost ai voice listen --source system --format jsonl
bifrost ai voice listen --source app --app "Zoom" --format jsonl
bifrost ai voice listen --source mic --dry-run --text "请打开宽增"
bifrost ai voice vocabulary list
bifrost ai voice vocabulary import ./terms.txt
bifrost ai voice feedback list
bifrost ai voice feedback apply <feedback_id>
```

输出模式：

- `--format text`：只输出服务端返回的稳定 delta。
- `--format jsonl`：逐行透传本机 Voice service 返回的 `connected/source_ready/asr_partial/asr_stable_delta/asr_final_utterance/done`。
- `--provider`：实时输入只接受 `qwen3_stateful_streaming`；历史 ASR server 窗口式 provider 已从 Voice realtime 链路移除。
- `--allow-stateful-large-model`：显式允许 stateful realtime 链路加载 `Qwen3-ASR-1.7B`，默认关闭以保护低资源机器。
- `--format sse`：给其它本地进程订阅。
- `--output clipboard`：仅作为非输入法 fallback；真正输入场景必须走 IMK marked/commit。
- `ime microphones set --device system_default`：默认跟随系统输入设备。
- `ime microphones set --device coreaudio:<uid>`：固定使用指定 USB/蓝牙/内置输入设备。

CLI source discovery 输出示例：

```json
{
  "platform": "macos",
  "sources": [
    {"id": "mic:default", "kind": "mic", "status": "ready"},
    {"id": "system:default", "kind": "system", "status": "needs_permission"},
    {"id": "app:com.apple.Music", "kind": "app", "status": "unsupported", "reason": "requires macOS 14.2+ Core Audio tap"}
  ]
}
```

## Stateful Streaming 推流策略

### 实时语音输入

实时语音输入不使用 ASR server 的 HTTP whole-file 窗口，也不保留 `qwen3_rs_http_chunked` 伪流式 provider。

默认参数：

```text
capture_frame_ms = 100
stateful_chunk_ms = 1000
min_commit_ms = 300
vad_silence_ms = 800
max_session_buffer_ms = 120000
```

流程：

1. 输入 adapter 持续产出 PCM frame。
2. Voice WebSocket session 收到 `start` 后拉起独立 `bifrost ai voice worker` 子进程，worker 内部初始化 `StreamingState`。
3. 每个 PCM chunk 通过 stdio 发送给 worker，由 worker 调用 `feed_audio(&mut StreamingState, samples)`。
4. provider 在模型内部达到 `chunk_size_sec` 或 finish 时返回 partial/final 文本。
5. transcript engine 做 suffix/prefix 去重；`asr_partial` 只更新 volatile hypothesis，不进入 committed transcript。
6. VAD 检测到静音、用户 finish/stop，或连续说话达到最长 utterance 时，daemon 调用 worker finish 并把当前 partial 作为 stable/final delta commit。
7. session finish/cancel/disconnect 后关闭 worker；长时间纯静音或 WebSocket 无音频输入时卸载/关闭 worker，避免模型常驻 daemon。

`qwen3_stateful_streaming` provider：

- 基于 Rust `qwen3-asr` crate，本机独立 worker 子进程加载 Qwen3-ASR，不走云端 API。
- Bifrost daemon 不持有 model engine cache；每个 Voice WebSocket session 单独拉起 worker，worker 持有模型和 `StreamingState`。
- session start 后 worker 调用 `AsrInference::init_streaming(StreamingOptions)`。
- 每个 16kHz mono PCM chunk 转成 f32 samples 后由 worker 调用 `feed_audio(&mut StreamingState, samples)`；provider 内部按 `chunk_size_sec` 聚合，未满足窗口时返回 `None`。
- finish 时 worker 调用 `finish_streaming(&mut StreamingState)`，输出 final ASR 文本。
- 0.6B 和 1.7B 使用同一套 stateful streaming 技术路径；两者都通过 `feed_audio(&mut StreamingState, samples)` 流式喂 PCM。
- 0.6B 是默认真实流式验证模型；1.7B 需要 CLI 显式传 `--allow-stateful-large-model`、Web/WS 显式传 `allow_stateful_17b=1`，或设置 `BIFROST_VOICE_ALLOW_STATEFUL_17B=1` 才允许 stateful provider 拉起，避免在低资源机器上再次触发内存压力事故。
- 适合高性能低延迟路线；代价是每个活跃 realtime session 会占用独立 worker 模型资源，因此必须通过 session 生命周期、静音提交、最长 utterance 和 idle unload 控制资源占用。

### 文件与目录任务

保持现状：

- 30 秒窗口。
- 2 秒 overlap。
- timeline segment 最大 30 秒。
- 目录任务继续使用现有资源保护、memory guard 和 fallback。

## 隐私与安全边界

硬约束：

- ASR 默认只能使用本地 provider。
- 音频默认不出设备。
- 原始音频默认不落盘。
- Debug 保存音频必须显式开启，UI/CLI 必须显示保存路径和清理命令。
- 云端或远端 ASR 不作为产品能力，禁止接入。未来如支持远端 LLM，也只能处理文本，必须显式 opt-in，且不在本方案 V1 范围内。
- provider 服务默认只监听 `127.0.0.1` 或 Unix domain socket。
- 日志不得打印原始音频，不得打印长段隐私文本；只允许短摘要、长度、时长、窗口号、错误码和资源指标。
- 系统音频和应用音频必须先展示权限状态，不得静默捕获。

下载边界：

- 首次安装模型和 runtime 可以访问 GitHub/Hugging Face/ModelScope。
- 下载完成后支持离线运行。
- CI 默认禁止下载大模型，沿用已有 `BIFROST_QWEN3_ASR_E2E_ONLINE` / CI guard 思路。

## 后置大模型处理

ASR 输出不是最终用户输入。Voice Input Runtime 应维护两条轨：

```text
raw_asr_text      模型原始识别，用于调试、回放和对照
refined_text      后置模型优化后的用户意图文本
```

后置处理目标：

- 修正同音错词、项目术语、标点和格式。
- 删除口头填充词。
- 根据当前输入场景调整风格。
- 保持忠实，不扩写事实。

触发策略：

- `asr_partial` 不触发 LLM。
- `asr_stable_delta` 累积到短语级别后可触发轻量增量改写。
- `asr_final_utterance` 触发一次强改写。
- 输入法场景先展示 raw draft，静音后替换为 refined text。

本地优先：

- 默认使用本地 LLM provider。
- 如果用户配置了远端 LLM，只能处理文本，不得发送原始音频。
- 远端 LLM 必须在 UI/CLI 中明确显示隐私提示和 opt-in 状态。

## 自定义词汇

新增 voice vocabulary store：

```json
{
  "version": 1,
  "profiles": [
    {
      "id": "default",
      "terms": [
        {
          "canonical": "Bifrost",
          "aliases": ["白 Frost", "比 frost", "宽增"],
          "category": "project",
          "weight": 1.0,
          "rewrite": true,
          "asr_context": true
        }
      ]
    }
  ]
}
```

词汇使用三层策略：

1. ASR context：provider 支持时，把高权重词汇注入 context / initial_text / prompt。
2. 轻量后处理：对 ASR stable delta 做别名替换和置信度安全纠错。
3. LLM rewrite：把词汇表和当前上下文传给后置本地模型，要求忠实改写。

Provider capability：

```text
supports_asr_context: true | false
supports_initial_text: true | false
supports_stateful_streaming: true | false
supports_timestamps: true | false
```

`qwen3_stateful_streaming` 先支持轻量后处理和 LLM rewrite；如果 `initial_text` / context 被 runtime 忽略，则不能假装 ASR context 生效。

## 改动范围

### Rust 后端

- 新增 `crates/bifrost-admin/src/handlers/voice.rs`。
- 新增 `crates/bifrost-admin/src/handlers/voice_stateful.rs`，封装 `qwen3-asr` crate 的 engine cache、session `StreamingState`、PCM16LE 转 f32 和 0.6B/1.7B 资源闸门；worker stdout 只承载 JSONL IPC，父进程会忽略非 JSON stdout 日志行，隐藏 `ai voice worker` 命令强制日志写文件，避免 `qwen3_asr` 初始化日志污染实时转写协议。
- 新增 voice session 管理：`VoiceInputSession`、`VoiceInputState`、`VoiceEvent`。
- 新增 voice ime 管理：`VoiceImeStatus`、`VoiceImeSetup`、`VoiceImeClientSession`、`VoiceImeFeedbackRecord`。
- 新增 microphone 管理：Core Audio device discovery、device selection、device disconnect fallback。
- 新增 audio pipeline 模块：PCM normalize、stateful session buffer、VAD、backpressure。
- 新增 provider trait：
  - `AsrRealtimeProvider`
  - `AsrFileProvider` 继续复用现有 ASR contract
  - `Qwen3StatefulStreamingProvider`
- 复用并逐步下沉现有 `asr_streaming.rs` 的 normalize、dedupe、append 逻辑。
- 新增本地 source discovery：
  - mic 设备枚举。
  - macOS ScreenCaptureKit/system audio 能力探测。
  - macOS Core Audio process tap 能力探测。
- 新增 vocabulary store 和 API。
- 新增 feedback store 和 API：记录 raw/refined/committed/user_final diff，本地生成 vocabulary/rewrite correction。
- `/api/asr/*` 保持兼容，不在 V1 中删除。

### macOS inputmethod / helper

- 新增 `macos/BifrostVoiceInputMethod/`：
  - `Info.plist`。
  - `main.swift` 创建 `IMKServer`。
  - `BifrostVoiceInputController.swift` 管理 client、marked text、commit、cancel/revert 和 edit feedback。
  - `FeedbackTracker.swift` 通过 input client 可用能力采样周边文本并生成 diff。
- 新增 `crates/bifrost-voice-helper` 或等价 Swift helper：
  - Core Audio / AVFoundation 麦克风设备枚举。
  - 指定设备采集 PCM。
  - 全局热键监听。
  - 与 inputmethod/daemon 的本机 IPC。
  - 权限 probe。

### CLI

- 新增 `bifrost ai voice` 命令族。
- `voice sources` 做只读能力探测。
- `voice listen` 启动本地音频监听并输出 text/jsonl/sse，实时 provider 只支持 `qwen3_stateful_streaming`。
- `voice vocabulary` 管理本地词汇。
- 新增 `bifrost ai voice ime` 子命令族：
  - `setup`：幂等安装或修复 inputmethod bundle、helper、LaunchAgent 和默认配置。
  - `status`：查看 inputmethod、helper、权限、热键、麦克风设备和最近 feedback 状态。
  - `repair`：修复 bundle/helper path、LaunchAgent、quarantine 和升级后的权限状态。
  - `activate-guide`：打开系统输入源设置并展示启用步骤。
  - `helper start/stop/start --foreground`：启动或停止 helper。
  - `microphones list/set/recheck`：枚举和选择输入设备。
  - `feedback list/apply`：查看并应用本地学习记录。

### WebUI

- ASR 工具页内的麦克风输入已改用 Voice Input Runtime：`Start Mic` 通过 `buildVoiceRealtimeUrl()` 连接 `/api/voice/listen-ws`，发送 `pcm_s16le` binary chunk，并消费 `connected/source_ready/asr_partial/asr_stable_delta/asr_final_utterance/done`。
- ASR 工具页内的文件上传仍走 ASR server 的 `/api/asr/transcribe-stream`，保持离线文件处理与实时语音输入的 provider 边界。
- Speech 设置中的模型下拉继续支持 `Qwen3-ASR-0.6B` / `Qwen3-ASR-1.7B`；实时语音默认 0.6B，选择 1.7B 时作为显式大模型 opt-in。
- Settings -> Speech Converter -> Voice Input 提供真正输入法安装、启用状态、helper、权限、热键、麦克风和学习反馈管理。
- 复用现有 Audio Input / Transcript 卡片。
- 新增 raw/refined 双轨展示。
- 新增 Vocabulary 管理入口。
- Agent composer 可复用同一 runtime；macOS 普通输入框必须以 InputMethodKit 为主路径。

### 文档与测试

- 新增 `human_tests/asr-realtime-voice-input.md`。
- 更新 `human_tests/readme.md`。
- 后续实现时补 E2E 脚本：
  - voice CLI source discovery。
  - voice WebSocket PCM fake input。
  - voice vocabulary correction。
  - macOS source probe smoke。

## 分阶段实施

### Phase 0：实验验证

只做实验，不改产品行为：

- 写独立 POC 脚本模拟 60 秒 PCM 输入。
- 对比当前 WebSocket 累积 WebM 转码和 PCM ring buffer 两种管线。
- 对 `qwen3_stateful_streaming` 测 `chunk_size_sec` 0.5、1.0、2.0 的 first partial、RTF、重复率；`qwen3_asr_rs/asr-server` 只用于离线文件路径对照。
- 在满足 GPU 条件的机器上验证官方 vLLM stateful streaming。
- 在 macOS 14.2+ 机器上验证 Core Audio process tap source discovery。

验收：

- 1000ms 窗口能稳定输出 partial/final。
- session 10 分钟内 flush 延迟不随会话长度线性增长。
- cancel 后无残留音频进程和临时文件。
- 系统音频不可用时能返回 `needs_permission` / `unsupported`，而不是假 ready。

### Phase 1：本地 V1

- 实现 `Bifrost Voice.inputmethod` 并作为默认交付入口。
- 实现 `bifrost ai voice ime setup/status/repair/activate-guide`。
- 实现 `qwen3_stateful_streaming` realtime provider。
- 实现 Web PCM/ring-buffer 实时链路。
- 实现 `bifrost ai voice sources` 和 `bifrost ai voice listen --source mic`。
- 实现 vocabulary store 与轻量后处理。
- 后置 rewrite 先支持关闭状态和本地 provider interface，不强依赖具体 LLM。
- 实现 `bifrost-voice-helper`：
  - `bifrost ai voice ime helper start/stop/start --foreground`。
  - 用户级 LaunchAgent。
  - 全局快捷键。
  - 麦克风设备枚举和指定设备录音。
  - 连接 `/api/voice/listen-ws`。
  - 与 inputmethod 共享 session 状态。
  - 权限状态上报给 Admin API。
- 实现 edit feedback tracker：
  - commit 后记录 raw/refined/committed。
  - 用户编辑后生成 diff。
  - 本地应用到 vocabulary/rewrite examples。
- WebUI `Voice Input` 开关安装/修复输入法和 helper，不默认启动 1.7B。

验收：

- Web 通道和 CLI mic 通道都可转写。
- `brew install bifrost && bifrost ai voice ime setup` 后，`Bifrost Voice.inputmethod` 安装到 `~/Library/Input Methods/` 并能在系统输入源中启用。
- 选中 Bifrost Voice 输入法后，按热键能在当前焦点 App 以 IMK marked/commit 路径插入 final/refined text。
- 三种热键模式的取消/撤销行为符合表格定义。
- 默认跟随系统麦克风；选择 USB/蓝牙设备后使用指定设备采集，断开时给出明确 fallback 状态。
- 用户删除/新增/替换已提交文本后，feedback 记录包含 committed_text、user_final_text 和 edit_ops。
- 未授予必要权限时返回明确状态，不伪装成功；fallback paste 不得冒充真正输入法路径。
- 升级或 helper path 变化后，WebUI 能提示重新 re-check 权限。
- 不发送原始音频到远端。
- JSONL 事件包含 partial、stable_delta、final_utterance。
- 自定义词汇能在后处理层稳定替换已知误识别词。

### Phase 2：系统音频与应用音频

- 实现 `--source system`。
- macOS 14.2+ 支持 `--source app --app <name>`。
- 引入权限状态和引导。
- 必须保留 `unsupported` / `needs_permission` / `profile_locked` 等可行动状态。

验收：

- `voice sources` 能列出 mic/system/app 及状态。
- 权限不足不会启动录制。
- 系统音频捕获成功后能进入同一 ASR 管线。

### Phase 3：本地 stateful streaming

- 引入本机 `qwen3_stateful_streaming` provider，当前实现基于 Rust `qwen3-asr` crate。
- 与 V1 chunked provider 同时保留。
- 增加 provider selection 和 fallback。
- 对比延迟、资源、稳定性和识别质量。

验收：

- 同一输入下 stateful provider first partial 和 CPU/内存优于 chunked provider。
- 断线/cancel 能释放 streaming state。
- streaming provider 不绑定外网，不调用云端 API。

### Phase 4：signed / notarized 发布体验强化

- 增加 signed / notarized 发布形态：`.app` helper onboarding、自动更新、输入法修复向导。
- raw draft 实时显示，refined final 替换提交。
- 支持 app-specific rewrite profile。
- 在具备 Developer ID 签名与 notarization 的发布形态中，强化 InputMethodKit 输入源安装、激活、提交文本和卸载的稳定性。

验收：

- signed / notarized `.app` helper 可以提供更自然的权限 onboarding；Homebrew unsigned helper 仍保留。
- 已安装的 `Bifrost Voice.inputmethod` 在升级后仍可加载。
- `.app` onboarding 能引导用户修复权限和输入源启用状态。
- 卸载后输入源、helper 和 LaunchAgent 不残留。

## 验证方法

### 单元测试

- `cargo test -p bifrost-admin voice`
  - provider selection：默认与显式 realtime provider 都是 `qwen3_stateful_streaming`，旧 `qwen3_rs_http_chunked` 被拒绝。
  - 1.7B guard：默认拒绝，显式 `allow_stateful_17b=1` 才允许。
  - ime status：installed/enabled/loaded/client_attached 状态序列化。
  - hotkey modes：hold/toggle 三种模式 cancel/revert 行为。
  - feedback diff：delete/insert/replace 生成 edit_ops。
  - microphone selection：system_default、coreaudio uid、disconnect fallback。
  - VAD endpoint：静音触发 final utterance。
  - dedupe：中文/英文 suffix-prefix 去重。
  - vocabulary：alias -> canonical 替换。
  - privacy log redaction：长文本和音频 bytes 不进入日志 payload。
- `cargo test -p bifrost-cli voice`
  - `voice sources/listen/vocabulary/ime/microphones/feedback` 参数解析。
  - source 状态 JSON schema。

### E2E

- 新增 `e2e-tests/tests/test_voice_input_runtime.sh`：
  - 启动临时 Bifrost，带 `--no-system-proxy`。
  - 使用 fake PCM sine/speech fixture 通过 `/api/voice/listen-ws` 发送 start/audio/finish。
  - 使用 mock local ASR provider 返回 partial/stable/final。
  - 断言事件顺序、cancel、done、错误路径。
- 新增 CLI E2E：
  - `bifrost ai voice sources --json`。
  - `bifrost ai voice listen --source mic --duration 3 --dry-run`。
  - `BIFROST_VOICE_E2E_REAL_ASR=1 bifrost -p <admin_port> ai voice listen --source file --input-file <say生成音频> --chunk-ms 1000 --provider qwen3_stateful_streaming`，断言 CLI stdout 来自 `/api/voice/listen-ws`，事件 detail 包含 `provider=qwen3_stateful_streaming`，且第一条 `asr_partial.emitted_at_ms` 小于音频总时长。真实 ASR 段只允许本机受控执行，默认 CI 和普通 E2E 不启动模型。
  - `bifrost ai voice vocabulary import/list`。
  - `bifrost ai voice ime status`。
  - `bifrost ai voice ime microphones`。
  - `bifrost ai voice feedback list`。
- macOS-only smoke：
  - inputmethod bundle setup/status。
  - source discovery 不要求真实录制成功，但必须返回明确状态。
  - CI 非 macOS 跳过系统音频真实捕获，不下载模型。

### human_tests

- 执行 `human_tests/asr-realtime-voice-input.md`：
  - Web 麦克风实时输入。
  - CLI 麦克风监听。
  - 真正 InputMethodKit 输入法安装、启用、切换和 IMK marked/commit 输入。
  - InputMethodKit 输入法安装、LaunchAgent、权限状态、热键输入和 IMK marked/commit 主路径。
  - hold/toggle 热键模式和 cancel/revert。
  - 用户编辑反馈学习。
  - 系统默认/USB/蓝牙麦克风选择。
  - unsigned/signed 输入法加载状态和升级后 re-check。
  - CLI 文件源实时回放验证，禁止依赖测试者讲话。
  - CLI 系统音频来源状态。
  - 自定义词汇纠错。
  - raw/refined 双轨展示。
  - 隐私边界：音频不落盘、不出网、服务只监听 loopback。

### 2026-05-21 质检整改：实时 0.6B 初始化、采样率与资源控制

- 离线 ASR 继续以准确率优先，Web/CLI `ai asr` 的默认模型保持 `Qwen3-ASR-1.7B`。
- 实时 Voice 链路资源优先，Web Mic 和 `bifrost ai voice listen` 默认模型固定为 `Qwen3-ASR-0.6B`，不能从离线 ASR 的保存模型继承到 `1.7B`。
- Voice WebSocket session 在收到 `start` 后进入模型前执行默认 0.6B 初始化检查：缺少 0.6B 资产时复用既有 ASR 初始化链路下载/准备 `~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-0.6B`；显式请求 1.7B 仍必须带 `allow_stateful_17b=1`，且不会静默自动下载/加载大模型。
- 浏览器采集音频必须在前端归一化为 16kHz mono PCM16；服务端只接受 `sample_rate=16000`、`channels=1`、`format=pcm_s16le`，避免浏览器实际 44.1k/48k 采样率被误当 16k 喂给模型。
- `qwen3-asr` 不能加载到 Bifrost 代理主进程内。每个 Voice session 由 daemon 拉起独立 `bifrost ai voice worker` 子进程，worker 持有模型和 `StreamingState`，daemon 仅通过 stdio 推 PCM16/收 transcript；session finish/cancel/连接断开时关闭 worker，避免模型内存、Metal/CPU 推理与代理主路径竞争资源。
- transcript 分为 `committed` 和 `partial` 两层：`asr_partial` 只展示当前模型假设，可以随实时上下文变长或变短；只有 silence boundary、Finish/Stop 或最长连续 utterance boundary 才发送 `asr_stable_delta` / `asr_final_utterance` 并把文本追加到 committed。
- daemon 侧按 PCM16 RMS 做轻量静音检测。连续约 1 秒静音会提交当前 partial 并关闭当前 worker；连续约 30 秒未停顿的 utterance 会强制提交一次，防止无限 session 让 `StreamingState` 和 transcript buffer 长期增长。
- 如果 session 已经没有 partial 且只收到静音 chunk，不会重新拉起 worker；如果 WebSocket 长时间没有音频消息，会先把已有 partial 作为 idle boundary commit，再发送 `worker_idle_unloaded` 并关闭 session。该策略保证按下 Start Mic 后即使用户不说话，也不会让模型无限常驻 daemon 或主进程。

### 2026-05-21 质检整改：worker IPC liveness 与 Voice handler 拆分

- Worker stdio IPC 增加 startup/request timeout：daemon 侧对 startup read、audio request write/flush/read、finish write/flush/read 均使用 `tokio::time::timeout`，默认 startup 15s、request 30s，可通过 `BIFROST_VOICE_WORKER_STARTUP_TIMEOUT_MS` / `BIFROST_VOICE_WORKER_REQUEST_TIMEOUT_MS` 做受控覆盖。
- 任一 worker IPC timeout 会立即 `start_kill()` 子进程并返回包含 `timed out` 和 `worker unloaded` 的错误。WebSocket handler 收到 feed/finish 错误后发送 `error` 事件并关闭 session，避免 handler 无限挂起，也避免 worker 继续持有 Qwen 模型内存。
- daemon 仍不直接持有 `AsrInference` 或模型 cache；真实 Qwen engine 只在 `bifrost ai voice worker` 子进程中加载。自动化边界测试在临时服务进程中设置 `BIFROST_VOICE_ENABLE_FAKE_STATEFUL=1`，再使用 `fake_stateful_worker=1` 查询参数构造不加载模型的 stateful session；普通运行时仅凭 query 不会启用 fake worker。
- Voice runtime 支持每连接测试覆盖参数：`silence_commit_ms`、`worker_idle_unload_ms`、`max_utterance_ms`、`ws_idle_timeout_ms`。默认值保持产品行为不变；E2E 使用缩短值稳定覆盖 30 秒最长 utterance、WebSocket idle unload、持续静音和 silence 后 Finish 边界。
- `crates/bifrost-admin/src/handlers/voice.rs` 拆为 `handlers/voice/mod.rs`、`audio.rs`、`sources.rs`、`vocabulary.rs`，入口文件保留路由/WebSocket 编排，音频边界、source discovery 和 vocabulary store 分别下沉到子模块，所有单文件保持小于 1500 行。

验证计划：

- 单元测试：`cargo test -p bifrost-admin voice_stateful --lib` 覆盖 startup/feed/finish hung worker timeout、stdout 日志行跳过和 kill；`cargo test -p bifrost-cli voice_worker_forces_logs_away_from_stdout_protocol --bin bifrost` 覆盖隐藏 worker 日志隔离；`cargo test -p bifrost-admin voice --lib` 覆盖 runtime tuning、VAD、transcript commit、provider selection、vocabulary。
- E2E：`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh` 覆盖 fake stateful worker 的 `reason=max_utterance_duration`、`worker_idle_unloaded`、silence/final committed、持续静音不输出 transcript。
- human_tests：更新并执行 `human_tests/asr-realtime-voice-input.md` 的 TC-VIR-18/TC-VIR-19，同时同步 `human_tests/readme.md` 索引。
- Review/Fix/Test：两轮均复核 `voice_stateful.rs`、`handlers/voice/*`、E2E、human_tests 和文件行数；发现遗漏后复跑相关单元/E2E/human_tests。
- 收尾校验：E2E 之后执行 rust-project-validate 要求的 fmt、clippy、workspace all-features test，并按修改范围评估 local-ci。

### 性能验证

指标：

- first partial latency。
- final latency after silence。
- per-stateful-feed ASR latency。
- session 10 分钟延迟曲线。
- peak RSS / physical footprint。
- duplicate delta ratio。
- dropped frame count。
- cancel cleanup time。

基准命令示例：

```bash
bifrost ai voice listen --source mic --duration 60 --format jsonl \
  --metrics /tmp/bifrost-voice-mic-metrics.json
```

验收线：

- first partial p50 < 1000ms。
- silence final p50 < 1500ms。
- 10 分钟连续 session 无线性变慢。
- cancel 后 2 秒内释放 source 和 provider session。
- 原始音频默认无落盘文件。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：
  - 本地原生部署。
  - Web/CLI 双通道。
  - 系统音频与应用音频来源选择。
  - 高性能低延迟稳定可控。
  - 后置本地优先 LLM。
  - 自定义词汇。
  - 隐私边界。
- 复核资料来源：
  - Qwen streaming 只在 vLLM backend。
  - qwen3_asr_rs 本地 HTTP 能力。
  - Apple ScreenCaptureKit/Core Audio taps 可行性与权限。
- 执行 `git status --short` 和 `git diff`。
- 检查设计文档是否明确改动范围和验证方法。
- 检查 human_tests 是否覆盖核心用户场景。

### 第 2 轮

- 再次复核 diff 和索引。
- 检查方案是否错误引入云端 ASR 或默认远端 LLM。
- 检查 V1/V2 分层是否能推进需求，不把高风险 stateful streaming 阻塞 V1。
- 检查所有未执行项是否有原因和风险。
- 如发现缺口，追加第 3 轮。

## 残余风险

- macOS 系统音频和应用音频受系统版本、权限、签名和 entitlement 影响，必须先 source discovery，再决定是否进入录制。
- `qwen3_asr_rs` V1 chunked realtime 的延迟和重复计算不如官方 stateful streaming，适合作为本地 V1，不是最终性能上限。
- 官方 vLLM stateful streaming 本机部署对 GPU/环境要求较高，在 Apple Silicon 上不一定优于 MLX/Rust 路线，需要实验数据决定。
- Qwen context/hotwords 能力在不同 runtime 的暴露不一致，V1 不能承诺 ASR 原生 hotword 一定生效；必须有后处理和本地 LLM rewrite 兜底。
- 后置 LLM 改写有误改风险，必须保留 raw ASR、支持撤销，并限制 prompt 为忠实改写。
