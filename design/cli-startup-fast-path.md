# CLI 启动快路径优化

## 背景

`bifrost start` 是 CLI 用户与代理服务的第一次接触。核心 SLO 是“秒级监听”——从命令回车到端口开始 accept 应尽量落在 1s 附近。历史实现里有若干阻塞主线程的操作：

- 同步 `check_and_print_update_notice()` 打 GitHub API，网络抖动时可能等数秒。
- System proxy 恢复 + 启用（`SystemProxyManager::recover_from_crash` / `enable`）在启动关键路径上跑，`networksetup` / `osascript` 阻塞会拖慢 listener bind。
- Remote Invoke worker 在启动路径上初始化，还要读整个 call history JSON，越用越慢。
- `FrameStore` 启动时预热 `frames/*.meta.json` 全量文件。
- Daemon 模式没有 readiness pipe，导致父进程可能在 listener 还没就绪时就报告成功。
- 默认 `info` 日志刷了太多常态生命周期事件（连接提前关闭、SSE ping、规则命中详情），既噪音又慢。
- 缺少统一的“阶段耗时”日志，用户报“启动慢”时无法定位。

本方案是一次“启动快路径 + 可观测性”重构，不改代理业务语义。目标是把关键路径压到最短，同时给运维留一条可诊断的日志。

## 用户目标验证清单

### 必须实现

- `bifrost start` 前台模式下，不阻塞主线程做 GitHub 版本检查；改为后台线程异步执行。
- `bifrost start --daemon` 下完全不打印 `Update available` 到控制台，避免污染 daemon 日志。
- System proxy 恢复 (`recover_from_crash`) + 启用 (`enable`) 从启动关键路径拆出，放到 listener 成功 bind 之后的后台线程。
- Remote Invoke worker 初始化从启动路径搬到后台任务，call history 从整文件 JSON 迁到 JSONL 追加。
- `FrameStore` 启动时不再预热 `*.meta.json`，改用 SQLite `frame_connection_metadata` 表按需查询 + LRU cache。
- 增加统一 `bifrost_cli::startup` 阶段日志（每阶段耗时 + 总耗时）。
- Daemon 模式增加 readiness pipe：父进程只在子进程真正 bind listener 后才报告 `Daemon started`。
- 前台 listener task 挂掉时主进程立即返回错误，避免“进程在、端口不监听”的假运行。
- Daemon 日志级别继承 CLI `--log-level`，`RUST_LOG` 仍优先。
- 默认 `info` 日志降噪：常态生命周期事件降级到 `debug`，规则命中详情降级到 `debug`/`trace`。

### 必须不破坏

- `bifrost start` 无冲突场景下的用户观感（打印顺序、成功消息、system proxy 状态最终一致）与既往一致。
- 代理业务语义（规则匹配、值解析、TLS 拦截、上游转发）零改动。
- 端口重绑（`bifrost port bind/update`）仍复用同一套 listener 生命周期管理。
- Remote Invoke 对外 API 语义不变；后台初始化未完成时短暂返回未启用是可接受降级。
- 现有 `RUST_LOG` 精确过滤能力保留；能通过 `RUST_LOG=bifrost_core::rules=debug,bifrost_proxy::rules=trace,info` 恢复详细规则命中日志。
- 旧 `admin/remote_invoke_call_history.json` 存在时被删除（不做 in-place 迁移），新 JSONL 文件按 client-key 分片。
- 旧 `frames/*.meta.json` 存在时被忽略/不参与查询，`FrameStore` 只信 SQLite。

### 必须真实验证

- 干净数据目录下从 `bifrost start` 回车到端口 accept 应在 1s 附近；`bifrost_cli::startup` 日志里能看到每阶段耗时和总耗时。
- 断网环境下 `bifrost start` 不再因 GitHub API 超时被拖慢；`Update available` 提示可能延迟出现但不阻塞。
- Daemon 模式下先起一个 dummy TCP holder 占端口，`bifrost start --daemon` 必须非零退出、不打印 `Daemon started with PID`。
- 前台 listener bind 后手动 kill listener task（模拟 UDP relay bind fail），主进程立即错误退出。
- 默认 `info` 日志下跑一天：不再看到规则命中详情、SSE ping、`hyper::Error(IncompleteMessage)`、WebSocket 正常关闭事件。

## 产品语义

### 启动快路径的四条原则

