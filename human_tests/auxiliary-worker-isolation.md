# 附加能力 Worker 进程隔离验收

## 目标

验证 Bifrost 在不改动代理核心数据面的前提下，将 External CLI、Browser、ASR、IM Gateway、Remote Invoke 与 Remote Execution 放入可独立回收的进程边界；附加能力故障不得导致代理转发或 Admin API 退出。

## 前置条件

- 使用当前分支构建的 `bifrost` 二进制。
- 所有用例必须使用临时 `BIFROST_DATA_DIR`，禁止读写 `~/.bifrost`。
- 使用动态端口，禁止占用正式端口 `9900`。
- 启动时设置 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并传入 `--no-system-proxy`。
- 进程清理只能针对测试记录的 PID 或测试数据目录中的运行时 PID 文件，禁止 `pkill`、`killall`。

## 自动化基线

```bash
SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
  bash e2e-tests/tests/test_auxiliary_worker_isolation.sh
```

## 测试用例

### TC-AWI-01：启动保持 lazy

1. 用全新临时数据目录启动 Bifrost。
2. 调用 `GET /_bifrost/api/workers`。
3. 检查系统进程列表。

预期：接口返回空数组；没有 Browser、Chromium、ASR、IM Gateway、Remote Invoke 或 Remote Execution worker；代理和 Admin API 正常。

### TC-AWI-02：回滚模式可观测

1. 调用 `GET /_bifrost/api/workers/modes`。
2. 分别以 `BIFROST_ASR_EXECUTION_MODE=legacy` 和默认环境启动临时实例。

预期：默认六类能力均为 `worker`；显式设置后只有 ASR 报告 `legacy`，其他能力不受影响；环境变量名称可从接口读取。

### TC-AWI-03：真实 Worker 握手与关闭

1. 通过隐藏入口启动 ASR worker 和 Remote Execution worker。
2. 校验 Hello 中的协议版本、kind、PID、instance ID 和随机启动 token。
3. 发送 Ping，再发送 Shutdown。

预期：两类 worker 均完成 `Hello → Ready → Response(pong) → Response(shutdown) → Goodbye/退出`；stdout 只包含 NDJSON 协议帧。

### TC-AWI-04：IPC 大帧拒绝

1. 启动独立 ASR worker。
2. 写入超过 1 MiB 的单帧。
3. 检查 worker stderr 和主进程状态。

预期：该 worker 拒绝输入并退出；stderr 包含 bounded-frame 错误；Bifrost 主进程、Admin API 和代理转发继续可用。

### TC-AWI-05：代理故障域不传播

1. 通过 Bifrost 代理访问本地 echo server 并记录成功结果。
2. 触发 TC-AWI-04，或终止一个已记录 PID 的 worker。
3. 再次通过代理访问 echo server。

预期：两次请求均成功；代理 PID 未变化；worker 故障没有触发 Bifrost 重启。

### TC-AWI-06：生命周期控制

1. 调用 `/api/workers/{kind}/stop`、`restart`、`reset-circuit`。
2. 在能力尚未使用、没有 spawn spec 时调用 `start`。
3. 实际使用一次能力后再次调用 stop/restart。

预期：无 spawn spec 时 start 返回 409，不伪造成功；有 spawn spec 后能按 kind 控制；操作不影响其他 worker 和代理。

### TC-AWI-07：Job、事件与 Artifact 边界

1. 运行 External CLI、Browser 或 ASR 任务。
2. 调用 `/api/worker-jobs`、`/{jobId}/events` 和 `/{jobId}/artifacts`。
3. 用 offset/limit 和 tail 读取 artifact。
4. 尝试读取未注册路径、目录、超出范围 offset 和超过 1 MiB 的单次范围。

预期：任务状态按 queued/running/terminal 演进；事件和 artifact 数量受限；仅显式注册的规范普通文件可读；非法范围被拒绝；主进程不一次性加载完整大文件。

### TC-AWI-08：External CLI 输出洪水与取消

1. 使用测试 runner 连续输出超过 stderr/stdout 内存 tail 限制的数据。
2. 观察增量日志、job events 和内存。
3. 在运行中调用 job cancel。

预期：完整输出增量落盘，内存只保留受限 tail；progress 队列饱和时允许丢弃进度但最终状态可靠；取消后进程树被回收。

### TC-AWI-09：Browser crash、CDP hang 与空闲退出

1. 触发 Browser worker lazy 启动。
2. 分别终止 Chromium 子进程、让 CDP 请求超时、终止 worker。
3. 等待默认 idle timeout。

预期：当前 job 失败或超时；Chromium 孤儿进程被清理；主进程继续工作；后续请求可重新拉起 worker；空闲后 worker 退出但 profile 保留。

### TC-AWI-10：ASR 资源与恢复

1. 运行含 ffmpeg、模型推理和 diarization 的离线任务。
2. 分别模拟 ffmpeg timeout、模型失败、worker kill、取消和高 CPU。
3. 重启后检查 checkpoint 和任务状态。

预期：重任务只存在于 ASR worker；任务被标记 interrupted/failed/cancelled；checkpoint 不重复提交已完成输出；代理延迟和错误率保持在设计门禁内。

### TC-AWI-11：IM Gateway 重连与事件风暴

1. 启用测试 provider 和 schedule，使 IM Gateway worker 按配置启动。
2. 断开 provider、注入突发事件、触发 scheduler reentry，再终止 worker。
3. 检查 inbox/outbox journal 和恢复结果。

预期：重连有退避；事件队列和 journal 有界且可恢复；同一事件不重复执行；主进程只保留配置控制面。

### TC-AWI-12：Remote Invoke 与执行 Worker

1. 建立测试 relay/pairing。
2. 执行 shell 命令并持续发送 stdin/接收 stdout。
3. 模拟 relay disconnect、输出洪水、worker restart 和 shell 卡死。

预期：relay worker 不直接执行命令；每个调用由 Remote Execution worker 承担；stdin/stdout 有背压；取消或超时回收完整进程树；duplicate frame 不重复执行。

### TC-AWI-13：主进程联合 Chaos

1. 在持续代理流量下并发运行 External CLI、Browser、ASR、IM 和 Remote。
2. 轮流 kill worker、填满其事件队列、制造慢磁盘与超时。
3. 记录代理 P50/P95、错误率、RSS、FD 和主进程 PID。

预期：附加能力按自身故障语义降级；代理主进程不退出、不重启；代理错误率增量不超过技术方案门禁；主进程 RSS/FD 不随 worker 输出无界增长。

## 清理步骤

1. 通过生命周期 API 或协议 Shutdown 停止 worker。
2. 向测试记录的 Bifrost PID 发送 INT/TERM，超时后仅回收其已记录进程树。
3. 停止 echo/mock relay/provider。
4. 确认没有测试 PID 存活后删除临时数据目录。
5. 保留需要审计的日志时，复制到测试 artifact 目录后再清理。
