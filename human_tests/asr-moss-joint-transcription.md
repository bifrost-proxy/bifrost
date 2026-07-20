# ASR MOSS 联合转录任务模式真实场景测试

## 功能模块说明

验证用户可在 ASR 目录任务中选择 `moss_joint` 模式、配置并持久化自定义 prompt，并在首次运行时自动安装可重定位 MLX runtime 和校验固定 8-bit 模型。隔离功能验证使用 18997/临时数据目录；TC-MOSS-09 按用户明确要求重启 9900 默认服务并继续 `~/.bifrost` 中的既有任务。性能验收硬门限为文件进入 Processing 到完成的端到端 RTF `<= 0.5`；达到门限必须杀死子进程并判失败。任何真实续跑都不得重新解码已成功资源。

## 前置条件

1. 当前机器为 Apple Silicon macOS，已安装 `ffmpeg`。
2. 已构建当前分支的 `target/debug/bifrost`，18997 预览服务使用该二进制，9900 生产服务保持原 PID。
3. 预览任务与真实样本：

   ```bash
   export ASR_TASK_ID=2a3e44aeee494d8682ac404e36cc746f
   export ASR_TASK_DIR="$HOME/.bifrost/moss-preview/asr/tasks/$ASR_TASK_ID"
   export MOSS_AUDIO=~/path/to/a/real-long-recording.wav
   ```

4. 已按 `.github/workflows/release.yml` 固定 MLX-Audio commit `64e8416c303fb3b3463dab8eb4ebd78c55a87c1a`、Python requirements 和模型 snapshot。模拟 release URL 时同时设置 runtime zip 的 `BIFROST_MOSS_RUNTIME_SHA256`。模型权重按 1,258,427,442 bytes 与 SHA-256 `469a8969e6b70c8b276411eca54a355a27de9ed6794f738dab53f4ffd3c83190` 校验。

## 测试用例

### TC-MOSS-01：任务模式选择、prompt 校验与重启持久化

操作步骤：

1. 执行 `SKIP_BUILD=true bash e2e-tests/tests/test_asr_moss_task_mode.sh`。
2. 脚本在隔离数据目录创建 `moss_joint` 任务，保存带 CRLF 和首尾空白的 prompt。
3. 读取创建响应，更新为空 prompt，提交 4001 字符的非法 prompt，创建没有新字段的旧格式任务，然后重启临时 Bifrost 并再次读取任务。
4. 在隔离 WebUI 打开 New Directory Task，选择 `MOSS joint transcription (speaker-aware)`，填写 prompt 并保存；重新打开 Edit 对话框检查 prompt 和模式。

预期结果：

- `transcription_mode=moss_joint` 和规范化后的 prompt 写入任务；空字符串可清除 prompt。
- 4001 字符 prompt 返回 HTTP 400；旧格式任务默认为 `standard` 和空 prompt。
- 服务重启后配置不丢失；MOSS 模式的有效文件并发数固定为 1。
- WebUI 选择 MOSS 后展示自动初始化说明和 prompt 输入框，并完全隐藏不适用的 Qwen runtime/model/language/并发/外部分轨控件；重开编辑框后 prompt 原文仍存在。

### TC-MOSS-02：首次运行自动初始化与真实任务音频转录

操作步骤：

1. 记录生产 9900 PID、预览 `files.json` 和真实 WAV 的 SHA-256。
2. 执行 `bash e2e-tests/tests/test_asr_moss_release_contract.sh`，确认 release workflow 与 Rust 初始化器的固定源码、模型、元数据、资产名和 checksum 契约一致。
3. 使用独立 `BIFROST_DATA_DIR="$HOME/.bifrost/moss-preview"` 和端口 18997 启动当前分支 Bifrost，并通过 `BIFROST_MOSS_RUNTIME_URL=file://...zip` 模拟正式 release 资产。
4. 通过 API 创建或读取 `moss_joint` 任务，确认 prompt 字段；触发 `/run`，轮询直到任务不再运行。
5. 推理进程开始后记录 PID、命令行和 `started_at_ms`；若 1800.15 秒音频运行达到 900.075 秒仍未完成，立即调用 `pause?force=true` 并判失败。

预期结果：

