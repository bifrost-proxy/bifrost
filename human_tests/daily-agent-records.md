# Daily Agent Records 测试用例

## 功能模块说明

Daily Agent Records 是 ASR 任务详情页中查看 Daily Agent 历史运行产物的记录页。它必须展示 `daily_agent_processed.json` 中已处理文档，也必须兜底发现任务 Daily 工作区中已经存在的 report 文件，避免历史任务只有 `daily/report/` 或 `daily/Report/` 文件但页面显示空状态。
Run Results tab 中的记录必须按时间倒序展示，最新日期优先，避免用户打开详情页后先看到旧运行数据。
Run Results tab 顶部必须支持按 Agent、Date、Runner 筛选记录，方便多 Agent 和多 Runner 场景下定位某一次结果。

## 前置条件

- 在仓库根目录执行命令前先运行 `source ~/.zshrc`。
- 使用临时数据目录启动 Bifrost，必须带 `--no-system-proxy`，避免修改系统代理：

```bash
BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy -y
```

- 测试数据可以通过 API 创建 ASR Directory Task，也可以直接在临时 `BIFROST_DATA_DIR/asr/data/text/<task_id>/daily/` 下创建最小目录结构。

## 测试用例列表

### TC-DAR-01 Report 目录历史报告兜底发现回归

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR`。
2. 创建 ASR 任务记录，或使用测试辅助脚本写入一个任务 ID 为 `daily-records-human-task` 的 ASR Directory Task。
3. 创建目录 `asr/data/text/daily-records-human-task/daily/Report/`。
4. 写入 `asr/data/text/daily-records-human-task/daily/Report/2026-05-14-report.md`，内容包含 `# Historical Daily Report`。
5. 不创建 `asr/tasks/daily-records-human-task/daily_agent_processed.json`。
6. 请求 `GET /_bifrost/api/asr/tasks/daily-records-human-task/daily-agent/runs`。
7. 请求 `GET /_bifrost/api/asr/tasks/daily-records-human-task/daily-agent/reports/2026-05-14`。

预期结果：

- `runs` 响应中 `processed_documents` 至少包含一条 `date=2026-05-14` 的记录。
- 该记录的 `report_path` 指向 `daily/Report/2026-05-14-report.md`。
- 该记录的 `runner` 为 `filesystem`，表示来自磁盘兜底发现。
- report 详情响应状态为 200，`content` 包含 `Historical Daily Report`。
- `GET /reports/%2E%2E%2Fsecret` 或等价路径穿越日期仍返回 400，不允许越权读文件。

### TC-DAR-02 processed state 元数据优先且补齐 report_path

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR`。
2. 创建任务 ID 为 `daily-records-state-task` 的 ASR Directory Task。
3. 创建 `asr/data/text/daily-records-state-task/daily/report/2026-05-15-report.md`。
4. 写入 `asr/tasks/daily-records-state-task/daily_agent_processed.json`，其中 `documents["2026-05-15"]` 包含 `runner=web`、`last_run_id=run-1`、`report_path=null`。
5. 请求 `GET /_bifrost/api/asr/tasks/daily-records-state-task/daily-agent/runs`。

预期结果：

- 响应只包含一条 `date=2026-05-15` 记录，不因磁盘扫描产生重复行。
- 记录保留 processed state 中的 `runner=web`、`last_run_id=run-1` 和 `processed_at_ms`。
- 记录的 `report_path` 被补齐为实际存在的 `daily/report/2026-05-15-report.md`。

### TC-DAR-03 Daily Agent 配置页展示 report 索引状态

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR`。
2. 创建任务 ID 为 `daily-records-index-task` 的 ASR Directory Task。
3. 创建 `asr/data/text/daily-records-index-task/daily/report/2026-05-14-report.md` 和 `2026-05-15-report.md`。
4. 创建 `asr/tasks/daily-records-index-task/daily_agent_processed.json`，只包含 `documents["2026-05-14"]`。
5. 请求 `GET /_bifrost/api/asr/tasks/daily-records-index-task/daily-agent`。
6. 再次检查 `daily_agent_processed.json` 内容没有被自动增加 `2026-05-15`。

预期结果：

