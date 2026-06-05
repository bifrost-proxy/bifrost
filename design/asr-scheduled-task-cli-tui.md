# ASR 定时任务 CLI TUI 方案

## 背景与目标

ASR Directory Task 已经支持按 `schedule` 定时扫描音频目录并执行转写。当前查看任务进展主要依赖 WebUI ASR 页面，长时间观察时会额外占用浏览器内存和 CPU。新增 CLI TUI 的目标是：在不打开浏览器的情况下，用轻量终端界面持续观察 ASR 定时任务的执行状态、执行进展和消耗信息。

用户目标验证清单：

- 必须实现：提供 ASR 定时任务观察用 CLI TUI。
- 必须实现：展示任务状态、下一次运行时间、当前运行进度、当前文件/分片进度、失败/部分成功信息。
- 必须实现：展示消耗信息，包括已处理音频时长、推理耗时、RTF、处理文件大小、文本字符数、失败分片数，以及可用时的 ASR 服务/进程信息。
- 必须实现：只有一个 ASR 定时任务时，执行命令后自动进入该任务详情 TUI。
- 必须实现：多个 ASR 定时任务时，支持传入任务参数直接进入，也支持交互式选择。
- 必须不破坏：现有 `bifrost ai asr task list/show/files/run/daily` 输出和脚本兼容性。
- 必须不破坏：ASR scheduler、pause/resume、retry chunks、Daily Docs、外接设备导入链路。
- 必须真实验证：CLI TUI 在真实 `bifrost start --no-system-proxy` 服务上可进入、刷新、退出，并能在单任务/多任务场景正确选择。
- 必须交付：设计文档、CLI/API 单元测试、E2E、human_tests 用例和两轮 Review/Fix/Test 闭环。

## 本轮自查结论

这版方案需要补强的点：

- 原方案建议的 `GET /api/asr/tasks/watch` 和现有 `GET /api/asr/tasks/{task_id}` 路由形态相近，容易在路由顺序或保留字上踩坑；应改成带哨兵段的集合 watch API。
- 原方案默认可从 `GET /api/asr/tasks` 做选择，但现有任务摘要可能触发目录扫描；TUI 高频刷新不能依赖会扫描音频目录的接口。
- 原方案把 `progress_current/progress_total` 同时用于文件级和 chunk 级展示，但当前 runner 在文件刚开始时写的是本轮文件进度，chunk 回调后又写 chunk 进度；watch API 必须归一化语义，不能让 UI 猜。
- 原方案的消耗信息没有区分“事实值”和“估算值”；RTF、ETA 和 source bytes 必须标注来源，避免用户误读。
- 原方案没有明确终端能力降级、宽窄屏布局、任务选择的模糊匹配规则，以及 TUI 快捷键执行写操作时的确认边界。

下面的方案已按这些问题补强。

## 当前代码基线

- CLI ASR 入口在 `crates/bifrost-cli/src/commands/asr.rs`，命令定义在 `crates/bifrost-cli/src/cli.rs` 的 `AiAsrTaskCommands`。
- 现有任务命令已经通过 `AsrTaskClient` 调用 `GET /_bifrost/api/asr/tasks`、`GET /_bifrost/api/asr/tasks/{task_id}` 和 `POST /_bifrost/api/asr/tasks/{task_id}/run`。
- 现有 CLI 已引入 `ratatui`、`crossterm`、`dialoguer`，并有 `crates/bifrost-cli/src/commands/status_tui.rs` 可复用终端生命周期、按键处理、布局和低频刷新模式。
- ASR 任务模型在 `crates/bifrost-admin/src/handlers/asr_jobs/state.rs`。`FileRecord` 已包含 `status`、`source_size`、`media_duration_ms`、`text_chars`、`chunk_metrics`、`started_at_ms`、`finished_at_ms`、`progress_current`、`progress_total`、`failed_chunks`。
- 当前 `GET /api/asr/tasks/{id}` 返回详情和文件列表，可以支撑基础 TUI；但长期每秒拉全量文件列表会放大 JSON 解析和文件扫描成本。

## 命令设计

新增命令放在现有 ASR task 命名空间下。主命令建议用 `watch`，`tui` 作为更直观的别名：

```bash
bifrost ai asr task watch [task]
bifrost ai asr task tui [task]        # watch 的别名
```

参数：

```text
[task]                    可选。支持完整 task id、唯一 id 前缀、唯一任务名。
--refresh-ms <ms>         默认 1000，最小 500。
--no-interactive-select   多任务且未传 task 时直接报错，适合脚本/非 TTY。
--all                     进入多任务总览，不自动选择单个任务。
--json-snapshot           打印一次轻量快照 JSON 后退出，便于调试 API 聚合结果。
--read-only               禁用 R/p 等写操作快捷键，只观察。
```

