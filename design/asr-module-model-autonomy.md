# ASR 模块模型自治配置与共享服务租约

## 背景

ASR 能力当前同时服务三类入口：Directory Tasks（定时目录任务）、WebUI Speech Workbench（上传文件 + 实时麦克风 + 离线转写）以及 CLI（`bifrost ai asr` / `bifrost ai voice`）。三类入口的模型偏好、语言默认、使用频率与资源占用都不同：

- Directory Tasks 长期批量跑固定模型，模型必须随任务持久化，避免用户在 WebUI 切换模型后夜里跑的定时任务被误替换。
- WebUI Speech Workbench 是探索性使用，用户可能在两个模型之间反复切换，模型偏好应存在本地 storage。
- CLI 场景包括临时 stream-file、CI/自动化脚本调用，模型必须由命令行参数显式声明。
- 模型管理面板只负责下载、初始化、健康探测；不应该抢占 ASR Server 的运行租约。

同时 ASR Server 是本机唯一常驻共享资源：一个模型对应一个 `asr-server` 进程，加载后需要大量显存/统一内存。多入口同时启动会导致资源竞争、模型误替换、任务被误停用。V1 通过给每个入口分配"owner_module + owner_id"租约字段实现互斥，让不同模块请求同模型时复用同一服务，请求不同模型时按停止权限规则协调。

## 用户目标验证清单

### 必须实现

- Directory Tasks：每个任务独立保存 `model` 与 `language`，创建、编辑、运行时均使用任务自身配置，不再从 WebUI/全局 ASR 参数继承。
- WebUI Speech Workbench：上传文件与实时麦克风共享同一 workbench 模型配置；workbench 可启动/停止其租用的 ASR 服务，且不覆盖 Directory Task 或 CLI 的模型选择。
- CLI：命令行调用通过显式 `--model` / `--language` 参数声明调用模型；服务状态展示包含租约归属，避免误判为全局配置。
- 模型管理：ASR 页面顶部不再放置 Start Service / Install / Initialize 的混合入口；模型管理板块只负责模型资产下载、初始化和共享状态展示。
- 共享状态与互斥：模型资产状态跨模块共享；`asr-server` 通过租约归属互斥使用。不同模块请求不同模型时返回冲突提示，防止相互抢占。
- 实时麦克风 owner 活跃时，离线 owner（`offline_job`、`directory_task`、`scheduled_task`）自动让步或退避重试。
- 旧 `service.json` 只有 `managed_by` 字段的用户升级后仍能加载，owner 通过 `legacy_owner_module` 回填。

### 必须不破坏

- 现有 Directory Task 已保存的 `model` 与 `language` 字段兼容读取，不需要用户手动重建任务。
- 现有 Qwen3-ASR 单模型链路端到端能力：上传文件转写、实时麦克风、Directory Task 全流程仍工作。
- 现有 `bifrost ai asr start/status/stop/stream-file` 与 `bifrost ai voice listen` 输出格式对下游脚本兼容；新增字段追加，不改已存在字段语义。
- Rules/Values/Sync 主链路不受影响。

### 必须真实验证

- CLI 状态输出显式展示 owner_module / owner_id。
- WebUI Speech Workbench 切换模型不改动 Directory Task 已保存的 `model`。
- 模型管理板块启动模型只做健康探测，不占用其它 owner 的服务租约。
- 实时麦克风 owner 活跃时 Directory Task 让步。
- E2E `test_asr_model_autonomy.sh` 通过。
- human_tests 三模块自治、租约、CLI 参数场景通过。

## 产品语义

### owner_module 划分

| 模块 | owner_module | 模型配置来源 | 服务使用策略 |
| --- | --- | --- | --- |
| Directory Task | `directory_task` | `AsrDirectoryTask.model` / `language` | 任务运行时按任务 ID 租用服务；任务结束后释放自己启动的服务 |
| WebUI Speech Workbench | `speech_workbench` | `localStorage:bifrost.asr.workbench.connection.v2` | 上传文件使用 workbench 租约；实时麦克风共享同一 workbench 模型参数 |
| CLI | `bifrost_cli` | CLI 参数默认值或用户显式 `--model` | `stream-file` 临时租用服务；命令结束后释放自己启动的服务 |
| 模型管理 | `model_management` | 模型选择器，仅用于资产状态/初始化 | 不启动长期服务，不占用 ASR Server 租约 |
| 实时语音 | `realtime_voice` | 由 `crates/bifrost-admin/src/handlers/speech.rs` 注入 | 实时麦克风会话租约；其它离线模块在该 owner 活跃时让步 |
| 离线任务 | `offline_job` | 由 `offline_jobs.rs` 在派发时强制写入 | 离线转写任务的服务租约，与 directory_task 区分 |

