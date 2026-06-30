# Remote Invoke 授权韧性改进

## 问题背景

当前 remote-invoke 的授权模型中，relay 仍承担了"授权会话真相源"的角色，导致在异常路径下出现状态分裂：

1. **SSE 重连不做双向对账**：SSE 断开期间丢失的 `grant_revoked` 事件不会被补偿，target 本地残留已失效的 grant
2. **local_grants 纯内存**：target 重启后 grant 状态全部丢失，完全依赖 relay 恢复
3. **Caller 侧无容错**：relay 返回不一致状态时 caller 直接报错退出，无自愈能力

## 改进方案

### 改进 1：SSE 重连双向对账

**现状**：`run_sse_session` 中 `fetch_active_grants` 只做"relay→target 合并"——把 relay 有的 grant 插入本地。不清理"本地有但 relay 已不存在"的 grant。

**改进**：SSE 重连时，以 relay 返回的 active grants 为权威集合，清理本地多余的过期 grant。

- 构建 relay_active_set（relay 返回的所有 active grant_id）
- 遍历 local_grants，凡不在 relay_active_set 中的 → 移除 grant + crypto + policy
- 保留 SSE 实时事件（grant_created/grant_revoked）作为增量通道

### 改进 2：local_grants 持久化

**现状**：`local_grants` 是 `Arc<RwLock<HashMap<String, GrantInfo>>>`，重启即丢。

**改进**：新增 `GrantInfoStore`，与 `GrantCryptoStore`/`GrantPolicyStore` 同模式：
- 文件：`remote_invoke_grant_info.json`
- 启动时从磁盘恢复 → 合并到 local_grants
- grant 变更时同步写盘（insert/remove）
- SSE 重连对账时批量写盘
- Grants 列表面向排障展示两个时间：
  - `first_connected_at`：等同本地 `first_authorized_at`，表示该 grant 第一次在 target 侧建立授权/连接的时间；pair-code grant 来自审批落地时间，SSH grant 来自 SSH connect 成功时间。
  - `last_command_at`：仅在 `call_open` 通过本地 grant 校验并准备执行真实远程命令时更新；单纯 SSH 连接、grant 列表刷新、SSE 对账不会更新该字段。

**展示要求**：
- Admin API `GET /_bifrost/api/remote-invoke/grants` 返回 `first_connected_at` 与 `last_command_at`，并保留 `first_authorized_at` / `last_used_at` 兼容字段。
- WebUI Settings → Remote Invoke → Grants 行内展示 `connected <time>` 与 `last command <time>`，未执行过命令时显示 `-`。
- CLI `bifrost setting grant list` 展示 `first connected` 与 `last command`，便于不用打开 UI 也能判断授权是否只是连上、还是实际执行过命令。

### 改进 3：Caller 侧容错

**现状**：`find_reusable_grant` 返回 None 时 caller 直接 `exit(1)`。

**改进**：
- 如果 relay 返回无可用 grant，自动从本地 connections 中移除该条目（标记为 stale）
- 给出更明确的错误信息和修复建议

### 改进 4：Recent Calls E2E 本地 fixture 端口 fallback

**现状**：`test_remote_invoke_recent_calls_args_preview_e2e.sh` 使用 `pick_free_port` 选择本地 HTTP echo fixture 端口，再启动 `http_echo_server.py --retries 5`。在 CI 并发 shell E2E 中，端口可能在选择后、bind 前被其他测试占用。`http_echo_server.py` 会按设计 fallback 到下一个可用端口并打印实际绑定端口，但脚本仍然轮询原端口，导致日志中出现 `READY` 后仍报“本地 echo fixture 未在超时内就绪”。

