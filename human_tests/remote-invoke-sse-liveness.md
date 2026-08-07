# Remote Invoke SSE 活性恢复真实场景测试

## TC-RI-SSE-01：SSE 半开连接必须在三次 keepalive 窗口内自动重连

### 背景

Remote Invoke target 可能出现状态仍为 `Connected`、HTTP heartbeat 仍成功，但下行 SSE 已不再收到事件的半开连接。此时远程调用持续失败，重启 Bifrost 后恢复，说明缺少的是 SSE 自身的活性判定而不是业务授权恢复。

### 操作步骤

1. 执行定向 worker 回归：`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin sse_idle_ --lib -- --nocapture`。
2. 测试用本地随机端口启动真实 HTTP/SSE mock relay，不启动正式代理端口，不修改系统代理。
3. 场景 A 建立 SSE 后保持 socket 打开但不再发送任何 chunk，观察 worker 是否在测试配置的短 idle deadline 后返回可重连错误。
4. 场景 B 在 idle deadline 内持续发送 SSE `ping` chunk，随后正常关闭，观察每个 chunk 是否刷新 deadline，连接不会被提前判死。
5. 执行 Remote Invoke 主流程 E2E：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_remote_invoke_e2e.sh`。
6. 只读检查正式服务：`bifrost status --format json-pretty`，确认正式 9900 服务未被测试停止或替换。

### 预期结果

- 静默 SSE 在 deadline 到期后返回包含 idle timeout 和 stream id 的网络错误；既有 run loop 随后进入 `Reconnecting`。
- 每个收到的 SSE chunk 都刷新 deadline；正常 30 秒 relay ping 下不会触发 90 秒 watchdog。
- 主 Remote Invoke E2E 的配对、授权、调用、断线恢复和清理继续通过。
- 测试只使用随机端口和隔离数据目录，不占用 9900，不修改系统代理。

### 实际执行结果

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-SSE-01 | ✅ PASS | 2026-08-07 编码前首次执行因本机缺少 Cargo 被归因为环境依赖，不计产品 RED；安装锁定工具链后执行定向测试，得到 watchdog 入口尚不存在的编译 RED。实现后执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin sse_idle_ --lib -- --nocapture`，静默超时与 ping 刷新 2/2 PASS。随后以当前 `target/debug/bifrost` 执行主 Remote Invoke E2E，随机端口为 relay `63482`、target `63481`、mock HTTP `63712`，配对、授权、status、traffic、search、cancel、client 重启、fresh reconnect 与 disconnect 共 73/73 PASS。E2E 前后只读查询正式服务均为版本 `0.0.172`、PID `83070`、端口 `9900`、系统代理启用，证明隔离测试未停止或替换正式进程。完整变更 crate coverage 为 3,022/3,022 PASS、changed-lines 17/17（100%）；workspace all-features、clippy、all-targets build 均 PASS。local-ci 的格式、coverage、clippy、workspace 均 PASS，初次依赖审计仅因本机缺少工具失败；安装 CI 锁定版本后单独执行同一审计脚本，`cargo-deny` 无 error，`cargo-udeps` 输出 `All deps seem to have been used.`。 |
