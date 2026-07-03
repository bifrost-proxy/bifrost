# Rule Sync Strategy

## 背景

Bifrost 的规则同步（`crates/bifrost-sync/src/manager.rs`）需要在远端服务不可修改、且没有版本号 / CAS / `If-Match` 支持的前提下，稳定地把多台设备之间的 rule 变化收敛。之前的做法混用 `name` / `remote_id` / `last_synced_at` 做主键或冲突判定，导致：

- 本地删除后又被远端同步回来。
- 多端同时编辑后出现莫名其妙的创建/更新/删除失败。
- 同名规则被误判为同一对象。
- 状态越补越复杂，最后谁也说不清该覆盖谁。

本方案明确选择：

- 不做冲突管理。
- 不引入服务端版本协商。
- 直接覆盖。

也就是接受“最终一致 + 明确优先级”，不追求强一致。

## 用户目标验证清单

### 必须实现

- 本地稳定 `rule_id` 作为逻辑主键；rename 不变。
- 同步主键优先使用 `remote_id`（而不是 `name`）。
- 删除意图写在 `sync-state.json::deleted_rules` 的 tombstone 中，tombstone 不清理就不允许把同 `remote_id` / 同 `rule_name` 对象拉回。
- 本地 `modified` 一律 push；本地 `synced` 才允许 pull。
- 本地新建 `local_only` 一律 create 到远端；失败时保持 `local_only` 等待重试。
- `sync_envs_to_local` 幂等：同一远端 env 第二次同步不写盘、不刷新 `last_synced_at`。
- 同一进程内同一 group 的落盘同步用同一把锁串行化，避免多端并发读取导致重复写。
- Startup 自动登录提示去重（`startup_login_prompt`）。

### 必须不破坏

- 已同步规则的 `remote_id` / `remote_user_id` / `remote_created_at` / `remote_updated_at` 元数据。
- 用户手工编辑 `.bifrost` 文件后运行时能通过 filesystem watcher 感知（详见 `rules-filesystem-hot-reload.md`）。
- Group 规则的 legacy 协议别名 / 最终换行归一化：远端 rule 先转成本地 canonical 内容再比较，避免每次 GET 都重复写。
- Group 同步不受 tombstone 影响（tombstone 只针对 My Rules 单文件规则）。

### 必须真实验证

- 多端删除：A 删本地 → tombstone 持续删远端；B synced 时如果远端已消失，删除本地副本。
- 多端修改：谁后同步谁覆盖。
- 本地 `modified` + 远端消失：重新 create，保留本地修改优先。
- 远端删除，本地 `synced`：删除本地副本。
- 远端删除，本地 `modified`：重新 create。
- Legacy 协议别名规则：sync 后本地不产生 diff，也不重复刷 `last_synced_at`。

## 产品语义

### 数据模型：规则文件同步元数据

规则文件保存 `[meta.sync]` 段（`crates/bifrost-storage/src/rules.rs::RuleSyncMetadata`）：

```toml
[meta.sync]
rule_id = "rl_xxx"
status = "local_only" # local_only | synced | modified
last_synced_at = "2026-03-25T10:00:00Z"
remote_id = "123456"
remote_user_id = "u_abc"
last_synced_content_hash = "sha256:xxxx"
remote_created_at = "2026-03-25T09:00:00Z"
remote_updated_at = "2026-03-25T10:00:00Z"
```

字段说明：

- `rule_id`：本地稳定 ID，创建时生成，rename 不变。
- `status`：`local_only`（尚未绑远端）/ `synced`（上次同步一致）/ `modified`（本地有未同步改动）。
- `remote_id`：已绑定的远端对象 ID。
- `last_synced_content_hash`：上次确认同步时的本地内容 hash，用于判断本地是否变化。
- `remote_updated_at`：远端对象最近一次更新时间。

