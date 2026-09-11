# Remote Search 独立限制参数设计

## 范围与当前契约

`bifrost remote traffic search` 与本地 `bifrost search` / `bifrost traffic search` 使用独立的返回上限和扫描预算。排序、过滤及分页的共享契约见 [search-jsonpath.md](search-jsonpath.md)。

- `--limit` 默认 `50`，决定未显式提供 `--max-results` 时的结果上限。
- `--max-results` 无独立默认值；显式提供时覆盖 `--limit`，不是两者取最小值。
- `--max-scan` 默认 `10000`，决定最多扫描多少条候选记录，与返回条数独立。
- 限制在执行端搜索引擎生效，不是 caller 收到全部结果后才截断。
- list/search 默认按请求 `timestamp DESC, sequence DESC`，最新请求优先，不按写入顺序。

```bash
# 最多返回 5 条，最多扫描 200 条候选
bifrost remote traffic search error --limit 5 --max-scan 200 --format json

# max-results 覆盖 limit，最多返回 3 条
bifrost remote traffic search error --limit 20 --max-results 3 --max-scan 500 --format json

# 无关键词过滤查询
bifrost remote traffic search --path '/v1/user_name' --latest 30m --limit 5 --format json
```

`--latest` 是时间窗口而不是条数。时间窗在 SQL 层裁剪候选，不浪费扫描预算。`has_more` 表示仍有未扫描候选，不保证这些候选会匹配；`searched_range` 仅统计实际扫描范围。remote search 不提供 CLI `--cursor` 参数，需要扩大预算或收窄查询窗口。

## 实现与依赖

### CLI 参数

`crates/bifrost-cli/src/cli/remote.rs` 的 `RemoteSearchArgs` 保存：

- `limit: usize`，默认 `50`；
- `max_results: Option<usize>`，不设置 clap 默认值；
- `max_scan: Option<usize>`，CLI 默认 `10000`。

`crates/bifrost-cli/src/commands/remote.rs` 的 `command_search_args` 将有效上限计算为 `max_results.unwrap_or(limit)`，写入共享 `bifrost-command::SearchArgs.max_results`；保留 `limit` 字段，同时透传 `max_scan`。因此 CLI 不再用默认 `max_results=100` 遮蔽用户的 `--limit`。caller summary 同样显示有效上限。

### 执行端

`crates/bifrost-admin/src/remote_invoke/executor.rs` 接收共享 `SearchArgs`，结果上限优先级为 `max_results > limit > 50`，同时透传 `max_scan`。command service 和 HTTP 搜索路径使用相同搜索引擎。

`crates/bifrost-admin/src/search/engine.rs` 每条命中后检查结果上限，避免流式批次突破上限。在批内达到结果或扫描上限时保留 continuation 信息；整页恰好耗尽候选时不误报下一页。数据源分页与时间/序号边界由 traffic DB query/store 实现。

上述默认值描述 CLI，不应推断成所有直接 API 调用的默认值。共享协议保留 `limit` / `max_results` / `max_scan`，不修改持久化配置、Sync 状态或数据库 schema。

## 能力与权限边界

- remote search 对齐核心关键词范围、路径、JSONPath、header 等值、时间窗与结果预算。
- Relay wrapper 暂不支持 search `--include` / `--max-body`，remote get 暂不支持批量 `--ids`。
- 需要上述能力时，可在已授权的 Admin Client 模式使用对应本机命令，或在单独取得 shell 授权后通过 `remote exec` 执行目标机 CLI。不得自动切换模式或扩大 grant。
- 不改 WebUI、正式代理进程、系统代理或远端授权。

## 验证方案

运行时变更应覆盖以下验证；仅同步本文等文档时，执行文档语义、示例、链接及两轮 review，不重复运行 Rust/E2E。

- CLI parser：无显式 max-results、显式覆盖、非法过滤条件、本地与远端参数一致性。
- caller 映射：`command_search_args` 与 `build_remote_command` 的有效上限、扫描预算及 filter-only 查询。
- 搜索引擎：流式每条命中截断、扫描上限、批内 continuation、精确末页、时间乱序及同时间排序。
- 真实 CLI：`e2e-tests/tests/test_traffic_search_matrix.py`；既有兼容路径由 `test_search_traffic_cli_isomorphic_e2e.sh` 验证。
- Relay 专项：`e2e-tests/tests/test_remote_invoke_e2e.sh` 的搜索参数透传；真实 relay 连通性不得用本地 parser 测试替代。
- Rust 生产代码修改时，先执行相关 E2E，再执行 rust-project-validate、fmt/clippy、workspace 测试、本地 changed-lines coverage 与远端 CI 门禁。

## Review / Fix / Test

1. 对照 CLI help、参数默认值、caller 序列化与执行端优先级，检查默认上限不再遮蔽 limit；修复后复测相关 parser / 映射 / 引擎用例。
2. 对照最新 diff 复查本地、Relay、Admin Client 边界，检查流式截断、分页和文档示例；有新缺陷则继续修复并追加复测。

## 文档同步

同步 `SKILL.md`、`skill_remote.md`、`docs/cli.md` 与 `docs-en/cli.md`。站点参考页由 `site/scripts/sync-docs.mjs` 从源文档生成，不手写第二套契约。
