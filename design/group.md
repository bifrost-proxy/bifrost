# Group 功能模块设计

## 功能概述

Group（小组）是 Bifrost V4 的核心协作功能，允许用户创建和加入小组，在小组维度下共享和管理规则。本设计将 V4 的 Group 概念引入当前 Bifrost 架构，适配当前的 Rust 后端 + React 前端 + bifrost-sync-server 同步服务的技术栈。

### 核心特性

1. **Group 管理**：创建、编辑、删除、搜索小组
2. **成员管理**：邀请成员、移除成员、角色管理（Owner/Master/User）
3. **Group 规则**：每个 Group 可以拥有独立的规则集，本地存储在 Rules 子文件夹中
4. **规则切换**：在规则页面顶部提供切换器，可在自己的规则和 Group 规则之间切换
5. **权限控制**：公开 Group 所有人可见，私有 Group 仅成员可查看
6. **条件展示**：仅当配置了同步服务且已登录时，才展示 Group 相关 UI 入口

## 数据模型

### Group

```typescript
interface Group {
  id: string;
  name: string;
  avatar?: string;         // 随机头像色值
  description?: string;    // 小组描述
  level?: GroupUserLevel;  // 当前用户在小组中的角色
  visibility: GroupVisibility; // 公开/私有
  create_time: string;
  update_time: string;
}

enum GroupVisibility {
  Public = 'public',   // 所有人可见
  Private = 'private', // 仅成员可见
}

enum GroupUserLevel {
  User = 0,    // 普通成员
  Master = 1,  // 管理员
  Owner = 2,   // 创建者
}
```

### GroupMember (Room)

```typescript
interface GroupMember {
  id: string;
  group_id: string;
  user_id: string;
  level: GroupUserLevel;
  create_time: string;
  update_time: string;
}
```

### GroupSetting

```typescript
interface GroupSetting {
  rules_enabled: boolean;       // 是否开启 Group 规则
  visibility: GroupVisibility;  // 公开/私有
}
```

## 架构设计

### 数据流

```
[Web Frontend] <-> [bifrost-admin API (proxy)] <-> [bifrost-sync-server (数据存储)]
                                                          |
                                                    [SQLite/MySQL]
```

Group 数据存储在 bifrost-sync-server 端，bifrost-admin 作为代理将请求转发到同步服务。

bifrost-sync-server 对非 Remote Invoke 路径保留全局 IP 限流，默认 `server.rate_limit_per_ip = 200`（每分钟），登录/注册路径还有独立的 `server.auth_rate_limit_per_ip = 20`。完整 Group E2E 这类本地 mock 链路会在一分钟内覆盖 CRUD、成员、规则、代理转发和 CLI 操作，测试脚本可以通过 `--rate-limit-per-ip 1000 --auth-rate-limit-per-ip 1000` 临时放宽测试服务配额；生产默认值不变。

### 本地规则存储

```
data_dir/
├── rules/
│   ├── my-rule-1.bifrost           # 用户自己的规则
│   ├── my-rule-2.bifrost
│   ├── .group_cache.json            # group_id <-> group_name 映射缓存
│   └── <group-name>/                # Group 规则子目录（按 group name 命名）
│       ├── group-rule-1.bifrost
│       └── group-rule-2.bifrost
```

实际实现中没有额外的 `groups/` 中间目录：admin handler 直接在 `state.rules_storage.base_dir()` 下用 `sanitize_group_dir_name(<group_name>)` 创建子目录，再通过同目录下的 `.group_cache.json` 维护 group_id 与目录名（group name）的双向映射，孤儿目录在每次列表请求后由 `cleanup_orphaned_group_dirs` 清理。

### Group 规则本地同步写盘策略

Group 规则列表接口会从远端拉取 env 并同步到本地 `rules/<group-name>/` 子目录。该同步路径必须避免无变化写盘：

