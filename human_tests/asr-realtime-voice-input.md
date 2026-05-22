# ASR 实时语音输入与本地 Voice Input Runtime

## 功能模块说明

验证 Bifrost Voice Input Runtime 的真实用户场景：Web 麦克风录音、CLI 本机音频监听、系统音频来源状态、自定义词汇、本地后置优化和隐私边界。该模块只允许本地原生 ASR provider；原始音频默认不上传云端、不落盘。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 启动 Bifrost 时必须使用临时数据目录，并加 `--no-system-proxy`。
- 已初始化本地 Qwen3-ASR runtime，或测试使用 mock local ASR provider。
- 非 macOS 平台允许把系统音频和应用音频捕获标记为 `unsupported`，但必须返回明确状态。
- macOS 系统音频捕获测试需要用户授予对应权限；权限未授予时预期状态为 `needs_permission`。
- 测试不得调用云端 ASR API。
- macOS 语音输入法体验 V1 必须由 `Bifrost Voice.inputmethod` 真正输入法提供；`bifrost-voice-helper` 是输入法伴随进程，负责热键、麦克风采集、权限探测和诊断，不替代输入法。

## 测试用例列表

### TC-VIR-01 Web 麦克风实时输入

操作步骤：

1. 使用临时数据目录启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-web.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18883 --unsafe-ssl --no-system-proxy
   ```
2. 在浏览器打开 `http://127.0.0.1:18883/_bifrost/ai?aiSection=tools-asr`。
3. 在 Settings -> Speech Converter 模型下拉确认默认 `Qwen3-ASR-0.6B` 可用；如切换 `Qwen3-ASR-1.7B`，必须确认这是显式大模型 opt-in。
4. 点击 Start Mic，说一段 5-8 秒中文，随后点击 Stop Mic。
5. 观察实时事件和 Transcript。
6. 使用浏览器开发者工具或 Playwright mock 检查实时麦克风 WebSocket URL 和帧：
   - URL 为 `/api/voice/listen-ws`。
   - query 包含 `provider=qwen3_stateful_streaming`、`source=web_mic`、`chunk_ms=1000`。
   - 默认模型为 `Qwen3-ASR-0.6B`；选择 `Qwen3-ASR-1.7B` 时 query 包含 `allow_stateful_17b=1`。
   - start message 为 `{"type":"start","source":"web_mic","sample_rate":16000,"channels":1,"format":"pcm_s16le"}`。
   - 后续音频帧为 binary PCM16 chunk，不是 `audio/webm` / `MediaRecorder` blob。

预期结果：

- 页面显示 `asr_partial` 或兼容的 partial 事件，不需要等 Stop Mic 才出现文本。
- Stop Mic 后出现 final utterance 或兼容 final 事件。
- 麦克风电平在录音时波动，停止后归零。
- 服务端只连接本地 ASR provider，不调用云端 ASR。
- 文件上传转写仍走 ASR server `/api/asr/transcribe-stream`，不误用 Voice realtime session。

### TC-VIR-02 WebSocket PCM 协议与取消

操作步骤：

1. 启动临时 Bifrost。
2. 通过 WebSocket 连接 `/api/voice/listen-ws?source=web_mic`。
3. 发送 `start`、多个 `audio` frame，然后发送 `cancel`。
4. 检查事件流和服务端资源。

预期结果：

- 服务端返回 `source_ready`。
- cancel 后返回 `done` 或 close。
- cancel 后不再继续读取音频 frame。
- 临时音频文件、provider session 和 source handle 被释放。

### TC-VIR-03 CLI 流式音频监听

操作步骤：

1. 生成一段本机测试音频并确认时长：
   ```bash
   say -r 120 -o /tmp/bifrost-voice-stream-test.aiff \
     "hello bifrost streaming voice input test. this is the first sentence. now we continue speaking for a little longer. the command must print transcript events before this audio stream has finished. this is the final sentence."
   ffmpeg -hide_banner -loglevel error -y \
     -i /tmp/bifrost-voice-stream-test.aiff \
     -ar 16000 -ac 1 /tmp/bifrost-voice-stream-test.wav
   ffprobe -v error -show_entries format=duration -of csv=p=0 /tmp/bifrost-voice-stream-test.wav
   ```
2. 启动本机 Bifrost 服务，使用临时数据目录且不修改系统代理。真实 ASR 转写前应先显式启动 ASR 服务并观察资源，不允许 Voice 连接默认偷偷拉起 1.7B 模型：
   ```bash
   export BIFROST_VOICE_ADMIN_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-admin.XXXXXX)"
   BIFROST_DATA_DIR="$BIFROST_VOICE_ADMIN_DATA_DIR" \
     cargo run --bin bifrost -- start -p 18887 --unsafe-ssl --no-system-proxy
   BIFROST_DATA_DIR="$BIFROST_VOICE_ADMIN_DATA_DIR" \
     cargo run --bin bifrost -- -p 18887 ai asr start --model Qwen3-ASR-0.6B --language chinese
   ```
3. 用实时文件源喂给 CLI，CLI 必须连接上一步服务的 Voice WebSocket，并走 stateful streaming provider：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-cli.XXXXXX)" \
     cargo run --bin bifrost -- -p 18887 ai voice listen \
       --source file \
      --input-file /tmp/bifrost-voice-stream-test.wav \
      --duration 7 \
      --chunk-ms 1000 \
      --model Qwen3-ASR-0.6B \
      --provider qwen3_stateful_streaming \
      --language english \
      --format jsonl
   ```
4. 如需真实麦克风 smoke，再执行：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-cli.XXXXXX)" \
    cargo run --bin bifrost -- -p 18887 ai voice listen --source mic --duration 15 --chunk-ms 1000 --format jsonl
   ```