选择规则：

```text
启动
  -> GET /api/asr/tasks
  -> tasks.len == 0: 打印 "No ASR directory tasks." 后退出
  -> tasks.len == 1 && 未传 task && 未传 --all: 自动进入唯一任务
  -> 已传 task: 按完整 id -> 唯一 id 前缀 -> 唯一名称解析；不存在或多重匹配则报错并列出候选
  -> tasks.len > 1 && TTY && 未传 --no-interactive-select: 打开交互式选择
  -> tasks.len > 1 && 非 TTY: 报错提示传 task 或 --all
```

写操作边界：

- 默认 TUI 允许快捷键 `R` 手动运行、`p` 暂停/恢复、`P` 强制暂停。运行中再次按 `R` 只提示任务已在运行，不再把 409 API 响应暴露成误导性的 `Config error`。
- `--read-only` 下隐藏写操作快捷键，按下也只提示当前为只读模式。
- 非 TTY 或 `--json-snapshot` 模式不执行任何交互写操作。

## 交互示意图

### 多任务选择

```text
? Select ASR task to watch
  > 01  Meeting Recorder       running    12/20 done   next 2026-05-24 02:00
    02  Daily Voice Notes      enabled     8/8 done    next 2026-05-24 22:30
    03  Device Import Inbox    paused      4/9 done    next -

Enter: open   ↑/↓: move   /: filter   Esc/q: quit
```

这里优先用 `dialoguer::Select` 实现，和现有 CLI 交互依赖保持一致；后续如果希望选择页也全屏化，再迁移到 ratatui。

### 单任务详情 TUI

```text
┌ ASR Task: Meeting Recorder (running) ───────────────────────────────────────┐
│ ID 8b83e9d2...  Model Qwen3-ASR-1.7B  Runtime reuse_per_file  Schedule daily │
│ Last 2026-05-24 09:12:03  Next 2026-05-25 02:00:00  Refresh 1.0s  API ok     │
├ Progress ───────────────────────────────────────────────────────────────────┤
│ Files  [██████████████████████░░░░░░] 12/20  60%   Pending 7  Failed 1       │
│ Chunk  [████████████░░░░░░░░░░░░░░░░] 18/45  40%   Current 2026-05-24/A.m4a  │
│ State  processing file 3/8 in this run; last update 2s ago                  │
├ Consumption ────────────────────────────┬ Current File ──────────────────────┤
│ Audio processed     02:18:44            │ 2026-05-24/A.m4a                   │
│ Inference elapsed   00:29:10            │ size 184.2 MB  duration 00:47:31   │
│ Average RTF         0.21  (4.8x real)   │ chunks 18/45  runner asr-server    │
│ Last chunk RTF      0.24                │ text chars 18,420                  │
│ Source bytes done   1.7 GB / 2.9 GB     │ started 09:36:20                   │
│ Text chars          94,120              │ eta 00:17:40                       │
├ Recent Files ────────────────────────────────────────────────────────────────┤
│ ✓ 2026-05-23/part-01.m4a       00:31:20  118 MB  RTF 0.19  12,048 chars      │
│ ◐ 2026-05-24/A.m4a             00:47:31  184 MB  18/45 chunks                │
│ ✕ 2026-05-24/B.m4a             00:12:09   42 MB  normalize failed            │
└ q quit  r refresh  R run now  p pause/resume  P force pause  Tab files/docs ┘
```

### 多任务总览可选页

`--all` 进入总览，只做观察，不自动切到某个任务：

```text
┌ ASR Scheduled Tasks ─────────────────────────────────────────────────────────┐
│ NAME                  STATE      PROGRESS        NEXT RUN             COST   │
│ Meeting Recorder      running    12/20 60%       2026-05-25 02:00    0.21rtf│
│ Daily Voice Notes     enabled     8/8 100%       2026-05-24 22:30    0.18rtf│
│ Device Import Inbox   paused      4/9 44%        -                   -      │
└ Enter open  ↑/↓ move  r refresh  q quit ────────────────────────────────────┘
```

### 窄屏与非 Unicode 降级

- 终端宽度小于 90 列时，隐藏 `Recent Files`，只保留 `Progress`、`Consumption` 和当前文件摘要。
- 终端宽度小于 70 列时，改为单列布局。
- 如果检测到 `NO_COLOR`、`TERM=dumb` 或 Unicode 宽度异常，进度条降级为 ASCII：`[########------]`，状态符号从 `✓/◐/✕` 降级为 `ok/run/err`。
- 截断路径必须保留文件名和末尾目录，例如 `.../2026-05-24/A.m4a`。