- 首次运行自动创建 `~/.bifrost/asr/moss_joint_mlx/runtime` 与 `model/model.safetensors`，不会写系统 Python。
- 可重定位 Python/MLX runtime smoke check、模型大小和 SHA-256 校验通过；归档中的 macOS 元数据不得落盘。
- PR CI 会在不下载模型权重的前提下检查 release metadata 至少覆盖运行时必需文件，且权重不会重复塞入 runtime zip。
- 子进程命令必须是 `moss_joint_mlx/runtime/python/bin/python3.12 ... moss_mlx_runner.py`，不能再调用旧 GGML `moss-transcribe`。
- 1800.15 秒真实音频成功转录，任务汇总为 `processed=1`、`failed=0`，推理耗时不超过 900.075 秒。

### TC-MOSS-03：原生说话人时间线与 prompt 隐私边界

操作步骤：

1. 从隔离任务的 `files.json` 取得 file key，通过 API 读取 timeline。
2. 检查 timeline 的 `model`、`diarization_profile` 和 segments，并检查同文件旁路 metadata JSON。
3. 在隔离输出目录和 Bifrost 日志中搜索完整自定义 prompt。

预期结果：

- timeline 使用 MOSS 模型标识和 `moss_joint_native` profile，包含多个有时间范围的原生 speaker segment。
- metadata 只记录 `transcription_mode=moss_joint` 与 `transcription_prompt_configured=true`，不写入 prompt 正文。
- 外部 diarization 配置即使为 enabled，也不会覆盖 MOSS 原生 speaker。

### TC-MOSS-04：线上任务/源音频不变与发布产物自包含

操作步骤：

1. 比较测试前后的线上 `files.json` 与真实 WAV SHA-256。
2. 用 `otool -L` 检查被自动安装的 Python，执行 runner `--self-test`，扫描安装目录中的 `._*` / `.DS_Store`。
3. 测试完成后按用户体验要求保留 18997 服务；只清理由失败归档生成的可恢复 quarantine，或在交付后征得用户同意再清理。

预期结果：

- 线上任务与源 WAV 哈希完全一致。
- Python 可在安装位置以 `PYTHONHOME` / `PYTHONPATH` 离线启动，不依赖 build-host 路径；安装目录不存在 macOS metadata sidecar。
- 线上 9900 服务及 `day` 任务配置未被修改；测试临时目录可完整清理。

### TC-MOSS-05：0.5 RTF watchdog、完整时间轴与多人结构代理验证

操作步骤：

1. 对 1.2 秒 fixture 使用会 sleep 2 秒的假 runtime 调用 `run_moss_joint_transcription`。
2. 对 30 秒、120 秒与 1800.15 秒真实音频记录 MLX 推理耗时；任何样本 RTF 超过 0.5 时立即中断后续长样本。
3. 检查 1800.15 秒产物的首末时间、segment 数、空 speaker、speaker 唯一值和模型标识。
4. 在 120 秒同源音频上比较 MLX 8-bit 与原 GGML Q5 的规范化文本长度、相似度、speaker 数和时间线；不把没有人工真值的比较表述为 WER/DER 结论。

预期结果：

- 慢 fixture 在约 600 ms 返回 `moss_rtf_exceeded`，子进程退出。
- 所有继续执行的真实样本 RTF 均 `<= 0.5`；30 分钟样本时间轴覆盖到最后 1 秒内。
- 每个 segment 都有非空 speaker，整段使用同一次解码维持全局 speaker label。
- MLX 与 GGML 的短样本结果没有明显文本或多人结构退化；最终准确率仍需人工标注 WER/DER 数据集确认。

### TC-MOSS-06：处理模式驱动的表单字段与配置保留

操作步骤：

1. 使用当前源码构建的 Bifrost 和隔离 `BIFROST_DATA_DIR` 启动非正式端口；记录 9900 与 18997 现有服务 PID，测试期间不得停止或重启它们。
2. 在浏览器打开 ASR Scheduled Tasks，点击 New；Standard 模式下检查 Runtime、File Concurrency、Speaker Diarization、Diarization Profile、Known Speakers、Voiceprint Matching、Task Model 和 Task Language。
3. 把 Task Model 改为 `Qwen3-ASR-1.7B`，再把 Transcription Mode 改为 `MOSS joint transcription (speaker-aware)`，填写自定义 MOSS Prompt。
4. 检查上述 Standard 专属字段已从表单移除；Recursive、Enabled 和 External Devices 仍可见。切回 Standard，确认模型仍为 `Qwen3-ASR-1.7B`；再次切回 MOSS，确认 Prompt 原文仍在。
5. 填写隔离音频目录并保存任务，重开 Edit，确认当前模式和 Prompt；切回 Standard，确认原标准模式配置没有被 MOSS 保存动作重置。
6. 分别在亮色和暗色主题检查弹窗，确认字段切换后没有空白占位、文字重叠、底部按钮遮挡或水平溢出。