1. **主线程只做 listener bind 不可让路的事**：配置加载、DB 初始化、admin state 构建、规则解析、listener bind。
2. **任何 I/O 到外部服务（GitHub、system proxy 系统调用、写系统级证书 trust）都必须能后台化**。
3. **Daemon readiness = listener bind 完成**：父进程用 pipe 等 readiness 信号，不用 sleep。
4. **可观测性优先于花活**：每个阶段一行 `startup phase X: Yms` 日志，总耗时一行。

### 更新提示的两种形态

- 前台模式：主线程 spawn 一个 `spawn_update_check_notice()` 后台线程。它异步查 GitHub，成功后追加打印一段 update banner；失败静默。
- Daemon 模式：完全不打印 update banner。理由是 daemon 会被 systemd/launchd/自定义 supervisor 抓 stdout；控制台里出现意外的 banner 会污染日志格式，而且 daemon 用户没法看到。

### System proxy 后台 reconcile

启动流程改为：

```
1. listener 成功 bind
2. 打印 "Requested (applying asynchronously)"（system proxy 状态行）
3. spawn 后台线程：
   a. SystemProxyManager::recover_from_crash 收回上次崩溃留下的 backup
   b. SystemProxyManager::enable 按当前配置真正启用
4. 后台线程完成后，通过 admin state 更新 system proxy 展示状态
```

即使系统层慢或需要授权，代理监听已经能承接流量。用户在 admin UI 上看到的状态可能短暂显示 “Requested”，几百毫秒后转为 `Enabled`。

### 阶段日志格式

`bifrost_cli::startup` target 下的 `info` 日志格式统一：

```
startup phase config load: 12ms
startup phase traffic db init: 43ms
startup phase frame store init: 5ms
startup phase config storage load: 21ms
startup phase app icon cache init: 3ms
startup phase script manager init: 6ms
startup phase replay db init: 7ms
startup phase admin state build: 14ms
startup phase rules parse + resolver init: 22ms
startup phase replay executor / push / metrics / watcher start: 9ms
startup phase proxy listener bind: 18ms
startup total: 187ms
```

用户报“启动慢”时，直接看这几行就能定位是数据库、规则、帧缓存还是其它模块。

## 技术细节

### 关键文件

- `crates/bifrost-cli/src/main.rs`：不在 daemon 分支提前初始化 tracing；把 update banner 移到后台。
- `crates/bifrost-cli/src/commands/update_check.rs`：新增 `spawn_update_check_notice()`。
- `crates/bifrost-cli/src/commands/start.rs`：
  - 加各阶段 `startup phase X: Yms` 日志和总耗时日志。
  - 新增 `spawn_managed_proxy_task()`：返回 `JoinHandle<Result<()>>`，主循环通过 `tokio::select!` 监听 shutdown、端口重绑、listener 结束。
  - 加 readiness pipe（daemon）。
  - system proxy reconcile 移到后台。
  - Remote Invoke worker 移到后台调度。
- `crates/bifrost-cli/src/commands/port.rs`：复用 `spawn_managed_proxy_task` 处理端口重绑。
- `crates/bifrost-core/src/logging.rs`：`reinit_logging_for_daemon(cli_log_level: &str)`，daemon 子进程按 CLI 参数初始化。
- `crates/bifrost-admin/src/state.rs`：`set_remote_invoke_worker(worker)` 支持后台注入。
- `crates/bifrost-admin/src/remote_invoke/call_history_store.rs`：JSONL 追加 + 按 client-key 分片 + `max_records=1000` compaction。
- `crates/bifrost-admin/src/frame_store.rs`：`frame_connection_metadata` 表 + LRU cache。
- `crates/bifrost-core/src/rules` + `crates/bifrost-proxy/src/rules`：日志降级。

### `spawn_managed_proxy_task` 语义

- 输入：绑定后的 listener、shutdown token、reload channel。
- 输出：`JoinHandle<Result<()>>`。
- 前台主循环：

```rust
tokio::select! {
    _ = shutdown.cancelled() => break,
    Some(req) = reload_rx.recv() => handle_port_reload(req).await?,
    result = &mut listener_handle => {
        let err = result??.err().unwrap_or_else(|| anyhow!("listener exited unexpectedly"));
        return Err(err);
    }
}
```

任何 listener 侧的 fatal 错误（UDP relay bind fail、accept loop 崩、内部 panic）都会立刻传播出来，主进程退出，runtime.json 由 stop 收敛。

### Daemon readiness pipe

```
parent:
  let (rx, tx) = pipe();
  fork();
  parent: close(tx); wait for rx (timeout N sec);
    - EOF/timeout -> exit(1), print "daemon failed to become ready"
    - byte 0x01 -> print "Daemon started with PID X", exit(0)

child:
  close(rx);
  reinit tracing with --log-level;
  do full startup...;
  after listener bind ok: write(tx, 0x01); close(tx);
  enter proxy loop.
```

