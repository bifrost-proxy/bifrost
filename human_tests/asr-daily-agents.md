# ASR Daily Agents 多 Agent 后处理

## 功能模块说明

验证 ASR Directory Task 的 Daily Agent 从单个 `daily_agent` 扩展为可配置的 `agents` 队列后，默认模板、每 Agent 独立指令文件、独立输出目录、记录查询、报告详情、手动运行和 IM 默认配置均符合真实用户可感知行为。

## 前置条件

- 在仓库根目录执行。
- 本用例使用临时 `BIFROST_DATA_DIR` 和临时音频目录，不污染本机数据。
- 启动 Bifrost 时必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并使用 `--no-system-proxy`。
- 可直接执行自动化真实场景脚本：
  ```bash
  e2e-tests/tests/test_asr_daily_agents_api.sh
  ```

## 测试用例列表

### TC-ADA-01 默认创建两个 Daily Agents

操作步骤：
1. 执行 `e2e-tests/tests/test_asr_daily_agents_api.sh`。
2. 脚本会创建一个新的 ASR Directory Task。
3. 脚本请求 `GET /_bifrost/api/asr/tasks/{task_id}/daily-agent`。

预期结果：
- `config.agents` 恰好包含两个默认 Agent。
- 第一个 Agent 为 `daily_report`，输出目录为 `report`。
- 第二个 Agent 为 `tomorrow_todo`，输出目录为 `tomorrow_todo`。
- `tomorrow_todo.im_delivery.enabled=true`，默认 channel 为 `owner:feishu-main`。

### TC-ADA-02 每个 Agent 有独立英文输出目录和记录

操作步骤：
1. 执行 `e2e-tests/tests/test_asr_daily_agents_api.sh`。
2. 脚本写入同一天的 `daily_report:YYYY-MM-DD` 和 `tomorrow_todo:YYYY-MM-DD` 两条 processed state。
3. 脚本请求 `GET /_bifrost/api/asr/tasks/{task_id}/daily-agent/runs`。

预期结果：
- 返回两条 `processed_documents`，同一天日期不会互相覆盖。
- 两条记录的 `agent_id` 分别为 `daily_report` 和 `tomorrow_todo`。
- 两条记录的 `output_dir` 分别为 `report` 和 `tomorrow_todo`。

### TC-ADA-03 按 Agent 打开报告详情

操作步骤：
1. 执行 `e2e-tests/tests/test_asr_daily_agents_api.sh`。
2. 脚本请求 `GET /_bifrost/api/asr/tasks/{task_id}/daily-agent/reports/YYYY-MM-DD?agent_id=tomorrow_todo`。

预期结果：
- 响应 `agent_id=tomorrow_todo`。
- 响应 `output_dir=tomorrow_todo`。
- 响应正文包含 `明日 To Do List`，且不是 `daily_report` 的报告内容。

### TC-ADA-04 按 Agent 读取指令 Markdown

操作步骤：
1. 执行 `e2e-tests/tests/test_asr_daily_agents_api.sh`。
2. 脚本请求 `GET /_bifrost/api/asr/tasks/{task_id}/daily-agent/agents?agent_id=tomorrow_todo`。

预期结果：
- 响应 `agent_id=tomorrow_todo`。
- 指令正文包含 `明日 To Do List`。
- 指令正文要求从转录中提取明天的 To Do List。

### TC-ADA-05 指定 Agent 手动运行可入队

操作步骤：
1. 执行 `e2e-tests/tests/test_asr_daily_agents_api.sh`。
2. 脚本请求 `POST /_bifrost/api/asr/tasks/{task_id}/daily-agent/run?agent_id=tomorrow_todo&date=YYYY-MM-DD&force=1`。

预期结果：
- 响应状态为 `queued` 或已有运行时的 `already_running`。
- 响应包含 `agent_id=tomorrow_todo`。
- 响应保留指定 `date`。

### TC-ADA-06 回归：ASR 完成后 daily 合成文档变更触发 Daily Agent

操作步骤：
1. 执行 `cargo test -p bifrost-admin daily_agent_after_asr_run_requires_changed_daily_markdown -- --nocapture`。
2. 执行 `cargo test -p bifrost-admin daily_agent_after_asr_run_ignores_failed_files_when_daily_markdown_changed -- --nocapture`。
3. 执行 `cargo test -p bifrost-admin daily_agent_after_asr_run_checks_all_agents_for_pending_markdown_changes -- --nocapture`。
4. 执行 `e2e-tests/tests/test_asr_daily_agents_api.sh`，其中脚本会先生成 `2026-05-22-report.md`，再追加 `2026-05-22.md` 并不带 `force` 重新触发 Daily Agent。
5. 对照真实任务状态或 watch 快照，确认 ASR run 已结束并刷新 `.daily/YYYY-MM-DD.md` 后，即使本轮存在 failed 文件，只要 daily 合成 Markdown 相对 processed state 有 `NewFile`、`Appended` 或 `Rewritten` 变更，就会继续推进 Daily Agent；如果 daily Markdown 未变化则跳过，避免空跑。

预期结果：
- 第 1 条测试通过，说明没有 daily Markdown 或所有 Agent 均已处理同一 source hash 时不会自动触发；daily Markdown 追加后会触发。
- 第 2 条测试通过，说明普通 failed 文件和 `diarization_no_asr_units` failed 文件不会阻塞已更新 daily Markdown 的后处理。
- 第 3 条测试通过，说明多 Agent 场景下只要任一 enabled Agent 对该 daily Markdown 仍有待处理变更，就会推进后续运行。
- 第 4 条 E2E 通过，说明真实 Admin API + Runner 链路中，非 force 的 appended daily Markdown 会更新两个 Agent 的 processed run_id，且 prompt 包含 `change_kind=Appended`。
- 该回归只改变 Daily Agent 自动触发门禁，不修改 ASR 文件失败状态、Daily Docs 生成结果、手动 force run API 或 runner readiness 语义。

### TC-ADA-07 Daily Agent 管理列表与 Agent 详情页分离

操作步骤：
1. 启动 WebUI 开发服务或打开正在运行的 Bifrost WebUI。
2. 在浏览器中打开 `/_bifrost/ai?aiSection=tools-asr&asrTask={task_id}&asrTaskTab=daily-agent`。
3. 查看 Daily Agent tab 首屏。
4. 点击任一 Agent 行的 `Edit` 按钮。
5. 在详情页检查 `Agent Configuration`、`IM Delivery`、`Last Run Status` 和 `Agent Instructions` 区域。
6. 点击详情页顶部 `Daily Agents` 返回按钮。