预期结果：

- Standard 模式只显示 Qwen pipeline 专属字段，不显示 MOSS Prompt。
- MOSS 模式只显示 MOSS Prompt 和模式说明，不显示任何不会生效的 Qwen pipeline 字段。
- 名称、音频目录、调度、Recursive、Enabled 和 External Devices 等任务级公共字段在两种模式下始终可用。
- 模式往返和保存重开不会丢失各自配置；亮色、暗色与窄窗口布局均可正常操作。

### TC-MOSS-07：统一模型管理页初始化入口与真实资产状态

1. 使用当前源码构建 `target/debug/bifrost`，以临时 `BIFROST_DATA_DIR`、端口 18998、`--no-system-proxy --no-intercept` 启动隔离服务；不得停止或重启 9900、18997。
2. 请求 `GET /_bifrost/api/asr/moss/status`，记录 platform、runtime/model 校验状态、安装目录和预期权重大小。
3. 使用 Chrome 打开 `http://127.0.0.1:18998/_bifrost/ai?aiSection=tools-asr&asrTab=management`，在 Model Management 的 Model 中选择 `MOSS joint transcription (MLX 8-bit)`。
4. 检查页面只展示 MOSS 实际生效的 Execution、自动语言、Runtime/Model 组件状态和 `~/.bifrost/asr/moss_joint_mlx`；不展示 Qwen Host、Service Port 或可租约服务含义。
5. 若真实资产已 Ready，确认 Runtime/Model 均为 verified 且不重复下载；若缺失，确认 Initialize 可见。任务执行侧的首次运行自动初始化仍作为兜底，不产生第二套目录。

预期结果：

- API 只在打包 Python/runner/site-packages 自检与固定 snapshot 权重校验均通过时返回 `status=ready`、`installed=true`。
- MOSS 与 Qwen 共用 Model Management 入口，但页面字段和说明按模型执行方式切换。
- 管理页、任务自动初始化和真实推理都使用 `~/.bifrost/asr/moss_joint_mlx`，不会写系统 Python。

### TC-MOSS-08：release.yml 的 PR CI 打包与 checksum 门禁

1. 执行 `bash e2e-tests/tests/test_asr_moss_release_contract.sh`，校验 `release.yml`、Rust 初始化器、固定源码/模型、metadata、requirements、资产名和共享打包脚本引用。
2. 在 macOS 执行 `bash scripts/ci/test-package-moss-release-runtime.sh`。fixture 首先带一个 `._config.json`，断言共享打包器拒绝该产物；删除 sidecar 后再次打包。
3. 确认生成的 `moss-joint-runtime-v0.0.0-aarch64-apple-darwin.zip` 包含 runtime 入口、12 个模型 metadata、license/notice，不包含 AppleDouble/DS_Store/__MACOSX，并且 `.sha256` 可复算。
4. 检查 `.github/workflows/ci.yml` 存在 macOS `Release Workflow Contract (MOSS macOS)` job，同时执行上述静态契约和共享打包器 fixture。
5. 执行 `bash scripts/ci/test-macos-release-core-payload.sh`，确认正常 CLI tar.gz/tar.xz、Desktop `.app` 与最终 DMG fixture 通过；混入 MOSS runtime、`.safetensors` 权重或超过 512 MiB 的主包必须失败。
6. 检查普通 PR CI 的两种 macOS CLI build 与两种 Desktop bundle jobs 均对实际 binary、`.app` 和 DMG 调用同一门禁，而不只运行 fixture。

预期结果：

- PR CI 无需下载 1.2 GB 权重即可真实执行与正式 release 相同的确定性打包和 checksum 逻辑。
- macOS CLI 主 archive 与 Desktop DMG 不携带 MOSS runtime、依赖或权重；用户选择初始化或首次运行时才动态下发到 `~/.bifrost/asr/moss_joint_mlx`。
- 正式 tag release 仍负责完整 MLX/Python 安装、自检和真实 runtime 产物；如果共享打包器或资源契约漂移，PR 阶段即失败。

### TC-MOSS-09：默认 9900 服务只续跑未完成资源与资源保护回归

操作步骤：