### owner_id 语义

`owner_id` 是可选字符串，用于同一 owner_module 下细化归属：

- `directory_task` 下 owner_id 为 task_id。
- `realtime_voice` 下 owner_id 为 session_id。
- `speech_workbench` / `model_management` / `bifrost_cli` 通常不需要 owner_id。

owner_id 只用于诊断展示与停止权限判定，不影响服务复用（同模型跨 owner_id 仍复用）。

### 服务复用规则

`start_managed_service(target)` 的顺序：

1. 从内存 `MANAGED_SERVICE` 与持久化 `service.json` 读当前服务。
2. 若健康服务的 `model/home` 与请求一致，允许跨 owner 复用（`target_matches_request`）；`language` 是转写请求级参数，不触发服务重启。
3. 若健康服务的 `model/home` 与请求不同（`find_conflicting_healthy_service`），返回 `service_busy_response`，调用方按 `AsrServiceResponse { ready: false, managed: false, message, detail }` 向用户展示占用者；HTTP 层不下发专门 4xx。
4. 停止只允许同 owner (`same_service_owner`) 操作；其它模块不能停止不属于自己的实例。
5. Directory Task 和 CLI 只在自己启动服务时自动停止，避免误停 workbench 已启动的服务。

### 实时语音让步

`crates/bifrost-asr/src/resources.rs` + `profiles.rs::pause_on_realtime_voice` 定义让步策略：

- `realtime_voice` 活跃时，`should_yield_for_realtime()` 返回 true。
- `offline_job` / `directory_task` / `scheduled_task` 检测到让步信号后暂停当前批处理循环，等待 realtime 结束或超时。
- Realtime session 结束后离线 owner 恢复。

### 模型资产共享

模型文件目录统一位于 `~/.bifrost/asr`。状态 API 按请求模型返回：

- `installed`：模型文件与 runtime binary 是否存在。
- `ready`：是否存在匹配请求模型/租约的健康服务。
- `managed`：是否由当前 Bifrost 进程管理。
- `message` / `detail`：缺失、已安装未启动、被其它模块占用等用户可理解状态。

模型管理板块只调用状态与初始化接口，不调用 Start/Stop Service。

## 技术细节

### AsrTarget 与租约字段

```rust
pub struct AsrTarget {
    pub host: String,
    pub port: Option<u16>,
    pub language: Option<String>,
    pub model: String,
    pub home: PathBuf,
    pub owner_module: OwnerModule,
    pub owner_id: Option<String>,
}

pub enum OwnerModule {
    SpeechWorkbench,
    DirectoryTask,
    BifrostCli,
    ModelManagement,
    RealtimeVoice,
    OfflineJob,
}
```

`target_from_query` 支持 URL 参数 `owner_module` / `owner_id`；未提供时默认 `model_management`，保证状态/初始化请求不会意外占用其它模块租约。

### AsrServiceState

```rust
pub struct AsrServiceState {
    pub host: String,
    pub port: u16,
    pub model: String,
    pub language: Option<String>,
    pub home: PathBuf,
    pub pid: u32,
    pub managed_by: LegacyManagedBy,        // 兼容旧字段
    pub owner_module: OwnerModule,
    pub owner_id: Option<String>,
    pub started_at_ms: u64,
}
```

反序列化时若缺 `owner_module`，通过 `legacy_owner_module()` 从 `managed_by` 推断；缺 `owner_id` 保持 `None`。

### 前端 Storage Key

- Speech Workbench：`bifrost.asr.workbench.connection.v2`
- Model Management：`bifrost.asr.model-management.connection.v2`
- 旧 key `connection` / `connection.v2` / `connection.v3` / `workbench.connection.v1` / `model-management.connection.v1` 首次加载时通过 `clearLegacyAsrParams()` 清理。

