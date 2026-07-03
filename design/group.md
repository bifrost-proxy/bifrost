# Group 功能模块设计

## 背景

Bifrost V4 引入 Group（小组）作为核心协作功能，允许用户创建/加入小组，在 Group 维度下共享和管理规则。当前 Bifrost 架构由 Rust 后端 + React 前端 + bifrost-sync-server 同步服务组成，Group 需要跨这三层落地：

- 后端 `bifrost-sync-server` 是集中式协作服务，承担用户、Group、成员、规则（env 表）持久化与鉴权。
- 本机 Bifrost 通过 `bifrost-admin` HTTP handler 把 Group 相关 API 代理到 sync-server，同时把 Group 规则内容以子目录形式落到本机 `data_dir/rules/<group-name>/` 供代理 runtime 使用。
- Web 前端在同步服务已配置且用户登录后展示 Groups 入口、Group 详情页、规则页顶部的 Group 切换器。
- CLI 通过 admin HTTP API 暴露 `bifrost group …` 命令，方便脚本化与远程运维。

本文档描述 Group 端到端的数据模型、API、本地存储、Rust crate 拆分、Web 组件、CLI 命令、权限矩阵、同步与写盘策略，以及测试与校验闭环。

## 用户目标验证清单

### 必须实现

- 用户可以创建、编辑、删除、搜索 Group，并区分 Public / Private 可见性。
- 用户可以邀请成员、移除成员、修改成员角色（Owner / Master / User）。
- 每个 Group 拥有独立规则集，内容落到本机 `rules/<group-name>/` 子目录。
- 规则页顶部提供切换器，可以在自己的规则和 Group 规则之间切换。
- Public Group 所有人可搜索/查看；Private Group 仅成员可搜索/查看；成员列表始终仅成员可见。
- 未配置同步服务或未登录时隐藏 Groups 入口与切换器。
- Group 规则列表刷新不得触发无变化写盘或“save → watcher → reload → 再拉列表”热加载循环。

### 必须不破坏

- 本地自建规则的 CRUD、启停、排序、导入导出继续工作。
- Rules watcher / hot reload 语义保持一致，只在真实内容或远端 metadata 变化时通知运行时刷新。
- `bifrost-sync-server` 上除 Group 外的既有 API（用户、鉴权、env、room）保持向后兼容。
- 全局 IP 限流（默认 `rate_limit_per_ip=200`，登录 `auth_rate_limit_per_ip=20`）保持不变；本地 E2E 通过临时提高到 1000 绕过，不改动生产默认。
- 已存在的 Default 全局默认规则保护语义不受本次影响。

### 必须真实验证

- CLI 走真实 admin HTTP API 完成 Group 与规则 CRUD。
- E2E 覆盖 sync-server 与 admin 之间的转发。
- 真实浏览器验证 Groups 列表、详情、成员管理与规则切换。

## 产品语义

### 数据模型

```typescript
interface Group {
  id: string;
  name: string;
  avatar?: string;             // 随机头像色值
  description?: string;
  level?: GroupUserLevel;      // 当前用户在小组中的角色
  visibility: GroupVisibility; // 公开/私有
  create_time: string;
  update_time: string;
}

enum GroupVisibility { Public = 'public', Private = 'private' }
enum GroupUserLevel { User = 0, Master = 1, Owner = 2 }

interface GroupMember {
  id: string;
  group_id: string;
  user_id: string;
  level: GroupUserLevel;
  create_time: string;
  update_time: string;
}

interface GroupSetting {
  rules_enabled: boolean;       // 是否开启 Group 规则
  visibility: GroupVisibility;
}
```

### 权限矩阵

Group 可见性：

| 可见性 | 搜索 | 查看详情 | 查看成员 |
|--------|------|----------|----------|
| Public | 所有人 | 所有人 | 仅成员 |
| Private | 仅成员 | 仅成员 | 仅成员 |

操作权限：

