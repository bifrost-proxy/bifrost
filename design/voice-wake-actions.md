# Voice Wake Actions

## 功能模块说明

Voice Wake Actions 为本机语音触发全局键盘指令能力。产品目标不是暴露 profile/binding 等内部对象，而是提供一条短流程：用户输入或确认一个短唤醒词，按下要触发的快捷键，选择已有声纹，点击 Start Listening 后由 Bifrost 后台进程用 sherpa-onnx KeywordSpotter 持续监听并触发真实 `key_press` 动作。

首版落地为可测试 MVP：

- 本机存储 `VoiceWakeProfile`：只作为唤醒动作的轻量配置壳，必须关联已有 ASR Speaker Diarization 声纹 profile，记录显示名、`voiceprint_profile_id`、speaker threshold。
- 本机存储 `VoiceWakeBinding`：记录触发词、profile、KWS score/threshold 元数据、speaker threshold、cooldown 和 `key_press` 动作。
- Admin API 与 CLI 支持 profile/binding 创建、列表、dry-run trigger，以及显式执行 key press。
- WebUI 在 ASR 页面提供 Voice Wake Actions 面板，支持可编辑唤醒词、录入试听样本、用输入框捕获全局快捷键、保存动作、启动/停止后台监听和事件查看；WebUI 不展示 dry-run 概念，后台监听命中后真实执行。
- 浏览器只用于一次性配置与样本采集；Start Listening 后由 Bifrost 主服务拉起独立 worker 进程完成麦克风采集、sherpa-onnx KWS、声纹识别、匹配和按键触发，页面关闭不影响后台 listener，主服务 stop 后 worker 必须跟随退出。
- 声纹录入不在 Voice Wake 内重复实现；用户先使用现有 `Speaker Diarization` 声纹录入模块创建 speaker profile，Voice Wake 只选择并绑定该已有声纹。

## 实现逻辑

数据文件位于：

```text
BIFROST_DATA_DIR/voice/wake/actions.json
```

API：

- `GET /api/voice/wake/status`
- `GET /api/voice/wake/profiles`
- `POST /api/voice/wake/profiles`
- `GET /api/voice/wake/bindings`
- `POST /api/voice/wake/bindings`
- `POST /api/voice/wake/trigger`
- `POST /api/voice/wake/listener/start`
- `POST /api/voice/wake/listener/stop`
- `GET /api/voice/wake/events`

CLI：

```bash
bifrost ai voice wake status
bifrost ai voice wake profile add --id wake_profile_eden --name Eden --voiceprint-profile-id speaker_eden
bifrost ai voice wake binding add --profile wake_profile_eden --phrase "打开录音" --key space --modifiers cmd
bifrost ai voice wake bind-audio ./wake.wav --voiceprint-profile-id speaker_eden --phrase "打开录音"
bifrost ai voice wake listener start
bifrost ai voice wake listener stop
bifrost ai voice wake trigger --profile wake_profile_eden --phrase "打开录音"
bifrost ai voice wake trigger --profile wake_profile_eden --phrase "打开录音" --execute
```

`bind-audio` 是 CLI 的一条完整配置路径：传入一次唤醒音频样本和用户确认的 `--phrase`，绑定已有 Speaker Diarization 声纹 profile；如果不传 `--key`，终端进入快捷键捕获模式，用户直接按下要绑定的组合键。自动化或无 TTY 环境可以显式传 `--key space --modifiers cmd`。该命令不再调用 Qwen/ASR 从音频样本中猜唤醒词，保存后的 phrase 会生成 sherpa-onnx KWS keywords。

`listener start` 调用 Bifrost 后台 listener。默认 `--source mic`，主服务先校验已经存在关联声纹的 Voice Wake profile 和启用的 binding，再拉起隐藏 `ai voice wake worker` 独立进程。worker 使用内置本机麦克风采集链路获取音频，不依赖浏览器麦克风、SpeechRecognition 或 Web 页面持续打开；默认 avfoundation 设备为 `:0`，可通过 CLI `--device` 指定。worker 启动时携带父进程 PID，父进程停止后 worker 自行退出，避免孤儿后台监听。

