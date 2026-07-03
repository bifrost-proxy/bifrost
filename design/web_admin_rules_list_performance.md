# Web Admin Rules 列表性能优化

## 背景

- Rules 页面首屏会调用 `GET /_bifrost/api/rules`。
- 早期实现里，后端为了返回列表页所需的 `{name, enabled, rule_count}`，会 `load_all()` 把所有规则正文完整解析，再逐条 `validate_rules_with_context` 走一遍 lint 校验。
- 用户规则数超过几十条、且部分规则 200+ 行时，首屏一次请求会：
  1. 阻塞 event loop 数百毫秒；
  2. 拉高 CPU 峰值到 1 core；
  3. 让 Web UI 出现明显白屏。

Rules 列表页真正需要的字段其实只有 `name / enabled / rule_count / sort_order / updated_at / capability flags`，无需在列表阶段执行全量语法校验。同时用户希望把列表排序偏好（Manual / Updated / Name）跨会话持久化，而不是每次刷新回退到 Manual。

本文档合并这两条优化的当前实现：**列表摘要读取轻量化 + 排序偏好持久化**，并给出对应测试位置。

## 用户目标验证清单

### 必须实现

- `GET /api/rules` 不再触发完整规则校验，只走 `RulesStorage::list_summaries()` 的轻量摘要读取。
- `list_summaries()` 逐文件读取摘要：对 `.bifrost` 只解析 `meta` 与 `options.rule_count`，不加载全部规则正文。
- 兼容 legacy `.json` 规则文件的摘要读取。
- Rules 列表支持 `manual` / `updated_desc` / `name_asc` 三种排序。
- 排序偏好保存在 `UnifiedConfig.ui.rules_sort_mode`（默认 `manual`）。
- `GET /api/config/ui` 返回 `rulesSortMode`，`PUT /api/config/ui` 接收 `rulesSortMode` 并持久化到 `config.toml`。
- Web Rules 页面加载时读取 `/config/ui` 恢复排序；用户切换排序方式立即保存。
- 前端仅接受合法枚举，未知值回退 `manual`。

### 必须不破坏

- 系统保留规则（如 `Default`）仍固定置顶，不被普通排序偏好推到中间。
- CLI `bifrost rule list` 复用 `list_summaries()`，行为与 Web 列表一致。
- 规则详情/编辑 API（`GET /api/rules/{name}`）仍返回完整正文并做校验。
- 非 `.bifrost` 与 `.json` 的文件（例如 `.group_cache.json`）不会被误当作规则解析（由 `list()` 的候选过滤保证）。

### 必须真实验证

- API smoke：`GET /_bifrost/api/rules` 在 50+ 规则场景下延迟 < 100ms（单机开发环境）。
- Web E2E：切换排序方式 → 刷新页面 → 选择器与顺序保持。
- Rust 单测：`list_summaries` 忽略非规则文件、`update_ui_config` 持久化 `rules_sort_mode`。

## 产品语义

### 「摘要」是列表页的最小数据集

`RuleSummary { name, enabled, sort_order, rule_count, created_at, updated_at }` 是列表页需要展示的最小字段。列表 API 只关心这些字段 + 权限 flag（`can_delete` / `can_disable` / `can_rename` / `can_reorder` / `can_edit_content`）；后者由 `rule_capabilities(name)` 计算，对系统 Default 强制约束。

规则正文与语法诊断只在打开某条规则时按需加载，这样：

- 列表页 IO 复杂度：`O(n * meta_size)`，而非 `O(n * body_size)`。
- 列表页 CPU 复杂度：不做 `validate_rules_with_context`。

### 排序偏好属于 UI 配置

`rules_sort_mode` 是「用户可感知的 UI 偏好」，应跨浏览器会话持久化。放在 `UnifiedConfig.ui` 中而不是 localStorage，能在多标签甚至多设备（若接入 sync）间保持一致。

合法枚举当前限定为：

- `manual`（默认，按 `sort_order` 排）
- `updated_desc`（按 `updated_at` 倒序）
- `name_asc`（按 `name` 升序）

前端在读取到未知值时回退 `manual`，避免脏配置导致 UI 崩溃。

## 技术细节

### 后端

- `crates/bifrost-storage/src/rules.rs`
  - `RulesStorage::list_summaries()`（第 842 行起）
    - `let names = self.list()?;` 拿到候选文件名（`list()` 已经过滤掉 `.group_cache` 等非规则文件，见 `cli-rule-list-legacy-skip.md`）。
    - 对每个 name 调用 `load_summary(&name)`，失败仅 `warn!` 跳过，不阻断整表返回。
    - 最后调用 `sort_rule_summaries` 保证系统 Default 仍置顶。
  - `sort_rule_summaries` 保留「系统 Default 优先 → `sort_order` → `name`」的排序不变式。
- `crates/bifrost-admin/src/handlers/rules.rs::list_rules`（第 615 行起）
  - `state.rules_storage.list_summaries()` → 映射成 `RuleFileInfo`。
  - `rule_capabilities(&info.name)` 覆盖系统规则的 flag。
- `crates/bifrost-storage/src/unified_config.rs`
  - `UiConfig.rules_sort_mode: String`（默认 `"manual"`）。
  - `UiConfigUpdate.rules_sort_mode: Option<String>`。