预期结果：

- stdout 输出 JSON Lines。
- 文件源测试先输出 `connected/source_ready`，随后输出多条 `asr_partial` 或 `asr_stable_delta`，最后输出 `asr_final_utterance/done`。
- 第一条 `asr_partial.emitted_at_ms` 必须小于音频总时长，证明不是等整段音频结束后才识别。
- 每条 `asr_partial` 包含 `captured_at_ms`、`emitted_at_ms`、`inference_ms`，事件 detail 包含 `provider=qwen3_stateful_streaming`。
- 命令结束后释放音频 source；Voice realtime 不启动 ASR server 的窗口式 provider。
- 如果当前环境无麦克风，命令返回明确 `needs_permission` 或 `source_unavailable`。

### TC-VIR-04 CLI 来源枚举

操作步骤：

1. 执行：
   ```bash
   cargo run --bin bifrost -- ai voice sources --json
   ```
2. 检查输出。

预期结果：

- 输出包含 `platform` 和 `sources`。
- 至少包含 `mic` 来源状态。
- macOS 上应返回 `system` 来源状态。
- 不支持应用音频时返回 `unsupported` 和原因，不伪装为 ready。

### TC-VIR-05 系统音频监听状态

操作步骤：

1. 执行：
   ```bash
   cargo run --bin bifrost -- ai voice listen --source system --duration 5 --format jsonl
   ```
2. 如果系统提示授权，按当前测试目标选择允许或拒绝。
3. 播放一段本机音频。

预期结果：

- 授权且平台支持时，命令输出 ASR 事件。
- 权限不足时，命令返回 `needs_permission`，并说明如何授权。
- 非支持平台返回 `unsupported`。
- 无论成功或失败，都不修改系统代理。

### TC-VIR-06 应用音频来源选择

操作步骤：

1. 执行：
   ```bash
   cargo run --bin bifrost -- ai voice sources --json
   ```
2. 如果 sources 中有 app 来源，选择一个正在播放音频的应用：
   ```bash
   cargo run --bin bifrost -- ai voice listen --source app --app "<app name>" --duration 5 --format jsonl
   ```

预期结果：

- 支持时只监听指定应用或明确标注系统能力限制。
- 不支持时返回 `unsupported`，原因包含系统版本、权限或 entitlement 限制。
- 不会静默退化为全系统音频而不提示用户。

### TC-VIR-07 自定义词汇纠错

操作步骤：

1. 导入词汇：
   ```bash
   cat >/tmp/bifrost-voice-terms.txt <<'EOF'
   Bifrost=宽增,比 frost,白 Frost
   EOF
   cargo run --bin bifrost -- ai voice vocabulary import /tmp/bifrost-voice-terms.txt
   ```
2. 启动 Web 或 CLI 语音输入，说包含 `Bifrost` 的短句。
3. 查看 raw ASR 与 refined text。

预期结果：

- raw ASR 保留模型原始输出。
- refined text 或 stable delta 中把已知别名纠正为 `Bifrost`。
- 词汇纠错不删除原始 raw ASR 证据。

### TC-VIR-08 后置本地改写

操作步骤：

1. 配置本地 rewrite provider 或启用 mock local rewrite provider。
2. 通过 Web 或 CLI 说一段口语化内容。
3. 等待 final utterance。

预期结果：

- 输出包含 raw ASR 和 refined text 两条轨。
- refined text 只做忠实整理、标点、错词和格式修正，不扩写新事实。
- 如果没有配置本地 rewrite provider，系统明确显示 rewrite disabled，不把文本发到远端。

### TC-VIR-09 隐私边界

操作步骤：

1. 启动实时语音输入 10 秒。
2. 检查日志、临时目录和监听地址。
3. 如有网络监控工具，确认没有向云端 ASR API 发出请求。

预期结果：

- 本地 ASR 服务只监听 `127.0.0.1` 或 Unix domain socket。
- 原始音频默认不落盘。
- 日志不包含音频 bytes 或长段隐私文本。
- 没有 DashScope/OpenAI/Gemini 等云端 ASR 请求。

### TC-VIR-10 长会话性能稳定性

操作步骤：

1. 执行 10 分钟实时语音输入或使用本地 PCM fixture 模拟。
2. 记录 first partial、每窗口延迟、final latency、内存和 dropped frame。

预期结果：

- flush 延迟不随 session 时长线性增长。
- cancel 后 2 秒内释放 source 和 provider session。
- 内存没有持续无界增长。

### TC-VIR-11 离线文件与实时 stateful 策略区分

操作步骤：

1. 使用 Web 文件上传或 `bifrost ai asr stream-file` 转写长音频。
2. 使用 Web/CLI 实时语音输入转写短句。
3. 对比事件 detail 或 metrics。

预期结果：

- 文件/目录任务仍使用 ASR server 的离线批处理/分段策略。
- 实时语音输入只使用 `qwen3_stateful_streaming`，不得暴露或调用 `qwen3_rs_http_chunked`。
- 两者不互相污染配置。

### TC-VIR-12 本地 stateful streaming provider 实验

操作步骤：

1. 确认本机已经初始化 `Qwen3-ASR-0.6B`，且不要在普通验证中启用 1.7B：
   ```bash
   test -d "$HOME/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-0.6B"
   test -z "${BIFROST_VOICE_ALLOW_STATEFUL_17B:-}"
   ```
