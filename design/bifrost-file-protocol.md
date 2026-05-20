# Bifrost 文件协议 (.bifrost)

## 现状结论

协议已经实现，但旧文档保留了很多理想化描述，没有完全反映 parser / writer / 管理端导入导出的真实行为。

## 当前已实现格式

- 头部：`01 <type>`
- 支持类型：
  - `rules`
  - `network`
  - `script`
  - `values`
  - `template`
- 基本结构：
  - TOML `[meta]`
  - TOML `[options]`
  - `---`
  - 正文内容

## 关键现状

- `rules` 的正文仍是原始规则文本，且带 `enabled`、`sort_order`、`created_at`、`updated_at` 等元信息。
- `network` / `script` / `values` / `template` 的正文是 JSON。
- `script` 导出已不只 request/response，当前还会包含 `decode` 脚本。
- `template` 已支持 `groups + requests` 结构。
- parser 具备一定 tolerant 能力，但容错主要体现在原始/规则解析路径，不应泛化成所有类型都能随意降级恢复。

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

## 建议

- 如果继续维护本文件，应该直接以以下实现为准补全字段说明：
  - [`crates/bifrost-core/src/bifrost_file/types.rs`](../crates/bifrost-core/src/bifrost_file/types.rs)
  - [`crates/bifrost-core/src/bifrost_file/parser.rs`](../crates/bifrost-core/src/bifrost_file/parser.rs)
  - [`crates/bifrost-core/src/bifrost_file/writer.rs`](../crates/bifrost-core/src/bifrost_file/writer.rs)