`origin_device_id` 与 `last_synced_remote_updated_at` 字段仍未落地（planned as of 2026-07-02），当前依赖 `last_synced_content_hash` + `remote_updated_at` 判断同步快照。这里不引入 `conflict`，因为策略是“直接覆盖”，不走冲突分支。

### sync-state.json 结构

`crates/bifrost-sync/src/manager.rs::SyncStateFile`：

```json
{
  "token": "...",
  "user": { "...": "..." },
  "last_sync_at": "2026-03-25T10:00:00Z",
  "last_sync_action": "bidirectional",
  "deleted_rules": {
    "rl_xxx": {
      "rule_id": "rl_xxx",
      "rule_name": "demo",
      "remote_id": "123456",
      "remote_user_id": "u_abc",
      "base_remote_updated_at": "2026-03-25T10:00:00Z",
      "base_content_hash": "sha256:xxxx",
      "deleted_at": "2026-03-25T10:01:00Z"
    }
  },
  "startup_login_prompt": {
    "prompted_at": "2026-03-25T10:00:00Z"
  }
}
```

为什么 tombstone 必须存在：

- rule 文件删掉后，活对象元数据也一起没了。
- 不保留 tombstone 就无法区分「本地明确删除」和「本地暂时还没拉到远端对象」。
- Tombstone 让多端删除意图真正落到远端，而不是被下一次拉取回滚。

## 技术细节

### 远端限制

远端服务不可修改：

- 没有 version。
- 没有 CAS。
- 不能做 `If-Match`。
- 不能做服务端冲突判断。

客户端能依赖：`remote_id` / `name` / `rule` / `create_time` / `update_time`。因此只能做保守的覆盖策略。

### 同步语义（保守但明确）

1. **删除优先**：只要某个 `rule_id` 有 tombstone → 优先删远端；未清除前不允许拉回同 `remote_id` / 同名对象。
2. **本地修改优先**：`modified` → 直接 push 覆盖远端；成功后回到 `synced`。
3. **本地未修改时接受远端覆盖**：`synced` 且远端内容变化 → 直接 pull 覆盖本地，更新同步快照。
4. **本地新建直接 create**：`local_only` → 直接 create；成功后写回 `remote_id` 和快照；同名冲突时报错让用户改名。

### 同步流程

1. 加载本地 rule 文件。
2. 加载 tombstone。
3. 拉取远端规则。
4. 优先处理 tombstone。
5. 再处理活规则。
6. 持久化状态。

#### 第一步：处理 tombstone

- 远端不再存在按 `remote_id` 或 `rule_name` 匹配的对象，且 tombstone 至少存在 `TOMBSTONE_MIN_AGE_SECS`（当前 120 秒）→ 清除 tombstone。
- 远端对象存在（`env.id == tombstone.remote_id || env.name == tombstone.rule_name`）→ 请求删除。
- 删除失败保留 tombstone，等待下次重试。
- 超过 `TOMBSTONE_MAX_AGE_SECS`（当前 7 天）自动过期清除。
- 严格收敛为“只按 `remote_id` 精确匹配”仍未落地（planned as of 2026-07-02）。

#### 第二步：处理活规则

本地存在、远端不存在：

- `local_only` → 直接 create。
- `modified` 且有 `remote_id` → 直接重新 create（保留本地修改优先语义）。
- `synced` 且有 `remote_id` → 远端被其他端删除 → **直接删除本地副本**（尊重远端删除意图）。这是解决多端删除冲突的关键。

本地存在、远端也存在：

- `modified` → 直接 update 远端。
- `synced` → 远端 hash 或更新时间变化 → pull 覆盖本地。
- `local_only`（未绑 `remote_id`）→ 直接 create；若因异常已有 `remote_id` → 按 `modified` 处理。

本地不存在、远端存在：

- 无 tombstone → 直接 pull 到本地。
- 有 tombstone → 跳过 pull，继续删远端。

### Group 同步幂等

