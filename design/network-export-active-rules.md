# Network 导出生效规则快照

## 功能模块详细描述

Network 右键导出 `.bifrost` 文件时，除了请求/响应内容与实际命中的 `matched_rules`，还需要附带该请求进入 Bifrost 时对应端口正在生效的规则快照。这样用户反馈问题时，只要提供导出的请求文件，排查方就能同时看到请求本身、已命中的规则，以及当时可能影响该请求的端口级规则上下文。

导出规则快照按请求记录的 `listener_port` 判定：

- 默认端口流量：导出默认代理端口启用的规则，包含每个规则文件的名称、规则数量、分组信息、原始内容，以及合并后的 `merged_content`。
- 自定义端口流量：导出该自定义端口绑定的启用规则，包含端口绑定规则列表、规则内容与合并后的 `merged_content`。
- 无法读取自定义端口规则时：仍导出请求记录，并在 `active_rules.unavailable_reason` 中写明原因，避免静默误导。

## 实现逻辑

`.bifrost` network record 新增字段：

- `listener_port`：请求实际进入的代理端口。
- `active_rules`：当前端口生效规则快照。
  - `source`：`default_port` 或 `custom_port`。
  - `admin_port`：主 Bifrost 端口。
  - `listener_port`：本快照对应的请求入口端口。
  - `total`：本快照内规则文件总数（聚合计数，便于消费端快速展示）。
  - `rules`：规则文件级明细，包含 `name`、`rule_count`、`group_id`、`group_name`、`content`。
  - `merged_content`：按照运行时加载顺序拼接后的完整规则内容。
  - `unavailable_reason`：快照无法获取时的原因。

后端导出入口 `/_bifrost/api/bifrost-file/export/network` 在 `traffic_to_network_record` 组装 record 时生成规则快照。默认端口从 `AdminState.rules_storage` 和 group 规则目录读取启用规则；自定义端口通过 `TemporaryPortManager::active_summary(listener_port)` 读取该端口绑定规则，避免把默认端口规则误导出到自定义端口请求中。

## 依赖项

- `TrafficRecord.listener_port`：识别请求来源端口。
- `RulesStorage::load_enabled()`：读取默认端口启用规则。
- `TemporaryPortManager::active_summary()`：读取自定义端口绑定规则。
- `.bifrost` network parser/writer：新字段保持 serde 向后兼容，旧文件缺少字段时可继续导入。

## 测试方案

### 单元测试

- `network_export_attaches_default_port_active_rules`：默认端口请求导出 `default_port` 快照，只包含启用规则，不包含禁用规则。
- `network_export_uses_custom_port_active_rules_for_request_port`：自定义端口请求导出 `custom_port` 快照，不混入默认端口规则。

### E2E 测试

- 更新 `e2e-tests/tests/test_temporary_port_bindings.sh`：在真实 Bifrost 服务中分别产生默认端口和临时端口流量，调用 Network export API，解析 `.bifrost` 内容并断言 `active_rules.source`、`listener_port`、`merged_content` 和规则内容均对应请求端口。

### 真实场景测试

- 新增 `human_tests/network-export.md`：覆盖默认端口导出、自定义端口导出、空选择导出保护和导入兼容检查。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标：请求导出必须包含端口对应生效规则，默认端口与自定义端口必须区分。
- 代码 review：检查序列化结构、端口判断、默认规则读取、自定义端口规则读取和错误显式化。
- 复测：运行 targeted Rust 单测和 `test_temporary_port_bindings.sh`。

第 2 轮：

- 复查最新 diff：确认 design、human_tests、E2E、单测和导出 JSON 字段一致。
- 复测：复跑受影响测试，补跑格式/Clippy/工作区测试。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin network_export`
- `e2e-tests/tests/test_temporary_port_bindings.sh`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/readme.md` 索引，新增 Network 导出生效规则快照真实场景用例。
- 本次变更不新增 CLI 参数、配置项或公开规则协议，README 无需同步。
