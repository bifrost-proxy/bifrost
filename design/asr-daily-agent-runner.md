# ASR Daily Agent Runner 设计方案

## 背景

ASR 定时任务(`AsrDirectoryTask`)按目录批量转写音频后,会在 `<BIFROST_DATA_DIR>/asr/data/text/<task_id>/.daily/YYYY-MM-DD.md` 生成每日转写汇总 Markdown。这些原始 daily markdown 只是转写产物,不承担业务表达:用户实际想要的是"日报"、"明日 To Do"、"复盘"、"重点摘录"等二次整理。旧方案只把 daily markdown 存盘,用户需要手动喂给外部工具生成日报,链路断裂。

Daily Agent Runner 是 ASR 定时任务的后处理阶段:ASR 音频转写成功、daily markdown 有增量后,自动为该 task 排队一个 Daily Agent 队列,按稳定依赖拓扑把每个 enabled Agent 交给对应 Runner(ChatGPT Web / Bifrost Agent / codex / 自定义)执行,输出 report 到 `.daily/agents/<agent_id>/output/<output_dir>/YYYY-MM-DD-report.md`,可选把同日上游产物注入下游 Agent,可选 IM 通道发送 summary 或 full report,可选把 report 复制到用户指定的外部目录(iCloud、企业网盘)。没有依赖配置时继续保留原数组顺序。

本方案不改动 ASR 音频转写主链路(chunk retry、diarization、daily markdown 生成不变);只在 ASR run terminal 后接一个后处理队列,并在 WebUI/CLI 上提供管理入口。

## 用户目标验证清单

### 必须实现

- `AsrDirectoryTask` 携带 `daily_agent: AsrDailyAgentConfig` 字段;`agents: Vec<AsrDailyAgentItem>` 承载多个 Agent 定义;默认双 Agent `daily_report`(输出到 `report/`) 与 `tomorrow_todo`(输出到 `tomorrow_todo/`,绑定 `owner:feishu-main` 发送 full_report)。
- Workspace 严格布局:`.daily/{.gitignore,.git/,YYYY-MM-DD.md,agents/<agent_id>/{AGENTS.md,input/YYYY-MM-DD.md,output/<output_dir>/YYYY-MM-DD-report.md}}`。Runner cwd 必须是 `.daily/agents/<agent_id>`;`allow_work_dirs` 仅允许 `daily_dir`。
- Agent 标识约束:`id`/`name`/`output_dir` 仅允许 `[A-Za-z0-9_-]`;不同 Agent 有独立 processed key `<agent_id>:<date>`,避免跨 Agent 覆盖。
- ASR run terminal 后调用 `maybe_enqueue_daily_agent_after_asr_run(&updated)`;仅当 daily markdown 有增量(new_file/appended/rewritten)时排队;`unchanged` 跳过。
- 并发控制:全局 `DAILY_AGENT_TASK_LOCKS`(per-task) + `DAILY_AGENT_RUNNING_TASKS`(去重) + `DAILY_AGENT_TASK_CONFIG_LOCK`(配置写);同 task 内多 Agent 串行,不并发抢 ChatGPT Web runner。
- Agent 依赖:`dependencies[{agent_id,include_output}]` 建立稳定 DAG;未知/自/重复/循环依赖保存失败;`dependency_failure_policy=skip|continue` 控制上游失败传播。`include_output=true` 时同日产物挂载到 `input/upstream/<agent_id>/<date>-report.md`,ChatGPT Web 每次消息直接注入正文。
- ChatGPT Web 大输入必须走剪贴板 + `Meta+V/Ctrl+V` 原生粘贴路径,不再按字符数分片;composer 大文本后 ChatGPT 可能上传为文件,输入框无正文属于正常状态;adapter 只轮询发送按钮可用状态。
- ChatGPT Web 契约输出:`daily_report` 必须含 `# YYYY-MM-DD 日报` / `## 今日概览` / `## 证据与不确定性`;`tomorrow_todo` 必须含 `# 明日 To Do List - YYYY-MM-DD` / `## 明天必须完成` / `## 可选推进` / `## 需要确认`;重试续写按 Agent 契约分流。
- ChatGPT Web browser 恢复只恢复与当前 `execution_mode` 一致的 orphan;headed 不复用 headless,反之亦然。
- Admin API 与 CLI 提供列表、增改、run、send、sync、reports 详情等入口;WebUI 提供 Daily Agent 管理列表页 + 单 Agent 详情页 + Daily Docs 行级 Run(All / 单 Agent) + report 全屏详情。
- `report_sync_dir` 自动同步:Runner 成功生成 report 后复制到目标目录,`last_report_sync` 记录 copied/skipped/failed;外部目录超时不影响 report 成功状态。
- Daily Agent Records 数据源:`daily_agent_processed.json` + 磁盘 `.daily/agents/<agent_id>/output/<output_dir>/`;兼容旧路径 `daily/<output_dir>/`、`.daily/agents/<agent_id>/<output_dir>/`、`daily/Report/`;processed state 缺失时仍展示磁盘已有报告。
- IM delivery 单字段 `channel = owner:<provider_id> | target:<target_id>`;`mode=summary|full_report`;超长 full_report 按固定大小拆分为多条,不降级 summary。

