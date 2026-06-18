# ASR Daily Agent Runner 技术规格

## 1. 概述

ASR 定时任务完成音频转写后，自动触发 Daily Agent Runner 队列对每日汇总做二次整理。一个每日转录文件可以被多个 Agent 按顺序处理，每个 Agent 有独立指令 Markdown、独立输出目录和独立 processed state；默认包含 `daily_report` 与 `tomorrow_todo` 两个 Agent。

**定位**：ASR 定时任务的后处理阶段，不是独立的定时任务。

**作用域**：本规格只涉及 ASR 每日转写汇总后的 Agent Runner。不改变 ASR 音频转写主链路（转写成功率、chunk retry、daily markdown 生成均由现有流程负责）。

### 目录结构

```text
<BIFROST_DATA_DIR>/asr/data/text/<task_id>/.daily/
├── .gitignore                   # 排除 .DS_Store
├── .git/                        # best-effort 版本追踪
├── YYYY-MM-DD.md                # ASR 生成的每日转写（只读）
└── agents/
    └── <agent_id>/
        ├── AGENTS.md            # 当前 Agent 的运行规范；Runner cwd 指向该目录
        ├── input/
        │   └── YYYY-MM-DD.md    # 每日转写源文件副本
        └── output/
            └── <output_dir>/
                └── YYYY-MM-DD-report.md
```

---

## 2. 设计决策

以下问题已确定，实现时直接执行：

| # | 决策 | 结论 |
|---|------|------|
| 1 | Phase 1 是否支持 `bifrost_agent` | 是。`runner=bifrost_agent` 走内置 Bifrost Agent，其它值按 AI -> Agent -> Runners 中的 runner id 执行 |
| 2 | 默认 Runner 选择 | WebUI 和 API 都只使用一个 `runner` 字段；不再暴露或持久化 `runner_type` / `runner_id` |
| 3 | Partial success 时是否触发 | 是。只要 daily markdown 有新增/更新就触发，prompt/report 中保留失败 chunk 证据 |
| 4 | IM target 选择器 | WebUI 和 API 都只使用一个 `im_delivery.channel` 字段；值为 `owner:<provider_id>` 或 `target:<target_id>` |
| 5 | IM 发送内容 | 默认 summary；`full_report` 发送原始报告内容；超长时按固定大小拆成多条 IM，不降级为 summary |
| 6 | AGENTS.md 存储 | 每个 Agent 独立写入 `.daily/agents/<agent_id>/AGENTS.md`，并同步 task config 的 `instructions` 副本（备份用途） |
| 7 | Git commit | Phase 1 只做 `git init`；Runner 运行后 best-effort `git add + commit` |
| 8 | Report 覆盖策略 | 默认不覆盖，`force=true` 时才允许覆盖 |
| 9 | Report 同步目录 | 可选 `report_sync_dir`；Runner 生成 report 后自动复制本轮 report，用户也可在 Daily Agent 配置页手动同步全部现有 report，便于 iCloud 等外部目录同步 |
| 10 | Daily Agent 多实例 | `daily_agent.agents[]` 是新的真实配置；旧单 Agent 字段保留为兼容镜像 |
| 11 | 默认 Agent | 默认启用两个 Agent：`daily_report` 输出到 `report/`；`tomorrow_todo` 输出到 `tomorrow_todo/`，并默认绑定 `owner:feishu-main` 发送完整报告 |
| 12 | Agent 标识约束 | `id`、`name`、`output_dir` 必须只包含英文字符、数字、`_`、`-`，避免跨平台路径与 URL 参数歧义 |
| 13 | Agent 工作目录 | Runner cwd 必须是 `.daily/agents/<agent_id>`；源文件消费路径固定为 `input/YYYY-MM-DD.md`，报告输出路径固定为 `output/<output_dir>/YYYY-MM-DD-report.md` |

---

## 3. 数据模型

### 3.1 AsrDirectoryTask 扩展

在 `AsrDirectoryTask`（`crates/bifrost-admin/src/handlers/asr_jobs/state.rs`）增加：

```rust
#[serde(default)]
pub daily_agent: AsrDailyAgentConfig,
```

`AsrDailyAgentConfig` 及相关数据模型实际定义在 `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_config.rs`，运行逻辑分散在 `daily_agent.rs`、`daily_agent_workspace.rs`、`daily_agent_prompt.rs`、`daily_agent_api.rs`、`daily_agent_records.rs` 等模块。

### 3.2 AsrDailyAgentConfig

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDailyAgentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_daily_agent_id")]
    pub agent_id: String,
    #[serde(default = "default_daily_agent_name")]
    pub name: String,
    #[serde(default = "default_daily_agent_runner")]
    pub runner: String,
    #[serde(default = "default_daily_agent_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub trigger_policy: AsrDailyAgentTriggerPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default)]
    pub instructions_source: AsrDailyAgentInstructionsSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default)]
    pub im_delivery: AsrDailyAgentImDeliveryConfig,
    #[serde(default = "default_daily_agent_output_dir")]
    pub output_dir: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AsrDailyAgentItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminology: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_sync_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_report_sync: Option<AsrDailyAgentReportSyncResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDailyAgentItem {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub runner: String,
    pub timeout_ms: u64,
    pub trigger_policy: AsrDailyAgentTriggerPolicy,
    pub session_key: Option<String>,
    pub instructions_source: AsrDailyAgentInstructionsSource,
    pub instructions: Option<String>,
    pub im_delivery: AsrDailyAgentImDeliveryConfig,
    pub output_dir: String,
    pub report_sync_dir: Option<String>,
    pub last_report_sync: Option<AsrDailyAgentReportSyncResult>,
    pub last_run_at_ms: Option<u64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_run_id: Option<String>,
}
```

**默认值**：

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `enabled` | `true` | ASR 任务默认具备后处理能力，可在任务配置中关闭 |
| `runner` | `bifrost_agent` | 内置 Agent；其它值是 Runners 中的 runner id |
| `timeout_ms` | `7_200_000` (2h) | — |
| `trigger_policy` | `after_asr_run` | — |
| `session_key` | `None` | 运行时默认 `asr-daily:<task_id>` |
| `instructions_source` | `default` | — |
| `instructions` | `None` | — |
| `report_sync_dir` | `None` | 可选外部同步目录；空字符串清除配置 |
| `im_delivery.enabled` | `false` | 需用户绑定 channel |
| `im_delivery.mode` | `summary` | — |
| `im_delivery.send_policy` | `on_success_with_report` | — |
| `agents` | `daily_report` + `tomorrow_todo` | 旧任务加载时会自动从 legacy 字段升级，并补齐 `tomorrow_todo` |

默认 `tomorrow_todo`：

- `id/name/output_dir=tomorrow_todo`。
- 指令模板要求从每日转录中提取明天 To Do List。
- `im_delivery.enabled=true`，`channel=owner:feishu-main`，`mode=full_report`，`send_policy=on_success_with_report`。

### 3.3 枚举类型

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentTriggerPolicy {
    #[default]
    AfterAsrRun,
    ManualOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentInstructionsSource {
    #[default]
    Default,
    Custom,
}
```

