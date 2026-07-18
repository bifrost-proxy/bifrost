# ASR MOSS 联合转录 Provider 第一阶段真实场景测试

## 功能模块说明

验证 Bifrost 第一阶段联合转录契约、只读真实任务基准工具，以及 MLX MOSS-Transcribe-Diarize 在 Apple Silicon 上处理现有 ASR 任务音频的可行性。测试不修改源音频、任务索引或既有时间线，也不把实验运行时安装进 Bifrost 数据目录。

## 前置条件

1. 当前机器为 Apple Silicon，已安装 `ffmpeg`。
2. 存在任务目录：

   ```bash
   export ASR_TASK_DIR=/Users/eden_studio/.bifrost/asr/tasks/735775510b384fff8903d9c6fc54f1a3
   ```

3. 使用 Python 3.10+ 在临时目录创建隔离环境，安装固定源码版本的 `mlx-audio[stt]`，下载 `vanch007/mlx-MOSS-Transcribe-Diarize-8bit` revision `7210aef739be6b9e068fba9b1d60369ec053655b`。本次验证环境：

   ```bash
   export MOSS_RUNTIME=/tmp/bifrost-moss-mlx-phase1.oAfRsH
   export MOSS_PY="$MOSS_RUNTIME/venv/bin/python"
   export MOSS_MODEL_DIR="$MOSS_RUNTIME/model"
   test -x "$MOSS_PY"
   test -f "$MOSS_MODEL_DIR/model.safetensors"
   ```

4. 真实样本：

   ```bash
   export MOSS_AUDIO_10M=/Users/eden_studio/audio/LEFT/TX_MIC005_20260707_104639/TX01_MIC040_20260713_190050_orig.wav
   export MOSS_AUDIO_30M=/Users/eden_studio/audio/LEFT/TX_MIC003_20260624_215843/TX01_MIC041_20260629_125741_orig.wav
   ```

## 测试用例

### TC-MOSS-01：真实任务只读选样与基线报告

操作步骤：

1. 记录 `files.json` 和两条目标 WAV 的 SHA-256。
2. 执行：

   ```bash
   shasum -a 256 "$ASR_TASK_DIR/files.json" "$MOSS_AUDIO_10M" "$MOSS_AUDIO_30M" > "$MOSS_RUNTIME/before.sha256"
   python3 scripts/asr/benchmark_joint_transcription.py \
     --task-dir "$ASR_TASK_DIR" \
     --target-seconds 600 1800 \
     --hash-inputs \
     --output "$MOSS_RUNTIME/real-baseline.json"
   shasum -a 256 "$ASR_TASK_DIR/files.json" "$MOSS_AUDIO_10M" "$MOSS_AUDIO_30M" > "$MOSS_RUNTIME/after.sha256"
   diff -u "$MOSS_RUNTIME/before.sha256" "$MOSS_RUNTIME/after.sha256"
   ```

3. 读取报告的样本时长、片段数、说话人数、参考语音终点和 RTF。

预期结果：

- 选择约 617 秒和 1800 秒的两个成功样本。
- 两次哈希完全一致；任务和源音频没有被修改。
- 30 分钟样本的 `speech_end_to_media_ratio` 小于 1，但报告只标记 `reference_only`，不误报截断。

### TC-MOSS-02：默认协议 prompt 的 10 分钟联合转录

操作步骤：

1. 使用默认 prompt 运行：

   ```bash
   time "$MOSS_PY" -m mlx_audio.stt.generate \
     --model "$MOSS_MODEL_DIR" \
     --audio "$MOSS_AUDIO_10M" \
     --output-path "$MOSS_RUNTIME/human-10m-default" \
     --format json \
     --max-tokens 8192
   ```