1. 确认任务 `735775510b384fff8903d9c6fc54f1a3` 为 `moss_joint` 且已强制暂停。读取默认 `~/.bifrost/asr/tasks/<task-id>/files.json`，列出磁盘仍存在的 Pending/Failed 文件；确认 Success/PartialSuccess 不在待执行集合。
2. 对已成功 MOSS 样本 `TX01_MIC052_20260624_123014_orig.wav` 记录 record status、`started_at_ms`、`finished_at_ms`、`text_chars`，并计算 source、text、metadata、timeline 四个文件的 SHA-256，保存快照。
3. 构建当前源码的 release CLI；备份并替换 `~/.local/bin/bifrost`。备份现有 `~/.bifrost/asr/moss_joint_mlx/runtime/moss_mlx_runner.py` 后安装当前 runner，保留已验证的 1.2 GB 权重，不重复下载或复制模型。
4. 使用默认 `BIFROST_DATA_DIR=$HOME/.bifrost` 停止旧 9900 服务，以原 host/port/system-proxy 语义启动新的 detached 9900 服务；确认 PID 更新、API ready、任务仍暂停、模型管理状态 Ready。
5. 恢复该任务。按来源时间排序观察前三条未完成资源：旧版耗时 530.363 秒的稀疏 1800.15 秒文件、缺时长文件、2.533 秒短文件。稀疏文件必须由 256-token 协议保护快速停止；后两条必须在 MLX 启动前分别返回 `moss_duration_unavailable` 与 `moss_audio_too_short`。
6. 继续到下一条正常 1800.15 秒待处理文件成功，记录端到端耗时、RTF、segment 数和 speaker 数。若任何文件从 `started_at_ms` 到 `finished_at_ms` 的 RTF `> 0.5`，立即调用 `pause?force=true`，终止测试并判失败。
7. 得到稀疏早停和一条正常成功证据后立即再次强制暂停，避免继续消费剩余队列。重新生成已完成样本快照并逐字段、逐哈希比较。
8. 检查 MOSS 子进程已退出，9900 服务仍可用，任务队列只减少本次实际处理的未完成资源；不调用任何强制重跑 Success 文件的 API。

预期结果：

- 默认 9900 服务确实运行当前修复版本，仍使用 `~/.bifrost` 和同一个任务/模型目录；权重不重复下载，macOS 发布主包也不增加模型体积。
- 稀疏/无合法协议输出不再消耗数分钟，缺时长、短音频和数字静音不会启动约 2 GiB MLX 子进程。
- 正常多人长音频继续一次全局联合解码，不做会重置 speaker label 的独立短块；端到端 RTF `<= 0.5`。
- 已成功 MOSS 样本的状态、开始/结束时间、文本长度和四个 SHA-256 全部不变，证明服务重启和任务恢复没有重复解码完成资源。
- 验证结束时任务为 paused、无 MOSS 子进程，9900 保持可体验；剩余未完成资源留在原队列供后续显式恢复。

### TC-MOSS-10：Review 评论回归、平台能力与资产自修复

操作步骤：