## 数据与 API 设计

### V1 先复用现有 API

为了尽快落地，TUI 初版可以先复用：

- `GET /api/asr/tasks`：选择任务、单任务自动进入、多任务总览。
- `GET /api/asr/tasks/{task_id}`：详情页刷新。
- `POST /api/asr/tasks/{task_id}/run`：快捷键 `R` 触发手动运行。
- `POST /api/asr/tasks/{task_id}/pause|resume`：快捷键 `p` 切换暂停状态；快捷键 `P` 调用 `pause?mode=long_term&force=true`，对齐 WebUI Force Pause 能力。

这个路径最小化服务端改动，但长期刷新有两个问题：

1. 任务详情会携带所有 `files` 和 `daily_documents`，TUI 高频刷新不够轻。
2. 消耗信息需要 CLI 遍历文件和 chunk 指标临时聚合，逻辑会散落在 CLI。

### V1 推荐新增轻量快照 API

推荐新增：

```http
GET /api/asr/tasks/{task_id}/watch
GET /api/asr/tasks/-/watch
```

`GET /api/asr/tasks/{task_id}/watch` 返回单任务轻量快照：

```json
{
  "task": {
    "id": "8b83e9d2...",
    "name": "Meeting Recorder",
    "enabled": true,
    "paused": false,
    "running": true,
    "schedule": { "kind": "daily", "hour": 2, "minute": 0 },
    "last_run_at_ms": 1779575523000,
    "next_run_at_ms": 1779636000000,
    "model": "Qwen3-ASR-1.7B",
    "language": "chinese",
    "runtime_strategy": "reuse_per_file"
  },
  "progress": {
    "discovered": 20,
    "processed": 12,
    "pending": 7,
    "failed": 1,
    "partial_success": 0,
    "failed_chunk_count": 2,
    "file_percent": 60.0,
    "current_file_key": "abc",
    "current_source_path": "/Recordings/2026-05-24/A.m4a",
    "current_file_index": 3,
    "current_file_total": 8,
    "current_chunk_done": 18,
    "current_chunk_total": 45,
    "eta_ms": 1060000
  },
  "consumption": {
    "source_bytes_total": 2919230105,
    "source_bytes_processed": 1701823010,
    "audio_duration_ms_total": 11524000,
    "audio_duration_ms_processed": 8324000,
    "inference_elapsed_ms": 1750000,
    "average_rtf": 0.21,
    "last_chunk_rtf": 0.24,
    "text_chars": 94120,
    "chunks_completed": 232,
    "chunks_failed": 2
  },
  "service": {
    "managed": true,
    "server_url": "http://127.0.0.1:57321",
    "pid": 12345,
    "owner_module": "directory_task",
    "owner_id": "8b83e9d2..."
  },
  "recent_files": [
    {
      "source_path": "/Recordings/2026-05-24/A.m4a",
      "status": "processing",
      "source_size": 193147392,
      "media_duration_ms": 2851000,
      "progress_current": 18,
      "progress_total": 45,
      "text_chars": 18420,
      "last_chunk_rtf": 0.24,
      "error": null
    }
  ],
  "last_error": null,
  "updated_at_ms": 1779577584000
}
```

`GET /api/asr/tasks/-/watch` 返回所有任务的轻量摘要，供选择页和 `--all` 使用。使用 `-` 哨兵段避免和 `{task_id}` 产生保留字冲突。路由实现必须把这两个 watch 分支放在通用 `GET /api/asr/tasks/{task_id}` 之前。

推荐路由顺序：

```rust
(&Method::GET, "/api/asr/tasks/-/watch") => list_task_watch_response(),
(&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/watch") => {
    // parse /api/asr/tasks/{task_id}/watch
}
// existing generic GET /api/asr/tasks/{task_id} stays after watch routes
```

watch API 的硬约束：

- 不做 `discover_audio_files()` 全目录扫描；只读取 task store、file store、运行中进度快照和 ASR service state。
- 不返回完整 `files` 和完整 `daily_documents`；`recent_files` 默认最多 8 条，`failed_files` 由独立分页参数后续扩展。
- 聚合逻辑放在 Admin API，CLI 只负责展示，避免多个 TUI 进程重复遍历和计算。
- 每次响应带 `snapshot_source`：`live_progress`、`file_store`、`stale_recovered` 或 `legacy_detail`，TUI 在状态栏展示来源。

### 聚合规则

