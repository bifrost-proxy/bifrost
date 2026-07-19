# ASR MOSS 联合转录任务模式真实场景测试

## 功能模块说明

验证用户可在 ASR 目录任务中选择 `moss_joint` 模式、配置并持久化自定义 prompt，并在首次运行时自动安装可重定位 MLX runtime 和校验固定 8-bit 模型。测试使用既有 ASR 任务目标中的真实音频和 18997 预览数据目录，不修改 9900 生产服务、源音频或生产转录结果。性能验收硬门限为推理 RTF `<= 0.5`；超过门限必须杀死子进程并判失败。

## 前置条件

1. 当前机器为 Apple Silicon macOS，已安装 `ffmpeg`。
2. 已构建当前分支的 `target/debug/bifrost`，18997 预览服务使用该二进制，9900 生产服务保持原 PID。
3. 预览任务与真实样本：

   ```bash
   export ASR_TASK_ID=2a3e44aeee494d8682ac404e36cc746f
   export ASR_TASK_DIR=/Users/eden_studio/.bifrost/moss-preview/asr/tasks/$ASR_TASK_ID
   export MOSS_AUDIO=/Users/eden_studio/demo/TX02_MIC005_20260707_171914_orig.wav
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
3. 使用独立 `BIFROST_DATA_DIR=/Users/eden_studio/.bifrost/moss-preview` 和端口 18997 启动当前分支 Bifrost，并通过 `BIFROST_MOSS_RUNTIME_URL=file://...zip` 模拟正式 release 资产。
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

## 清理步骤

1. 用户要求继续体验时保留 18997；否则停止且仅停止本测试启动的预览 PID。
2. 不得删除 `$ASR_TASK_DIR`、转录产物或 `$MOSS_AUDIO`。失败资源只移动到带时间戳 quarantine，确认无需回滚后再清理。
3. 再次检查 9900 PID，确认生产服务未被重启或停止。

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