- 如果远端 env 的规则内容、远端 ID、远端创建/更新时间、group 名、本地 enabled 状态和本地 `rule_id` 均未变化，不得更新 `.bifrost` 文件。
- `last_synced_at` 只表示本地最近一次实际同步写入时间，不能作为每次列表刷新都写盘的理由。
- 同步已有规则时必须复用本地已有 `rule_id`，避免纯列表刷新制造新的本地规则身份。
- 只有实际内容或远端 metadata 变化时才保存文件；其中 enabled 规则内容变化需要通知运行时刷新 active rules 与 Badge cache。
- `RulesStorage::save` 对生成内容与磁盘内容完全一致的保存请求应直接返回，避免无意义修改 mtime 触发文件系统 watcher。

该策略用于切断 `GET /api/group-rules/:group` 或 WebUI 列表刷新触发的 `save -> filesystem watcher -> rules reload -> 再次列表/同步` 热加载循环，同时保留真实远端更新的本地落盘能力。

## 实现方案

### 1. bifrost-sync-server 扩展

#### 1.1 数据库表

**groups 表**

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

**group_members 表**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | TEXT PK | UUID |
| group_id | TEXT FK | 关联 groups.id |
| user_id | TEXT | 用户 ID |
| level | INTEGER | 0=User, 1=Master, 2=Owner |
| create_time | TEXT | 创建时间 |
| update_time | TEXT | 更新时间 |

**group_settings 表**

| 字段 | 类型 | 说明 |
|------|------|------|
| group_id | TEXT PK FK | 关联 groups.id |
| rules_enabled | INTEGER | 是否启用规则 |
| visibility | TEXT | public/private |

#### 1.2 API 路由

bifrost-sync-server 上实际暴露的 Group 相关接口（admin 端通过 `sync_manager.proxy_forward` 转发到这些路径）：

| 方法 | 路径 | 说明 | 鉴权 |
|------|------|------|------|
| POST | /v4/group | 创建 Group | Token |
| GET | /v4/group | 搜索/列出 Group | Token |
| GET | /v4/group/:id | 获取 Group 详情（visibility 由 admin 从 setting 补齐） | Token + 权限检查 |
| PATCH | /v4/group/:id | 更新 Group | Token + Master/Owner |
| DELETE | /v4/group/:id | 删除 Group | Token + Owner |
| GET | /v4/group/:id/setting | 获取 Group 设置 | Token + Master/Owner |
| PATCH | /v4/group/:id/setting | 更新 Group 设置 | Token + Master/Owner |
| POST | /v4/group/invite | 邀请成员 | Token + Master/Owner |
| GET | /v4/room?group_id=... | 获取成员列表（room 表） | Token + 成员 |
| GET | /v4/env?group_id=... | 列出 Group 规则（env 表） | Token + 成员 |
| POST | /v4/env | 创建 Group 规则 | Token + Master/Owner |
| PATCH | /v4/env/:env_id | 更新 Group 规则 | Token + Master/Owner |
| DELETE | /v4/env/:env_id | 删除 Group 规则 | Token + Master/Owner |

> 注意：成员管理与规则管理在远端复用了既有的 `/v4/room` 和 `/v4/env` 资源，并非 `/v4/group/:id/members` / `/v4/group/:id/envs` 这样的嵌套子资源。`/v4/group/:id/member/:user_id`、`/v4/group/:id/leave` 等嵌套路径（planned, not yet shipped as of 2026-06-16）尚未在当前 bifrost-sync-server 上实现，admin/CLI 暂时通过 room/env 接口组合达成等价效果。

#### 1.3 DAO 接口

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

### 2. bifrost-sync crate（Rust）扩展

#### 2.1 新增 Group 类型

在 `crates/bifrost-sync/src/types.rs` 中添加：

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

同文件还导出 `CreateGroupReq`、`UpdateGroupReq`、`InviteGroupReq`、`UpdateGroupSettingReq`、`GroupListResponse`、`GroupMemberListResponse` 等请求/响应类型。

#### 2.2 新增 Group HTTP 客户端方法

在 `SyncHttpClient` 中添加 Group 相关 API 调用方法。

### 3. bifrost-admin 扩展

#### 3.1 Group handler

`crates/bifrost-admin/src/handlers/group.rs`

以代理层身份将 `/api/group...` 请求重写为 `/v4/group...` 并经 `SyncManager::proxy_forward` 转发；同时承担以下副作用：

