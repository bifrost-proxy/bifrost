# Daily Agent Records 测试用例

## 功能模块说明

Daily Agent Records 是 ASR 任务详情页中查看 Daily Agent 历史运行产物的记录页。它必须展示 `daily_agent_processed.json` 中已处理文档，也必须兜底发现任务 Daily 工作区中已经存在的 report 文件，避免历史任务只有 `daily/report/` 或 `daily/Report/` 文件但页面显示空状态。

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

## 清理步骤

1. 停止测试端口上的 Bifrost 进程。
2. 删除临时 `BIFROST_DATA_DIR`。

## 执行记录

| 日期 | 用例 | 命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-05-20 | TC-DAR-01 Report 目录历史报告兜底发现回归 | `SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR=/tmp/bifrost-dar-human.RzRlVx cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy -y`；创建临时 ASR task `dfcd83a68a744307b8ef56edfc58d7f4`，将 task daily 工作区下 `report` 重命名为真实 `Report`，写入 `Report/2026-05-14-report.md` 且不创建 `daily_agent_processed.json`；请求 `/_bifrost/api/asr/tasks/<task_id>/daily-agent/runs`、`/_bifrost/api/asr/tasks/<task_id>/daily-agent/reports/2026-05-14`、`/_bifrost/api/asr/tasks/<task_id>/daily-agent/reports/%2E%2E%2Fsecret` | PASS：`processed_documents` 返回 1 条 `date=2026-05-14`，`runner=filesystem`，`report_path` 指向 `daily/Report/2026-05-14-report.md`；详情接口返回同一 `Report` 路径且正文包含 `Historical Daily Report`；路径穿越日期返回 400 |
| 2026-05-20 | TC-DAR-02 processed state 元数据优先且补齐 report_path | 同一真实服务与临时数据目录；创建临时 ASR task `aa81bf23510c4eb2b727c42c8ba93514`，写入 `daily/report/2026-05-15-report.md` 与 `asr/tasks/<task_id>/daily_agent_processed.json`，其中 `runner=web`、`last_run_id=run-1`、`report_path=null`；请求 `/_bifrost/api/asr/tasks/<task_id>/daily-agent/runs` | PASS：响应 1 条记录，没有重复行；保留 `runner=web`、`last_run_id=run-1`、`processed_at_ms=100`、`source_sha256=abc123`；`report_path` 补齐为 `daily/report/2026-05-15-report.md` |