### 3.4 IM Delivery 配置

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AsrDailyAgentImDeliveryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default)]
    pub mode: AsrDailyAgentImDeliveryMode,
    #[serde(default)]
    pub send_policy: AsrDailyAgentImSendPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sent_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_send_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentImDeliveryMode {
    #[default]
    Summary,
    FullReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentImSendPolicy {
    #[default]
    OnSuccessWithReport,
    OnSuccess,
    Always,
}
```

### 3.5 已处理文档状态

存储路径：`<BIFROST_DATA_DIR>/asr/tasks/<task_id>/daily_agent_processed.json`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AsrDailyAgentProcessedState {
    pub version: u32,
    pub documents: BTreeMap<String, AsrDailyAgentProcessedDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrDailyAgentProcessedDocument {
    #[serde(default = "default_daily_agent_id")]
    pub agent_id: String,
    #[serde(default = "default_daily_agent_name")]
    pub agent_name: String,
    #[serde(default = "default_daily_agent_output_dir")]
    pub output_dir: String,
    pub date: String,
    pub source_path: String,
    pub source_sha256: String,
    pub source_len_bytes: u64,
    pub processed_at_ms: u64,
    pub runner: String,
    pub report_path: Option<String>,
    pub last_run_id: String,
}
```

当前实现未保留 `source_mtime_ms`、`report_sha256` 或 `last_delivery_mode` 字段，也未引入 `AsrDailyAgentDeliveryMode` 枚举；投递模式信息直接来自 `AsrDailyAgentImDeliveryConfig.mode`，processed state 只记录足够 dedup 的事实。

### 3.6 Conversation State（ChatGPT Web 专用）

存储路径：`<BIFROST_DATA_DIR>/asr/tasks/<task_id>/daily_agent_conversation_<agent_id>.json`；legacy `daily_agent_conversation.json` 仅作为 `daily_report` 兼容读取。

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrDailyAgentConversationState {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    adapter: Option<String>,
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    conversation_id: Option<String>,
    // 其余字段（thread_id、initialized_at_ms、last_message_at_ms 等）以 `#[serde(default)]` 兼容历史 JSON
}
```

### 3.7 Report 索引可见性

Daily Agent 同时维护三类事实：

- `daily_agent_processed.json`：系统内 Runner 成功处理后的增量索引；新 key 使用 `<agent_id>:<YYYY-MM-DD>`，legacy `YYYY-MM-DD` key 只作为 `daily_report` 兼容读取。
- `.daily/agents/<agent_id>/input/YYYY-MM-DD.md`：分发给该 Agent 的每日转写源文件副本。新增 Agent、保存配置、GET 初始化 workspace、ASR 转录完成触发 Daily Agent 前、手动运行前都必须同步，确保 Agent 只靠自己的工作目录也能运行。同步必须做差异判断：目标缺失、文件长度不同或内容不同才复制；内容一致时跳过，避免数千个 Daily Docs 时每次全量写盘。
- `.daily/agents/<agent_id>/output/<output_dir>/*-report.md`：磁盘上已经存在的报告文件，可能由历史版本、外部 Agent 或人工流程生成；旧版 `daily/<output_dir>/`、`.daily/agents/<agent_id>/<output_dir>/` 和历史 `Report/` 继续兼容扫描。

`GET /daily-agent` 返回 `report_index_status`，用于 UI 展示 report 文件与 processed state 的对齐状态：

```json
{
  "report_files": 6,
  "processed_documents": 5,
  "indexed_reports": 5,
  "unindexed_reports": 1,
  "processed_missing_report": 0,
  "unindexed_dates": ["2026-05-19"]
}
```

约束：

1. `report_index_status` 只做可见性提示，不自动写入 `daily_agent_processed.json`。
2. 普通 `Run Now` 仍以 `daily_agent_processed.json` 中记录的 source hash/size 作为唯一增量判断依据。
3. `Force Run` 仍显式绕过增量判断，刷新匹配日期。
4. Records 页和配置页复用同一套 report 目录扫描逻辑，避免 `report/` 与历史 `Report/` 兼容行为漂移。
5. `/daily-agent/runs` 返回的 Records 列表必须按 `date` 倒序排列；同一日期存在多条候选记录时按 `processed_at_ms` 倒序，确保 Run Results tab 首屏优先展示最新数据。

### 3.8 Report 同步状态

`report_sync_dir` 只表示额外副本目录，不改变 `.daily/agents/<agent_id>/output/<output_dir>/` 作为系统事实源。同步时按 report 文件名复制到目标目录；为避免 iCloud、CloudDocs、网络盘等目标目录中的占位文件在读取 hash 时触发云端下载或长时间阻塞，目标文件存在时不读取目标内容做 SHA256 比对，而是通过同目录临时文件 + rename 覆盖目标。只有源路径与目标路径完全相同时计入 skipped。路径支持 `~/...` 展开，目标目录不存在时创建；目标存在但不是目录时返回错误。

同步文件系统调用必须通过 blocking worker 隔离，并设置超时和全局并发门禁。手动 `/daily-agent/sync` 超时或已有同步进行中时返回结构化失败结果并更新 `last_report_sync`；阻塞的文件系统调用即使被 iCloud 拖住，也不得占住代理 admin/proxy runtime。自动同步只记录 `last_report_sync` 和 warning，不得让成功生成 report 的 Daily Agent run 因外部同步目录失败而变成失败。

最近一次同步结果保存在 `last_report_sync`，用于 `Last Run Status` 展示：

```json
{
  "target_dir": "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/ASR Reports",
  "total_files": 12,
  "copied_files": 2,
  "skipped_files": 10,
  "failed_files": 0,
  "synced_at_ms": 1779212999000,
  "errors": []
}
```

自动同步只处理本轮 Runner 生成或更新的 report；手动同步通过 WebUI 按钮调用 `/daily-agent/sync`，扫描并同步全部现有 `.daily/agents/<agent_id>/output/<output_dir>/` 报告，同时兼容历史 `daily/report/`、`daily/Report/` 和 `.daily/agents/<agent_id>/<output_dir>/` 报告。

---

## 4. 并发控制

| 锁 | 类型 | 用途 |
|----|------|------|
| `DAILY_AGENT_TASK_LOCKS` | `StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>` | 每个 task 一把独立锁；不同 task 之间允许并发 |
| `DAILY_AGENT_RUNNING_TASKS` | `StdMutex<HashSet<String>>` | 防止同一 task 重复触发 Daily Agent |
| `DAILY_AGENT_TASK_CONFIG_LOCK` | `StdMutex<()>` | Daily Agent 写回 task 配置时序列化 read-modify-write |

**关键约束**：
- `DAILY_AGENT_RUNNING_TASKS` 与现有 `RUNNING_TASKS`（ASR run 用）完全独立，互不影响。
- ASR run 正在运行时仍可排队 Daily Agent（等 ASR 完成后才执行）。
- 没有全局 `DAILY_AGENT_RUN_LOCK`：跨任务 Agent 可并发，单任务通过 per-task `DAILY_AGENT_TASK_LOCKS` 串行。
- ASR 链路上的 `ASR_JOB_RUN_LOCK` 不复用到 Daily Agent；Daily Agent 通过 `RUNNING_TASKS` 检测来等待 ASR run 结束后再启动。

---

## 5. 核心实现

### 5.1 Workspace 初始化

```rust
fn ensure_asr_daily_workspace(task: &AsrDirectoryTask) -> Result<AsrDailyWorkspaceStatus, String>
```

**执行步骤**：

1. 计算 `daily_dir = text_output_dir(data_dir) / task.id / ".daily"`
2. `mkdir -p .daily/agents/<agent_id>/input/` 和 `mkdir -p .daily/agents/<agent_id>/output/<output_dir>/`
3. 如果 `.daily/agents/<agent_id>/AGENTS.md` 不存在：
   - `instructions_source == Custom && instructions.is_some()` → 写入 custom
   - 否则 → 写入内置默认模板（替换 `{{task_name}}`/`{{daily_dir}}`/`{{report_dir}}`）
4. 将 `daily/YYYY-MM-DD.md` 分发到所有 Agent 的 `input/YYYY-MM-DD.md`，新增 Agent 时也要补齐既有 Daily Docs。分发使用差异复制，内容一致的文件不刷新 mtime、不重复写盘。
5. 历史任务中已存在的 `agents/<agent_id>/AGENTS.md` 需要做兼容迁移：只替换旧模板中“源文件在当前目录根部 / report 或 tomorrow_todo 输出目录”的路径说明，改为 `input/YYYY-MM-DD.md` 与 `output/<output_dir>/`，保留用户其它自定义指令。
5. 写入 `.gitignore`（内容：`.DS_Store`）
6. 尝试 `git init`（失败只 warn，不阻塞）

**调用时机**：
- 创建 ASR task 成功后
- 任务详情 GET 时（轻量 ensure）
- `generate_daily_summaries()` 写 daily markdown 前
- 用户保存 Daily Agent 配置或 AGENTS.md 时

### 5.2 Change Planner

```rust
fn build_daily_agent_change_plan(
    task: &AsrDirectoryTask,
    trigger_source: DailyAgentTriggerSource,
    requested_date: Option<NaiveDate>,
    force: bool,
) -> Result<AsrDailyAgentChangePlan, String>
```

**变更类型判定**：

| 条件 | change_kind | 投递行为 |
|------|-------------|----------|
| `force == true` | `force` | 投递完整文件/文件清单 |
| 无历史记录 | `new_file` | 投递完整内容 |
| sha256 相同 | `unchanged` | 不投递 |
| 当前内容以前次内容为前缀 | `appended` | 只投递新增 tail |
| hash 变化但非 append | `rewritten` | 投递 diff/摘要 |

**短路规则**：所有日期都是 `unchanged` 且 `!force` → 返回 `skipped_no_daily_changes`，不启动 Runner。

**状态更新**：
- Runner 成功 → 原子更新 `daily_agent_processed.json`
- Runner 失败 → 不更新（保留下次重试机会）

### 5.3 Runner 触发

```rust
async fn maybe_enqueue_daily_agent_after_asr_run(task: &AsrDirectoryTask)
```

实际签名只接受 `task`，触发判定使用 `task.daily_agent` 与 `daily_agent_has_changed_daily_markdown(...)`；ASR run outcome 由调用点（`runner.rs` 中 `update_task_after_run` 之后）决定是否进入 hook。

**插入位置**（`runner.rs` 中 `run_directory_task()`）：

```text
process_pending_files()
  → stop_any_managed_service()
  → refresh_task_daily_summaries()        ← daily markdown 写完
  → update_task_after_run()               ← ASR 状态持久化
  → maybe_enqueue_daily_agent_after_asr_run()  ← 新增
```

**自动触发条件**：

```rust
task.daily_agent.enabled
    && task.daily_agent.trigger_policy == AfterAsrRun
    && !DAILY_AGENT_RUNNING_TASKS.contains(&task.id)
```

### 5.4 Runner 执行

**执行步骤**：

1. 获取 per-task 锁 `DAILY_AGENT_TASK_LOCKS[<task_id>]`（`tokio::sync::Mutex`）并把 `task_id` 加入 `DAILY_AGENT_RUNNING_TASKS`
2. `ensure_asr_daily_workspace(task)`
3. `build_daily_agent_change_plan(task, trigger_source, date, force)`
4. 如果 `skipped_no_daily_changes` → 记录 skipped，退出
5. 根据 `runner` 分发：

**Bifrost Agent 路径**：
```rust
bifrost_agent::session::run_turn(/* ... */)
```
- `work_dir = daily_dir`
- tool registry 限制文件读写在 `daily_dir`
- prompt 只发 change plan 文件清单

**External CLI 路径**：
```rust
ExternalCliRuntime::run(ExternalCliRunRequest {
    message: prompt,
    runner_id: Some(task.daily_agent.runner.clone()),
    session_key: Some(format!("asr-daily:{}", task.id)),
    work_dir: Some(daily_dir),
    allow_work_dirs: vec![daily_dir.to_string_lossy().to_string()],
    // ...
})
```

6. Post-run actions：
   - 更新 `last_run_at_ms` / `last_status` / `last_error` / `last_run_id`
   - 成功时更新 `daily_agent_processed.json`
   - 如果 `im_delivery.enabled && send_policy` 匹配 → 发送 IM
   - best-effort `git add report/ && git commit`

### 5.5 Runner 消息组织

#### Bifrost Agent / Codex（可读本地文件）

```text
请根据当前目录 AGENTS.md，检查并处理以下变更文件：