- `crates/bifrost-admin/src/handlers/group_rules.rs::sync_envs_to_local`：远端 rule 先转成本地 canonical 内容（协议别名归一化、去掉最终换行、清理 `last_synced_at` 类同步元信息）后与已落盘规则比较；相同则不写盘、不刷 `last_synced_at`。
- 同一进程内同一 group 的落盘同步用同一把锁（`group_sync_lock_reuses_lock_for_same_group`）串行化，避免多端并发读取导致重复写。
- Group 同步不受 tombstone 影响；tombstone 只针对 My Rules 单文件规则。

## CLI + Web + Admin API

- 本方案不新增 CLI 命令；同步走后台任务与 `bifrost login` / `bifrost sync now`（现有命令）。
- Rule 编辑页顶部展示：Sync status、Created at、Updated at、Last synced at、Remote updated at。
- Sync 面板展示：pending tombstone 数量、最近一次同步时间、最近一次同步动作。
- 不展示 conflict，因为没有 conflict。
- Admin API `GET /api/rules/{name}` 返回的 `sync` 字段包含 `status` / `remote_id` / `remote_updated_at` / `last_synced_content_hash` 等；WebUI 直接消费即可。

## Sync 边界（本方案自身的自我约束）

- 同一台设备的规则文件删除 → tombstone；tombstone 存在期间禁止拉回同 remote_id / 同 name 对象。
- Group 规则和 My Rules 分别处理：Group 走 group handler + `sync_envs_to_local`，My Rules 走 `SyncManager` 主循环。
- 若同名冲突（远端已有同名对象），客户端直接报错让用户改名，不做自动合并。
- Startup 自动登录只提示一次，通过 `startup_login_prompt.prompted_at` 去重。

## Phase 划分

### Phase 1：规则文件补齐同步元数据

- rule 文件补 `rule_id`。
- rule 文件补 `last_synced_content_hash`。
- tombstone 补 `base_remote_updated_at` 与 `base_content_hash`。

### Phase 2：同步主循环收敛

- 明确 tombstone 永远优先；`modified` 永远 push；`synced` 才允许 pull。
- 本地 `synced` + 远端消失 → 删除本地副本。
- 本地 `modified` + 远端消失 → 重新 create。

### Phase 3：Group 幂等与并发

- `sync_envs_to_local` 归一化远端 env 内容后比较。
- 同一 group 加锁串行化落盘。
- 单元测试 `sync_envs_to_local_skips_unchanged_normalized_remote_rule_write`、`group_sync_lock_reuses_lock_for_same_group`。

### Phase 4：观测与真实场景验证

- UI 展示同步状态与 pending tombstone。
- Human tests 覆盖多端删除 / 多端修改 / rename / 远端删除等场景。

## 多端边界策略

### A 删除，B 修改

- A 写 tombstone，持续删远端。
- B 稍后同步会把本地修改重新推上去。
- 最终取决于谁最后一次同步成功。

### A 与 B 同时修改

- 谁后同步谁覆盖。

### A 本地删除，B 本地新建同名

- A 由 tombstone 删除旧远端对象。
- B 若创建同名规则成功则绑定新对象。
- 若因远端同名限制失败则提示用户改名。

### A rename，B edit

- rename 视为一次普通 update。
- 谁后同步谁覆盖。

### 远端被其他端删掉

- 本地 `synced` → 删除本地副本。
- 本地 `modified` → 重新 create 到远端。
- 本地也删了且有 tombstone → 保持删除语义。

### Tombstone 匹配

- 当前按 `remote_id` 或 `rule_name` 任一命中匹配远端对象。
- 同名拉取阻断：tombstone 中的 `rule_name` 阻止从远端拉回同名规则。
- 严格收敛为“只按 `remote_id` 精确匹配”以减少误删其他端同名新规则的风险（planned as of 2026-07-02）。

### Tombstone 生命周期管理

- 远端再也找不到匹配对象，且 tombstone ≥ `TOMBSTONE_MIN_AGE_SECS`（120 秒）后清除，避免在同一轮里把刚 commit 的 tombstone 立刻清掉。
- Tombstone 超过 `TOMBSTONE_MAX_AGE_SECS`（7 天）自动过期清除，避免永久累积。