2. 使用 TC-VIR-03 的 `/tmp/bifrost-voice-stream-test.wav` fixture，启动临时 Bifrost 服务：
   ```bash
   export BIFROST_VOICE_STATEFUL_ADMIN_DIR="$(mktemp -d /tmp/bifrost-voice-stateful-admin.XXXXXX)"
   BIFROST_DATA_DIR="$BIFROST_VOICE_STATEFUL_ADMIN_DIR" \
     cargo run --bin bifrost -- start -p 18897 --unsafe-ssl --no-system-proxy
   ```
3. 用 CLI 通过 `qwen3_stateful_streaming` provider 真实流式喂入同一段文件音频：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-stateful-cli.XXXXXX)" \
     cargo run --quiet --bin bifrost -- -p 18897 ai voice listen \
       --source file \
      --input-file /tmp/bifrost-voice-stream-test.wav \
      --duration 7 \
      --chunk-ms 1000 \
      --model Qwen3-ASR-0.6B \
      --provider qwen3_stateful_streaming \
       --language english \
       --format jsonl | tee /tmp/bifrost-voice-stateful-06b.jsonl
   ```
4. 在高性能机器上验证 1.7B 时，使用同一 provider，只切换模型并显式确认大模型加载：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-stateful-cli.XXXXXX)" \
     cargo run --quiet --bin bifrost -- -p 18897 ai voice listen \
       --source file \
      --input-file /tmp/bifrost-voice-stream-test.wav \
      --duration 7 \
      --chunk-ms 1000 \
      --model Qwen3-ASR-1.7B \
       --provider qwen3_stateful_streaming \
       --allow-stateful-large-model \
       --language english \
       --format jsonl | tee /tmp/bifrost-voice-stateful-17b.jsonl
   ```
5. Web/WS 直接调用时，1.7B 等价查询参数为 `model=Qwen3-ASR-1.7B&provider=qwen3_stateful_streaming&allow_stateful_17b=1`。
6. 解析 JSONL，记录首条 `asr_partial.emitted_at_ms`、最后一条 `asr_partial.emitted_at_ms`、`inference_ms` 最大值、final 输出和 Bifrost 进程 RSS 峰值。

预期结果：

- stateful provider 不调用云端 API。
- 每个 Voice WebSocket session 有独立 start/chunk/finish 生命周期，事件 detail 包含 `provider=qwen3_stateful_streaming`。
- CLI stdout 在音频结束前输出 `asr_partial`，不是等整段文件推完后一次性输出。
- `asr_partial` 可以没有 HTTP 窗口的 `window_start_ms/window_end_ms`，但必须包含 `captured_at_ms`、`emitted_at_ms`、`inference_ms`、`delta` 和 `committed`。
- 0.6B 和 1.7B 都走同一套 `qwen3_stateful_streaming` 流式处理路径。
- 0.6B stateful provider 可在本机完成；1.7B 未传 `--allow-stateful-large-model` / `allow_stateful_17b=1` / `BIFROST_VOICE_ALLOW_STATEFUL_17B=1` 时返回明确拒绝，不导致低资源机器误加载大模型。

### TC-VIR-13 真正 InputMethodKit 输入法安装、启用与文本提交

操作步骤：

1. 模拟 Homebrew 或脚本安装后的路径，确认 `bifrost`、`bifrost-voice-helper` 和输入法 bundle 可被发现：
   ```bash
   command -v bifrost || cargo build --bin bifrost
   command -v bifrost-voice-helper || echo "当前版本尚未拆出 helper binary，按设计文档记录实现缺口"
   test -d "/opt/homebrew/opt/bifrost/share/bifrost/input-method/Bifrost Voice.inputmethod" \
     || test -d "$HOME/.bifrost/share/input-method/Bifrost Voice.inputmethod" \
     || echo "当前版本尚未打包 inputmethod bundle，按设计文档记录实现缺口"
   ```
2. 执行幂等 setup。当前版本若尚未实现命令，则检查设计文档中的安装产物、InputMethodKit bundle、LaunchAgent 和权限状态模型，并记录为方案阶段缺口：
   ```bash
   cargo run --bin bifrost -- ai voice ime setup
   cargo run --bin bifrost -- ai voice ime status --json
   ```
