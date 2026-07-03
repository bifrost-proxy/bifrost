# CLI 默认文件日志

## 背景

前台 `bifrost start` 历史上默认把 tracing 标准日志同时输出到 Console Terminal 和文件。Console Terminal 会持续刷新滚动，一旦后续在同一 Terminal 区域承载其他能力（`bifrost status -t` TUI、`bifrost skill` 交互输出、Codex/Agent 对接进度、JSONL/stdio IPC 协议帧等），标准日志就会污染终端协议、遮盖用户交互，甚至撑爆滚动缓冲让关键错误被冲走。

同时数据目录里的历史日志文件在长期运行的机器上会不断累积。当前只有一份粗粒度按日期滚动的 `bifrost.YYYY-MM-DD.log`，缺乏统一的保留期与容量上限；Tray、daemon、Desktop sidecar/bootstrap、guardian/restart/upgrade、audit 等分散产物由各自组件在各自路径下写入，从来没有集中清理入口。

本方案把 CLI 全局 `--log-output` 默认值改为 `file`，把 Console Terminal 让位给交互；同时把日志目录默认保留 7 天、总量上限 1GiB 落到统一清理逻辑上，覆盖当前所有已知 Bifrost 日志产物。

## 用户目标验证清单

### 必须实现

- 前台 `bifrost start`、无子命令的默认启动、以及所有普通 CLI 命令的默认 `--log-output` 都必须是 `file`，不再向 Console 打 tracing。
- 用户显式传 `--log-output console` 或 `--log-output console,file` 时，Console tracing 打开；文件日志始终保留，不因为 opt-in 到 console 而丢文件。
- daemon 模式（后台脱离终端）继续调用既有 `reinit_logging_for_daemon`，写入 `<data_dir>/logs/bifrost*.log`。
- macOS 系统级 LaunchDaemon `system-proxy cleanup-daemon` 隐藏命令保持默认 `console,file`，因为 launchd plist 已把 `StandardOutPath` / `StandardErrorPath` 指向 `/var/log/bifrost-system-proxy-cleanup.*`，Console 输出实际落到系统 stdio 文件。
- stdout 协议型隐藏 worker（如 voice/asr worker）强制文件日志，禁止 Console 参数把日志灌进 stdio 协议帧。
- 日志目录默认保留 7 天，总容量上限默认 1GiB；两条策略在同一个清理函数里生效，7 天窗口内如果总量超上限，也按修改时间从旧到新删除。
- 清理入口覆盖：CLI 主进程启动、daemon 启动、Tray 启动、Desktop sidecar/bootstrap 启动，四路都调用共享清理函数。
- 清理白名单覆盖当前所有已知产物：`bifrost.YYYY-MM-DD.log`、daemon err 轮转、Tray 日志、历史 `log-YYYY-MM-DD.log`、desktop sidecar/bootstrap、guardian/restart/upgrade 日志、`*-audit.json`。

### 必须不破坏

- 用户已有 `RUST_LOG` / `--log-level` 等日志级别控制继续生效。
- daemon 模式向文件写日志的既有行为、文件路径命名和轮转粒度不变。
- 未列入白名单的普通用户文件在日志目录里不被误删。
- LaunchDaemon plist stdio 重定向路径 `/var/log/bifrost-system-proxy-cleanup.*` 不被 CLI 侧规则挪走。
- CLI 全局参数 `--log-output` 的字符串枚举语义（`console` / `file` / `console,file`）在配置文件、env fallback 和帮助文档中保持向下兼容。
- 前台 `bifrost start` 的欢迎输出、DEFAULT BEHAVIOR 提示、错误报告仍然直接走 stdout/stderr，不受日志出口切换影响。

### 必须真实验证

- 真实执行 `bifrost start`（临时数据目录、非 9900 端口、`--no-system-proxy`），控制台没有 tracing 日志，`<data_dir>/logs/bifrost*.log` 非空。
- 真实执行 `bifrost --log-output console start` 或 `--log-output console,file`，控制台有 tracing 日志，文件日志也非空。
- macOS 上真实执行 `bifrost system-proxy cleanup-daemon`，LaunchDaemon stdio 路径 `/var/log/bifrost-system-proxy-cleanup.*` 收到 tracing 输出。
- 构造 7 天以外的旧日志和 >1GiB 的滚动日志，启动一次 bifrost 后清理逻辑生效，未知普通文件保留。

