# Search JSONPath / Header / 时间窗 / NDJSON

针对 `bifrost search` 与 `bifrost remote traffic search` 新增的 wave-1 能力，覆盖请求/响应体 JSONPath 字段过滤、HTTP header 等值过滤、时间窗（since/until/latest）剪枝，以及结构化 NDJSON 输出。

## 范围
- crates/bifrost-admin/src/search/{types.rs,engine.rs,json_path.rs,mod.rs}
- crates/bifrost-admin/src/{query_service.rs,handlers/search.rs}
- crates/bifrost-command/src/lib.rs (`SearchArgs.time_range` / `TimeRange`)
- crates/bifrost-cli/src/cli.rs / cli/remote.rs / commands/{search.rs,remote.rs,traffic.rs} / main.rs

## 测试用例

| 序号 | 场景 | 预期 |
|------|------|------|
| 1 | `bifrost search --req-json $.user.id=42`：单文件 JSONPath 字段过滤 | 仅返回 request body JSON 中 `$.user.id == 42` 的记录；非 JSON / 路径不存在记录被剪除；命令 exit code 0 |
| 2 | `bifrost search --res-json $.errno --res-json-gt 0`：响应体数值比较 | 仅返回 `res.body.$.errno > 0` 的记录；`searched_range.scanned_count` ≥ 命中数；非数字 errno 不被误判 |
| 3 | `bifrost search --req-header-eq X-Trace-Id=abc123`：header 等值过滤（大小写无关） | 命中 `x-trace-id: abc123` 与 `X-Trace-Id: abc123`；缺失该 header 的记录被剪除 |
| 4 | `bifrost search --since 2026-06-17T00:00:00Z --until 2026-06-17T23:59:59Z`：RFC3339 时间窗 | `searched_range.oldest_ts_ms/newest_ts_ms` 落在窗口内；窗口外记录在 SQL `timestamp` 条件层被剪掉，未进入 matcher，也不消耗 `--max-scan` |
| 5 | `bifrost search --latest 30m`：相对时间窗 | 等价 `--since now-30m`；CLI duration 解析单元测试覆盖 `30s/5m/2h/1d/1w`、无单位默认秒、非法单位报错 |
| 6 | `bifrost search --format ndjson token`：行式 JSON 输出 | 每行一个独立 JSON 对象，含 `type: "result"|"progress"|"done"|"error"`；适合 `| jq`；`done` 行包含 `total_matched/total_searched/has_more`；`bifrost remote traffic search` 同样支持 |

## 联调说明
- `bifrost remote traffic search --req-json $.x=1 --since 1h --format ndjson`：CLI flag 通过 `command_search_args` 透传到远端 admin `SearchRequest.time_range`；远端 `SearchEngine` 命中 `time_range` 预剪枝路径。
- body 加载经 `body_cache`（per-search HashMap<key, BodyCacheEntry>），同一条记录的 req.body / res.body 多 JSONPath 条件仅 load + parse 一次。
- 错误码兜底：JSONPath 语法错误 / time_range RFC3339 解析失败 → CLI 直接报错并非 0 退出，不静默忽略。

## 边界
- JSONPath 仅支持点路径与数组索引（`$.a.b[0]`），不支持 `..` 递归下降、`?(filter)` 表达式。
- header 过滤仅支持等值（contains/regex 留待后续 wave）。
- 时间窗使用 i64 unix-ms；`until - since < 0` 视为空集，admin 返回 0 命中且 `searched_range.scanned_count == 0`。

## 本次回归执行（2026-06-18）

- TC-SJ-04 执行 `cargo test -p bifrost-admin time_range_excludes_records_outside_window -- --nocapture` 通过，验证超窗记录经 SQL `timestamp <= ?` 条件剪掉后 `total_matched == 0` 且 `searched_range.scanned_count == 0`。
- 执行 `BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 bash e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh` 通过，其中 `stale --until window is filtered before max-scan` 断言真实 CLI JSON 输出 `total_matched == 0` 且 `searched_range.scanned_count == 0`。