- `file_percent = (processed + failed + partial_success) / discovered`，`discovered == 0` 时显示 `0%` 或 `idle`。
- 当前文件优先选择 `status=processing` 的文件；没有时取最近 `finished_at_ms` 最大的文件。
- 当前 chunk 不能直接盲信 `progress_current/progress_total`。watch API 必须输出显式字段：`current_file_index/current_file_total` 和 `current_chunk_done/current_chunk_total`。如果底层只有旧 `progress_current/progress_total`，只有在当前文件 `status=processing` 且已有 `chunk_metrics` 或 runner chunk 回调证据时，才把它解释为 chunk 进度；否则只展示文件级进度。
- `audio_duration_ms_processed` 聚合 `success/partial_success` 文件的 `media_duration_ms`，当前 processing 文件可按 chunk 比例估算。
- `inference_elapsed_ms` 聚合所有 `chunk_metrics.elapsed_ms`，当前 chunk 未完成时不猜测。
- `average_rtf = inference_elapsed_ms / audio_duration_ms_processed`；没有音频时长时显示 `-`。
- `eta_ms` 只在 `running` 且有稳定 `average_rtf`、剩余音频时长或 chunk 比例时展示；否则显示 `calculating`。返回中必须带 `eta_confidence`：`none`、`low`、`medium`。
- `source_bytes_processed` 聚合 `success/partial_success/failed` 文件大小，当前 processing 文件可按 chunk 比例估算。

### 运行进度快照

为了避免把 `FileRecord` 同时当作“任务运行状态”和“文件结果记录”，建议新增轻量运行进度文件：

```text
<BIFROST_DATA_DIR>/asr/tasks/<task_id>/run_progress.json
```

结构：

```json
{
  "run_id": "1779575523000-8b83e9d2",
  "trigger": "schedule",
  "status": "running",
  "started_at_ms": 1779575523000,
  "updated_at_ms": 1779577584000,
  "current_source_path": "/Recordings/2026-05-24/A.m4a",
  "current_file_index": 3,
  "current_file_total": 8,
  "current_chunk_done": 18,
  "current_chunk_total": 45,
  "processed_now": 2,
  "failed_now": 0,
  "message": "processing chunk 18/45"
}
```

runner 在以下时机更新：

- 发现 pending 列表后写入 `current_file_total`。
- 每个文件开始时更新 `current_file_index/current_source_path`。
- chunk 回调更新 `current_chunk_done/current_chunk_total`。
- pause、failed、completed 时写入终态和 `finished_at_ms`。

服务重启时如果 `run_progress.status=running` 但 `task_is_running=false`，watch API 返回 `stale_recovered` 并用现有恢复逻辑重新入队或显示可恢复错误。

### 消耗信息可信度

TUI 展示消耗信息时按可信度分层：

- 事实值：已经完成 chunk 的 `elapsed_ms`、`rtf`、`text_chars`、已完成文件的 `source_size`、`media_duration_ms`。
- 估算值：当前文件按 chunk 比例折算的 source bytes、当前文件剩余时长、ETA。
- 不展示猜测值：未完成 chunk 的耗时、缺失 duration 的 RTF、没有历史样本时的 ETA。

JSON 字段对估算值使用 `_estimated` 后缀或配套 `*_confidence`，例如 `source_bytes_processed_estimated`、`eta_confidence`。TUI 文案用 `est.` 标识估算。

## TUI 实现结构

建议新增文件：

```text
crates/bifrost-cli/src/commands/asr_tui.rs
```

职责拆分：

- `selection`：任务发现、单任务自动进入、多任务选择、非 TTY 错误。
- `client`：复用或提取 `AsrTaskClient`，提供 `list_watch_snapshots()`、`get_watch_snapshot()`、`run_task()`、`pause_task()`、`resume_task()`。
- `model`：CLI 侧反序列化结构和格式化函数，单元测试覆盖 RTF/ETA/百分比。
- `app`：ratatui App 状态、刷新节流、按键状态。
- `render`：布局、表格、gauge、帮助弹窗。

如果 `asr.rs` 已接近文件行数上限，不继续把 TUI 放进 `asr.rs`，只保留命令分发调用，避免单文件继续膨胀。

终端生命周期复用 `status_tui.rs` 模式：

- `enable_raw_mode()` + `EnterAlternateScreen`
- panic/错误路径恢复 raw mode 和 alternate screen
- `q/Esc/Ctrl-C` 退出
- 网络失败时保留上一帧，状态栏显示 `API disconnected; retrying`

此外需要一个 RAII guard 包住 raw mode 和 alternate screen，确保任意 `?` help、确认弹窗、API 错误、Ctrl-C 或 panic unwind 后都恢复终端。

## 低资源策略