## 产品语义

### 默认关闭 Console tracing，把终端让给交互

`--log-output` 的语义：

- `file`（新默认）：只写文件日志。
- `console`：只写 Console tracing；仍然额外保留文件日志，用于事后诊断。
- `console,file`：显式两路都写；等价于 `console` 语义并保留文件层。

设计原则是「文件日志永远开」；Console 是可选叠加。这样切换默认值不会让任何历史场景「只有 Console 没有文件」，事后仍可从 `<data_dir>/logs/` 找回。

### 日志目录 = 单一清理域

`<data_dir>/logs/` 是 Bifrost 唯一的托管日志目录。所有已知产物统一清理，未知文件保留，避免误删用户放进去的东西。清理策略两条：

1. 时间维度：默认保留最近 7 天；`mtime < now - 7d` 且属于白名单的文件删除。
2. 容量维度：总量上限默认 1GiB；超上限时按 mtime 从旧到新继续删除白名单文件，直到回到上限内或者白名单文件全部清空。

两条策略不冲突：7 天保留是软下限（老的直接删），1GiB 上限是硬上限（不管几天都要清）。

### 隐藏命令与协议 worker 的例外

- `system-proxy cleanup-daemon`：LaunchDaemon 场景需要 Console 输出被 launchd 抓到重定向文件，所以默认保留 `console,file`。
- stdout 协议 worker（voice/asr JSONL）：强制文件日志，即使显式传 `console` 也拒绝把 tracing 打到 stdout，避免协议帧被日志破坏。

## 技术细节

### 常量

```rust
// crates/bifrost-core/src/logging.rs
pub const DEFAULT_LOG_RETENTION_DAYS: u64 = 7;
pub const DEFAULT_LOG_DIR_MAX_BYTES: u64 = 1 * 1024 * 1024 * 1024; // 1 GiB

pub enum LogOutput {
    File,
    Console,
    ConsoleAndFile,
}

impl Default for LogOutput {
    fn default() -> Self { LogOutput::File }
}
```

### CLI 参数

```rust
// crates/bifrost-cli/src/cli.rs
#[arg(long = "log-output", global = true, default_value = "file", value_parser = parse_log_output)]
pub log_output: LogOutput,
```

help 文案：`Where tracing logs go: file (default) | console | console,file. File output is always retained.`

### 计算入口

```rust
// crates/bifrost-cli/src/main.rs
fn resolve_log_output(cli: &Cli, command: &Command) -> LogOutput {
    if command.is_launchd_cleanup_daemon() {
        return LogOutput::ConsoleAndFile;
    }
    if command.is_stdout_protocol_worker() {
        return LogOutput::File; // 强制文件日志
    }
    cli.log_output
}
```

### 清理函数

```rust
// crates/bifrost-core/src/logging.rs
pub fn cleanup_bifrost_log_dir(
    log_dir: &Path,
    retention: Duration,
    max_bytes: u64,
) -> Result<CleanupReport>;

fn is_bifrost_managed_log(entry: &DirEntry) -> bool; // 白名单
```

白名单模式（glob）：

- `bifrost.*.log`、`bifrost.*.log.*`
- `bifrostd*.err*`、`bifrostd*.out*`
- `bifrost-tray.*.log`
- `log-*.log`（历史）
- `bifrost-desktop-sidecar.*.log`、`bifrost-desktop-bootstrap.*.log`
- `bifrost-guardian.*.log`、`bifrost-restart.*.log`、`bifrost-upgrade.*.log`
- `*-audit.json`

### 相关文件

