# Bifrost 文件协议 (.bifrost)

## 现状结论

协议已经实现，但旧文档保留了很多理想化描述，没有完全反映 parser / writer / 管理端导入导出的真实行为。

## 当前已实现格式

- 头部：`VV TYPE`，writer 始终以 `{:02} {type}` 格式输出（当前 `BIFROST_FILE_VERSION = 1`，因此实际写入为 `01 <type>`）。
- 支持类型（`BifrostFileType`，serde lowercase）：
  - `rules`
  - `network`
  - `script`
  - `values`
  - `template`
- 基本结构（严格模式分隔符为 `\n---\n`；容错模式可识别行尾 `\n---`）：
  - 第一行：header
  - TOML 段（含 `[meta]`，部分类型含 `[options]`）
  - 分隔符 `---`
  - 正文内容（rules 为原始文本，其余为 JSON）

## Writer 实际写出的 `[options]` 字段

各类型 writer 写出的 `[options]` 并不一致，导入端不应假定字段集合统一：

- `rules`：`rule_count`（按非空、非注释行计数）。
- `network`：`count`、`include_body = true`、`include_response = true`。
- `script` / `values`：`count`。
- `template`：`request_count`、`group_count`。

## 关键现状

- `rules` 的 `[meta]` 含 `name`、`enabled`、`sort_order`、`version`、`created_at`、`updated_at`，以及可选 `description`、`group` 和子表 `[meta.sync]`（`rule_id`、`status`、`last_synced_at` 等），正文仍是原始规则文本。
- `network` / `script` / `values` / `template` 的 `[meta]` 由 `ExportMeta` 写出（`name`、`version`、`created_at`、可选 `description`），正文是 JSON。
- `script` 的 `ScriptItem.script_type` 由调用方提供为自由字符串，写入侧未限定枚举，类型管理端可包含 request / response / decode 等场景脚本。
- `template` 已支持 `groups + requests` 结构（`TemplateContent { groups, requests }`），`groups` 为空时在序列化中被省略。
- parser 容错能力是分级的：
  - 严格 API：`parse_rules` / `parse_network` / `parse_script` / `parse_values` / `parse_template`，任何缺失或字段错误都会返回 `ParseError`。
  - 容错 API：仅有 `parse_tolerant`（raw）、`parse_rules_tolerant` 和通用 `parse_json_tolerant`，能补默认 header、推断类型、修复尾逗号 / 未闭合括号；其它类型没有对应的 tolerant 入口，不应假设它们能自动降级恢复。

## Network 空包导入/导出约束

2026-05-20 修复 Network `.bifrost` 空包误报成功问题：

- `network` 文件正文为空数组 `[]` 时，管理端导入必须返回 `400`，错误说明文件包含 0 条记录，不能显示成功 Toast。
- Network 导出请求必须包含至少一个选中的 Traffic 记录 ID。
- WebUI 公共导出入口必须在发起请求前检查 Network 选中数量；当选中数量为 0 时只提示用户选择记录，不得调用导出 API 或触发下载。
- 如果导出时任一选中 ID 已不存在，管理端必须返回 `400`，避免静默生成少记录或 0 记录的 `.bifrost` 包。
- WebUI 导入成功提示必须读取 `record_count` 等实际导入数量；即使兼容旧服务返回 `success: true, record_count: 0`，也只能提示空包，不得自动跳转并套用 Imported 筛选。

## 与旧文档的差异

- `.bifrost` 目前既用于规则文件，也用于管理端导入导出；“全面替代所有旧 JSON 存储”不宜写成已经完成的事实。
- 旧文档未覆盖 decode script、template group 等当前真实字段。

## 权威实现位置

所有字段、错误码与容错路径以以下文件为准，文档与其冲突时以代码为准：

- [`crates/bifrost-core/src/bifrost_file/types.rs`](../crates/bifrost-core/src/bifrost_file/types.rs) — `BifrostFileType`、`RuleFileMeta`、`ExportMeta`、`NetworkRecord`、`ScriptItem`、`TemplateContent` 等类型定义与 `BIFROST_FILE_VERSION` 常量。
- [`crates/bifrost-core/src/bifrost_file/parser.rs`](../crates/bifrost-core/src/bifrost_file/parser.rs) — 严格 / 容错解析、JSON 修复、`ParseError` 枚举。
- [`crates/bifrost-core/src/bifrost_file/writer.rs`](../crates/bifrost-core/src/bifrost_file/writer.rs) — 各类型 writer、TOML 字符串转义、`[options]` 字段集合。
- [`crates/bifrost-admin/src/handlers/bifrost_file.rs`](../crates/bifrost-admin/src/handlers/bifrost_file.rs) — 管理端导入/导出 HTTP handler（含空包 400、`record_count` 返回字段、`validate_network_import_records`）。