3. 使用临时数据目录启动 Bifrost，并通过参数或 WebUI 打开 Voice Input：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-helper.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18891 --unsafe-ssl --no-system-proxy --voice-input
   ```
   如果当前版本尚未实现该参数，则按设计文档执行等价的 WebUI `Settings -> Speech Converter -> Voice Input` 开关验证。
4. 在 WebUI 检查输入法与 helper 状态：
   - install mode 为 `homebrew_formula` 或 `script`。
   - `Bifrost Voice.inputmethod` installed。
   - input method enabled / active / loaded 状态明确。
   - 当前 client bundle id 能在焦点输入框激活后显示。
   - helper path 指向稳定路径，例如 `/opt/homebrew/opt/bifrost/bin/bifrost-voice-helper` 或 `~/Library/Application Support/Bifrost/bin/bifrost-voice-helper`。
   - LaunchAgent loaded。
   - helper running。
   - unsigned mode 显示清晰提示。
   - Microphone permission。
   - Accessibility permission。
   - Input Monitoring permission。
   - ASR service ready 或 waiting for first use。
5. 在 macOS 输入源中切换到 `Bifrost Voice`，打开 TextEdit、浏览器输入框或 VS Code 编辑器，把光标放在可输入区域。
6. 按配置热键说一句包含 `Bifrost` 的短句。
7. 检查输入过程中是否出现 marked text，结束后是否通过 IMK commit 写入 final/refined text。
8. 模拟升级或 helper path 变化后执行：
   ```bash
   cargo run --bin bifrost -- ai voice ime repair
   cargo run --bin bifrost -- ai voice ime status --json
   ```

预期结果：

- 启动 Bifrost 时不会无提示拉起 1.7B ASR；默认使用 0.6B 或等待用户首次使用/显式 Start ASR。
- `Bifrost Voice.inputmethod` 安装到 `~/Library/Input Methods/`，状态区能区分 installed、enabled、active、loaded、client_attached。
- 权限不足时 helper 返回明确 `needs_microphone_permission`、`needs_accessibility_permission` 或 `needs_input_monitoring_permission`，并提供打开系统设置的入口。
- 权限满足且 ASR ready 时，热键录音后文本通过 InputMethodKit marked/commit 写入当前光标位置，不走剪贴板伪装。
- 如果目标 App 的 input client 不可观察，返回 `input_method_client_unobservable`，不能伪装已完成编辑反馈监控。
- WebUI 诊断能看到最近一次 session 的 source、model、first partial latency、final latency 和 insertion status。
- unsigned Homebrew/script helper 必须显示“升级后可能需要重新授权”的提示。
- helper path 变化后状态中出现 `helper_path_changed_after_upgrade` 或等价 repair 提示，repair 后 LaunchAgent 路径与 WebUI 状态一致。
- 失败时不得静默吞掉文本；只有用户显式启用非输入法 fallback 时，才允许将 final text 放入剪贴板并提示降级原因。

### TC-VIR-14 用户编辑反馈学习

操作步骤：

1. 选中 `Bifrost Voice` 输入法，在 TextEdit 输入框中通过语音输入短句，例如“Bifrost 代理”。
2. 等 final/refined text commit 后，手动编辑输入框内容：
   - 删除一个词。
   - 新增一个词。
   - 把一个误识别词替换成正确词。
3. 保持焦点在同一个输入框内 5 秒，或再次按下热键触发 feedback flush。
4. 查看本地反馈记录：
   ```bash
   cargo run --bin bifrost -- ai voice feedback list
   ```
5. 对高置信 feedback 执行本地应用：
   ```bash
   cargo run --bin bifrost -- ai voice feedback apply <feedback_id>
   cargo run --bin bifrost -- ai voice vocabulary list --json
   ```
6. 重新说同一句话，观察 known alias / rewrite correction 是否生效。

预期结果：

- feedback 记录包含 `raw_asr_text`、`refined_text`、`committed_text`、`user_final_text`、`edit_ops`、`app_bundle_id`、`microphone_device_id`。
- 删除、新增、替换分别被记录为 `delete`、`insert`、`replace`。
- 如果目标 App 不支持周边文本读取，系统返回 `input_method_client_unobservable` 或 `feedback_observation_lost`，不声称已经学习。
- `feedback apply` 只更新本地 vocabulary/rewrite examples，不上传音频，不训练远端模型。
- 下一次同音输入能优先使用用户修正后的词汇或改写示例。

### TC-VIR-15 热键模式与取消/撤销语义

操作步骤：

1. 在 WebUI 或 CLI 配置 `hold_to_dictate_revert_on_release`：
   ```bash
   cargo run --bin bifrost -- ai voice ime config set --hotkey-mode hold_to_dictate_revert_on_release
   ```
2. 选中 `Bifrost Voice` 输入法，在 TextEdit 按住热键说一句话，确认 marked text 出现。
3. 松开热键。
4. 配置 `hold_to_dictate_commit_on_release`，重复录音并松开。
5. 配置 `toggle_to_dictate_second_press_cancel`，按一下开始，说话后第二次按下取消。

预期结果：

- `hold_to_dictate_revert_on_release`：松开热键取消本次输入，marked text 被撤销，不 commit。
- `hold_to_dictate_commit_on_release`：松开热键提交 final/refined text；`Esc` 或取消热键能撤销。
- `toggle_to_dictate_second_press_cancel`：第二次按下热键取消并撤销本次 marked/commit。
- 所有模式的事件都带同一个 `voice_input_session_id`，cancel 时先停止音频采集再 cancel Voice WebSocket。
- 如果已 commit 且当前 client 不支持可靠替换，返回 `cancel_revert_unavailable` 并提示用户使用系统 Undo。

### TC-VIR-16 麦克风设备选择

操作步骤：

1. 查看设备列表：
   ```bash
   cargo run --bin bifrost -- ai voice ime microphones
   ```
2. 默认选择 `system_default`，用系统默认输入设备录音。
3. 插入 USB 麦克风或连接蓝牙耳机，重新执行设备列表。
4. 选择具体设备：
   ```bash
   cargo run --bin bifrost -- ai voice ime microphones set --device coreaudio:<uid>
   ```
5. 通过 `Bifrost Voice` 输入法录音一次。
6. 断开该设备，再次按热键录音。

预期结果：

- 设备列表包含 `system_default` 和具体 Core Audio device uid、name、kind、sample rates、channel count、connected、is_default。
- 默认配置跟随系统输入设备。
- 指定 USB/蓝牙设备后，helper 使用该设备采集音频，WebUI 显示 selected device。
- 指定设备断开时产生 `microphone_device_disconnected`；如果 `fallback_to_system_default=true` 则明确回退，否则拒绝录音并提示用户重新选择。
- 不得在用户指定具体设备时静默切到其它具体设备。

### TC-VIR-17 实时 0.6B 初始化、独立 worker、PCM16 采样率与提交边界

操作步骤：

1. 清空 Web ASR 参数后打开 ASR 页面，确认离线 ASR 状态请求仍使用 `model=Qwen3-ASR-1.7B`。
2. 保存离线 ASR 参数为 `Qwen3-ASR-1.7B`，点击 Web `Start Mic`，观察 Voice WebSocket URL。
3. 通过 48kHz 浏览器音频 mock 或真实浏览器默认麦克风采样率启动录音，观察第一条 `start` 消息和后续 binary chunk。
4. 使用临时数据目录启动 Bifrost，再通过 CLI 启动实时链路：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18887 --unsafe-ssl --no-system-proxy --skip-cert-check
   cargo run --quiet --bin bifrost -- -p 18887 ai voice listen --source file --input-file /tmp/bifrost-voice-stream-test.wav --duration 7 --format jsonl
   ```
