# Desktop 后端 watchdog 稳定性

## 背景

Desktop 当前每 2 秒探测一次后端管理接口，单次超时 450ms，连续两次失败就触发恢复。用户日志显示，在系统代理高负载期间，仍存活的后端因为管理接口短时无法在 450ms 内响应，被 watchdog 连续回收并拉起，代理 PID 在数分钟内变化 5 次。依赖系统代理长连接的 Codex 因此反复断线重连；关闭系统代理后绕过该链路即恢复。

上游连接稳定性治理已经通过全局背压、渐进连接池淘汰和资源压力退避降低后端过载概率，但 watchdog 仍需要区分“进程已经退出”和“进程存活但管理面暂时迟滞”。

## 用户目标验证清单

### 必须实现

- Desktop 管理的后端进程真实退出时，watchdog 继续快速恢复。
- 后端进程仍存活时，短时健康探针超时只进入 degraded 状态，不立即终止进程。
- 持续不可用超过有界宽限窗口后，使用更长超时的最终确认探针；确认仍失败才恢复。
- 日志记录连续失败次数、degraded 持续时间、探针错误和恢复结果，便于区分拥塞、拒绝连接与 HTTP 错误。

### 必须不破坏

- 不改变 Desktop/CLI runtime ownership、升级 handoff、退出时停止 Desktop-owned core 的现有语义。
- 不修改代理规则、TLS、Traffic、系统代理设置或正式 `9900` 数据目录。
- 外部 CLI-owned 后端持续不可用时仍进入手动启动门禁，不由 Desktop 擅自替换。
- watchdog 在 Desktop shutdown、force exit 或已有 recovery 进行中时不启动新的恢复。

### 必须真实验证

- 单元测试覆盖短时失败、失败后恢复、最少失败次数、宽限窗口、最终确认和探针错误诊断。
- macOS E2E 对隔离 Desktop-owned core 发送 `SIGSTOP`，暂停 12 秒（超过旧版两次失败门槛但低于新版 15 秒宽限）后恢复，断言 PID 不变且健康端点恢复。
- 同一 E2E 随后真实终止 core，断言 watchdog 拉起不同 PID，证明退出恢复没有被宽限窗口拖慢。
- human test 按隔离数据目录与动态端口执行，不影响正式服务。
- 远端 CI 执行 `bash scripts/ci/coverage-all.sh --json --gate` 覆盖率门禁。

## 设计

### 两级故障判定

watchdog 保留原有 2 秒轮询，但把 HTTP 健康失败交给独立状态机：

1. 第一次失败记录 `first_failure_at` 和连续失败次数，进入 degraded。
2. 任意一次成功立即清空失败窗口；如果此前 degraded，记录恢复耗时和失败次数。
3. 只有同时满足“至少 4 次连续失败”和“degraded 已持续 15 秒”才进入最终确认。
4. 最终确认使用 3 秒超时；成功则保留当前进程，失败才执行现有 recovery。

最少失败次数避免线程调度停顿造成时间窗口瞬间跨越；时间窗口避免短时间内快速失败直接触发重启。两者必须同时满足。

### Liveness 与 readiness 分层

- `poll_managed_backend_exit` 是 liveness 信号：子进程已经退出时立即进入恢复，不等待 HTTP 宽限窗口。
- HTTP 管理接口是 readiness 信号：进程存活但接口短时无响应时只标记 degraded。
- 外部 CLI-owned 后端没有 Desktop child handle，持续 readiness 失败后仍按现有逻辑标记需要手动启动，不替换外部进程。

### 探针可观测性

健康探针返回结构化结果，而不是单一布尔值：

- 是否健康；
- 实际耗时；
- client 构建失败、请求失败或非成功 HTTP 状态的摘要。

日志不包含请求头、认证信息或响应正文，只记录本地健康 URL 的错误类别与耗时。

### 模块边界

- `backend_runtime/watchdog.rs` 独立承载 watchdog 状态机与轮询编排，`backend_runtime.rs` 保留进程恢复、ownership 和启动生命周期能力。
- `tests/watchdog.rs` 独立承载状态机与探针单元测试，避免 Desktop 主实现和主测试模块超过仓库 1500 行门禁。

## 测试方案

### 单元测试

- 单次和多次短时失败保持 degraded，不触发最终确认。
- 即使持续时间超过 15 秒，少于 4 次失败也不确认。
- 即使失败次数达到 4 次，未满 15 秒也不确认。
- 同时满足两个门槛才确认；随后健康会重置状态。
- delayed health server 超过自定义超时会返回带耗时和错误摘要的失败结果。
- 非 2xx 健康响应被识别为失败。

### E2E 与 human test

扩展 `e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`：

1. 使用临时 data-dir、动态端口和 `--no-system-proxy` 启动真实 Desktop-owned core。
2. `SIGSTOP` 暂停 core 12 秒，超过旧版约 4～5 秒误杀门槛但低于 15 秒宽限，再 `SIGCONT`。
3. 断言 runtime PID 未变化、健康端点恢复、日志出现 degraded/recovered 且没有 watchdog recovery。
4. `SIGKILL` 真实终止同一 core，断言 watchdog 在有界时间内拉起不同 PID 并恢复健康。
5. 继续执行既有 CLI stop/restart ownership 与 Desktop graceful shutdown 回归。

## 风险与边界

- 存活但永久卡死的 managed core 从首次探针失败到最终确认失败约需 18 秒；加上首次轮询等待，通常在约 20 秒后开始恢复。这是避免误杀活进程的刻意取舍；真实进程退出仍按 2 秒轮询快速恢复。
- `SIGSTOP` 是 Unix/macOS E2E 手段，脚本在非 macOS 或无 WindowServer 时按既有规则跳过；状态机单元测试在通用 Rust 测试中提供跨平台覆盖。
- 本轮不修复系统代理重启后 bypass 为空的问题，该问题与 watchdog 误判分开交付，避免混入不同配置生命周期边界。