- `crates/bifrost-storage/src/config_manager.rs::update_ui_config`
  - 应用 `if let Some(rules_sort_mode) = update.rules_sort_mode { config.ui.rules_sort_mode = rules_sort_mode; }`。
- `crates/bifrost-admin/src/handlers/config.rs`
  - `UiConfigResponse.rules_sort_mode: String` / `UiConfigUpdateRequest.rules_sort_mode: Option<String>`。
  - `PUT /api/config/ui` 落库到 `config.toml`。

### 前端

- Rules 页面加载时：
  1. `GET /api/config/ui`；
  2. 读取 `rulesSortMode`，若非合法枚举则回退 `manual`；
  3. 应用到列表排序状态。
- 切换排序方式：
  1. 立即更新本地状态；
  2. `PUT /api/config/ui { rulesSortMode: "<mode>" }`；
  3. 失败时提示但不回滚（下一次刷新会从持久值恢复）。

### CLI

- `crates/bifrost-cli/src/commands/rule.rs::list` 使用 `storage.list_summaries()`，输出与 Web 列表一致。CLI 侧不使用 `rules_sort_mode`（TUI 里的排序是即时视图状态）。

## Sync 边界

- Rules 列表本身与 sync 无关，只是本地读取。
- `UnifiedConfig.ui.rules_sort_mode` 是 UI 偏好，属于本地 `config.toml`，当前不进入 sync 载荷；如未来 UI 偏好接入 sync，需要在设计里明确迁移策略。

## Phase 1 – 摘要 API 落地

- 新增 `RulesStorage::list_summaries()`。
- `GET /api/rules` 改用摘要读取。
- 单测覆盖非规则文件被跳过。

## Phase 2 – Sort 偏好持久化

- `UnifiedConfig.ui.rules_sort_mode` 字段与默认值 `manual`。
- `GET/PUT /api/config/ui` 支持 `rulesSortMode`。
- `ConfigManager::update_ui_config` 持久化。

## Phase 3 – 前端消费

- Rules 页面读取偏好并应用到排序状态。
- 切换排序即时 PUT。
- 非法值回退 `manual`。

## Phase 4 – 系统规则一致性

- 保证 `sort_rule_summaries` 让系统 Default 始终置顶，不被 Sort 偏好推走。
- CLI `bifrost rule list` 输出与 Web 列表保持一致。

## 测试方案

### Rust 单元测试

- `crates/bifrost-storage/src/rules.rs`
  - `test_list_summaries`
  - `test_list_summaries_ignores_non_bifrost_files`
- `crates/bifrost-storage/src/unified_config.rs`
  - 默认值断言：`assert_eq!(config.ui.rules_sort_mode, "manual");`
  - 序列化往返：`assert_eq!(config.ui.rules_sort_mode, parsed.ui.rules_sort_mode);`
- `crates/bifrost-storage/src/config_manager.rs`
  - `test_update_ui_rules_sort_mode_persists`
  - `test_get_and_update_ui_config`

### E2E / API smoke

- （新增）`e2e-tests/tests/test_rules_list_summary_perf.sh`
  - 启动临时 daemon，导入 50+ 规则；断言 `GET /_bifrost/api/rules` 响应时间显著低于 legacy 全量校验实现。
- 现有 `test_rules_admin_api.sh` 系列覆盖列表返回字段结构。

### Web E2E

- （新增或扩展）`web/tests/ui/rules-sort-mode.spec.ts`
  - 切换 `updated_desc` → 刷新页面 → 选择器与顺序保持。
  - 切换 `name_asc` → 刷新 → 保持。
  - 手工塞入非法值 `custom` → 前端回退 `manual`。

### 真实场景 human_tests

- `human_tests/webui-rules.md`：新增 Rules 列表排序保持用例。
- 更新 `human_tests/readme.md` 中 WebUI Rules 用例数量。

启动约束：临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-storage list_summaries`
- `cargo test -p bifrost-storage update_ui_rules_sort_mode`
- `cargo test -p bifrost-admin list_rules`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 `list_rules` handler 未意外触发 `load_all` / `validate_rules_with_context`。
- 复核 `list_summaries` 的错误处理：单条规则损坏时不能整表失败。
- 复核 `sort_rule_summaries` 中系统 Default 置顶不变式。
- 复核前端 sort 切换与 `PUT /api/config/ui` 的失败降级。

### 第 2 轮

- 大量规则（100+）场景手工回归首屏耗时。
- `updated_desc` 排序在批量导入后是否与实际 `updated_at` 一致。
- 切换 sort mode 后系统 Default 仍在首位。

## 文档影响

- 新增 UI 配置字段 `ui.rules_sort_mode`，仅影响 WebUI 偏好。
- 无需更新 `README.md`（属于 UI 内部行为）。
- `docs/` 中 API 文档需在 `PUT /api/config/ui` 中补 `rulesSortMode` 参数说明。

## 风险与决策

- **列表 vs 详情语义分裂**：列表不再做校验，用户在列表页看不到「有语法错误」的红点；如需列表级校验反馈，需要单独增量校验器，不在本方案内。
- **非法排序值**：若配置文件被外部改坏，前端回退 `manual`，后端保留原字符串（不主动修正）；下次 UI 保存时会覆盖。
- **多设备同步**：`rules_sort_mode` 目前不进入 sync，避免多设备偏好互相覆盖。
- **系统 Default 与 sort 的交互**：所有 sort 模式下 Default 都被强制置顶，不给用户「把 Default 移走」的错觉。
