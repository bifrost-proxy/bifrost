# ASR Daily Agent 依赖编排与研究流水线

## 功能模块说明


## 前置条件

- 使用独立临时 `BIFROST_DATA_DIR` 和开发端口启动当前源码，必须带 `--no-system-proxy`。
- 不修改或停止正在运行的 `9900` 实例。

## 测试用例列表

### TC-DAP-01 依赖配置、稳定执行顺序与同日上游产物

1. 运行 `e2e-tests/tests/test_asr_daily_agents_api.sh`，保存反向排列但有显式依赖的 Agent 数组并触发 Run All。
2. 检查两个 Runner 收到的 prompt 和工作目录。


### TC-DAP-02 非法依赖配置被拒绝

1. 分别保存不存在的依赖、自依赖、重复依赖和循环依赖。
2. 运行 `cargo test -p bifrost-admin daily_agent --lib`。

预期：API/配置校验拒绝四类非法配置，单元测试全部通过。

### TC-DAP-03 依赖失败时跳过或继续

1. 将上游 Runner 配置为必然失败，分别验证下游 `skip` 与 `continue` 策略。

预期：`skip` 下游状态为 `skipped_dependency_failed` 并记录上游失败；`continue` 不被依赖门禁直接跳过；其他独立分支不受影响。

### TC-DAP-05 周度洞察 Schedule 可复用研究产物

1. 创建测试 Schedule，Prompt 要求聚合最近七天日报与研究报告，只输出反复判断、未解决问题、方法论和产品机会。
2. 手动触发并检查历史，随后删除测试 Schedule。

预期：Schedule 产出非空周度洞察，不逐篇复述日报，并保留日期/来源。

### TC-DAP-06 微信 Provider 投递最终结果

1. 确认现有 `jcc-reader-weixin` Provider 可用。
2. 仅发送一条带测试标记的精简调研验证摘要，不发送原始录音或完整转录。

预期：发送成功且 outbound 历史存在记录；消息不含凭据或本地绝对路径。

### TC-DAP-07 WebUI 配置依赖关系

1. 用浏览器打开隔离服务的 ASR Task -> Daily Agent。
2. 编辑下游 Agent，保存 `Depends On`、`Include Output` 和 `On Dependency Failure`，刷新后检查列表。

预期：不能选择自己；保存后配置保持；列表展示 `Dependencies`；依赖失败跳过状态使用可辨认的警示样式。

### TC-DAP-08 大型 ASR 任务详情不阻塞管理 API

1. 使用至少 500 条历史文件记录的真实或合成 ASR Task，打开 `Daily Agent` 页面并等待完整列表加载。
2. 页面保持打开时，并发请求任务详情、微信 Provider 列表和 TLS 配置端点，持续至少 30 秒。
3. 检查任务详情中的 Project URL、运行时数据目录和五个 Agent，再执行一次 `bifrost status --format json`。

预期：大型目录扫描在阻塞线程池执行；页面和所有管理端点持续响应，没有因为同步 `read_dir` 占满 Tokio worker；五个 Agent 与研究配置保持可见。

## 真实执行记录

- 2026-07-14：TC-DAP-08 通过。正式 `9900` 服务打开包含 640 条历史记录的真实 ASR Task 与五 Agent 配置页，保持页面打开并连续 6 轮、30 秒并发请求任务详情、微信 Provider 列表和 TLS 配置；全部返回 200，任务详情耗时 0.24-0.30 秒，其他管理端点保持毫秒级响应。Project URL 与 `<local-runtime-data-dir>` 运行时目录均在视口和表格单元格内可见；截图保存在 Codex 本地验证产物中。
- 2026-07-13：在最新 `main` 已移除内置 Agent 的架构上重建并执行 TC-DAP-01/02。`cargo test -p bifrost-admin daily_agent --lib -- --nocapture` 通过 60 项；`SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_asr_daily_agents_api.sh` 使用外部 Codex mock mock、动态端口和临时数据目录通过。测试故意倒序保存五个 Agent，确认依赖拓扑、同日产物复制和非法依赖 400 拒绝均保持有效，未触碰正式 9900 服务。
- 2026-07-12：TC-DAP-01/02/03 通过。`cargo test -p bifrost-admin daily_agent --lib` 通过 49 项；`test_asr_daily_agents_api.sh` 使用隔离数据目录验证反向数组仍按依赖拓扑执行、同日产物复制、非法依赖拒绝、上游失败时下游 `skipped_dependency_failed`，并覆盖陈旧 source hash 拦截。
- 2026-07-12：TC-DAP-05/06 通过。默认 `9900` 服务临时创建并手动执行周度 Codex Schedule，run `6f4a092e` 为 `success`，微信 outbound 成功后删除测试 Schedule；随后把真实研究报告精简结论发送到 `jcc-reader-weixin`，message id `bifrost-weixin-1783824014855-adfbaab4-d661-4d1c-962b-28767e42de60`，未发送原始录音或完整转录。
- 2026-07-12：TC-DAP-07 通过。浏览器真实页面显示 `Dependencies` 列；研究 Agent 详情页正确展示 `daily_report`、`Include output` 和 `Skip this agent`，关键截图保存在 `/tmp/bifrost-dap-agent-dependencies.png`。
