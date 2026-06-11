# Bifrost Sync Server

一个独立的、轻量级的 Bifrost 规则同步服务器，基于 Node.js + TypeScript 构建。它实现了 Bifrost 客户端的完整同步协议，允许用户自行部署一套规则同步服务，配合 Bifrost 代理使用。

## 为什么需要这个项目？

Bifrost 代理客户端内置了规则同步能力，可以将本地代理规则同步到远程服务器，也可以从远程拉取规则。默认情况下，同步服务由官方提供。但在以下场景中，你可能需要自行部署同步服务：

- **私有化部署**：在内网环境中运行，不希望数据外传
- **自定义鉴权**：需要接入自有的用户体系（LDAP、OAuth 等）
- **二次开发**：基于此项目扩展更多功能（规则审计、权限控制、团队协作等）

本项目提供了一个开箱即用的参考实现，你可以直接使用，也可以基于它进行二次开发。

## 功能特性

- **配置文件驱动** — 通过 YAML/JSON 配置文件一站式管理服务器、存储、认证
- **双存储后端** — 默认 SQLite（零配置），可一键切换 MySQL
- **双认证模式** — 默认用户名/密码认证，可配置 OAuth2/OIDC
- **浏览器页面** — 内置登录页面 (`/v4/sso/login`) 和注册页面 (`/v4/sso/register-page`)
- **多用户支持** — 每个用户拥有独立的规则空间，按 `user_id` 隔离
- **规则 CRUD** — 完整的规则环境增删改查 API
- **双向同步** — 支持 Bifrost 客户端的批量同步协议（push + pull + 冲突检测）
- **DAO 分层架构** — 清晰的 DAO 接口层，方便扩展新的存储后端
- **零框架依赖** — 直接使用 Node.js 原生 `http` 模块，无 Express/Koa
- **可编程集成** — 导出 `createSyncServer` / `startSyncServer` 工厂函数，可嵌入到你的应用中

## 快速开始

### 环境要求

- Node.js >= 18
- pnpm（推荐）

### 安装

```bash
cd packages/bifrost-sync-server
pnpm install
```

### 启动服务

**开发模式（使用 tsx 直接运行 TypeScript）：**

```bash
pnpm dev
# 或指定配置文件
npx tsx src/cli.ts -c config.yaml
# 或指定参数覆盖
npx tsx src/cli.ts -p 8686 -d ./my-data
```

**生产模式（先编译再运行）：**

```bash
pnpm build
pnpm start
# 或
node dist/cli.js -c config.yaml
```

### CLI 参数

| 参数 | 缩写 | 默认值 | 说明 |
|------|------|--------|------|
| `--config` | `-c` | 自动查找 `config.yaml` | 配置文件路径（YAML/JSON） |
| `--port` | `-p` | `8686` | 监听端口（覆盖配置文件） |
| `--host` | `-H` | `0.0.0.0` | 绑定地址（覆盖配置文件） |
| `--data-dir` | `-d` | `./bifrost-sync-data` | SQLite 数据目录（覆盖配置文件） |
| `--rate-limit-per-ip` | — | `200` | 每个 IP 每分钟全局请求数上限（覆盖配置文件） |
| `--auth-rate-limit-per-ip` | — | `20` | 每个 IP 每分钟登录/注册请求数上限（覆盖配置文件） |
| `--help` | `-h` | — | 显示帮助信息 |

CLI 参数优先级高于配置文件，方便在不修改配置文件的情况下临时调整。

## 配置文件

服务启动时会按顺序查找 `config.yaml`、`config.yml`、`config.json`，也可以通过 `--config` 指定路径。

复制 `config.example.yaml` 作为起点：

```bash
cp config.example.yaml config.yaml
```

### 完整配置示例

```yaml
# ─── Server ─────────────────────────────────────────────────
server:
  port: 8686
  host: 0.0.0.0
  rate_limit_per_ip: 200         # 每个 IP 每分钟全局请求数上限
  auth_rate_limit_per_ip: 20     # 每个 IP 每分钟登录/注册请求数上限

# ─── Storage ────────────────────────────────────────────────
storage:
  type: sqlite                   # sqlite 或 mysql
  sqlite:
    data_dir: ./bifrost-sync-data

# ─── Auth ───────────────────────────────────────────────────
auth:
  mode: password                 # password 或 oauth2
```

### SQLite 配置（默认）

无需额外配置，开箱即用：

```yaml
storage:
  type: sqlite
  sqlite:
    data_dir: ./bifrost-sync-data
```

### MySQL 配置

使用 MySQL 时，需要先创建数据库并执行 `sql/init-mysql.sql` 初始化表结构：

```bash
mysql -u root -p < sql/init-mysql.sql
```

然后配置：

```yaml
storage:
  type: mysql
  mysql:
    host: 127.0.0.1
    port: 3306
    user: root
    password: your-password
    database: bifrost_sync
```

### OAuth2 认证配置

