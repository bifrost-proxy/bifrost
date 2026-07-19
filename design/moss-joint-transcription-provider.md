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
- ASR Management 的 Model Management 统一展示并初始化 MOSS 专用 runtime、Python 依赖和固定版本模型；首次运行 MOSS 任务时仍保留同一初始化器作为兜底。

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
4. 首次运行时自动准备独立、可重定位的 Python 3.12 + MLX-Audio runtime，以及固定 snapshot 的 8-bit MLX 模型；准备完成后同一任务直接继续推理。
5. 发布包为 Apple Silicon macOS 生成 `moss-joint-runtime` 资产。MLX-Audio 固定 commit `64e8416c303fb3b3463dab8eb4ebd78c55a87c1a`，8-bit 模型固定 snapshot `90c3a1ab78fa56e47e1493ddea48e3ababaf2f71`。初始化器从同版本 release checksum manifest 读取 runtime zip 的 SHA-256，校验后才解压；1,258,427,442-byte 权重固定 SHA-256 `469a8969e6b70c8b276411eca54a355a27de9ed6794f738dab53f4ffd3c83190`，下载后必须校验。测试覆盖的自定义 runtime URL 必须同时提供 `BIFROST_MOSS_RUNTIME_SHA256`。`test_asr_moss_release_contract.sh` 在普通 PR CI 中校验 workflow 与初始化器的 commit、snapshot、metadata、资产名和 checksum 契约，避免只有真正发版时才发现配置漂移。

### 统一模型管理

- Model Management 的模型选择同时列出 Qwen3-ASR 和 `MOSS-Transcribe-Diarize-MLX-8bit`。Qwen 继续管理可租约的本地 ASR service；MOSS 是任务按需执行的端到端 runtime，不展示 Host、Service Port 等无效服务配置。
- MOSS 状态接口分别报告 runtime 自检、模型 snapshot 校验、安装目录和预期权重大小。初始化时执行完整 runtime self-test 与 1.2 GB 权重 SHA-256，成功后写入带固定模型 SHA/大小/mtime 和 Python/runner 哈希的验证标记；日常状态读取复核标记与小文件哈希，避免每次打开页面重新扫描 1.2 GB。权重被同大小替换后 mtime 变化也会撤销 Ready。
- 用户可在管理页主动初始化或修复 `~/.bifrost/asr/moss_joint_mlx`。下载复用断点续传和同一进程内初始化锁，管理页与同一服务中的任务同时触发时不会并行写入同一资源。
- Directory Task 不保存另一份模型路径或依赖配置；它只保存 `transcription_mode` 与任务 Prompt。运行时若发现资产尚未准备好，调用与管理页完全相同的校验和初始化函数作为自动兜底。

### 发布前 CI 门禁

- `release.yml` 与 PR CI 必须调用同一个 MOSS runtime 打包脚本。正式发布传入真实 runtime 目录；PR CI 在 Apple Silicon macOS runner 上构造最小 fixture，真实生成 zip 和 `.sha256`，并验证入口、metadata、notice/license、无 AppleDouble sidecar 以及 checksum 可复算。
- 普通 PR 不下载 1.2 GB 权重，也不重复安装完整 MLX Python 环境；`test_asr_moss_release_contract.sh` 负责固定 commit、requirements、模型 metadata、权重 URL/大小/SHA-256、release 资产名和共享打包脚本调用的静态契约。
- 正式 tag release 仍执行完整 Python/MLX 安装、runner self-test、host-path 动态依赖检查、真实 runtime 打包、checksum 汇总和 release asset 上传。PR 的轻量门禁不能替代 tag release，但保证 workflow 语法引用和确定性打包结果不会到发版时才首次执行。
6. 用户 Prompt 通过仅当前进程可读的临时文件传入。runtime 始终先使用官方时间戳/说话人协议 Prompt，再追加用户上下文，避免自定义 Prompt 破坏结构化输出协议。
7. 整段推理采用硬 watchdog：耗时达到音频时长的 `0.5x` 时杀死子进程并返回 `moss_rtf_exceeded`，不会继续消耗资源或悄悄回退到不同模型。
8. release 打包禁用 macOS resource fork，并扫描拒绝 `._*`、`.DS_Store`、`__MACOSX`；安装器也跳过这些元数据，避免 Python 模型加载器误读 AppleDouble 文件。

`runtime_strategy`、文件并发、任务模型、任务语言、外部说话人分离与声纹匹配只控制标准 Qwen 链路；MOSS 联合模式固定模型、自动识别语言，并以单文件串行方式执行整文件推理。WebUI 必须按当前模式只展示真实生效的配置：标准模式展示 Qwen pipeline 配置并隐藏 MOSS Prompt，MOSS 模式只展示 MOSS Prompt 和模式说明，并隐藏整组 Qwen pipeline 配置。名称、音频目录、调度、递归扫描、启用状态和外接设备导入属于任务级公共配置，两种模式都展示。被隐藏的标准模式配置保留在表单状态和任务中，切回标准模式后继续使用，不因切换或保存 MOSS Prompt 被重置。

### 2.3 后续阶段

- 长驻 sidecar：固定 API 契约、限制 CORS、健康检查、取消和显式资源回收。
- 长音频进度：在不改变单次全局解码语义的前提下增加可观测进度、断点恢复和显式资源回收。
- 质量评估：人工标注小集上的说话人错误率、转录错误率、时间戳偏移与重复/漏字。

