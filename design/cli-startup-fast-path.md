# CLI 启动快路径优化

## 功能模块详细描述

优化 `bifrost start` 的前台启动体验，目标是让代理核心启动尽快进入监听态，并为后续慢启动问题提供稳定、可读的耗时日志。

当前前台启动存在两个问题：

- CLI 在进入 `start` 主流程前会同步检查版本更新，网络抖动时可能阻塞数秒。
- 启动路径缺少统一的阶段耗时日志，出现慢启动时难以判断是数据库、规则、帧缓存还是其他模块导致。
- 若启用了 system proxy，启动/daemon 链路会同步执行系统代理恢复和设置，阻塞端口监听。

本次改动只调整 CLI 启动路径的执行时机和可观测性，不改变代理功能语义。

## 实现逻辑

### 1. 更新检查移出启动关键路径

- 对 `bifrost start` 前台模式，不再在主线程同步执行 `check_and_print_update_notice()`
- 改为后台线程异步执行，避免 GitHub API 请求阻塞端口监听
- 对非 `start` 命令，保持原有同步行为
- 对 `start --daemon`，不打印更新提示，避免后台守护进程产生额外控制台输出

### 2. 启动阶段耗时日志

- 在 `crates/bifrost-cli/src/commands/start.rs` 中为关键初始化阶段增加 `bifrost_cli::startup` 的 `info` 日志
- 覆盖的阶段包括：
  - 配置加载
  - body / ws payload / traffic db / frame store 初始化
  - config storage 加载
  - app icon cache / script manager / replay db 初始化
  - admin state 构建
  - 规则解析与 resolver 初始化
  - replay executor / push / metrics / watcher 启动
  - 代理 listener bind
- 增加启动总耗时日志，便于判断是否满足秒启动目标

### 2.1 System proxy 改为后台 reconcile

- `bifrost start` / daemon 模式下，不再在关键启动路径里同步执行：
  - `SystemProxyManager::recover_from_crash`
  - `SystemProxyManager::enable`
- 改为在代理 listener 成功绑定后启动后台线程执行：
  - 先恢复 crash 遗留的系统代理 backup
  - 再按当前配置尝试启用 system proxy
- 这样即使系统层操作较慢、需要管理员授权、或 `networksetup` / `osascript` 阻塞，也不会拖慢核心代理启动
- 前台状态展示改为：
  - `Requested (applying asynchronously)`
- 端口重绑时仍只在“system proxy 已成功启用”的前提下尝试同步更新；该路径不是常规启动热路径

### 2.2 Daemon 日志级别继承 CLI 参数

- `main.rs` 在 `start --daemon` 模式下不再提前初始化 tracing，避免父进程持有前台日志输出状态
- daemon 子进程在 `fork` 后调用 `reinit_logging_for_daemon(...)` 时，会显式继承 CLI 传入的 `--log-level`
- 若设置 `RUST_LOG`，仍保持 `RUST_LOG` 高于 `--log-level` 的优先级
- 未显式传参时，默认值仍为 `info`，与 CLI 参数默认行为一致
- 这样 daemon 模式下的 `bifrost_cli::startup`、规则加载和运行期 tracing 日志不再被硬编码为 `info`

### 2.3 Listener 生命周期与 daemon readiness

- 前台模式不再把 `run_with_listener()` 作为只记录日志的 detached task。
- `spawn_managed_proxy_task()` 返回 `JoinHandle<Result<()>>`，前台主循环通过 `tokio::select!` 同时监听：
  - shutdown 信号
  - 端口重绑请求
  - proxy listener task 结束
- 如果 listener task 因 UDP relay bind 失败、accept loop fatal error、panic 等原因提前退出，主流程立即返回错误，避免出现“进程仍在、runtime info 仍在、端口不监听”的假运行状态。
- daemon 模式在 `fork()` 前创建 readiness pipe；父进程只在子进程完成真实 listener bind 并写入 readiness 信号后才打印 `Daemon started with PID` 并返回 0。
- 子进程启动失败、listener bind 失败或初始化阶段提前退出时，父进程会收到 pipe EOF/超时并返回非零错误，避免 daemon 模式提前报告启动成功。