- 响应包含 `report_index_status.report_files=2`。
- 响应包含 `report_index_status.processed_documents=1`。
- 响应包含 `report_index_status.indexed_reports=1`。
- 响应包含 `report_index_status.unindexed_reports=1`。
- 响应包含 `report_index_status.unindexed_dates=["2026-05-15"]`。
- `daily_agent_processed.json` 仍只包含 `2026-05-14`，证明配置页状态只做提示，不做自动回填。

### TC-DAR-04 Run Results 最新日期优先倒序展示

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR`。
2. 创建任务 ID 为 `daily-records-sort-task` 的 ASR Directory Task。
3. 在 `daily_agent_processed.json` 中按非倒序写入 `2026-05-14`、`2026-05-16`、`2026-05-15` 三条 processed documents。
4. 请求 `GET /_bifrost/api/asr/tasks/daily-records-sort-task/daily-agent/runs`。
5. 打开 ASR 任务详情页的 `Daily Agent Records` tab，确认 `Run Results` 表格首行日期。

预期结果：

- API 响应中的 `processed_documents` 日期顺序为 `2026-05-16`、`2026-05-15`、`2026-05-14`。
- `Run Results` 表格首行显示 `2026-05-16`，最新数据优先。
- 点击任一 report 链接的行为不受排序影响，仍能进入对应日期的全屏 Markdown 详情。

### TC-DAR-05 详情页读取与列表状态路径一致回归

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR`。
2. 创建任务 ID 为 `daily-records-state-path-task` 的 ASR Directory Task。
3. 创建 `asr/data/text/daily-records-state-path-task/daily/report/2026-05-20-report.md`，内容包含 `# State Path Report`。
4. 写入 `asr/tasks/daily-records-state-path-task/daily_agent_processed.json`，其中 `documents["2026-05-20"].report_path` 指向上一步创建的真实 report 文件。
5. 请求 `GET /_bifrost/api/asr/tasks/daily-records-state-path-task/daily-agent/runs`，确认列表记录包含同一个 `report_path`。
6. 请求 `GET /_bifrost/api/asr/tasks/daily-records-state-path-task/daily-agent/reports/2026-05-20`。
7. 直接访问非法日期 API：
   ```bash
   curl -i 'http://127.0.0.1:<port>/_bifrost/api/asr/tasks/daily-records-state-path-task/daily-agent/reports/%2E%2E%2Fsecret'
   ```

预期结果：

- `runs` 响应中 `date=2026-05-20` 的记录展示 `daily_agent_processed.json` 里的 `report_path`。
- report 详情响应状态为 200，`path` 与列表记录中的 `report_path` 一致，`content` 包含 `State Path Report`。
- 详情接口不会重新拼接另一个 Daily workspace 路径导致 404。
- 非法日期或路径穿越仍返回 400/404，不允许越权读文件。

### TC-DAR-06 Daily Agent report 同步目录 CLI 控制

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost，必须带 `--no-system-proxy`。
2. 创建一个 ASR Directory Task，并在 `asr/data/text/<task_id>/.daily/report/` 下写入 `2026-05-17-report.md`。
3. 执行：
   ```bash
   BIFROST_DATA_DIR=<temp> bifrost ai asr task daily set-sync-dir <task_id> --dir <sync_dir>
   ```
4. 执行：
   ```bash
   BIFROST_DATA_DIR=<temp> bifrost ai asr task daily sync <task_id>
   ```
5. 检查 `<sync_dir>/2026-05-17-report.md`。
6. 将目标文件内容改成短的旧内容，例如 `stale-short`。
7. 再次执行：
   ```bash
   BIFROST_DATA_DIR=<temp> bifrost ai asr task daily sync <task_id> --json
   ```
8. 检查目标文件已恢复为原 report 内容。
9. 第三次执行同一条 `sync --json`。

预期结果：

- `set-sync-dir` 输出包含配置的同步目录。
- 首次 `sync` 输出包含 `Copied: 1`、`Skipped: 0`、`Failed: 0`。
- 同步目录中存在 `daily_report/2026-05-17-report.md`，内容与原 report 一致；同步根目录下不存在未分 Agent 的 `2026-05-17-report.md`。
- 第二次 `sync --json` 返回 `sync.total_files=1`、`copied_files=1`、`skipped_files=0`、`failed_files=0`；目标文件 hash 与源 report 不一致时通过覆盖写副本修复短文件。
- 第三次 `sync --json` 返回 `copied_files=0`、`skipped_files=1`、`failed_files=0`；目标文件 hash 与源 report 一致时不重复复制。