**改进**：
- 启动 fixture 后从日志中的 `Starting HTTP Echo Server on <host>:<actual_port>...` 解析实际绑定端口。
- 只对实际绑定端口执行 `/health` 探活，避免误命中占用原端口的其他服务。
- 如果实际端口与请求端口不同，输出 fallback 日志，并更新后续 Recent Calls 流量使用的 `MOCK_HTTP_PORT`。
- 使用 `python3 -u` 启动 fixture，并把 `/health` ready 等待预算提升到 60 秒，避免 macOS shell shard 高并发/高负载时 Python 进程尚未打印端口或完成启动就被 10 秒窗口误判失败。
- 保持 `http_echo_server.py --retries 5` 行为不变，避免影响其他复用该 fixture 的测试。

### 改进 5：未登录 sync session 时的 relay 注册日志降噪

**现状**：Remote Invoke worker 会随 Bifrost 启动自动启动。用户尚未登录 sync session、`sync-state.json` 中没有 token 时，worker 仍进入 relay 注册流程，并把“缺少 sync session token”当作 `ERROR failed to register with relay` 周期性输出。该状态不是 relay 网络故障，而是“等待用户登录/配置 token”，持续刷 ERROR 会遮蔽真正问题。

**改进**：
- 在进入 relay 注册前先检查 sync session token，空 token 或纯空白 token 均视为缺失。
- 缺 token 时 worker 保持 `Disconnected`，只输出一次 `INFO remote invoke relay registration waiting for sync session token`。
- 保留原有退避唤醒机制，后续一旦 token 可用，worker 会重新进入正常注册流程。
- 真正的 relay 网络错误、401 后重新注册失败、SSE 失败仍按原有 `ERROR` / `WARN` 级别输出，避免把真实故障静默掉。

### 改进 6：target 上报 call frame 的发送层短重试

**现状**：target worker 执行远端命令时会通过
`/v4/remote-invoke/client/calls/:callId/frame` 和
`/stream-frame` 把输出帧回传给 relay。PPE 全链路验证中出现过一次
reqwest 发送层错误：部署侧 `bifrost.server.v4` access log 能看到同一 call
的其它 frame/stream-frame/exit 均为 HTTP 200，且没有 `relay_queue` 错误，
说明问题发生在 target 到 TLB/relay 的连接发送层，而不是业务拒绝。

**改进**：
- 只对 `post_call_frame` / `post_call_stream_frame` 的 request send error
  做短重试，延迟为 `150ms/500ms/1000ms`。
- 如果 relay 已返回 HTTP 4xx/5xx 或业务 `code != 0`，仍按原逻辑立即失败，
  避免掩盖真正的权限、队列或服务端错误。
- 单元测试用本地 TCP mock relay 模拟第一次连接断开、第二次成功，验证
  target frame post 能恢复。

### 改进 7：Grants stale session 保留上限

**现状**：`bifrost remote conn up --ssh-key` 等复用连接会为每次成功连接生成新的本地 grant。SSH key 本身可以长期有效，但单次 session grant 如果一直保留，Settings -> Remote Invoke -> Grants 列表会随着重复连接无限增长，影响排查和页面可读性。

**改进**：
- Grants 列表仍以 `RemoteInvokeWorker::list_grants_and_cleanup()` 为数据源，清理发生在后端而不是只在 WebUI 隐藏。
- 每次列出 grants 和周期性 cleanup 时，按 `max(first_authorized_at, last_used_at, last_command_at)` 计算 grant 最近活动时间。
- 最近活动时间超过 48 小时的本地 stale grant 会被移除，并同步清理该 grant 的本地 crypto、policy、grant_info 记录。
- 如果 grant 当前仍有 active call 正在运行，即使已经过期或时间戳超过 48 小时也不会清理，避免长任务被列表刷新误删授权上下文。
- SSH key 记录和 SSH key 级默认文件策略不受 stale grant cleanup 影响；后续 caller 再次连接会创建新的 session grant。

## 测试方案