- 列表请求结果中的 `id <-> name` 写入 `state.group_name_cache()` 并持久化到 `rules/.group_cache.json`。
- 列表请求触发 `cleanup_orphaned_group_dirs`：本地有目录但远端已不存在的 group 被禁用并删除子目录，必要时通知 rules 重载。
- 单个 group 详情若缺少 `visibility` 字段，再向 `/v4/group/:id/setting` 拉一次 setting，根据 `level` 字段补齐 visibility（0=private、1=public）后返回。

#### 3.2 Group rules handler

`crates/bifrost-admin/src/handlers/group_rules.rs`

独立于通用 group 代理之外，负责把本地 Bifrost 规则语义映射到远端 env/room 资源，统一暴露 `/api/group-rules/...` 这套与本地规则一致的 API。主要职责：

- `GET /api/group-rules/:group_ref`：先 `resolve_group_ref` 把 id 或 name 归一化，再按需调用 `/v4/group/:id`、`/v4/room`、`/v4/env`、`/v4/user/peer` 拉到该 group 的 env 列表，落盘到本地 `rules/<group-name>/` 子目录，并返回 `{group_id, group_name, writable, rules:[...]}`。
- `GET /api/group-rules/:group_ref/:name`：从本地 `RulesStorage` 读规则详情。
- `POST /api/group-rules/:group_ref` / `PUT /api/group-rules/:group_ref/:name`：先在本地 `RulesStorage` 保存或更新规则，再 `POST /v4/env` 或 `PATCH /v4/env/:remote_id` 同步到远端；远端失败时本地仍保留。
- `DELETE /api/group-rules/:group_ref/:name`：删除本地规则文件并 `DELETE /v4/env/:remote_id`。
- `PUT /api/group-rules/:group_ref/:name/enable` / `.../disable`：仅切换本地 enabled 状态（通过 `RulesStorage::set_enabled`），不动远端。

#### 3.3 路由注册

在 `router.rs` 中，`/api/group-rules` 前缀必须先匹配并交给 `handle_group_rules`，再让 `/api/group` 前缀走 `handle_group`，避免前缀冲突。

### 4. bifrost-storage 扩展

#### 4.1 Group 规则存储

实际实现中没有给 `RulesStorage` 增加专门的 `group_storage()` / `list_groups()` 方法（设计上的 `groups/` 中间层目录也未引入）。当前做法是：

- `RulesStorage` 继续暴露 `with_dir(dir)` 和 `base_dir()`，admin handler 直接以 `state.rules_storage.base_dir().join(sanitize_group_dir_name(group_name))` 构造 Group 子目录并通过 `RulesStorage::with_dir` 复用同一套读写逻辑。
- `RuleFile` 在 `crates/bifrost-storage/src/rules.rs` 中增加 `group: Option<String>` 字段，并配套 `with_group()` 构造与 `rule_reference_key` 拼接 `<group>/<name>` 作为规则引用键。
- 顶层 `load_all` 在迭代规则目录时识别已知 group 子目录，把其中的规则也带 `group` 元信息一并返回；非 group 的孤立子目录会被跳过（参见 `test_load_enabled_with_subdirs_keeps_group_directories`）。
- group_id <-> group_name 的映射不在 `RulesStorage` 内部，而是放在 admin state 的 `group_name_cache` 中，持久化到 `rules/.group_cache.json`。

如果未来要把 group 子目录管理收回 storage 层（独立的 `group_storage()` / `list_groups()` API），相关接口仍属于（planned, not yet shipped as of 2026-06-16）。

### 5. Web 前端

#### 5.1 路由配置

```
/groups           - Group 列表页
/groups/:id       - Group 详情页（含成员管理）
```

#### 5.2 侧边栏入口

在 `IconSidebar` 中新增 Group 导航项，仅当同步服务已配置且用户已登录时展示。

#### 5.3 规则页切换器

在 Rules 页面顶部添加一个 Group 选择器下拉框：
- 默认选中"My Rules"（当前用户自己的规则）
- 列出用户加入的所有 Group
- 切换后加载对应 Group 的规则列表

#### 5.4 API 层

新增 `web/src/api/group.ts`：