预期结果：
- Daily Agent tab 首屏是 Agent 列表，列表列包括 `Enabled`、`Agent`、`Output Dir`、`Runner`、`IM Delivery`、`Last Run`、`Actions`。
- 首屏不再混排单个 Agent 的 `Agent Configuration`、`IM Delivery`、`Last Run Status` 或 `Agent Instructions` 大块详情内容。
- 点击 `Edit` 后进入单个 Agent 详情页，详情页显示 `Agent Configuration`、`IM Delivery`、`Last Run Status` 和 `Agent Instructions`。
- 详情页的 Runner、Trigger Policy、Timeout、Session Key、Output Dir、IM Channel、Mode、Send Policy 均在当前 Agent 详情中配置。
- 返回按钮能回到 Daily Agent 列表，列表状态和 Agent 行仍可见。

### TC-ADA-08 ASR 顶层 tab 英文文案与窄窗口列表布局

操作步骤：
1. 在浏览器中打开 `/_bifrost/ai?aiSection=tools-asr`。
2. 查看 ASR 首页顶层 tab。
3. 进入任一任务的 `Daily Agent` tab。
4. 将浏览器窗口缩窄到约 900px 或更小。
5. 查看 Daily Agents 表格表头、日期、输出目录、IM Channel 和 Actions 区域。

预期结果：
- ASR 首页顶层 tab 文案为英文：`Scheduled Tasks`、`ASR Management`、`Voiceprint & Wake`。
- 窄窗口下 Daily Agents 表格启用横向滚动，列宽保持稳定。
- `Output Dir`、`Last Run` 日期、IM Channel 和 `Actions` 不会按单字符折成竖排。
- 用户可以横向滚动查看完整列和操作按钮。
- 亮色和暗色主题下表格文字、Tag、按钮都可读可操作。

### TC-ADA-09 Daily Docs 行级 Run Daily Agent 可选择全部或单个 Agent

操作步骤：
1. 在浏览器中打开 `/_bifrost/ai?aiSection=tools-asr&asrTask={task_id}&asrTaskTab=daily`。
2. 在 Daily Docs 表格中找到任一日期行。
3. 查看行内运行按钮。
4. 点击主按钮 `Run All Agents`。
5. 再打开同一按钮右侧下拉菜单，查看可选项。

预期结果：
- Daily Docs 行级按钮主文案为 `Run All Agents`，表示省略 `agent_id` 并按顺序运行全部 enabled Agents。
- 下拉菜单包含 `Run All Agents` 和每个 Agent 的单独运行动作，例如 `Run daily_report`、`Run tomorrow_todo`。
- 选择某个单独 Agent 时，前端调用 Daily Agent run API 时携带该 Agent 的 `agent_id`。
- 禁用的 Agent 在菜单中不可选。
- 该行运行期间按钮展示 loading，避免同一日期重复触发。

### TC-ADA-10 多 Agent 共用 ChatGPT Web Runner 的串行运行与失败隔离

操作步骤：
1. 请求 `GET /_bifrost/api/im-gateway/chat/config`，确认存在启用的 `chatgpt_web` runner。
2. 请求 `GET /_bifrost/api/asr/tasks/{task_id}/daily-agent`，备份当前 Daily Agent 配置。
3. 确认 `daily_report` 和 `tomorrow_todo` 都配置为同一个 `chatgpt_web` runner；如需验证失败隔离，可临时关闭 IM Delivery 并把 timeout 缩短到可控时间，测试结束后恢复配置。
4. 请求 `POST /_bifrost/api/asr/tasks/{task_id}/daily-agent/run?date=YYYY-MM-DD&force=1`，触发 Run All Agents。
5. 在 run 未结束时再次请求同一个 Run All API。
6. 轮询 `GET /_bifrost/api/asr/tasks/{task_id}/daily-agent`，直到两个 Agent 都离开 `running` 状态。
7. 恢复第 2 步备份的原始 Daily Agent 配置。

预期结果：
- 两个 Agent 都使用同一个 ChatGPT Web runner 时，Run All 按 Agent 顺序串行执行，不会并发抢占同一个 runner。
- 运行中重复触发返回 `already_running`，不会再启动第二个同 task 的 Daily Agent 队列。
- 如果第一个 Agent 因 ChatGPT Web 登录失效、超时或其他 runner 错误失败，第二个 Agent 仍会获得自己的 `run_id` 并继续执行。
- 两个 Agent 的默认 `session_key` 和 conversation state 按 `task_id + agent_id` 隔离，避免同 task 的不同 Agent 复用同一 ChatGPT Web conversation。
- 如果测试中临时修改了配置，测试结束后原始配置被恢复，避免污染用户真实任务配置。

### TC-ADA-11 每个 Agent 工作目录包含 input 副本并按 Runner 独立落档

操作步骤：
1. 使用默认服务 `http://127.0.0.1:9900`，确认服务由当前源码重启，且 `System Proxy: Disabled`。
2. 从已有默认任务 `a911c68b0f7a43afa29d1863cc02229a` 的 `.daily` 目录复制真实 Daily Docs，例如 `2026-05-18.md` 和 `2026-05-20.md`，到一个新建临时 ASR task 的 `.daily` 目录。
3. 配置两个 Agent，分别使用 Codex 和 ChatGPT Web runner；两者均关闭 IM Delivery，并写入各自的 custom `AGENTS.md` marker。
4. 保存配置后请求 `GET /daily-agent`，检查每个 Agent 工作目录都包含 `AGENTS.md`、`input/`、`output/<output_dir>/`。
5. 对每个 Agent 分别请求 `POST /daily-agent/run?agent_id=<agent>&date=2026-05-18&force=1`，等待该 Agent 状态变为 `success`。
6. 请求 `/daily-agent/runs` 与 `/daily-agent/reports/2026-05-18?agent_id=<agent>`，并在 WebUI Daily Agent Records 页面打开报告详情。

预期结果：
- 每个 Agent 的工作目录为 `.daily/agents/<agent_id>/`，Runner cwd 指向该目录。
- 每个 Agent 的 `input/2026-05-18.md` 和 `input/2026-05-20.md` 与源 `.daily/YYYY-MM-DD.md` 逐字一致，新增 Agent 后不会缺失既有 Daily Docs 副本。
- 当源 `.daily/YYYY-MM-DD.md` 已存在但内容刷新时，下一次 workspace 初始化或运行前同步会更新每个 Agent 的 `input/YYYY-MM-DD.md`；内容一致时跳过，不刷新 mtime。
- 每个 Agent 的 report 都写入 `.daily/agents/<agent_id>/output/<output_dir>/2026-05-18-report.md`，不会写入旧版 `.daily/<output_dir>/`。
- `/daily-agent/runs` 返回两条 records，runner 分别为配置的 Codex 与 ChatGPT Web runner。
- `/daily-agent/reports/{date}` 能读取对应 report 内容；WebUI Daily Agent Records 列表和详情页都能访问内容。
- Daily Agent 编辑详情页使用 `asrDailyAgentEdit=<agent_id>` 路由，刷新后仍保持详情页，不回到列表页。

