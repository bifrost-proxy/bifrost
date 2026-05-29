# Voice Wake Actions

## 功能模块说明

验证本机 Voice Wake Actions 能复用现有 Speaker Diarization 声纹录入模块中的 speaker profile，由用户输入或确认短唤醒词，绑定到全局快捷键，并在启动后台监听后通过“sherpa-onnx KWS 候选 + 本人声纹”双门禁触发真实按键动作。默认 listener 使用 sherpa-onnx KWS，不默认拉起 Qwen3-ASR；`backend_asr_phrase_match` 作为 listener engine 已被拒绝。CLI/API 仍保留非执行 trigger 测试路径用于自动化安全验证；WebUI 不展示该概念。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 使用临时数据目录，不能污染本机 `~/.bifrost`。
- 启动服务必须带 `--no-system-proxy`。
- 如执行真实按键用例，macOS 需要给当前终端或 Bifrost 进程授予 Accessibility 权限。
- 如执行真实后台监听，macOS 需要给当前终端或 Bifrost 进程授予 Microphone 权限，并确保 `ffmpeg` 可用。
- 默认轻量唤醒需要初始化 sherpa-onnx KWS 模型资产；资产缺失时 listener start 自动初始化轻量 KWS 资产，初始化失败必须明确报错，不能隐式拉起 Qwen3-ASR。

推荐准备：

```bash
export BIFROST_DATA_DIR="$(mktemp -d ./.bifrost-human-voice-wake.XXXXXX)"
export CARGO_TARGET_DIR="./.bifrost-human-voice-wake-target"
cargo build --bin bifrost
mkdir -p "$BIFROST_DATA_DIR/asr/diarization/speaker-profiles"
cat > "$BIFROST_DATA_DIR/asr/diarization/speaker-profiles/speaker_human.json" <<'JSON'
{
  "id": "speaker_human",
  "display_name": "Human Tester",
  "source": "human_test_fixture",
  "diarization_profile": "sherpa-onnx-balanced",
  "embedding_model": "human-test-fixture",
  "embedding_dim": 16,
  "embedding": [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
  "sample_rate": 16000,
  "total_duration_ms": 2000,
  "samples": [],
  "created_at_ms": 1,
  "updated_at_ms": 1
}
JSON
./.bifrost-human-voice-wake-target/debug/bifrost start -p 18892 --unsafe-ssl --skip-cert-check --no-system-proxy
```

另开终端或后台启动后执行以下用例。所有命令前均需执行 `source ~/.zshrc`。

## 测试用例列表

### TC-VWA-01：服务启动后 Voice Wake 状态可查询

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost start -p 18892 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 查询状态：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake status --json
   ```

预期结果：

- 命令返回 JSON。
- `enabled=true`。
- `store_path` 位于临时 `BIFROST_DATA_DIR/voice/wake/actions.json`。

实际结果：

- 2026-05-27：通过。服务以临时数据目录和 `--unsafe-ssl --skip-cert-check --no-system-proxy` 启动，`ai voice wake status --json` 返回 `enabled=true`、`profile_count=0`、`binding_count=0`，且 `store_path` 位于临时数据目录下。

### TC-VWA-02：使用已有 Speaker Diarization 声纹创建唤醒 profile

操作步骤：

1. 确认已有声纹 profile 存在：
   ```bash
   test -f "$BIFROST_DATA_DIR/asr/diarization/speaker-profiles/speaker_human.json"
   ```
2. 创建 Voice Wake profile，并显式关联已有声纹：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake profile add --id wake_profile_human --name "Human Tester" --voiceprint-profile-id speaker_human --json
   ```
3. 列出 profile：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake profile list --json
   ```

预期结果：

- 创建响应包含 `id=wake_profile_human`。
- 创建响应包含 `voiceprint_profile_id=speaker_human`。
- 列表中包含 `display_name=Human Tester`。
- `speaker_threshold` 默认约为 `0.72`。

实际结果：

- 2026-05-27：通过。本轮 `e2e-tests/tests/test_voice_wake_actions.sh` 先在临时数据目录准备已有 `asr/diarization/speaker-profiles/spk-e2e.json`，再用 `--voiceprint-profile-id spk-e2e` 创建 `wake_profile_e2e`，响应包含 `voiceprint_profile_id=spk-e2e`。

### TC-VWA-03：CLI 使用唤醒音频样本绑定触发词和按键动作

操作步骤：

1. 准备一个唤醒音频样本。真实测试时用本人说出唤醒词的音频；该样本不用于 ASR 文本解析，唤醒词必须通过 `--phrase` 明确传入：
   ```bash
   printf 'voice wake fixture' > /tmp/wake-human.wav
   ```
2. 使用 CLI 从音频样本创建 Voice Wake profile 和 binding；交互使用时可以省略 `--key/--modifiers`，然后在终端里直接按下快捷键：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake bind-audio /tmp/wake-human.wav --voiceprint-profile-id speaker_human --profile-id wake_profile_human_audio --binding-id wake_binding_human --name "Human Tester" --phrase "打开录音" --key space --modifiers cmd --cooldown-ms 1 --json
   ```