### TC-DAR-07 Daily Agent WebUI report 同步目录与状态展示

操作步骤：

1. 打开 ASR task 详情页，切换到 `Daily Agent` tab。
2. 在 `Configuration` 区域找到 `Report Sync Dir` 输入框，填写一个同步目录并点击 `Save`。
3. 点击配置区下方 `Sync Reports` 按钮。
4. 查看 `Last Run Status` 区域。

预期结果：

- `Report Sync Dir` 可以保存到后端 `report_sync_dir`。
- 未配置目录时 `Sync Reports` 按钮不可用；配置后按钮可用。
- 点击 `Sync Reports` 后页面显示成功提示。
- `Last Run Status` 展示最近同步结果，包含 copied/total、skipped、Last Sync 和 Sync Dir；如失败则展示错误摘要。

### TC-DAR-08 Daily Agent CLI 同步目录 normalize 回归

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost，必须带 `--no-system-proxy`。
2. 创建一个默认 ASR Directory Task，保留默认双 Daily Agent 配置。
3. 执行：
   ```bash
   BIFROST_DATA_DIR=<temp> bifrost ai asr task daily set-sync-dir <task_id> --dir <sync_dir>
   ```
4. 触发一次会重新 `load_tasks()` 的后续请求：
   ```bash
   BIFROST_DATA_DIR=<temp> bifrost ai asr task daily sync <task_id>
   ```
5. 执行单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_report_sync_dir_update_survives_task_normalization --lib -- --nocapture
   ```

预期结果：

- `set-sync-dir` 同时写入 legacy `daily_agent.report_sync_dir` 和 primary agent `agents[0].report_sync_dir`。
- 任务经过 `load_tasks()` / `normalize_daily_agent_config` 后仍保留配置的同步目录。
- 后续 `daily sync` 不返回 `Daily Agent report sync directory is not configured`。
- 同步仍按 TC-DAR-06 复制 report；当目标副本 hash 不一致时覆盖，hash 一致时跳过。

### TC-DAR-13 Daily Agent report 同步外部目录卡死回归

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost，必须带 `--no-system-proxy` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
2. 创建一个 ASR Directory Task，并准备一个已生成的 Daily Agent report。
3. 配置 `report_sync_dir` 为一个外部同步目录替身，例如临时目录下的 `icloud-sync`。
4. 在目标 `daily_report/2026-05-14-report.md` 预先写入旧内容，并将该目标文件权限改为不可读。
5. 执行：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_report_sync_overwrites_unreadable_target_when_hash_cannot_be_read --lib -- --nocapture
   ```
