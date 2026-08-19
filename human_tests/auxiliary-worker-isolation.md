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

# ASR 的真实 ffmpeg 压缩链路
SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
  bash e2e-tests/tests/test_asr_source_compression.sh

# IM Gateway 的本地 Weixin mock provider 链路
SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
  bash e2e-tests/tests/test_weixin_provider_e2e.sh

# Remote Invoke 的本地 relay、SSH grant 和 Remote Execution 链路
SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
  bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh

# IM worker -> 主进程的 Guide/Stop 控制面与显式 Queue 语义
SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
  bash e2e-tests/tests/test_external_runner_live_guide.sh
```

以上命令构成合入门禁，均不依赖真实账号或云端 relay。真实浏览器账号、真实 ASR 模型、外部 IM 平台断网风暴和长时间资源压力属于发布前扩展 soak，不得在未执行时记为本用例通过。

## 测试用例

### TC-AWI-01：启动保持 lazy

1. 用全新临时数据目录启动 Bifrost。
2. 调用 `GET /_bifrost/api/workers`。
3. 检查系统进程列表。

预期：接口返回空数组；没有 Browser、Chromium、ASR、IM Gateway、Remote Invoke 或 Remote Execution worker；代理和 Admin API 正常。

### TC-AWI-02：回滚模式可观测

1. 调用 `GET /_bifrost/api/workers/modes`。
2. 检查六类 worker 的默认模式与对应环境变量名称。

预期：默认六类能力均为 `worker`；每类的回滚环境变量名称可从接口读取，且均以 `BIFROST_` 开头。

### TC-AWI-03：真实 Worker 握手与关闭

1. 通过隐藏入口启动 ASR worker 和 Remote Execution worker。
2. 校验 Hello 中的协议版本、kind、PID、instance ID 和随机启动 token。
3. 发送 Ping，再发送 Shutdown。

预期：两类 worker 均完成 `Hello → Ready → Response(pong) → Response(shutdown) → 退出`；Hello 中的 kind、PID、instance ID、token 和协议版本均正确；stdout 只包含 NDJSON 协议帧。

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

1. 在能力尚未使用、没有 spawn spec 时调用 `/api/workers/asr/start`。
2. 调用 `/api/workers/asr/reset-circuit`。
3. 检查主进程、代理请求和 worker job API。

预期：无 spawn spec 时 start 返回 409，不伪造成功；reset-circuit 成功；操作不影响主进程、代理或 job API。

### TC-AWI-07：Job、事件与 Artifact 边界

1. 通过 Admin API 运行真实的 External CLI per-job worker。
2. 调用 `/api/worker-jobs`、`/{jobId}/events` 和 `/{jobId}/artifacts`。
3. 用 offset/limit 和 tail 读取 artifact。
4. 尝试读取未注册路径、目录、超出范围 offset 和超过 1 MiB 的单次范围。

预期：任务进入 succeeded 终态并保留开始/结束时间；事件和 artifact 数量受限；result/stdout/stderr/normalized_events 均已注册；仅显式注册的规范普通文件可读；非法范围被拒绝；主进程不一次性加载完整大文件。

### TC-AWI-08：External CLI 输出洪水与取消

1. 使用测试 runner 生成大于 320 KiB 的最终结果，检查完整 result artifact 和受限范围读取。
2. 再启动一个持续等待的 sessionless External CLI job。
3. 在 queued/running 阶段调用 job cancel，并等待调用线程和 job 进入终态。

预期：大结果完整落盘且可分页读取；取消请求返回 202；sessionless job 稳定映射到同一 logical job，并最终为 cancelled；子进程树被回收；代理继续可用。

### TC-AWI-09：Browser Worker 操作与故障隔离

1. 通过隐藏 worker 入口启动 Browser worker。
2. 执行 `browser.clear_session_conversation` 并校验响应。
3. 在持续代理探测期间终止记录到的 Browser worker PID。

预期：Browser 操作在独立 PID 内完成；终止 worker 不改变 Bifrost 主进程 PID；后续代理请求仍返回 200。

### TC-AWI-10：ASR Worker 与真实压缩链路

1. 对不存在的 task 调用 `asr.run_directory_task`，校验有界失败响应。
2. 运行 `test_asr_source_compression.sh`，让 ASR worker 通过真实 ffmpeg 把 WAV 压缩为 FLAC，并验证哈希与错误保留。
3. 在持续代理探测期间终止记录到的 ASR worker PID。

预期：任务执行和 ffmpeg 均位于 ASR worker 边界；成功产物与失败信息正确持久化；终止 worker 后代理仍返回 200。

### TC-AWI-11：IM Gateway Worker 与 provider 链路

1. 启动 IM Gateway worker，调用 `im.runtime_status`。
2. 通过 worker 的 `im.send_message` 与 `im.upload_message` 文件引用协议执行缺失 provider 校验，确认请求文件在 worker 读取后删除。
3. 运行 `test_weixin_provider_e2e.sh` 的本地 mock provider 全链路，并通过 Admin 消息发送接口验证实时上下文仍由 IM worker 使用。
4. 在持续代理探测期间终止记录到的 IM Gateway worker PID。

预期：provider 状态、消息收发、附件上传、上下文持久化与恢复均通过；Admin 发送和上传不会回落到主进程 provider；worker 终止不影响主进程和代理；主进程只保留配置、文件引用与 broker 控制面。

### TC-AWI-12：Remote Invoke 与执行 Worker

1. 启动本地 relay，建立 SSH pairing/grant。
2. 执行 File、Shell、Query、traffic/search/replay、detached job 与 stdin/exit-code 链路，并多次切换 grant scope/file access。
3. 直接校验 Remote Execution worker 的 prepare/stdin/stdin_close 协议；分别终止 Remote Invoke 与 Remote Execution worker PID 后探测代理。

预期：relay transport 与命令执行跨进程隔离；主进程 broker 对每次调用重新鉴权；非 Shell 与 Shell stdout 均不丢最后一帧；权限升降级立即生效；两个 worker 任一退出均不影响代理。

### TC-AWI-13：主进程确定性联合 Chaos

1. 同时启动 Browser、ASR、IM Gateway、Remote Invoke 与 Remote Execution worker，并保留各自 PID。
2. 先完成一次代理请求，再按 PID 逐个终止五个 worker；每次终止后立即再发代理请求。
3. 最后执行 External CLI 大结果与取消场景，再次探测代理并检查主进程 PID。

预期：所有代理探测均返回 200；Bifrost 主进程不退出、不重启；worker 故障和 External CLI 取消不跨越到代理故障域。

### TC-AWI-14：IM Worker 跨进程 Runner 控制

1. 保持 IM Gateway 与 External CLI 的默认 worker 隔离模式，通过 debug inbound 启动 Codex/Traex 当前 turn。
2. 从 IM worker 发送普通后续消息和 `/g`，确认主进程 Runner 收到实时 Guide；发送 `/q`，确认只在当前 turn 完成后执行。
3. 向仍在运行的 session 发送 `/stop`，确认主进程 Runner 收到协议级 interrupt。
4. 检查不存在 session、错误 capability token 与 broker 缺失配置的失败边界。

预期：IM worker 不读取自己的空 Runner registry；Guide 与 Stop 通过 loopback capability broker 到达主进程，显式 Queue 语义不变，控制失败不会伪造成功。

### TC-AWI-15：Worker 生命周期与最小环境能力

1. 运行 `test_auxiliary_worker_isolation.sh`，确认真实 worker 的握手、超大帧拒绝、进程回收和代理存活。
2. 运行 `goodbye_reaps_process_and_blocked_stdin_write_fails_worker`，分别模拟 worker 发送 Goodbye 后继续存活，以及持续心跳但不读取 stdin。
3. 运行 `inherited_worker_environment_excludes_unlisted_secrets`、External CLI transport failure 与 run retention 跨进程锁回归。
4. 运行 Weixin provider 与 Browser mock 链路，确认 `env_clear` 后仅显式的 kind 专属非敏感控制变量仍可用。

预期：Goodbye 只有在 OS 进程树完成回收后才进入 Stopped；stdin 写满在生产超时内失败并回收 worker；worker 不继承 token/secret/password 等环境；External CLI wrapper 异常退出时实际 CLI 进程组被回收；保留仍持锁的运行中 run，同时可清理已释放锁且没有 `result.json` 的失败 run；Browser/IM E2E 开关通过 kind 白名单工作，不恢复环境全量继承。

### TC-AWI-16：Remote Broker relay 绑定与 grant 事务

1. 运行 `test_remote_invoke_ssh_e2e.sh`，建立本地 relay 与 SSH grant，依次验证 Full Trust、Shell、Files、Read-only、恢复 Full Trust、撤销。
2. 运行 `worker_runtime::remote_broker::tests` 与 `remote_invoke::grant_info_store::tests`。
3. 检查 shell 请求携带的旧 `policy_id` 被主进程丢弃，并按持久 grant `policy_binding` 重新选择；策略版本变化时拒绝执行。
4. 并发消费/撤销 grant，确认跨进程事务不会丢更新，失败授权不扣额度。

预期：broker token 只映射到单一 relay，caller 不能提交 relay；合法 shell 命令在主进程重选策略后成功，越权策略不能生效；grant validate/consume/revoke 使用同一跨进程事务与原子持久化；Remote Execution worker 不持有真实 Admin listener 坐标。

### TC-AWI-17：IM 跨进程配置、日志与手动 Schedule

1. 运行 `test_auxiliary_worker_isolation.sh`，创建 provider、target 和 script schedule 后立即手动 Run，不等待 15 秒 reconcile。
2. 断言 run 成功、stdout 包含 `awi-schedule-ok`、worker job operation 为 `im.run_schedule`，并从主进程 API 立即读到 `manual_run` history。
3. 运行 `test_weixin_provider_e2e.sh` 与 `test_external_runner_live_guide.sh`。
4. 运行 message/session/run store 并发回归，确认多实例写入不丢数据，损坏文件不在读取路径静默删除。

预期：Target/Route/Provider/Schedule 的新配置对 worker 立即可见；manual schedule 不回落主进程；run history、message log、session state 使用跨进程锁和原子替换；provider disconnect/cancel 等待 transport drain；Weixin 上下文与 Runner Guide/Stop 控制链路保持可用。

## 清理步骤

1. 通过生命周期 API 或协议 Shutdown 停止 worker。
2. 向测试记录的 Bifrost PID 发送 INT/TERM，超时后仅回收其已记录进程树。
3. 停止 echo/mock relay/provider。
4. 确认没有测试 PID 存活后删除临时数据目录。
5. 保留需要审计的日志时，复制到测试 artifact 目录后再清理。

## 执行记录

### 2026-08-19

使用当前分支构建的 `target/debug/bifrost`，以独立临时数据目录和动态端口连续执行自动化基线中的五项真实场景测试：

- `test_auxiliary_worker_isolation.sh`：PASS；覆盖 worker 隔离主链路及新增的手动 Schedule worker job、stdout 与主进程 run history 可见性。
- `test_asr_source_compression.sh`：PASS；真实 ffmpeg 压缩、产物校验和失败持久化均符合预期。
- `test_weixin_provider_e2e.sh`：PASS；`env_clear` 后显式 IM worker 环境白名单和 provider 链路可用。
- `test_remote_invoke_ssh_e2e.sh`：PASS；relay、grant、Shell policy 重选、Remote Execution 与 revoke 链路均符合预期。
- `test_external_runner_live_guide.sh`：PASS；IM worker 到主进程的 Guide/Stop 控制面与显式 Queue 语义未回归。

同时执行 Worker 生命周期、External CLI transport、Remote Broker、GrantInfoStore、MessageLogStore、SessionState 和 RunStore 的对应 Rust 专项回归，均为 PASS；这些专项测试与上述五项真实链路共同覆盖 TC-AWI-01～17。

所有脚本均通过各自的 trap 按已记录 PID 清理测试进程和临时目录；未使用 `pkill` 或 `killall`，未触碰正式 `9900` 服务。