### 实时麦克风参数派生

`loadVoiceRealtimeParams()` 从 workbench 配置派生 `model` / `language` / `host` / `chunkMs`，确保实时麦克风与上传文件绑定同一 workbench 配置。用户在 workbench 切换模型时实时麦克风下一次连接自动生效。

### Directory Task 表单

Directory Task 创建/编辑弹窗读写 `values.model` 与 `values.language`，不再从 workbench 或全局默认继承。任务保存后，Scheduler 触发 `run_directory_task()` 时按任务字段构造 target，owner_module 固定为 `directory_task`，owner_id 为 task_id。

### CLI

- `bifrost ai asr start/status/stop` 与 `stream-file` 的 target 中 owner_module 默认 `bifrost_cli`。
- `bifrost ai voice listen` 的 WebSocket URL 显式携带 `?owner_module=bifrost_cli`，便于用户判断是否与 WebUI 冲突。
- 状态输出新增 `owner_module` / `owner_id` 字段。

## CLI + Web + Admin API

### Admin API

```text
GET  /api/asr/status?model=Qwen3-ASR-0.6B&owner_module=model_management
GET  /api/asr/status?model=Qwen3-ASR-0.6B&owner_module=speech_workbench&owner_id=session-xxx
POST /api/asr/service/start
POST /api/asr/service/stop
POST /api/asr/init-stream
POST /api/asr/transcribe-stream
GET  /api/asr/transcribe-ws
```

- `owner_module` / `owner_id` 通过 query 或 body 传入，未提供默认 `model_management`。
- Start/Stop 服务时校验租约。
- Stop 不属于自己的服务返回 `409 not_owner`。

### Web

- ASR 页面顺序调整为：Model Management → Directory Tasks → Speech Workbench；Model Management 不放 Start/Stop 按钮。
- Speech Workbench 独立 Start/Stop Service 控件，owner_module=speech_workbench。
- Directory Task 表单独立 model/language 字段。

### CLI

- `bifrost ai asr status` 输出新增 `owner_module` / `owner_id`。
- `bifrost ai asr stop --force` 才允许跨 owner 停止（可选，本 V1 建议不引入 force，保持严格隔离）。
- CLI help 展示 `--model` / `--language` 说明。

## Sync 边界

- ASR 服务状态、模型资产、workbench storage、Directory Task model 均为本机运行时/用户偏好，不参与 Rules/Values sync。
- Directory Task 若未来支持跨设备同步，需要单独设计"任务同步"机制，不能复用现有 Rule sync；本方案不承诺 Directory Task sync。

## 实现切分

### Phase 1：租约字段与兼容

- 新增 `OwnerModule` 枚举与 `owner_module` / `owner_id` 字段。
- `service.json` schema 兼容旧格式；`legacy_owner_module()` 回填。
- 单元测试覆盖旧 state 反序列化。

### Phase 2：服务复用与停止规则

- `start_managed_service()` 检查 owner_module / model / home。
- `same_service_owner()` 只允许同 owner 停止。
- `should_yield_for_realtime()` 实现让步。
- 单元测试覆盖复用、冲突、让步。

### Phase 3：前端 storage 分离

- Speech Workbench / Model Management 分离 storage key。
- `clearLegacyAsrParams()` 清理旧 key。
- Directory Task 表单读写自身字段。
- 实时麦克风从 workbench 派生参数。

### Phase 4：CLI + 文档 + 测试

- CLI 显式携带 owner_module；状态输出新增字段。
- 新增 E2E `test_asr_model_autonomy.sh`。
- human_tests 三模块自治用例。
- 更新本设计文档与 `human_tests/readme.md`。

## 测试方案

### 单元测试