3. 列出 binding：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake binding list --json
   ```

预期结果：

- `bind-audio` 创建成功，输出包含 `profile` 和 `binding`。
- `profile.voiceprint_profile_id=speaker_human`。
- `binding.phrase=打开录音`；该短语来自用户传入的 `--phrase`，不是后台 ASR 识别结果。
- action 为 `type=key_press`，`key=space`，`modifiers=["cmd"]`。
- 绑定默认 enabled。

实际结果：

- 2026-05-27：通过。本轮 E2E 使用 `ai voice wake bind-audio`，绑定已有 `spk-e2e` 声纹，传入音频样本路径和快捷键参数，输出包含 `profile.voiceprint_profile_id=spk-e2e`、`binding.id=wake_binding_e2e`、`binding.phrase=打开录音` 和 `key_press Cmd+Space` 动作。

### TC-VWA-04：dry-run 触发不会发送真实按键

操作步骤：

1. 执行 dry-run trigger，并传入声纹分数：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake trigger --profile wake_profile_human --phrase "打开录音" --speaker-confidence 0.91 --json
   ```
2. 查询事件：
   ```bash
   curl -sS http://127.0.0.1:18892/_bifrost/api/voice/wake/events
   ```

预期结果：

- trigger 返回 `matched=true`。
- `action_result.dry_run=true`。
- `action_result.executed=false`。
- `speaker_confidence` 记录为 `0.91`。
- events 中记录本次触发。

实际结果：

- 2026-05-27：通过。本轮 E2E dry-run trigger 匹配，未执行真实按键，events 落盘；listener 触发事件记录 `speaker_confidence>=0.9`。

### TC-VWA-05：显式 execute 才进入真实按键执行路径

操作步骤：

1. 创建一个使用低影响按键 `escape` 的隔离 binding：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake binding add --id wake_binding_execute_guard --profile wake_profile_human --phrase "测试执行门控" --key escape --cooldown-ms 1 --json
   ```
2. 不带 `--execute` 触发：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake trigger --profile wake_profile_human --phrase "测试执行门控" --speaker-confidence 0.91 --json
   ```