默认使用用户名/密码认证。如需接入 OAuth2 提供商（如 GitHub、Google、企业 SSO 等），配置如下：

```yaml
auth:
  mode: oauth2
  oauth2:
    client_id: your-client-id
    client_secret: your-client-secret
    authorize_url: https://accounts.example.com/oauth2/authorize
    token_url: https://accounts.example.com/oauth2/token
    userinfo_url: https://accounts.example.com/oauth2/userinfo
    scopes:
      - openid
      - profile
      - email
    redirect_uri: http://localhost:8686/v4/sso/callback  # 可选，默认自动检测
    user_id_field: sub          # userinfo 响应中的用户 ID 字段
    nickname_field: name        # 昵称字段
    email_field: email          # 邮箱字段
    avatar_field: picture       # 头像字段
```

**OAuth2 认证流程：**

1. 用户访问 `GET /v4/sso/login?next=<callback>` → 重定向到 OAuth2 授权页面
2. 用户在 OAuth2 提供商处登录授权
3. 回调到 `GET /v4/sso/callback?code=xxx&state=yyy`
4. 服务端用 code 换取 access_token，再获取用户信息
5. 自动注册/登录用户，生成 bifrost token
6. 重定向回 `next` 地址并附带 `?token=xxx`

**字段映射说明：** `user_id_field` 等支持嵌套路径，如 `data.user.id`，适配不同 OAuth2 提供商的响应格式。

#### GitHub OAuth2 示例

```yaml
auth:
  mode: oauth2
  oauth2:
    client_id: Iv1.xxxxxxxxxx
    client_secret: xxxxxxxxxxxxxxxx
    authorize_url: https://github.com/login/oauth/authorize
    token_url: https://github.com/login/oauth/access_token
    userinfo_url: https://api.github.com/user
    scopes: ["user:email"]
    user_id_field: login
    nickname_field: name
    email_field: email
    avatar_field: avatar_url
```

#### Google OAuth2 示例

```yaml
auth:
  mode: oauth2
  oauth2:
    client_id: xxxxx.apps.googleusercontent.com
    client_secret: GOCSPX-xxxxxxxxxx
    authorize_url: https://accounts.google.com/o/oauth2/v2/auth
    token_url: https://oauth2.googleapis.com/token
    userinfo_url: https://www.googleapis.com/oauth2/v3/userinfo
    scopes: ["openid", "profile", "email"]
    user_id_field: sub
    nickname_field: name
    email_field: email
    avatar_field: picture
```

### 使用流程

**1. 注册用户（密码模式）**

通过浏览器访问 `http://localhost:8686/v4/sso/register-page`，或通过 API：

```bash
curl -X POST http://localhost:8686/v4/sso/register \
  -H "Content-Type: application/json" \
  -d '{"user_id": "your-username", "password": "your-password", "nickname": "Your Name"}'
```

响应中会返回 `token`，后续请求均通过 `x-bifrost-token` 请求头携带此 token 进行认证。

**2. 登录（如果已注册）**

通过浏览器访问 `http://localhost:8686/v4/sso/login`，或通过 API：

```bash
curl -X POST http://localhost:8686/v4/sso/login \
  -H "Content-Type: application/json" \
  -d '{"user_id": "your-username", "password": "your-password"}'
```

**3. 配置 Bifrost 客户端同步**

在 Bifrost 代理管理界面中，配置远程同步地址为 `http://<your-server>:8686`，并设置获取到的 token。

或通过 API 配置：

```bash
curl -X PUT http://localhost:8800/_bifrost/api/sync/config \
  -H "Content-Type: application/json" \
  -d '{"enabled": true, "remote_base_url": "http://localhost:8686"}'

curl -X POST http://localhost:8800/_bifrost/api/sync/session \
  -H "Content-Type: application/json" \
  -d '{"token": "your-token-here"}'
```

**4. 触发同步**

```bash
curl -X POST http://localhost:8800/_bifrost/api/sync/run
```

## SQL 初始化

项目在 `sql/` 目录下提供了数据库初始化脚本：

| 文件 | 说明 |
|------|------|
| `sql/init-sqlite.sql` | SQLite 表结构（SQLite 模式下自动执行，无需手动） |
| `sql/init-mysql.sql` | MySQL 表结构（需手动执行） |

```bash
# MySQL 初始化
mysql -u root -p < sql/init-mysql.sql
```

## API 参考

所有需要认证的接口均通过 `x-bifrost-token` 请求头进行身份验证。

### 认证接口 (SSO)

| 端点 | 方法 | 说明 |
|------|------|------|
| `/v4/sso/register` | POST | 注册新用户（密码模式） |
| `/v4/sso/register-page` | GET | 浏览器注册页面 |
| `/v4/sso/login` | POST | 用户名密码登录 |
| `/v4/sso/login?next=<url>` | GET | 浏览器登录页面（密码模式渲染表单，OAuth2 模式重定向） |
| `/v4/sso/callback` | GET | OAuth2 回调端点 |
| `/v4/sso/check` | GET | 验证 Token 有效性 |
| `/v4/sso/info` | GET | 获取当前用户信息 |
| `/v4/sso/logout` | GET | 注销 |