2. 断言 JSON 包含多个片段、`speaker_id`、有效时间范围，最后片段接近既有参考语音终点：

   ```bash
   "$MOSS_PY" - "$MOSS_RUNTIME/human-10m-default.json" <<'PY'
   import json, sys
   data = json.load(open(sys.argv[1], encoding="utf-8"))
   segments = data["segments"]
   assert len(segments) > 1
   assert all(segment.get("speaker_id") for segment in segments)
   assert all(0 <= segment["start"] <= segment["end"] for segment in segments)
   assert abs(max(segment["end"] for segment in segments) - 605.135) < 5
   print({"segments": len(segments), "speakers": len({s["speaker_id"] for s in segments}), "last_end": max(s["end"] for s in segments)})
   PY
   ```

预期结果：

- 模型在本机 MLX/Metal 上成功完成整段推理。
- 输出包含可解析的时间戳和说话人，最后语音终点与参考时间线差异小于 5 秒。

### TC-MOSS-03：自定义 prompt 不得替换协议 prompt

操作步骤：

1. 用普通自然语言 prompt 替换默认 prompt：

   ```bash
   time "$MOSS_PY" -m mlx_audio.stt.generate \
     --model "$MOSS_MODEL_DIR" \
     --audio "$MOSS_AUDIO_10M" \
     --output-path "$MOSS_RUNTIME/human-10m-replaced-prompt" \
     --format json \
     --max-tokens 8192 \
     --prompt 'Transcribe the full meeting audio with accurate timestamps and consistent speaker labels.'
   ```

2. 检查输出退化为缺少 `speaker_id` 的单整段回退：

   ```bash
   "$MOSS_PY" - "$MOSS_RUNTIME/human-10m-replaced-prompt.json" <<'PY'
   import json, sys
   data = json.load(open(sys.argv[1], encoding="utf-8"))
   assert len(data["segments"]) == 1
   assert "speaker_id" not in data["segments"][0]
   assert "[S01]" in data["text"]
   print("replacement prompt reproduced the structured-output regression")
   PY
   ```

预期结果：

- 复现上游 prompt 替换导致结构化时间戳丢失的风险。
- Bifrost 能力注册把 MOSS 标记为 `protocol_prompt_required=true`；后续 sidecar 只能追加上下文，不能替换协议 prompt。

### TC-MOSS-04：30 分钟稀疏语音与末段差异

操作步骤：

1. 使用默认 prompt 运行 30 分钟样本：

   ```bash
   time "$MOSS_PY" -m mlx_audio.stt.generate \
     --model "$MOSS_MODEL_DIR" \
     --audio "$MOSS_AUDIO_30M" \
     --output-path "$MOSS_RUNTIME/human-30m-default" \
     --format json \
     --max-tokens 8192
   ```

2. 断言存在多个说话人，且 MOSS 的最后片段晚于既有时间线的 1706.785 秒：

   ```bash
   "$MOSS_PY" - "$MOSS_RUNTIME/human-30m-default.json" <<'PY'
   import json, sys
   data = json.load(open(sys.argv[1], encoding="utf-8"))
   segments = data["segments"]
   assert len({s["speaker_id"] for s in segments}) > 1
   assert max(s["end"] for s in segments) > 1706.785
   print({"segments": len(segments), "speakers": len({s["speaker_id"] for s in segments}), "last_end": max(s["end"] for s in segments)})
   PY
   ```

3. 验证末段附近不是纯静音：

   ```bash
   ffmpeg -hide_banner -ss 1780 -t 10 -i "$MOSS_AUDIO_30M" -af volumedetect -f null - 2>&1 | grep -E 'mean_volume|max_volume'
   ```

预期结果：

- 30 分钟整文件成功生成结构化结果，不因长输入崩溃或截断到既有时间线终点。
- 1780–1790 秒窗口存在明显非静音峰值，证明既有时间线不是完整性真值；报告只能陈述差异，不能宣称任一模型绝对正确。

## 清理步骤

1. 确认所有摘要和测试日志已记录。
2. 仅删除本次明确的临时实验目录，不删除任务目录或源音频：

   ```bash
   test "$MOSS_RUNTIME" = /tmp/bifrost-moss-mlx-phase1.oAfRsH
   rm -rf "$MOSS_RUNTIME"
   ```
