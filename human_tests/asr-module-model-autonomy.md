# ASR 模型自治与共享服务复用

## 功能模块说明

验证 ASR 三类消费入口的模型配置自治与共享资源边界：

- ASR Directory Tasks：每个任务独立保存 `model` / `language`，不继承 Speech Workbench 或 CLI 的模型选择。
- Speech Workbench：上传文件和实时麦克风共用 Workbench 自己的模型配置，可启动/停止共享 ASR Server，租约 owner 为 `speech_workbench`。
- CLI：`bifrost ai asr start`、`bifrost ai asr stream-file`、`bifrost ai voice listen` 通过显式 `--model` 参数选择模型，租约 owner 为 `cli`。
- Model Management：只负责共享模型资产状态、下载和初始化，不在 ASR 页面顶部混放 Start Service / Stop Service。
- 共享 ASR Server：同一 `host/home/model/port` 的托管 Qwen3-ASR runtime 可被 Workbench、Directory Task、offline-jobs 和 CLI 复用；owner 仍用于状态解释和请求归属，不能再因为 owner 不同阻止同模型复用。

## 前置条件

1. 在仓库根目录执行。
2. Rust、Node、pnpm、curl、python3 可用。
3. 启动 Bifrost 服务时必须使用临时 `BIFROST_DATA_DIR` 与 `--no-system-proxy`，不得使用 9900 端口。
4. 以下用例默认不下载模型；涉及真实下载/转写的扩展验证需用户主动确认模型资产已可用。

## 测试用例列表

### TC-ASR-AUTO-01：Model Management 查询不占用 Workbench/Task 租约

操作步骤：

1. 执行 `e2e-tests/tests/test_asr_model_autonomy.sh`。
2. 观察脚本中的 `model-management status is owner scoped` 步骤。
3. 检查响应 JSON。

预期结果：

- `/api/asr/status?model=Qwen3-ASR-0.6B&owner_module=model_management` 返回 `model=Qwen3-ASR-0.6B`。
- 响应包含 `owner_module=model_management`。
- 响应只展示共享资产/状态，不启动 ASR Server。

### TC-ASR-AUTO-02：ASR Directory Task 独立保存任务模型

操作步骤：

1. 执行 `e2e-tests/tests/test_asr_model_autonomy.sh`。
2. 观察脚本中的 `directory task keeps per-task model` 步骤。
3. 检查创建任务和任务详情响应。

预期结果：

- 创建任务时提交 `model=Qwen3-ASR-0.6B`、`language=english`。
- 任务详情仍返回 `model=Qwen3-ASR-0.6B`、`language=english`。
- Workbench 与 Model Management 默认模型不会覆盖任务模型。

### TC-ASR-AUTO-03：CLI 暴露显式模型参数并写入 CLI owner

操作步骤：

1. 执行 `e2e-tests/tests/test_asr_model_autonomy.sh`。
2. 观察脚本中的 `CLI help exposes explicit model options` 步骤。
3. 执行 `cargo test -p bifrost-cli status_reads_persisted_asr_service_state -- --nocapture`。

预期结果：

- `bifrost ai asr stream-file --help` 包含 `--model`。
- `bifrost ai voice listen --help` 包含 `--model`。
- CLI ASR 状态结构支持 `owner_module` / `owner_id`；旧 `service.json` 仍可反序列化。

### TC-ASR-AUTO-04：共享 ASR Server 同模型跨 owner 复用

操作步骤：

1. 执行 `e2e-tests/tests/test_asr_model_autonomy.sh`。
2. 观察脚本中的 `seed speech_workbench service and verify same-model runtime sharing` 步骤。
3. 执行 `cargo test -p bifrost-admin qwen3_service_runtime_is_shared_across_modules_for_same_model -- --nocapture`。

预期结果：

- seed 的 `speech_workbench` 服务可被 `directory_task` 同模型启动请求复用，返回 `ready=true` 和同一个 `server_url`。
- 单元测试确认同模型跨 owner 可复用；不同模型或不可复用资源才需要明确等待/拒绝。

### TC-ASR-AUTO-05：ASR 页面入口重组与主题兼容

操作步骤：

1. 执行 `pnpm --dir web run test:unit -- src/api/asr.test.ts`。
2. 打开 ASR 页面，确认页面顶部不是混合 Start Service / Install / Initialize 入口。
3. 在 Model Management 卡片中确认只显示模型资产初始化/状态。
4. 在 Speech Workbench 卡片中确认上传文件和麦克风共享同一个模型选择，并提供 Start Service / Stop Service。
5. 切换亮色和暗色主题，确认新增/修改区域无硬编码颜色导致的不可读问题。

预期结果：

- 单元测试确认 Workbench 与 Model Management 使用不同 `owner_module`。
- Model Management 不提供 Start Service / Stop Service。
- Workbench 提供模块内服务启动/停止，并说明 Directory Tasks 和 CLI 使用独立模型选择。
- 亮色和暗色主题下文本、边框、背景、按钮均可读。

## 清理步骤

- `test_asr_model_autonomy.sh` 自动清理临时 Bifrost 数据目录和音频目录。
- 如果手动启动过 Bifrost，执行 `kill <pid>` 并删除临时 `BIFROST_DATA_DIR`。
- 不删除 `~/.bifrost/asr` 共享模型资产目录。

## 执行记录

| 日期 | 用例 | 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-05-22 | TC-ASR-AUTO-01/02/03/04 | 已执行 `BIFROST_ASR_MODEL_AUTONOMY_E2E_PORT=18991 bash e2e-tests/tests/test_asr_model_autonomy.sh`，脚本构建当前 bifrost，启动临时 Bifrost `18991` 且使用 `--no-system-proxy`，覆盖 Model Management status、Directory Task 模型持久化、CLI `--model` help、seed busy `speech_workbench` 后 `directory_task` start 返回 409 | 通过 |
| 2026-05-22 | TC-ASR-AUTO-03 | 已执行 `cargo test -p bifrost-cli voice_ws_url_includes_runtime_chunk_options --lib` | 通过；CLI realtime voice URL 显式携带 `owner_module=cli`、`model`、`language` 和 chunk 参数 |
| 2026-05-22 | TC-ASR-AUTO-05 | 已执行 `npm --prefix web run test:unit -- src/api/asr.test.ts` | 通过；5 个前端 API 单测确认 Workbench/Model Management owner 隔离，实时语音 URL 携带 `owner_module=speech_workbench` 且跟随 Workbench 模型 |
| 2026-05-22 | TC-ASR-AUTO-03/04 | 已执行 `cargo test -p bifrost-admin qwen3_service_owner_isolation_blocks_other_modules -- --nocapture`、`cargo test -p bifrost-cli status_reads_persisted_asr_service_state -- --nocapture` | 通过 |
| 2026-05-28 | TC-ASR-AUTO-04 | CI `E2E Shell (aarch64-apple-darwin, shard 2/3)` 暴露旧断言仍要求不同 owner 返回 busy；已更新 `test_asr_model_autonomy.sh`，并执行 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost BIFROST_ASR_MODEL_AUTONOMY_E2E_PORT=18991 bash e2e-tests/tests/test_asr_model_autonomy.sh` | 通过；`speech_workbench` seed runtime 被 `directory_task` 同模型启动请求复用，返回同一 `server_url` |