- source=input/2026-05-15.md: change_kind=appended, source_sha256=..., report=output/report/2026-05-15-report.md
- source=input/2026-05-16.md: change_kind=new_file, source_sha256=..., report=output/report/2026-05-16-report.md

只刷新这些日期对应的 report。不要修改原始 YYYY-MM-DD.md。
```

#### ChatGPT Web（不可读本地文件）

**每次日期运行**：
- Daily Agent 会清理该 Agent session 关联的历史 Web conversation，并以新 ChatGPT 对话发送完整 `AGENTS.md`、术语和当日 Markdown 内容。
- `session_key` 不传给 ChatGPT Web adapter，避免不同日期或不同 Agent 复用长对话状态。
- 如果首次响应未通过日报标题、日期、关键章节和长度门禁，且 adapter 返回了新的 `conversationId`，后端会在同一新对话内追加一次明确“直接输出完整日报正文”的纠偏重试。

**超时与失败诊断**：
- Daily Agent 外层仍使用 `timeout_ms` 作为最终运行上限。
- ChatGPT Web adapter 的内部 `timeout_secs` 会被下压到 `timeout_ms - 30s`（至少 1 秒），且不会放大用户在 runner 上配置的更短 timeout；这样浏览器 handoff、最终回复等待或页面扫描应先由 Web adapter 失败，外层 Daily timeout 只作为兜底。
- `ExternalCliRuntime` 必须把 ChatGPT Web 普通失败持久化为 failed run，而不是直接返回错误；run 目录必须包含 `result.json`、`cli.stderr.log`、`normalized_events.jsonl`、`last_message.md` 和必要的 `failure_diagnostics.json` 路径元数据。
- 失败状态会返回到 Daily Agent 并记录到当前 Agent 的 `last_status`、`last_error` 和 `last_run_id`；不更新 processed state，也不写入缺失或未通过门禁的 report。

**Conversation 管理**：
- 默认本地状态 key 为 `asr-daily:<task_id>:<agent_id>`，用于隔离不同 Agent 的状态文件和可观测信息。
- ChatGPT Web 不复用该 key 对应的旧 conversation；非 Web runner（Codex/Trae 等）仍可按 runner metadata 复用 thread/conversation。
- 修改 `AGENTS.md` 后，下一次 ChatGPT Web 运行会重新发送完整指令和当日内容。

### 5.6 IM 发送

**前提**：`im_delivery.enabled == true` 且 `im_delivery.channel` 已配置。

**流程**：

1. 确定本次新增/更新的 report 文件
2. 按 `mode` 构造消息：
   - `Summary`：日期 + 核心结论 + report 路径 + run_id
   - `FullReport`：完整 report 文本，超长时降级为 Summary
3. 通过 IM Gateway provider outbound 发送
4. 按 `send_policy` 决定是否发送：
   - `OnSuccessWithReport`：成功且有新 report
   - `OnSuccess`：成功即发送
   - `Always`：失败也发送状态

**失败处理**：记录到 `last_send_error`，不回滚 report，不影响 ASR 状态。

---

## 6. API

### 6.1 接口列表

| Method | Path | 用途 |
|--------|------|------|
| GET | `/api/asr/tasks/{task_id}/daily-agent` | 获取配置 + workspace 状态 + 最近 run |
| PUT | `/api/asr/tasks/{task_id}/daily-agent` | 更新 Daily Agent 配置 |
| GET | `/api/asr/tasks/{task_id}/daily-agent/agents` | 获取 AGENTS.md 内容 |
| PUT | `/api/asr/tasks/{task_id}/daily-agent/agents` | 保存 AGENTS.md |
| POST | `/api/asr/tasks/{task_id}/daily-agent/run` | 手动触发 run |
| GET | `/api/asr/tasks/{task_id}/daily-agent/runs` | 获取 run 历史 |
| GET | `/api/asr/tasks/{task_id}/daily-agent/reports/{date}` | 获取指定日期 report 全文（Markdown），用于全屏详情页 |
| POST | `/api/asr/tasks/{task_id}/daily-agent/send` | 发送最近 report 到 IM |
| POST | `/api/asr/tasks/{task_id}/daily-agent/sync` | 手动同步全部现有 report 到 `report_sync_dir` |

### 6.2 GET /daily-agent 响应

```json
{
  "task_id": "...",
  "config": {
    "enabled": true,
    "runner": "codex",
    "timeout_ms": 7200000,
    "trigger_policy": "after_asr_run",
    "session_key": null,
    "instructions_source": "default",
    "im_delivery": { "enabled": false },
    "report_sync_dir": "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/ASR Reports",
    "last_report_sync": {
      "total_files": 12,
      "copied_files": 2,
      "skipped_files": 10,
      "failed_files": 0
    }
  },
  "workspace": {
    "daily_dir": "...",
    "report_dir": "...",
    "agents_path": "...",
    "agents_exists": true,
    "git_available": true,
    "git_initialized": true,
    "git_error": null,
    "report_count": 2
  },
  "last_run": {
    "run_id": "...",
    "status": "success",
    "trigger_source": "asr_completion",
    "started_at_ms": 0,
    "finished_at_ms": 0,
    "error": null
  },
  "im_delivery": {
    "enabled": true,
    "channel": "target:chat_xxx",
    "mode": "summary",
    "send_policy": "on_success_with_report",
    "last_sent_at_ms": 0,
    "last_send_error": null
  }
}
```

### 6.3 GET/PUT /daily-agent/agents

- Query: `agent_id=<id>`，缺省选择第一个 Agent。
- Body: `{ "content": "..." }`
- 写入 `.daily/agents/<agent_id>/AGENTS.md`，Runner cwd 指向该 agent 工作目录。
- 更新对应 `agents[]` 项：`instructions_source=custom`, `instructions=<content>`。
- Best-effort: `git add . && git commit -m "update ASR daily agent instructions"`
- Git 失败返回 `git_warning` 但保存成功

### 6.4 POST /daily-agent/run

- 已有 active run → 返回 202 + 当前 run 状态
- 无 active run → 排队执行，返回 202 + run_id
- 可选参数：`date`（指定日期）/ `force`（强制覆盖）/ `send`（运行后发送 IM）
- 可选参数：`agent_id`；不传时按 `agents[]` 顺序串行运行所有 enabled Agent，传入时只运行指定 Agent。
- `date=YYYY-MM-DD` 必须把 change plan 限定到单个 `daily/YYYY-MM-DD.md`，用于 WebUI Daily Docs 行级 `Run Daily Agent` 操作；未传 `force` 时保持普通增量语义，不覆盖未变化的既有 report。

### 6.5 POST /daily-agent/send

- 不重新运行 Agent
- 读取最近/指定日期的 report
- 按 im_delivery 配置发送到绑定通道
- 可选 `agent_id`，只发送指定 Agent 输出目录下的最新 report。

### 6.6 验证规则

| 场景 | 响应 |
|------|------|
| 启用但未选 runner | 400: "必须选择 runner 或使用 Bifrost Agent" |
| 启用 IM 但未选 provider/target | 400: "必须完成 IM 绑定或关闭发送" |
| 关闭 Daily Agent | 不删除 workspace/report/AGENTS.md |

---

## 7. WebUI

### 7.1 ASR 创建/编辑页面

新增折叠区 **"Daily Agent Runner"**，字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| Enable daily agent | Toggle | — |
| Runner | Select | 单一 Runner 下拉；包含 `Bifrost Agent` 和 AI -> Agent -> Runners 中配置的 runner id。选择后直接保存为 `runner=<value>` |
| Trigger | Select | "After ASR run completes" / "Manual only" |
| Timeout | Number (分钟) | 默认 120 min |
| Session key | Text | 可空，默认运行时自动生成 |
| Report sync dir | Input + Save | 可选外部目录；保存到 `report_sync_dir`，空值表示不自动同步 |
| IM delivery | Toggle | 开启后展示 provider/target/mode/policy |
| IM Channel | Select | 单一通道下拉；列出可发送的 Provider Owner 通道和 IM Targets，直接保存为 `im_delivery.channel` |
| Send mode | Select | Summary / Full report |
| Send policy | Select | On success with report / On success / Always |
| Instructions | Editor | 默认展示内置手册；修改后 `instructions_source=custom` |

### 7.2 Task Detail - Daily Agent Tab

展示信息：
- Workspace path / Git status / AGENTS.md 状态
- Daily Agents 表格：启停、Agent 摘要、独立 output dir、Runner、IM 状态、Last Run 摘要、单 Agent Run、详情入口、删除、新增 Agent。
- Report count / Last run (status, run_id, duration, error)
- Report sync 状态：最近一次同步目标、总数、copied/skipped/failed、同步时间和错误摘要
- IM delivery 状态 (provider, target, last sent, last error)

操作按钮：
- Agent 详情页提供 `Agent Configuration` / `IM Delivery` / `Last Run Status` / `Agent Instructions`，并在详情内保存单 Agent 的 Runner、Trigger Policy、Timeout、Session Key、Output Dir、IM Channel、Mode、Send Policy 和 Instructions。
- Agent 行内 `Run` 和详情页 `Run Now` 只运行该 Agent；`Force Run` 只强制刷新该 Agent；`Send Report` 按当前 Agent 发送；列表页保留任务级 `Sync reports`。
- `Refresh status` / `Open Daily Docs` / `Open Reports`

### 7.2.1 Task Detail - Daily Docs Row Action

Daily Docs 表格的每个 `YYYY-MM-DD.md` 行在 `Action` 列同时提供：

- `Open document`：保持原行为，进入 `asrDay=<date>` 的日文档详情页。
- `Run All Agents`：调用 `POST /api/asr/tasks/{task_id}/daily-agent/run?date=<date>`，省略 `agent_id`，只对当前行日期按 `agents[]` 顺序排队全部 enabled Agent，不切换 tab、不影响其它日期文档。
- Agent 下拉动作：同一按钮右侧菜单列出 `Run <agent_id>`；选择后调用 `POST /api/asr/tasks/{task_id}/daily-agent/run?date=<date>&agent_id=<agent_id>`，只运行指定 Agent。禁用 Agent 在菜单中不可选。
- 按钮在请求提交期间展示行级 loading，并在已有行级请求未完成时禁用其它行的同类动作，避免用户连续触发同一 task 的并发 run。

---

## 7.3 CLI

Daily Agent report sync 必须同时提供 CLI 控制入口，便于不打开 WebUI 的自动化或 iCloud 目录配置：

```bash
bifrost ai asr task daily set-sync-dir <task> --dir "$HOME/Library/Mobile Documents/com~apple~CloudDocs/ASR Reports"
bifrost ai asr task daily set-sync-dir <task> --clear
bifrost ai asr task daily sync <task>
bifrost ai asr task daily sync <task> --dir "$HOME/Library/Mobile Documents/com~apple~CloudDocs/ASR Reports"
```

约束：
- `set-sync-dir` 只更新配置，不触发复制；`--clear` 清空 `report_sync_dir`。
- `set-sync-dir` 必须同时更新 legacy Daily Agent 配置和 primary agent 的 `report_sync_dir`；任务重新加载时会从 `agents[0]` 镜像 legacy 字段，不能让刚保存的同步目录在后续 `sync` 请求中被 normalize 覆盖。
- `sync` 在已有配置上手动同步全部 report；传 `--dir` 时先保存该目录再同步。
- 两个命令都复用 ASR task 的单任务自动选择/名称或 ID 前缀解析能力，并支持 `--json` 输出原始 API 响应。
- 普通文本输出必须显示 target、total、copied、skipped、failed。

---

## 8. 安全边界

- Runner `work_dir` 限制在 `daily_dir`
- `allow_work_dirs` 只包含 `daily_dir`
- 默认 prompt 明确禁止修改原始 `YYYY-MM-DD.md`
- Bifrost Agent file tools 限制在 `daily_dir`
- 不把 `~/.bifrost` 或音频源目录授予 runner
- API 写文件只允许 `AGENTS.md`
- IM 发送只使用用户显式绑定的 provider/target
- 未绑定时不得猜测默认群或默认个人

---

## 9. Git 行为规范

| 场景 | 行为 |
|------|------|
| `git` 不存在 | 跳过，UI 显示 "Git unavailable" |
| `git init` 失败 | warn，UI 显示错误，不阻塞 |
| `git commit` 失败 | warn，保存/运行仍然成功 |
| 已有 `.git` | 不删除，使用 `git -C <daily>` |
| `.DS_Store` | `.gitignore` 排除 |
| 全局 git user | 不自动配置 |

---

## 10. 日志规范

所有日志使用 `tracing` 结构化输出，必须包含 `task_id` 字段。

```text
initialized ASR daily agent workspace task_id=... daily_dir=... git_initialized=true
planned ASR daily agent changes task_id=... changed=1 unchanged=3 change_kinds="appended:1"
ASR daily agent delivery plan task_id=... runner=web adapter=chatgpt_web mode=incremental_payload dates=2026-05-19
queued ASR daily agent run task_id=... trigger_source=asr_completion source_dates=2026-05-19 runner=web
skipped ASR daily agent run task_id=... trigger_source=asr_completion reason="no daily markdown changes"
ASR daily agent run completed task_id=... run_id=... reports=2 duration_ms=...
updated ASR daily agent processed state task_id=... date=2026-05-19 source_sha256=... report_sha256=...
ASR daily agent IM delivery sent task_id=... run_id=... channel=target:daily-report-room
ASR daily agent IM delivery skipped task_id=... reason="channel not bound"
ASR daily agent git commit skipped task_id=... reason="git executable not found"
```

---

## 11. 内置 AGENTS.md 模板

**文件位置**：`crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_template.md`

**Rust 引用**：

```rust
const DEFAULT_ASR_DAILY_AGENTS_MD: &str = include_str!("daily_agent_template.md");
```

**模板核心规则**：
- 禁止修改 `~/.zshrc`
- 原始转写文件是 `YYYY-MM-DD.md`（只读）
- 报告输出到 `report/YYYY-MM-DD-report.md`
- 模板头部说明会被写入 Daily Agent 工作目录的 `AGENTS.md` 并由 Runner 实际读取；这些提示是运行契约，不是注释、示例或可忽略说明
- 优先提取：用户声音、工作事实、判断、灵感、待办、长期知识
- 每个“灵感爆发时刻”必须补充相关方向资料搜索、关键发现、可行性分析、方案草案、风险和下一步验证；如果 Runner 不能联网搜索，必须显式标记资料搜索受限，不得编造来源
- 不确定归因必须保留不确定性
- 明确报告结构和证据状态
- 知识沉淀必须内嵌在同一份 report 的“报告内知识沉淀模块”，按长期想法、方向决策、跨天待办、人物协作、学习资料、生活状态、术语误识别等模块分栏输出
- 报告内知识沉淀分栏不是装饰性标题；它们用于让同一份 report 同时承担日报、复盘、待办追踪和长期记忆索引职责
- 默认不得创建、建议创建或引用 `knowledge/*` 这类额外知识目录；Daily Agent 的运行模型是一对输入生成一份最终 report

**变量占位**（写入时替换，用户修改后保存为纯文本不再替换）：

```text
{{task_name}}
{{daily_dir}}
{{report_dir}}
```

---

## 12. 流程图

### 12.1 总体架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                        ASR Directory Task                            │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──────────┐    ┌──────────────┐    ┌──────────────────────────┐  │
│  │ Scheduler │───▶│ Audio        │───▶│ Daily Markdown           │  │
│  │ (10s tick)│    │ Processing   │    │ Generation               │  │
│  └──────────┘    └──────────────┘    └────────────┬─────────────┘  │
│                                                    │                 │
│                                                    ▼                 │
│                                      ┌──────────────────────────┐  │
│                                      │ maybe_enqueue_daily_      │  │
│                                      │ agent_after_asr_run()     │  │
│                                      └────────────┬─────────────┘  │
│                                                    │                 │
└────────────────────────────────────────────────────┼─────────────────┘
                                                     │
                                                     ▼
┌─────────────────────────────────────────────────────────────────────┐
│                     Daily Agent Runner Pipeline                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──────────────────┐    ┌──────────────────┐    ┌──────────────┐  │
│  │ ensure_workspace │───▶│ ChangePlanner     │───▶│ Runner       │  │
│  │ (dirs/git/agents)│    │ (diff/hash/plan)  │    │ Dispatch     │  │
│  └──────────────────┘    └──────────────────┘    └──────┬───────┘  │
│                                                          │          │
│                          ┌───────────────────────────────┼───┐      │
│                          │                               │   │      │
│                          ▼                               ▼   ▼      │
│                   ┌─────────────┐   ┌─────────────┐  ┌────────┐   │
│                   │Bifrost Agent│   │External CLI  │  │ChatGPT │   │
│                   │(run_turn)   │   │(Codex/Local) │  │Web     │   │
│                   └──────┬──────┘   └──────┬───────┘  └───┬────┘   │
│                          │                 │              │          │
│                          └────────────┬────┘──────────────┘          │
│                                       ▼                              │
│                          ┌──────────────────────────┐               │
│                          │ Post-run Actions          │               │
│                          │ • Update processed state  │               │
│                          │ • Write output report     │               │
│                          │ • IM delivery             │               │
│                          │ • Git commit              │               │
│                          └──────────────────────────┘               │
└─────────────────────────────────────────────────────────────────────┘
```

### 12.2 Workspace 初始化

```
ensure_asr_daily_workspace(task)
│
├── 1. daily_dir = text_output_dir/<task_id>/.daily
│
├── 2. 创建目录
│   ├── mkdir -p .daily/
│   └── mkdir -p agents/<agent_id>/input + output/<output_dir>
│
├── 3. AGENTS.md
│   ├── [存在?] ─── Yes ──▶ 跳过
│   └── No
│       ├── [custom && instructions 非空?]
│       │   ├── Yes ──▶ 写入 custom
│       │   └── No  ──▶ 写入默认模板（替换变量）
│       └── 写入 .gitignore
│
├── 4. Git
│   ├── [git 可用?]
│   │   ├── No  ──▶ warn, git_available=false
│   │   └── Yes
│   │       ├── [.git 存在?] ── Yes ──▶ 跳过
│   │       └── No ──▶ git init
│   │                  ├── 成功 ──▶ git_initialized=true
│   │                  └── 失败 ──▶ warn, git_error=<msg>
│   └── 不阻塞
│
└── 返回 AsrDailyWorkspaceStatus
```

### 12.3 ChangePlanner 决策

```
build_daily_agent_change_plan(task, trigger, date, force)
│
├── 1. 扫描 daily/*.md（排除 AGENTS.md / report/ / 隐藏文件）
│
├── 2. 对每个 YYYY-MM-DD.md:
│   ├── 计算 sha256 / len / mtime
│   ├── 读取 processed state
│   │
│   ├── [force?] ─── Yes ──▶ force
│   ├── [无记录?] ── Yes ──▶ new_file
│   ├── [sha256 同?] Yes ──▶ unchanged
│   ├── [前缀匹配?] Yes ──▶ appended（只投递 tail）
│   └── 否则 ──────────────▶ rewritten（投递 diff）
│
├── 3. [全部 unchanged && !force?]
│   ├── Yes ──▶ skipped_no_daily_changes
│   └── No  ──▶ 返回 ChangePlan
│
└── 4. 后续:
    ├── 成功 ──▶ 更新 processed state
    └── 失败 ──▶ 不更新
```

---

## 13. 时序图

### 13.1 ASR 完成后自动触发 Daily Agent

```
┌────────┐  ┌──────────────┐  ┌────────────────┐  ┌──────────────┐  ┌──────────┐  ┌────────┐
│Scheduler│  │run_directory_│  │refresh_daily_  │  │maybe_enqueue_│  │Change    │  │Runner  │
│(10s)    │  │task()        │  │summaries()     │  │daily_agent() │  │Planner   │  │        │
└────┬────┘  └──────┬───────┘  └───────┬────────┘  └──────┬───────┘  └────┬─────┘  └───┬────┘
     │              │                  │                   │               │             │
     │─due task───▶ │                  │                   │               │             │
     │              │                  │                   │               │             │
     │              │─process audio────│                   │               │             │
     │              │  (chunks/retry)  │                   │               │             │
     │              │                  │                   │               │             │
     │              │─stop asr-server──│                   │               │             │
     │              │                  │                   │               │             │
     │              │─────────────────▶│                   │               │             │
     │              │                  │─write daily .md──▶│               │             │
     │              │                  │◀─paths[]─────────│               │             │
     │              │                  │                   │               │             │
     │              │─update_task_after_run()──────────────▶               │             │
     │              │                  │                   │               │             │
     │              │─────────────────────────────────────▶│              │             │
     │              │                  │                   │─check config──│             │
     │              │                  │                   │               │             │
     │              │                  │                   │─ensure_workspace            │
     │              │                  │                   │               │             │
     │              │                  │                   │──────────────▶│             │
     │              │                  │                   │              │─scan + hash  │
     │              │                  │                   │◀─change_plan─│             │
     │              │                  │                   │               │             │
     │              │                  │                   │─[has changes?]────────────▶│
     │              │                  │                   │               │             │
     │              │                  │                   │               │    run agent│
     │              │                  │                   │               │             │
     │              │                  │                   │◀──result──────────────────  │
     │              │                  │                   │               │             │
     │              │                  │                   │─update state / IM / git     │
     │              │                  │                   │               │             │
```

### 13.2 ChatGPT Web 首次 + 后续投递

```
┌────────────┐  ┌──────────────┐  ┌─────────────────┐  ┌──────────────┐
│Daily Agent │  │Conversation  │  │ExternalCli      │  │ChatGPT Web   │
│Runner      │  │State         │  │Runtime          │  │(Browser/CDP) │
└─────┬──────┘  └──────┬───────┘  └────────┬────────┘  └──────┬───────┘
      │                │                    │                   │
      │─check state───▶│                    │                   │
      │◀─not_initialized                    │                   │
      │                │                    │                   │
      │─[首次: 2 条消息]─────────────────── ▶│                  │
      │                │                    │─msg1: AGENTS.md──▶│
      │                │                    │                   │
      │                │                    │─msg2: daily text──▶│
      │                │                    │◀─response─────────│
      │◀─result + conv_ref─────────────────│                   │
      │                │                    │                   │
      │─persist state─▶│                    │                   │
      │─write report   │                    │                   │
      │                │                    │                   │
      ╠═══════════════ 次日 ════════════════════════════════════╣
      │                │                    │                   │
      │─check state───▶│                    │                   │
      │◀─initialized   │                    │                   │
      │                │                    │                   │
      │─[后续: 1 条消息]─────────────────── ▶│                  │
      │                │                    │─msg: tail only───▶│
      │                │                    │  (不重发AGENTS.md) │
      │                │                    │◀─response─────────│
      │◀─result────────────────────────────│                   │
      │                │                    │                   │
```

### 13.3 Bifrost Agent / Codex 本地运行

```
┌────────────┐  ┌──────────────────┐  ┌─────────────┐
│Daily Agent │  │Bifrost Agent /   │  │File System  │
│Runner      │  │Codex (work_dir)  │  │(daily/)     │
└─────┬──────┘  └────────┬─────────┘  └──────┬──────┘
      │                   │                    │
      │─prompt: file list▶│                    │
      │  work_dir=daily/  │                    │
      │                   │                    │
      │                   │─read AGENTS.md────▶│
      │                   │◀─instructions──────│
      │                   │                    │
      │                   │─read 05-19.md─────▶│
      │                   │◀─content───────────│
      │                   │                    │
      │                   │─write report/      │
      │                   │  05-19-report.md──▶│
      │                   │                    │
      │◀─result───────────│                    │
      │                   │                    │
      │─update state      │                    │
      │─git commit        │                    │
      │─IM delivery       │                    │
      │                   │                    │
```

### 13.4 手动 Run Now + IM 发送

```
┌──────┐  ┌─────────┐  ┌────────────┐  ┌──────────┐  ┌────────────┐  ┌──────────┐
│WebUI │  │API      │  │Daily Agent │  │Runner    │  │IM Gateway  │  │IM Target │
└──┬───┘  └────┬────┘  └─────┬──────┘  └────┬─────┘  └─────┬──────┘  └────┬─────┘
   │           │              │              │               │              │
   │─POST /run▶│              │              │               │              │
   │           │─[active?] No │              │               │              │
   │◀─202──────│              │              │               │              │
   │           │─enqueue─────▶│              │               │              │
   │           │              │─workspace    │               │              │
   │           │              │─change plan  │               │              │
   │           │              │─dispatch────▶│               │              │
   │           │              │              │─process       │              │
   │           │              │◀─result──────│               │              │
   │           │              │              │               │              │
   │           │              │─[IM enabled?]│               │              │
   │           │              │─send message─────────────────▶              │
   │           │              │              │               │─send────────▶│
   │           │              │              │               │◀─ok──────────│
   │           │              │◀─delivery ok─────────────────│              │
   │           │              │              │               │              │
   │           │              │─git commit   │               │              │
   │           │              │              │               │              │
   │─GET /daily-agent────────▶              │               │              │
   │◀─{last_run: success}────│              │               │              │
```

### 13.5 服务重启 Orphan 恢复

```
┌───────────┐  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────┐
│ Server    │  │ Scheduler        │  │ Processed State  │  │ Daily Agent  │
│ Startup   │  │ ensure_started() │  │ JSON             │  │ Queue        │
└─────┬─────┘  └────────┬─────────┘  └────────┬─────────┘  └──────┬───────┘
      │                  │                     │                    │
      │─start──────────▶│                     │                    │
      │                  │                     │                    │
      │                  │─load tasks─────────▶│                   │
      │                  │                     │                    │
      │                  │─[daily_agent.enabled │                    │
      │                  │  && running_run_id   │                    │
      │                  │  && not active?]     │                    │
      │                  │  → orphan detected   │                    │
      │                  │                     │                    │
      │                  │─[ASR completed      │                    │
      │                  │  after last agent?]  │                    │
      │                  │                     │                    │
      │                  │─re-enqueue──────────────────────────────▶│
      │                  │  trigger=retry_after_restart             │
      │                  │                     │                    │
```

---

## 14. 不变量约束

实现过程中必须始终保证：

1. ASR 原始 `YYYY-MM-DD.md` 由 `generate_daily_summaries()` 生成，Daily Agent 不修改
2. 旧任务无 `daily_agent` 字段时升级为默认双 Agent；旧单 Agent 配置加载时保留 legacy runner/instructions/status 并补齐 `tomorrow_todo`
3. 已存在的 `.daily/agents/<agent_id>/AGENTS.md` 不被默认模板覆盖
4. Daily Agent 不得在 ASR 音频处理、chunk retry、daily markdown 刷新完成前启动
5. Daily Agent 失败不影响 ASR task 状态
6. `update_task_after_run()` 不触碰 `daily_agent` 字段
7. Daily Agent 与 ASR run 使用独立的并发锁和运行状态集

---

## 15. 实施任务

所有功能一次性实现，不分阶段。

| # | 任务 | 涉及文件 |
|---|------|----------|
| 1 | 新增 `asr_daily_agents_default.md` asset | `crates/bifrost-admin/assets/` |
| 2 | 扩展 `AsrDirectoryTask` 数据模型 | `asr_jobs/state.rs` |
| 3 | 实现 `ensure_asr_daily_workspace()` | 新文件 `asr_jobs/daily_agent.rs` |
| 4 | 创建任务/详情时初始化 workspace | `asr_jobs/api.rs` |
| 5 | 新增 Daily Agent API (6 个接口) | `asr_jobs/api.rs` |
| 6 | 实现 `DailyAgentChangePlanner` | `asr_jobs/daily_agent.rs` |
| 7 | 实现 `maybe_enqueue_daily_agent_after_asr_run()` | `asr_jobs/runner.rs` |
| 8 | Runner 执行逻辑 (Bifrost Agent + External CLI) | `asr_jobs/daily_agent.rs` |
| 9 | IM delivery 逻辑（通道选择 + 默认 Owner 发送） | `asr_jobs/daily_agent.rs` |
| 10 | ChatGPT Web conversation 管理 | `asr_jobs/daily_agent.rs` |
| 11 | Git init + 每次 run 后自动 commit | `asr_jobs/daily_agent.rs` |
| 12 | 服务重启 orphan detection + re-enqueue | `asr_jobs/daily_agent.rs` |
| 13 | WebUI: ASR 创建/编辑页 Daily Agent 配置区 | WebUI |
| 14 | WebUI: Task Detail Daily Agent Tab | WebUI |
| 15 | WebUI: Daily Agent report 可点击全屏 Markdown 详情 | WebUI + `/daily-agent/reports/{date}` |

---

## 16. 测试计划

### 16.1 单元测试

| 测试点 | 断言 |
|--------|------|
| 缺少 `daily_agent` 字段 | 反序列化/加载后为默认双 Agent |
| 旧单 Agent 配置 | 保留 legacy 字段并补齐 `tomorrow_todo` |
| `ensure_asr_daily_workspace()` | 创建 daily/ + agents/<agent_id>/AGENTS.md + input/ + output/<output_dir>/ + .gitignore，并分发 daily markdown 副本 |
| 已存在 AGENTS.md | 不覆盖 |
| git 不存在 | 不阻塞，返回 git_available=false |
| `PUT /daily-agent/agents` | 保存 custom instructions + 写文件 |
| `maybe_enqueue_daily_agent_after_asr_run()` | 只在 ASR terminal + daily 已刷新后排队 |
| ChangePlanner: 首次文件 | `new_file` |
| ChangePlanner: append-only | `appended` + 正确 byte range |
| ChangePlanner: non-append 变化 | `rewritten` |
| ChangePlanner: hash 相同 | `unchanged` |
| Runner 成功 | 更新 processed state |
| 同日多 Agent | processed key 为 `<agent_id>:<date>`，Records 不互相覆盖 |
| Runner 失败 | 不更新 processed state |
| ChatGPT Web plan | 不含 unchanged 文档 |
| Bifrost Agent plan | 不含 daily markdown 全文 |
| 已有 active run | 不重复启动 |
| work_dir 限制 | 只允许 daily_dir |

### 16.2 E2E / API

| 测试场景 | 验证点 |
|----------|--------|
| 创建 ASR task | daily workspace 自动存在 |
| 启用 Daily Agent | WebUI 通过单一 Runner 下拉保存 `runner/trigger_policy/timeout` |
| ASR run 完成 | 先完成音频处理，再看到 Daily Agent queued |
| ASR 仍在 processing | 不启动 Daily Agent |
| daily markdown 未变化 | skipped，不产生 Runner run |
| 追加内容后再触发 | ChatGPT Web 只收新增 tail |
| IM delivery 绑定 | 保存 `channel/mode/policy` |
| 手动 Run now | report/ 生成文件 |
| Daily Docs 行级 Run All Agents | 点击某日文档行主按钮后，请求只携带该行 `date`，不携带 `force`/`agent_id`，后端 change plan 只包含该日期并按顺序运行全部 enabled Agent |
| Daily Docs 行级 Run 单 Agent | 点击某日文档行 Agent 下拉项后，请求携带该行 `date` 和对应 `agent_id`，只运行指定 Agent |
| Report sync dir 自动同步 | 配置 `report_sync_dir` 后 Runner 成功生成 report，会把本轮 report 复制到目标目录，并在 `last_report_sync` 记录 copied/skipped/failed；外部目录超时只记录同步错误，不影响 report 生成成功状态 |
| 手动同步 report | 调用 `/daily-agent/sync` 后同步全部现有 report，目标目录已有文件时不读取目标 hash，使用临时文件覆盖；iCloud/外部目录卡住时 API 超时返回失败结果但代理进程继续响应其他请求 |
| CLI 同步控制 | `daily set-sync-dir` 能设置/清除目录，`daily sync` 能手动同步并输出 target/total/copied/skipped/failed |
| 打开 report 详情 | `/daily-agent/reports/{date}` 返回 report Markdown 全文，非法日期拒绝，缺失 report 返回 404 |
| 历史 report 发现 | `/daily-agent/runs` 合并 `daily_agent_processed.json` 与磁盘 `.daily/agents/<agent_id>/output/<output_dir>/`，并兼容旧版 `daily/<output_dir>/`、`.daily/agents/<agent_id>/<output_dir>/` 和 `daily/Report/` 下的 `YYYY-MM-DD-report.md`；即使 processed state 缺失，Daily Agent Records 也必须展示已有报告，并按日期倒序返回 |
| 自动 completion hook | run detail 记录 trigger_source=asr_completion |
| 未绑定 IM | 不发送，记录 skipped |
| git 不存在 | 创建任务和保存 AGENTS.md 仍成功 |

### 16.3 WebUI

- 创建页展示 Daily Agent Runner 配置
- Runner 下拉复用 AI -> Agent -> Runners 中已配置的 runner id，且不再暴露单独的 Runner type / Runner ID 输入
- 选择 `Bifrost Agent`、`codex`、`web` 等选项时，保存 payload 直接写入单字段 `runner`
- IM delivery 只展示一个 IM Channel 下拉，列出 Provider Owner 和 IM Targets，不再要求用户手填 Provider ID / Target ID
- 选择 IM Channel 后，保存 payload 直接写入单字段 `im_delivery.channel`
- Trigger 显示为跟随 ASR 完成后运行
- IM delivery 开关联动通道选择/mode/policy
- 默认指导手册在 editor 中可见
- 保存后 Daily Agent tab 读取到 custom AGENTS.md
- Configuration 区域展示 Report Sync Dir 输入框和 Save 按钮；下方提供 Sync Reports 手动按钮，未配置目录时禁用。
- Last Run Status 区域展示最近同步状态，包含 copied/total、skipped、同步目录和失败错误摘要。
- Processed Documents 中任一 report 文件名可点击，进入全屏详情页并使用 Markdown 渲染器展示正文
- Daily Docs 表格每行提供 `Run All Agents` 主按钮和单 Agent 下拉菜单，调用 date-scoped run API；行级动作与 `Open document` 共存，不能破坏 Daily Docs tab 的 URL 恢复和文档打开行为。
- Daily Agent Records 列表和 report 详情必须使用一致的状态来源：当 `daily_agent_processed.json` 中某日期记录了已存在的 `report_path` 时，`/daily-agent/reports/{date}` 必须读取同一个状态路径，而不是重新拼接另一个 workspace 路径导致列表可见、详情 404。
- Daily Agent Records 不只依赖 `daily_agent_processed.json`：页面刷新时必须通过 `/daily-agent/runs` 展示磁盘中已存在的 `YYYY-MM-DD-report.md`，兼容历史任务里的 `Report` 大写目录；从该兜底记录打开详情时 `/daily-agent/reports/{date}` 必须读取同一真实文件。
- Run Results 表格必须以最新数据优先展示，按 `date` 倒序排列；前端在消费 API 时保留同样的防御性排序，避免旧服务或 mock 数据无序导致用户先看到旧记录。
- Daily Agent Records 的 Run Results 顶部必须支持按 Agent、Date、Runner 三个维度筛选；筛选只影响当前列表展示，不改变已生成 report 或后端 processed state。
- Daily Agent Records 表格在窄窗口下必须把横向滚动限制在表格内部，不能撑宽整个 ASR task tab 或导致左侧列被页面级横向滚动裁切。
- Daily Agent report 详情页正文不允许嵌套纵向滚动；Markdown 内容自然撑开页面，只使用 ASR 页面最外层滚动条。
- Daily Agent tab 是 Agent 管理列表页，只保留任务级启用、Report Sync、Refresh、Add、Run、Edit、Delete 等列表管理动作；单个 Agent 的 Runner、Trigger Policy、Timeout、Session Key、Output Dir、IM Delivery、Last Run Status 和 Instructions 必须进入 Agent 详情页配置，避免列表和详情信息混排。
- 每个 Agent 都可以在管理列表行和 Agent 详情页单独运行/调试；Daily Docs 行级 `Run Daily Agent` 必须提供 `Run All Agents` 默认动作，并在下拉菜单中列出各个 Agent 的单独运行动作，调用 API 时通过 `agent_id` 区分单 Agent，省略 `agent_id` 表示全部 Agent。
- `Run All Agents` 对同一 task 的多个 Agent 必须串行执行并复用同一个 task-level running lock；多个 Agent 即使配置为同一个 ChatGPT Web runner，也不能并发抢占 runner。单个 Agent 的 runner 失败必须只记录到该 Agent 的 `last_status`、`last_error` 和 `run_id`，队列继续执行后续 Agent；默认 `session_key` 和 conversation state 必须包含 `task_id + agent_id`，避免不同 Agent 复用同一个 ChatGPT Web conversation。
- Daily Agent 列表在窄窗口下必须保持稳定列宽并启用横向滚动，禁止把日期、输出目录、IM Channel 或 Actions 按单字符折行成竖排；关键单元格使用 nowrap。
- ASR 首页顶层 tab 文案统一使用英文：`Scheduled Tasks`、`ASR Management`、`Voiceprint & Wake`。
- report 详情页展示任务、日期、路径、大小、修改时间、处理时间和 Runner，并支持返回 Daily Agent 列表
- 任务详情 tab 使用 URL 参数保持状态；刷新 `Daily Agent` tab 或 report 全屏详情页时，页面必须从 `asrTaskTab=daily-agent` / `asrDailyReport=YYYY-MM-DD` 恢复，不要求用户重新点击切换
- 亮色/暗色主题下均可读可操作

### 16.4 human_tests

| 用例编号 | 名称 |
|----------|------|
| TC-ASPB-25 | Daily Agent Runner 方案文档验收 |
| TC-ASPB-26 | ASR task 创建时初始化 daily workspace |
| TC-ASPB-27 | WebUI 配置 Runner 并编辑 AGENTS.md |
| TC-ASPB-28 | ASR 音频处理完成后自动触发 Daily Agent 生成 report 并写入 Git 历史 |
| TC-ASPB-29 | 绑定 IM 通道后 Daily Agent 发送处理结论 |
| TC-ASPB-35 | Daily Agent Processed Documents report 全屏 Markdown 详情 |
| TC-ADA-07 | Daily Agent 管理列表与 Agent 详情页分离 |
| TC-ADA-08 | ASR 顶层 tab 英文文案与窄窗口列表布局 |
| TC-ADA-09 | Daily Docs 行级 Run Daily Agent 可选择全部或单个 Agent |
| TC-ADA-10 | 多 Agent 共用 ChatGPT Web Runner 的串行运行与失败隔离 |
| TC-ADA-11 | 每个 Agent 工作目录包含 input 副本并按 Runner 独立落档 |
| TC-ADA-15 | ChatGPT Web Daily Agent 失败先由 adapter 收敛并持久化诊断产物 |
| TC-DAR-01 | Daily Agent Records 从已有 Report 目录兜底发现历史报告 |
| TC-DAR-09 | Daily Agent Records 支持按 Agent、Date、Runner 筛选 |
| TC-DAR-10 | Daily Agent Records 窄窗口表格横向滚动不撑宽 tab |
| TC-DAR-11 | Daily Agent report 详情页不嵌套纵向滚动 |
| TC-QASR-25 | Daily Docs 单文档行级 Run Daily Agent |

---

## 17. 代码对齐参照

| 现有模块 | 复用方式 |
|----------|----------|
| `AsrDirectoryTask` (`state.rs`) | 新增 `#[serde(default)] pub daily_agent` 字段；Daily Agent 数据模型与逻辑全部住在 `asr_jobs/daily_agent*.rs` 子模块 |
| `run_directory_task()` (`runner.rs`) | 在 `update_task_after_run()` 之后调用 `maybe_enqueue_daily_agent_after_asr_run(&updated)`；同步 `retry.rs` 中两个重试路径也调用该 hook |
| `ASR_JOB_RUN_LOCK` (`state.rs`) | Daily Agent 使用 per-task 锁，不占用全局 GPU 锁 |
| `RUNNING_TASKS` (`state.rs`) | 新增独立 `DAILY_AGENT_RUNNING_TASKS`；另有 `DAILY_AGENT_TASK_LOCKS` 提供 per-task 串行 |
| `ExternalCliRunRequest` (`external_cli/mod.rs`) | 复用 session_key / work_dir / allow_work_dirs / adapter_config |
| `ScheduleConversationRef` (`types.rs`) | ChatGPT Web conversation 复用 conversation_id/thread_id |
| `requested_or_session_conversation()` (`chatgpt_web/interaction.rs`) | session map 查找复用 |
| `bifrost_agent::session::run_turn` (`agent.rs`) | Bifrost Agent 路径直接调用 |
| `generate_daily_summaries()` (`asr_jobs_timeline.rs`) | daily markdown 生成 |
| `text_output_dir()` (`asr_runtime`) | workspace 路径计算 |
| `TaskRunFileLock` (`state.rs`) | 文件锁复用模式 |
| `repair_interrupted_processing_records_on_startup()` (`runner.rs`) | Phase 2 orphan recovery 参照 |

---

## 18. 实现注意事项

1. **并发锁**：使用 `tokio::sync::Mutex<()>` + `TaskRunFileLock("daily-agent:<task_id>")`，与现有 ASR run 锁模式一致但完全独立。

2. **状态隔离**：`update_task_after_run()` 只更新 ASR 级别的 `last_run_at_ms`/`last_error`；Daily Agent 状态在 `AsrDailyAgentConfig` 内部独立维护。

3. **ChatGPT Web 大输入投递**：ChatGPT Web composer 超过 120 字符时必须走浏览器原生剪贴板 + 原生粘贴快捷键路径，避免把完整正文嵌入 `Input.insertText` 导致 CDP 卡死；不要再按固定字符数人为分片。该路径通过 CDP 写入当前浏览器上下文的 `navigator.clipboard`，再触发 `Meta+V` / `Ctrl+V`，不依赖系统剪贴板或用户授权弹窗。粘贴大文本后 ChatGPT 可能把内容上传为文件，此时输入框没有可采样正文是正常状态；adapter 不再对 composer 文本做 head/tail/长度采样校验，只轮询发送按钮是否变为可发送状态，按钮可用后立即继续；超时时间只是最大上限，用于覆盖长文档上传/解析耗时。

4. **`appended` 判定**：读取 processed state 中的 `source_len_bytes`，取当前文件前 N bytes 与前次 sha256 比对。如果前缀匹配，remainder 为 tail；否则判定为 `rewritten`。

5. **资源释放顺序**：Daily Agent 排队前确认 ASR managed server / asr 进程 / ffmpeg 子进程均已释放，避免资源竞争。

6. **`AsrDailyAgentProcessedState` 原子写入**：写入临时文件后 rename，避免写入中断导致状态损坏。

7. **Terminology / TERMS.md**（2026-06 新增）：`AsrDailyAgentConfig.terminology` 保存用户配置的专有名词列表；`ensure_asr_daily_workspace` 通过 `sync_daily_agent_terms_file` 写入 `.daily/agents/<agent_id>/TERMS.md`，并由 `ensure_daily_agent_terms_reference` 在 `AGENTS.md` 中维护一个 managed reference block。Daily Agent prompt（`daily_agent_prompt.rs`）会在 TERMS 存在时把内容嵌入提示词，让 Runner 在写 report 前应用术语纠错。
