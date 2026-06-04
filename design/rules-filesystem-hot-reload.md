# Rules Filesystem Hot Reload

## 功能模块说明

规则运行时已经支持通过 `ConfigChangeEvent::RulesChanged` 热更新解析器、Badge 活跃规则缓存和临时端口绑定规则。但 CLI 本地规则命令和用户直接编辑 `rules/` 目录文件时，只修改磁盘文件，不经过 Admin API，因此运行中的代理进程无法感知变化。未登录 Sync 时用户更容易依赖本地规则和 CLI 操作，于是会出现 UI 显示启用、代理运行时仍使用旧规则的严重不一致。

本模块在 `bifrost start` 常驻进程中监听规则存储目录的系统文件事件，并使用文件快照做最终变化判定。系统 watcher 负责低延迟触发；低频 fallback scan 负责兜底，避免极端平台事件丢失或目录重建导致漏更新。变化被确认稳定后，向现有 `ConfigManager` 发送 `RulesChanged`，复用现有热更新链路完成 resolver、Badge cache 和临时端口规则刷新。

## 实现逻辑

1. 启动规则 resolver 后，同时启动 `spawn_rules_filesystem_watcher_task`。
2. watcher 递归监听 `RulesStorage::base_dir()`；收到 create/modify/remove/any 事件后触发一次快照校验。
3. 快照记录相对路径、文件长度和内容 hash，避免同长度内容修改被漏判。
4. 如果系统 watcher 创建失败或事件丢失，30 秒 fallback scan 会重新比较快照并补发事件。
5. 发现快照变化后等待短 debounce，再重新采样；稳定变化才触发 `ConfigChangeEvent::RulesChanged`。
6. 现有 `spawn_rules_watcher_task` 收到事件后继续执行：
   - 重新加载启用规则；
   - 按当前 Sync 登录态过滤 group 子目录；
   - 更新 `DynamicRulesResolver`；
   - 刷新 Badge 活跃规则缓存；
   - 重载临时端口规则绑定。

## 依赖项

- `notify = "6"`：跨平台系统文件 watcher。该 crate 已被 workspace 中 `skills` crate 使用，本模块只是在 `bifrost-cli` 中显式声明直接依赖。
- 使用 `std::fs` 递归扫描和 `DefaultHasher` 生成进程内变更指纹。
- 复用现有 `ConfigManager::notify(ConfigChangeEvent::RulesChanged)`。

## 测试方案

### 单元测试

- `rules_filesystem_snapshot_detects_same_length_content_changes`：同长度规则内容替换也会改变快照。
- `rules_filesystem_snapshot_tracks_nested_rule_files`：子目录规则文件被纳入快照。
- `rules_filesystem_snapshot_ignores_dot_json_cache_files`：`.group_cache.json` 等点文件不会触发规则 reload，旧版 `legacy.json` 仍被纳入。

### E2E 测试

- `e2e-tests/tests/test_rules_filesystem_hot_reload.sh`：
  - 启动真实 Bifrost 代理和本地 echo server；
  - 使用 CLI `rule add` 写入本地规则，不调用 Admin API 保存；
  - 使用 CLI `rule update` 修改本地规则，不调用 Admin API 保存；
  - 验证代理请求状态码自动变化，`/api/rules/active-summary` 自动包含新规则；
  - 直接修改 `.bifrost` 文件内容，验证代理和 active summary 自动更新；
  - 使用 CLI `rule delete` 删除本地规则，不调用 Admin API 保存；
  - 直接删除 `.bifrost` 文件，验证代理恢复原始响应且 active summary 清理。

### 真实场景测试

- `human_tests/rules-filesystem-hot-reload.md` 覆盖 CLI 本地规则写入、直接文件编辑、直接文件删除三条用户可感知路径。
- 执行方式优先使用同一 E2E 脚本，因为该脚本运行真实 CLI、真实代理、真实 Admin active summary 和真实 HTTP 代理请求。

## Review/Fix/Test 闭环方案

第 1 轮复核：
- 对照用户目标确认 CLI/直接文件写入都会触发运行时 reload。
- Review `start.rs` watcher 生命周期、快照粒度、daemon/foreground 分支覆盖。
- 运行精准单元测试和新增 E2E 脚本。

第 2 轮复核：
- 复查第 1 轮修复后的 diff、human_tests 文档和索引。
- 复跑受影响测试，确认没有只覆盖 Admin API 的旧路径。
- 运行项目级格式/校验命令，记录未执行项原因。

## 校验要求

- `cargo test -p bifrost-cli rules_filesystem_snapshot -- --nocapture`
- `BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh`
- `cargo fmt --all -- --check`
- 按 `rust-project-validate` 技能执行适用校验。

## 文档更新要求

- 更新 `human_tests/readme.md` 索引。
- 本次不改变 CLI 参数、规则语法、Admin API 或 README 用户说明。
