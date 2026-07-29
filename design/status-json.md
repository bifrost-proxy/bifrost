# `bifrost status --format json` Schema v1 设计方案

## 背景

`bifrost status` 面向三类使用者：

1. 交互终端用户 — 需要人类可读的分段文本。
2. Devbox/CI/自动化脚本 — 需要稳定、机器可读的结构化输出。
3. 上层 IDE / Agent（例如 Cursor、Codex、Mira）— 需要一次性获取运行时、监听、系统代理、TLS、规则、临时端口等状态用于诊断决策。

修复前只有交互终端一档；脚本要靠 `grep`/`awk` 撕文本，一旦文本换行、颜色、字段顺序变动就会破。为此新增 `--format` 标志，输出稳定的 JSON schema（schema_version=1），文本路径完全向后兼容。

## 用户目标验证清单

### 必须实现

- 新增 `bifrost status --format <text|json|json-pretty>`，默认 `text`。
- 文本路径输出与旧 `bifrost status` 逐字节一致（含既有 6 个 `format_*` 单测覆盖的字段顺序）。
- `--format json` 输出单行 JSON；`--format json-pretty` 输出 2 空格缩进 JSON。
- JSON 顶层字段包含：`schema_version`, `version`, `running`, `runtime_source`, `pid`, `uptime_sec`, `listener`, `system_proxy`, `tls`, `active_rules`, `data_dir`, `ports`, `errors`。
- 进程未运行时仍能返回 `running=false` 的完整 JSON，不 panic 不阻塞。
- Admin API 子调用失败时对应字段为 `null`，`errors` 记录来源。

### 必须不破坏

- 未传 `--format` 的旧脚本行为不变，所有 `println!` 顺序与既有单测锁定。
- `--tui` 分支保持独立入口 `run_status_tui()`；`tui=true` 时优先走 TUI。
- 与 `bifrost status --help` 现有排版兼容。
- `bifrost` 二进制返回码语义不变。

### 必须真实验证

- CLI 单元测试覆盖 running/stopped/partial-failure 三态 JSON。
- `human_tests/status-json.md` 有 3 个真实手测用例，都以临时数据目录 + 非默认端口执行。
- schema 由 `serde_json` 序列化生成，序列化失败时降级为最小 `{"schema_version":1,"error":"serialize_failed:..."}` 而不是 panic。

## 产品语义

### `--format` 与 `--tui` 的关系

`bifrost status` 命令签名（`crates/bifrost-cli/src/cli.rs` line 272）：

```rust
Status {
    #[arg(long)]
    tui: bool,
    #[arg(long, default_value_t = StatusFormat::Text)]
    format: StatusFormat,
}
```

- `tui=false`：走 `run_status(format)` 一体化路径。
- `tui=true`：优先走 `run_status_tui()`，`format` 参数忽略。
- 该互斥语义在 `main.rs` 的调用点显式实现，避免用户同时传 `--tui --format json` 时误分流。

### schema_version 兼容承诺

- schema_version=1 是本次锁定的契约。字段的“存在与类型”不再向下兼容改。
- 新增字段必须保持 optional / 默认值，保证旧消费方 `jq` 表达式不报错。
- 破坏性变更必须递增 `schema_version` 并同步 human_tests。

## 数据契约（schema_version=1）

```json
{
  "schema_version": 1,
  "version": "0.0.x",
  "running": true,
  "runtime_source": "runtime_file",
  "pid": 12345,
  "uptime_sec": 3600,
  "listener": {
    "host": "127.0.0.1",
    "port": 9900,
    "socks5_port": 9901
  },
  "system_proxy": {
    "supported": true,
    "enabled": true,
    "host": "127.0.0.1",
    "port": 9900,
    "bypass": "",
    "error": null
  },
  "tls": {
    "enabled": true,
    "include_domains": [], "exclude_domains": [],
    "include_apps": [], "exclude_apps": [],
    "include_ips": [], "exclude_ips": [],
    "unsafe_ssl": false,
    "disconnect_on_config_change": false
  },
  "active_rules": [
    { "group": "default", "rule_count": 12, "enabled": true }
  ],
  "data_dir": "/Users/.../bifrost",
  "ports": [
    { "port": 9901, "host": "127.0.0.1", "status": "running", "name": "mobile", "binding": "127.0.0.1:9901" }
  ],
  "errors": []
}
```