- 默认刷新间隔 1 秒，失败时指数退避到 5 秒。
- 详情 TUI 使用轻量 watch API；如果 API 不存在，临时降级到现有详情 API，但状态栏提示 `legacy snapshot`。
- 总览页只拉任务摘要，不拉每个任务完整 `files`。
- 最近文件列表最多 8 条；失败列表按快捷键 `f` 单独查看，避免持续渲染大量记录。
- 不订阅 WebUI push，不启动浏览器，不读取 Traffic 大聚合接口。
- CLI 只做展示聚合；文件扫描和 chunk 指标聚合尽量放在 Admin API，避免多开 TUI 时重复重算。
- 手动刷新 `r` 可以立即请求一次，但连续触发需要 300ms 防抖。
- `--all` 总览页默认 2 秒刷新一次；进入单任务详情后才使用 1 秒刷新。

## 错误与边界

- 服务未启动：提示 `Start the proxy with: bifrost start --no-system-proxy`，不进入空 TUI。
- 无任务：提示用 WebUI 或 API 创建 ASR Directory Task。
- 多任务非交互：提示传 `task` 或 `--all`。
- 任务运行中被暂停：状态改为 `paused`，进度保留最后快照，显示 `released compute`。
- 任务运行锁遗留：沿用现有 scheduler 恢复逻辑；TUI 只展示 `last_error` 和 `run.lock` 归一化后的状态。
- ASR service 被其他 owner 占用：在 `service` 区展示 owner/module/model，任务错误区显示冲突信息。
- `run_progress.json` 损坏：watch API 忽略损坏快照并返回 `snapshot_source=file_store`，同时在 `warnings` 中返回解析错误摘要。
- 任务名重复：命令行名称选择必须报 `ambiguous task name`，要求使用 id 或 id 前缀。
- ID 前缀重复：报 `ambiguous task id prefix` 并列出匹配任务。
- 终端不支持 raw mode：降级为 `--json-snapshot` 风格的一次性文本快照，或提示用户改用普通 `list/show/files` 命令。

## 测试方案

### 单元测试

- `select_target_task_single_auto_enters`：一个任务且未传 `task` 时自动选择。
- `select_target_task_many_requires_id_when_non_tty`：多任务非 TTY 且未传参数时报错。
- `select_target_task_rejects_ambiguous_name_or_prefix`：重复名称和重复 id 前缀必须报错。
- `watch_snapshot_formats_progress_and_cost`：聚合 file/chunk 进度、RTF、ETA、bytes、text chars。
- `watch_snapshot_handles_zero_duration`：空任务或缺少音频时长时不除零。
- `watch_snapshot_uses_run_progress_before_file_store_guess`：存在 `run_progress.json` 时优先使用明确的运行进度字段。
- `watch_snapshot_marks_estimated_eta_confidence`：ETA 缺少稳定样本时不展示假精确值。
- `render_recent_files_truncates_long_paths`：长路径不挤爆终端布局。
- `terminal_guard_restores_raw_mode_on_error`：渲染或 API 错误后恢复终端模式。

### E2E 测试

新增或扩展：

```text
e2e-tests/tests/test_asr_task_tui.sh
```

验证点：

- 用临时 `BIFROST_DATA_DIR` 启动 `cargo run --bin bifrost -- start -p <port> --unsafe-ssl --no-system-proxy`。
- 创建 0 个任务时，`bifrost ai asr task tui --no-interactive-select` 输出无任务提示并退出。
- 创建 1 个任务时，使用 pseudo-terminal 启动 TUI，确认自动进入详情页并能按 `q` 退出。
- 创建 2 个任务时，非 TTY 未传 `task` 返回明确错误；传 `task` 直接进入。
- 创建 2 个重名任务时，传名称进入必须报歧义，传完整 id 成功。
- `--json-snapshot` 输出包含 `progress` 和 `consumption`。
- 使用窄 PTY 启动 TUI，确认布局降级后文本不重叠，按 `q` 可退出。

### human_tests

新增：

```text
human_tests/asr-task-cli-tui.md
```

用例：

- `TC-ASR-TUI-01` 单任务自动进入详情 TUI。
- `TC-ASR-TUI-02` 多任务交互式选择进入指定任务。
- `TC-ASR-TUI-03` 多任务传 `task` 直接进入，跳过选择。
- `TC-ASR-TUI-04` 任务运行中展示文件进度、chunk 进度、RTF、耗时、source bytes 和失败信息。
- `TC-ASR-TUI-05` 服务未启动/任务不存在/非交互多任务错误提示。
- `TC-ASR-TUI-06` 窄终端和非 Unicode 环境下布局降级，进度条和路径仍可读。
- `TC-ASR-TUI-07` `--read-only` 模式下不会触发 run/pause/resume/force pause 写操作。