### TC-ADA-12 回归：Daily Agent 每次任务完成后自动按 Agent 分目录同步

操作步骤：
1. 执行 `BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh`。
2. 脚本创建临时 ASR task，并通过 `PUT /_bifrost/api/asr/tasks/{task_id}/daily-agent` 配置任务级 `report_sync_dir`。
3. 脚本写入 `2026-05-22.md`，调用 `POST /daily-agent/run?date=2026-05-22&force=1` 触发两个 enabled Agents。
4. 脚本等待 `daily_report` 和 `tomorrow_todo` 均生成 processed record 与 report 文件。
5. 脚本检查同步根目录和 `GET /daily-agent` 返回的每 Agent `last_report_sync`。

预期结果：
- 两个 Agent 的 report 均生成成功，且 processed records 中 `agent_id` 分别为 `daily_report` 和 `tomorrow_todo`。
- Daily Agent run 完成后无需点击 `Sync Reports`，同步根目录自动出现 `daily_report/2026-05-22-report.md` 和 `tomorrow_todo/2026-05-22-report.md`。
- 同步根目录下不存在未分目录的 `2026-05-22-report.md`，避免两个 Agent 的同名报告互相覆盖或被当作 identical target 跳过。
- `daily_report.last_report_sync.target_dir` 指向 `<sync_root>/daily_report`，`tomorrow_todo.last_report_sync.target_dir` 指向 `<sync_root>/tomorrow_todo`。
- 两个 Agent 的 `last_report_sync.total_files` 均为 1，且 `failed_files=0`。

### TC-ADA-13 Daily Agent 全局专有名词配置自动注入

操作步骤：
1. 执行 `BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh`。
2. 脚本通过 `PUT /_bifrost/api/asr/tasks/{task_id}/daily-agent` 保存任务级 `terminology`，内容包含 `Jennie = Daily Agent 专有项目名`、`Qwen3-ASR = 语音识别模型` 和 `E2E_TERMS_MARKER`。
3. 脚本请求 `GET /_bifrost/api/asr/tasks/{task_id}/daily-agent`，检查返回的 `config.terminology`。
4. 脚本检查 `.daily/agents/daily_report/TERMS.md` 与 `.daily/agents/tomorrow_todo/TERMS.md`。
5. 脚本检查两个 Agent 的 `AGENTS.md` 是否包含相对文件引用 `` `TERMS.md` ``。
6. 脚本触发两个默认 Agent 真实运行，并检查 mock model 收到的 prompt/context 中包含 `TERMS.md` 相对引用，且报告生成与按 Agent 分目录同步仍成功。
7. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_terminology --lib -- --nocapture` 和 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_prompt_uses_file_list_for_file_capable_runners --lib -- --nocapture`，验证 ChatGPT Web 首轮和后续轮次都会把术语块放在 prompt 最前面。

预期结果：
- `config.terminology` 按保存内容返回，空白不会污染配置。
- 每个 Agent 工作目录根目录都有最新 `TERMS.md`，内容包含 `E2E_TERMS_MARKER`。
- 每个 Agent 的 `AGENTS.md` 都通过托管块引用相对路径 `TERMS.md`，用户自定义指令正文不被覆盖。
- Codex 等文件型外部 Runner 的 prompt 使用相对文件引用，不内联专有名词正文。
- GPT Web Runner 首轮和后续轮次的 prompt 都以 `## 专有名词配置（每次运行动态注入）` 开头，并包含最新术语正文。
- 专有名词配置不影响两个 Agent 生成 report、processed records、IM 配置和 report sync 分目录同步。

### TC-ADA-14 回归：ChatGPT Web Daily Agent 每次运行使用新对话并稳定取回最终报告

操作步骤：
1. 打开真实任务 `76612de33e9740bc92440ce64a98a4cb` 的 Daily Agent 页面：`http://127.0.0.1:9900/_bifrost/ai?aiSection=tools-asr&asrTask=76612de33e9740bc92440ce64a98a4cb&asrTaskTab=daily-agent&asrDailyAgentEdit=daily_report`。
2. 请求 `GET /_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent`，检查 `daily_report` 的 `runner=web`、`timeout_ms=7200000`，以及历史 `last_error`。
3. 检查失败 run 目录 `/Users/eden/.bifrost/im_gateway/runs/1781646729992-4ed94466-6c30-4c99-a186-f7c4a001b83b/`，确认 `prompt.md` 约 599KB、`runtime_snapshot.json` 中存在旧 `conversationId`，且没有 `result.json`。
4. 执行 `cargo test -p bifrost-admin daily_agent_`，确认 ChatGPT Web 参数构造不再传旧 `conversationId`，Codex 仍保留 `threadId`。
5. 执行 `npx tsc --noEmit --project tsconfig.json`，确认 Daily Agent 前端轮询 timeout 改为跟随 agent 配置。
6. 使用已安装的当前构建启动默认服务：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 /Users/eden/.local/bin/bifrost start -p 9900 -d --no-system-proxy`，并确认 9900 由该进程监听。
7. 分别触发 `POST /_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/run?force=1&date=2026-06-15&agent_id=daily_report` 和 `date=2026-06-16`。
8. 轮询 `GET /daily-agent` 与 `GET /daily-agent/runs`，直到 `daily_report.last_status=success`，并检查 `processed_documents` 中 `2026-06-15` 和 `2026-06-16` 的 `daily_report` 记录都写入 `last_run_id`。
9. 请求 `GET /daily-agent/reports/2026-06-15?agent_id=daily_report` 与 `GET /daily-agent/reports/2026-06-16?agent_id=daily_report`，检查正文分别以 `# 2026-06-15 日报` / `# 2026-06-16 日报` 开头，并包含 `## 今日概览` 与 `## 证据与不确定性`。
10. 检查相关 `im_gateway/runs/*/runtime_snapshot.json` 和 `result.json`，确认首次运行 `session_key=null`、`params=null`，且 6/16 的短计划文本未写入 report，而是通过同一新 `conversationId` 纠偏重试后写入完整正文。

预期结果：
- ChatGPT Web Daily Agent 不再复用历史 `conversationId`，每个日期运行都以新 ChatGPT 对话发送。
- 每个新对话 prompt 都包含 AGENTS 指令，避免依赖旧对话上下文。
- Codex 等非 ChatGPT Web runner 仍可复用 thread，不受本回归影响。
- 前端不会在 10 分钟固定超时后提前停止轮询；轮询安全时间与后端 `timeout_ms` 对齐。
- 旧失败的根因被归类为“ChatGPT 页面可能已完成但长对话最终结果获取/扫描未返回”，而不是模型本身未完成。
- ChatGPT Web DOM fallback 不会把 `正在思考`、短流式前缀或计划说明当成最终报告；Daily Agent 只在响应通过标题、日期、关键章节和长度门禁后写入 report。
- 如果 ChatGPT Web 首次响应是计划说明或短文本，系统使用该次新对话返回的 `conversationId` 追加一次明确输出正文的纠偏重试，成功后再写入 processed state。