### 必须不破坏

- ASR 音频转写主链路(转写成功率、chunk retry、daily markdown 生成)不受影响。
- ASR run terminal 判定、`update_task_after_run` 语义、`repair_interrupted_processing_records_on_startup` 恢复流程不变。
- 全局 `ASR_JOB_RUN_LOCK`(GPU 单例) 不被 Daily Agent 占用;Daily Agent 只用自身 per-task 锁。
- 已有 AGENTS.md 与 conversation state 不被覆盖;git 不可用时不阻塞任务创建与 report 生成。
- Bifrost Agent / codex / 其他 runner 的现有配置(session_key、adapter_config、allow_work_dirs)保留原语义。
- 旧单 Agent 配置(`agent_id`/`name`/`runner`/`output_dir`/`instructions`)保留为兼容镜像;加载时补齐默认 `tomorrow_todo`。

### 必须真实验证

- ASR task 创建时 daily workspace 自动初始化:`.daily/.gitignore`、`agents/<agent_id>/AGENTS.md` 都存在;git 不可用时 `git_available=false` 但不阻塞。
- ASR 音频处理完成后自动触发 Daily Agent 生成 report;git commit best-effort 出现。
- Daily Docs 行级 `Run All Agents` 按行 date 串行运行所有 enabled Agent;单 Agent 下拉只运行指定 agent_id。
- 多 Agent 共用同一 ChatGPT Web runner 时按 task-level 锁串行;单 Agent 失败只写自身 `last_status/last_error/last_run_id`,不阻断队列。
- 手动 Run Now + IM 绑定通道 → 成功发送 summary 或 full_report;超长 full_report 分多条。
- `report_sync_dir` 设置后 report 复制到目标目录;iCloud 卡住时同步失败但 report 生成成功。
- Daily Agent Records 与 report 详情 `/daily-agent/reports/{date}` 数据一致,列表可见的 report 详情必须能打开(不能列表存在、详情 404)。
- 服务重启后 orphan browser 仅恢复与当前 execution_mode 匹配的实例。

## 产品语义

### Daily Agent 队列是 ASR 后处理阶段,不是独立定时任务

Daily Agent 不注册独立 cron;触发点固定为 ASR run terminal + daily markdown 有增量。这样的好处:

1. 用户配置心智:只有"ASR task"这一个调度对象,Daily Agent 是其后处理配置,不需要再管一个调度。
2. 避免与 ASR run 竞争 GPU / conversation runner;Daily Agent per-task 锁独立于 ASR 全局锁。
3. `unchanged` daily markdown 不触发 Runner,避免空转和 IM 打扰。

用户仍可通过 API/CLI/WebUI `Run Now` 手动触发,不依赖 ASR。

### 每个 Agent 有独立 workspace,不共享 conversation 与 output

- 独立 `AGENTS.md`(runner cwd 指向该目录)。
- 独立 `input/YYYY-MM-DD.md`(daily markdown 副本,避免 Agent 误改原文件)。
- 独立 `output/<output_dir>/YYYY-MM-DD-report.md`。
- 独立 `session_key`(默认包含 `task_id + agent_id`),ChatGPT Web conversation 不跨 Agent 复用。
- 独立 processed key `<agent_id>:<date>`,Records 不互相覆盖。

这样"日报 Agent"与"明日 To Do Agent"可以有完全不同的 prompt 契约、runner、IM 通道、失败恢复,不会因共享资源污染彼此。

### ChatGPT Web 契约输出与恢复策略

不同 Agent 输出 heading 契约不同,续写/纠偏必须按契约分流:

