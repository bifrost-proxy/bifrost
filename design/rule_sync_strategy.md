# Rule Sync Strategy

## 目标

在远端服务不可修改的前提下，为 rule 同步定义一套简单、稳定、可落地的策略，避免出现：

- 本地删除后又被远端同步回来
- 多端同时编辑后出现莫名其妙的创建/更新/删除失败
- 同名规则被误判为同一对象
- 状态越补越复杂，最后谁也说不清该覆盖谁

本方案明确选择：

- 不做冲突管理
- 不引入服务端版本协商
- 直接覆盖

换句话说，我们接受“最终一致 + 明确优先级”，不追求强一致。

## 总体原则

1. 远端只当对象存储，不承担并发协调职责。
2. 活规则的同步状态写在 rule 文件里。
3. 删除意图写在 `sync-state.json` 的 tombstone 里。
4. 同步主键优先使用 `remote_id`，不是 `name`。
5. 不做 conflict 状态，不做人工合并。
6. 同步优先级为：
   - 删除优先
   - 本地修改优先
   - 本地未修改时才接受远端覆盖

## 数据模型

### 规则文件

每个规则文件保存以下同步元数据：

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

- `rule_id`
  - 本地稳定 ID，创建时生成，rename 不变
- `status`
  - `local_only`：只有本地有，还没绑定远端
  - `synced`：上次同步后，本地与远端一致
  - `modified`：本地有未同步改动
- `remote_id`
  - 已绑定的远端对象 ID
- `last_synced_content_hash`
  - 上次确认同步时本地写回的内容 hash，用于判断本地是否变化
- `remote_updated_at`
  - 远端对象最近一次更新时间（拉取时由服务端给出）

实际字段定义见 `crates/bifrost-storage/src/rules.rs` 的 `RuleSyncMetadata`。

`origin_device_id` 与 `last_synced_remote_updated_at` 字段（planned, not yet shipped as of 2026-06-17）暂未落地，当前依赖 `last_synced_content_hash` + `remote_updated_at` 判断同步快照。

这里不引入 `conflict`，因为策略已经定为“直接覆盖”，不走冲突分支。

### sync-state.json

`sync-state.json` 保存：

