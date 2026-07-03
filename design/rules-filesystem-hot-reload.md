# Rules Filesystem Hot Reload

## 背景

Bifrost 运行时（`bifrost start` 常驻进程）已经支持通过 `ConfigChangeEvent::RulesChanged` 热更新 resolver、Badge 活跃规则缓存以及临时端口绑定规则。但 CLI 本地规则命令 (`bifrost rule add/update/delete`) 和用户直接编辑 `rules/` 目录里的 `.bifrost` 文件时，只修改磁盘文件，不经过 Admin API，因此运行中的代理进程无法感知变化。未登录 Sync 的用户更依赖本地规则和 CLI 操作，一旦代理运行时使用旧规则、Web UI 又显示 “已启用”，就会出现严重不一致。

本模块在 `bifrost start` 常驻进程中监听规则存储目录的系统文件事件，并使用文件快照做最终变化判定：系统 watcher 负责低延迟触发，低频 fallback scan 兜底极端平台事件丢失或目录重建的情况；变化确认稳定后，向现有 `ConfigManager` 发送 `ConfigChangeEvent::RulesChanged(RulesChangeOrigin::Filesystem)`，复用现有热更新链路刷新 resolver、Badge cache 和临时端口规则。

2026-06 日志风暴回归修复后，热更新还必须区分“运行时语义变化”和“同步元信息变化”，避免多端并发拉 Group 时不停写盘并互相唤醒 Sync。

## 用户目标验证清单

### 必须实现

- CLI `rule add/update/delete` 写入本地文件后，运行中的代理无需重启即可感知变化。
- 用户手工编辑或删除 `.bifrost` 文件后，运行时同样感知。
- Filesystem watcher 与 fallback scan 都能触发 `ConfigChangeEvent::RulesChanged(RulesChangeOrigin::Filesystem)`。
- 快照只指纹运行时会使用的规则字段：相对路径、规则名、启用状态、排序、group 和 canonical 规则正文。
- `last_synced_at`、`remote_updated_at`、`last_synced_content_hash` 等同步元信息变化不触发 runtime reload。
- Group 规则列表同步幂等：远端 rule 先转成本地 canonical 内容再和已落盘规则比较，避免每次 GET 都写盘。
- 同一进程内同一 group 的本地落盘同步串行化，避免多端并发读取时重复写。
- `RulesChanged` 事件带 `RulesChangeOrigin`：`LocalApi` 和经过 runtime 指纹确认的 `Filesystem` 变化会立即唤醒 Sync；`RemoteSync` 只刷新 runtime，不反过来唤醒 Sync。

### 必须不破坏

- 真实规则正文、启用状态、排序、group 或文件增删仍必须触发 runtime reload。
- Badge cache 与临时端口绑定规则在 reload 后一致。
- 未登录 Sync 时 group 子目录被过滤，reload 不会误加载不该出现的 group 规则。
- `.group_cache.json` 之类点文件不参与 reload；旧版 `legacy.json` 仍纳入。

### 必须真实验证

- CLI 本地规则写入 / 修改 / 删除后，代理请求状态码自动变化，`/api/rules/active-summary` 自动更新。
- 直接改 `.bifrost` 文件内容 → 代理和 active summary 自动更新。
- 直接删除 `.bifrost` 文件 → 代理恢复原始响应且 active summary 清理。
- 重复 Group env 同步不写盘、不刷新 `last_synced_at`。
- Metadata-only 文件变化不触发 runtime snapshot；真实规则内容变化仍触发。

## 产品语义

### RulesChangeOrigin 分级

- `RulesChangeOrigin::LocalApi`：Admin API 写入、CLI 走 Admin HTTP 写入 → runtime + Sync 都唤醒。
- `RulesChangeOrigin::Filesystem`：filesystem watcher 或 fallback scan 检测到并被 runtime 指纹确认 → runtime + Sync 都唤醒。
- `RulesChangeOrigin::RemoteSync`：远端拉下来同步到本地 → 只刷新 runtime，不唤醒 Sync，避免 sync 写盘触发 filesystem watcher 又唤醒 Sync 形成自激环。