### TC-ADA-15 回归：ChatGPT Web Daily Agent 失败先由 adapter 收敛并持久化诊断产物

操作步骤：
1. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_runtime_persists_chatgpt_web_adapter_errors --lib -- --nocapture`。
2. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_chatgpt_web_timeout_is_bounded_below_outer_timeout --lib -- --nocapture`。
3. 检查第 1 条测试中的临时 `im_gateway/runs/<run_id>/` 产物，确认失败 run 返回 `status=failed`。
4. 检查第 2 条测试断言，确认 `daily_report.timeout_ms=7200000` 时 ChatGPT Web 内部 `timeout_secs` 被下压为 `7170`，且 runner 上显式配置的更短 timeout 不被放大。
5. 对照真实失败 run `/Users/eden/.bifrost/im_gateway/runs/1781721910933-573fb907-f5a0-4ce5-ba4d-05ad81746a84/`，确认旧问题是只有 `prompt.md` 和 `runtime_snapshot.json`，缺少 `result.json`、stdout、stderr、events 和 last message。

预期结果：
- ChatGPT Web adapter 普通错误不再让 `ExternalCliRuntime` 直接提前返回，而是生成 failed `ExternalCliRunResult`。
- 失败 run 目录包含 `result.json`、`cli.stdout.log`、`cli.stderr.log`、`normalized_events.jsonl` 和 `last_message.md`。
- stderr 和 last message 中包含真实错误摘要，`normalized_events.jsonl` 包含 `run_failed` 事件。
- 如果 `chatgpt_web` 写入了 `failure_diagnostics.json`，`result.metadata.failureDiagnostics` 指向该文件，便于 UI/API 后续定位。
- Daily Agent 外层 timeout 仍是最终兜底；对 ChatGPT Web runner，内部 timeout 会先触发，避免再次出现只剩 `prompt.md` 和 `runtime_snapshot.json` 的不可诊断 2 小时超时。

### TC-ADA-16 回归：真实安装服务重跑 2026-06-16 ChatGPT Web 日报成功

操作步骤：
1. 在仓库根目录执行 `cargo install --path crates/bifrost-cli --bin bifrost --force`，确认安装后的 `/Users/eden/.cargo/bin/bifrost` 与 `/Users/eden/.local/bin/bifrost` 为同一当前构建。
2. 设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `BIFROST_DISABLE_TRAY=1`，用已安装二进制重启真实默认服务：`/Users/eden/.cargo/bin/bifrost start -p 9900 -d --no-system-proxy`。
3. 确认 9900 监听进程是刚重启的 `bifrost`，并请求 `GET http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent`，确认 `daily_report.runner=web` 且 `timeout_ms=7200000`。
4. 强制触发真实 2026-06-16 日报：`POST /_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/run?force=1&date=2026-06-16&agent_id=daily_report`。
5. 轮询 `GET /daily-agent` 和 `GET /daily-agent/runs`，直到 `daily_report.last_status=success`，记录新的 `run_id`。
6. 请求 `GET /daily-agent/reports/2026-06-16?agent_id=daily_report`，确认 `metadata.last_run_id` 等于第 5 步的新 run id，而不是旧缓存。
7. 检查报告正文：只有一个 `# 2026-06-16 日报` 标题，包含 `## 今日概览` 与 `## 证据与不确定性`，且不是计划说明、占位文本或截断报告。
8. 检查本地 report 文件和外部同步目录文件：`.daily/agents/daily_report/output/report/2026-06-16-report.md` 与 `/Users/eden/Desktop/个人/report/daily_report/2026-06-16-report.md` 均存在，大小一致。
9. 检查最新 ChatGPT Web run 目录的 `result.json` 和 `runtime_snapshot.json`，确认 `status=succeeded`、`runtime_snapshot.timeoutSecs=7170`、`params=null`，并记录 `conversationId`。
10. 对照此前失败：旧错误 `auth_required: ChatGPT browser login window was closed before login completed` 在有效 `auth_state.json` 已捕获时不再复现；旧错误 `chatgpt_web daily report response missing required report sections for 2026-06-16` 通过同会话续写合并后不再导致最终失败。

预期结果：
- 使用安装后的真实二进制和真实 `9900` 服务重跑，而不是只跑单元测试或使用旧缓存。
- 2026-06-16 的 Daily Agent run 最终为 `success`，且 report API 的 `last_run_id` 必须等于本轮新 run id。
- 报告正文通过标题、日期、关键章节、长度和单一标题门禁；若 ChatGPT Web 返回截断正文，系统会同会话续写并合并后再写入 report。
- 登录捕获完成后浏览器窗口关闭或 CDP 断开时，只要 `auth_state.json` 有效，系统会读取已捕获登录态并继续运行。
- 自动同步报告成功，`last_report_sync.failed_files=0`。

### TC-ADA-17 回归：真实运行服务下 ChatGPT Web 日报 DOM fallback 不过早结束

操作步骤：
1. 使用当前已经运行的默认服务 `http://127.0.0.1:9900`，不要重启或替换用户正在使用的 Bifrost 进程。
2. 请求 `GET /_bifrost/api/asr/tasks`，确认存在任务 `c1c57318206c4f338f1267b7f37a81b8`，名称为 `work`，`daily_report.runner=chatgpt`，`timeout_ms=7200000`。
3. 强制触发真实日报：`POST /_bifrost/api/asr/tasks/c1c57318206c4f338f1267b7f37a81b8/daily-agent/run?force=1&date=2026-06-17&agent_id=daily_report`。
4. 轮询 `/daily-agent`、`/daily-agent/runs` 和 `/Users/eden_studio/.bifrost/logs/bifrost.YYYY-MM-DD.log`，直到本轮 run 结束。
5. 检查最新 ChatGPT Web run 目录的 `runtime_snapshot.json`、`conversation_handoff.json`、`conversation_final.json`、`result.json`、`prompt.md` 和 `last_message.md`。
6. 检查日志中 `chatgpt_web send: injected composer text via native clipboard paste path`、`injection_mode=NativeClipboardPaste`、`chatgpt_web wait_final: DOM-only mode`、`DOM content candidate waiting for stability` 和 `generation complete` 记录。
7. 对照历史问题 run，例如同一 conversation 中先返回约 14KB 报告、随后追加 run 从半句开头继续输出的情况，确认根因是否为 DOM fallback 在长输出仍可能继续时提前判定完成。