| 操作 | User | Master | Owner |
|------|------|--------|-------|
| 查看/使用 Group 规则 | 允许 | 允许 | 允许 |
| 编辑 Group 信息 | 拒绝 | 允许 | 允许 |
| 管理 Group 规则 | 拒绝 | 允许 | 允许 |
| 邀请/移除成员 | 拒绝 | 允许 | 允许 |
| 管理成员角色 | 拒绝 | 允许 | 允许 |
| 修改 Group 设置 | 拒绝 | 允许 | 允许 |
| 删除 Group | 拒绝 | 拒绝 | 允许 |

## 技术细节

### 架构与数据流

```
[Web Frontend] <-> [bifrost-admin API (proxy)] <-> [bifrost-sync-server] <-> [SQLite/MySQL]
```

Group 数据存储在 bifrost-sync-server 端，bifrost-admin 作为代理转发。sync-server 对非 Remote Invoke 路径保留全局 IP 限流；完整 Group E2E 会在一分钟内跑完 CRUD + 成员 + 规则 + 代理 + CLI，通过 `--rate-limit-per-ip 1000 --auth-rate-limit-per-ip 1000` 临时放宽，不影响生产默认值。

### 本地存储布局

```
data_dir/
├── rules/
│   ├── my-rule-1.bifrost           # 用户自己的规则
│   ├── my-rule-2.bifrost
│   ├── .group_cache.json           # group_id <-> group_name 映射缓存
│   └── <group-name>/               # Group 规则子目录（按 group name 命名）
│       ├── group-rule-1.bifrost
│       └── group-rule-2.bifrost
```

实际实现没有额外的 `groups/` 中间目录：admin handler 直接在 `state.rules_storage.base_dir()` 下用 `sanitize_group_dir_name(<group_name>)` 创建子目录，通过同目录 `.group_cache.json` 维护 group_id 与目录名（group name）的双向映射，孤儿目录在每次列表请求后由 `cleanup_orphaned_group_dirs` 清理。

### Group 规则本地同步写盘策略

Group 规则列表接口会从远端拉取 env 并同步到本地子目录，必须避免无变化写盘：

- 如果远端 env 的规则内容、远端 ID、远端创建/更新时间、group 名、本地 enabled 状态和本地 `rule_id` 均未变化，不得更新 `.bifrost` 文件。
- `last_synced_at` 只表示本地最近一次实际同步写入时间，不能作为每次列表刷新都写盘的理由。
- 同步已有规则时必须复用本地已有 `rule_id`，避免纯列表刷新制造新的本地规则身份。
- 只有实际内容或远端 metadata 变化时才保存文件；其中 enabled 规则内容变化需要通知运行时刷新 active rules 与 Badge cache。
- `RulesStorage::save` 对生成内容与磁盘内容完全一致的保存请求应直接返回，避免无意义修改 mtime 触发文件系统 watcher。

该策略切断 `GET /api/group-rules/:group` 或 WebUI 列表刷新触发的 `save -> filesystem watcher -> rules reload -> 再次列表/同步` 热加载循环，同时保留真实远端更新的本地落盘能力。

### bifrost-sync-server 扩展

数据库表：

**groups**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | TEXT PK | UUID |
| name | TEXT | 小组名称 |
| avatar | TEXT | 头像色值 |
| description | TEXT | 描述 |
| visibility | TEXT | public/private |
| created_by | TEXT | 创建者 user_id |
| create_time | TEXT | 创建时间 |
| update_time | TEXT | 更新时间 |

**group_members**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | TEXT PK | UUID |
| group_id | TEXT FK | 关联 groups.id |
| user_id | TEXT | 用户 ID |
| level | INTEGER | 0=User, 1=Master, 2=Owner |
| create_time | TEXT | 创建时间 |
| update_time | TEXT | 更新时间 |

**group_settings**

| 字段 | 类型 | 说明 |
|------|------|------|
| group_id | TEXT PK FK | 关联 groups.id |
| rules_enabled | INTEGER | 是否启用规则 |
| visibility | TEXT | public/private |

sync-server 实际暴露的 Group 相关接口（admin 端通过 `sync_manager.proxy_forward` 转发）：

