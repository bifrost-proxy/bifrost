# 系统代理生命周期快速恢复

## 背景

如果 Bifrost 主进程异常退出，系统代理仍指向该进程的端口，用户会失去网络。此前 lifecycle helper 主要依赖 2 秒一次、连续 3 次失败的兼容性轮询，最坏需要约 6 秒才进入恢复。

端口连接失败、Admin HTTP 超时或调度延迟不能证明进程已退出：高并发或 CPU 压力下，它们可能只是 readiness 降级。快速路径因此只能基于操作系统确认的进程实例身份，不能使用 listener 或 HTTP 探针。

## 设计

`ProcessIdentityStatus` 将观察结果区分为：

- `Alive`：PID 存在，且记录的启动时间匹配或无法取得启动时间。
- `Exited`：操作系统明确报告 PID 已不存在。Unix 仅接受 `ESRCH`；Windows 仅接受 `OpenProcess` 的 `ERROR_INVALID_PARAMETER` 或已退出的 process handle。
- `Reused`：PID 存在，但启动时间与记录值不匹配。
- `Unknown`：权限不足、平台查询异常、PID 缺失或其它无法确认的状态。

lifecycle helper 每 250ms 进行一次身份检查。只有 `Exited` 和 `Reused` 可立即进入现有 guarded recovery；`Unknown` 不改变系统代理。原有 2 秒、连续 3 次的 `is_process_running` 检查继续保留，专门兜底 zombie 和历史平台行为。

正常 restart 会先写入 `PreserveForRestart` marker。即使旧 PID 已消失，helper 也必须记录 `recovery_action=preserve_for_restart` 并退出，不得恢复或关闭系统代理。

## 可观测性

每个恢复入口写 `detection_method`：`pid_missing`、`pid_reused`、`poll_confirmed_exit` 或信号名。开始、完成和失败日志关联以下字段：

- `helper_pid`、`parent_pid`、`parent_started_at_ms`
- `detection_method`、`recovery_action`、`elapsed_ms`
- 失败时的 `error`

`recovery_action` 为 `background_cleanup`、`already_cleaned`、`preserve_for_restart` 或 `restart_or_restore`。

## 验证

- 单元测试验证仅明确 PID 消失可触发 `Exited`，权限错误保持 `Unknown`，启动时间不匹配识别为 `Reused`。
- 生命周期单元测试验证触发原因和 restart 参数。
- 真实系统代理写入回归由远端 CI 隔离环境或专用测试设备执行；日常开发机不运行该类 E2E/human test，避免改动开发者正在使用的代理配置。
- 本地不执行覆盖率命令；远端 CI 继续执行 Rust 覆盖率门禁。

## 风险与边界

- 高 CPU、端口不可达和 HTTP readiness 超时不会触发快速恢复。
- PID 查询权限不足时 fail-closed，保留原代理配置，避免误回滚仍运行实例。
- PID 复用被视为旧实例已消失，但仍会经过 shutdown marker 和既有 ownership/managed-runtime guard。
