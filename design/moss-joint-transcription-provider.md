# MOSS 联合转录 Provider 与任务模式设计

## 1. 背景与目标

Bifrost 已具备本地 ASR 服务、离线任务、分块转录、说话人分离和时间线产物，但当前整文件转录接口只在内部传递 `(start_ms, end_ms, text)`，会丢失模型原生返回的说话人信息，也没有统一表达 token 截断、取消和生成用量。

MOSS-Transcribe-Diarize 能在一次推理中同时返回转录、时间戳与说话人。第一阶段不替换现有 Qwen 实时链路，也不把实验性运行时直接设为默认值，而是先建立可复用的联合转录契约和真实数据验证路径，使 MOSS 可以作为离线 Provider 接入。

### 1.1 用户目标验证清单

必须实现：

- 整文件转录结果能够保留 `speaker` / `speaker_id` / `speaker_label`。
- 结果能够表达完成、长度截断、取消、失败和未知结束原因。
- Provider 能力通过注册信息表达，避免在业务逻辑中散落模型名判断。
- 提供可重复执行的真实任务基准工具，从现有 ASR 任务中选择约 10 分钟和约 30 分钟音频并读取既有基线。
- Directory Task 可以显式选择“标准转录”或“MOSS 联合转录”模式。
- MOSS 模式允许配置任务级 Prompt；Prompt 随任务保存、读取、编辑和清空。
- 首次运行 MOSS 任务时自动初始化专用 runtime 和固定版本模型，无需用户手工执行初始化命令。

必须不破坏：

- Qwen 及其他现有 OpenAI 兼容服务不返回说话人时，行为与旧版本一致。
- 现有分块、重试、字幕、时间线和 CLI 流程继续接受无说话人结果。
- 不修改现有任务源音频、`tasks.json`、`files.json` 或时间线产物。
- MOSS 不进入实时默认路径；旧任务缺少模式字段时继续使用标准 Qwen 链路。
- 自动初始化只写入 Bifrost ASR 数据目录，不改系统 Python 环境，也不覆盖现有 Qwen 资产。

必须真实验证：

- 用本机已有 ASR 任务的真实音频和既有产物生成基准报告。
- 验证稀疏语音/尾部静音不会被简单的“最后片段结束时间 ÷ 音频总时长”误判为截断。
- 隔离验证结构化说话人 JSON 与旧 JSON 的兼容性。

必须交付：

- 设计文档、单元测试、E2E、`human_tests/` 用例与真实执行记录。
- 两轮 Review/Fix/Test、提交、Draft PR 和远端 CI（含 coverage 90% gate）看护。

## 2. 分阶段范围

### 2.1 第一阶段（本次）

1. 在 `bifrost-asr` 定义与运行时无关的结构化转录结果。
2. 在整文件 OpenAI 兼容调用中解析说话人、结束原因和生成用量。
3. 现有调用方继续使用原有无说话人片段视图；离线时间线优先消费结构化片段并保留模型原生说话人。
4. 增加 Provider 能力注册信息，登记 Qwen 实时/纯转录和 MOSS 离线联合转录的差异。
5. 增加只读基准工具，使用已有任务元数据选择真实样本并生成 JSON 报告。

### 2.2 第二阶段：Directory Task 可选择模式

本阶段把第一阶段契约接入真实 ASR Directory Task：

1. 新增 `transcription_mode=standard|moss_joint`。Serde 默认值为 `standard`，保证历史 `tasks.json` 无迁移写回也能读取。
2. 新增 `transcription_prompt` 字符串。空字符串表示未配置；创建、PATCH、GET 和 Web 表单均保留该值，最大 4000 个 Unicode 字符。
3. `moss_joint` 走整文件联合转录，不执行 Qwen 30 秒分块，也不再叠加 Sherpa/Pyannote 分离；模型原生 speaker 直接进入 timeline。
4. 首次运行时自动准备独立 runtime 和 Q5 模型；准备完成后同一任务直接继续推理。
5. 发布包为 Apple Silicon macOS 生成独立 `moss-transcribe` runtime 资产。runtime 源码与 ggml 子模块固定到审核过的 commit；初始化器从同版本 release checksum manifest 读取 runtime zip 的 SHA-256，校验后才解压；模型固定文件名、大小和 SHA-256，下载后必须校验。测试覆盖的自定义 runtime URL 必须同时提供 `BIFROST_MOSS_RUNTIME_SHA256`。
6. 用户 Prompt 通过仅当前进程可读的临时文件传入。runtime 始终先使用 GGUF 内置协议 Prompt，再追加用户上下文，避免自定义 Prompt 破坏时间戳和 speaker 输出协议。
7. Bifrost 为原生子进程设置 `GGML_METAL_NO_RESIDENCY=1`，规避固定 GGML 版本在部分 Apple Silicon 上完成推理后的 residency-set 退出断言。该开关只关闭可选缓存，不改变模型和解码参数。