3. 显式带 `--execute` 触发：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake trigger --profile wake_profile_human --phrase "测试执行门控" --speaker-confidence 0.91 --execute
   ```

预期结果：

- 不带 `--execute` 时返回 `matched=true`、`action_result.dry_run=true`、`action_result.executed=false`。
- 带 `--execute` 时返回 `executed=true`，或在缺少 macOS Accessibility 权限时返回明确的 `osascript key_press failed` 错误；两种结果都证明只有显式执行才会尝试发送真实按键。

实际结果：

- 2026-05-27：通过。隔离 binding 使用 `escape`；不带 `--execute` 时返回 dry-run 且 `executed=false`，带 `--execute` 时进入真实执行路径并返回 `key press executed`。

### TC-VWA-06：WebUI 手动确认唤醒词并启动 sherpa-onnx KWS 后台监听

操作步骤：

1. 打开 ASR 页面：
   ```text
   http://127.0.0.1:18892/_bifrost/ai?aiSection=tools-asr
   ```
2. 在 `Wake phrase` 输入框手动输入 `测试唤醒`。
3. 可选点击 `Record Wake Audio` 录制一段本人说出 `测试唤醒` 的样本，停止录音后确认页面只提示样本已捕获，不出现 `Recognizing` 或 ASR service 启动。
4. 聚焦 `Global shortcut` 输入框，直接按下要绑定的组合键，例如 `Cmd+Space`。
5. 在 `Voiceprint` 中选择已有 `Human Tester` 声纹。
6. 保存语音指令后，启用 `Voice command` 开关或点击 `Start Listening`。
7. 关闭或刷新页面后重新打开，确认 listener 状态仍来自后端。
8. 对麦克风说 `测试唤醒`。

预期结果：

- ASR 页面显示 `Voice Wake Actions` 面板。
- 面板提供 `Record Wake Audio`、`Wake phrase`、`Global shortcut` 输入框、`Voice command` 开关和 `Start Listening`。
- `Wake phrase` 展示在录音区域下方，可由用户手动输入或修正。
- 录音停止后不会由后端 ASR 自动生成唤醒文本，也不会启动 Qwen3-ASR 服务。
- `Voiceprint` 展示已有 Speaker Diarization 声纹，用户只能选择已有声纹，不能在 Voice Wake 中手工录入声纹 ID。
- `Global shortcut` 不是下拉框；用户按下组合键后，输入框直接展示捕获到的快捷键。
- 启用 `Voice command` 开关或点击 `Start Listening` 后状态显示 backend listener running，listener engine 为 `lightweight_kws_listener`。
- 页面关闭后 listener 仍在 Bifrost 后台进程内运行。
- CLI 等价启动命令 `ai voice wake listener start` 默认使用 Bifrost 后台内置麦克风采集链路，不需要浏览器打开或参与音频读取；未指定设备时默认使用 `:0`。
- 本人说出唤醒词后 sherpa-onnx KWS 命中候选，再通过声纹验证，事件表显示 `测试唤醒`，结果为 `Executed`。
- 默认 listener 不启动 Qwen3-ASR 服务；显式请求 `backend_asr_phrase_match` 也会返回 400，不能作为常驻唤醒路径。
- 页面不展示 `dry-run`。

实际结果：

- 2026-05-27：通过。本轮 `web/tests/ui/voice-wake-actions.spec.ts` 验证 ASR 页面显示 `Voice Wake Actions`、`Record Wake Audio`、`Wake phrase`、`Voiceprint` 已有声纹选择、`Global shortcut` 快捷键输入框和 `Voice command` 开关；测试模拟直接按下 `Alt+Shift+A` 后输入框展示 `shift+option+a`，点击 `Save` 后事件表也显示该快捷键；启用开关后进入后端 listener 状态，状态区显示 `91% voice`，事件表显示 `Executed`，页面未展示 `dry-run`，未使用浏览器 SpeechRecognition。真实麦克风说话触发需用户在 18892 服务里现场验证。
- 2026-05-28：通过。已补充默认 listener engine 为 `lightweight_kws_listener`，并通过真实服务 `GET /api/voice/wake/kws/status` 验证该 engine 不要求默认 Qwen3-ASR；`backend_asr_phrase_match` listener start 返回 400，避免把重 ASR fallback 当默认唤醒方案。
- 2026-05-28：通过。已在 `http://127.0.0.1:19010` 真实服务与 WebUI 验证：`Wake phrase` 输入框 `readOnly=false`，可手动填入 `哈喽哈喽`；点击 Save 后 binding phrase/normalized_phrase 均保存为手输文本；页面无 `Recognizing` 状态，录入和保存过程中进程列表没有 `qwen3_asr_rs/asr-server`；启动真实 mic listener 后 status 为 `engine=lightweight_kws_listener`、`kws.engine=sherpa-onnx`、`requires_qwen_by_default=false`。

### TC-VWA-07：声纹分数不足不能触发同一唤醒词

操作步骤：

