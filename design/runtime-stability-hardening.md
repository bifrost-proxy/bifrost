# Runtime Stability Hardening

## 状态

已实施并于 2026-08-20 完成单元、E2E、真实场景与 changed-lines 覆盖率验证。本文定义高资源压力下 Bifrost Core、Desktop watchdog 与 system proxy lifecycle helper 的 P0/P1 稳定性语义。

## 问题与根因

当前运行时把多个不同故障压缩成了同一个信号：Desktop watchdog 只探测主代理端口上的 Admin API。主 listener、Admin API、数据面和多数附加任务共享 Tokio runtime 与连接许可；因此 CPU、FD、连接或队列压力可能只让 Admin 请求超时，但 watchdog 会把它解释为 Core 已失效，主动终止仍存活的 managed child。系统代理仍指向旧端口时，重启耗时又形成系统级网络黑洞。

现有 lifecycle helper 已能在 parent PID 明确消失时快速进入 guarded recovery，但 restart 是阻塞等待，缺少短期 fail-open/fail-closed 策略，也没有稳定的 ownership generation、结构化事件和 doctor 输出。发生问题后只能拼接 Desktop bootstrap、daemon 与 system proxy 日志猜测。

## 用户目标验证清单

### 必须实现

- watchdog 不得只因 Admin API 超时终止 managed child。
- watchdog 使用四类信号：managed child exit、独立 scheduler heartbeat、数据面 loopback canary、Admin health。
- 独立 health lane 绑定随机 loopback 端口，由独立 OS 线程服务；主 Tokio scheduler 只更新时间戳。
- 资源压力进入 degraded/critical 后停止 Body/WS payload 持久化，暂停图标、Traffic 大查询、脚本测试和附加任务执行，拒绝新重任务；CONNECT、基础转发、用户主动发起的单次 Replay，以及 AI/IM、Remote Invoke、Scripts 的配置与状态控制面不进入拒绝路径。
- 结构化 lifecycle event 落盘，覆盖 PID、退出码/信号、各探针耗时、RSS、CPU、FD、活动连接、队列、恢复耗时和 system proxy action。
- system proxy owner state 包含 generation、target、原始代理、runtime/helper、策略和最近动作；提供 `bifrost system-proxy doctor`。
- lifecycle helper 发现 daemon 明确消失后立即启动 replacement。
- replacement 在配置的 3–5 秒 grace 内未 ready 时执行 fail-open 或 fail-closed。
- fail-open 的 restore 与 ready 后 re-enable 都必须校验相同 ownership generation，并确认期间没有外部代理/用户配置接管。

### 必须不破坏

- managed child 明确退出仍立即进入 bounded recovery。
- 外部/CLI-owned runtime 仍不由 Desktop 强制终止。
- 系统代理只在当前实际 target 与持久化 Bifrost owner 一致时恢复；generation 不一致时保留外部状态。
- CONNECT、普通 HTTP/HTTPS/SOCKS 转发不因资源降级被主动拒绝。
- 旧 `runtime.json`、`proxy_state.json` 与 `config.toml` 缺少新增字段时按兼容默认值读取。
- Linux 不获得系统代理写能力；Windows 不引入 macOS-only API。

## Watchdog 多信号状态机

每轮采样：

1. `managed_child_exit`：`Child::try_wait()` 明确返回退出。该信号独立成立，立即走既有 bounded recovery。
2. `scheduler_heartbeat`：Desktop 读取 `runtime.json.health_port`，访问独立 health lane。响应包含主 Tokio scheduler 最后心跳年龄与资源快照。
3. `data_plane_canary`：经主代理端口请求本地内建 `bifrost-runtime-canary.invalid` canary，不访问外网，覆盖 accept、HTTP parse 与数据面 dispatch。
4. `admin_health`：保留 Admin support API 探针，只代表管理面就绪性。

对仍存活的 managed child，决策真值如下：

| Admin | Data plane | Scheduler | 动作 |
| --- | --- | --- | --- |
| fail | healthy | 任意 | 保留 PID；管理面 degraded |
| fail | fail | fresh | 保留 PID；进入 resource degradation |
| 任意 | fail | stale | 继续累计故障窗口 |
| fail | fail | stale | 达到连续次数与 grace 后才允许 bounded recovery |

最终确认阶段必须重新执行三类探针；任一信号恢复即取消 recovery。health lane 不可达本身不是 kill 证据，必须与数据面和 Admin 同时失败。

## 独立 health lane

- Core 启动时在 `127.0.0.1:0` 绑定 TCP listener，将端口写入 `runtime.json.health_port`。
- listener 在命名 OS 线程运行，不占 Tokio connection semaphore。
- 仅接受 loopback，固定响应小型 JSON；不提供修改能力。
- Tokio task 每 500ms 更新 scheduler heartbeat，并同步 active connections / traffic queue depth。
- health 线程采样 RSS、CPU、FD，计算 resource pressure，并通过全局原子 governor 发布 `normal/degraded/critical`。
- health lane stop handle 随主 runtime 生命周期释放；旧客户端忽略新增 runtime 字段。

## 资源压力降级

信号使用带回滞的三档控制器：

- `degraded`：RSS/系统内存、FD、活动连接、traffic queue 或 scheduler lag 任一达到软阈值。
- `critical`：任一达到硬阈值。
- 恢复：连续多个正常采样后才回到 `normal`，避免频繁抖动。

降级矩阵：