`runtime_strategy` 只控制标准 Qwen 链路；MOSS 联合模式固定单文件串行整文件推理。WebUI 在 MOSS 模式下显示该约束，但保留原 Qwen 配置，切回标准模式后可继续使用。

### 2.3 后续阶段

- 长驻 sidecar：固定 API 契约、限制 CORS、健康检查、取消和显式资源回收。
- 可选 MLX runtime：在有可稳定再分发的固定 harness 后，作为 Apple Silicon 高性能后端加入同一能力契约。
- 长音频运行：按 VAD/静音边界切块、上下文衔接、断点恢复、进度和取消。
- 质量评估：人工标注小集上的说话人错误率、转录错误率、时间戳偏移与重复/漏字。

本阶段不把通用 `mlx-audio` server 直接嵌入 Bifrost，不承诺 MOSS 实时流式能力，也不把 MOSS 设为历史任务或新任务的默认模式。

## 3. 领域模型

### 3.1 `TranscriptionSegment`

每个片段包含：

- `start_ms` / `end_ms`
- `text`
- 可选 `speaker`
- `overlap`，默认 `false`

服务端字段兼容顺序为 `speaker`、`speaker_id`、`speaker_label`。空字符串归一化为 `None`。时间范围需满足 `end_ms >= start_ms`。

### 3.2 `TranscriptionResult`

结果包含全文、结构化片段、结束原因和可选生成用量。为降低第一阶段迁移风险，管理端继续保留现有 `WholeFileTranscription.segments` 三元组视图，同时新增结构化片段；两者由同一解析结果生成，旧调用方行为不变。

结束原因枚举：

- `completed`：服务明确完成。
- `length`：达到 token/长度限制，应视为可能截断。
- `cancelled`：用户或调度器取消。
- `failed`：服务明确失败。
- `unknown`：旧服务未提供结束原因。

### 3.3 完整性判断

不能用 `last_segment_end / media_duration` 单独判定长音频是否完整，因为录音尾部可能没有语音。第一阶段提供保守判断：

1. `finish_reason=length|cancelled|failed` 直接判为不完整。
2. `finish_reason=completed` 且有可信预期语音终点（来自独立 VAD 或人工真值）时，比较结构化结果覆盖到的语音终点。
3. 没有预期语音终点且结束原因未知时返回 `unknown`，不制造“已完整”的假阳性。
4. 总时长仅用于报告，不作为唯一门禁。

## 4. Provider 能力注册

注册信息只表达稳定能力，不负责启动运行时：

| Provider | 场景 | 实时 | 原生说话人 | Prompt | 协议 Prompt 不可替换 | 结构化时间戳 |
| --- | --- | --- | --- | --- | --- | --- |
| `qwen-openai` | 实时/离线纯转录 | 是 | 否 | 依服务而定 | 否 | 是 |
| `moss-mlx` | 离线联合转录 | 否 | 是 | 是 | 是 | 是 |
| `moss-cpp` | 离线联合转录 | 否 | 是 | 是 | 是 | 是 |

任务层通过 `transcription_mode` 匹配能力；`standard` 使用 `qwen-openai`，`moss_joint` 使用发布包中的 `moss-cpp` runtime。注册表仍保留 `moss-mlx`，供后续在不改变任务配置的前提下切换实现。

真实样本验证发现，直接用自定义 prompt 替换 MOSS 默认 prompt 时，模型仍会生成 `[Sxx]` 标签，但不再生成可解析的时间戳，最终只能回退成整文件单片段。因此 MOSS sidecar 必须保留协议 prompt，只允许把词汇、语言或会议上下文追加到协议约束中。