- `crates/bifrost-cli/src/cli.rs`：全局 `--log-output` 参数和 help。
- `crates/bifrost-cli/src/main.rs`：`resolve_log_output` 和 `LogConfig` 初始化。
- `crates/bifrost-core/src/logging.rs`：`LogConfig`、`LogOutput`、`cleanup_bifrost_log_dir`、白名单常量。
- `crates/bifrost-cli/src/commands/start.rs`：前台启动前调用一次清理。
- `crates/bifrost-cli/src/commands/tray/tray.rs`：Tray 启动路径复用清理。
- `crates/bifrost-cli/src/commands/system_proxy.rs`：`cleanup-daemon` 隐藏命令的默认 LogOutput 分支。
- `desktop/src-tauri/src/main.rs`：Desktop bootstrap/sidecar 打开日志前调用清理。

## CLI + Web + Admin API

### CLI

- `bifrost start`：默认 file。前台 stdout 只保留欢迎横幅、DEFAULT BEHAVIOR 提示、fatal 错误。
- `bifrost --log-output console start`：Console tracing 打开，文件日志继续。
- `bifrost --log-output console,file start`：等价上一条。
- `bifrost start -d` / `bifrost daemon`：daemon 模式仍走文件日志，不受 CLI 参数影响 stdout。
- `bifrost system-proxy cleanup-daemon`：默认 `console,file`，用户不能通过 `--log-output file` 关掉 console（launchd 需要），CLI 检测到这条命令时忽略降级请求并记 warning。
- `bifrost voice`/asr worker 等 stdout 协议 worker：静默强制 file。

### Admin API

Log 配置本身不通过 Admin API 暴露（属于进程启动参数），Admin API 只在诊断路径显示当前进程实际的 LogOutput：

- `GET /api/system/logging` 返回 `{ "output": "file", "log_dir": "...", "retention_days": 7, "max_bytes": 1073741824, "last_cleanup_report": {...} }`。

Web UI：Settings → System → Logging tab 只读展示当前 LogOutput、日志目录、上一次清理报告；不提供在线切换（切换需要重启进程）。

## Sync 边界

- 不同步：`--log-output`、日志目录路径、保留策略常量都属于进程级 CLI 参数或平台差异（macOS `/var/log/...` vs Linux `~/.local/share/...`）。
- 日志文件本身不同步。

## 实现切分

### Phase 1：默认值切换

- 常量与枚举默认值改为 `File`。
- `cli.rs` 默认值和 help 更新。
- `main.rs` `resolve_log_output` 分支：LaunchDaemon cleanup、stdout worker、普通命令。
- 单元测试覆盖三条分支。

### Phase 2：统一清理函数

- 实现 `cleanup_bifrost_log_dir`：白名单 + 7 天 + 1GiB。
- 提供 `CleanupReport` 结构（删除数量、释放字节、跳过未知文件数）。
- 单元测试构造 tmpdir 覆盖所有白名单类别。

### Phase 3：接入所有启动路径

- CLI `commands/start.rs` 启动前调一次。
- daemon `reinit_logging_for_daemon` 后调一次。
- Tray 启动路径调一次。
- Desktop sidecar/bootstrap 打开自己的日志前调一次。
- 保证并发下同一次清理只跑一次（简单文件锁或 once-cell）。

### Phase 4：文档 + E2E + human_tests

- 更新 `docs/cli.md` 中 `--log-output` 说明。
- 更新 `human_tests/cli-log-output-default.md`。
- 更新 `human_tests/readme.md` 索引。
- 新增 E2E 脚本 `test_cli_start_log_output_default_file.sh`。

## 测试方案

### 单元测试（新增）

- `default_log_output_writes_file_only`
- `explicit_log_output_can_enable_console`
- `explicit_console_log_output_keeps_file_logging`
- `launchd_cleanup_daemon_keeps_console_logs_for_standard_paths`
- `voice_worker_forces_logs_away_from_stdout_protocol`
- `cleanup_bifrost_log_dir_removes_legacy_and_shared_dated_logs`
- `cleanup_bifrost_log_dir_removes_old_fixed_log_artifacts_by_mtime`
- `cleanup_bifrost_log_dir_enforces_total_size_by_removing_oldest_logs`
- `cleanup_bifrost_log_dir_preserves_unknown_user_files`

