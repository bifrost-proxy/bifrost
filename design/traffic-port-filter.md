# Traffic Port Filter

## 功能模块详细描述

Traffic 页面主筛选器需要支持按代理入口端口过滤记录。端口字段对应 traffic summary/detail 中的 `listener_port`，用于区分主代理端口和临时代理端口产生的流量。

## 实现逻辑

- WebUI `FilterBar` 新增 `Port` 字段，内部字段名为 `listener_port`。
- 选择 `Port` 时默认使用 `Equals` 操作符，并展示端口输入框。
- 前端本地过滤将 `record.listener_port` 转成字符串参与 `contains/equals/regex/not_contains/is_empty/is_not_empty` 匹配。
- 后端 `/traffic` 查询支持 `listener_port=<port>`，用于刷新、API 验证和服务端查询路径。
- Fuzzy Search 的 `filters.conditions` 支持 `listener_port`，`equals` 会下推到数据库查询，最终 compact 过滤仍以 `lp` 字段校验。
- CLI `traffic list` / `traffic search` / 顶层 `search` / `remote traffic list` / `remote traffic search` 新增 `--listener-port <PORT>`，并提供 `--proxy-port` 作为同义别名。`traffic list/get --port` 已用于选择 Admin API 端口，因此端口筛选使用更明确的 `--listener-port`，避免语义冲突。

## 依赖项

- `web/src/components/FilterBar/index.tsx`
- `web/src/stores/useTrafficStore.ts`
- `web/src/types/index.ts`
- `crates/bifrost-admin/src/traffic_db/query.rs`
- `crates/bifrost-admin/src/handlers/traffic.rs`
- `crates/bifrost-admin/src/search/engine.rs`
- `crates/bifrost-cli/src/cli.rs`
- `crates/bifrost-cli/src/commands/traffic.rs`
- `crates/bifrost-cli/src/commands/search.rs`
- `crates/bifrost-cli/src/commands/remote.rs`
- `crates/bifrost-command/src/lib.rs`
- `web/tests/ui/traffic.spec.ts`

## 测试方案

- 单元测试：`build_where_clause_supports_listener_port_filter` 验证 `listener_port` 被编译为 SQL 条件。
- E2E 测试：`主筛选器支持按代理端口过滤 Traffic` 覆盖：
  - `/traffic?listener_port=...` API 查询只返回对应端口流量。
  - Fuzzy Search condition `listener_port equals ...` 只返回对应端口流量。
  - Traffic 页面主筛选器下拉出现 `Port`，选择后默认 `Equals`，输入端口后列表只展示匹配记录。
- Shell E2E：`test_temporary_port_bindings.sh` 覆盖 `bifrost traffic list --listener-port` 和 `bifrost traffic search --listener-port` 只返回目标代理端口记录。
- CLI 单元/解析测试：覆盖 `traffic list/search` help 展示 `--listener-port` / `--proxy-port`，以及搜索请求体中的 `listener_port equals <PORT>` 条件。
- 真实场景测试：在 `human_tests/webui-traffic.md` 增加端口筛选用例，并按用例执行。

## 校验要求

- 执行相关 Playwright UI E2E。
- 执行 `cargo test -p bifrost-admin traffic_db::query::tests::build_where_clause_supports_listener_port_filter`。
- 执行 `cargo test -p bifrost-cli`.
- 执行 `e2e-tests/tests/test_temporary_port_bindings.sh`。
- 执行 `pnpm --dir web exec tsc -b --pretty false`。
- 执行 `cargo fmt --all -- --check`。
- 执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。

## 文档更新要求

- 本次改动属于 WebUI Traffic 筛选能力补充，不涉及公开 README 协议/命令变更。
- 必须同步更新 `human_tests/webui-traffic.md` 与 `human_tests/readme.md`。