1. 使用 trigger 测试路径模拟同一唤醒词但低于 speaker threshold 的声纹分数：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake trigger --profile wake_profile_human --phrase "打开录音" --speaker-confidence 0.1 --json
   ```
2. 查询事件：
   ```bash
   curl -sS http://127.0.0.1:18892/_bifrost/api/voice/wake/events
   ```

预期结果：

- trigger 返回错误，包含 `speaker confidence ... below threshold`。
- events 数量不会因为这次低声纹分数而增加。
- 真实 listener 路径下，非绑定声纹或低于阈值的最新 wake window 会记录 rejected 状态，不触发按键。

实际结果：

- 2026-05-27：通过。本轮 E2E 使用 mock listener 传入同一文本和 `mock_speaker_profile_id=spk-other`，状态返回 `last_speaker_profile_id=spk-other` 且 `last_error` 包含 `speaker verification failed`；events 数量保持为 2，没有新增触发事件。
- 2026-05-28：通过。`backend_asr_phrase_match` listener engine 已被禁止；用 trigger 低分路径覆盖按键门禁，真实 listener 则由 sherpa-onnx KWS 命中后再用 wake window 声纹结果决定 allowed/rejected。

### TC-VWA-08：后台服务不可达时录音样本采集给出明确错误

操作步骤：

1. 打开已加载的 ASR 页面：
   ```text
   http://127.0.0.1:18892/_bifrost/ai?aiSection=tools-asr
   ```
2. 停止或杀掉 18892 上的 Bifrost 服务。
3. 在 `Voice Wake Actions` 面板点击 `Record Wake Audio`，录制一段 1-3 秒唤醒音频后停止录音。

预期结果：

- 页面不再只展示浏览器原始 `Failed to fetch`。
- 状态区域或 toast 明确提示 `Bifrost backend is not reachable. Restart Bifrost and record wake audio again.`。
- 重新启动 Bifrost 后可再次录音采集样本。

实际结果：

- 2026-05-27：通过。确认 18892 无监听进程时，录音样本处理失败原因是后端不可达；WebUI 已将原始 `Failed to fetch` 收敛为明确的 Bifrost 后台不可达提示。

### TC-VWA-09：WebUI 无声纹或无指令时不能启用后台监听

操作步骤：

1. 使用空的临时 `BIFROST_DATA_DIR` 启动 Bifrost，打开：
   ```text
   http://127.0.0.1:18892/_bifrost/ai?aiSection=tools-asr
   ```
2. 查看 `Voice Wake Actions` 面板中的 `Voice command` 开关。
3. 准备已有 Speaker Diarization 声纹但不保存 Voice Wake 指令，再刷新页面。
4. 再次查看 `Voice command` 开关和 `Start Listening` 按钮。

预期结果：

- 没有 Speaker Diarization 声纹时，`Voice command` 开关禁用，页面展示需要先录入声纹的提示。
- 已有声纹但没有保存语音指令 binding 时，`Voice command` 开关仍禁用，页面展示需要录音、捕获快捷键并保存指令的提示。
- `Start Listening` 同样禁用，不能绕过开关直接启动后台监听。
- 后端 `POST /api/voice/wake/listener/start` 在缺少关联声纹或缺少 binding 时返回 400，不会启动 worker。

实际结果：

- 2026-05-27：通过。`web/tests/ui/voice-wake-actions.spec.ts` 分别覆盖无声纹和已有声纹但无保存指令两种状态，`Voice command` 开关和 `Start Listening` 均禁用，并显示对应提示；`e2e-tests/tests/test_voice_wake_actions.sh` 还验证未保存 voice command 前 CLI/API listener start 被 400 拒绝，不会启动后台监听。
- 2026-05-28：通过。CI Linux shard 2 暴露 `test_voice_wake_actions.sh` 在 `SKIP_BUILD=true` 时仍重新 debug build，导致 shard timeout；已改为复用 `BIFROST_BIN`/`target/release/bifrost`。本地执行 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost BIFROST_VOICE_WAKE_E2E_PORT=18992 bash e2e-tests/tests/test_voice_wake_actions.sh`，验证未保存 voice command 拒绝启动、绑定音频快捷键、dry-run 触发、mock listener 同声纹触发和异声纹拒绝均通过。
- 2026-05-28：通过。默认 listener 改为轻量 KWS 后，本地执行 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost BIFROST_VOICE_WAKE_E2E_PORT=18992 bash e2e-tests/tests/test_voice_wake_actions.sh`，验证未保存 voice command 拒绝启动、`bind-audio --phrase`、dry-run trigger、`backend_asr_phrase_match` listener start 返回 400、mock KWS 同声纹触发和异声纹拒绝均通过。

### TC-VWA-10：后台监听 worker 跟随主服务 stop 退出

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost，并创建关联已有声纹的 Voice Wake profile 与 binding。
2. 执行：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost -p 18892 ai voice wake listener start --json
   ```
3. 查询 listener 状态，记录 `listener.worker_pid`。
4. 停止主服务：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target/debug/bifrost stop
   ```
5. 使用 `kill -0 "$worker_pid"` 或 `ps -p "$worker_pid"` 检查 worker 是否仍存在。

预期结果：

- listener start 返回 `running=true` 且 `worker_pid` 非空。
- 主服务停止后 worker 自行退出，不留下孤儿麦克风监听进程。
- 如果通过 `ai voice wake listener stop` 单独停止，也会终止同一个 worker。

实际结果：

- 2026-05-27：通过。使用临时 `BIFROST_DATA_DIR` 在 18894 启动当前构建的 Bifrost，创建 fixture 声纹和 binding 后用 `ai voice wake listener start --device :999 --dry-run --chunk-ms 1000 --json` 拉起 worker，状态返回 `worker_pid=27495`；随后停止主服务进程，`kill -0 27495` 在 10 秒内变为不存在，确认 worker 跟随主服务退出。

## 清理步骤

```bash
pkill -f ".bifrost-human-voice-wake-target/debug/bifrost start -p 18892" || true
rm -rf "$BIFROST_DATA_DIR" ./.bifrost-human-voice-wake-target
```
