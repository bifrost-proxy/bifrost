# `bifrost status --format json` (Schema v1)

## 目标

为运维 / Agent / 上层 IDE 集成提供 `bifrost status` 的机器可读输出。新增 `--format` 标志，文本路径保持完全向后兼容。

## CLI

- 现有命令：`bifrost status [-- tui]`
- 新增标志：`--format <text|json|json-pretty>`，默认 `text`
- 兼容性：不传 `--format` 时与现行 `bifrost status` 输出逐字节一致；`--tui` 与 `--format` 互斥的语义由旧逻辑保留——`tui=true` 时优先走 TUI。

## 数据契约（schema_version=1）

```json
{
  "schema_version": 1,
  "version": "0.0.x",
  "running": true,
  "pid": 12345,
  "uptime_sec": 3600,
  "listener": { "host": "127.0.0.1", "port": 9900, "socks5_port": 9901 },
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
  "active_rules": [{ "group": "default", "rule_count": 12, "enabled": true }],
  "data_dir": "/Users/.../bifrost",
  "ports": [{ "port": 9901, "host": "127.0.0.1", "status": "running", "name": "mobile", "binding": "127.0.0.1:9901" }],
  "errors": []
}
```

### 字段说明

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `schema_version` | u32 | 固定 `1`；后续不兼容变更必须递增。 |
| `version` | string | 来自 `CARGO_PKG_VERSION`。 |
| `running` | bool | `runtime.json` 存在且 PID 进程存活。 |
| `pid` | u32 \| null | 仅 running 时填充。 |
| `uptime_sec` | u64 \| null | `now_ms - started_at_ms` 除以 1000；缺失时 `null`。 |
| `listener` | object \| null | 仅 running 时填充；包含 host/port 与可选 socks5_port。 |
| `system_proxy` | object | 始终存在；`supported=false` 时 host/port/bypass 为 null。 |
| `tls` | object \| null | admin API `/_bifrost/api/config/tls` 调通时填充；否则 `null` 并在 `errors` 中记录 `tls_config`。 |
| `active_rules` | array \| null | admin API `/_bifrost/api/rules` 的精简映射；失败时 `null` 并 `errors:[{source:"rules"}]`。 |
| `data_dir` | string \| null | `get_bifrost_dir()` 的 display 路径。 |
| `ports` | array \| null | 来自 `/_bifrost/api/ports` 的临时端口绑定；失败时 `null` 并 `errors:[{source:"ports"}]`。 |
| `errors` | array | 当 admin 子调用失败时累计 `{source, message}`；成功时为空数组。 |

### 进程未运行

- `running=false, pid=null, listener=null, tls=null, active_rules=null, ports=null`
- `system_proxy` 仍按平台真实状态返回（不会被压成 null）。
- `errors` 为空（未尝试调用 admin API，不算失败）。

### 部分失败

- 若某个 admin API 调用失败（如 TLS API timeout、rules 500），对应字段为 `null`，并在 `errors` 中追加 `{source: "tls_config"|"rules"|"ports"|"active_summary", message: "..."}`。

## 实现拆解

1. `cli.rs`：新增 `StatusFormat { Text, Json, JsonPretty }` 枚举，附在 `Status { tui, format }`。
2. `main.rs`：解构出 `format` 透传给 `run_status(format)`；同时把现有 `Commands::Status { tui: false }` 调用点改成带 `format: StatusFormat::Text`。
3. `commands/status.rs`：
   - 抽出 `gather_status()`：一次性收集 runtime/tls/rules/ports/active_summary，每个字段是 `Result<T, String>` 或 `Option<...>`。
   - 文本渲染：`render_status_text(g)` 复用现有 `format_*` 函数 + `println!`，与 main 分支输出 100% 一致。
   - JSON 渲染：`build_status_json(g)` 构造 `StatusJson`，调用 `serde_json::to_string[_pretty]` 序列化；序列化失败回退打印一个最小 schema 错误对象（防止 panic）。

## 兼容性

- 文本路径未改变任何 `println!` 顺序或字段，已通过现有 6 个 `format_*` 单测保护。
- 进程未运行场景沿用旧的 `Status: Stopped` 文本；JSON 形态下 `errors=[]`。
- 与 `--tui` 互斥：`if tui { run_status_tui() } else { run_status(format) }`，保持旧入口。

## 测试

- 单元测试（`crates/bifrost-cli/src/commands/status.rs::tests`）：
  - `build_status_json_running_serializes_schema_v1` —— running + 所有字段齐全的 JSON 形态。
  - `build_status_json_stopped_omits_runtime_and_keeps_keys` —— stopped 时关键字段 null，`errors=[]`。
  - `build_status_json_partial_failures_recorded_in_errors` —— admin API 全部失败时 `errors` 累计来源。
- `human_tests/status-json.md`：3 个真实手测用例（running / stopped / API 部分失败）。

## 已知限制 / TODO

- `active_summary` 在 v1 schema 中没有独立 JSON 字段，仅在失败时被追加到 `errors`，未来若需暴露合并规则可在 v2 扩展。
- 当前 `listener.host` 为 `RuntimeInfo.host`（可能是 `0.0.0.0`），消费方需要自行处理 LAN/loopback 语义；后续可扩展 `listener.advertised_addresses`。