## 5. OpenAI 兼容响应

兼容的 verbose JSON 示例：

```json
{
  "text": "你好，开始开会。",
  "segments": [
    {
      "start": 0.0,
      "end": 2.4,
      "text": "你好，开始开会。",
      "speaker_id": "speaker_00",
      "overlap": false
    }
  ],
  "finish_reason": "completed",
  "usage": {
    "prompt_tokens": 128,
    "completion_tokens": 42,
    "total_tokens": 170
  }
}
```

旧服务只返回 `text` 或没有扩展字段时继续走现有回退逻辑。

## 6. 真实任务基准工具

工具只读取任务状态，不写入任务或音频：

1. 输入任务目录或从 `~/.bifrost/asr/tasks/<task-id>` 定位任务。
2. 读取任务配置、`files.json` 与时间线产物。
3. 在成功样本中按目标时长选择最接近的文件，默认目标为 600 秒和 1800 秒。
4. 报告音频路径、时长、既有全文字符数、片段数、说话人数、最后语音终点、既有推理耗时与 RTF。
5. 明确标记源文件和任务文件未被修改。

该报告是后续 MOSS 实跑的可复现输入清单，也是 Qwen/MOSS 对比的参考；它不是 WER/DER 结论，也不能把既有 ASR 时间线当作完整性真值。真实 30 分钟样本中，MOSS 在 1784.84 秒发现了片段，而既有时间线结束于 1706.785 秒，末段波形也确认存在非静音信号。没有人工真值时，只比较耗时、稳定性、结构化输出和候选差异，不宣称模型质量优劣或单方完整。

## 7. 测试方案

### 7.1 单元测试

- Provider 能力查询与未知 Provider。
- 结束原因反序列化和向后兼容。
- 有尾部静音但已覆盖预期语音终点时判定完整。
- `length`、取消、失败优先判定不完整。
- verbose JSON 中 `speaker_id` 与旧无说话人响应均能解析。

### 7.2 E2E

- 使用临时任务 fixture 执行基准工具。
- 断言按目标时长选择正确样本、报告包含说话人数与 RTF、源 fixture 哈希不变。
- E2E 必须离线运行，不依赖模型下载。

### 7.3 `human_tests`

- 对现有 `day` 任务运行只读基准，验证约 10 分钟与约 30 分钟样本。
- 对比运行前后 `tasks.json`、`files.json` 与目标 WAV 哈希。
- 验证 30 分钟稀疏语音样本不会仅因尾部静音被报告为截断。
- 若本机隔离 MLX MOSS 运行时可成功安装，则对真实样本执行 MOSS；若上游运行时或模型在当前系统不可用，保留完整错误证据并将运行时接入列为明确阻塞，不能把基准选择误报为模型验证通过。

## 8. 风险与回退

- 新模型运行时快速演进：发布 runtime 固定源 commit、子模块 commit 和模型哈希；Bifrost 核心只消费稳定 JSON 契约。
- 自动初始化下载中断：使用可续传临时文件，哈希不匹配时拒绝安装并保留明确错误；不会回退成 Qwen 后悄悄产出不同语义的结果。
- Prompt 泄露或协议破坏：Prompt 不放入命令行参数，临时文件随单次推理销毁；runtime 追加而非替换协议 Prompt。
- 长音频 token 截断：保留结束原因和用量，后续结合 VAD 语音终点与分块重试。
- 说话人标签跨块漂移：本阶段只保留单次响应标签，不宣称跨块身份稳定；后续接 voiceprint/聚类归并。
- 资源占用：MOSS 仅离线串行启用，Qwen 实时默认链路不变。
- 回退：Provider 不可用或能力不匹配时继续使用现有 Qwen + diarization 流程。

## 9. Review/Fix/Test 门禁

第 1 轮重点检查响应兼容、旧调用方行为、时间单位、空 speaker、真实任务只读性；运行相关 crate 单测与基准 E2E。

第 2 轮基于最新 diff 复查历史任务默认值、Prompt 保存/清空、发布资产生成、下载校验、原生 speaker timeline、错误路径、文档和 `human_tests` 一致性，并复跑失败路径、真实基准与项目校验。远端 CI 执行 coverage 90% gate，本地不运行全量 coverage。
