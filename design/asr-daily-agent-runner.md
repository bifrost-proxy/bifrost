# ASR Daily Agent Runner 技术规格

## 1. 概述

ASR 定时任务完成音频转写后，自动触发 Daily Agent Runner 对每日汇总做二次整理，将结果写入 `report/` 并用 Git 追踪变化。

**定位**：ASR 定时任务的后处理阶段，不是独立的定时任务。

**作用域**：本规格只涉及 ASR 每日转写汇总后的 Agent Runner。不改变 ASR 音频转写主链路（转写成功率、chunk retry、daily markdown 生成均由现有流程负责）。

### 目录结构

```text
<BIFROST_DATA_DIR>/asr/data/text/<task_id>/daily/
├── AGENTS.md                    # 指导手册
├── .gitignore                   # 排除 .DS_Store
├── .git/                        # best-effort 版本追踪
├── YYYY-MM-DD.md                # ASR 生成的每日转写（只读）
└── report/
    └── YYYY-MM-DD-report.md     # Agent Runner 生成的报告
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
| 6 | AGENTS.md 存储 | 双写：同时写入文件和 task config 的 `instructions` 副本（备份用途） |
| 7 | Git commit | Phase 1 只做 `git init`；Runner 运行后 best-effort `git add + commit` |
| 8 | Report 覆盖策略 | 默认不覆盖，`force=true` 时才允许覆盖 |

---

## 3. 数据模型

### 3.1 AsrDirectoryTask 扩展

在 `AsrDirectoryTask`（`crates/bifrost-admin/src/handlers/asr_jobs/state.rs`）增加：

```rust
#[serde(default)]
pub daily_agent: AsrDailyAgentConfig,
```

### 3.2 AsrDailyAgentConfig

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDailyAgentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
}
```

