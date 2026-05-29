# Agent Chat History Pagination 真实场景测试

## 功能模块说明

验证 Agent Chat 页面不再打开即全量加载历史详情。会话列表只加载摘要；选中某个历史会话后，详情首屏只加载最新事件页；继续查看旧内容时再按需加载上一页；运行中的历史轮询只拉取新增事件。

## 前置条件

1. 在仓库根目录执行命令前先运行 `source ~/.zshrc`。
2. 使用独立临时数据目录，避免污染本机 Bifrost 数据。
3. 启动服务时必须带 `--no-system-proxy`，除非测试目标是系统代理。
4. 测试用 history 文件至少包含 6 条 event，用于验证 tail、cursor、since。

## 测试用例列表

### TC-ACH-01 列表接口只返回摘要

操作步骤：

1. 启动测试 Bifrost 服务。
2. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/all`。
3. 检查响应中的 session 条目。

预期结果：

- 响应包含 `session_key`、`title`、`history_path`、`timeline_event_count` 等摘要字段。
- 响应不包含 `events` 数组。
- 响应不包含完整对话正文详情。

### TC-ACH-02 选中详情首屏只加载尾页

操作步骤：

1. 使用 TC-ACH-01 得到的 `history_path`。
2. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/history/{history_path}?tail=true&limit=2`。
3. 检查分页元数据与事件数量。

预期结果：

- `count` 为 `2`。
- `total_count` 大于 `count`。
- `start_index` 指向尾页起始下标。
- `has_more` 为 `true`。
- 响应只包含尾页事件，不包含整份 JSONL 的全部事件。

### TC-ACH-03 向上查看时加载旧页

操作步骤：

1. 读取 TC-ACH-02 响应中的 `next_cursor`。
2. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/history/{history_path}?cursor={next_cursor}&limit=2`。
3. 检查返回事件与尾页不同。

预期结果：

- `count` 为 `2`。
- 返回的是尾页之前的旧事件。
- `end_index` 等于上一次尾页的 `start_index`。
- 如果更早还有内容，`has_more` 继续为 `true`。

### TC-ACH-04 运行中轮询只加载新增事件

操作步骤：

1. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/history/{history_path}?since=5`。
2. 检查响应事件数量和下标。

预期结果：

- 响应只包含下标 `5` 及之后的事件。
- `start_index` 为 `5`。
- `end_index` 等于当前 `total_count`。
- 不返回 `since` 之前的旧事件。

## 清理步骤

1. 停止测试 Bifrost 进程。
2. 删除临时 `BIFROST_DATA_DIR`。
3. 删除测试期间生成的临时响应文件。

## 执行记录

- 通过。2026-05-29 执行 `bash e2e-tests/tests/test_agent_history_pagination_api.sh`，脚本使用独立 `BIFROST_DATA_DIR` 创建 7 条事件的 JSONL，启动测试 Bifrost 服务并逐条验证 TC-ACH-01 至 TC-ACH-04：`sessions/all` 只返回摘要且无 `events`；`tail=true&limit=2` 只返回尾页并带 `has_more=true`；`cursor=5&limit=2` 返回旧页；`since=6` 只返回新增事件。最终输出 `agent history pagination API checks passed`。
