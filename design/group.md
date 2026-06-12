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
│   └── groups/                      # Group 规则子目录
│       ├── <group-id>/
│       │   ├── group-rule-1.bifrost
│       │   └── group-rule-2.bifrost
│       └── <group-id>/
│           └── ...
```

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

| 方法 | 路径 | 说明 | 鉴权 |
|------|------|------|------|
| POST | /v4/group | 创建 Group | Token |
| GET | /v4/group | 搜索/列出 Group | Token |
| GET | /v4/group/:id | 获取 Group 详情 | Token + 权限检查 |
| PATCH | /v4/group/:id | 更新 Group | Token + Master/Owner |
| DELETE | /v4/group/:id | 删除 Group | Token + Owner |
| GET | /v4/group/:id/members | 获取成员列表 | Token + 成员 |
| POST | /v4/group/:id/invite | 邀请成员 | Token + Master/Owner |
| DELETE | /v4/group/:id/member/:user_id | 移除成员 | Token + Master/Owner |
| PATCH | /v4/group/:id/member/:user_id | 更新成员角色 | Token + Master/Owner |
| POST | /v4/group/:id/leave | 退出 Group | Token + 成员 |
| GET | /v4/group/:id/setting | 获取 Group 设置 | Token + Master/Owner |
| PATCH | /v4/group/:id/setting | 更新 Group 设置 | Token + Master/Owner |
| GET | /v4/group/:id/envs | 获取 Group 规则列表 | Token + 成员 |
| POST | /v4/group/:id/envs | 创建 Group 规则 | Token + Master/Owner |
| PATCH | /v4/group/:id/envs/:env_id | 更新 Group 规则 | Token + Master/Owner |
| DELETE | /v4/group/:id/envs/:env_id | 删除 Group 规则 | Token + Master/Owner |

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
    pub avatar: String,
    pub description: String,
    pub visibility: String,
    pub level: Option<i32>,
    pub create_time: String,
    pub update_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGroupMember {
    pub id: String,
    pub group_id: String,
    pub user_id: String,
    pub level: i32,
    pub nickname: String,
    pub avatar: String,
    pub email: String,
    pub create_time: String,
    pub update_time: String,
}
```

#### 2.2 新增 Group HTTP 客户端方法

在 `SyncHttpClient` 中添加 Group 相关 API 调用方法。

### 3. bifrost-admin 扩展

#### 3.1 新增 Group handler

`crates/bifrost-admin/src/handlers/group.rs`

作为代理层，将前端请求转发到 bifrost-sync-server 的 Group API，并附带当前用户的 Token。

#### 3.2 路由注册

在 `router.rs` 中添加 `/api/group` 路径的路由分发。

### 4. bifrost-storage 扩展

#### 4.1 Group 规则存储

扩展 `RulesStorage`，支持 Group 子目录的规则读写：

```rust
impl RulesStorage {
    pub fn group_storage(&self, group_id: &str) -> Result<RulesStorage> {
        let group_dir = self.base_dir.join("groups").join(group_id);
        RulesStorage::with_dir(group_dir)
    }

    pub fn list_groups(&self) -> Result<Vec<String>> {
        let groups_dir = self.base_dir.join("groups");
        if !groups_dir.exists() {
            return Ok(Vec::new());
        }
        // 列出所有子目录名
    }
}
```

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

```
pages/Groups/
├── index.tsx              # 列表页：搜索、我管理的 Group、我加入的 Group
├── GroupCard.tsx           # Group 卡片
├── CreateGroupModal.tsx    # 创建/编辑 Group 弹窗
├── SearchGroup.tsx         # 搜索 Group 组件
├── GroupDetail/
│   ├── index.tsx          # 详情页布局
│   ├── GroupInfo.tsx      # Group 信息展示
│   └── Members.tsx        # 成员列表
└── GroupRulesSwitcher.tsx  # 规则页面顶部的 Group 切换组件
```

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