WebUI：

- 入口：`AI -> ASR -> Voice Wake Actions`
- `Wake Audio`：通过浏览器麦克风录入一次唤醒音频样本，用于试听和后续声纹校验体验，不调用 ASR 识别文本。
- `Wake phrase`：紧跟在录音样本下方，用户可直接输入或修正短唤醒词；保存时生成 sherpa-onnx KWS keywords。
- `Voiceprint`：选择现有 ASR Speaker Diarization 声纹 profile；没有已录入声纹时不能保存唤醒动作。
- `Global shortcut`：只读快捷键输入框；用户聚焦后直接按键盘组合键，WebUI 捕获并展示组合键，不再拆成 key/modifiers 下拉选择。
- `Voice command` 开关：WebUI 的后台监听入口。启用前必须已经有 Speaker Diarization 声纹和保存过的语音指令 binding；缺任一项时展示提示并禁用开关。
- `Start Listening`：与开关使用同一门禁，调用 `POST /api/voice/wake/listener/start`，Bifrost 主服务拉起独立 worker 进程；worker 用本机麦克风采集音频并持续喂给 sherpa-onnx `KeywordSpotter`，命中 binding phrase 后从滑动 ring buffer 取最近 wake window 做 speaker embedding 校验；只有 KWS 和声纹都命中且分数达到阈值后才执行真实 macOS `key_press`。

如果录音后请求后端失败，WebUI 需要把浏览器原始 `Failed to fetch` 收敛成明确的后台不可达提示，引导用户重启 Bifrost 后再录音。

CLI 仍保留非执行测试路径以便自动化验证；WebUI 不暴露该路径。macOS 执行使用 `osascript` + System Events，依赖 Accessibility 权限；其它平台返回明确 unsupported。这里不允许用户配置任意 shell/script，避免误触发扩大副作用面。

## sherpa-onnx 对接边界

sherpa-onnx 官方能力在本模块中的落地边界：

- KWS 文本侧：listener 默认且唯一使用 sherpa-onnx `KeywordSpotter` 流式检测 binding phrase；不允许使用 Qwen/ASR 大模型作为常驻关键词检测或录入解析路径。
- Speaker Embedding 声纹侧：复用已有 ASR Speaker Diarization 声纹录入模块持久化的 `speaker-profiles/*.json` embedding；listener 对当前触发音频计算 embedding，并通过 cosine similarity 找到已注册 speaker profile。
- 双门禁：`KeywordSpotter` 命中 binding phrase 且 `identified_profile_id == VoiceWakeProfile.voiceprint_profile_id` 且 `confidence >= binding.speaker_threshold` 才触发动作。

因此 Voice Wake 不允许用户手动录入或编辑声纹标识，也不在本模块里重新采集声纹；唯一声纹来源是现有声纹录入模块。

## 依赖项

