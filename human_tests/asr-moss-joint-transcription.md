# ASR MOSS 联合转录任务模式真实场景测试

## 功能模块说明

验证用户可在 ASR 目录任务中选择 `moss_joint` 模式、配置并持久化自定义 prompt，并在首次运行时自动安装原生运行时和校验固定 Q5 模型。测试复用既有 `day` 任务目标中的真实音频，但使用独立的 HOME、数据目录和管理端口，不修改线上任务、源音频或既有转录结果。

## 前置条件

1. 当前机器为 Apple Silicon macOS，已安装 `ffmpeg`。
2. 已构建当前分支的 `target/debug/bifrost`。
3. 线上只读任务与真实样本：

   ```bash
   export ASR_TASK_ID=735775510b384fff8903d9c6fc54f1a3
   export ASR_TASK_DIR=/Users/eden_studio/.bifrost/asr/tasks/$ASR_TASK_ID
   export MOSS_AUDIO=/Users/eden_studio/audio/LEFT/TX_MIC005_20260707_104639/TX01_MIC040_20260713_190050_orig.wav
   ```

4. 已按 `.github/workflows/release.yml` 的固定 commit 和补丁构建静态 `moss-transcribe`，将其打包为与当前 Bifrost 版本一致的 zip；模拟 release URL 时同时设置该 zip 的 `BIFROST_MOSS_RUNTIME_SHA256`。真实模型仍由初始化器从固定 URL 下载，并按 648174592 bytes 与 SHA-256 `7e9ce1de5648ed49fc5c4f5e003d61a7421a63c14074f7275dc8a8cc664ff865` 校验。

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
- WebUI 选择 MOSS 后展示自动初始化说明和 prompt 输入框，禁用不适用的 Qwen runtime/model/外部分轨控件；重开编辑框后 prompt 原文仍存在。

### TC-MOSS-02：首次运行自动初始化与真实任务音频转录

操作步骤：

1. 记录线上 `files.json` 和真实 WAV 的 SHA-256。
2. 创建独立临时目录，将真实 WAV 只读链接到测试音频目录。
3. 使用独立 `HOME`、`BIFROST_DATA_DIR` 和端口启动当前分支 Bifrost，并通过 `BIFROST_MOSS_RUNTIME_URL=file://...zip` 模拟正式 release 资产。
4. 通过 API 创建 `moss_joint` 任务，prompt 为 `Bifrost、NextOnCall 是专有名词。保留会议中的说话人标签和完整时间戳。`，触发 `/run`，轮询直到任务不再运行。

预期结果：

- 首次运行自动创建 `~/.bifrost/asr/moss_joint/moss-transcribe` 与 `moss-transcribe-q5_0.gguf`。
- 原生运行时 smoke check、模型大小和 SHA-256 校验通过。
- 子进程禁用固定 GGML 版本的不稳定 Metal residency-set 缓存，完成推理后正常退出，不得以 SIGABRT 将成功结果误标为失败。
- 约 617 秒真实音频成功转录，任务汇总为 `processed=1`、`failed=0`。

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
2. 用 `otool -L` 检查被自动安装的 `moss-transcribe`。
3. 停止隔离 Bifrost，清理本次临时 HOME、数据目录和模拟 release zip。

预期结果：

- 线上任务与源 WAV 哈希完全一致。
- 运行时只依赖 macOS 系统库/框架，不依赖 `@rpath/libggml*` 或构建目录动态库。
- 线上 9900 服务及 `day` 任务配置未被修改；测试临时目录可完整清理。

## 清理步骤

1. 停止且仅停止本测试启动的临时 Bifrost PID。
2. 删除本测试由 `mktemp` 创建的隔离目录和模拟 runtime zip；不得删除 `$ASR_TASK_DIR` 或 `$MOSS_AUDIO`。
3. 再次请求线上 9900 的任务详情，确认 `day` 任务仍存在且保持原配置。

## 执行记录

| 日期 | 用例 | 实际结果 |
| --- | --- | --- |
| 2026-07-18 | TC-MOSS-01 | PASS：API 创建、规范化、清空、超长拒绝、旧任务默认值及重启持久化全部通过；MOSS 有效并发为 1。内置浏览器真实验证选择 MOSS 后出现自动初始化文案与 prompt 输入，runtime/model 等不适用控件 disabled；保存 `Bifrost、NextOnCall 是专有名词。` 后重开 Edit 原文仍存在。 |
| 2026-07-18 | TC-MOSS-02 | PASS（修复后复测）：首次自动安装 2.3 MB 静态 runtime 和 648174592-byte Q5 模型；首次测试发现 GGML residency-set 退出断言并设置 `GGML_METAL_NO_RESIDENCY=1` 修复。617.210 秒真实音频复测为 `processed=1`、`failed=0`。 |
| 2026-07-18 | TC-MOSS-03 | PASS：timeline 119 segments、8 speakers（S01-S08）、最后语音终点 606010 ms、profile=`moss_joint_native`；metadata 仅含 `transcription_mode=moss_joint`、`transcription_prompt_configured=true`，日志与转录产物未出现 prompt 正文。 |
| 2026-07-18 | TC-MOSS-04 | PASS：静态 runtime 无 `@rpath/libggml*` 依赖；线上 `day` 任务未运行且原安装版本没有新字段；线上 `files.json` SHA-256 为 `713578de862a619d7676ac85bf423ad149eba6bbfd67848ec37f1760bcf2e289`，源 WAV SHA-256 为 `3d7ec54485b498833220520db9fab63218ecf20ffa2a7dddd69a253650770795`，前后完全一致。 |