### 2.4 默认日志降噪

- 默认 `info` 日志只保留启动阶段、真实错误、用户可感知状态变化和必要的业务事件。
- 以下常态生命周期事件降级为 `debug`：
  - 短连接提前关闭导致的 `hyper::Error(IncompleteMessage)`
  - macOS/短生命周期连接无法反查客户端进程的归因 miss
  - WebUI push WebSocket 注册、正常关闭和客户端主动关闭
  - Remote Invoke SSE `ping` 心跳分发
- 保持真实异常路径的等级不变：HTTP 连接服务错误继续 `error`，WebSocket 协议/IO 错误继续 `warn`，异步进程解析任务失败继续 `warn`，非 ping SSE 事件继续按现有业务日志输出。

### 3. Frame metadata 落入 SQLite 独立表

- `FrameStore metadata` 不再存储/读取 `frames/*.meta.json`
- 改为写入现有 admin 侧 `traffic.db` 中的独立表 `frame_connection_metadata`
- `FrameStore` 启动时只初始化 SQLite 连接与表结构，不再预热历史 metadata
- 查询路径改为按需从 SQLite 读取，并用进程内 cache 做热点复用
- 清理逻辑改为直接按 SQL 查询过期且已关闭的连接，再删除对应 frame 文件
- 由于本次明确不兼容历史 metadata 文件，旧 `.meta.json` 不参与迁移

表结构：

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

读写策略：

- 写入 frame 文件时同步 upsert metadata 行
- 关闭连接时将 `is_closed` 标记为 `1`
- 读取 metadata 时优先命中进程内 cache，miss 时按 `connection_id` 查 SQLite
- 列出连接和过期清理直接走 SQLite，不再依赖启动期全量预热

## 依赖项

- `crates/bifrost-cli/src/main.rs`
- `crates/bifrost-cli/src/commands/update_check.rs`
- `crates/bifrost-cli/src/commands/start.rs`

## 测试方案（含 e2e）

1. 使用临时数据目录执行 `BIFROST_DATA_DIR=./.bifrost-test-<run-id> cargo run --bin bifrost -- start -p <PORT> --unsafe-ssl`
2. 确认服务可以正常监听，且更新提示不会阻塞启动
3. 使用 `RUST_LOG=info` 观察 `bifrost_cli::startup` 日志，确认能看到阶段耗时与总耗时
4. 使用 daemon 模式执行 `BIFROST_DATA_DIR=./.bifrost-test-<run-id> cargo run --bin bifrost -- -l debug start -p <PORT> --unsafe-ssl --daemon`，确认文件日志中出现 `DEBUG` 级别输出
5. 执行 `bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh`：
   - 前台启动时占用同端口 UDP，验证 listener task 失败后主进程退出且 admin API 不可达
   - daemon 启动时占用同端口 UDP，验证父进程返回非零、不会打印 daemon started
6. 真实场景测试：更新并执行 `human_tests/cli-start-stop-status.md` 中的 TC-CSS-26 / TC-CSS-27
7. 真实场景测试：更新并执行 `human_tests/cli-log-output-default.md` 中的 TC-LOD-07，验证默认 `info` 日志不再反复输出常态连接关闭、进程归因 miss、WebSocket 正常关闭和 SSE ping
8. 执行与启动链路相关的 E2E / 校验命令，确认无回归

## 校验要求（含 rust-project-validate）

- 先执行本次改动涉及的启动链路验证或 E2E
- 再执行 `rust-project-validate` 要求的 fmt / clippy / test / build

## 文档更新要求

- 本次改动为 CLI 启动性能与可观测性优化，不涉及 README 或对外配置说明更新