### E2E 测试

新增 `e2e-tests/tests/test_cli_start_log_output_default_file.sh`：

1. 用临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy` 启动前台 `bifrost start`；断言 stdout 无 tracing 行，`<data_dir>/logs/bifrost.*.log` 非空。
2. 同一脚本用 `--log-output console`：断言 stdout 有 tracing 行且文件日志仍非空。
3. macOS 分支：真实执行 `bifrost system-proxy cleanup-daemon`；断言 stderr/stdout 收到 tracing。
4. 构造 8 天前的 `bifrost.OLD.log` 和 >1GiB 白名单文件、以及一个 `user-notes.txt`；启动一次 bifrost；断言老文件被删、超量文件被删、`user-notes.txt` 保留。

### 真实场景测试（human_tests）

更新 `human_tests/cli-log-output-default.md`：

- TC-CLD-01：默认 file 输出，console 无 tracing。
- TC-CLD-02：显式 `console` opt-in，console 有 tracing，文件仍写。
- TC-CLD-03：daemon 模式文件日志路径正确。
- TC-CLD-04：LaunchDaemon cleanup 隐藏命令保留 console。
- TC-CLD-05：日志目录 7 天保留 + 1GiB 上限回归；未知文件保留。

### 覆盖率与项目校验

本地默认执行：

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-core --lib log_output`
- `cargo test -p bifrost-core --lib cleanup_bifrost_log_dir`
- `cargo test -p bifrost-cli --lib default_log_output`
- `SKIP_BUILD=true e2e-tests/tests/test_cli_start_log_output_default_file.sh`（有当前二进制时）
- `git diff --check`

远端 CI 负责：`cargo test --workspace --all-features`、clippy、`bifrost-cli` bin 级单测和完整构建。

本地 no-local-coverage 约定下不跑 `make coverage`；交付说明豁免依据。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：默认 file、显式 console opt-in、文件日志始终保留、LaunchDaemon 与 stdout worker 例外、7 天 + 1GiB。
- 复核 diff：`cli.rs`、`main.rs`、`logging.rs`、`start.rs`、`tray.rs`、`desktop` bootstrap、E2E 脚本、human_tests。
- 重点 review：`resolve_log_output` 是否漏了某个隐藏子命令；清理白名单是否有遗漏产物；stdout 协议 worker 是否被强制。
- 复测：focused 单测 + 新增 E2E。

### 第 2 轮

- 复查第 1 轮修复后的 diff、stdout/stderr/file 三路边界、human_tests 索引和验证命令。
- 复跑受影响测试，包括 macOS 分支的 LaunchDaemon cleanup。
- 检查未知文件保留策略在真实数据目录（可能残留旧版本命名）下不误删。

## 风险与决策

- 白名单遗漏：新组件加日志文件时必须扩展白名单，否则 1GiB 上限触发不了。缓解：白名单集中在 `logging.rs` 常量数组，新增组件必须同时改常量；`CleanupReport` 里返回「跳过的未知文件数」和示例路径，便于事后发现遗漏。
- 用户误放大文件：如果用户把非日志大文件放进日志目录，7 天/1GiB 策略也不会清（白名单保护）。这是有意行为，避免误删。文档明确「Bifrost 只管自己的产物」。
- Console 用户体验回归：老用户可能习惯 `bifrost start` 直接看日志。缓解：DEFAULT BEHAVIOR 帮助块和 `docs/cli.md` 显式说明「日志默认写入 `<data_dir>/logs/`；使用 `--log-output console` 恢复终端 tracing」。
- LaunchDaemon 例外泄漏：如果 `cleanup-daemon` 隐藏命令被普通用户在 shell 里直接跑，Console 会突然亮起来。这不算回归，属于 launchd 场景语义；但 CLI 输出的 warning 帮助用户理解为什么这条命令不受 `--log-output file` 影响。
- 并发清理：同一台机器多个 bifrost 进程同时启动可能重复清理。缓解：用 once_cell + 文件锁短暂互斥；失败仅记 debug，不阻塞启动。