- `daily_report`:`# YYYY-MM-DD 日报` / `## 今日概览` / `## 证据与不确定性`。
- `tomorrow_todo`:`# 明日 To Do List - YYYY-MM-DD` / `## 明天必须完成` / `## 可选推进` / `## 需要确认`。

adapter 不允许把 `tomorrow_todo` 输出误导为日报格式。ChatGPT Web browser 重启后仅恢复与当前 `execution_mode` 一致的 orphan;headless/headed 严格隔离,防止在错误进程里粘贴大文本导致 CDP 卡死或跨模式污染。

### `report_sync_dir` 与 IM 是两条独立分发通道

- `report_sync_dir`:本地目录复制,面向 iCloud、企业网盘等异步同步;失败只影响同步状态,不影响 report 生成成功。
- IM `im_delivery.channel`:主动推送到飞书 owner/target;`mode=summary` 只发摘要,`mode=full_report` 发完整 report(超长按固定大小拆多条)。

两条通道解耦,便于用户按 Agent 独立配置。

## 技术细节

### 关键模块

- `crates/bifrost-admin/src/handlers/asr_jobs/state.rs`:`AsrDirectoryTask.daily_agent`;`DAILY_AGENT_TASK_LOCKS`、`DAILY_AGENT_RUNNING_TASKS`、`DAILY_AGENT_TASK_CONFIG_LOCK`;`TaskRunFileLock("daily-agent:<task_id>")`。
- `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_config.rs`:`AsrDailyAgentConfig`、`AsrDailyAgentItem`、`AsrDailyAgentImDeliveryConfig`、枚举 `AsrDailyAgentTriggerPolicy` / `AsrDailyAgentInstructionsSource`、`AsrDailyAgentReportSyncResult`。
- `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_workspace.rs`:`ensure_asr_daily_workspace`、`text_output_dir` 拓展、AGENTS.md 分发、git init(best-effort)。
- `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs`:队列 loop、`maybe_enqueue_daily_agent_after_asr_run`、ChangePlanner(`new_file` / `appended` / `rewritten` / `unchanged`)、Runner 执行、per-Agent 结果聚合、IM 发送、report_sync 复制。
- `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_prompt.rs`:按 Agent 契约生成 system + user prompt;ChatGPT Web 走大输入路径。
- `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_api.rs`:HTTP handlers。
- `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_records.rs`:合并 processed json + 磁盘扫描的兜底发现;兼容旧路径。
- `crates/bifrost-admin/src/handlers/asr_jobs/runner.rs` / `retry.rs`:`update_task_after_run` 之后调用 hook。
- `crates/bifrost-cli/src/commands/asr/daily.rs`:CLI 子命令 `bifrost asr daily {list|show|run|send|sync|set-sync-dir|edit-agent}`。
- `web/src/pages/AI/asr/daily-agent/`:WebUI 管理列表 + Agent 详情 + Daily Docs 行级 Run + report 全屏详情。

### Workspace 布局

```text
<BIFROST_DATA_DIR>/asr/data/text/<task_id>/.daily/
├── .gitignore                       # 排除 .DS_Store
├── .git/                            # best-effort
├── YYYY-MM-DD.md                    # ASR 生成的每日转写(只读)
├── daily_agent_processed.json       # per-agent processed state
└── agents/
    └── <agent_id>/
        ├── AGENTS.md                # Runner cwd 指向该目录
        ├── input/YYYY-MM-DD.md      # daily markdown 副本
        ├── input/upstream/          # 显式依赖的同日产物
        │   └── <upstream_agent_id>/YYYY-MM-DD-report.md
        └── output/<output_dir>/YYYY-MM-DD-report.md
```

Runner `work_dir = <task>/.daily/agents/<agent_id>`;`allow_work_dirs = [<daily_dir>]`;禁止 Runner 逸出。

### 并发锁

- `DAILY_AGENT_TASK_LOCKS: Mutex<HashMap<TaskId, Arc<Mutex<()>>>>`:per-task 串行,同一 task 的多 Agent 串行执行。
- `DAILY_AGENT_RUNNING_TASKS: Mutex<HashSet<TaskId>>`:去重,已在运行的 task 不重复排队。
- `DAILY_AGENT_TASK_CONFIG_LOCK: Mutex<HashMap<TaskId, Arc<Mutex<()>>>>`:配置读写。
- `TaskRunFileLock("daily-agent:<task_id>")`:进程重启后跨进程恢复保护。
- 与 `ASR_JOB_RUN_LOCK` 完全独立,不共享。

### ChangePlanner

