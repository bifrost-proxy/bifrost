# ASR 模型自治配置与共享服务租约

## 背景与目标

ASR 能力现在同时服务于 Directory Tasks、WebUI 语音工作台（实时麦克风与上传文件）和 CLI。三类入口的使用频率、模型体积与资源占用不同，因此模型配置必须自治；同时模型文件、初始化状态和 `asr-server` 进程仍是本机共享资源，不能让一个模块的全局配置或长时服务隐式影响其它模块。

本方案覆盖以下目标：

- Directory Tasks：每个任务独立保存 `model` 与 `language`，创建、编辑、运行时均使用任务自身配置，不再从 WebUI/全局 ASR 参数继承。
- WebUI 语音工作台：上传文件和实时麦克风共享同一个工作台模型配置；工作台可启动/停止其租用的 ASR 服务，且不覆盖 Directory Task 或 CLI 的模型选择。
- CLI：命令行调用通过显式 `--model`/`--language` 参数声明调用模型；服务状态展示包含租约归属，避免误判为全局配置。
- 模型管理：ASR 页面顶部不再放置 Start Service / Install / Initialize 的混合入口；模型管理板块只负责模型资产下载、初始化和共享状态展示。
- 共享状态与互斥：模型资产状态共享；ASR Server 是单实例共享资源，通过租约归属互斥使用。不同模块请求不同租约或不同模型时返回冲突提示，防止相互抢占。

## 模块划分

| 模块 | owner_module | 模型配置来源 | 服务使用策略 |
| --- | --- | --- | --- |
| Directory Task | `directory_task` | `AsrDirectoryTask.model` / `language` | 任务运行时按任务 ID 租用服务；任务结束后释放自己启动的服务 |
| WebUI 语音工作台 | `speech_workbench` | `localStorage:bifrost.asr.workbench.connection.v1` | 上传文件使用工作台租约；实时麦克风共享同一工作台模型参数 |
| CLI | `cli` | CLI 参数默认值或用户显式 `--model` | `stream-file` 临时租用服务；命令结束后释放自己启动的服务 |
| 模型管理 | `model_management` | 模型选择器，仅用于资产状态/初始化 | 不启动长期服务，不占用 ASR Server 租约 |

## 后端实现逻辑

### ASR Target 与租约字段

`AsrTarget` 与 `AsrServiceState` 包含租约归属：

- `owner_module`: `speech_workbench` / `directory_task` / `cli` / `model_management`。
- `owner_id`: 可选实例标识，例如 Directory Task ID。
- `model` 与 `language`: 当前服务进程真实加载的模型与语言。

`target_from_query` 支持 `owner_module` 与 `owner_id`，未提供时默认 `model_management`。这使状态/初始化请求不会意外占用工作台或任务的租约。

### 互斥规则

1. `start_managed_service(target)` 先检查内存中的 `MANAGED_SERVICE` 和持久化 `service.json`。
2. 如果已有健康服务的 `model/home/owner_module/owner_id` 与请求匹配，允许复用；`language` 是转写请求级参数，不触发服务重启，避免仅切换语言就误杀或重建同一模型服务。
3. 如果已有健康服务归属其它模块或其它模型，返回冲突信息；HTTP 接口返回 `409 Conflict`，调用者展示当前租用方与模型。
4. 停止服务时，只有相同目标与相同租约归属可以停止；其它模块不能停止不属于自己的 ASR Server。
5. Directory Task 和 CLI 只在自己启动了服务时自动停止，避免误停用户已显式启动的工作台服务。

### 共享模型状态

模型文件目录仍统一位于 `~/.bifrost/asr`。状态 API 根据请求模型返回：

- `installed`: 模型文件与运行时是否存在。
- `ready`: 是否存在匹配请求模型/租约的健康服务。
- `managed`: 是否由当前 Bifrost 进程管理。
- `message/detail`: 缺失、已安装未启动、被其它模块占用等用户可理解状态。

模型管理板块只调用状态和初始化接口，不调用 Start/Stop Service。

## 前端实现逻辑

### ASR 页面布局

ASR 页面顺序调整为：

