# Group 缓存与 Admin 稳定性真实场景测试

## 功能模块说明

验证远端 Group 服务不可用时，本地规则继续可见，缓存补全 single-flight 并按指数退避，同时 active-summary 的文件扫描不阻塞轻量 Admin 健康接口。

## 前置条件

- 在仓库根目录执行，使用当前分支代码。
- 正式 9900 服务保持运行；E2E 使用隔离端口 28180 和测试 runner 临时目录。
- 不修改用户规则、登录态或系统代理。

## 测试用例列表

### TC-GCAS-01：状态机 single-flight、退避和 generation

**操作步骤**：

```bash
cargo test -p bifrost-admin group_cache_ -- --nocapture
```

**预期结果**：

- 同 generation 第二次 begin 被合并。
- 连续失败退避从 5 秒增长到 10 秒。
- 成功后保持 resolved，显式失效后才能重试。
- 旧 generation 完成结果不能覆盖新状态。

### TC-GCAS-02：远端失败下本地规则与 Admin readiness

**操作步骤**：

```bash
BIFROST_E2E_TEST_TIMEOUT=30 cargo run -p bifrost-e2e -- \
  -t group_rules_active_summary_survives_remote_cache_resolution_failure \
  -p 28180 -v
```

**预期结果**：

- 远端请求失败后，本地 Group 规则在前后两次 active-summary 中都存在。
- 日志只显示一次远端解析并进入 `retry_after_ms=5000`。
- `/api/proxy/system/support` 在 450ms 门槛内返回 200。
- 立即再次请求不能绕过退避，Group 目录不被删除。

### TC-GCAS-03：阻塞文件操作隔离

**操作步骤**：

```bash
rg -n "spawn_blocking|build_active_summary_snapshot|refresh_badge_rules_cache_async" \
  crates/bifrost-admin/src/handlers/rules.rs crates/bifrost-admin/src/state.rs
```

**预期结果**：

- active-summary 的本地规则扫描由 `spawn_blocking` 执行。
- 远端补全后的 Badge cache 重建由 `spawn_blocking` 执行。

## 清理步骤

- runner 自动停止隔离代理并清理临时目录。
- 确认 `lsof -nP -iTCP:9900 -sTCP:LISTEN` 的正式 PID 未变化。

## 2026-07-31 执行记录

- `TC-GCAS-01`：通过，3 个状态机用例全部成功。
- `TC-GCAS-02`：通过，远端返回 401 时本地规则保留，E2E 596ms 完成并记录 `retry_after_ms=5000`。
- `TC-GCAS-03`：通过，active-summary snapshot 和 Badge cache refresh 均由 `spawn_blocking` 执行。
- 清理确认：隔离端口无残留，共享 9900 仍由执行前 PID 5988 监听。
