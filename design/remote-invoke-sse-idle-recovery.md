# Remote Invoke SSE 静默断链自恢复

## 现状

Remote Invoke target worker 通过一条长期 SSE 连接接收 relay 下发的调用。当前实现只在 SSE 明确关闭、读取返回错误、鉴权失败或收到显式 reconnect 通知时重连。

线上现场出现了另一种失败：本地状态仍显示已连接，relay 的 HTTP heartbeat 也可继续成功，但下行 SSE 已经不再收到任何字节。此时 remote invoke 持续收不到调用；重启 Bifrost 后建立新 SSE 连接，调用立即恢复。HTTP heartbeat 与 SSE 是两条独立连接，因此 heartbeat 200 不能证明下行 SSE 仍可读。

Relay 每 30 秒向所有 SSE client 发送一次 `ping`。客户端当前没有利用该信号判断连接活性，导致半开连接可以无限期停留在 `Connected`。

## 目标

- 正常 SSE 至少每 30 秒收到 relay ping 时保持连接，不产生无意义重连。
- 连续 90 秒没有收到任何 SSE 字节时，worker 必须把该连接视为失活并返回现有重连循环。
- 任何 SSE chunk（业务事件、ping 或 comment）都必须刷新空闲期限。
- 修复只改变连接保活，不改变 grant、pairing、active call 或加密协议；重连继续复用现有状态恢复逻辑。
- 测试不得占用正式 9900 端口、不得停止正式 Bifrost、不得修改系统代理。

## 方案

在 `RemoteInvokeWorker::run_sse_session` 内增加 idle watchdog，期限取客户端 `sse_keepalive_ms` 的三倍；默认配置为 30 秒，因此默认期限为 90 秒。watchdog 从 SSE 响应建立并完成首次 grant reconciliation 后开始计时；每次 `bytes_stream` 返回 chunk 时重置 deadline。若 deadline 到期，返回可诊断的 `BifrostError::Network`，由既有 `run_loop` 将状态切换为 `Reconnecting` 并按退避策略重新注册、建立新 stream。

90 秒等于 relay 默认 30 秒 keepalive 的三倍，可容忍单次调度或网络抖动，同时把假在线窗口限制在 90 秒左右。这里不把 HTTP heartbeat 成功当成下行存活证据，也不因单次 heartbeat 非鉴权错误立即重连，避免两条连接的语义混淆和瞬时网络抖动导致重连风暴。

为便于确定性测试，测试 worker 直接给生产入口注入更短的 `sse_keepalive_ms` 配置，不新增只供测试调用的运行时入口。本地 mock relay 使用真实 HTTP/SSE socket，分别覆盖“建立后静默”和“持续 ping 后正常关闭”。

## 影响与风险

- relay 如果连续三次未按约定发送 ping，即使业务连接仍在，也会触发一次安全重连；这是 fail-recover 行为，已有重连流程负责恢复。
- 重连期间可能有短暂调用延迟，但优于无限期假在线。
- 不修改 server keepalive、HTTP heartbeat 或公网路由，不引入双路径和新的持久化状态。

## 验证计划

- L1：`bifrost-admin` worker 单测用真实本地 SSE socket 验证静默超时和 ping 重置。
- L2：Remote Invoke shell E2E 使用随机端口与隔离数据目录，验证正常 pair/grant/call/reconnect 回归。
- L3：复用正式服务做只读状态检查；当前源码的故障注入在隔离 relay 与临时目录中执行，确认不影响 9900。正式 relay 上的 90 秒断链恢复在可部署版本产生后再做真实网络故障注入。
- 回归：`cargo fmt`、`clippy`、workspace tests、changed-lines coverage、local CI。
- Review/Fix/Test：实现后和全量验证前各执行一轮独立目标、范围、代码和测试复核。

## Review / Fix / Test 记录

- 第 1 轮：确认 HTTP heartbeat 与下行 SSE 是两条独立连接；将固定 90 秒改为客户端 keepalive 配置的三倍，并为测试增加 2 秒外层上限。定向单测与 73 项 Remote Invoke E2E 复测通过。
- 第 2 轮：发现早期实现保留了只为测试覆盖超时值的私有入口；测试改为直接调用生产 `run_sse_session`，随后移除单调用方包装。完整 `bifrost-admin` 3,022 项测试、changed-lines 100%、workspace、clippy、构建与依赖审计复测通过。