5. 生成“短语音 + 2 秒静音”的实时文件源，通过 `qwen3_stateful_streaming` 监听并保存 JSONL：
   ```bash
   say -r 150 -o /tmp/bifrost-voice-boundary-test.aiff \
     "hello bifrost boundary test this phrase should become stable after silence"
   ffmpeg -hide_banner -loglevel error -y \
     -i /tmp/bifrost-voice-boundary-test.aiff \
     -af "apad=pad_dur=2" \
     -ar 16000 -ac 1 /tmp/bifrost-voice-boundary-test.wav
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-voice-boundary-data.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18900 --unsafe-ssl --no-system-proxy --skip-cert-check
   cargo run --quiet --bin bifrost -- -p 18900 ai voice listen \
     --source file \
     --input-file /tmp/bifrost-voice-boundary-test.wav \
     --duration 5 \
     --model Qwen3-ASR-0.6B \
     --provider qwen3_stateful_streaming \
     --language english \
     --format jsonl | tee /tmp/bifrost-voice-boundary-test.jsonl
   ```
6. 检查 JSONL 中 `asr_partial`、`asr_stable_delta`、`asr_final_utterance` 和 `done` 的顺序；同时用 `ps` 确认实时链路结束后没有残留 `bifrost ai voice worker` 子进程。
7. 在 0.6B 资产缺失的受控机器上重复步骤 4，观察是否进入 0.6B 初始化；不要在普通验证中显式启用 1.7B。
8. 显式请求 1.7B 时验证 guard：
   ```bash
   cargo run --quiet --bin bifrost -- -p 18887 ai voice listen --source file --input-file /tmp/bifrost-voice-stream-test.wav --duration 7 --model Qwen3-ASR-1.7B --format jsonl
   cargo run --quiet --bin bifrost -- -p 18887 ai voice listen --source file --input-file /tmp/bifrost-voice-stream-test.wav --duration 7 --model Qwen3-ASR-1.7B --allow-stateful-large-model --format jsonl
   ```

预期结果：

- 离线 ASR 默认仍为 `Qwen3-ASR-1.7B`，目录任务和文件转写不被实时链路默认值降级。
- Web/CLI 实时 Voice 默认使用 `Qwen3-ASR-0.6B`；保存离线 1.7B 不会让 Web realtime 自动继承 1.7B。
- WebSocket `start` 消息固定为 `sample_rate=16000`、`channels=1`、`format=pcm_s16le`；48kHz 输入被前端重采样，binary PCM16 字节数约为输入帧按 16k 归一化后的长度。
- 后端拒绝非 16k mono PCM16 start/audio，不能静默把 48k 当 16k 喂给模型。
- 默认 realtime session 缺少 0.6B 资产时复用既有 ASR 初始化链路准备 0.6B；显式 1.7B 未带 opt-in 时返回明确拒绝，带 opt-in 后才允许加载。
- Stateful 模型加载和推理运行在独立 worker 子进程中；Bifrost 代理主进程不直接持有模型 cache，session finish/cancel/断连后 worker 进程退出或被回收。
- `asr_partial` 的 `text` 可以随上下文变长或变短，但 `committed` 在 partial 阶段保持稳定；UI 只用 partial 覆盖临时假设，不把它追加进正式 transcript。
- 约 1 秒静音后输出 `asr_stable_delta`，其 `detail` 包含 `reason=silence; stable=true`，并把当前 partial 提交到 `committed`。
- Finish/Stop 后输出 `asr_final_utterance` 和 `done`；如果静音已经提交过文本，final 的 `delta` 可以为空但 `committed` 必须保持完整。
- 长时间连续说话约 30 秒必须形成一次 stable boundary，防止单个 `StreamingState` 和 transcript buffer 无界增长；Start 后持续静音或 WebSocket idle 时，已有 partial 必须先提交，worker 不应无限常驻。

### TC-VIR-18 worker IPC 超时、最长 utterance、idle unload 与静音回归

操作步骤：

1. 执行 worker IPC hung 回归单测：
   ```bash
   cargo test -p bifrost-admin voice_stateful --lib
   ```
2. 执行 Voice runtime 边界单测：
   ```bash
   cargo test -p bifrost-admin voice --lib
   ```
3. 使用临时数据目录、`--no-system-proxy` 和临时服务环境变量 `BIFROST_VOICE_ENABLE_FAKE_STATEFUL=1` 启动 E2E，覆盖 fake stateful worker 的最长 utterance、无消息 WebSocket idle unload、silence 后 Finish final committed 保持完整、持续静音不输出 transcript：
   ```bash
   BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh
   ```
4. 检查拆分后的核心文件行数：
   ```bash
   wc -l crates/bifrost-admin/src/handlers/voice/mod.rs \
     crates/bifrost-admin/src/handlers/voice/audio.rs \
     crates/bifrost-admin/src/handlers/voice/sources.rs \
     crates/bifrost-admin/src/handlers/voice/vocabulary.rs \
     crates/bifrost-admin/src/handlers/voice_stateful.rs
   ```