1. 执行 `! rg -n '/(Users|home)/[^/]+/' human_tests/asr-moss-joint-transcription.md`，确认测试说明不依赖开发者机器路径。
2. 执行 `cargo test -p bifrost-admin moss_ --lib -- --nocapture`，覆盖 MOSS 原生 diarization ready、平台门禁、metadata 缺失/损坏修复、生成长度终止、倒置和越界时间戳、缺时长与严格 watchdog；metadata 修复用例会在删除模型下载源后继续执行，证明已通过 SHA-256 校验的现有权重可离线复用。
3. 执行 `pnpm --dir web exec vitest run src/pages/ASR/directoryTaskMode.test.ts`，确认 MOSS 选项在能力确认前及不支持的平台禁用，只在 Apple Silicon macOS 启用。
4. 执行 `bash e2e-tests/tests/test_asr_moss_release_contract.sh`，确认 runner 保留 `completed|length`、release tag 版本经 `BIFROST_VERSION` 注入资产 URL，并继续执行固定 snapshot/打包契约。
5. 执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_asr_moss_task_mode.sh`。Apple Silicon 上验证创建、PATCH、重启持久化；不支持的平台验证创建和切换均返回 HTTP 400。
6. 用当前二进制在临时数据目录和非正式端口启动服务，浏览器进入 ASR Scheduled Tasks 并打开 New Directory Task。确认平台支持时 MOSS 选项可选；切换亮色和暗色主题，检查 disabled/说明文字、表单尺寸和可读性。

预期结果：

- MOSS 任务摘要把模型原生 speaker 视为 ready，不要求外部 diarization 资产。
- runtime/model metadata 缺失或内容改变会撤销 Ready，并从已校验 runtime archive 恢复；有效权重不会重复下载。
- 达到生成 token 上限时任务失败且不发布截断全文；倒置时间戳被拒绝，超出音频的片段被丢弃或裁剪。
- 严格端到端 `0.5x` watchdog 不增加启动宽限；短于 10 秒和缺时长输入在 MLX 启动前明确失败。
- WebUI 与 API 使用同一平台边界，亮色和暗色下状态表达清晰且不依赖硬编码颜色。

### TC-MOSS-11：Runtime 依赖完整性、整文件耗时与基准样本唯一性

操作步骤：

1. 执行 `cargo test -p bifrost-admin moss_ --lib -- --nocapture`，确认 verification marker 校验 `site-packages` 非缓存文件，并在状态读取时真实执行带硬超时的打包 Python `--self-test`；删除或损坏 `runtime/python/lib` 下的 framework fixture、删除或损坏 `site-packages`、自检进程卡死都会撤销 runtime Ready。
2. 在同一 Rust 回归中执行 MOSS task fixture，确认成功结果包含唯一一条 `runner=moss_joint`、`status=ok`、`elapsed_ms>=1` 的整文件 metric，且时长、文本字符数和文本 SHA-1 与产物一致。
3. 执行 `bash e2e-tests/tests/test_asr_joint_transcription_benchmark.sh`，确认正常的 600/1800 秒目标选择两个不同录音；请求 4 个目标但只有 3 个不同成功源文件时明确报错，且不生成报告。

预期结果：

- 删除、增加或修改已登记的 `site-packages` 依赖文件，或删除、损坏 Python framework、自检超时后，Model Management 不再显示 Ready，修复入口重新可用；卡死的自检子进程不会无限阻塞状态接口。
- 每个成功 MOSS 文件的 `files.json` 都有可供 benchmark 汇总的真实整文件 elapsed metric，RTF 不再因空 metrics 固定为零。
- benchmark 不会让同一 `source_path` 同时代表多个目标时长；不同成功源文件不足时 fail closed。

### TC-MOSS-12：完整模型 metadata 与配置变更重处理

操作步骤：

1. 执行 `cargo test -p bifrost-admin moss_ --lib -- --nocapture`，确认模型 verification marker 覆盖 release packager 要求的 12 个 metadata 文件，任一文件缺失或损坏都会撤销 model Ready。
2. 执行 `bash e2e-tests/tests/test_asr_moss_release_contract.sh`，确认 Rust 校验列表、release workflow 下载列表和 runtime packager 必需列表保持一致。
3. 执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_asr_moss_task_mode.sh`。在 Apple Silicon 上先写入成功文件记录，再修改 MOSS prompt，确认记录变为 pending，旧产物引用和 metrics 被清空；不支持平台继续验证 MOSS 创建/PATCH 返回 400。

预期结果：

- `added_tokens.json`、`chat_template.jinja`、`merges.txt`、`vocab.json` 等全部发布 metadata 与原有文件一样受 checksum 保护和自动修复。
- 实际切换转录模式或修改生效中的 MOSS prompt 后，成功、部分成功、失败或遗留 processing 记录都会重新排队，下一次运行不会 merge-only 保留旧转录。
- 相同值 PATCH 不制造无意义的重复转录；任务运行中仍拒绝高风险配置变更。

### TC-MOSS-13：无有效协议结果去重与原生长片段规范化

操作步骤：

