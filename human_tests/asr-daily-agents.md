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
3. 在临时 task 上配置三个 Agent：`builtin_io` 使用 `bifrost_agent`，`codex_io` 使用 `codex` runner，`chatgpt_io` 使用 ChatGPT Web runner `abc`；三个 Agent 均关闭 IM Delivery，并写入各自的 custom `AGENTS.md` marker。
4. 保存配置后请求 `GET /daily-agent`，检查每个 Agent 工作目录都包含 `AGENTS.md`、`input/`、`output/<output_dir>/`。
5. 对每个 Agent 分别请求 `POST /daily-agent/run?agent_id=<agent>&date=2026-05-18&force=1`，等待该 Agent 状态变为 `success`。
6. 请求 `/daily-agent/runs` 与 `/daily-agent/reports/2026-05-18?agent_id=<agent>`，并在 WebUI Daily Agent Records 页面打开报告详情。

预期结果：
- 每个 Agent 的工作目录为 `.daily/agents/<agent_id>/`，Runner cwd 指向该目录。
- 每个 Agent 的 `input/2026-05-18.md` 和 `input/2026-05-20.md` 与源 `.daily/YYYY-MM-DD.md` 逐字一致，新增 Agent 后不会缺失既有 Daily Docs 副本。
- 当源 `.daily/YYYY-MM-DD.md` 已存在但内容刷新时，下一次 workspace 初始化或运行前同步会更新每个 Agent 的 `input/YYYY-MM-DD.md`；内容一致时跳过，不刷新 mtime。
- 每个 Agent 的 report 都写入 `.daily/agents/<agent_id>/output/<output_dir>/2026-05-18-report.md`，不会写入旧版 `.daily/<output_dir>/`。
- `/daily-agent/runs` 返回三条 records，runner 分别为 `bifrost_agent`、`codex`、`abc`，且 `report_path` 都指向各自 `output/<output_dir>/`。
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
- Bifrost Agent/Codex 等文件型 Runner 的 prompt 使用相对文件引用，不内联专有名词正文。
- GPT Web Runner 首轮和后续轮次的 prompt 都以 `## 专有名词配置（每次运行动态注入）` 开头，并包含最新术语正文。
- 专有名词配置不影响两个 Agent 生成 report、processed records、IM 配置和 report sync 分目录同步。

### TC-ADA-14 回归：ChatGPT Web Daily Agent 每次运行使用新对话

操作步骤：
1. 打开真实任务 `76612de33e9740bc92440ce64a98a4cb` 的 Daily Agent 页面：`http://127.0.0.1:9900/_bifrost/ai?aiSection=tools-asr&asrTask=76612de33e9740bc92440ce64a98a4cb&asrTaskTab=daily-agent&asrDailyAgentEdit=daily_report`。
2. 请求 `GET /_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent`，检查 `daily_report` 的 `runner=web`、`timeout_ms=7200000`，以及历史 `last_error`。
3. 检查失败 run 目录 `/Users/eden/.bifrost/im_gateway/runs/1781646729992-4ed94466-6c30-4c99-a186-f7c4a001b83b/`，确认 `prompt.md` 约 599KB、`runtime_snapshot.json` 中存在旧 `conversationId`，且没有 `result.json`。
4. 执行 `cargo test -p bifrost-admin daily_agent_`，确认 ChatGPT Web 参数构造不再传旧 `conversationId`，Codex 仍保留 `threadId`。
5. 执行 `npx tsc --noEmit --project tsconfig.json`，确认 Daily Agent 前端轮询 timeout 改为跟随 agent 配置。

预期结果：
- ChatGPT Web Daily Agent 不再复用历史 `conversationId`，每个日期运行都以新 ChatGPT 对话发送。
- 每个新对话 prompt 都包含 AGENTS 指令，避免依赖旧对话上下文。
- Codex 等非 ChatGPT Web runner 仍可复用 thread，不受本回归影响。
- 前端不会在 10 分钟固定超时后提前停止轮询；轮询安全时间与后端 `timeout_ms` 对齐。
- 旧失败的根因被归类为“ChatGPT 页面可能已完成但长对话最终结果获取/扫描未返回”，而不是模型本身未完成。

## 执行记录