预期结果：
- 长 prompt 必须走系统剪贴板加浏览器原生粘贴路径，不能使用 CDP `Input.insertText`。
- 如果 `stream_handoff` 后短时间内拿不到后端 conversation detail，adapter 可以进入 DOM fallback，但 DOM fallback 必须把 `连接已中断` / `正在等待完整回复` 视为仍在生成，不能保存该状态文本作为最终报告。
- 对超过 12KB 的长报告，DOM fallback 需要至少等待 30 秒文本长度稳定后才判定完成；不能只等 3 秒就返回。
- 成功报告必须以 `# 2026-06-17 日报` 开头，包含 `## 今日概览` 和 `## 证据与不确定性`，且 tail 不能停在未闭合的半句或明显续写前缀。
- `conversation_final.json.source` 可以是 `dom_fallback_outcome`，但 `last_message.md` / `result.json.response` 必须是完整日报正文，不是计划说明、状态文案或截断片段。
- 本轮真实运行结束后，报告自动同步到 `/Users/eden_studio/Desktop/个人/report/daily_report/2026-06-17-report.md`，日志中 `failed_files=0`。

### TC-ADA-18 回归：ChatGPT Web Tomorrow ToDo Agent 使用明日待办契约

操作步骤：
1. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_chatgpt_web_tomorrow_todo_response_uses_todo_contract --lib -- --nocapture`。
2. 执行 `BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh`。
3. 检查 E2E 脚本中的源码哨兵确认 `daily_agent.rs` 包含 `validate_chatgpt_web_tomorrow_todo_response` 与 `上一条回复不是最终明日待办`。
4. 对照 `daily_agent.rs`，确认 `tomorrow_todo` 的 retry prompt 不包含 `今日概览`、`证据与不确定性` 或 `日报正文`。

预期结果：
- `daily_report` 仍要求 `# YYYY-MM-DD 日报`、`## 今日概览` 和 `## 证据与不确定性`。
- `tomorrow_todo` 要求 `# 明日 To Do List - YYYY-MM-DD`、`## 明天必须完成`、`## 可选推进` 和 `## 需要确认`。
- 如果 ChatGPT Web 首次对 `tomorrow_todo` 返回日报格式，后端拒绝该响应并用明日待办专用 prompt 在同 conversation 纠偏。
- 真实 Admin API + Runner E2E 仍能生成两个默认 Agent 的 report、processed records 和 report sync，不因契约分流破坏旧链路。

### TC-ADA-19 回归：Headed run 不复用历史 headless orphan browser

操作步骤：
1. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin recovered_browser_mode_must_match_requested_execution_mode --lib -- --nocapture`。
2. 执行 `BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh`。
3. 检查 E2E 脚本中的源码哨兵确认 `browser.rs` 包含 `orphaned browser mode mismatch`。
4. 在真实服务中把 ChatGPT Web runner `browser.executionMode` 切到 `headed` 后，如存在同 profile 的旧 headless orphan，下一次 run 日志应出现 mode mismatch 并重新 launch headed browser，而不是直接记录 `recovered orphaned browser` 后无窗口。

预期结果：
- 请求 headless 时只恢复命令行含 `--headless` 的 orphan browser。
- 请求 headed 时只恢复命令行不含 `--headless` 的 orphan browser。
- 无法确认 PID 或模式时不恢复该 DevTools 端口，避免把未知历史进程错误登记为当前模式。
- 真实 headed run 能启动可见浏览器窗口；不需要通过重启 Bifrost 服务来规避旧 headless 进程。

### TC-ADA-20 自动运行只处理起始日期之后的日报且 IM 只尝试最新一份

操作步骤：
1. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_automatic_ --lib -- --nocapture`。
2. 执行 `e2e-tests/tests/test_asr_daily_agents_api.sh`；脚本使用动态端口、临时数据目录、禁用系统代理与真实登录弹窗。
3. 检查 E2E 保存并读取 `auto_process_from_date`、普通 Agent `chatgpt_project_url`，同时检查无效日期返回 400。

预期结果：
- `asr_completion` 且未指定日期的自动 change plan 忽略起始日期之前的 Daily Docs，只包含起始日期及之后的文件。
- 明确指定历史日期的手动运行仍可越过自动下限，避免删除用户主动修复历史日报的能力。
- 一次自动运行即使生成多份报告，IM 也只选择日期最新的一份。
- 发送前按 Agent、日期和报告 SHA-256 持久化 attempt；同一内容即使发送失败也不会再次自动尝试，报告内容变化后可产生一次新尝试。
- 普通 Agent Project URL 规范化为 ChatGPT Project 首页，研究 fan-out 的 Project URL 行为不变。

### TC-ADA-21 正式 WebUI 可配置未来日期下限与普通 Agent Project

操作步骤：
1. 备份正式任务配置，将任务暂停，并使用当前源码构建的 Bifrost 启动 9900；确认 System Proxy 仍为 Disabled。
2. 请求 `PUT /_bifrost/api/asr/tasks/{task_id}/daily-agent`，写入 `auto_process_from_date=YYYY-MM-DD`，再用 GET 读取并确认持久化。
3. 打开任务的 Daily Agent 列表，确认 `Auto process from` 输入框显示同一日期，并显示“更早日期只对自动运行忽略、手动指定日期仍可用”的说明。
4. 打开普通 `daily_report` Agent 详情，确认存在独立 `ChatGPT Project URL` 输入框，并显示“只影响新会话、旧报告不迁移”的说明。
5. 在亮色与暗色主题之间切换，分别确认日期输入框和 Project URL 输入框可见、可读且布局不溢出。
6. 在尚未创建并绑定目标 Project 时保持任务暂停，不触发 Runner、Pro 研究或真实 IM；绑定完成后再恢复任务。

预期结果：
- 日期下限经服务重启后仍存在，正式任务在绑定 Project 前保持 paused。
- 普通 Agent Project URL 与 research fan-out Project URL 是两个独立字段，不会互相覆盖。
- WebUI 的亮色、暗色主题均可操作；修改字段需要明确保存，刷新后由后端配置回填。
- 本用例不迁移、不读取、不重新生成任何历史日报，也不发送测试微信消息。

## 执行记录