预期结果：

- worker startup/feed/finish/read response 挂起时在超时后返回包含 `timed out` 和 `worker unloaded` 的明确错误，测试进程被 kill，不依赖真实 Qwen 模型。
- `reason=max_utterance_duration` 的 `asr_stable_delta` 被自动化断言覆盖，提交文本进入 `committed`；fake worker 只在临时测试服务显式设置 `BIFROST_VOICE_ENABLE_FAKE_STATEFUL=1` 后启用。
- WebSocket start 后无音频消息会在缩短的 idle timeout 内输出 `worker_idle_unloaded` 和 `done`，worker 不继续持有模型资源。
- silence boundary 后 `asr_final_utterance` 的 `committed` 保持完整，`delta` 为空，不重复追加已提交文本。
- 持续静音路径不输出 `asr_partial` / `asr_stable_delta`，Finish final 为空，证明卸载后不会被静音反复拉起。
- 拆分后的 `voice/mod.rs`、`audio.rs`、`sources.rs`、`vocabulary.rs`、`voice_stateful.rs` 任一文件均小于 1500 行。

### TC-VIR-19 stateful worker stdout 日志污染回归

操作步骤：

1. 执行 stateful worker IPC 单测，覆盖 stdout 先出现 ANSI tracing 日志、后出现 ready JSON 的场景：
   ```bash
   cargo test -p bifrost-admin voice_stateful --lib
   ```
2. 执行 CLI 隐藏 worker 日志隔离单测，确认 `ai voice worker` 即使全局传入 `--log-output console` 也会强制使用文件日志，stdout 只留给 JSONL IPC：
   ```bash
   cargo test -p bifrost-cli voice_worker_forces_logs_away_from_stdout_protocol --bin bifrost
   ```
3. 在有 `Qwen3-ASR-0.6B` 资源的本机执行真实实时文件源 smoke，确认 `qwen3_asr` 初始化日志不会再导致 `parse stateful ASR worker response`：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-voice-worker-log-data \
     cargo run --bin bifrost -- start -p 18941 --unsafe-ssl --no-system-proxy --skip-cert-check
   BIFROST_DATA_DIR=/tmp/bifrost-voice-worker-log-cli \
     cargo run --quiet --bin bifrost -- -p 18941 ai voice listen \
     --source file \
     --input-file "$HOME/.bifrost/asr/qwen3_asr_rs/sample3.wav" \
     --duration 3 \
     --model Qwen3-ASR-0.6B \
     --provider qwen3_stateful_streaming \
     --language chinese \
     --format jsonl
   ```

预期结果：

- Worker stdout 中出现非 JSON 日志行时，父进程跳过日志并继续等待 JSON response，不再把日志行当成协议错误。
- Hidden worker 的 console logging 被强制关闭，`qwen3_asr` 的 `Using Metal device` 类日志不进入 stdout。
- 真实 realtime smoke 不再出现 `voice stateful ASR is not ready` / `parse stateful ASR worker response`，并输出 `connected/source_ready/asr_partial/asr_final_utterance/done`。

## 清理步骤

```bash
rm -rf /tmp/bifrost-voice-web.* /tmp/bifrost-voice-cli.* /tmp/bifrost-voice-helper.* \
  /tmp/bifrost-voice-boundary-* /tmp/bifrost-voice-worker-log-data \
  /tmp/bifrost-voice-worker-log-cli /tmp/bifrost-voice-terms.txt