Bind 失败或 startup 中途 panic 时，child 直接退出，parent 收到 EOF，从错误退出。

### Frame metadata 表

```sql
CREATE TABLE frame_connection_metadata (
    connection_id TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    frame_count INTEGER NOT NULL DEFAULT 0,
    last_frame_id INTEGER NOT NULL DEFAULT 0,
    is_closed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_frame_metadata_updated
    ON frame_connection_metadata(updated_at DESC);
CREATE INDEX idx_frame_metadata_closed_updated
    ON frame_connection_metadata(is_closed, updated_at DESC);
```

- 写 frame 文件时同步 upsert 一行。
- 关闭连接时把 `is_closed=1`。
- 读 metadata 优先命中 LRU；miss 走 SQL。
- 过期清理直接 `SELECT connection_id WHERE is_closed=1 AND updated_at<?`，再删 frame 文件。
- 旧 `.meta.json` 不迁移、不读、不 fallback。

### Remote Invoke JSONL 存储

- 路径：`admin/remote_invoke_call_history/<client-key>.jsonl`。
- 写：每次 call 开始/完成/失败/取消 追加一行完整快照，不预读全量历史。
- 读：`GET /_bifrost/api/remote-invoke/calls`，支持 `limit`、`before` 游标，按 `call_id` 取最新快照。
- Compaction：超过 `max_records=1000` 或探测到坏行时，重写 JSONL 只留最新 1000 条。
- 旧 `admin/remote_invoke_call_history.json` 在启动时被删除。
- 前端 Recent Calls 默认拉 100 条。

### 日志降噪清单

- `hyper::Error(IncompleteMessage)`（短连接提前关闭）：`error` → `debug`。
- macOS 短连接的 client attribution miss：`warn` → `debug`。
- WebUI push WebSocket 注册/正常关闭/客户端主动关闭：`info` → `debug`。
- Remote Invoke SSE `ping` 心跳：`debug` → `trace`（不再默认输出）。
- `bifrost_core::rules::rule matcher candidate matched`：`info` → `debug`。
- `bifrost_core::rules::rule selected`：`info` → `debug`。
- `bifrost_proxy::rules::rules matched for request`：`info` → `debug`。
- `bifrost_proxy::rules::matched rule detail`：`info` → `trace`。
- 未变：HTTP 服务错误保持 `error`；WebSocket 协议/IO 错误保持 `warn`；非 ping SSE 事件保持业务日志级别。

## CLI / Web / Admin API 呈现

### CLI

- 无新增子命令；`bifrost start` 输出可能新增一行 `System proxy: Requested (applying asynchronously)`。
- 前台模式 `Update available` banner 可能延迟出现或不出现（网络不可达时）。
- `RUST_LOG=info` 下能看到 `bifrost_cli::startup phase ... : Xms` 阶段日志。

### Web

- Settings 页面显示的 system proxy 状态可能短暂显示 `Requested`，然后转 `Enabled`。前端已存在的 polling / websocket 通道无需改。

### Admin API

- 无 API 契约变化。`/api/remote-invoke/calls` 新增 `limit` + `before` 分页参数，默认返回一页。

## Sync 边界

- 本方案属于本机启动路径优化，不参与跨设备同步。
- Remote Invoke JSONL 是每个 client 独立文件；relay 侧只做转发，不感知本地存储格式变化。
- Frame metadata SQLite 表是本机流量记录一部分，不在 sync 范围内。

## 实现切分

### Phase 1：可观测性 & 更新检查异步化

- 增加 `bifrost_cli::startup` 阶段日志与总耗时。
- `check_and_print_update_notice` → `spawn_update_check_notice`（后台线程）；daemon 完全不打印。

### Phase 2：System proxy 后台 reconcile

- listener bind 后 spawn 后台线程执行 recover_from_crash + enable。
- 前台状态展示 `Requested (applying asynchronously)`。

### Phase 3：Listener 生命周期 + Daemon readiness

- `spawn_managed_proxy_task` 返回 `JoinHandle`，主循环 `tokio::select!`。
- Daemon readiness pipe：只在 listener bind 后写 readiness。
- `main.rs` daemon 分支不提前初始化 tracing；子进程 `reinit_logging_for_daemon` 继承 `--log-level`。

### Phase 4：Remote Invoke 后台 worker + JSONL 存储