- 2026-06-17：执行 TC-ADA-14 回归验证。`GET /_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent` 返回 `daily_report.runner=web`、`timeout_ms=7200000`、`last_error="daily agent run timed out after 7200000ms"`；失败 run 目录 `1781646729992-4ed94466-6c30-4c99-a186-f7c4a001b83b` 中 `prompt.md` 为 599132 bytes，`runtime_snapshot.json` 显示旧 `conversationId=6a22320b-0688-83ec-a25d-4e544fa281c5`，且没有 `result.json` / stdout / stderr，证明 Rust 等外部 ChatGPT Web runner 等满 2 小时未取回最终结果。修复后执行 `cargo fmt --check`、`cargo test -p bifrost-admin daily_agent_`、`npx tsc --noEmit --project tsconfig.json` 均通过；新增单测确认 ChatGPT Web Daily Agent 参数为 `null`，不再传旧 `conversationId`，Codex 仍保留 `threadId`；前端轮询安全时间改为 `timeout_ms + 60s`。
- 2026-06-05：执行 TC-ADA-13 回归验证。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_terminology --lib -- --nocapture` 通过，确认全局术语会在 normalize 后保留，并在 `task_for_daily_agent` 派生单 Agent task 时继承；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_prompt_uses_file_list_for_file_capable_runners --lib -- --nocapture` 通过，确认文件型 Runner prompt 只引用相对 `TERMS.md`，ChatGPT Web 首轮和后续轮次都以 `## 专有名词配置（每次运行动态注入）` 开头并包含术语正文；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_workspace_creates_per_agent_instruction_and_output_dirs --lib -- --nocapture` 通过，确认两个默认 Agent 都写入 `TERMS.md` 且 `AGENTS.md` 包含相对引用；`BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh` 复跑通过，最终临时 task 为 `94b31bc914364597892f5c459e8e4fd3`，验证 API 保存 `terminology`、两个 Agent 工作目录生成 `TERMS.md`、`AGENTS.md` 引用 `TERMS.md`、真实 run 生成 report 并继续按 Agent 分目录自动同步。
- 2026-06-05：执行 TC-ADA-12 回归验证。首次运行 `BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh` 时，产品已成功自动同步 `daily_report` 和 `tomorrow_todo`，但脚本用逐字路径比较误判了包含 `T//` 的临时目录；修正为 `os.path.normpath` 后复跑通过，临时 task 为 `f9d476d4aaa8412cbb3f3432712dd079`。验证结果：两个 Agent 跑完后无需点击 `Sync Reports`，同步根目录自动出现 `daily_report/2026-05-22-report.md` 和 `tomorrow_todo/2026-05-22-report.md`，根目录下不存在未分目录的 `2026-05-22-report.md`，两个 Agent 的 `last_report_sync.target_dir` 分别指向各自子目录且 `total_files=1`、`failed_files=0`。
- 2026-06-04：执行 TC-ADA-06 回归验证。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_after_asr_run_requires_changed_daily_markdown -- --nocapture`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_after_asr_run_ignores_failed_files_when_daily_markdown_changed -- --nocapture`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_after_asr_run_checks_all_agents_for_pending_markdown_changes -- --nocapture` 均通过；`BIFROST_E2E_PORT=18997 BIFROST_DAILY_AGENT_MOCK_PORT=18998 SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_asr_daily_agents_api.sh` 通过，首轮临时 task 为 `f7cb1db771ef436d891a97d6d683f227`，第二轮复跑临时 task 为 `1ff9f0947a3e46bcb6f9bfd7817bf0da`。确认 ASR run 完成后以 daily 合成 Markdown 变更作为 Daily Agent 自动触发门禁；普通 failed 与 `diarization_no_asr_units` failed 不再阻塞已经刷新出的 daily report 后处理；无 daily 变更时仍跳过；非 force 的 appended daily Markdown 会推进两个 Agent 的 processed run_id 更新。
- 2026-06-03：执行 TC-ADA-07 / TC-ADA-08 WebUI 验证。使用 Playwright 打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=a911c68b0f7a43afa29d1863cc02229a&asrTaskTab=daily-agent`，确认 Daily Agent 首屏仅展示列表列，不出现 `Agent Configuration` / `Agent Instructions` 详情卡；点击 `daily_report` 行 `Edit` 后进入详情页，并显示 `Agent Configuration`、`IM Delivery`、`Last Run Status`、`Agent Instructions` 与返回按钮；900px 窄窗口下表格滚动容器 `clientWidth=742`、`scrollWidth=1180`，确认横向滚动替代竖排折行。另打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr`，确认顶层 tab 为 `Scheduled Tasks`、`ASR Management`、`Voiceprint & Wake`，旧中文 tab 不再出现。
- 2026-06-03：执行 TC-ADA-09 WebUI 验证。使用 Playwright 打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=a911c68b0f7a43afa29d1863cc02229a&asrTaskTab=daily`，拦截 `POST /daily-agent/run` 避免真实触发任务。点击行级主按钮 `Run All Agents` 后，请求为 `/daily-agent/run?date=2026-05-20`，确认省略 `agent_id` 表示全部 Agent；打开右侧下拉菜单，菜单包含 `Run All Agents`、`Run daily_report`、`Run tomorrow_todo`；选择 `Run daily_report` 后，请求为 `/daily-agent/run?date=2026-05-20&agent_id=daily_report`。
- 2026-06-03：执行 TC-ADA-10 真实 ChatGPT Web Runner 失败隔离验证。当前 `abc` runner 为 `chatgpt_web`，`daily_report` 与 `tomorrow_todo` 均临时使用 `abc`，timeout 缩短为 15000ms，IM Delivery 关闭；ChatGPT Web auth status 为 `auth_required`，`accountStatus=401`，提示 authorization token 已于 `2026-06-03 03:24:20 UTC` 过期。触发 `POST /daily-agent/run?date=2026-05-20&force=1` 后返回 `queued`；运行中再次触发返回 `already_running`。轮询结果显示 `daily_report` 生成新 `run_id=1780457232354-07553a92-e688-43b9-90d2-671adcd6d47c` 并因 `daily agent run timed out after 15000ms` 失败，随后 `tomorrow_todo` 也生成独立新 `run_id=1780457247477-662ecfbe-2d64-43ea-80ec-9da7360d06af` 并同样超时失败，确认第一个 ChatGPT Web runner 失败没有阻断第二个 Agent 执行；脚本最后已恢复原始 Daily Agent 配置。
- 2026-06-03：执行 TC-ADA-10 真实 ChatGPT Web Runner 成功链路验证。用户完成登录后，`GET /im-gateway/chat/adapters/chatgpt-web/auth/status?runnerId=abc` 返回 `state=logged_in`、`loggedIn=true`、`identityComplete=true`、`accountCheckOk=true`、`accountStatus=200`，确认后端登录检查成功态是 `logged_in` 而不是 `ready`。触发 `POST /daily-agent/run?date=2026-05-20&force=1` 返回 `queued`，运行中再次触发返回 `already_running`。轮询显示 `daily_report` 先进入 `running` 并以 `run_id=1780457629759-4577aa35-096d-43fb-8a41-55790f68345b` 成功结束；随后 `tomorrow_todo` 才进入 `running` 并以 `run_id=1780457946462-79723d55-d02f-49d1-a49e-551fa0044b27` 成功结束。`/daily-agent/runs` 返回 `2026-05-20` 两条新 processed records，分别写入 `.daily/report/2026-05-20-report.md` 和 `.daily/tomorrow_todo/2026-05-20-report.md`，确认两个 Agent 共用同一 ChatGPT Web runner 时串行运行、不会抢占，且成功产出各自报告。
- 2026-06-03：执行 TC-ADA-11 默认服务真实三 Runner 验证。先用当前源码重启默认 `9900` 服务，确认 `System Proxy: Disabled`；创建临时 task `158a1e559db74e93a91eea697dcb9a5d`，从默认任务 `a911c68b0f7a43afa29d1863cc02229a` 复制真实 Daily Docs `2026-05-18.md` 与 `2026-05-20.md`。保存三个 Agent 后，逐字比对确认 `builtin_io`、`codex_io`、`chatgpt_io` 的 `input/2026-05-18.md` 和 `input/2026-05-20.md` 均与源文件一致。分别运行 `bifrost_agent`、`codex`、ChatGPT Web `abc`，三者均成功，records 的 `report_path` 分别为 `.daily/agents/builtin_io/output/builtin_io/2026-05-18-report.md`、`.daily/agents/codex_io/output/codex_io/2026-05-18-report.md`、`.daily/agents/chatgpt_io/output/chatgpt_io/2026-05-18-report.md`。Playwright 打开 `http://127.0.0.1:3000/_bifrost/ai?...asrTask=158a1e559db74e93a91eea697dcb9a5d&asrTaskTab=daily-agent-records`，确认三条记录可见；打开 ChatGPT report 详情能看到 `REAL_IO_TEST_CHATGPT_OUTPUT` 和 `REAL_IO_TEST_REAL_DAILY_SOURCE`；打开 `asrDailyAgentEdit=codex_io` 并刷新后仍保持 Agent 详情页。
- 2026-06-03：补充执行同 task 的差异复制验证。先确认 `codex_io/input/2026-05-20.md` 与源文件一致；将源 `.daily/2026-05-20.md` 修改 1 个字节且保持文件长度不变，再调用 `GET /daily-agent` 触发 workspace sync，确认 input sha256 从 `abea9dcd...` 更新为 `d203c43f...`；随后恢复源文件，再次 sync 后 input sha256 恢复为 `abea9dcd...`，证明同步判断会比较内容而不是只看文件是否存在。
- 2026-06-03：补充执行历史任务兼容验证。使用默认服务打开历史任务 `a911c68b0f7a43afa29d1863cc02229a` 的 Daily Agent 页面，触发 workspace 初始化后，确认旧 `daily_report/AGENTS.md` 从“源文件位于当前目录”迁移为 `input/YYYY-MM-DD.md`，旧 `tomorrow_todo/AGENTS.md` 从“当前目录根部”迁移为 `input/YYYY-MM-DD.md`，输出目录迁移为 `./output/tomorrow_todo/`。逐字比对确认该任务所有顶层 Daily Docs 与两个 Agent 的 `input/` 副本一致。随后调用 `POST /daily-agent/run?date=2026-05-14&agent_id=tomorrow_todo&force=1`，返回 queued，轮询状态从 running 到 success，报告落档到 `.daily/agents/tomorrow_todo/output/tomorrow_todo/2026-05-14-report.md`，records 中 `report_path` 也指向该新路径。

## 清理步骤

- 成功执行时，脚本自动停止测试服务并删除临时数据目录、临时音频目录。
- 失败时，脚本保留临时目录并在 stderr 打印 `server.log`，用于排查。
