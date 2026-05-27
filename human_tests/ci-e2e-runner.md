# CI E2E Runner 并发稳定性

## 功能模块说明

验证 `bifrost-e2e` 自定义 runner 在 CI 并发模式下不会让存在进程级副作用的 standalone 用例互相污染，同时保留 stdout streaming 语义检查。

## 前置条件

- 在仓库根目录执行。
- Rust 工具链可用，已完成至少一次 `cargo test -p bifrost-e2e --no-run` 编译。
- 测试命令只启动测试内临时服务，不修改系统代理。

## 测试用例列表

### TC-CER-01：并发 runner 下 IM Gateway 长期记忆用例串行隔离

操作步骤：

1. 执行：`BIFROST_E2E_RUNNER_JOBS=8 cargo run -p bifrost-e2e -- --category admin --test im_gateway_agent_auto_memory_new_session_consumes --test-timeout 180`。
2. 观察输出中的测试结果。

预期结果：

- 命令退出码为 `0`。
- 输出显示 `im_gateway_agent_auto_memory_new_session_consumes` 通过。
- 不出现 `new session did not consume auto memory` 或 `new session request did not include generated memory instructions`。

### TC-CER-02：并发 runner 下 IM Gateway session 恢复用例串行隔离

操作步骤：

1. 执行：`BIFROST_E2E_RUNNER_JOBS=8 cargo run -p bifrost-e2e -- --category admin --test im_gateway_agent_chat_restores_history_after_service_restart --test-timeout 180`。
2. 观察输出中的测试结果。

预期结果：

- 命令退出码为 `0`。
- 输出显示 `im_gateway_agent_chat_restores_history_after_service_restart` 通过。
- 恢复后的 `/status` 使用最新 response context snapshot（约 `10` prompt/context tokens），同时 API 累计 token 保持 `15`。

### TC-CER-03：并发 runner 下 ChatGPT Web 恢复用例串行隔离

操作步骤：

1. 执行：`BIFROST_E2E_RUNNER_JOBS=8 cargo run -p bifrost-e2e -- --category admin --test im_gateway_chatgpt_web_restores_conversation_after_service_restart --test-timeout 180`。
2. 观察输出中的测试结果。

预期结果：

- 命令退出码为 `0`。
- 输出显示 `im_gateway_chatgpt_web_restores_conversation_after_service_restart` 通过。
- 不出现 `Expected chatgpt_web mock run snapshot` 或 conversationId 恢复失败。

### TC-CER-04：Remote shell stdout streaming CI 阈值回归

操作步骤：

1. 执行：`BIFROST_E2E_RUNNER_JOBS=8 cargo run -p bifrost-e2e -- --category remote_shell_exec --test remote_shell_exec_streams_stdout --test-timeout 180`。
2. 观察输出中的测试结果。

预期结果：

- 命令退出码为 `0`。
- 输出显示 `remote_shell_exec_streams_stdout` 通过。
- 断言仍验证至少两个 stdout chunk，且内容依次为 `stream-one`、`stream-two`。

## 清理步骤

- 测试用例内部使用临时目录并在结束时清理。
- 若命令被手动中断，执行 `ps aux | grep bifrost-e2e` 检查残留测试进程，并按需终止。