对比 `daily_agent_processed.json` 中记录的 `<agent_id>:<date>` 与磁盘 `daily_dir/YYYY-MM-DD.md`,输出四种变更:

- `new_file`:首次出现,发送全文。
- `appended`:size 增长 + 前缀 hash 相同,只发 tail 字节区间。
- `rewritten`:hash 变化且非 append,发送全文并标记覆盖。
- `unchanged`:hash 相同,skip。

ChatGPT Web plan 不包含 `unchanged`;Bifrost Agent plan 不塞 daily markdown 全文(只传路径/摘要,减少 token)。

### Runner 触发

`maybe_enqueue_daily_agent_after_asr_run(&updated_task)`:

1. 若 daily_agent.enabled = false → skip。
2. 若 `agents` 为空 → 用 legacy 单 Agent 迁移。
3. 加 `DAILY_AGENT_RUNNING_TASKS`(已存在则 skip)。
4. 遍历 enabled Agent,对每个 Agent 计算 change plan,若非 unchanged 则加入队列。
5. spawn task-level queue runner,持 `DAILY_AGENT_TASK_LOCKS[task_id]`,串行执行。

### Runner 消息组织

统一走 `ExternalCliRunRequest`(runner=chatgpt_web / bifrost_agent / codex / custom):

- `session_key`:默认 `daily-agent:<task_id>:<agent_id>`。
- `work_dir`:`.daily/agents/<agent_id>`。
- `allow_work_dirs`:`[daily_dir]`。
- `input`:按 ChatGPT Web / Bifrost Agent / codex 三分支组织;ChatGPT Web 走剪贴板大输入路径。
- `adapter_config`:透传 conversation ref、execution_mode。

### IM 发送

- `channel = owner:<provider_id>` → 通过 owner provider 找默认 target。
- `channel = target:<target_id>` → 直接向 target 发。
- `mode = summary`:发送 report 前 N 行摘要或 prompt-derived summary。
- `mode = full_report`:发送原始 report;超过 IM 平台上限时按固定字节数拆分为多条,不降级 summary。
- 未绑定 IM 或 `enabled=false` → 记录 `im: skipped`。

## CLI 与 Admin API

### CLI

- `bifrost asr daily list` / `show <task>` / `run <task> [--date] [--agent] [--force]` / `send <task> --date <d>` / `sync <task>` / `set-sync-dir <task> <path>` / `edit-agent <task> <agent_id>`。
- `daily sync` 输出 `target/total/copied/skipped/failed`。
- `daily set-sync-dir` 支持传空字符串清除。

### Admin API

- `GET /_bifrost/api/asr/tasks/{id}/daily-agent`:配置 + last run 状态 + processed 概览。
- `GET/PUT /_bifrost/api/asr/tasks/{id}/daily-agent/agents`:agent 列表增改。
- `POST /_bifrost/api/asr/tasks/{id}/daily-agent/run`:body `{date?, agent_id?, force?}`;省略 `agent_id` = 全部;省略 `date` = 最新 daily。
- `POST /_bifrost/api/asr/tasks/{id}/daily-agent/send`:重新触发 IM 发送。
- `POST /_bifrost/api/asr/tasks/{id}/daily-agent/sync`:手动同步全部现有 report 到 `report_sync_dir`。
- `GET /_bifrost/api/asr/tasks/{id}/daily-agent/reports/{date}`:返回 report Markdown 全文;非法日期拒绝,缺失 404。
- `GET /_bifrost/api/asr/tasks/{id}/daily-agent/runs`:合并 processed json + 磁盘 fallback,按 date 倒序。

### WebUI

- ASR 创建/编辑页:Daily Agent 开关 + Runner 单字段下拉(复用 AI → Agent → Runners)+ IM Channel 单字段下拉(owner + target 列表)+ terminology + report_sync_dir。
- Task Detail → Daily Agent tab:Agent 管理列表(任务级启用 / Report Sync / Refresh / Add / Run / Edit / Delete),单 Agent 详情页承载 Runner / Trigger / Timeout / Session Key / Output Dir / IM Delivery / Last Run Status / Instructions。
- Task Detail → Daily Docs tab:每行 `Run All Agents` 主按钮 + 单 Agent 下拉;URL 参数 `asrTaskTab` / `asrDailyReport` 恢复;窄窗口横向滚动限制在表格内。
- Report 全屏详情:Markdown 渲染,不嵌套纵向滚动。
- ASR 顶层 tab 英文文案:`Scheduled Tasks` / `ASR Management` / `Voiceprint & Wake`。