| 能力 | degraded / critical |
| --- | --- |
| CONNECT、基础 HTTP/HTTPS/SOCKS 转发 | 保留 |
| BodyStore / WsPayloadStore 新 payload | 停止持久化，已有 reader 保留 |
| app icon miss / bundle 扫描 | 503，不启动新提取 |
| Traffic list/query/batch/search | 503；轻量单条读取与健康接口保留 |
| Scripts 列表、读取、保存、重命名、删除 | 保留，允许修正配置；不触发脚本执行 |
| Scripts test、req/res/decode 新执行 | 503 或跳过，并记录 pressure reason |
| AI/IM providers、channels、runner config、routes、schedules CRUD | 保留，避免整个 AI/IM 管理面失效 |
| AI/IM 新消息发送、附件上传、Agent/runner turn、schedule 手动 run | 503，不启动新重任务 |
| Remote Invoke 状态、授权、连接与访问控制管理 | 保留；这些 Admin API 不直接执行远端命令 |
| worker jobs、ASR 等附加重任务 | 503，不启动新任务 |
| 单次 Replay、Replay 收藏与历史 | 保留；继续使用并发上限，payload 持久化仍服从 pressure governor |

所有拒绝使用稳定错误码/消息；降级状态变化写 lifecycle event，不对每个请求刷日志。

## System proxy ownership generation

`proxy_state.json` 升级为兼容 schema：

```json
{
  "schema_version": 2,
  "generation": "uuid-v7",
  "original": { "enable": false, "host": "", "port": 0, "bypass": "" },
  "target": { "enable": true, "host": "127.0.0.1", "port": 9900, "bypass": "..." },
  "applied": true
}
```

- 首次接管或 target 变化生成新 generation。
- 同 target restart handoff 保留 generation 与原始代理快照。
- `suspend_if_generation(g)`：持 system proxy lock，确认 state generation 和实际 target 后恢复 original；保留 state 并标记 `applied=false`。
- `resume_if_generation(g)`：确认 generation 未变化、state 仍 suspended、实际代理仍等于 original；满足后才恢复 target。
- generation 或实际状态不匹配返回 `OwnershipChanged`，不写系统代理。

`system_proxy_owner_state.json` 是可诊断投影，不替代 `proxy_state.json` 的写入依据。它通过独立 diagnostics lock 原子更新，包含 runtime/helper、policy、phase、last action/error。诊断路径只读取现有 ownership，不迁移 state、不申请 system proxy mutation lock；diagnostics lock 最多等待 2 秒，超时记录 warning 并让核心启动/重启/转发链路继续。

## Helper recovery 状态机

配置项位于 `[system_proxy]`：

- `recovery_mode = "fail_open" | "fail_closed"`，默认 `fail_open`。
- `recovery_grace_secs = 5`，保存时限制在 3–5 秒。

parent 明确退出后：

1. 读取 restartable runtime、owner generation 与 policy。
2. 立即 `spawn` replacement，不阻塞等待启动命令退出。
3. 每 250ms 检查本地数据面 canary readiness。
4. grace 内 ready：保留代理，记录 recovered。
5. grace 超时：
   - fail-closed：保持当前代理，继续等待总恢复预算。
   - fail-open：调用 `suspend_if_generation(g)` 恢复直连/原始代理，replacement 后台继续启动。
6. replacement ready：若曾 suspend，调用 `resume_if_generation(g)`；generation 或实际代理改变则保持外部状态。
7. 总预算耗尽：fail-open 保持直连；fail-closed 保持代理并输出 critical doctor finding。

Desktop-owned runtime 不由 helper 拉起，仍由 Desktop child watchdog 管理；前台 runtime 默认不 restart。

## 生命周期事件与 doctor

事件文件：`logs/system_proxy_events.jsonl`，单行 JSON、10 MiB rotation、最多 3 份。独立 `.system_proxy_diagnostics.lock` 串行化跨进程 append 与 owner state 原子更新；锁等待有界，诊断竞争不得阻塞 daemon 恢复。

公共字段包括：

- `ts/schema_version/event/component/trigger/decision/error`
- `old_pid/new_pid/exit_code/exit_signal`
- `admin_probe_ms/data_plane_probe_ms/health_lane_probe_ms/scheduler_heartbeat_age_ms`
- `rss_bytes/cpu_percent/fd_count/fd_limit/active_connections/queue_depth/queue_capacity`
- `ownership_generation/system_proxy_action/recovery_elapsed_ms`

`bifrost system-proxy doctor [--format text|json|json-pretty]` 只读聚合：runtime marker、owner/proxy state、helper heartbeat、health lane、当前 system proxy ownership、最近 lifecycle events，并给出 `ok/warn/critical` findings。doctor 不修改系统代理。

## 验证路由

- 单元测试：watchdog 真值表、pressure 回滞、payload/重任务降级、generation suspend/resume 决策、helper fail-open/fail-closed 时间线、旧 schema 兼容。
- E2E：独立 data-dir + 动态端口验证 Admin 单独失败不重启、health lane/数据 canary、pressure forced mode、helper policy 与 generation conflict。
- E2E：Critical 压力下验证 AI/IM providers、Agent config、channel config、Remote Invoke 状态和 Scripts CRUD 可用，同时 Scripts test 与新 AI turn 仍返回 503。
- human tests：macOS Desktop pause/kill、真实 system proxy 快照恢复、fail-open 黑洞窗口、外部代理接管、doctor 证据完整性。
- Rust 门禁：受影响 crate 测试、fmt/clippy、`make coverage-changed`，再由 GitHub Actions 执行 workspace/coverage/E2E。