### 快照粒度只包含运行时字段

`crates/bifrost-cli/src/commands/start.rs::collect_rules_filesystem_snapshot` 收集：

- 相对路径（把 base_dir 剥掉后的 path）。
- 若 `.bifrost` 能解析：rule name、enabled、sort_order、group、canonical rule text（`content_hash` 只覆盖运行时字段）。
- 若 legacy `.json` 或无法解析：退回全文 hash。

不参与快照的字段：`last_synced_at` / `remote_updated_at` / `last_synced_content_hash` / description 元数据等。

### Group 同步幂等

`crates/bifrost-admin/src/handlers/group_rules.rs::sync_envs_to_local`：

- 远端 rule 先转换成本地 canonical 内容（协议别名归一化、去掉最终换行、清理 sync 元信息）。
- 与已落盘规则做字节比较；若一致，跳过写入，不刷新 `last_synced_at`。
- 同一 group 用同一把进程内锁串行化（`group_sync_lock_reuses_lock_for_same_group`）。

## 技术细节

### 启动装配

- `crates/bifrost-cli/src/commands/start.rs`：
  - `let rules_watcher_task = spawn_rules_watcher_task(...);` 消费 `ConfigChangeEvent::RulesChanged`。
  - `let rules_filesystem_watcher_task = spawn_rules_filesystem_watcher_task(...);` 监听 `RulesStorage::base_dir()`。
  - Daemon 与 foreground 分支都必须装配这两个 task（`start.rs` 里两处调用位置需保持一致）。
- 快照采样：
  - `collect_rules_filesystem_snapshot(base_dir)` 递归调用 `collect_rules_filesystem_snapshot_dir(...)`。
  - 只纳入支持的规则扩展名（`.bifrost` / legacy `.json`）；跳过 `.group_cache.json` 等点文件。

### 事件回路

1. 启动 resolver 后启动 `spawn_rules_filesystem_watcher_task`。
2. watcher 递归监听 base_dir；create/modify/remove/any → 触发一次快照采样。
3. 快照记录相对路径 + 运行时语义指纹；`.bifrost` 优先解析 rule meta/content 后忽略同步元信息，无法解析或 legacy `.json` 退回全文 hash。
4. 系统 watcher 失败或事件丢失时，30 秒 fallback scan 兜底。
5. 快照变化 → 短 debounce → 再次采样确认稳定 → `ConfigChangeEvent::RulesChanged(RulesChangeOrigin::Filesystem)`。
6. `spawn_rules_watcher_task` 收到事件后：
   - 重新加载启用规则；
   - 按当前 Sync 登录态过滤 group 子目录；
   - 更新 `DynamicRulesResolver`；
   - 刷新 Badge 活跃规则缓存；
   - 重载临时端口规则绑定。

### 依赖

- `notify = "6"`：跨平台系统文件 watcher（`skills` crate 已在使用，本模块在 `bifrost-cli` 中显式声明直接依赖）。
- `std::fs` + `DefaultHasher` 生成进程内快照指纹。
- 复用 `ConfigManager::notify(ConfigChangeEvent::rules_changed(...))`。

## CLI + Web + Admin API

- 本模块不新增 CLI 参数、规则语法、Admin API 或 README 用户说明。
- Admin API 仍通过 `RulesChangeOrigin::LocalApi` 触发 reload；此路径以前就存在，本模块只补齐 CLI/直接文件编辑路径。
- `bifrost port active <port>` / `/api/rules/active-summary` 在 reload 后立刻反映新状态。

## Sync 边界