| 方法 | 路径 | 说明 | 鉴权 |
|------|------|------|------|
| POST | /v4/group | 创建 Group | Token |
| GET | /v4/group | 搜索/列出 Group | Token |
| GET | /v4/group/:id | 获取 Group 详情（visibility 由 admin 从 setting 补齐） | Token + 权限 |
| PATCH | /v4/group/:id | 更新 Group | Token + Master/Owner |
| DELETE | /v4/group/:id | 删除 Group | Token + Owner |
| GET | /v4/group/:id/setting | 获取 Group 设置 | Token + Master/Owner |
| PATCH | /v4/group/:id/setting | 更新 Group 设置 | Token + Master/Owner |
| POST | /v4/group/invite | 邀请成员 | Token + Master/Owner |
| GET | /v4/room?group_id=… | 获取成员列表（room 表） | Token + 成员 |
| GET | /v4/env?group_id=… | 列出 Group 规则（env 表） | Token + 成员 |
| POST | /v4/env | 创建 Group 规则 | Token + Master/Owner |
| PATCH | /v4/env/:env_id | 更新 Group 规则 | Token + Master/Owner |
| DELETE | /v4/env/:env_id | 删除 Group 规则 | Token + Master/Owner |

> 成员管理与规则管理复用既有 `/v4/room` 和 `/v4/env` 资源，不是 `/v4/group/:id/members` / `/v4/group/:id/envs` 这样的嵌套子资源。`/v4/group/:id/member/:user_id`、`/v4/group/:id/leave` 等嵌套路径为（planned, not yet shipped as of 2026-06-16），admin/CLI 暂时通过 room/env 接口组合达成等价效果。

DAO 接口（TypeScript / sync-server 端）：

```typescript
interface IGroupDao {
  create(req: CreateGroupReq): Promise<Group>;
  findById(id: string): Promise<Group | undefined>;
  update(id: string, fields: UpdateGroupReq): Promise<Group | undefined>;
  delete(id: string): Promise<boolean>;
  search(query: SearchGroupQuery): Promise<{ list: Group[]; total: number }>;
}

interface IGroupMemberDao {
  add(groupId: string, userId: string, level: number): Promise<GroupMember>;
  remove(groupId: string, userId: string): Promise<boolean>;
  updateLevel(groupId: string, userId: string, level: number): Promise<boolean>;
  findByGroupAndUser(groupId: string, userId: string): Promise<GroupMember | undefined>;
  listByGroup(groupId: string, query?: { keyword?: string; offset?: number; limit?: number }): Promise<{ list: GroupMember[]; total: number }>;
  listByUser(userId: string): Promise<GroupMember[]>;
}

interface IGroupSettingDao {
  get(groupId: string): Promise<GroupSetting>;
  update(groupId: string, fields: Partial<GroupSetting>): Promise<void>;
}
```

### bifrost-sync crate (Rust) 扩展

