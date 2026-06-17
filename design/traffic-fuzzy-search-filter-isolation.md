# Traffic Fuzzy Search Filter Isolation

## 功能模块详细描述

Traffic 页的 `Add Filter` 条件只用于当前列表过滤，不应影响 Fuzzy Search 的关键词搜索结果。（planned, not yet shipped as of 2026-06-17：当前 `web/src/components/SearchMode/index.tsx` 的 `buildFilters` 仍把 `filterConditions` 写入 `filters.conditions`。）

## 实现逻辑

- 保留 Fuzzy Search 对 toolbar filter 与左侧 panel filter 的支持。
- 构造搜索请求时，不再把 `FilterBar` 的 `filterConditions` 写入 `filters.conditions`。（planned, not yet shipped as of 2026-06-17）
- 这样 Fuzzy Search 只受关键词、scope 和显式筛选面板影响，不会被列表临时过滤条件误伤。（planned, not yet shipped as of 2026-06-17）

## 依赖项

- `web/src/components/SearchMode/index.tsx`
- `web/tests/ui/traffic.spec.ts`

## 测试方案（含 e2e）

- 新增 UI 回归用例：
  - 在 Traffic 页添加一个不会命中任何记录的 `Add Filter`
  - 切换到 Fuzzy Search 并搜索已存在请求
  - 断言搜索请求中的 `filters.conditions` 为空
  - 断言搜索结果仍然能返回目标请求

## 校验要求（含 rust-project-validate）

- 先执行与本改动相关的 UI E2E
- 再执行 `rust-project-validate` 要求的 fmt / clippy / test / build

## 文档更新要求

- 本次不涉及 README / API / 配置项变更，无需额外更新公开文档

## 2026-03-17 CLI Search 条件过滤补充

### 功能模块详细描述

- 为 `bifrost search` 与 `bifrost traffic search` 补充基础条件过滤参数：`--host`、`--path`。
- 修正现有 `--method` 仅出现在 CLI 但未实际下发到搜索请求的问题。
- 这些参数用于缩小检索范围，与关键词搜索共同生效。

### 实现逻辑

- CLI 层在 `SearchOptions` 中新增 `filter_host`、`filter_path`。
- 构造搜索请求体时，把：
  - `--method` 映射为 `filters.conditions += { field: "method", operator: "equals" }`
  - `--host` 映射为 `filters.conditions += { field: "host", operator: "contains" }`
  - `--path` 映射为 `filters.conditions += { field: "path", operator: "contains" }`
- 保持现有 `status/domain/protocol/content_type` 过滤方式不变，避免影响既有行为。

### 依赖项

- `crates/bifrost-cli/src/cli.rs`
- `crates/bifrost-cli/src/main.rs`
- `crates/bifrost-cli/src/commands/search.rs`

### 测试方案（含 e2e）

- 新增 CLI 请求体组装单元测试，验证 `method/host/path` 会被写入 `filters.conditions`
- 执行与 CLI 搜索相关的测试与构建校验

### 校验要求（含 rust-project-validate）

- 先执行与本次改动相关的 E2E / CLI 范围验证
- 再执行 `cargo fmt --all -- --check`
- 再执行 `cargo clippy --all-targets --all-features -- -D warnings`
- 再执行按改动范围的 `cargo test`
- 最后执行按改动范围的 `cargo build`

### 文档更新要求

- 更新 `README.md` 中的 CLI 示例
- 更新 CLI help 中 `search` 的参数说明

## 2026-03-17 CLI Search Scope 细粒度补充

### 功能模块详细描述

- 为 CLI 搜索补充与管理端一致的细粒度搜索范围：
  - `--url`
  - `--req-header` / `--res-header`
  - `--req-body` / `--res-body`
- 保留原有 `--headers`、`--body` 作为兼容别名，分别同时覆盖请求和响应两侧。

### 实现逻辑

- CLI 层在 `SearchOptions` 中新增四个细粒度 scope 字段。
- 构造搜索请求体时：
  - `--headers` 等价于同时开启 `request_headers + response_headers`
  - `--body` 等价于同时开启 `request_body + response_body`
  - 新增细粒度参数可单独打开请求侧或响应侧范围
- 只要任一 scope 被设置，就向后端发送 `scope.all = false` 的精确范围请求。

### 依赖项

- `crates/bifrost-cli/src/cli.rs`
- `crates/bifrost-cli/src/main.rs`
- `crates/bifrost-cli/src/commands/search.rs`
- `crates/bifrost-admin/src/search/types.rs`

### 测试方案（含 e2e）

- 新增请求体单元测试，验证细粒度 scope 会正确映射到 `scope.request_headers / response_headers / request_body / response_body`
- 保留兼容性测试，验证 `--headers` / `--body` 仍会同时覆盖双侧
- 执行真实 CLI 搜索验证，确认请求头、响应头、请求体、响应体可被独立检索

### 校验要求（含 rust-project-validate）

- 先执行本次 CLI scope 相关的真实验证
- 再执行 `cargo fmt --all -- --check`
- 再执行 `cargo clippy --all-targets --all-features -- -D warnings`
- 再执行按改动范围的 `cargo test`
- 最后执行按改动范围的 `cargo build`

### 文档更新要求

- 更新 `README.md` 中的 CLI 搜索示例
- 更新 CLI help 中 `search` 的 scope 参数说明