1. 执行 `cargo test -p bifrost-admin moss_ --lib -- --nocapture`，让 fixture 返回 `MOSS MLX returned no valid speaker-aware segments`，确认错误带当前版本 `moss_non_retryable_v*` 前缀。
2. 让 token-budget 回归传入超过 `MOSS_MAX_WHOLE_FILE_SECONDS` 的时长，确认 `moss_audio_too_long` 同样带当前版本 `moss_non_retryable_v*` 前缀。
3. 在同一回归中让 MOSS fixture 返回一个 63.8 秒的 S02 连续片段，确认产物 timeline 拆成最长 30 秒的片段，所有拆分片段保留 S02，末端绝对时间为 75000 ms，拼接文本仍为完整字母串。
4. 执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_asr_moss_task_mode.sh`，确认配置变更 E2E 仍通过且临时服务/目录被清理。
5. 执行 `cargo test -p bifrost-admin moss_hash_model_and_archive_validation_cover_success_and_failures --lib -- --nocapture`、`bash scripts/ci/test-package-moss-release-runtime.sh` 和 `bash e2e-tests/tests/test_asr_moss_release_contract.sh`，确认 runtime ZIP 的相对符号链接被保留、逃逸链接被拒绝，release packager 会解压成品并从解压目录执行 self-test。

预期结果：

- 无有效 timestamp/speaker segment 的确定性结果不会在源文件和版本未变化时重复加载模型。
- 超出整文件上限的确定性输入不会在源文件和版本未变化时重复归一化。
- MOSS timeline、SRT/VTT 不产生超过 30 秒的单 cue；拆分不丢 speaker、绝对时间或文本。
- 独立 CPython runtime 解压后仍保留 framework/library 相对符号链接；恶意逃逸链接不会写出安装根目录，发布归档通过 extract-then-self-test。
- Unix 上已有目录不会被 runtime symlink 替换；Windows 不尝试物化 archive symlink，即使目标路径已存在目录也稳定返回 `unsupported on this platform`，两种平台都保持安全拒绝。

### TC-MOSS-14：独立 Runtime 与核心 beta Release 隔离演练

操作步骤：

1. 执行 `bash e2e-tests/tests/test_asr_moss_release_contract.sh`，确认核心 `release.yml` 不包含 MOSS builder、Python/MLX 安装或 runtime asset，独立 `moss-runtime-release.yml` 固定调用共享 builder/packager。
2. 下载并复算 builder 固定的 `python-build-standalone` CPython 3.12 arm64 SHA-256；把归档移动到另一目录后执行 `python3.12 -c 'import ssl, sqlite3'`，并用 `otool -L` 确认没有 runner/toolcache 绝对依赖。
3. 执行 `bash scripts/ci/test-package-moss-release-runtime.sh`，确认 universal Mach-O 的 architecture header 不会被误判成 dylib 依赖，同时真实指向 runner/toolcache 的缩进依赖项仍被拒绝。
4. 从修复分支创建唯一的 `moss-runtime-v1.0.0-beta.*` tag，触发真实独立 Runtime workflow；看护 build、checksum、GitHub prerelease 到成功，并下载资产复算 checksum。
5. 发布稳定 `moss-runtime-v1.0.0` 后，从同一修复分支创建唯一的 `v0.0.157-beta.*` tag；看护核心 CLI、Desktop、combined checksum 和 prerelease 到成功，确认核心 Release 不包含 MOSS runtime asset。

预期结果：

- 独立 Runtime workflow 使用经过 SHA-256 固定、可重定位的 Python，不依赖 GitHub hosted toolcache 路径。
- Runtime beta/stable Release 分别生成独立 zip 和 `.sha256`，不包含 `model.safetensors`；模型权重继续在用户初始化时下载。
- 核心 beta Release 全部 job 成功且不下载/构建/上传 MOSS runtime；CLI、App、DMG 的包体轻量门禁继续生效。

## 清理步骤

1. 用户要求继续体验时保留 18997；否则停止且仅停止本测试启动的预览 PID。TC-MOSS-09 的 9900 服务按用户要求保留运行，但任务在取得有限验证证据后重新暂停。
2. 不得删除 `$ASR_TASK_DIR`、转录产物或 `$MOSS_AUDIO`。失败资源只移动到带时间戳 quarantine，确认无需回滚后再清理。
3. 对 TC-MOSS-01 至 TC-MOSS-08 再次检查 9900 PID 未变化；TC-MOSS-09 必须确认 PID 已按计划更新、默认数据目录不变且成功记录快照不变。

## 执行记录

| 日期 | 用例 | 实际结果 |
| --- | --- | --- |
| 2026-07-18 | TC-MOSS-01 | PASS：API 创建、规范化、清空、超长拒绝、旧任务默认值及重启持久化全部通过；MOSS 有效并发为 1。内置浏览器真实验证选择 MOSS 后出现自动初始化文案与 prompt 输入，runtime/model 等不适用控件 disabled；保存 `Bifrost、NextOnCall 是专有名词。` 后重开 Edit 原文仍存在。 |
| 2026-07-18 | TC-MOSS-02 | PASS（修复后复测）：首次自动安装 2.3 MB 静态 runtime 和 648174592-byte Q5 模型；首次测试发现 GGML residency-set 退出断言并设置 `GGML_METAL_NO_RESIDENCY=1` 修复。617.210 秒真实音频复测为 `processed=1`、`failed=0`。 |
| 2026-07-18 | TC-MOSS-03 | PASS：timeline 119 segments、8 speakers（S01-S08）、最后语音终点 606010 ms、profile=`moss_joint_native`；metadata 仅含 `transcription_mode=moss_joint`、`transcription_prompt_configured=true`，日志与转录产物未出现 prompt 正文。 |
| 2026-07-18 | TC-MOSS-04 | PASS：静态 runtime 无 `@rpath/libggml*` 依赖；线上 `day` 任务未运行且原安装版本没有新字段；线上 `files.json` SHA-256 为 `713578de862a619d7676ac85bf423ad149eba6bbfd67848ec37f1760bcf2e289`，源 WAV SHA-256 为 `3d7ec54485b498833220520db9fab63218ecf20ffa2a7dddd69a253650770795`，前后完全一致。 |
| 2026-07-19 | TC-MOSS-01 | PASS：API/E2E 再验证模式、prompt 保存/清空、4001 字符拒绝、旧任务默认值和重启持久化；WebUI 文案已更新为 MLX 全局单次解码和 0.5 RTF watchdog。 |
| 2026-07-19 | TC-MOSS-02 | PASS：18997 真实任务自动安装到 `~/.bifrost/asr/moss_joint_mlx`，进程为打包 Python + `moss_mlx_runner.py`；1800.15 秒音频 83.261 秒完成，RTF 0.04625，`processed=1`、`failed=0`。 |
| 2026-07-19 | TC-MOSS-03 | PASS：timeline 248 segments、9 speakers（S01-S09）、无空 speaker，首段 120 ms、末段 1,800,140 ms；metadata 模型为 `MOSS-Transcribe-Diarize-MLX-8bit` 且不含 prompt 正文。 |
| 2026-07-19 | TC-MOSS-04 | PASS：真实打包首次发现 AppleDouble UTF-8 失败并在 2.58 秒停止；打包与安装双层过滤修复后，安装目录 metadata sidecar 数为 0。9900 保持 PID 22956；18997 按用户要求保留运行供体验。 |
| 2026-07-19 | TC-MOSS-05 | PASS：1.2 秒慢 fixture 约 600 ms 返回 `moss_rtf_exceeded`；30 秒 2.05 秒、120 秒 4.07 秒、1800.15 秒 83.261 秒，均小于 0.5 RTF。120 秒 MLX/GGML 规范化文本均 319 字符，相似度 0.9969，speaker 数与时间线目视一致；未宣称 WER/DER。 |
| 2026-07-19 | TC-MOSS-02 | PASS（发布门禁复核）：新增发布契约 E2E 通过，确认 MLX-Audio commit、模型 snapshot、12 个 metadata 文件、runtime 资产名与 checksum manifest 一致；仅保留发布清单文件的 30 秒真实音频推理 1.753 秒完成，RTF 0.05842，得到 9 段、3 个 speaker。 |
| 2026-07-19 | TC-MOSS-06 | PASS（修复后复测）：使用当前源码与隔离数据目录在 18998 启动真实服务；Standard 的 8 个 Qwen pipeline 字段均显示且 MOSS Prompt 隐藏，MOSS 下 8 个字段均从 DOM 移除且 Prompt、Recursive、Enabled、External Devices 保持可用。模式往返保留 Prompt 与 `Qwen3-ASR-1.7B`；首次真实保存发现隐藏字段未进入 `validateFields()` 导致模型回落，改用校验后读取完整表单状态并复测，保存 MOSS、重开 Edit、切回 Standard 后仍为 1.7B。Prompt API 回显原文，字符计数无重叠，亮色与暗色布局均无空白占位、遮挡或水平溢出。9900 保持 PID 22956，18997 保持 PID 56155。 |
| 2026-07-19 | TC-MOSS-07 | PASS（性能修复后复测）：当前源码服务以临时数据目录在 18998 启动。首次真实状态读取发现全量权重哈希与 MLX 自检超过 30 秒，改为初始化时写入包含固定模型 SHA-256/大小/mtime 及 Python/runner 哈希的 schema v2 `verification.json`；修复后未初始化状态 0.002771 秒返回，真实资产完整校验生成 v2 标记后 Ready 状态 0.599707 秒返回。真实 Chrome 选择 MOSS 后显示 Ready、On demand / whole file、Automatic multilingual、Runtime/Model verified、1.17 GB / 1.17 GB 和 `~/.bifrost/asr/moss_joint_mlx`，Model Management 不再显示 Qwen Host/Service Port。9900/PID 22956 与 18997/PID 56155 未变化，18998 已停止且临时目录已清理。 |
| 2026-07-19 | TC-MOSS-08 | PASS：静态发布契约确认 MLX-Audio/model 固定 commit、12 个 metadata、1,258,427,442-byte 权重 SHA-256、共享 packager 与 macOS PR CI job 一致；真实 macOS fixture 先因 `._config.json` 被拒绝，清理后生成 zip 和可复算 `.sha256`，runtime 入口、metadata、license/notice 齐全。 |
| 2026-07-19 | TC-MOSS-08 | PASS（动态下发边界复测）：CLI tar.gz/tar.xz、Desktop `.app` 与实际挂载的 fixture DMG 均通过轻量核心包检查；混入 `moss-joint-runtime`、`model.safetensors` 或超过配置上限均被拒绝。runtime packager 同时拒绝权重，只允许 runtime、固定 metadata 与 license/notice，权重继续由初始化器单独下载。 |
| 2026-07-19 | TC-MOSS-09 | PASS（两次发现并修复真实质量/资源缺口后复测）：使用 release CLI 重启默认 `~/.bifrost:9900`，真实队列验证 daemon PID `98617`；最终源码重新构建安装后 daemon PID `36918`，模型仍在 `~/.bifrost/asr/moss_joint_mlx` 且 `installed_model_bytes=1258427442`，未重复下载。初始 20 个磁盘仍存在的未完成资源中，旧版稀疏 1800.15 秒文件从 530363 ms/RTF 0.2946 降至 16293 ms/RTF 0.00905；缺时长 335 ms 返回 `moss_duration_unavailable`，2.533 秒文件 342 ms 返回 `moss_audio_too_short`。首次协议保护只检查前缀仍让稀疏文件运行 219 秒，立即 force-pause 后收紧为 256 token 内必须形成完整正时长片段；随后发现一个 462966 ms 的重复“嗯”零时长输出被错误包装为 success，立即隔离该轮新产物、恢复为未完成并增加 Python/Rust 双层退化拒绝，复测 16199 ms 正确失败。增加同版本确定性失败去重后，新一轮待执行总数由 20 降到 11，不再重复加载上述坏输入；正常 1800.15 秒未完成文件 `TX02_MIC027_20260714_135743_orig.wav` 最终 150771 ms/RTF 0.08375 成功，11001 字、353 segments、9 speakers，时间轴 10–1799740 ms。每次发现无价值路径都 force-pause，所有完成/失败 RTF 均未超过 0.5。最终服务确认任务 `paused=true/running=false`、无 MOSS 子进程，9900 继续运行。重启前已成功样本 `TX01_MIC052_20260624_123014_orig.wav` 的 status、started/finished、5217 字及 source/text/metadata/timeline 四个 SHA-256 前后完全一致，证明没有重跑已完成资源。 |
| 2026-07-20 | TC-MOSS-10 | PASS：路径可移植性检查无命中；Rust MOSS 回归 21/21，其中 metadata 损坏后先删除模型下载源，初始化仍复用已校验权重并仅恢复 metadata；Web 模式选项 3/3、release 契约和真实 task-mode E2E 均通过。隔离服务运行在 18996 且系统代理保持关闭；Puppeteer Chrome 在 1280×900 下逐项验证亮色和暗色主题，New Directory Task 的 MOSS 选项在 Apple Silicon 上均可见、可选，19/19 浏览器步骤通过、59 个 API 请求无失败。测试服务、临时数据目录和临时场景文件均已清理。 |
| 2026-07-20 | TC-MOSS-11 | PASS：Rust MOSS 回归确认 `site-packages` 内容损坏，以及 `runtime/python/lib` framework 缺失或损坏都会撤销 Ready；成功 MOSS task 生成一条非零整文件耗时 metric。benchmark E2E 正常选择不同录音，并在 4 个目标只有 3 个不同成功源文件时明确失败且不写报告。 |
| 2026-07-20 | TC-MOSS-12 | PASS：Rust MOSS 回归 20/20 覆盖完整 12 个发布 metadata，配置重排状态矩阵/幂等单测 1/1；release contract E2E 确认下载、打包、Rust 校验列表一致；真实 task-mode API E2E 在修改 MOSS prompt 后把成功记录重置为 pending 并清空旧产物引用/metrics。 |
| 2026-07-20 | TC-MOSS-13 | PASS：Rust MOSS 回归把无有效 speaker segment 和超出整文件上限都标为带版本的确定性失败；65 秒 fixture 的 63.8 秒 S02 turn 被拆为 3 段，加上 S01 共 4 段，所有段不超过 30 秒，speaker、75000 ms 绝对终点和完整文本均保留；runtime ZIP 的 Python 相对 symlink 保留且逃逸 symlink 被拒绝，Unix 目录冲突与 Windows symlink 不支持均保持安全拒绝，release contract 强制 extract-then-self-test；task-mode API E2E 继续通过。 |