`crates/bifrost-sync/src/types.rs` 新增：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGroup {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub avatar: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub description: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub visibility: String,
    pub level: Option<i32>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default, deserialize_with = "nullable_string")]
    pub create_time: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub update_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGroupMember {
    pub id: String,
    pub group_id: String,
    pub user_id: String,
    pub level: i32,
    #[serde(default, deserialize_with = "nullable_string")]
    pub nickname: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub avatar: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub email: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub create_time: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub update_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGroupSetting {
    pub group_id: String,
    #[serde(default)]
    pub rules_enabled: bool,
    #[serde(default)]
    pub visibility: String,
}
```

同文件导出 `CreateGroupReq`、`UpdateGroupReq`、`InviteGroupReq`、`UpdateGroupSettingReq`、`GroupListResponse`、`GroupMemberListResponse` 等请求/响应类型。`SyncHttpClient` 增加 Group 相关 API 调用方法。

### bifrost-admin 扩展

`crates/bifrost-admin/src/handlers/group.rs` 以代理层身份将 `/api/group…` 重写为 `/v4/group…` 并经 `SyncManager::proxy_forward` 转发；同时承担副作用：

- 列表结果中的 `id <-> name` 写入 `state.group_name_cache()` 并持久化到 `rules/.group_cache.json`。
- 列表请求触发 `cleanup_orphaned_group_dirs`：本地有目录但远端已不存在的 group 被禁用并删除子目录，必要时通知 rules 重载。
- 单个 group 详情若缺少 `visibility` 字段，再向 `/v4/group/:id/setting` 拉一次 setting，根据 `level` 字段补齐 visibility（0=private、1=public）后返回。

`crates/bifrost-admin/src/handlers/group_rules.rs` 独立于通用 group 代理之外，负责把本地 Bifrost 规则语义映射到远端 env/room 资源，统一暴露 `/api/group-rules/…`：

- `GET /api/group-rules/:group_ref`：先 `resolve_group_ref` 把 id 或 name 归一化，再按需调用 `/v4/group/:id`、`/v4/room`、`/v4/env`、`/v4/user/peer` 拉到该 group 的 env 列表，落盘到本地 `rules/<group-name>/` 子目录，并返回 `{group_id, group_name, writable, rules:[…]}`。
- `GET /api/group-rules/:group_ref/:name`：从本地 `RulesStorage` 读规则详情。
- `POST /api/group-rules/:group_ref` / `PUT /api/group-rules/:group_ref/:name`：先在本地 `RulesStorage` 保存或更新规则，再 `POST /v4/env` 或 `PATCH /v4/env/:remote_id` 同步到远端；远端失败时本地仍保留。
- `DELETE /api/group-rules/:group_ref/:name`：删除本地规则文件并 `DELETE /v4/env/:remote_id`。
- `PUT /api/group-rules/:group_ref/:name/enable` / `.../disable`：仅切换本地 enabled 状态（`RulesStorage::set_enabled`），不动远端。

`router.rs` 中 `/api/group-rules` 前缀必须先匹配并交给 `handle_group_rules`，再让 `/api/group` 前缀走 `handle_group`，避免前缀冲突。

### bifrost-storage 扩展

- `RulesStorage` 继续暴露 `with_dir(dir)` 和 `base_dir()`，admin handler 直接以 `state.rules_storage.base_dir().join(sanitize_group_dir_name(group_name))` 构造 Group 子目录并通过 `RulesStorage::with_dir` 复用同一套读写逻辑，没有 `group_storage()` / `list_groups()` 专门 API。
- `RuleFile` 在 `crates/bifrost-storage/src/rules.rs` 增加 `group: Option<String>` 字段，并配套 `with_group()` 构造与 `rule_reference_key` 拼接 `<group>/<name>` 作为规则引用键。
- 顶层 `load_all` 在迭代规则目录时识别已知 group 子目录，把其中的规则带 `group` 元信息一并返回；非 group 的孤立子目录会被跳过（参见 `test_load_enabled_with_subdirs_keeps_group_directories`）。
- group_id <-> group_name 的映射不在 `RulesStorage` 内部，而是放在 admin state 的 `group_name_cache` 中，持久化到 `rules/.group_cache.json`。

如果未来要把 group 子目录管理收回 storage 层（独立的 `group_storage()` / `list_groups()` API），相关接口仍属于（planned, not yet shipped as of 2026-06-16）。

## CLI / Web / Admin API

### Web 前端

路由：

```
/groups           - Group 列表页
/groups/:id       - Group 详情页（含成员管理）
```

`IconSidebar` 新增 Group 导航项，仅在同步服务已配置且用户已登录时展示。Rules 页面顶部添加 Group 选择器：默认选中 “My Rules”，列出用户加入的所有 Group，切换后加载对应 Group 规则列表。

新增 `web/src/api/group.ts`：

```typescript
createGroup(req): Promise<Group>
searchGroups(query?): Promise<GroupList>
getGroup(id): Promise<Group>
updateGroup(id, req): Promise<void>
deleteGroup(id): Promise<void>

getGroupMembers(id, query?): Promise<MemberList>
inviteMembers(id, userIds, level?): Promise<void>
removeMember(groupId, userId): Promise<void>
updateMemberLevel(groupId, userId, level): Promise<void>
leaveGroup(id): Promise<void>

getGroupSetting(id): Promise<GroupSetting>
updateGroupSetting(id, req): Promise<void>

getGroupEnvs(id): Promise<EnvList>
createGroupEnv(id, req): Promise<Env>
updateGroupEnv(id, envId, req): Promise<void>
deleteGroupEnv(id, envId): Promise<void>
```

Store 层新增 `web/src/stores/useGroupStore.ts`（Zustand）。页面组件实际落地：

```
pages/Groups/
├── index.tsx              # 列表页：搜索、我管理的 Group、我加入的 Group；内含 GroupCardGrid
└── GroupDetail.tsx        # 详情页（含成员列表与 Group 信息）
```

规则页 Group 切换器、创建/编辑弹窗、搜索 Group 等独立子组件（`CreateGroupModal.tsx`、`SearchGroup.tsx`、`GroupRulesSwitcher.tsx`）为（planned, not yet shipped as of 2026-06-16）。

### CLI (`bifrost-cli`)

`crates/bifrost-cli/src/commands/group.rs` 通过 admin API（HTTP）实现，需要代理运行中：

| 命令 | 对应 API | 说明 |
|------|----------|------|
| `bifrost group list [-k keyword] [-l limit]` | `GET /api/group?keyword=&offset=0&limit=` | 列出/搜索 groups |
| `bifrost group show <group_id>` | `GET /api/group/{group_id}` | 查看 group 详情 |
| `bifrost group rule list <group_id>` | `GET /api/group-rules/{group_id}` | 列出 group 规则 |
| `bifrost group rule show <group_id> <name>` | `GET /api/group-rules/{group_id}/{name}` | 查看 group rule 详情 |
| `bifrost group rule add <group_id> <name> [-c\|-f]` | `POST /api/group-rules/{group_id}` | 创建 group rule |
| `bifrost group rule update <group_id> <name> [-c\|-f]` | `PUT /api/group-rules/{group_id}/{name}` | 更新 group rule |
| `bifrost group rule delete <group_id> <name>` | `DELETE /api/group-rules/{group_id}/{name}` | 删除 group rule |
| `bifrost group rule enable <group_id> <name>` | `PUT /api/group-rules/{group_id}/{name}/enable` | 启用 group rule |
| `bifrost group rule disable <group_id> <name>` | `PUT /api/group-rules/{group_id}/{name}/disable` | 禁用 group rule |

## Sync 边界

- Group 数据（groups / group_members / group_settings / env）由 sync-server 集中持久化；Bifrost 本机只缓存 group_id <-> group_name 映射与 Group 规则内容。
- Group 规则同步不允许覆盖用户本地 enabled 状态；`enabled` 是本地开关，`rule_id` 是本地身份。
- `Default` 系统保留规则不属于任何 Group，也不参与 Group 规则同步（详见 `default-global-rule.md`）。
- 未配置同步服务时，Group 相关 CLI 命令与 admin API 返回明确错误码 `SYNC_NOT_CONFIGURED`，前端隐藏入口。

## 实现切分

### Phase 1：sync-server 与 sync crate 数据模型

- 建表 `groups` / `group_members` / `group_settings`。
- 暴露 `/v4/group…` 系列 API。
- Rust `RemoteGroup` / `RemoteGroupMember` / `RemoteGroupSetting` 与 `SyncHttpClient` 方法。

### Phase 2：admin handler 与本地存储

- `handlers/group.rs` 代理转发 + 缓存持久化 + 孤儿清理。
- `handlers/group_rules.rs` 本地规则子目录 CRUD 与远端 env 同步。
- `RuleFile.group` 字段、`rule_reference_key`、子目录 load。

### Phase 3：Web 前端与 CLI

- Groups 列表 / 详情 / 成员管理页面与 store。
- Rules 页 Group 切换器 MVP（若属于 planned，先做“列表 + 详情 + CRUD”，切换器留 phase 4）。
- `bifrost group` CLI 命令。

### Phase 4：可观测性与文档

- 更新 README、ADMIN_API、CLI docs、SKILL 索引。
- 增加 human_tests Group 用例。
- 增加 rate-limit 临时放宽脚本与开发环境备注。

## 测试方案

### 单元测试

- bifrost-sync-server：Group DAO 层测试（SQLite）。
- bifrost-storage：Group 规则子文件夹存储测试（`test_load_enabled_with_subdirs_keeps_group_directories` 等）。
- bifrost-sync：Group 类型序列化 / nullable_string 兼容测试。
- bifrost-cli：Group CLI 命令测试（33 个测试，使用 mock HTTP server 覆盖全部命令）。mock HTTP server 必须串行持有测试锁，避免 lib/bin 两套 group 测试在 workspace 并发运行时共享 HTTP mock 时序导致相邻 add/update 用例偶发拿不到预期响应。
- bifrost-admin：`handlers/group_rules.rs` 单元覆盖 `resolve_group_ref` / `cleanup_orphaned_group_dirs` / 写盘幂等。

### E2E 测试

- 通过 bifrost-admin API 覆盖完整 Group CRUD 流程。
- 覆盖 Group 规则同步：写盘幂等、无变化不 rebuild watcher。
- 覆盖权限矩阵：Owner/Master/User 三种角色的允许/拒绝分支。
- `e2e-tests/tests/test_group_sync_no_logstorm_e2e.sh`：验证 group-rules 列表刷新不引发热加载风暴。

### 真实场景测试

新增 `human_tests/groups.md`：

- TC-GRP-01：登录后 Groups 入口出现；未登录/未配置同步隐藏。
- TC-GRP-02：创建 Public/Private Group；搜索可见性符合权限矩阵。
- TC-GRP-03：邀请 / 移除 / 改角色成员。
- TC-GRP-04：Group 规则 CRUD 与规则页切换器。
- TC-GRP-05：Group 规则本地落盘幂等；rules watcher 不触发风暴。
- TC-GRP-06：CLI `bifrost group …` 全命令覆盖。

所有服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-storage rules -- --nocapture`
- `cargo test -p bifrost-admin group -- --nocapture`
- `cargo test -p bifrost-cli group -- --nocapture`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定下豁免 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：Group CRUD、成员管理、规则子目录、切换器、可见性权限、写盘幂等、未登录隐藏。
- 复核 diff：admin handler、router 前缀顺序、storage 子目录识别、sync crate 类型、CLI 命令 33 个测试。
- 重点 review：`.group_cache.json` 并发写；孤儿目录清理是否会误删活跃 Group；rate-limit 是否只在测试脚本临时放宽。
- 复测：单元 + E2E + human_tests 关键用例。

### 第 2 轮

- 复查第 1 轮修复的最新 diff。
- 重点 review：真实浏览器登录/退出切换时 sidebar 与 store 是否残留；规则页切换器切换后是否触发额外 fetch；权限拒绝分支的错误消息是否稳定可测试。
- 复测：全量 workspace test；用真实同步服务过一次端到端。

## 风险与决策点

- **Rate-limit 与 E2E**：E2E 通过临时放宽 rate-limit 通过 CI；生产默认不改，避免降低鉴权强度。
- **孤儿目录清理**：只在 group 列表接口触发；若用户手动删除 sync-server 上的 Group，本地目录会被清理并触发 watcher。文档需提示用户先在客户端 leave/delete 而不是手动删目录。
- **本地 rule_id 复用**：规则同步严格复用现有 rule_id，避免每次列表刷新都造出新身份；命中冲突时以远端 env_id 为主键。
- **切换器 planned**：MVP 阶段可以先只在 URL query 里带 group_id 切换，待切换器组件独立拆分后升级为 UI 顶栏。
- **多设备一致性**：Group 规则最终一致；本机写入远端后本地立即落盘；远端后续下发时若 metadata 不变不触发写盘。
- **命名冲突**：`sanitize_group_dir_name` 保证目录名安全；若两个 Group 因清洗后重名，第二个附加短 hash 后缀，映射记录在 `.group_cache.json`。