### 字段说明

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `schema_version` | u32 | 固定 `1`；不兼容变更递增。 |
| `version` | string | `env!("CARGO_PKG_VERSION")`。 |
| `running` | bool | runtime marker 的 PID 存活，或目标端口通过 Bifrost Admin API fallback 身份校验。 |
| `runtime_source` | string | `runtime_file`、`admin_api`、`stale_runtime_file` 或 `none`；用于区分状态证据来源。该字段是 schema v1 的向后兼容增量。 |
| `pid` | u32 \| null | 仅 running 时填充。 |
| `uptime_sec` | u64 \| null | `(now_ms - started_at_ms) / 1000`；缺失时 null。 |
| `listener` | object \| null | 仅 running 时填充；含 host/port 与可选 `socks5_port`。 |
| `system_proxy` | object | 始终存在；`supported=false` 时 host/port/bypass 为 null。 |
| `tls` | object \| null | admin API `/_bifrost/api/config/tls` 调通时填充；否则 null 并 `errors:[{source:"tls_config"}]`。 |
| `active_rules` | array \| null | admin API `/_bifrost/api/rules` 精简映射；失败时 null 并 `errors:[{source:"rules"}]`。 |
| `data_dir` | string \| null | `get_bifrost_dir()` 显示路径。 |
| `ports` | array \| null | admin API `/_bifrost/api/ports`；失败时 null 并 `errors:[{source:"ports"}]`。 |
| `errors` | array | 每项 `{source, message}`；成功时为空数组。 |

### 进程未运行

- `running=false, runtime_source="none", pid=null, listener=null, tls=null, active_rules=null, ports=null`；若存在陈旧 marker，则 `runtime_source="stale_runtime_file"`。
- `system_proxy` 仍按平台真实状态返回（不会被压成 null），因为它由 OS 拿而非 admin API。
- `errors` 为空 —— status 可能执行只读的 runtime identity fallback probe，但 stopped
  状态不会继续调用 TLS/rules/ports 等字段 API，identity 未命中也不是字段错误。

### 部分失败

- 单个 admin 子调用失败（TLS API timeout、rules 500、ports 404）时，对应字段为 null，`errors` 追加 `{source, message}`，`source` 取值：`tls_config` / `rules` / `ports` / `active_summary`。

## 技术细节

### 关键实现点（`crates/bifrost-cli/src/commands/status.rs`）

- `run_status(format)` 入口（line 516）
  - 一次性收集 `GatheredStatus`（line 539 `gather_status()`）。
  - 按 format 分派到 `render_status_text` / `render_status_json`。
- `build_status_json(g)`（line 785）
  - 返回 `StatusJson`（`schema_version` 固定 `1`，line 904）。
- 序列化失败兜底（line 692）：
  ```rust
  let value = build_status_json(g);
  match serde_json::to_string(&value) {
      Ok(s) => println!("{}", s),
      Err(e) => println!("{{\"schema_version\":1,\"error\":\"serialize_failed:{}\"}}", e),
  }
  ```

### CLI 定义（`crates/bifrost-cli/src/cli.rs`）

```rust
#[derive(ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum StatusFormat {
    Text,
    Json,
    #[value(name = "json-pretty")]
    JsonPretty,
}
```

### 文本路径不变

`render_status_text` 复用现有 `format_active_summary_status_block`、`format_service_overview_lines` 等函数，被 line 921 的 `use super::{...}` 与 `tests` 模块引用；6 个 `format_*` 单测保证字段顺序稳定。

## CLI / Web / Admin API 边界