```

如测试中开启了 debug audio dump，必须删除对应路径并确认后续默认关闭。

## 执行记录

| 日期 | 用例 | 命令 / 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-05-21 | 方案阶段资料复核 | Qwen3-ASR GitHub/Hugging Face、qwen3_asr_rs README、Apple ScreenCaptureKit/Core Audio taps 文档检索 | PASS：确认本地 Qwen3-ASR、短窗口实时、macOS 原生音频捕获和本地隐私约束具备可推进方案基础；产品实现待后续开发 |
| 2026-05-21 | TC-VIR-02 / TC-VIR-04 / TC-VIR-07 / TC-VIR-09 / TC-VIR-12 V1 协议与本地边界回归 | `cargo test -p bifrost-admin voice --lib`；`cargo test -p bifrost-cli voice --lib`；`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh` | PASS：CLI `ai voice sources/vocabulary/listen --dry-run` 可用，Vocabulary alias 可把 `宽增` 纠正为 `Bifrost`；Admin `/api/voice/sources/status/vocabulary/sessions` 通过；`/api/voice/listen-ws` 真实 WebSocket 握手并完成 `start/audio/finish`，输出 `source_ready/asr_partial/asr_final_utterance/done`；状态明确 `local_only=true`、`audio_leaves_device=false`，系统/应用音频在 V1 返回 capability 状态而非静默捕获 |
| 2026-05-21 | TC-VIR-03 CLI 流式音频监听无人值守回归 | `say -r 120 ...` 生成 6.929s 本地语音；启动 Bifrost `-p 18890 --unsafe-ssl --no-system-proxy`；`BIFROST_DATA_DIR=/tmp/bifrost-voice-stream-data cargo run --bin bifrost -- -p 18890 ai voice listen --source file --input-file /tmp/bifrost-voice-stream-test.wav --duration 7 --window-ms 1000 --overlap-ms 100 --format jsonl` | PASS：CLI stdout 透传 Voice service 的 `connected/source_ready/asr_partial/asr_stable_delta/asr_final_utterance/done`；首条 partial `captured_at_ms=1049`、`emitted_at_ms=6464`、`inference_ms=5413`，小于音频总时长 6929ms，证明不是等整段结束后才输出 |
| 2026-05-21 | 真实 ASR 资源事故复盘 | `BIFROST_VOICE_E2E_REAL_ASR=1 ... test_voice_input_runtime.sh` | FAIL：真实 Qwen3-ASR 1.7B 测试导致本机内存压力过高并触发系统重启；本轮后续禁止继续真实 ASR 压测，整改为 Voice 默认不自动启动/预热 ASR，真实模型测试必须显式 opt-in 并先确认资源闸门 |
| 2026-05-21 | TC-VIR-13 / TC-VIR-14 / TC-VIR-15 / TC-VIR-16 一步到位输入法方案补充 | 复核 `design/asr-realtime-voice-input.md` 的真正 InputMethodKit 输入法、用户编辑反馈学习、热键取消/撤销语义、麦克风设备选择方案 | PASS：用例已覆盖 inputmethod setup/status/repair、IMK marked/commit、feedback edit_ops、hold/toggle 热键模式、system default/USB/蓝牙麦克风选择；当前实现尚未具备 inputmethod bundle、helper 和 feedback store，标记为后续开发验证项 |
| 2026-05-21 | TC-VIR-03 真实 0.6B CLI 流式回归 | `init-stream?model=Qwen3-ASR-0.6B&language=chinese` 下载 1.8G 资产；`POST /api/asr/service/start?model=Qwen3-ASR-0.6B&language=chinese`；`cargo run --quiet --bin bifrost -- -p 18894 ai voice listen --source file --input-file /tmp/bifrost-voice-stream-test.wav --duration 7 --window-ms 1000 --overlap-ms 100 --model Qwen3-ASR-0.6B --language english --format jsonl` | PASS：输出 18 条 JSONL，其中 7 条 `asr_partial`；音频时长 6929ms，首条 partial `emitted_at_ms=1054`，最后 partial `emitted_at_ms=6448`，最大窗口推理 1192ms；`asr-server` RSS 从启动约 150MB 增至推理时约 1.7GB；确认 CLI 是边推边收，不是整段结束后统一转写 |
| 2026-05-21 | TC-VIR-12 Rust qwen3-asr 0.6B stateful streaming 回归 | `cargo run --bin bifrost -- start -p 18897 --unsafe-ssl --no-system-proxy`；`cargo run --quiet --bin bifrost -- -p 18897 ai voice listen --source file --input-file /tmp/bifrost-voice-stream-test.wav --duration 7 --window-ms 1000 --overlap-ms 100 --model Qwen3-ASR-0.6B --provider qwen3_stateful_streaming --language english --format jsonl`；解析 `/tmp/bifrost-voice-stateful-06b-final.jsonl` | PASS：输出 13 条 JSONL，其中 7 条 `asr_partial`；音频时长 6929ms，warm 后首条 partial `captured_at_ms=503`、`emitted_at_ms=1136`、`inference_ms=633`，首包输入后延迟 633ms；最后 partial/final `emitted_at_ms=7436`，最后输入后延迟 939ms，最大窗口推理 997ms；Bifrost RSS warm 瞬时峰值约 3.82GB、稳定约 1.76GB；事件 detail 包含 `provider=qwen3_stateful_streaming`，final 不再重复拼接历史假设 |
| 2026-05-21 | TC-VIR-11 / TC-VIR-12 realtime 伪流式 provider 移除回归 | `cargo test -p bifrost-admin voice --lib`；`cargo test -p bifrost-cli voice --lib`；`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh` | PASS：Voice provider 默认与 session 创建均返回 `qwen3_stateful_streaming`；`provider=qwen3_rs_http_chunked` 被服务端拒绝；`/api/voice/status` 不再暴露旧 provider；CLI help 不再提供 `--window-ms` / `--overlap-ms` / `--allow-asr-autostart` / `--warmup-asr`，改用 `--chunk-ms`；WebSocket mock 实时链路仍完成 `source_ready/asr_partial/asr_final_utterance/done` |
| 2026-05-21 | TC-VIR-01 Web 麦克风实时输入改走 Voice Runtime 回归 | `npm --prefix web run test:unit -- asr.test.ts`；`cd web && npx eslint src/api/asr.ts src/api/asr.test.ts src/pages/ASR/index.tsx src/pages/ASR/asrUtils.ts tests/ui/asr-microphone-meter.spec.ts`；`npm --prefix web run test:ui -- tests/ui/asr-microphone-meter.spec.ts`；`npm --prefix web run build`；`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh` | PASS：Web `Start Mic` mock 连接 `/api/voice/listen-ws`，query 包含 `provider=qwen3_stateful_streaming` 且默认 `model=Qwen3-ASR-0.6B`；start message 为 `source=web_mic/sample_rate=16000/channels=1/format=pcm_s16le`；后续发送 binary PCM16 chunk，不再使用 `MediaRecorder` / `audio/webm`；文件上传仍通过 `/api/asr/transcribe-stream`；1.7B URL 单测验证会携带 `allow_stateful_17b=1`；后端 Voice E2E 仍通过 |
| 2026-05-21 | TC-VIR-17 质检整改回归 | `npm --prefix web run test:unit -- asr.test.ts asrUtils.test.ts`；`cargo test -p bifrost-admin voice --lib`；`cargo test -p bifrost-cli voice --lib`；`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh`；`cargo run --bin bifrost -- ai voice --help`；`cargo run --bin bifrost -- ai voice worker --help`；`BIFROST_DATA_DIR=/tmp/bifrost-voice-worker-smoke-data-2 cargo run --bin bifrost -- start -p 18899 --unsafe-ssl --no-system-proxy --skip-cert-check`；`cargo run --quiet --bin bifrost -- -p 18899 ai voice listen --source file --input-file /tmp/bifrost-voice-stream-test.wav --duration 3 --model Qwen3-ASR-0.6B --provider qwen3_stateful_streaming --language english --format jsonl` | PASS：离线 Web 默认单测保持 `Qwen3-ASR-1.7B`；Web realtime 默认和后端 target 默认均为 `Qwen3-ASR-0.6B`，且保存离线 1.7B 不会污染 realtime；48kHz 输入会重采样为 16k PCM16，WebSocket start 固定 `sample_rate=16000/channels=1/format=pcm_s16le`，服务端拒绝非 16k mono PCM16；普通 `ai voice --help` 不暴露 worker，隐藏 `ai voice worker` 可作为子进程入口；真实 0.6B stateful listen 输出 `connected/source_ready/asr_partial/asr_final_utterance/done`，证明模型加载和推理可在 worker 子进程链路完成；1.7B opt-in 压测本轮未执行，避免重复触发已记录的本机内存事故 |
| 2026-05-21 | TC-VIR-17 partial/stable/final 提交边界与 worker 回收回归 | `cargo test -p bifrost-admin voice --lib`；`npm --prefix web run test:unit -- asr.test.ts asrUtils.test.ts`；`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh`；生成 `/tmp/bifrost-voice-boundary-test.wav`；`BIFROST_DATA_DIR=/tmp/bifrost-voice-boundary-data cargo run --bin bifrost -- start -p 18900 --unsafe-ssl --no-system-proxy --skip-cert-check`；`cargo run --quiet --bin bifrost -- -p 18900 ai voice listen --source file --input-file /tmp/bifrost-voice-boundary-test.wav --duration 5 --model Qwen3-ASR-0.6B --provider qwen3_stateful_streaming --language english --format jsonl`；`ps` 检查 worker | PASS：`asr_partial` 阶段 `committed` 保持空/稳定且 `detail` 标记 `stable=false`；静音后输出 `asr_stable_delta`，`detail` 包含 `reason=silence; stable=true` 并提交完整文本；Finish 后输出 `asr_final_utterance` 和 `done`，final 不重复追加已提交文本；listen 结束后没有残留 `bifrost ai voice worker` 子进程；旧测试遗留的 orphan `asr-server` 已清理，避免进程数和内存判断被污染 |
| 2026-05-21 | TC-VIR-18 worker IPC 超时、最长 utterance、idle unload 与静音回归 | `cargo test -p bifrost-admin voice_stateful --lib`；`cargo test -p bifrost-admin voice --lib`；`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh`；`wc -l crates/bifrost-admin/src/handlers/voice/mod.rs crates/bifrost-admin/src/handlers/voice/audio.rs crates/bifrost-admin/src/handlers/voice/sources.rs crates/bifrost-admin/src/handlers/voice/vocabulary.rs crates/bifrost-admin/src/handlers/voice_stateful.rs` | PASS：worker startup/feed/finish hung 单测均在超时后返回 `timed out` / `worker unloaded` 并 kill 测试子进程；Voice runtime 单测覆盖可缩短的 max utterance、silence commit、idle unload；E2E 使用 fake stateful worker 断言 `reason=max_utterance_duration`、`worker_idle_unloaded`、silence 后 final committed 保持完整、持续静音不输出 transcript；拆分后文件行数分别为 1391 / 181 / 144 / 80 / 569，均小于 1500 |
| 2026-05-21 | TC-VIR-18 rebase 后 worker liveness/runtime boundary 复核 | `git rebase origin/main`；`cargo test -p bifrost-admin voice_stateful --lib`；`cargo test -p bifrost-admin voice --lib`；`BIFROST_VOICE_E2E_PORT=18887 e2e-tests/tests/test_voice_input_runtime.sh` | PASS：rebase 到 `origin/main` 后 worker startup/feed/finish hung timeout 单测 3/3 通过；Voice runtime 单测 13/13 通过；E2E 再次断言 max utterance、idle unload、silence final 保持 committed、持续静音不输出 transcript，确认 TC-VIR-18 语义在主干更新后仍保留 |
| 2026-05-21 | TC-VIR-19 stateful worker stdout 日志污染回归 | `cargo test -p bifrost-admin voice_stateful --lib`；`cargo test -p bifrost-cli voice_worker_forces_logs_away_from_stdout_protocol --bin bifrost`；`BIFROST_DATA_DIR=/tmp/bifrost-voice-worker-log-data cargo run --bin bifrost -- start -p 18941 --unsafe-ssl --no-system-proxy --skip-cert-check`；`BIFROST_DATA_DIR=/tmp/bifrost-voice-worker-log-cli cargo run --quiet --bin bifrost -- -p 18941 ai voice listen --source file --input-file ~/.bifrost/asr/qwen3_asr_rs/sample3.wav --duration 3 --model Qwen3-ASR-0.6B --provider qwen3_stateful_streaming --language chinese --format jsonl` | PASS：新增 `worker_stdout_log_lines_are_ignored_before_json_response` 覆盖 `qwen3_asr: Using Metal device` 这类 ANSI stdout 日志先于 JSON ready 的路径，父进程跳过非 JSON 行后成功读取 `ready`；隐藏 `ai voice worker` 即使解析到 `--log-output console` 也强制 `LogOutput::File`，stdout 只留给 JSONL IPC；真实 0.6B smoke 输出 `connected/source_ready/asr_partial/asr_final_utterance/done`，不再出现 `voice stateful ASR is not ready` 或 `parse stateful ASR worker response`。 |