6. 执行自动同步回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_report_sync_auto_after_generation_uses_isolated_copy_path --lib -- --nocapture
   ```
7. 执行 CLI/API 回归：
   ```bash
   bash e2e-tests/tests/test_asr_task_cli.sh
   ```

预期结果：

- 同步过程在目标文件可读时按 hash 判断是否跳过；不可读的已有目标文件会被临时文件 + rename 覆盖为新 report。
- 每日任务执行结束后的自动同步也走同一套隔离复制路径，并把 `last_report_sync` 写回对应 Agent。
- `daily sync` 首次、hash 不一致二次和 hash 一致三次执行都能返回，二次同步返回 `copied_files=1`、`skipped_files=0`、`failed_files=0`，三次同步返回 `copied_files=0`、`skipped_files=1`、`failed_files=0`。
- 如果真实外部目录/I/O 超过同步超时，`/_bifrost/api/asr/tasks/<task_id>/daily-agent/sync` 返回结构化失败结果，代理进程仍能响应其他 admin/proxy 请求。

### TC-DAR-09 Daily Agent Records 支持按 Agent、Date、Runner 筛选

操作步骤：

1. 打开 ASR 任务详情页：
   `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>&asrTaskTab=daily-agent-records`。
2. 确认 `Run Results` 顶部展示 Agent、Date、Runner 三个筛选控件。
3. 在 Agent 筛选中选择 `tomorrow_todo`。
4. 确认表格只展示 `tomorrow_todo` 的记录。
5. 在 Date 筛选中选择 `2026-05-20`。
6. 确认表格只展示该日期的记录，且仍满足 Agent 筛选。
7. 在 Runner 筛选中选择 `abc`。
8. 确认表格只展示该 Runner 的记录，且仍满足 Agent 和 Date 筛选。
9. 依次清空 Runner、Date、Agent 筛选。

预期结果：

- 三个筛选控件均可见、可选择、可清空。
- Agent 筛选只影响当前 Run Results 表格展示，不修改后端 processed state。
- Date 筛选与 Agent 筛选可以叠加。
- Runner 筛选与 Agent、Date 筛选可以叠加。
- 清空筛选后恢复展示所有 Daily Agent records。

### TC-DAR-10 Daily Agent Records 窄窗口表格横向滚动不撑宽 tab

操作步骤：

1. 使用窄窗口打开 ASR 任务详情页：
   `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>&asrTaskTab=daily-agent-records`。
2. 确认页面停留在 `Daily Agent Records` tab。
3. 查看 `Run Results` 表格在窄窗口下的横向滚动行为。
4. 将表格内部横向滚动条拖到最右侧，确认可以看到 `Report` 列。
5. 确认 ASR task tab 内容区本身没有被表格撑出页面级横向滚动。

预期结果：

- `Run Results` 表格内部存在横向滚动，能查看所有列。
- 外层 ASR task tab 内容区不会因为 Records 表格列宽而产生页面级横向滚动。
- 左侧 `Date` / `Agent` 列不会因为整页横向偏移而被裁切到不可读。
- 筛选控件和 `Refresh` 按钮仍位于可见内容区内。

### TC-DAR-11 Daily Agent report 详情页使用自身内容区滚动

操作步骤：

1. 打开 Daily Agent report 详情页：
   `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>&asrTaskTab=daily-agent-records&asrDailyReport=2026-05-20&asrDailyAgent=tomorrow_todo`。
2. 等待 report 内容加载完成。
3. 检查 report 页面根节点、Card body 和 Markdown 内容容器的纵向滚动行为。
4. 使用 report 详情页内容区滚动到内容末尾。

预期结果：

- Daily Agent report 详情页不让浏览器窗口或 ASR 最外层页面滚动。
- 详情页 Card body 负责纵向滚动。
- Markdown 内容容器不设置 `max-height` 和纵向 `overflow:auto`。
- 返回按钮、详情元信息和 report 正文仍正常展示。

### TC-DAR-12 ASR 任务详情页滚动限制在当前 Tab 内部

操作步骤：

1. 使用较小浏览器视口打开 ASR 任务详情页：
   `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>&asrTaskTab=files`。
2. 检查任务详情根节点、Card body 和 `.asr-task-detail-tabs > .ant-tabs-content-holder` 的纵向滚动行为。
3. 切换到 `Daily Docs`、`Daily Agent`、`Daily Agent Records` tab 后重复检查。
4. 尝试滚动浏览器窗口、任务详情根节点和 Card body。
5. 尝试滚动当前 Tab 的内容区。

预期结果：

- 浏览器窗口、任务详情根节点和任务详情 Card body 不产生纵向滚动。
- `.asr-task-detail-tabs > .ant-tabs-content-holder` 是当前 Tab 的纵向滚动容器。
- Files、Daily Docs、Daily Agent、Daily Agent Records 四个 tab 都保持同样滚动模型。
- 表格横向滚动仍限制在表格内部，不引入页面级横向滚动。

## 清理步骤

1. 停止测试端口上的 Bifrost 进程。
2. 删除临时 `BIFROST_DATA_DIR`。

## 执行记录

| 日期 | 用例 | 命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-05-20 | TC-DAR-01 Report 目录历史报告兜底发现回归 | `SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR=/tmp/bifrost-dar-human.RzRlVx cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy -y`；创建临时 ASR task `dfcd83a68a744307b8ef56edfc58d7f4`，将 task daily 工作区下 `report` 重命名为真实 `Report`，写入 `Report/2026-05-14-report.md` 且不创建 `daily_agent_processed.json`；请求 `/_bifrost/api/asr/tasks/<task_id>/daily-agent/runs`、`/_bifrost/api/asr/tasks/<task_id>/daily-agent/reports/2026-05-14`、`/_bifrost/api/asr/tasks/<task_id>/daily-agent/reports/%2E%2E%2Fsecret` | PASS：`processed_documents` 返回 1 条 `date=2026-05-14`，`runner=filesystem`，`report_path` 指向 `daily/Report/2026-05-14-report.md`；详情接口返回同一 `Report` 路径且正文包含 `Historical Daily Report`；路径穿越日期返回 400 |
| 2026-05-20 | TC-DAR-02 processed state 元数据优先且补齐 report_path | 同一真实服务与临时数据目录；创建临时 ASR task `aa81bf23510c4eb2b727c42c8ba93514`，写入 `daily/report/2026-05-15-report.md` 与 `asr/tasks/<task_id>/daily_agent_processed.json`，其中 `runner=web`、`last_run_id=run-1`、`report_path=null`；请求 `/_bifrost/api/asr/tasks/<task_id>/daily-agent/runs` | PASS：响应 1 条记录，没有重复行；保留 `runner=web`、`last_run_id=run-1`、`processed_at_ms=100`、`source_sha256=abc123`；`report_path` 补齐为 `daily/report/2026-05-15-report.md` |
| 2026-05-21 | TC-DAR-03 Daily Agent 配置页展示 report 索引状态 | `BIFROST_DATA_DIR=/tmp/bifrost-dar-index-human.w5hMjx target/debug/bifrost start -p 18881 --unsafe-ssl --no-system-proxy --skip-cert-check -y`；创建临时 ASR task `eeb1f24cdef34d5ab31b0c3c8745482c`，写入 2 个 report 文件和只包含 `2026-05-14` 的 `daily_agent_processed.json`；请求 `/_bifrost/api/asr/tasks/<task_id>/daily-agent` 并复查状态文件 keys | PASS：`report_index_status` 返回 `report_files=2`、`processed_documents=1`、`indexed_reports=1`、`unindexed_reports=1`、`unindexed_dates=["2026-05-15"]`；状态文件仍只包含 `2026-05-14`，未自动回填 |
| 2026-05-21 | TC-DAR-04 Run Results 最新日期优先倒序展示 | `SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR=/tmp/bifrost-dar-sort-human.sjmERC target/debug/bifrost start -p 55092 --unsafe-ssl --no-system-proxy --skip-cert-check -y`；创建临时 ASR task `a4e4520ba02c43a99508fea6785d732e`，在 `daily_agent_processed.json` 中按非倒序写入 `2026-05-14`、`2026-05-16`、`2026-05-15`，并请求 `/_bifrost/api/asr/tasks/<task_id>/daily-agent/runs` 与 `/_bifrost/api/asr/tasks/<task_id>/daily-agent/reports/2026-05-16` | PASS：`processed_documents` 返回日期顺序 `2026-05-16,2026-05-15,2026-05-14`；最新日期 report 详情返回 200 且正文包含 `Report 2026-05-16`；临时服务、数据目录和音频目录已清理 |
| 2026-05-26 | TC-DAR-06 Daily Agent report 同步目录 CLI 控制 | `source ~/.zshrc && bash e2e-tests/tests/test_asr_task_cli.sh` | PASS：脚本使用临时 Bifrost、临时 ASR task 和临时同步目录，执行 `daily set-sync-dir <task_id> --dir <sync_dir>` 后输出同步目录；执行 `daily sync <task_id>` 后复制 `2026-05-17-report.md` 到同步目录，输出 `Copied: 1`、`Skipped: 0`；篡改目标短文件后二次 `daily sync <task_id> --json` 预期为 hash 不一致覆盖目标副本，返回 `total_files=1`、`copied_files=1`、`skipped_files=0`、`failed_files=0`；三次同步 hash 一致返回 `copied_files=0`、`skipped_files=1`、`failed_files=0` |
| 2026-05-26 | TC-DAR-07 Daily Agent WebUI report 同步目录与状态展示 | `source ~/.zshrc && pnpm --dir web exec playwright test tests/ui/asr-daily-agent-runner.spec.ts --grep "simple Runner"` | PASS：WebUI mock 验证 `Report Sync Dir` 输入框可保存 `report_sync_dir`，`Sync Reports` 按钮触发 `/daily-agent/sync`，toast 显示 `Synced 2 copied, 0 skipped`，Last Run Status 展示 `2 copied / 2 total` 和同步目录 |
| 2026-06-03 | TC-DAR-08 Daily Agent CLI 同步目录 normalize 回归 | `source ~/.zshrc; SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_report_sync_dir_update_survives_task_normalization --lib -- --nocapture`；`source ~/.zshrc; bash e2e-tests/tests/test_asr_task_cli.sh` | PASS：单元回归验证 `set_primary_daily_agent_report_sync_dir` 同时更新 legacy 与 primary agent，`normalize_daily_agent_config` 后 `report_sync_dir` 不丢失；真实 CLI/API E2E 验证 `daily set-sync-dir` 后立刻 `daily sync` 成功复制 report，未再返回 `Daily Agent report sync directory is not configured` |
| 2026-06-03 | TC-DAR-09 Daily Agent Records 支持按 Agent、Date、Runner 筛选 | `source ~/.zshrc; pnpm --dir web exec node --input-type=module <Playwright script>` 打开 `asrTaskTab=daily-agent-records`，选择 Agent=`tomorrow_todo`、Date=`2026-05-20`、Runner=`abc`，再依次清空筛选；同脚本打开 `asrTaskTab=daily` 复查 Daily Docs 表头高度 | PASS：Run Results 初始 9 行，Agent 筛选后 1 行，Date 叠加后 1 行，Runner 叠加后 1 行，样例行为 `2026-05-20 tomorrow_todo ... abc 2026-05-20-report.md`；清空筛选后恢复 9 行；Daily Docs 表头最大高度 40px |
| 2026-06-03 | TC-DAR-12 ASR 任务详情页滚动限制在当前 Tab 内部 | `source ~/.zshrc; node <Playwright script>` 使用 900x520 视口打开历史任务 `a911c68b0f7a43afa29d1863cc02229a`，分别检查 `files`、`daily`、`daily-agent-records`；另用 1000x620 视口检查 `overview`、`daily-agent` | PASS：各 tab 中 `window.scrollY=0`、任务详情根节点 `scrollTop=0`、任务详情 Card body `scrollTop=0`；`.asr-task-detail-tabs > .ant-tabs-content-holder` 的 `overflow-y=auto` 且可滚动，Files tab `holderScroll=1595/client=261`、Daily Docs `552/261`、Records `588/261`；Overview 与 Daily Agent 也只在 tab content-holder 内滚动 |
| 2026-06-06 | TC-DAR-13 Daily Agent report 同步外部目录卡死回归 | `source ~/.zshrc; SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_report_sync_overwrites_unreadable_target_when_hash_cannot_be_read --lib -- --nocapture`；`source ~/.zshrc; SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_report_sync_auto_after_generation_uses_isolated_copy_path --lib -- --nocapture`；`source ~/.zshrc; bash e2e-tests/tests/test_asr_task_cli.sh` | PASS：目标文件可读时同步通过 hash 判断，hash 不一致使用临时文件覆盖为新 report，hash 一致返回 skipped；目标文件不可读时不因 hash 读取失败卡死，继续使用临时文件覆盖；每日任务执行结束后的自动同步同样走隔离复制路径并写回 Agent `last_report_sync` |
| 2026-06-23 | TC-DAR-06/13 Daily Agent report 同步 hash 回归 | `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_report_sync --lib -- --nocapture`；`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 bash e2e-tests/tests/test_asr_daily_agent_report_sync_hash.sh`；对真实任务 `c1c57318206c4f338f1267b7f37a81b8` 比对 `.daily/agents/{daily_report,tomorrow_todo}/output/.../2026-06-{19,20}-report.md` 与 `/Users/eden_studio/Desktop/个人/report/{daily_report,tomorrow_todo}/2026-06-{19,20}-report.md` 的大小和 sha256 | PASS：6 个同步单测通过；窄 E2E 使用临时服务验证首次同步 `copied_files=1`，目标短文件 hash 不一致后二次同步 `copied_files=1` 并恢复原 report，第三次 hash 一致返回 `copied_files=0`、`skipped_files=1`；真实任务 19/20 号 `daily_report` 与 `tomorrow_todo` 的源/目标 sha256 均一致，其中 `tomorrow_todo` 目标文件短是源 report 本身短，不是同步丢失 |
