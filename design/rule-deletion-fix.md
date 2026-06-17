# 规则删除阻塞修复

## 功能模块详细描述

修复已同步规则（有 `remote_id`）在 `sync_manager` 不可用时无法删除的严重 Bug。

### 问题表现
1. 启用的规则被删除后仍然显示为启用状态
2. 规则无法被删除，前端显示操作无响应
3. 由于规则文件未从磁盘移除，代理继续使用该规则匹配流量

### 根因分析
`delete_rule` 处理器在删除有 `remote_id` 的规则时，如果 `sync_manager` 不可用（未配置同步服务），会直接返回 `SERVICE_UNAVAILABLE (503)` 错误，阻止了本地文件删除。前端捕获该错误后未向用户显示有用的错误信息。

## 实现逻辑

### 后端修复 (`crates/bifrost-admin/src/handlers/rules.rs`)
- 当 `sync_manager` 不可用时，记录警告日志但不阻止删除
- 当 `record_deleted_rule` 失败时，记录警告日志但仍继续执行本地删除
- 删除后正常触发 `RulesChanged` 通知和缓存失效

### 前端修复 (`web/src/stores/useRulesStore.ts`)
- 使用 `normalizeApiErrorMessage` 获取更友好的错误消息
- 通过 `message.error` 直接向用户显示删除失败的原因

## 依赖项
- 无新依赖

## 测试方案

### 单元测试
- `test_delete_synced_rule_without_sync_manager`：验证无 sync_manager 时删除有 remote_id 的规则仍然成功 (planned, not yet shipped as of 2026-06-17)

### E2E 测试
- 创建规则 → 启用规则 → 删除规则 → 验证规则列表不再包含该规则

### 真实场景测试
- 启动 bifrost 服务，创建规则，模拟同步标记后删除，观察规则列表是否正确更新

## 校验要求
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求
- 无文档更新需求