- CLI：`bifrost status [--format <text|json|json-pretty>] [--tui]`。
- Web UI：不消费 JSON schema；Web 状态由自身 admin API 拉取。
- Admin API：无新增；本方案是纯 CLI 端 aggregation，从 admin API 读到的字段做二次编排。

## Sync 边界

- Sync 不涉及；schema 输出的所有字段都来自本机运行时或本机 admin API。

## Phase 1 - 4

### Phase 1：CLI 骨架

- 在 `cli.rs` 引入 `StatusFormat` 与 `Status { tui, format }`。
- 在 `main.rs` 拆解 `format` 并透传。

### Phase 2：JSON 构造

- 在 `commands/status.rs` 抽出 `gather_status()`，把 runtime / tls / rules / ports / active_summary 集中收集。
- 引入 `StatusJson`、`build_status_json`。

### Phase 3：错误累计

- admin 子调用失败时 append `errors:[{source, message}]`，对应字段设 null。
- 序列化失败降级为最小 `{"schema_version":1,"error":"serialize_failed:..."}`。

### Phase 4：测试与文档

- 新增 3 个单元测试。
- 新增 `human_tests/status-json.md` 3 个手测用例。
- 更新 `bifrost status --help`；更新文档链接。

## 测试方案

### 单元测试

`crates/bifrost-cli/src/commands/status.rs::tests`：

- `build_status_json_running_serializes_schema_v1`（line 1208） — running + 所有字段齐全。
- `build_status_json_stopped_omits_runtime_and_keeps_keys`（line 1294） — stopped 时关键字段 null、`errors=[]`。
- `build_status_json_partial_failures_recorded_in_errors`（line 1333） — 全部 admin API 失败时 `errors` 累计来源。

### human_tests

`human_tests/status-json.md`（已落地）：

- 用例 1：running 场景。启动 `BIFROST_DATA_DIR=./.bifrost-test-status-json ... target/debug/bifrost start -p 28800 --no-system-proxy`，执行 `bifrost status --format json | jq .` 与 `--format json-pretty`。校验 `data_dir` 指向临时目录、`listener.port=28800`。
- 用例 2：stopped 场景。启动后 `bifrost stop`，`--format json` 应返回 `running:false`，关键字段 null，`errors=[]`。
- 用例 3：admin API 部分失败场景。用 iptables/防火墙或强制 kill admin listener 制造 `tls_config`/`rules`/`ports` 500/超时，验证 `errors` 累积。

护栏（必须传）：

- `BIFROST_DATA_DIR=./.bifrost-test-status-json`
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
- `BIFROST_DISABLE_TRAY=1`
- `--no-system-proxy`

### 收尾

- `cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`；`cargo test -p bifrost-cli status`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：`--format` 三档、schema 稳定、文本兼容、失败可诊断。
- 复核 diff：`StatusFormat`、`gather_status`、`build_status_json`、`errors` 收集、序列化兜底。
- 复测：`build_status_json_*` 单测；跑 human_tests 3 个用例。

### 第 2 轮

- 复核第 1 轮修复；`git status --short` 干净。
- 复查 human_tests 索引；确认 `--help` 呈现 `--format`。
- 复测：`cargo test -p bifrost-cli status`；重跑 human_tests。

## 风险与决策

- `listener.host` 目前为 `RuntimeInfo.host`（可能是 `0.0.0.0`），消费方需要自行处理 LAN/loopback。后续可扩展 `listener.advertised_addresses`（v2 才引入）。
- `active_summary` 未在 v1 独立暴露，只在失败时进入 `errors`。若产品明确需要暴露合并规则视图，走 v2 schema 或独立命令。
- serde 序列化失败极小概率发生，但保持“最小 JSON 兜底”避免脚本消费者拿到空/半 JSON。
- `--tui` 与 `--format` 互斥语义应保留；若未来允许在 TUI 内嵌 JSON 面板，需要单独设计新 subcommand。
- schema_version 递增策略：不兼容变更强制走 v2；新增字段（optional 且 default null）不递增。