实现阶段必须同步更新 `human_tests/readme.md` 索引，并按用例真实执行。

## Review/Fix/Test 闭环计划

## 2026-05-24 增量：Daily/Jennie Agent 状态与文件打开

### 背景与缺口

当前 `bifrost ai asr task watch/tui` 已能观察 ASR 文件进度、chunk 进度和消耗信息，但 Daily/Jennie Agent 后处理状态仍分散在 `daily-agent` 与 `daily` API 中，CLI TUI 里无法看到 Agent 正在处理什么、还有多少每日文档待处理、已经处理多少。另外 `bifrost ai asr task daily list` 强制要求 `<TASK_ID>`，没有复用 watch/tui 的单任务自动进入与多任务交互选择体验。

### 新增交互规则

- `bifrost ai asr task daily list [task]`：
  - 无任务时报错。
  - 只有一个 ASR task 时自动进入。
  - 多个 ASR task 且未传 `task` 时，在 TTY 中打开交互式选择；非 TTY 下报错并提示传 task id/name/prefix。
  - `task` 支持完整 id、唯一 id 前缀、唯一任务名。
- `bifrost ai asr task daily show <date> [--task <task>]`：
  - 不传 `--task` 时复用上述选择规则。
  - 保留兼容旧用法 `daily show <task> <date>`。
- TUI 单任务详情页：
  - 增加 `Daily/Jennie Agent` 面板，展示 enabled/runner/status、已处理/待处理、report/unindexed 统计、last run/error。
  - `Tab` 在 `Recent Files` 与 `Daily Agent Docs` 两个列表间切换。
  - `↑/↓` 选择当前列表行。
  - `Enter` 用系统默认程序打开当前行文件；`o` 在文件管理器中定位当前行文件。
  - 支持鼠标点击当前列表行打开文件；不支持鼠标的终端仍可使用键盘。
  - `R` 立即运行在已运行状态下只显示 `already running`；`p` 暂停/恢复；`P` 强制暂停并释放计算资源；控制 API 的 JSON `message` 直接显示到底栏。

### Watch API 增量字段

`GET /api/asr/tasks/{task_id}/watch` 与 `/api/asr/tasks/-/watch` 的每个 task snapshot 增加：

```json
{
  "daily_agent": {
    "enabled": true,
    "runner": "codex",
    "status": "running",
    "last_run_id": "...",
    "last_error": null,
    "last_run_at_ms": 1779618486232,
    "daily_files": 5,
    "processed_documents": 3,
    "pending_documents": 2,
    "report_files": 3,
    "indexed_reports": 3,
    "unindexed_reports": 0,
    "processed_missing_report": 0,
    "recent_documents": [
      {
        "date": "2026-05-24",
        "status": "pending",
        "change_kind": "new_file",
        "source_path": ".../.daily/2026-05-24.md",
        "report_path": ".../.daily/report/2026-05-24-report.md"
      }
    ]
  }
}
```

`pending_documents` 基于每日 Markdown 源文件与 `daily_agent_processed.json` 的 hash/size 对比计算：新文件、追加、重写、缺失 report 都算待处理；源文件未变化且 report 存在才算已处理。磁盘上已有 report 但 processed state 未索引时展示为 `report_only`，并进入 `unindexed_reports` 统计。

### 测试补充

- 单元测试：Daily Agent watch summary 统计 processed/pending/report-only；daily show 参数解析兼容旧用法和新 `--task` 用法；TUI 打开命令构造覆盖 macOS/Linux/Windows 分支中当前平台路径。
- E2E：扩展 `e2e-tests/tests/test_asr_task_tui.sh`，构造 `.daily/*.md`、`report/*-report.md` 与 `daily_agent_processed.json`，验证 watch JSON 的 `daily_agent` 字段、`daily list` 无 task 自动选择、多任务非交互错误、TUI 渲染 `Daily/Jennie Agent`，并通过 PTY/API 验证刷新、暂停/恢复、强制暂停、暂停时 run 的用户可读错误。
- human_tests：在 `human_tests/asr-task-cli-tui.md` 新增用例，覆盖 daily 命令交互选择、TUI Agent 状态、文件列表打开行为、刷新/立即运行/强制暂停/暂停恢复控制动作。

第 1 轮：

- 目标复核：逐条确认命令选择规则、单任务自动进入、多任务交互选择、进度和消耗信息、Daily/Jennie Agent 已处理/待处理、文件打开。
- 代码 review：检查 `asr.rs` 文件增长、TUI raw mode/mouse capture 恢复、API 刷新频率、JSON 聚合边界、watch 路由顺序、run_progress 与 daily_agent 状态一致性。
- 测试运行：目标单元测试、`cargo test -p bifrost-cli asr_tui`、相关 Admin API 聚合测试、ASR TUI E2E。