**默认值**：

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `enabled` | `false` | 必须用户显式启用 |
| `runner` | `bifrost_agent` | 内置 Agent；其它值是 Runners 中的 runner id |
| `timeout_ms` | `7_200_000` (2h) | — |
| `trigger_policy` | `after_asr_run` | — |
| `session_key` | `None` | 运行时默认 `asr-daily:<task_id>` |
| `instructions_source` | `default` | — |
| `instructions` | `None` | — |
| `im_delivery.enabled` | `false` | 需用户绑定 channel |
| `im_delivery.mode` | `summary` | — |
| `im_delivery.send_policy` | `on_success_with_report` | — |

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
pub(crate) struct AsrDailyAgentProcessedDocument {
    pub date: String,
    pub source_path: String,
    pub source_sha256: String,
    pub source_len_bytes: u64,
    pub source_mtime_ms: Option<u64>,
    pub processed_at_ms: u64,
    pub runner: String,
    pub report_path: Option<String>,
    pub report_sha256: Option<String>,
    pub last_run_id: String,
    pub last_delivery_mode: AsrDailyAgentDeliveryMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentDeliveryMode {
    IncrementalPayload,
    FileList,
}
```

### 3.6 Conversation State（ChatGPT Web 专用）

存储路径：`<BIFROST_DATA_DIR>/asr/tasks/<task_id>/daily_agent_conversations.json`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDailyAgentConversationState {
    pub task_id: String,
    pub runner: String,
    pub adapter: String,
    pub session_key: String,
    pub initialized_at_ms: Option<u64>,
    pub conversation_ref: Option<ScheduleConversationRef>,
    pub last_message_at_ms: Option<u64>,
}
```

---

## 4. 并发控制

| 锁 | 类型 | 用途 |
|----|------|------|
| `DAILY_AGENT_RUN_LOCK` | `tokio::sync::Mutex<()>` | 全局串行，避免多任务 Agent 同时运行 |
| `DAILY_AGENT_RUNNING_TASKS` | `StdMutex<HashSet<String>>` | 防止同一 task 重复触发 Daily Agent |
| `TaskRunFileLock` | 文件锁 | key=`daily-agent:<task_id>`，跨进程保护 |

**关键约束**：
- `DAILY_AGENT_RUNNING_TASKS` 与现有 `RUNNING_TASKS`（ASR run 用）完全独立，互不影响。
- ASR run 正在运行时仍可排队 Daily Agent（等 ASR 完成后才执行）。
- `ASR_JOB_RUN_LOCK` 释放后才能获取 `DAILY_AGENT_RUN_LOCK`。

---

## 5. 核心实现

### 5.1 Workspace 初始化

```rust
fn ensure_asr_daily_workspace(task: &AsrDirectoryTask) -> Result<AsrDailyWorkspaceStatus, String>
```

**执行步骤**：

1. 计算 `daily_dir = text_output_dir(data_dir) / task.id / "daily"`
2. `mkdir -p daily/` 和 `mkdir -p daily/report/`
3. 如果 `AGENTS.md` 不存在：
   - `instructions_source == Custom && instructions.is_some()` → 写入 custom
   - 否则 → 写入内置默认模板（替换 `{{task_name}}`/`{{daily_dir}}`/`{{report_dir}}`）
4. 写入 `.gitignore`（内容：`.DS_Store`）
5. 尝试 `git init`（失败只 warn，不阻塞）

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
async fn maybe_enqueue_daily_agent_after_asr_run(
    task: &AsrDirectoryTask,
    outcome: &AsrRunOutcome,
)
```

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

1. 获取 `DAILY_AGENT_RUN_LOCK` + `TaskRunFileLock("daily-agent:<task_id>")`
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

- 2026-05-15.md: change_kind=appended, source_sha256=..., report=report/2026-05-15-report.md
- 2026-05-16.md: change_kind=new_file, source_sha256=..., report=report/2026-05-16-report.md

只刷新这些日期对应的 report。不要修改原始 YYYY-MM-DD.md。
```

#### ChatGPT Web（不可读本地文件）

**首次**（conversation 未初始化）：
- 消息 1：`AGENTS.md` 全文 + 规则说明
- 消息 2：change plan 中需处理的内容 + 输出要求

**后续**（conversation 已初始化）：
- 只发 change plan 中 `new_file/appended/rewritten/force` 的内容
- 不重发 `AGENTS.md`
- `unchanged` 不发送

**Conversation 管理**：
- 默认 `session_key = asr-daily:<task_id>`（任务级长期会话）
- 修改 `AGENTS.md` 后不自动重发，UI 提醒用户手动 reset
- Conversation 重置条件：用户手动 reset / 修改 session_key / 后端检测 404

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
| POST | `/api/asr/tasks/{task_id}/daily-agent/send` | 发送最近 report 到 IM |

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
    "im_delivery": { "enabled": false }
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

### 6.3 PUT /daily-agent/agents

- Body: `{ "content": "..." }`
- 写入 `daily/AGENTS.md`
- 更新 config: `instructions_source=custom`, `instructions=<content>`
- Best-effort: `git add AGENTS.md && git commit -m "update ASR daily agent instructions"`
- Git 失败返回 `git_warning` 但保存成功

### 6.4 POST /daily-agent/run

- 已有 active run → 返回 202 + 当前 run 状态
- 无 active run → 排队执行，返回 202 + run_id
- 可选参数：`date`（指定日期）/ `force`（强制覆盖）/ `send`（运行后发送 IM）

### 6.5 POST /daily-agent/send

- 不重新运行 Agent
- 读取最近/指定日期的 report
- 按 im_delivery 配置发送到绑定通道

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
| IM delivery | Toggle | 开启后展示 provider/target/mode/policy |
| IM Channel | Select | 单一通道下拉；列出可发送的 Provider Owner 通道和 IM Targets，直接保存为 `im_delivery.channel` |
| Send mode | Select | Summary / Full report |
| Send policy | Select | On success with report / On success / Always |
| Instructions | Editor | 默认展示内置手册；修改后 `instructions_source=custom` |

### 7.2 Task Detail - Daily Agent Tab

展示信息：
- Workspace path / Git status / AGENTS.md 状态
- Report count / Last run (status, run_id, duration, error)
- IM delivery 状态 (provider, target, last sent, last error)

操作按钮：
- `Edit AGENTS.md` / `Save config`
- `Run now` / `Send last report now`
- `Refresh status` / `Open Daily Docs` / `Open Reports`

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

**文件位置**：`crates/bifrost-admin/assets/asr_daily_agents_default.md`

**Rust 引用**：

```rust
const DEFAULT_ASR_DAILY_AGENTS_MD: &str =
    include_str!("../../assets/asr_daily_agents_default.md");
```

**模板核心规则**：
- 执行 shell 前 `source ~/.zshrc`
- 禁止修改 `~/.zshrc`
- 原始转写文件是 `YYYY-MM-DD.md`（只读）
- 报告输出到 `report/YYYY-MM-DD-report.md`
- 优先提取：用户声音、工作事实、判断、灵感、待办、长期知识
- 不确定归因必须保留不确定性
- 明确报告结构和证据状态

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
│                          │ • Write report/           │               │
│                          │ • IM delivery             │               │
│                          │ • Git commit              │               │
│                          └──────────────────────────┘               │
└─────────────────────────────────────────────────────────────────────┘
```

### 12.2 Workspace 初始化

```
ensure_asr_daily_workspace(task)
│
├── 1. daily_dir = text_output_dir/<task_id>/daily
│
├── 2. 创建目录
│   ├── mkdir -p daily/
│   └── mkdir -p daily/report/
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
2. 旧任务无 `daily_agent` 字段时正常反序列化为默认 disabled
3. 已存在的 `daily/AGENTS.md` 不被默认模板覆盖
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

---

## 16. 测试计划

### 16.1 单元测试

| 测试点 | 断言 |
|--------|------|
| 缺少 `daily_agent` 字段 | 反序列化为 disabled |
| `ensure_asr_daily_workspace()` | 创建 daily/ + report/ + AGENTS.md + .gitignore |
| 已存在 AGENTS.md | 不覆盖 |
| git 不存在 | 不阻塞，返回 git_available=false |
| `PUT /daily-agent/agents` | 保存 custom instructions + 写文件 |
| `maybe_enqueue_daily_agent_after_asr_run()` | 只在 ASR terminal + daily 已刷新后排队 |
| ChangePlanner: 首次文件 | `new_file` |
| ChangePlanner: append-only | `appended` + 正确 byte range |
| ChangePlanner: non-append 变化 | `rewritten` |
| ChangePlanner: hash 相同 | `unchanged` |
| Runner 成功 | 更新 processed state |
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
- 亮色/暗色主题下均可读可操作

### 16.4 human_tests

| 用例编号 | 名称 |
|----------|------|
| TC-ASPB-25 | Daily Agent Runner 方案文档验收 |
| TC-ASPB-26 | ASR task 创建时初始化 daily workspace |
| TC-ASPB-27 | WebUI 配置 Runner 并编辑 AGENTS.md |
| TC-ASPB-28 | ASR 音频处理完成后自动触发 Daily Agent 生成 report 并写入 Git 历史 |
| TC-ASPB-29 | 绑定 IM 通道后 Daily Agent 发送处理结论 |

---

## 17. 代码对齐参照

| 现有模块 | 复用方式 |
|----------|----------|
| `AsrDirectoryTask` (`state.rs`) | 新增 `#[serde(default)] pub daily_agent` 字段 |
| `run_directory_task()` (`runner.rs`) | 在 `refresh_task_daily_summaries()` + `update_task_after_run()` 之后插入 hook |
| `ASR_JOB_RUN_LOCK` (`state.rs`) | Daily Agent 使用独立锁，不占用 |
| `RUNNING_TASKS` (`state.rs`) | 新增独立 `DAILY_AGENT_RUNNING_TASKS` |
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

3. **ChatGPT Web 消息长度**：单条消息不超过 30K 字符。`AGENTS.md` 全文 + 初始化说明超出时按段落分片发送。

4. **`appended` 判定**：读取 processed state 中的 `source_len_bytes`，取当前文件前 N bytes 与前次 sha256 比对。如果前缀匹配，remainder 为 tail；否则判定为 `rewritten`。

5. **资源释放顺序**：Daily Agent 排队前确认 ASR managed server / asr 进程 / ffmpeg 子进程均已释放，避免资源竞争。

6. **`AsrDailyAgentProcessedState` 原子写入**：写入临时文件后 rename，避免写入中断导致状态损坏。