- Sync 写盘要走 `RulesChangeOrigin::RemoteSync`，避免被自己触发的 filesystem watcher 再唤醒 Sync。
- CLI / 直接编辑走 `LocalApi` / `Filesystem`，需要唤醒 Sync 让本地修改上推。
- Group 同步幂等 + 同一 group 加锁，保证多端并发 GET group 不会重复写文件、不会互相唤醒对方触发的 Sync。

## Phase 划分

### Phase 1：快照与 watcher 骨架

- 引入 `notify` 依赖，实现 `collect_rules_filesystem_snapshot(_dir)`。
- 在 `bifrost start` 装配 `spawn_rules_filesystem_watcher_task`（daemon + foreground 分支）。
- 事件流水与短 debounce。

### Phase 2：快照粒度收敛

- `.bifrost` 优先解析 rule meta/content；忽略 sync 元信息。
- 支持 legacy `.json` 全文 hash。
- 点文件 (`.group_cache.json`) 与不支持的扩展名跳过。

### Phase 3：Group 幂等与并发

- `sync_envs_to_local` 归一化远端 env 内容后比较。
- 同一 group 加锁串行化落盘。

### Phase 4：RulesChangeOrigin 分级与文档

- 引入 `RulesChangeOrigin::{LocalApi, Filesystem, RemoteSync}`。
- Sync manager 消费事件时按 origin 判断是否唤醒。
- 更新 `human_tests/rules-filesystem-hot-reload.md` 与索引。

## 测试方案

### 单元测试（实际落地）

在 `crates/bifrost-cli/src/commands/start.rs::tests`：

- `rules_filesystem_snapshot_detects_same_length_content_changes`：同长度规则内容替换也改变快照。
- `rules_filesystem_snapshot_tracks_nested_rule_files`：子目录规则文件被纳入。
- `rules_filesystem_snapshot_ignores_dot_json_cache_files`：`.group_cache.json` 等点文件不参与 reload；legacy `.json` 仍纳入。
- `rules_filesystem_snapshot_ignores_sync_metadata_only_changes`：只改同步元信息不触发 runtime snapshot 变化。
- `rules_filesystem_snapshot_detects_runtime_rule_content_changes`：真实规则正文变化仍触发。
- `collect_rules_filesystem_snapshot_handles_missing_base_dir`
- `collect_rules_filesystem_snapshot_errors_for_non_directory`
- `collect_rules_filesystem_snapshot_ignores_unrelated_extensions`
- `collect_rules_filesystem_snapshot_includes_supported_files`
- `rules_filesystem_snapshot_equality_detects_difference`
- `rules_filesystem_snapshot_equality_treats_identical_snapshots_as_equal`
- `spawn_rules_filesystem_watcher_task_noop_without_config_manager`
- `spawn_rules_watcher_task_noop_without_config_manager`

在 `crates/bifrost-admin/src/handlers/group_rules.rs::tests`：

- `sync_envs_to_local_skips_unchanged_normalized_remote_rule_write`：远端 legacy 格式规则同步到本地 canonical 后，第二次相同远端 env 不写盘、不刷新 `last_synced_at`。
- `group_sync_lock_reuses_lock_for_same_group`：同一 group 用同一把进程内锁。
- `sync_envs_to_local_reports_enabled_rule_content_changes`
- `sync_envs_to_local_reports_enabled_rule_removal`
- `sync_envs_to_local_ignores_inactive_remote_metadata_changes`
- `sync_envs_to_local_skips_unchanged_remote_rule_write`

### E2E 测试

`e2e-tests/tests/test_rules_filesystem_hot_reload.sh`：

- 启动真实 Bifrost 代理 + 本地 echo server。
- CLI `rule add` 写入本地规则（不调用 Admin API 保存）→ 代理请求状态码自动变化，`/api/rules/active-summary` 自动包含新规则。
- CLI `rule update` 修改本地规则 → 同上。
- 直接改 `.bifrost` 文件内容 → 代理和 active summary 自动更新。
- CLI `rule delete` 删除本地规则 → 代理恢复原始响应且 active summary 清理。
- 直接删除 `.bifrost` 文件 → 同上。

