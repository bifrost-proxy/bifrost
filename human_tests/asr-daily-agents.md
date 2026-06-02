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

## 清理步骤

- 成功执行时，脚本自动停止测试服务并删除临时数据目录、临时音频目录。
- 失败时，脚本保留临时目录并在 stderr 打印 `server.log`，用于排查。