### 单元测试
- `test_grant_info_store_round_trip` (planned, not yet shipped as of 2026-06-16): 验证写入/读取/按 relay 过滤。当前 `grant_info_store.rs` 已实现 `load_for_relay` / `upsert` / `remove` / `retain_only`，但尚未补单元测试。
- `test_grant_info_store_remove` (planned, not yet shipped as of 2026-06-16): 验证移除后不再出现。
- `test_sse_reconcile_purges_stale_grants` (planned, not yet shipped as of 2026-06-16): 验证对账逻辑。对应的 production 代码已经落地：`worker.rs::run_sse_session` 在 `fetch_active_grants` 成功后会调用 `GrantInfoStore::retain_only` 以 relay 返回的活动集合清理本地多余 grant，仍需补一个直接覆盖该路径的单元测试。
- `test_validate_grant_accepts_active_permanent`: 验证有效 grant 执行命令时同步写入 `last_command_at`（已实现，位于 `crates/bifrost-admin/src/remote_invoke/worker.rs`）。
- `test_validate_grant_accepts_once_and_consumes`: 验证 once grant 消耗时也写入 `last_command_at`（已实现，位于 `crates/bifrost-admin/src/remote_invoke/worker.rs`）。
- `test_normalize_registration_session_token_rejects_missing_or_empty`: 验证缺失或纯空白 token 不会进入 relay 注册（已实现，位于 `crates/bifrost-admin/src/remote_invoke/worker.rs`）。
- `test_normalize_registration_session_token_trims_valid_token`: 验证有效 token 会被 trim 后继续注册（已实现，位于 `crates/bifrost-admin/src/remote_invoke/worker.rs`）。
- `post_call_frame_retries_send_failure_once`: 验证 target 上报 call frame
  时遇到一次发送层断连会短重试并成功（已实现，位于
  `crates/bifrost-admin/src/remote_invoke/relay_client.rs`）。
- `list_grants_and_cleanup_prunes_stale_inactive_grants`: 验证超过 48 小时未活动的 grant 会被清理，最近 grant 与最近执行过命令的 grant 保留。
- `list_grants_and_cleanup_keeps_stale_grant_with_active_call`: 验证正在执行 active call 的旧 grant 不会被 cleanup 误删。

### E2E 测试
- 使用 e2e-test 技能验证 SSE 重连对账
- 更新 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`：pair-code 授权后先断言 Grants API 有 `first_connected_at` 且 `last_command_at` 为空；执行一次远程搜索命令后断言 `last_command_at` 非空且不早于 `first_connected_at`。
- 更新 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`：当本地 echo fixture 请求端口被占用并 fallback 到新端口时，脚本必须解析实际端口、使用实际端口生成 Recent Calls 流量，并完整通过参数预览、长参数截断、落盘恢复与清理断言。
- 更新 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`：在 CI 高负载下 fixture 日志为空或启动变慢时，脚本必须等待到 60 秒并通过无缓冲输出拿到实际端口；提前退出仍要打印 mock server 日志。
- 新增 `e2e-tests/tests/test_remote_invoke_missing_sync_token_log_e2e.sh`：使用空临时数据目录启动 Bifrost，断言日志只出现一次等待 sync session token 的 INFO，且不再出现缺 token 导致的 `failed to register with relay` ERROR。

### 真实场景测试
- 更新 `human_tests/remote-invoke.md`，新增 Grants 时间字段回归用例，并按文档逐条执行：进入 discovery、批准授权、查看 Grants、执行远程命令、再次查看 Grants。
- 更新 `human_tests/remote-invoke.md`，新增 Recent Calls fixture 端口 fallback 回归用例，并按文档执行：预占 mock 请求端口、运行脚本、确认脚本切换到实际端口且 Recent Calls 断言全部通过。
- 更新 `human_tests/remote-invoke.md`，新增缺 sync session token 日志降噪回归用例，并按文档执行：使用空临时数据目录启动 Bifrost，确认只输出一次等待登录提示且没有重复 ERROR。
- 更新 `human_tests/remote-invoke.md`，新增 Grants stale session cleanup 回归用例，并按文档执行：构造超过 48 小时未活动、近期活动、正在执行 active call 的 grants，确认列表 cleanup 只移除 stale inactive grant。