## Sync / 导入导出 / 分享边界

- Daily Agent 配置属于 `AsrDirectoryTask` 的一部分,随任务导入导出;不参与 rule sync / group sync / rule share URL。
- Workspace `.daily/*` 与 `daily_agent_processed.json` 属于本机衍生数据,不同步。
- `report_sync_dir` 是本机路径,导入到另一台机器时应清空或让用户重新指定。
- IM `channel` 依赖本地 Provider Owner / Target 表;跨设备迁移时应做 channel 校验,缺失则记录 skipped。

## 实现切分

### Phase 1:数据模型 + Workspace + 基础队列

- `AsrDirectoryTask.daily_agent`;`AsrDailyAgentConfig` / `AsrDailyAgentItem`;默认双 Agent(`daily_report` + `tomorrow_todo`)。
- `ensure_asr_daily_workspace`:创建 `.daily/`、agents/<id>/AGENTS.md、input、output;`.gitignore`;git init(best-effort)。
- 队列 loop 骨架 + per-task 锁;`unchanged` skip。

### Phase 2:Runner 集成 + ChangePlanner + processed state

- ChangePlanner 四态判定;processed key `<agent_id>:<date>`。
- `ExternalCliRunRequest` 组装;ChatGPT Web 大输入剪贴板路径;Bifrost Agent / codex 直调。
- ASR terminal hook `maybe_enqueue_daily_agent_after_asr_run` 接入 `runner.rs` + `retry.rs`。

### Phase 3:IM + report_sync + API + CLI

- IM channel 单字段解析 + summary/full_report 拆分。
- `report_sync_dir` 自动/手动同步;`last_report_sync` 状态。
- Admin API 全部 endpoint;CLI `bifrost asr daily *` 子命令。

### Phase 4:WebUI + 恢复 + Records + human_tests

- Daily Agent tab 列表 + 详情页;Daily Docs 行级 Run;Report 全屏详情;URL 参数恢复;窄窗口布局。
- Records 合并 processed + 磁盘 fallback;兼容旧路径。
- ChatGPT Web orphan browser 按 execution_mode 隔离恢复。
- `human_tests/asr-daily-agent-runner.md` + `webui-asr-daily-agent-*.md` 全部执行。

## 测试方案

### 单元测试

- 默认配置反序列化 → 双 Agent。
- Legacy 单 Agent 配置 → 保留 legacy 字段 + 补 `tomorrow_todo`。
- `ensure_asr_daily_workspace` → 创建 `.daily/`、`agents/<id>/AGENTS.md`、`input/`、`output/<output_dir>/`、`.gitignore`,分发 daily markdown 副本。
- 已存在 AGENTS.md 不覆盖;git 不存在返回 `git_available=false`。
- `PUT /daily-agent/agents` 保存 custom instructions + 写文件。
- `maybe_enqueue_daily_agent_after_asr_run` 只在 ASR terminal + daily 刷新后排队。
- ChangePlanner 四态(new_file / appended / rewritten / unchanged) + byte range 正确。
- Runner 成功 → 更新 processed state;失败 → 不更新;同日多 Agent processed key 不覆盖。
- ChatGPT Web plan 不含 unchanged;Bifrost Agent plan 不塞 daily markdown 全文。
- Active run 不重复启动;work_dir 只允许 daily_dir。
- IM channel 解析 `owner:xxx` / `target:xxx`;full_report 超长拆多条。

### E2E / API

覆盖场景(节选,完整列表见 `human_tests`):

- 创建 ASR task → daily workspace 自动存在。
- 启用 Daily Agent → WebUI 单字段 Runner 下拉保存 `runner/trigger_policy/timeout`。
- ASR run 完成 → 音频处理完再看到 Daily Agent queued;仍在 processing 时不启动。
- daily markdown unchanged → skipped;追加内容后 ChatGPT Web 只收新增 tail。
- 手动 Run Now → `report/` 生成文件。
- Daily Docs 行级 `Run All Agents` → 请求只带 `date`,后端串行运行全部 enabled Agent;单 Agent 下拉 → 带 `agent_id` 只运行指定 Agent。
- `report_sync_dir` 自动同步 + 手动同步;iCloud 卡住时同步失败但 report 成功;`last_report_sync` 记录 copied/skipped/failed。
- CLI `daily set-sync-dir` / `daily sync` 输出 target/total/copied/skipped/failed。
- `/daily-agent/reports/{date}` 返回 report Markdown 全文;非法日期拒绝;缺失 404。
- `/daily-agent/runs` 合并 processed + 磁盘 fallback,兼容旧路径,按日期倒序。
- 自动 completion hook → run detail 记录 `trigger_source=asr_completion`。
- 未绑定 IM → 不发送,记录 skipped;git 不存在 → 创建任务 + 保存 AGENTS.md 仍成功。