### human_tests

`human_tests/rules-filesystem-hot-reload.md` 覆盖 CLI 本地规则写入、直接文件编辑、直接文件删除三条用户可感知路径。同一文档新增 Group 同步日志风暴回归条目：

- TC-RFHR-01：CLI 本地 add → 代理立即命中。
- TC-RFHR-02：CLI 本地 update → 代理立即反映最新。
- TC-RFHR-03：CLI 本地 delete → 代理恢复默认响应。
- TC-RFHR-04：直接编辑 `.bifrost` 内容 → 代理立即反映。
- TC-RFHR-05：直接删除 `.bifrost` → 代理恢复默认响应。
- TC-RFHR-06：重复 Group env 同步不写盘、不刷新 `last_synced_at`。
- TC-RFHR-07：Metadata-only 文件变化不触发 runtime snapshot。
- TC-RFHR-08：真实规则内容变化仍触发 snapshot。

执行方式优先使用同一 E2E 脚本（`test_rules_filesystem_hot_reload.sh`），因为它跑真实 CLI、真实代理、真实 Admin active summary 和真实 HTTP 代理请求。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：CLI / 直接文件写入都触发 runtime reload。
- Review `start.rs` watcher 生命周期、快照粒度、daemon/foreground 分支覆盖。
- Review `group_rules.rs::sync_envs_to_local` 归一化与锁复用。
- 跑单元测试与新增 E2E 脚本。

### 第 2 轮

- 复查第 1 轮修复后的 diff、human_tests 文档和索引。
- 复跑受影响测试，确认没有只覆盖 Admin API 的旧路径。
- 跑项目级格式/校验命令，记录未执行项原因。

若仍出现日志风暴 / 只覆盖 Admin API 路径 / metadata-only 变化触发 reload / RemoteSync 触发 Sync 自激环，追加第 3 轮。

## 校验命令

- `cargo test -p bifrost-cli rules_filesystem_snapshot -- --nocapture`
- `cargo test -p bifrost-cli spawn_rules_filesystem_watcher_task -- --nocapture`
- `cargo test -p bifrost-admin group_rules::tests:: -- --nocapture`
- `BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh`
- `cargo fmt --all -- --check`
- 按 `rust-project-validate` 技能执行适用校验。

## 文档更新要求

- 更新 `human_tests/rules-filesystem-hot-reload.md` 与 `human_tests/readme.md` 索引。
- 本次不改变 CLI 参数、规则语法、Admin API 或 README 用户说明。

## 风险与决策

- **notify crate 平台差异**：Linux inotify、macOS FSEvents、Windows ReadDirectoryChangesW 事件粒度不同；30 秒 fallback scan 是必要兜底，不能只依赖系统 watcher。
- **快照粒度过粗 → 日志风暴**：如果指纹包含 `last_synced_at` 会让每次同步都误报变化，这是 2026-06 回归的根因。快照粒度收敛必须在代码 review 时把 `last_synced_at` / `remote_updated_at` / `last_synced_content_hash` 显式列入排除清单。
- **快照粒度过细 → 漏 reload**：只 hash content 会漏掉 rename / enabled 切换；快照必须包含 `enabled` / `sort_order` / `group` / rule name。
- **RemoteSync 触发 Sync 的自激环**：`RulesChangeOrigin::RemoteSync` 必须只刷新 runtime，不唤醒 Sync；一旦漏掉 origin 分级，Sync 会持续被自己的写盘唤醒。
- **Group 并发写盘**：多端 WebUI/CLI 同时 GET 同一 group 时若不加锁，全部拿到旧内容再各自写盘，容易触发多次 `RulesChanged`。当前用同一把进程内锁串行化，跨进程场景需要在 `spawn_rules_watcher_task` 层通过 debounce 兜底。