- 2026-07-17：执行 TC-ADA-20 / TC-ADA-21。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_automatic_ --lib -- --nocapture` 通过 2 个回归用例；`e2e-tests/tests/test_asr_daily_agents_api.sh` 在动态端口与临时数据目录通过，临时 task 为 `f4ed1078a1ca4199854cf86d9387082f`，确认日期下限、普通 Agent Project URL 规范化、无效日期 400、官方模板保留日期下限和完整五段研究链路。当前源码构建已安装到正式 9900，System Proxy 为 Disabled；正式任务持久化 `auto_process_from_date=2026-07-17` 且保持 paused。真实 WebUI 在 light/dark 两个主题下确认日期输入框值为 `2026-07-17`，普通 `daily_report` 详情存在独立 Project URL 输入框与“旧报告不迁移”说明；研究 fan-out 继续保留原 Project URL，普通日报 Project 尚未绑定时没有触发 Runner、Pro 研究或真实 IM。验证过程中未迁移、读取或重新生成历史日报，也未发送测试微信消息。
- 2026-07-01：执行 TC-ADA-18 / TC-ADA-19 回归验证。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_chatgpt_web_tomorrow_todo_response_uses_todo_contract --lib -- --nocapture` 通过，确认 `tomorrow_todo` 接受 `# 明日 To Do List - 2026-06-15`、`## 明天必须完成`、`## 可选推进`、`## 需要确认`，拒绝 `# 2026-06-15 日报`，且 retry prompt 不包含 `今日概览`、`证据与不确定性` 或 `日报正文`。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin recovered_browser_mode_must_match_requested_execution_mode --lib -- --nocapture` 通过，确认 recovered browser 只有实际 headless/headed 与请求模式一致时才可复用，未知模式一律拒绝恢复。随后执行 `BIFROST_E2E_PORT=19197 BIFROST_DAILY_AGENT_MOCK_PORT=19198 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh` 通过，临时 task 为 `2e9972e09c1f45909a0b73835ea02715`，验证真实 Admin API + Runner 链路仍能生成 `daily_report` 与 `tomorrow_todo` 两个默认 Agent 的 report、processed records 和按 Agent 分目录 report sync；脚本源码哨兵确认 `daily_agent.rs` 包含 `validate_chatgpt_web_tomorrow_todo_response` 与 `上一条回复不是最终明日待办`，`browser.rs` 包含 `orphaned browser mode mismatch`。
- 2026-06-18：执行 TC-ADA-17 真实运行服务回归验证。当前运行服务为 `/Users/eden_studio/.local/bin/bifrost 0.0.107`，`status --format json` 显示监听 `0.0.0.0:9900`，数据目录为 `/Users/eden_studio/.bifrost`；本轮按用户要求没有重启服务。任务 `c1c57318206c4f338f1267b7f37a81b8` 名称为 `work`，`daily_report.runner=chatgpt`，`timeout_ms=7200000`。先检查历史 run，发现同一 ChatGPT conversation `6a33f937-eaf0-83ec-9e44-9dd5b14b4336` 中 `1781791097556-6c4255ec-400f-47c9-a45f-de584021bed4` 返回约 14KB 且 tail 停在未完成表述，随后 `1781791307109-1c12c5d9-d314-40b5-9666-dfe8ab23a682` 从半句开头继续输出，证明旧问题不是输入粘贴失败，而是最终输出检查过早。日志还显示旧链路在 `stream_handoff` 后进入 `DOM-only mode`，遇到 `连接已中断。正在等待完整回复` 状态但未作为 in-progress 处理。随后触发真实 `2026-06-17` `daily_report`，外层 run `1781793348962-2a2bb9e1-cd66-48de-aff4-12f012372d62` 成功；ChatGPT Web 子 run `1781793348984-635cab44-13f5-4c92-be02-1dcb8aa3be3f` 的 `prompt.md` 为 114172 bytes，日志明确 `injection_mode=NativeClipboardPaste` 并走 native clipboard paste path，`conversationId=6a34025b-0158-83ec-90c4-6168fdb3f3a4`，`conversation_final.source=dom_fallback_outcome`。DOM fallback 文本从 41 持续增长到 39450 bytes，旧代码只等待 `required_stable_ms=3000` 后返回；本次代码修复把 12KB 以上长文本稳定窗口提升到 30 秒，并新增 `waitingForCompleteReply` 状态识别。最终报告保存到 `.daily/agents/daily_report/output/report/2026-06-17-report.md`，日志显示 `len=39450`、同步 `copied_files=1 failed_files=0`，`result.json.status=succeeded`，正文以 `# 2026-06-17 日报` 开头，包含 `## 今日概览` 与 `## 证据与不确定性`，tail 为完整不确定性列表。额外执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin chatgpt_web::interaction::tests --lib` 通过，覆盖长文本稳定窗口、`连接已中断。正在等待完整回复` pending 状态和 `waiting_for_complete_reply` in-progress 原因。
- 2026-06-18：执行 TC-ADA-16 真实安装回归验证。先执行 `cargo install --path crates/bifrost-cli --bin bifrost --force` 安装 `bifrost 0.0.106`，并确认 `/Users/eden/.cargo/bin/bifrost` 与 `/Users/eden/.local/bin/bifrost` 的 sha256 均为 `4a5519bd6348f67357002609a56989afa5ea71cecbca25c87a8a4cdc260a79fd`。随后设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `BIFROST_DISABLE_TRAY=1`，用 `/Users/eden/.cargo/bin/bifrost start -p 9900 -d --no-system-proxy` 重启真实服务，最终监听 PID 为 `63967`。旧失败链路先复现并定位出两类新问题：一次在有效 `auth_state.json` 已写入后仍因 `auth_required: ChatGPT browser login window was closed before login completed` 失败，确认是登录捕获完成后浏览器关闭/CDP 断开的竞态；另一次 ChatGPT Web 子 run 成功但响应截断在“灵感：把复杂甬道/代理封装成环境切换挂件”，缺少 `## 证据与不确定性`，Daily Agent 报错 `chatgpt_web daily report response missing required report sections for 2026-06-16`。修复后重新安装并重启真实 9900，强制触发 `2026-06-16` `daily_report`，run `1781745903066-18fb5379-7c81-4107-9a7c-beb9f75bb608` 成功；报告 API 的 `last_run_id` 等于该新 run id，report size 为 `44451`，`last_report_sync` 显示 `copied_files=1`、`failed_files=0`、`target_dir=/Users/eden/Desktop/个人/report/daily_report`。报告正文第 1 行是唯一的 `# 2026-06-16 日报`，第 5 行包含 `## 今日概览`，第 567 行包含 `## 证据与不确定性`；本地 `.daily/agents/daily_report/output/report/2026-06-16-report.md` 与桌面同步文件均为 `44451` bytes，mtime 为 2026-06-18 09:28:13。最新 ChatGPT Web 子 run `1781745903120-f2c8055f-4d6f-4d96-9c9d-7ba571cb4ec8` 的 `result.json.status=succeeded`、`durationMs=190569`、`responseLen=18751`，metadata `conversationId=6a334908-0790-83ec-b1cd-7d4f2e3cc941`，`runtime_snapshot.timeoutSecs=7170` 且 `params=null`。验证结论：本次修复不是只“看起来可用”，而是在安装后的真实服务上重跑 6 月 16 日报告成功，并证明登录捕获竞态和长报告截断续写两个实际失败点均被覆盖。
- 2026-06-18：执行 TC-ADA-15 回归验证。旧失败 run `1781721910933-573fb907-f5a0-4ce5-ba4d-05ad81746a84` 只包含 `prompt.md` 与 `runtime_snapshot.json`，`runtime_snapshot.json.timeoutSecs=720000`，Daily Agent 外层报错为 `daily agent run timed out after 7200000ms`，确认旧链路由外层 timeout 截断且没有失败结果产物。修复后执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_runtime_persists_chatgpt_web_adapter_errors --lib -- --nocapture` 通过，验证 ChatGPT Web 普通错误会落盘 failed `result.json`、stderr、events、last message 和 `failureDiagnostics` metadata；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_chatgpt_web_timeout_is_bounded_below_outer_timeout --lib -- --nocapture` 通过，验证 2 小时 Daily timeout 下 Web adapter 内部 timeout 被压到 7170 秒，且不会放大 runner 显式更短 timeout。
- 2026-06-17：执行 TC-ADA-14 回归验证。`GET /_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent` 返回 `daily_report.runner=web`、`timeout_ms=7200000`、`last_error="daily agent run timed out after 7200000ms"`；失败 run 目录 `1781646729992-4ed94466-6c30-4c99-a186-f7c4a001b83b` 中 `prompt.md` 为 599132 bytes，`runtime_snapshot.json` 显示旧 `conversationId=6a22320b-0688-83ec-a25d-4e544fa281c5`，且没有 `result.json` / stdout / stderr，证明旧链路是 ChatGPT Web 长对话最终结果获取/扫描未返回，不能简单归因为模型未完成或 Rust 提前退出。修复后执行 `cargo fmt --check`、`cargo test -p bifrost-admin daily_agent_`、`npx tsc --noEmit --project tsconfig.json` 均通过；新增单测确认 ChatGPT Web Daily Agent 参数为 `null`，不再传旧 `conversationId`，Codex 仍保留 `threadId`；前端轮询安全时间改为 `timeout_ms + 60s`。进一步真实验证：使用已安装二进制 `8496cc1a4d9faae7e94c50703f208cc5a8e8ba1acb5ce9f7cef6c39978aa24bc` 启动 `9900`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。`2026-06-15` run `1781665446287-de3ab9fe-e751-4d6e-86a3-db03dc18d362` 成功，ChatGPT Web run `1781665446328-7596e8b2-40e0-44a9-aa09-fef1d239ec7d` 的 `runtime_snapshot` 显示 `session_key=null`、`params=null`，返回 `conversationId=6a320ecf-2efc-83ec-85e4-0d160c45ce8f`，`result.response_len=18013`，报告文件 `2026-06-15-report.md` 为 42671 bytes，详情 API 正文以 `# 2026-06-15 日报` 开头并包含 `## 今日概览`、`## 证据与不确定性`。`2026-06-16` run `1781666244340-86159d97-bfab-4107-9a39-d5d4907943fd` 首次 ChatGPT Web run `1781666244394-96b84375-a0d5-4aa0-bbf8-d190c6a90260` 返回 64 字计划说明，未写入 report；后端响应门禁记录 `response too short` 并用同一个新对话 `conversationId=6a3211d8-992c-83ec-b516-9a593002d210` 触发纠偏重试，retry run `1781666296564-51f1323c-b29e-4be3-9f93-4a1026989e1f` 成功返回 `response_len=16624`，报告文件 `2026-06-16-report.md` 为 39546 bytes，详情 API 正文以 `# 2026-06-16 日报` 开头并包含两个关键章节。`daily_agent_processed.json` 中 `daily_report:2026-06-15` 和 `daily_report:2026-06-16` 均写入对应 `last_run_id`、`source_sha256` 和新 `.daily/agents/daily_report/output/report/` 路径；日志显示 6/15 `status=success reports=1 duration_ms=309205`，6/16 `status=success reports=1 duration_ms=303248`。最终安装后再次验证：`cargo install --path crates/bifrost-cli --bin bifrost --force` 成功安装 `bifrost 0.0.104`，并同步 `/Users/eden/.local/bin/bifrost`，两者 sha256 均为 `1b7e923803427be7d002c0607ced36dda513fabda9e836ca1f38f6652caa2b3d`；随后用 `/Users/eden/.local/bin/bifrost start -p 9900 -d --no-system-proxy` 重启服务。重新 force 触发 `2026-06-15` 后，run `1781669626946-b731f211-9719-4c7e-bd97-e4f8763aaebf` 成功，ChatGPT Web run `1781669626970-6dcd0ff3-d354-4c52-bd05-876ad4f0060f` 的 `runtime_snapshot` 显示 `params=null`，`conversationId=6a321f19-053c-83ec-9123-f024807112c4`，`result.response_len=21559`，详情 API 正文以 `# 2026-06-15 日报` 开头并包含关键章节，且不是计划或占位文本。重新 force 触发 `2026-06-16` 后，run `1781669861081-f7f55eaa-a74e-45b0-9cb7-9b95a99ac363` 成功，ChatGPT Web run `1781669861101-87f2c319-6d21-4240-aafa-578616351f6f` 的 `runtime_snapshot` 显示 `params=null`，`conversationId=6a321ffa-4d54-83ec-8749-c9d7838f7ef8`，`result.response_len=17199`，详情 API 正文以 `# 2026-06-16 日报` 开头并包含关键章节，且不是计划或占位文本。`/_bifrost/api/asr/tasks/.../daily-agent/runs` 返回两天 `daily_report` processed records，分别指向新 run id、`source_sha256` 和 `.daily/agents/daily_report/output/report/` 路径。当前不稳定因素已收敛为 ChatGPT Web 可能返回计划说明、占位状态或流式短前缀；代码通过 DOM 稳定窗口、占位过滤、最终报告门禁和一次同会话纠偏重试兜底。
- 2026-06-05：执行 TC-ADA-13 回归验证。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_terminology --lib -- --nocapture` 通过，确认全局术语会在 normalize 后保留，并在 `task_for_daily_agent` 派生单 Agent task 时继承；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_prompt_uses_file_list_for_file_capable_runners --lib -- --nocapture` 通过，确认文件型 Runner prompt 只引用相对 `TERMS.md`，ChatGPT Web 首轮和后续轮次都以 `## 专有名词配置（每次运行动态注入）` 开头并包含术语正文；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_workspace_creates_per_agent_instruction_and_output_dirs --lib -- --nocapture` 通过，确认两个默认 Agent 都写入 `TERMS.md` 且 `AGENTS.md` 包含相对引用；`BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh` 复跑通过，最终临时 task 为 `94b31bc914364597892f5c459e8e4fd3`，验证 API 保存 `terminology`、两个 Agent 工作目录生成 `TERMS.md`、`AGENTS.md` 引用 `TERMS.md`、真实 run 生成 report 并继续按 Agent 分目录自动同步。
- 2026-06-05：执行 TC-ADA-12 回归验证。首次运行 `BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh` 时，产品已成功自动同步 `daily_report` 和 `tomorrow_todo`，但脚本用逐字路径比较误判了包含 `T//` 的临时目录；修正为 `os.path.normpath` 后复跑通过，临时 task 为 `f9d476d4aaa8412cbb3f3432712dd079`。验证结果：两个 Agent 跑完后无需点击 `Sync Reports`，同步根目录自动出现 `daily_report/2026-05-22-report.md` 和 `tomorrow_todo/2026-05-22-report.md`，根目录下不存在未分目录的 `2026-05-22-report.md`，两个 Agent 的 `last_report_sync.target_dir` 分别指向各自子目录且 `total_files=1`、`failed_files=0`。
- 2026-06-04：执行 TC-ADA-06 回归验证。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_after_asr_run_requires_changed_daily_markdown -- --nocapture`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_after_asr_run_ignores_failed_files_when_daily_markdown_changed -- --nocapture`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_after_asr_run_checks_all_agents_for_pending_markdown_changes -- --nocapture` 均通过；`BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh` 通过，首轮临时 task 为 `f7cb1db771ef436d891a97d6d683f227`，第二轮复跑临时 task 为 `1ff9f0947a3e46bcb6f9bfd7817bf0da`。确认 ASR run 完成后以 daily 合成 Markdown 变更作为 Daily Agent 自动触发门禁；普通 failed 与 `diarization_no_asr_units` failed 不再阻塞已经刷新出的 daily report 后处理；无 daily 变更时仍跳过；非 force 的 appended daily Markdown 会推进两个 Agent 的 processed run_id 更新。
- 2026-06-03：执行 TC-ADA-07 / TC-ADA-08 WebUI 验证。使用 Playwright 打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=a911c68b0f7a43afa29d1863cc02229a&asrTaskTab=daily-agent`，确认 Daily Agent 首屏仅展示列表列，不出现 `Agent Configuration` / `Agent Instructions` 详情卡；点击 `daily_report` 行 `Edit` 后进入详情页，并显示 `Agent Configuration`、`IM Delivery`、`Last Run Status`、`Agent Instructions` 与返回按钮；900px 窄窗口下表格滚动容器 `clientWidth=742`、`scrollWidth=1180`，确认横向滚动替代竖排折行。另打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr`，确认顶层 tab 为 `Scheduled Tasks`、`ASR Management`、`Voiceprint & Wake`，旧中文 tab 不再出现。
- 2026-06-03：执行 TC-ADA-09 WebUI 验证。使用 Playwright 打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=a911c68b0f7a43afa29d1863cc02229a&asrTaskTab=daily`，拦截 `POST /daily-agent/run` 避免真实触发任务。点击行级主按钮 `Run All Agents` 后，请求为 `/daily-agent/run?date=2026-05-20`，确认省略 `agent_id` 表示全部 Agent；打开右侧下拉菜单，菜单包含 `Run All Agents`、`Run daily_report`、`Run tomorrow_todo`；选择 `Run daily_report` 后，请求为 `/daily-agent/run?date=2026-05-20&agent_id=daily_report`。
- 2026-06-03：执行 TC-ADA-10 真实 ChatGPT Web Runner 失败隔离验证。当前 `abc` runner 为 `chatgpt_web`，`daily_report` 与 `tomorrow_todo` 均临时使用 `abc`，timeout 缩短为 15000ms，IM Delivery 关闭；ChatGPT Web auth status 为 `auth_required`，`accountStatus=401`，提示 authorization token 已于 `2026-06-03 03:24:20 UTC` 过期。触发 `POST /daily-agent/run?date=2026-05-20&force=1` 后返回 `queued`；运行中再次触发返回 `already_running`。轮询结果显示 `daily_report` 生成新 `run_id=1780457232354-07553a92-e688-43b9-90d2-671adcd6d47c` 并因 `daily agent run timed out after 15000ms` 失败，随后 `tomorrow_todo` 也生成独立新 `run_id=1780457247477-662ecfbe-2d64-43ea-80ec-9da7360d06af` 并同样超时失败，确认第一个 ChatGPT Web runner 失败没有阻断第二个 Agent 执行；脚本最后已恢复原始 Daily Agent 配置。
- 2026-06-03：执行 TC-ADA-10 真实 ChatGPT Web Runner 成功链路验证。用户完成登录后，`GET /im-gateway/chat/adapters/chatgpt-web/auth/status?runnerId=abc` 返回 `state=logged_in`、`loggedIn=true`、`identityComplete=true`、`accountCheckOk=true`、`accountStatus=200`，确认后端登录检查成功态是 `logged_in` 而不是 `ready`。触发 `POST /daily-agent/run?date=2026-05-20&force=1` 返回 `queued`，运行中再次触发返回 `already_running`。轮询显示 `daily_report` 先进入 `running` 并以 `run_id=1780457629759-4577aa35-096d-43fb-8a41-55790f68345b` 成功结束；随后 `tomorrow_todo` 才进入 `running` 并以 `run_id=1780457946462-79723d55-d02f-49d1-a49e-551fa0044b27` 成功结束。`/daily-agent/runs` 返回 `2026-05-20` 两条新 processed records，分别写入 `.daily/report/2026-05-20-report.md` 和 `.daily/tomorrow_todo/2026-05-20-report.md`，确认两个 Agent 共用同一 ChatGPT Web runner 时串行运行、不会抢占，且成功产出各自报告。
- 2026-06-03：补充执行同 task 的差异复制验证。先确认 `codex_io/input/2026-05-20.md` 与源文件一致；将源 `.daily/2026-05-20.md` 修改 1 个字节且保持文件长度不变，再调用 `GET /daily-agent` 触发 workspace sync，确认 input sha256 从 `abea9dcd...` 更新为 `d203c43f...`；随后恢复源文件，再次 sync 后 input sha256 恢复为 `abea9dcd...`，证明同步判断会比较内容而不是只看文件是否存在。
- 2026-06-03：补充执行历史任务兼容验证。使用默认服务打开历史任务 `a911c68b0f7a43afa29d1863cc02229a` 的 Daily Agent 页面，触发 workspace 初始化后，确认旧 `daily_report/AGENTS.md` 从“源文件位于当前目录”迁移为 `input/YYYY-MM-DD.md`，旧 `tomorrow_todo/AGENTS.md` 从“当前目录根部”迁移为 `input/YYYY-MM-DD.md`，输出目录迁移为 `./output/tomorrow_todo/`。逐字比对确认该任务所有顶层 Daily Docs 与两个 Agent 的 `input/` 副本一致。随后调用 `POST /daily-agent/run?date=2026-05-14&agent_id=tomorrow_todo&force=1`，返回 queued，轮询状态从 running 到 success，报告落档到 `.daily/agents/tomorrow_todo/output/tomorrow_todo/2026-05-14-report.md`，records 中 `report_path` 也指向该新路径。

## 清理步骤

- 成功执行时，脚本自动停止测试服务并删除临时数据目录、临时音频目录。
- 失败时，脚本保留临时目录并在 stderr 打印 `server.log`，用于排查。