### WebUI

Playwright / 真实浏览器覆盖:创建页 Runner 单字段下拉、IM Channel 单字段下拉、trigger 说明、instructions editor 默认可见;Daily Agent tab 列表与 Agent 详情页分离(列表只保留管理动作,详情承载配置);Daily Docs 行级 `Run All Agents` + 单 Agent 下拉与 `Open document` 共存,URL 参数恢复;Records 支持 Agent / Date / Runner 三维筛选;report 全屏 Markdown 详情不嵌套纵向滚动;窄窗口 Daily Agent 列表与 Records 表格横向滚动限制在表格内;亮/暗主题均可读。

### human_tests

`human_tests/asr-daily-agent-runner.md` + `human_tests/webui-asr-daily-agent-*.md` 维护以下用例并逐条真实执行:

| 用例编号 | 名称 |
|----------|------|
| TC-ASPB-25 | Daily Agent Runner 方案文档验收 |
| TC-ASPB-26 | ASR task 创建时初始化 daily workspace |
| TC-ASPB-27 | WebUI 配置 Runner 并编辑 AGENTS.md |
| TC-ASPB-28 | ASR 完成后自动触发 Daily Agent 生成 report 并写 Git 历史 |
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

`human_tests/readme.md` 同步索引与总数。真实执行使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_DATA_DIR=$(mktemp -d)`、`--no-system-proxy`、非 9900 端口。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin asr_jobs::daily_agent`
- `cargo test -p bifrost-cli asr::daily`
- `pnpm --dir web test:ui asr-daily-agent*.spec.ts`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 时不跑 `make coverage`;交付说明豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标:workspace 布局、per-Agent 隔离、ChangePlanner 四态、并发锁矩阵、ChatGPT Web 大输入路径与 orphan 恢复、IM 单字段 channel。
- 复核 diff:`git status --short` / `git diff` 覆盖 `asr_jobs/daily_agent*`、`runner.rs`/`retry.rs` hook、CLI `daily.rs`、WebUI Daily Agent 页面、human_tests。
- 重点 review:agent_id 字符集校验;`allow_work_dirs` 只含 daily_dir;`session_key` 含 `task_id + agent_id`;Records 数据源与 report 详情读同一路径。
- 复测:定向单测 + 全量 API E2E + Playwright UI。

### 第 2 轮

- 复查第 1 轮修复:多 Agent 串行时锁未泄漏;ChatGPT Web headed/headless 恢复严格隔离;report_sync 异步失败不阻塞 report 成功。
- 再次 `git status --short` / `git diff`,确认没有遗漏 WebUI 布局或 URL 参数恢复分支。
- 复跑受影响单测、E2E、human_tests;新问题追加轮次。

## 风险与决策点

- ChatGPT Web 大输入必须走剪贴板 + `Meta+V/Ctrl+V`,不再按字符数分片;composer 上传为文件后输入框无正文属正常状态,adapter 只轮询发送按钮可用,不做 head/tail/长度采样。
- `ASR_JOB_RUN_LOCK` 是 GPU 单例,Daily Agent 不占用;新增 `DAILY_AGENT_*` 独立锁,避免拖慢 ASR 音频处理。
- 多 Agent 共用同一 ChatGPT Web runner 时按 task 级串行,牺牲吞吐换 conversation 隔离;跨 task 仍可并行。
- `report_sync_dir` 指向 iCloud/外部目录时可能卡住;必须超时后只写 sync 错误,不影响 report 成功状态和其他 API 响应。
- Records 数据源双通道(processed json + 磁盘 fallback):必须保证列表可见的 report 详情能打开;禁止列表存在、详情 404 的不一致。
- Agent 标识字符集限制 `[A-Za-z0-9_-]`:避免跨平台路径与 URL 参数歧义;历史 task 若含非法字符需在加载时清洗并写日志。
- Legacy 单 Agent 兼容镜像:短期内保留;若未来彻底下线,需 migration + human_tests 覆盖。
- git 不可用是常见部署形态(容器、只读镜像):必须 best-effort;不能阻塞任务创建或 report 生成。