- 复用 `bifrost-asr` 的 sherpa-onnx KWS 资产初始化、keywords 生成和流式 `KeywordSpotter`。
- 复用现有 ASR Speaker Diarization 的 `speaker-profiles`、`/api/asr/speaker-profiles`、声纹 identify 逻辑和 sherpa-onnx speaker embedding extractor。
- 后台麦克风 listener 当前在 macOS worker 进程上使用 `ffmpeg -f avfoundation -i :0` 采集默认麦克风；未指定设备时固定使用 `:0`。
- macOS 真按键执行需要 Accessibility 权限。
- 后台麦克风采集需要给 Bifrost/终端进程授予 Microphone 权限。
- 默认启动测试必须使用 `--no-system-proxy`，避免修改系统代理。

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin voice::wake`：验证触发词规范化、AppleScript key press 生成、profile-bound trigger、后台 listener 声纹必填和 speaker confidence 事件记录。
- `cargo test -p bifrost-cli ai_voice_wake_commands_parse --test cli_commands`：验证 CLI 子命令解析。
- `cargo test -p bifrost-cli voice_wake_worker --lib`：验证 worker 父进程守护、KWS 配置和快捷键/设备参数解析。

### E2E 测试

- `e2e-tests/tests/test_voice_wake_actions.sh`
  - 使用临时 `BIFROST_DATA_DIR`；
  - 编译当前 worktree 的 `bifrost`；
  - 以 `--unsafe-ssl --skip-cert-check --no-system-proxy` 启动服务；
  - 在临时数据目录准备已有 ASR speaker voiceprint profile；
  - 验证未保存 voice command 前 listener start 被拒绝；
  - 通过 CLI `bind-audio --phrase` 从音频样本创建关联已有 voiceprint profile 的 Voice Wake profile 和 trigger phrase 到 key press 的 binding；
  - 通过 CLI `listener start` 验证启动后台监听入口；
  - 通过 CLI dry-run trigger；
  - 通过真实 listener start 验证默认 engine 只能是 `lightweight_kws_listener`，并拒绝 `backend_asr_phrase_match`；
  - 验证不同 speaker profile 被拒绝且不会新增触发事件；
  - 通过 API 查询 events 并断言事件落盘。
- `web/tests/ui/voice-wake-actions.spec.ts`
  - 验证 ASR 页面显示 Voice Wake Actions 入口；
  - 验证录音入口、可编辑唤醒词在录音区域下方、已有声纹选择、快捷键输入框捕获和 Voice command 开关；
  - 断言没有声纹或没有保存 binding 时开关禁用并展示提示；
  - 断言开关调用后台 listener API；
  - 断言事件表显示 `Executed`。

### 真实场景测试

- `human_tests/voice-wake-actions.md`
  - 覆盖服务启动、已有声纹准备、CLI profile 创建、binding 创建、API events 查询；
  - 覆盖 WebUI 录音入口、手动输入或修正唤醒词、选择已有声纹、快捷键绑定、Start Listening、后台监听触发真实执行；
  - 覆盖 WebUI Voice command 开关启动门禁和后台 worker 独立进程状态；
  - 覆盖主服务 stop 后 worker 跟随退出。
  - 覆盖他人声纹或低 confidence 不触发动作。
  - 可选执行真实 macOS key press，必须显式标注 Accessibility 权限和 `--execute` 风险。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标：唤醒词录入和识别全链路使用 sherpa-onnx KWS、快捷键绑定、后台启动监听、说话触发真实按键是否可见且可测。
- 复核变更范围：`git status --short`、`git diff`，确认没有修改主 worktree。
- Review 后端 API、CLI 参数、安全边界、E2E、人测文档。
- 运行 `cargo test -p bifrost-admin voice::wake`、`cargo test -p bifrost-cli ai_voice_wake_commands_parse --test cli_commands` 和 E2E。

第 2 轮：

- 复查第 1 轮修复后的 diff。
- 复查 human_tests 索引、API/CLI 输出、WebUI 不展示 dry-run、允许手动确认唤醒词且监听请求进入后台 KWS listener。
- 复跑受影响测试和最终启动验证。

## 校验要求

- E2E 必须先于 rust-project-validate。
- 最终执行 `cargo fmt --all -- --check`、相关 clippy/test/build。
- 至少执行一次 `cargo test --workspace --all-features`；若环境或时间阻塞，必须在最终验证矩阵中说明风险。

## 文档更新要求

- 新增 `human_tests/voice-wake-actions.md` 并同步 `human_tests/readme.md`。
- 本轮 API/CLI 仍为本地 AI voice 子命令实验能力，不更新 README 主入口；后续补 WebUI 后再同步用户文档。