第 2 轮：

- 目标复核：复查第 1 轮修复后的 diff，确认现有 ASR CLI 输出未变。
- 代码 review：检查 human_tests、E2E、CLI help、API 文档和设计文档同步。
- 测试运行：`e2e-tests/tests/test_asr_task_tui.sh`、`cargo test --workspace --all-features`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`，最后执行 `rust-project-validate`。

如任一轮发现 TUI 退出恢复、API 性能、任务选择或消耗信息错误，追加第 3 轮直到关闭。

## 2026-06-04 增量：Daily Agent 自动触发改为 daily 合成文档变更门禁

### 问题

真实任务 `76612de33e9740bc92440ce64a98a4cb` 在 2026-06-04 04:12 生成了新的 `.daily/2026-06-03.md`，但 Daily Agent 没有自动运行。排查发现 ASR 任务最新 run 已结束并刷新了 daily 合成 Markdown，但总体仍有 failed 文件，其中既包含：

```text
diarization_no_segments: sherpa-onnx returned no speaker segments
diarization_no_asr_units: diarization produced no transcribable ASR units
```

旧门禁 `daily_agent_asr_completion_ready` 从 ASR 文件状态判断是否允许 Daily Agent 自动触发：只放行全成功或特定 `diarization_no_segments:` 失败。这个规则仍会把“ASR run 已结束且 daily 合成文档已更新”的有效结果挡住，导致 `2026-06-03.md` 已生成但 `2026-06-03-report.md` 不生成。

### 修复语义

- 自动触发点仍在 ASR run 完成、`refresh_task_daily_summaries` 刷新 daily 合成文档之后。
- 是否排队 Daily Agent 不再由 failed 文件类型决定，而是复用 `DailyAgentChangePlanner` 检查 enabled 且 `trigger_policy=after_asr_run` 的 Agent 是否存在 `NewFile`、`Appended` 或 `Rewritten` daily Markdown。
- 只要任一可运行 Agent 有 daily 合成文档变更，就排队执行 `run_daily_agents(..., trigger_source=asr_completion, force=false)`；各 Agent 内部仍按自己的 processed state 只处理有变化的日期。
- 如果所有 Agent 的 daily Markdown 均为 `Unchanged`，或没有 daily Markdown 文件，则跳过自动运行，避免空跑。
- 这样即使本轮 ASR 有普通 failed 文件，只要 run 已经结束并产出了新的 `.daily/*.md`，Daily Agent 仍能继续生成 report；真正没有合成文档更新的 run 不会触发后处理。

### 测试补充

- 单元测试：覆盖无 daily Markdown 不触发、已有 processed state 且 source hash 未变不触发、daily Markdown 新增/追加触发、存在普通 failed 和 `diarization_no_asr_units` failed 时只要 daily Markdown 变更仍触发、多 Agent 中任一 Agent 待处理即触发。
- E2E：扩展 `e2e-tests/tests/test_asr_daily_agents_api.sh`，在已有 report 后追加 daily Markdown，不带 `force` 再运行，断言 processed run_id 更新且 prompt 包含 `change_kind=Appended`。
- human_tests：更新 `human_tests/asr-daily-agents.md` 的回归用例，验证 ASR run 完成后以 daily 合成文档变更作为 Daily Agent 自动触发门禁，不再绑定某个 diarization 错误字符串。

## 2026-06-05 增量：Daily Agent 报告完成后自动按 Agent 分目录同步

### 问题背景

Daily Agent 页面提供任务级 `report_sync_dir` 和 `Sync Reports` 操作，但旧同步路径把报告直接复制到同步根目录，多个 Agent 在同一天生成的 `YYYY-MM-DD-report.md` 会争用同一个文件名。另一个问题是任务级同步目录只镜像到主 Agent：`tomorrow_todo` 这类后续 Agent 跑完后没有继承同步目录，因此即使报告生成成功也不会自动同步。

### 修复语义

- 任务级 `report_sync_dir` 是所有 Daily Agents 共用的同步根目录；保存该目录时同步写入每个 Agent item，历史配置中 Agent item 为空时也从任务级字段继承。
- 每个 Agent 成功生成 report 后立即同步本轮生成的 report，不等待用户手动点击 `Sync Reports`。
- 同步目标按 Agent 分目录：`<report_sync_dir>/<agent_id>/YYYY-MM-DD-report.md`，避免不同 Agent 同一天报告覆盖或误判为相同内容跳过。
- 手动 `Sync Reports` 遍历全部已配置 Agent，返回任务级汇总结果，同时分别更新各 Agent 的 `last_report_sync`。
- 既有 processed state、IM delivery、Git commit 和 change plan 语义不变。

### 测试补充

- 单元测试：覆盖任务级同步目录同步到所有 Agent、同一天两个 Agent 报告复制到不同 agent 子目录且不会落到同步根目录。
- E2E：扩展 `e2e-tests/tests/test_asr_daily_agents_api.sh`，配置同步根目录后触发两个 Agent 真实运行，断言 `daily_report` 与 `tomorrow_todo` 的报告自动出现在各自子目录，并检查 `last_report_sync.target_dir`。
- CLI E2E：更新 `e2e-tests/tests/test_asr_task_cli.sh`，手动 `daily sync` 断言报告复制到 `daily_report/` 子目录。
- human_tests：更新 `human_tests/asr-daily-agents.md` 的自动同步回归用例。

## 2026-06-05 增量：Daily Agent 全局专有名词配置

### 问题背景

Daily Agent 运行时需要参考随任务不断变化的专有名词、项目代号、人名、缩写和固定翻译。旧模型只能把这些内容手写进每个 Agent 的 `AGENTS.md`，多 Agent 场景会重复维护；ChatGPT Web Runner 又会复用固定 conversation，只有首轮注入 `AGENTS.md`，后续轮次无法感知每天或每次任务更新的术语。

### 实现语义

- `daily_agent.terminology` 是任务级全局文本配置，由 Daily Agent 页面提供 `Terminology` 文本输入框保存；空白内容归一化为未配置。
- Workspace 初始化、保存配置、读取配置和运行前都会把术语写入每个 enabled/known Agent 工作目录根目录的 `TERMS.md`。该文件位于 `.daily/agents/<agent_id>/TERMS.md`，与 `AGENTS.md`、`input/`、`output/` 同级。
- `AGENTS.md` 通过 Bifrost 托管块引用相对路径 `TERMS.md`，说明 Runner 运行前必须读取该文件；托管块可重复刷新，不覆盖用户自定义指令正文。清空术语时删除 `TERMS.md` 并移除托管块。
- Bifrost Agent、Codex 和其他文件型 Runner 的 prompt 只引用 `TERMS.md` 相对文件路径，不内联术语正文，保证工作目录是单一事实源。
- `chatgpt_web` Runner 每次构造 prompt 时都从 `TERMS.md` 或最新配置读取术语，并把 `## 专有名词配置（每次运行动态注入）` 放在 prompt 最前面；不依赖固定 conversation 首轮缓存，后续轮次也会注入最新术语。
- 单 Agent task projection 必须继承任务级术语，避免 normalize 或按 agent 串行运行时丢失全局配置。

### 测试补充

- 单元测试：覆盖术语归一化、配置 normalize、`task_for_daily_agent` 继承、每个 Agent 写入 `TERMS.md` 和 `AGENTS.md` 相对引用，以及 ChatGPT Web 首轮/后续轮次都前置注入术语。
- E2E：扩展 `e2e-tests/tests/test_asr_daily_agents_api.sh`，通过 API 保存全局术语，断言两个默认 Agent 都生成 `TERMS.md`、`AGENTS.md` 引用 `TERMS.md`，真实 run 仍生成并同步报告。
- human_tests：新增 `TC-ADA-13`，验证 Daily Agent 全局术语配置在 WebUI/API、文件型 Runner 工作目录和 ChatGPT Web prompt 策略三类入口上的用户可感知行为。

### Review/Fix/Test 闭环

- 第 1 轮复核配置模型、workspace 写入、prompt 注入顺序和 WebUI 保存入口；运行 Daily Agent targeted 单测与 API E2E。
- 第 2 轮复核 custom `AGENTS.md` 保存不会覆盖 `TERMS.md` 托管引用、清空术语会清理托管文件、report sync 多 Agent 行为不回退；复跑受影响单测、E2E 和 human_tests。

## 实施顺序建议

1. 新增 `run_progress.json` 写入/恢复逻辑和单元测试。
2. 新增轻量 watch snapshot 聚合 API：`/api/asr/tasks/{task_id}/watch`、`/api/asr/tasks/-/watch`。
3. 抽出/复用 ASR task CLI client，新增 `task watch/tui` 命令定义。
4. 实现任务选择规则、歧义处理、`--json-snapshot` 和 `--read-only`。
5. 实现 ratatui 详情页、窄屏降级、ASCII 降级与 `--all` 总览页。
6. 补 E2E 和 human_tests，真实启动服务验证。
7. 两轮 Review/Fix/Test 后再进入提交/推送。