- Worker 构造只用 identity/crypto/policy/info，不读 call history。
- 后台任务完成后 `set_remote_invoke_worker` 注入。
- JSONL 追加 + 按 client-key 分片 + compaction。
- 旧整文件历史在启动时删除。
- API 增加 `limit` / `before` 分页。

### Phase 5：Frame metadata SQLite 化

- `frame_connection_metadata` 表 + 索引。
- Upsert / mark_closed / lookup / list / cleanup 全部走 SQL + LRU cache。
- 不再读 `*.meta.json`。

### Phase 6：日志降噪 + 手工回归

- 按降噪清单调等级。
- 更新 `human_tests/cli-log-output-default.md` TC-LOD-07 / TC-LOD-08。

## 测试方案

### E2E

1. 干净数据目录 `BIFROST_DATA_DIR=./.bifrost-test-<run-id> cargo run --bin bifrost -- start -p <PORT> --unsafe-ssl`，断言启动可访问、`bifrost_cli::startup` 日志包含 `startup total: <n>ms`。
2. Daemon: `-l debug start ... --daemon`，断言文件日志出现 `DEBUG`；父进程仅在 readiness 到达后打印 `Daemon started with PID`。
3. `bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh`：
   - 前台起 dummy TCP holder 占端口 → 主 listener bind 失败 → 主进程退出、admin API 不可达；
   - 断言错误消息包含 `already in use` 或 `another process is already listening on this port`；
   - Daemon 版本：父进程返回非零、不打印 `Daemon started`，接受 `readiness wait failed` 或 `already in use` 错误。
   - Admin API 探针使用短 `connect-timeout` / `max-time` 避免 CI 挂起。

### 真实场景

- `human_tests/cli-start-stop-status.md` TC-CSS-26 / TC-CSS-27：交互重启回归。
- `human_tests/cli-log-output-default.md` TC-LOD-07：默认 `info` 日志不再刷常态生命周期事件。
- `human_tests/cli-log-output-default.md` TC-LOD-08：默认 `info` 不再输出规则命中详情；`RUST_LOG=bifrost_proxy::rules=trace` 时可看到。
- `human_tests/cli-start-stop-status.md` TC-CSS-32：构造旧 JSON + 新 JSONL 历史，确认旧 JSON 被删、启动日志先出 proxy listener bind 再异步完成 Remote Invoke worker。

### 单元

- `spawn_update_check_notice`：后台线程执行、失败静默。
- `check_and_resolve_port_conflict_returns_ok_when_port_free`（已落地）。
- Frame metadata：upsert / mark_closed / cleanup by is_closed+updated_at 断言。
- Remote Invoke JSONL：append → read → compaction 后仍能读到最新快照。

## Review / Fix / Test 闭环

- 先跑启动链路 E2E + `test_startup_listener_readiness_e2e.sh`。
- `rust-project-validate`：fmt / clippy / test / build。
- daemon 模式手动跑 TC-CSS-26 / TC-LOD-07 / TC-LOD-08 / TC-CSS-32。

## 风险与决策

- **决策 1**：Update banner 在 daemon 完全静默。理由：daemon stdout 可能被 systemd 抓成 journal，banner 会污染日志。用户仍可 `bifrost version-check` 手动查。
- **决策 2**：system proxy 后台化时“先设 Requested，后转 Enabled”的两阶段状态是对外可见的。理由：不能骗用户说“已启用”；两阶段状态比同步阻塞几秒更能被用户理解。
- **决策 3**：Frame metadata / Remote Invoke 历史都不做 in-place 迁移。理由：一次性丢弃可以省掉迁移路径的持续维护；用户可接受“历史 traffic 元数据丢失”，因为流量 body 本身仍在。
- **决策 4**：默认 `info` 里彻底关掉规则命中详情。理由：即使一台机器 QPS 不高，规则命中日志一天也会打上百 MB；反例是保留摘要 + 关闭详情，但摘要格式化本身也不便宜。
- **风险**：Daemon readiness pipe 在 Windows 上没有真正的 fork，需要另一条 handshake（例如共享内存 + Event）。当前实现优先覆盖 macOS / Linux；Windows 侧作为 P2 补齐。
- **风险**：System proxy 后台 reconcile 期间用户如果立刻 `bifrost stop`，需要能 join 掉未完成的后台线程。缓解：把后台线程 handle 也放进 admin state，`stop` 走 shutdown token + join with timeout。

## 文档更新要求

- 本方案是 CLI 启动性能与可观测性优化，README 无需改。
- `docs/troubleshooting.md`：新增“启动慢排查”一节，引导看 `bifrost_cli::startup` 阶段日志。