## 失败处理

- **创建失败**：网络错误保留 `local_only` 下次重试；同名冲突直接报错，不自动合并。
- **更新失败**：网络错误保留 `modified`，下次继续重试。
- **删除失败**：保留 tombstone，下次继续删。
- **远端返回异常数据**：跳过本轮对象，保留本地状态，记录日志。

## 测试方案

### 单元测试（实际落地，见 `crates/bifrost-sync/src/manager.rs::tests` 与 `crates/bifrost-admin/src/handlers/group_rules.rs::tests`）

- `crates/bifrost-sync/src/manager.rs`：
  - Tombstone 过期与最小 age（`TOMBSTONE_MIN_AGE_SECS` / `TOMBSTONE_MAX_AGE_SECS` 覆盖的多个 `#[tokio::test]`）。
  - 本地 `modified` push、`synced` pull、`local_only` create 路径的 async 测试。
  - `startup_login_prompt` 去重。
- `crates/bifrost-admin/src/handlers/group_rules.rs`：
  - `group_sync_lock_reuses_lock_for_same_group`
  - `sync_envs_to_local_reports_enabled_rule_content_changes`
  - `sync_envs_to_local_reports_enabled_rule_removal`
  - `sync_envs_to_local_ignores_inactive_remote_metadata_changes`
  - `sync_envs_to_local_skips_unchanged_remote_rule_write`
  - `sync_envs_to_local_skips_unchanged_normalized_remote_rule_write`

### E2E / 真实场景

- 多端删除 / 多端修改场景应通过 `bifrost login` 后手工在两个数据目录之间切换验证；当前仓库暂无专用 `test_rule_sync_strategy.sh` 脚本（planned as of 2026-07-02）。
- Group 幂等回归由 `sync_envs_to_local_skips_unchanged_normalized_remote_rule_write` 保护，配合 `human_tests/rules-filesystem-hot-reload.md` 中的 Group 日志风暴回归条目使用。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核策略：tombstone 优先、modified push、synced 允许 pull、local_only create。
- 复核代码：tombstone 匹配逻辑、`sync_envs_to_local` 归一化、Group 锁复用。
- 跑 focused：`cargo test -p bifrost-sync -- tombstone` + `cargo test -p bifrost-admin group_sync_lock_reuses_lock_for_same_group`。

### 第 2 轮

- 复查 `rules-filesystem-hot-reload.md` 与本文档的边界一致（`RulesChanged` 事件带来源，避免 sync 写盘触发自激环）。
- 跑 `cargo test --workspace --all-features` 与两个 Group 幂等测试。
- 若仍出现同步风暴 / tombstone 死循环 / synced 未删本地副本，追加第 3 轮。

## 风险与决策

- **同名冲突不做自动合并**：让用户改名是最简单可控的语义；引入自动 rename 会牵扯 UI 提示、rule_id 迁移，得不偿失。
- **Tombstone 按 `rule_name` 命中的误伤风险**：如果其他端在同名 slot 上新建了完全不同的规则，可能被 tombstone 误删。收敛为“只按 `remote_id`”前，运营侧需要在同名场景下慎重（planned as of 2026-07-02）。
- **7 天过期是否偏长**：过短会让离线设备恢复后立即拉回已删除对象；7 天在多端离线场景下更稳。
- **Group 同步的自激环**：`RulesChanged` 事件必须带来源（`LocalApi` / `Filesystem` / `RemoteSync`）；`RemoteSync` 触发的 reload 不能反过来唤醒 Sync，否则会形成写盘 → filesystem watcher → sync 循环。这一约束由 rules-filesystem-hot-reload 模块保障。
- **不做冲突分支**：本方案的核心决策就是不引入 `conflict` 状态；后续若产品必须支持人工合并，需要重构同步状态机，不建议在当前策略上打补丁。