1. session（token / user / last_sync_at / last_sync_action）
2. tombstone（`deleted_rules`）
3. `startup_login_prompt`：首次启动自动唤起登录的去重记录（见 `crates/bifrost-sync/src/manager.rs` 的 `SyncStateFile` 与 `StartupLoginPromptFile`）

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
  }
}
```

为什么 tombstone 还必须存在：

- rule 文件删掉后，活对象元数据也一起没了
- 不保留 tombstone，就无法区分：
  - 这是本地明确删除
  - 这是本地暂时还没拉到远端对象

## 远端限制

远端服务不可修改，意味着：

- 没有 version
- 没有 CAS
- 不能做 `If-Match`
- 不能做服务端冲突判断

所以客户端只能依赖：

- `remote_id`
- `name`
- `rule`
- `create_time`
- `update_time`

在这个前提下，客户端不能做严格并发控制，只能做保守的覆盖策略。

## 最终同步语义

### 1. 删除优先

只要某个 `rule_id` 有 tombstone：

- 优先删远端
- tombstone 未清除前，不允许把同名远端对象重新拉回本地
- tombstone 未清除前，不允许把同 `remote_id` 的对象当成新对象同步

这是为了彻底解决“本地删除后远端又被拉回来”的问题。

### 2. 本地修改优先

如果本地 rule 是 `modified`：

- 无论远端是否也更新过，都直接用本地内容覆盖远端
- update 成功后，状态回到 `synced`

这就是“不要管理冲突，直接覆盖”的落地语义。

### 3. 本地未修改时接受远端覆盖

如果本地 rule 是 `synced`，且远端内容变化：

- 直接用远端覆盖本地
- 更新本地同步快照

这意味着：

- 本地没动时，远端优先
- 本地动过时，本地优先

### 4. 本地新建直接创建远端

如果本地 rule 是 `local_only`：

- 直接 create
- 成功后写回 `remote_id` 和同步快照

如果远端因同名创建失败：

- 不做复杂冲突管理
- 当前策略直接报错给用户
- 用户改名后再创建

## 同步流程

同步必须严格按顺序处理：

1. 加载本地 rule 文件
2. 加载 tombstone
3. 拉取远端规则
4. 优先处理 tombstone
5. 再处理活规则
6. 持久化状态

### 第一步：处理 tombstone

对每个 tombstone：

1. 如果远端不再存在按 `remote_id` 或 `rule_name` 匹配的对象，且 tombstone 至少存在 `TOMBSTONE_MIN_AGE_SECS`（当前 120 秒），即可清除该 tombstone
2. 如果远端对象存在（按 `remote_id` 或同 `rule_name` 命中），直接请求删除
3. 删除失败则保留 tombstone，等待下次重试
4. 超过 `TOMBSTONE_MAX_AGE_SECS`（当前 7 天）的 tombstone 自动过期清除

注意：当前实现 tombstone 仍按 `remote_id` 或 `rule_name` 任一命中来匹配远端对象（见 `crates/bifrost-sync/src/manager.rs` 中 `env.id == tombstone.remote_id || env.name == tombstone.rule_name`）。严格收敛为只按 `remote_id` 精确匹配（planned, not yet shipped as of 2026-06-17）。

也就是说：

- 只要 tombstone 在，就持续按 `remote_id` 或同名匹配并删除目标远端对象
- 直到删成功、远端再也找不到匹配对象（且 tombstone 满 120 秒）、或 7 天过期

### 第二步：处理活规则

#### 本地存在，远端不存在

- `status = local_only`
  - 直接 create 到远端
- `status = modified` 且已有 `remote_id`
  - 本地有未同步修改，远端对象消失
  - 直接重新 create（保留本地修改优先语义）
- `status = synced` 且已有 `remote_id`
  - 远端对象消失，说明被其他端删除
  - **直接删除本地副本**（尊重远端删除意图）
  - 这是解决多端删除冲突的关键：synced 状态表示本地未修改，远端消失即意味着应同步删除

#### 本地存在，远端也存在

- `status = modified`
  - 直接 update 远端
- `status = synced`
  - 如果远端 hash 或更新时间变化，直接 pull 覆盖本地
  - 否则不动
- `status = local_only`
  - 若未绑定 `remote_id`，优先 create
  - 若因异常已有 `remote_id`，按 `modified` 处理，直接 update

#### 本地不存在，远端存在

- 若无 tombstone
  - 直接 pull 到本地
- 若有 tombstone
  - 跳过 pull，继续删远端

## 多端边界策略

### A 删除，B 修改

策略：

- A 写 tombstone，持续删远端
- B 若稍后同步，会把本地修改重新推上去
- 最终结果取决于谁最后一次同步成功

这是“直接覆盖”策略下的自然结果，不做额外冲突治理。

### A 与 B 同时修改

策略：

- 谁后同步，谁覆盖
- 不做人工干预

### A 本地删除，B 本地新建同名

策略：

- A 由 tombstone 删除旧远端对象
- B 若创建同名规则成功，则绑定新对象
- 若因远端同名限制失败，则提示用户改名

### A rename，B edit

策略：

- rename 视为一次普通 update
- 谁后同步，谁覆盖

### 远端被其他端删掉

策略：

- 若本地是 `synced` 状态（未修改），直接删除本地副本
- 若本地是 `modified` 状态（有未同步修改），重新 create 到远端
- 若本地也删了且有 tombstone，继续保持删除语义

### Tombstone 匹配

当前实现：

- Tombstone 按 `remote_id` 或 `rule_name` 任一命中来匹配远端对象
- 同名拉取阻断：tombstone 中的 `rule_name` 会阻止从远端拉回同名规则
- 未来计划（planned, not yet shipped as of 2026-06-17）：进一步收敛为只按 `remote_id` 匹配以减少误删其他端同名新规则的风险

### Tombstone 生命周期管理

策略：

- 远端再也找不到匹配对象，且 tombstone 满 `TOMBSTONE_MIN_AGE_SECS`（120 秒）后清除，避免在同一轮里把刚 commit 的 tombstone 立刻清掉
- Tombstone 超过 `TOMBSTONE_MAX_AGE_SECS`（7 天）自动过期清除，避免永久累积
- 远端删除成功后下一轮即可被清理（仍受 min age 约束）

## 失败处理

### 创建失败

- 常见原因：同名冲突、网络错误
- 处理：
  - 网络错误：保留 `local_only`，下次重试
  - 同名冲突：直接报错，不做自动合并

### 更新失败

- 网络错误：保留 `modified`
- 下次继续重试

### 删除失败

- 保留 tombstone
- 下次继续删

### 远端返回异常数据

- 跳过本轮对象
- 保留本地状态
- 记录日志

## UI 建议

rule 编辑页顶部展示：

- Sync status
- Created at
- Updated at
- Last synced at
- Remote updated at

Sync 面板展示：

- pending tombstone 数量
- 最近一次同步时间
- 最近一次同步动作

不展示 conflict，因为没有 conflict。

## 最小落地顺序

建议按这个顺序做：

1. rule 文件补 `rule_id`
2. rule 文件补 `last_synced_content_hash`
3. tombstone 补 `base_remote_updated_at` 和 `base_content_hash`
4. 同步逻辑明确成：
   - tombstone 永远优先
   - modified 永远 push
   - synced 才允许 pull

## 最终结论

在远端不可改、且你明确要求“不做冲突管理，直接覆盖”的前提下，最佳策略就是：

- 删除优先
- 本地修改优先
- 本地未修改才接受远端覆盖
- tombstone 持续重试删除远端
- 所有并发问题都用“最后一次成功同步覆盖”解决

这不是最强一致的方案，但它是当前约束下最简单、最稳定、最不容易出现“莫名其妙卡死状态”的方案。