1. Model Management：模型资产初始化、下载进度、共享状态展示。该板块不展示 Start Service / Stop Service。
2. Directory Tasks：创建/编辑弹窗提供独立 Model 和 Language 字段；任务列表展示每个任务的模型，验证任务配置不会被工作台设置覆盖。
3. Speech Workbench：上传文件和实时麦克风共享工作台模型配置；提供 Start Service / Stop Service 控件，使用 `owner_module=speech_workbench`。

### 配置隔离

- `loadAsrParams` / `saveAsrParams` 迁移为工作台专用配置，存储在 `bifrost.asr.workbench.connection.v1`。
- `loadModelManagementParams` / `saveModelManagementParams` 使用独立 storage key，避免初始化模型选择影响工作台。
- `loadVoiceRealtimeParams` 从工作台配置派生模型、语言、host、chunkMs，保证实时麦克风与上传文件绑定同一工作台配置。
- Directory Task 表单保存 `values.model` 和 `values.language`，不再读取工作台当前模型。

## CLI 实现逻辑

CLI 的 `ai asr start`、`ai asr stream-file`、`ai voice listen` 保留并强调 `--model` 参数。离线 ASR 服务状态写入 `owner_module=cli`，状态输出显示 `owner_module` 和 `owner_id`；实时 voice listen 的 WebSocket URL 也显式携带 `owner_module=cli`，便于用户判断是否与 WebUI 或 Directory Task 冲突。

## 测试方案

### 单元测试

- `target_from_query_parses_owner_for_workbench`：验证 owner 参数被正确解析。
- `same_owner_requires_matching_owner_id`：验证同模型不同模块/任务不会互相复用租约。
- `status_reads_legacy_service_state_without_owner`：验证旧版 `service.json` 反序列化时 owner 默认兼容。
- CLI 状态测试验证输出包含 `owner_module`。

### E2E 测试

新增 `e2e-tests/tests/test_asr_model_autonomy.sh`：

- 使用独立 `BIFROST_DATA_DIR` 启动最新 bifrost，必须带 `--no-system-proxy`。
- API 验证 `/api/asr/status?model=Qwen3-ASR-0.6B&owner_module=model_management` 返回模型字段与状态字段。
- 创建 Directory Task，传入 `model=Qwen3-ASR-0.6B`；再查询任务详情，断言任务模型未被工作台默认模型覆盖。
- CLI help 验证 `ai asr stream-file --help` 和 `ai voice listen --help` 暴露 `--model` 参数。

### human_tests

更新以下人工场景文档并立即按文档执行：

- `human_tests/asr-module-model-autonomy.md`：新增三模块自治、模型管理、互斥提示、CLI 参数场景。
- `human_tests/readme.md`：同步索引。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：Directory Tasks、WebUI 工作台、CLI、模型管理、共享状态与互斥。
- 执行 `git status --short`、`git diff`，检查是否误改前序 ASR fallback 文件。
- Review 后端租约兼容性、前端 storage key 隔离、任务表单模型来源、CLI 输出。
- 运行最小测试：`cargo test -p bifrost-admin asr_target same_owner`、`cargo test -p bifrost-cli status_reads`、E2E 脚本与 human_tests。

### 第 2 轮

- 基于第 1 轮修复后的最新 diff 再次复核所有目标。
- 再次执行 `git status --short`、`git diff`。
- 检查设计文档、E2E、human_tests、WebUI 文案是否一致。
- 复跑受影响测试和最终项目校验。

## 校验要求

按顺序执行：

1. 相关单元测试。
2. ASR 自治 E2E。
3. human_tests 文档中的真实场景测试。
4. `rust-project-validate`：`cargo fmt --all -- --check`、desktop fmt、clippy、`cargo test --workspace --all-features`、必要构建。
5. 如执行 `scripts/ci/local-ci.sh` 成本过高或当前任务范围已被上述检查覆盖，最终交付说明未执行原因和风险。

## 文档更新要求

- 更新本设计文档保持与实现一致。
- 更新 `human_tests/` 场景和索引。
- 如果后续引入新的模型注册 API 或配置文件，需要补充 README 中 ASR 相关说明。