- `qwen3_default_owner_is_model_management`：未携带 owner 参数时默认归属 `model_management`。
- `qwen3_service_runtime_is_shared_across_modules_for_same_model`：同模型可被不同 owner 复用。
- `qwen3_service_runtime_is_shared_across_owner_ids_for_same_model`：同模型跨 owner_id 复用。
- `qwen3_resolved_state_preserves_owner_and_port`：`with_state` 后 owner、port 与持久化状态保持一致。
- `qwen3_owner_agnostic_status_reuses_matching_port`：状态查询按模型匹配。
- `lease_owner_module_prefers_owner_module_and_maps_webui`：旧版 `service.json`（仅 `managed_by`）反序列化时 owner 映射兼容。
- `same_service_owner_rejects_cross_owner_stop`。
- `should_yield_for_realtime_pauses_offline_jobs`。
- CLI 状态测试验证输出包含 `owner_module` / `owner_id`。

### E2E 测试

`e2e-tests/tests/test_asr_model_autonomy.sh`：

- 独立 `BIFROST_DATA_DIR`，非 9900 端口，`--no-system-proxy`。
- API `/api/asr/status?model=Qwen3-ASR-0.6B&owner_module=model_management` 返回模型/状态字段。
- 创建 Directory Task 传入 `model=Qwen3-ASR-0.6B`；查任务详情断言模型未被 workbench 默认模型覆盖。
- CLI help `ai asr stream-file --help` 与 `ai voice listen --help` 暴露 `--model`。
- 模拟两个 owner 请求同模型，服务复用，不重启。
- 模拟两个 owner 请求不同模型，返回 `service_busy_response`。

### 真实场景测试 human_tests

`human_tests/asr-module-model-autonomy.md` 覆盖：

- TC-AMA-01：三模块租约独立，切换 workbench 模型不影响 Directory Task 已保存模型。
- TC-AMA-02：Model Management 面板不启动 asr-server，只做健康探测。
- TC-AMA-03：Realtime 麦克风活跃时 Directory Task 让步。
- TC-AMA-04：CLI `bifrost ai asr status` 输出 owner_module / owner_id。
- TC-AMA-05：跨 owner stop 被拒绝，错误提示明确。
- TC-AMA-06：旧 `service.json` 无 owner_module 字段升级后仍能加载。

`human_tests/readme.md` 同步索引。

启动 Bifrost 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin asr_target same_owner`
- `cargo test -p bifrost-asr runtime lease`
- `cargo test -p bifrost-cli status_reads`
- `bash e2e-tests/tests/test_asr_model_autonomy.sh`
- `pnpm --dir web exec tsc -b --pretty false`
- `rust-project-validate`
- 本机若沿用 no-local-coverage 约定，则不跑 `make coverage`；交付时说明依赖 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：Directory Tasks、workbench、CLI、模型管理、租约互斥、realtime 让步。
- 复核 diff：`AsrTarget` 字段、`AsrServiceState` 序列化兼容、Directory Task 表单、workbench storage 迁移、CLI 输出。
- 重点 review：模型管理面板是否真的不启动服务；跨 owner 停止是否被拒；旧 state 反序列化路径。
- 复测：单元测试 + `test_asr_model_autonomy.sh` + human_tests 关键条目。

### 第 2 轮

- 基于第 1 轮修复后的 diff 再次复核所有目标。
- 再次执行 `git status --short`、`git diff`。
- 检查设计文档、E2E、human_tests、WebUI 文案一致。
- 复跑受影响测试与 `rust-project-validate`。

## 风险与决策点

- owner_module 与 owner_id 兼容旧数据：旧 `service.json` 无 owner_module，需要通过 `legacy_owner_module` 回填；一旦未来删除 `managed_by`，需要显式迁移脚本。
- 让步策略与用户预期：Realtime 让步会打断离线任务；如果用户在录音期间以为定时任务失败，需要在 Directory Task 详情页展示 "paused by realtime voice" 状态。
- workbench storage 迁移：清理旧 key 时若用户跨版本回退可能丢配置；建议保留 30 天兼容读取或提示导出配置。
- 模型管理面板启动服务的历史遗留：老版本 UI 曾在模型管理面板暴露 Start Service，本版本移除后旧用户可能找不到入口；需要在 workbench 卡片顶部提示"Start Service 已迁移到 Speech Workbench"。
- CLI `--force stop` 是否引入：本 V1 不引入，避免破坏隔离；如果运维场景确实需要跨 owner 强停，作为独立后续能力。
- Directory Task sync：本方案不承诺 sync；未来若上线任务云同步，需要考虑 owner_id 冲突与本机租约状态如何合并。