本阶段不把通用 `mlx-audio` server 暴露成网络服务，不承诺 MOSS 实时流式能力，也不把 MOSS 设为历史任务或新任务的默认模式。不能仅为缩短耗时把一段会议拆成多个独立 MOSS 请求：官方长录音示例明确提示跨 clip 的 speaker label 会重置，缺少可靠的跨块身份归并时会损失多人识别一致性。

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
| `moss-cpp` | 兼容登记（旧实验后端） | 否 | 是 | 是 | 是 | 是 |

任务层通过 `transcription_mode` 匹配能力；`standard` 使用 `qwen-openai`，`moss_joint` 使用发布包中的 `moss-mlx` runtime。`moss-cpp` 只保留能力兼容登记，不再用于正式任务执行。

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

### 6.1 官方推荐与本机后端选择

- OpenMOSS 官方模型卡将该模型定义为单次完成长音频转录、时间戳和说话人标注，最长支持 90 分钟；CUDA 服务端官方推荐 SGLang Omni，Transformers 作为直接调用示例。
- 官方长录音脚本在超长输入上按约 55 分钟切分，同时明确 speaker label 会在 clip 之间重置。因此 Bifrost 对 55 分钟以内输入保持整段单次解码，不用短片段独立推理换速度。
- Apple Silicon 没有官方 SGLang Omni/CUDA 路径。Bifrost 使用 MLX-Audio 的 Apple Silicon 移植，并固定 `majentik/MOSS-Transcribe-Diarize-MLX-8bit`；该模型卡报告其样例相对 MLX BF16 的 CER 为 0，而 4/6-bit 存在更明显差异。该结论只说明量化移植的一致性样例，不等于真实会议 WER/DER 真值。

参考：

- <https://huggingface.co/OpenMOSS-Team/MOSS-Transcribe-Diarize>
- <https://huggingface.co/datasets/uv-scripts/transcription/blob/main/moss-transcribe-diarize-server.py>
- <https://github.com/Blaizzy/mlx-audio>
- <https://huggingface.co/majentik/MOSS-Transcribe-Diarize-MLX-8bit>

### 6.2 2026-07-19 性能与质量代理验证

| 样本 | MLX 8-bit 耗时 | RTF | 结果 |
| --- | ---: | ---: | --- |
| 30 秒 | 2.05 秒 | 0.068 | 11 segments，2 speakers |
| 120 秒 | 4.07 秒 | 0.034 | 覆盖到 120.02 秒，25 segments，5 speakers |
| 1800.15 秒真实会议 | 83.261 秒 | 0.04625 | 覆盖到 1800.14 秒，248 segments，9 speakers |

120 秒同源样本与原 GGML Q5 输出规范化文本均为 319 字符，`SequenceMatcher` 相似度为 0.9969，speaker label 数量和时间线目视一致。该对比说明后端切换没有出现明显文本/多人结构退化，但没有人工标注，不能替代 WER、DER 或说话人身份真值评估。原 GGML 在整段 30 分钟输入上出现超过 1 小时的超线性退化，因此不再作为正式 Apple Silicon 后端。

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
- 运行时归档含 AppleDouble 元数据时，安装器必须忽略且不得落盘。
- 1.2 秒 fixture 推理达到 600 ms 时必须返回 `moss_rtf_exceeded`，并终止子进程。

### 7.3 `human_tests`

- 对现有 `day` 任务运行只读基准，验证约 10 分钟与约 30 分钟样本。
- 对比运行前后 `tasks.json`、`files.json` 与目标 WAV 哈希。
- 验证 30 分钟稀疏语音样本不会仅因尾部静音被报告为截断。
- 对 30 分钟真实任务执行完整 MLX MOSS 链路；推理阶段超过 900.075 秒必须立即中断并判失败，不能为了等待结果继续消耗资源。

## 8. 风险与回退

- 新模型运行时快速演进：发布 runtime 固定 MLX-Audio commit、Python 依赖和模型 snapshot/hash；Bifrost 核心只消费稳定 JSON 契约。
- 自动初始化下载中断：使用可续传临时文件，哈希不匹配时拒绝安装并保留明确错误；不会回退成 Qwen 后悄悄产出不同语义的结果。
- Prompt 泄露或协议破坏：Prompt 不放入命令行参数，临时文件随单次推理销毁；runtime 追加而非替换协议 Prompt。
- 长音频 token 截断：按音频时长计算输出 token budget，并保留完整性信息；超过 55 分钟在没有跨块 speaker 归并前拒绝处理。
- 说话人标签跨块漂移：正式路径使用一次全局解码，不采用独立短块；未来若切块，必须先具备跨块 voiceprint/聚类归并和 DER 回归门禁。
- 资源占用：MOSS 仅离线串行启用，Qwen 实时默认链路不变；0.5 RTF watchdog 是硬失败门禁。
- 回退：Provider 不可用或能力不匹配时继续使用现有 Qwen + diarization 流程。

## 9. Review/Fix/Test 门禁

第 1 轮重点检查响应兼容、旧调用方行为、时间单位、空 speaker、真实任务只读性；运行相关 crate 单测与基准 E2E。

第 2 轮基于最新 diff 复查历史任务默认值、Prompt 保存/清空、发布资产生成、下载校验、原生 speaker timeline、错误路径、文档和 `human_tests` 一致性，并复跑失败路径、真实基准与项目校验。远端 CI 执行 coverage 90% gate，本地不运行全量 coverage。