```typescript
// Group CRUD
createGroup(req): Promise<Group>
searchGroups(query?): Promise<GroupList>
getGroup(id): Promise<Group>
updateGroup(id, req): Promise<void>
deleteGroup(id): Promise<void>

// 成员管理
getGroupMembers(id, query?): Promise<MemberList>
inviteMembers(id, userIds, level?): Promise<void>
removeMember(groupId, userId): Promise<void>
updateMemberLevel(groupId, userId, level): Promise<void>
leaveGroup(id): Promise<void>

// 设置
getGroupSetting(id): Promise<GroupSetting>
updateGroupSetting(id, req): Promise<void>

// Group 规则
getGroupEnvs(id): Promise<EnvList>
createGroupEnv(id, req): Promise<Env>
updateGroupEnv(id, envId, req): Promise<void>
deleteGroupEnv(id, envId): Promise<void>
```

#### 5.5 Store 层

新增 `web/src/stores/useGroupStore.ts`（Zustand）

#### 5.6 页面组件

实际落地结构与最初设想略有差异：列表页内嵌 `GroupCardGrid` 组件而未拆出 `GroupCard.tsx`；`GroupDetail` 是单文件 `.tsx` 而非子目录。

```
pages/Groups/
├── index.tsx              # 列表页：搜索、我管理的 Group、我加入的 Group；内含 GroupCardGrid
└── GroupDetail.tsx         # 详情页（含成员列表与 Group 信息）
```

规则页 Group 切换器、创建/编辑弹窗、搜索 Group 子组件的独立拆分（`CreateGroupModal.tsx`、`SearchGroup.tsx`、`GroupRulesSwitcher.tsx` 等）属于（planned, not yet shipped as of 2026-06-16）。

### 6. CLI 支持（bifrost-cli）

`crates/bifrost-cli/src/commands/group.rs`

通过 admin API（HTTP）实现 Group CLI 命令，需要代理运行中：

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

## 权限控制

### Group 可见性

| 可见性 | 搜索 | 查看详情 | 查看成员 |
|--------|------|----------|----------|
| Public | ✅ 所有人 | ✅ 所有人 | ❌ 仅成员 |
| Private | ✅ 仅成员 | ❌ 仅成员 | ❌ 仅成员 |

### 操作权限

| 操作 | User | Master | Owner |
|------|------|--------|-------|
| 查看 Group 规则 | ✅ | ✅ | ✅ |
| 使用 Group 规则 | ✅ | ✅ | ✅ |
| 编辑 Group 信息 | ❌ | ✅ | ✅ |
| 管理 Group 规则 | ❌ | ✅ | ✅ |
| 邀请成员 | ❌ | ✅ | ✅ |
| 移除成员 | ❌ | ✅ | ✅ |
| 管理成员角色 | ❌ | ✅ | ✅ |
| 删除 Group | ❌ | ❌ | ✅ |
| 修改 Group 设置 | ❌ | ✅ | ✅ |

## 测试方案

### 单元测试
- bifrost-sync-server: Group DAO 层测试（SQLite）
- bifrost-storage: Group 规则子文件夹存储测试
- bifrost-sync: Group 类型序列化测试
- bifrost-cli: Group CLI 命令测试（33 个测试，使用 mock HTTP server 覆盖全部命令）。mock HTTP server 必须串行持有测试锁，避免 lib/bin 两套 group 测试在 workspace 并发运行时共享 HTTP mock 时序导致相邻 add/update 用例偶发拿不到预期响应。

### E2E 测试
- 通过 bifrost-admin API 测试完整的 Group CRUD 流程
- 测试 Group 规则同步功能
- 测试权限控制逻辑

## 校验要求

- 提交前执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 执行 `cargo test --workspace --all-features`
- 使用 e2e-test 技能进行端到端验证
- 使用 rust-project-validate 技能进行项目验证

## 文档更新要求

- 更新 README.md 添加 Group 功能说明
- 更新 ADMIN_API.md 添加 Group API 文档
- 更新 docs/cli.md 添加 Group CLI 命令文档
- 更新 SKILL.md 添加 Group 管理能力映射

## 依赖项

- 现有 bifrost-sync-server（扩展）
- 现有 bifrost-sync crate（扩展）
- 现有 bifrost-admin crate（扩展）
- 现有 bifrost-storage crate（扩展）
- 现有 bifrost-cli crate（扩展，Group 命令）
- 现有 web 前端（扩展）
