# Traffic list / search 过滤与排序

## 功能与实现

CLI `bifrost search` 与 `bifrost traffic search` 共用搜索实现；`traffic list` 使用 SQL 过滤，搜索使用 SQL 候选集与 body/header matcher。远端 list/search 经 command service 进入相同后端。

### 排序与分页

- CLI list 与 search 默认按请求 `timestamp DESC, sequence DESC`，最新匹配优先；时间戳相同时按序号消除排序歧义。
- list 显式 `--direction forward` 按时间、序号升序。默认 backward 向更早记录翻页。
- `QueryParams.order_by_time` 只由 CLI list、command list 和 search 启用；实时增量更新仍沿用序号顺序，避免把请求时间和更新序号混用。
- 时间排序的 cursor 仍为记录序号，SQL 根据该记录定位 `(timestamp, sequence)` 边界。数据保留清理或 clear 删除游标记录后，应重新从首屏查询。
- SQL 多取一个候选后截断，用额外候选判断 `has_more`，整页恰好耗尽时不再返回虚假的下一页。
- search 在每条命中后检查结果上限，不允许流式模式越过上限；在批次内截断或达到扫描上限时保留 cursor 与剩余候选提示。
- `has_more` 表示仍有候选可扫描，不保证剩余候选一定匹配关键词。

### CLI 过滤语义

- list 的 host/url/path/client-app/client-ip/content-type 子串过滤按字面解释 `%`、`_`、反斜杠，绑定 SQL 参数并使用 `LIKE ... ESCAPE`；不接受 SQL 通配符语义。
- 同类高级过滤可重复，多个条件取 AND；JSONPath 数组通配命中的多个节点取任一匹配。
- `--req-json PATH=VALUE`、`--res-json PATH=VALUE` 支持 `$` 根、`.member`、`[0]`、`[*]`；允许省略 `$.` 前缀。值中的 `=` 保留。
- 不支持递归下降、切片、过滤表达式。CLI 使用后端同源解析器校验，无效路径在发请求前报错；API matcher 对无效路径返回不匹配。
- JSON null 的等值文本为 `null`，区别于空字符串。等值比较沿用大小写不敏感的文本语义，不引入 JSON 类型严格等值。
- `--req-header-eq NAME=VALUE` / `--res-header-eq NAME=VALUE` 是等值过滤；`--req-header` / `--res-header` 是无参数的关键词搜索范围开关。
- 布尔过滤 true/false 均生效，包括 WebSocket、SSE、H3、tunnel。
- `--since` / `--until` 接受 RFC3339、整数 epoch 毫秒或相对时长；`--latest 5m` 等价最近 5 分钟，不是只取一条。时长支持 ms/s/m/h/d/w、小数，无单位按秒。
- 非法路径、缺失等号、空 header 名、无效时间和 include token 均拒绝，不静默扩大查询范围。

### 上限与输出

- CLI `--limit` 默认 50；显式 `--max-results` 覆盖它。`--max-scan` 限制扫描候选数量，SQL 时间窗剪枝不消耗扫描预算。
- `searched_range` 只描述实际扫描范围，不代表数据库全量时间范围。
- 非终端输入或非 table 输出在没有关键词时执行过滤查询，不自动进入 TUI；显式 `--interactive` 仍支持交互模式。
- `--no-color` 的空结果也不输出 ANSI 转义。
- search 支持 table、compact、json、json-pretty、ndjson。include 与 batch get 见 [search-include-body.md](search-include-body.md)。

## 依赖与边界

- SQLite 的复合比较完成游标定位，既有 timestamp 索引用于时间排序，不修改 schema、不删除旧库。
- JSONPath 使用 `serde_json` 和项目自有解析器，不引入外部解释器。
- body cache 在单次搜索内复用，超大正文受现有解压预算限制。
- 不修改 WebUI、Sync、正式服务或系统代理。远端参数解析和 command 映射纳入单测；真实 relay 连通性另由远端专项套件验证。

## 本次回归发现与修复

| 问题 | 修复 | 防回归证据 |
|---|---|---|
| path 的 `_` / `%` 意外成为 SQL 通配符 | 所有 SQL 子串过滤转义 | 字面路径与组合过滤精确集合断言 |
| false 协议标记被忽略 | 统一 true/false flag 条件 | SQL 正反分支、真实 SSE 排除 |
| streaming 超出结果上限，limit 被默认 max-results 遮蔽 | 每条命中检查上限，明确覆盖关系 | limit/max-results/max-scan 矩阵 |
| 批内截断丢失 has_more、整页末尾误报下一页 | 保留未扫描候选，多取一个判断末页 | 续查、精确末页、扫描上限测试 |
| 序号倒序不等于请求时间倒序 | 按时间与序号稳定排序 | 乱序写入、相同时间戳、双向游标测试 |
| 根数组/根标量 JSONPath 损坏，null 被当空串 | 保留根路径，区分 null | 根路径、通配、压缩 JSON 真实查询 |
| 非法过滤条件静默忽略 | clap 同源校验 | 本地两个入口与远端 parser 测试 |
| 无关键词 JSON 查询误进 TUI | 按终端和输出格式决定默认交互 | 无关键词过滤、TTY 专项测试 |
| 显式 batch json-pretty 输出 NDJSON | 区分格式缺省和显式选择 | batch 三格式精确解析 |
| no-color 空结果仍含 ANSI | 移除硬编码转义 | 空结果 table/compact 断言 |

## 验证方案

本次属于 CLI/Admin Rust 行为变更，执行以下适用验证：

- 单元：traffic_db query/store、search engine/json_path、CLI search 和三入口参数解析；覆盖时间乱序、相同时间、末页、非法输入及根路径。
- 真实 CLI：`python3 e2e-tests/tests/test_traffic_search_matrix.py`，独立数据目录、动态端口、本地 mock 请求，逐项断言实际输出；`crates/bifrost-cli/tests/traffic_search_cli.rs` 将同一矩阵接入 Cargo 集成测试和覆盖率。
- 交互与原有行为：`bash e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh`，覆盖 aliases、TTY、clear、replay 等旧路径；该套件末尾执行新增矩阵。
- human_tests：执行 [cli-traffic-search](../human_tests/cli-traffic-search.md)、[search-jsonpath](../human_tests/search-jsonpath.md)、[search-include-body](../human_tests/search-include-body.md)。
- E2E 后执行 rust-project-validate；fmt、workspace clippy、workspace all-features tests、`make coverage-changed`，远端 CI 验证 coverage 门禁。
- 不改前端，视觉与主题验证不适用。

## Review / Fix / Test

1. 第一轮核对用户目标与 diff，审查排序游标、输入校验、截断和本地/远端参数一致性；发现问题先补断言再修复，运行相关单元与 CLI 矩阵。
2. 第二轮基于修复后 diff 检查文档、help、末页和组合过滤覆盖，再跑完整 CLI 套件与 Rust 门禁；若发现新问题继续追加轮次。
3. 本地验证后提交任务分支、创建 draft PR，并跟进所有已触发 CI；只按真实执行结果汇报，不把未测试的 relay 或协议捕获称为通过。

## 文档同步

同步 README 搜索说明、CLI help、search include 设计和上述 human_tests 索引；示例仅使用仓库相对路径、动态测试端口与隔离数据。
