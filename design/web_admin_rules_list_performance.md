# Web Admin Rules 列表性能优化

## 背景

- 打开 `Rules` 页面时，前端会请求 `GET /_bifrost/api/rules`
- 旧实现中，后端会先加载全部规则文件，再对每个规则内容执行一次完整 `validate_rules_with_context`
- 当规则文件数量较多、单文件内容较大时，页面首次加载会出现明显延迟和 CPU 峰值

## 问题定位

- `crates/bifrost-admin/src/handlers/rules.rs`
  - `list_rules()` 为了返回列表摘要，逐条重新校验规则内容
- `crates/bifrost-storage/src/rules.rs`
  - `list_summaries()` 依赖 `load_all()`，会把所有规则正文完整载入

列表页实际只依赖 `name`、`enabled`、`rule_count`，不需要在列表请求阶段执行全量语法校验。

## 实现方案

1. `GET /api/rules` 改为使用 `rules_storage.list_summaries()`
2. `RulesStorage::list_summaries()` 改为逐文件读取轻量摘要
3. 对 `.bifrost` 规则文件，仅解析 `meta` 与 `options.rule_count`
4. 保留 legacy `.json` 规则文件的兼容摘要读取

## Rules 列表排序偏好持久化

Rules 左侧列表支持 `Manual`、`Updated`、`Name` 三种展示排序。排序方式属于用户可感知的 UI 偏好，应存入统一 UI 配置而不是只保存在页面内存中。

实现逻辑：

1. 在 `UnifiedConfig.ui` 中新增 `rules_sort_mode` 字段，默认值为 `manual`
2. `GET /api/config/ui` 通过 `rulesSortMode` 返回当前排序偏好
3. `PUT /api/config/ui` 接收 `rulesSortMode` 并持久化到 `config.toml`
4. Web Rules 页面加载时读取 `/config/ui` 并恢复排序方式；用户切换排序方式时立即保存
5. 前端只接受 `manual`、`updated_desc`、`name_asc`，未知值回退为 `manual`

## 预期收益

- 打开 `Rules` 页面时不再触发 N 次规则校验
- 列表读取避免为摘要场景解析完整规则正文
- CPU 峰值和页面首屏等待时间显著下降
- 刷新 WebUI 后 Rules 列表仍保持用户上次选择的排序方式

## 测试方案

- API smoke test：启动临时 `bifrost` 实例后访问 `/_bifrost/api/rules`
- Rust 单测：执行 `bifrost-storage` 相关测试，确认摘要读取与排序行为正常
- Rust 单测：验证 `UiConfig.rules_sort_mode` 默认值与 `ConfigManager::update_ui_config` 持久化
- Web E2E：在 Rules 页面切换排序方式，刷新页面后确认选择器和值顺序保持
- human_tests：更新并执行 `human_tests/webui-rules.md` 的 Rules 列表排序用例，覆盖刷新保持状态
- 最终执行 `rust-project-validate` 规定的 fmt / clippy / test / build

## 文档影响

- 新增 UI 配置字段 `ui.rules_sort_mode`，仅影响 WebUI 偏好
- 暂不需要更新 `README.md`