### 规则环境接口 (Env)

| 端点 | 方法 | 说明 |
|------|------|------|
| `/v4/env` | GET | 搜索规则 |
| `/v4/env` | POST | 创建规则 |
| `/v4/env/:id` | GET | 获取单条规则 |
| `/v4/env/:id` | PATCH | 更新规则 |
| `/v4/env/:id` | DELETE | 删除规则 |
| `/v4/env/sync` | POST | 批量同步（Bifrost 客户端自动调用） |

## 编程方式集成

除了 CLI 启动，你也可以将 sync server 作为模块嵌入到自己的 Node.js 应用中：

```typescript
import { startSyncServer, loadConfig } from '@aspect-build/bifrost-sync-server';

// 方式 1：使用配置文件
const config = loadConfig('./config.yaml');
const instance = await startSyncServer(config);

// 方式 2：直接传入配置
const instance = await startSyncServer({
  server: { port: 8686, host: '0.0.0.0' },
  storage: { type: 'sqlite', sqlite: { data_dir: './data' } },
  auth: { mode: 'password' },
});

console.log(`Sync server running on port ${instance.port}`);

// 关闭
await instance.close();
```

### 导出的类型

```typescript
interface SyncServerConfig {
  server: ServerConfig;
  storage: StorageConfig;
  auth: AuthConfig;
}

interface ServerConfig {
  port: number;
  host: string;
}

interface StorageConfig {
  type: 'sqlite' | 'mysql';
  sqlite?: { data_dir: string };
  mysql?: MysqlConfig;
}

interface AuthConfig {
  mode: 'password' | 'oauth2';
  oauth2?: OAuth2Config;
}

interface SyncServerInstance {
  server: Server;
  storage: IStorage;
  port: number;
  close: () => Promise<void>;
}
```

## 项目结构

```
src/
├── cli.ts              # CLI 入口，解析命令行参数、加载配置文件
├── index.ts            # 模块入口，导出 createSyncServer / startSyncServer
├── config.ts           # 配置文件加载（YAML/JSON，支持深度合并）
├── types.ts            # TypeScript 类型定义
├── http.ts             # HTTP 工具函数（JSON 响应、鉴权中间件、请求解析）
├── dao/
│   ├── types.ts        # DAO 接口定义（IUserDao, IEnvDao, IStorage）
│   ├── index.ts        # DAO 工厂（createStorage）
│   ├── sqlite.ts       # SQLite DAO 实现
│   └── mysql.ts        # MySQL DAO 实现
└── routes/
    ├── sso.ts          # SSO 认证路由（注册、登录、登出、Token 校验）
    ├── oauth2.ts       # OAuth2 认证路由（授权重定向、回调处理）
    └── env.ts          # 规则环境路由（CRUD + 批量同步）

sql/
├── init-sqlite.sql     # SQLite 表结构
└── init-mysql.sql      # MySQL 表结构

config.example.yaml     # 配置文件示例
test/
└── e2e-sync.sh         # 端到端测试脚本
```

## 开发指南

### 本地开发

```bash
pnpm install
pnpm dev            # 开发模式运行
pnpm lint           # 类型检查
pnpm build          # 构建
pnpm clean          # 清理构建产物
```

### 运行 E2E 测试

```bash
bash packages/bifrost-sync-server/test/e2e-sync.sh
```

### 二次开发建议

**1. 接入自有用户体系**

修改 `src/routes/sso.ts`，或通过配置文件接入 OAuth2 提供商。DAO 层接口清晰，只需确保：
- `IUserDao.findByToken()` — token → user 映射
- `IUserDao.saveToken()` — 保存 token
- `IUserDao.clearToken()` — 清除 token

**2. 添加新的存储后端**

实现 `IStorage`、`IUserDao`、`IEnvDao` 接口，然后在 `dao/index.ts` 的 `createStorage` 中注册即可。

**3. 规则审计日志**

在 DAO 层添加 `audit_logs` 表和相关方法，在 `env.ts` 路由中记录操作日志。

### 协议兼容性

本项目实现的 API 完全兼容 Bifrost 客户端（`bifrost-sync` crate）的同步协议。如果你修改了 API 路径或响应格式，需要同步修改 Bifrost 客户端的配置。

## 技术细节

| 项目 | 说明 |
|------|------|
| 运行时 | Node.js >= 18 |
| 语言 | TypeScript 5 |
| HTTP | 原生 `http` 模块，无框架 |
| 存储 | SQLite（默认） / MySQL（可配置） |
| 认证 | 用户名/密码（默认） / OAuth2（可配置） |
| 配置 | YAML / JSON |
| 密码哈希 | `crypto.scryptSync` + 随机 salt |
| ID 生成 | `nanoid` |
| Token | 32 字符随机字符串（`nanoid(32)`） |
