# Sync Provider Architecture

## Background

Bifrost 当前 Sync 体系把远端同步服务建模为一个全局 `remote_base_url`。这个模型能覆盖官方或自建 Bifrost Sync Server, 但无法表达以下新目标:

- 同时启用多个同步目标, 例如 ByteDance Internal + Bifrost Cloud + GitHub Gist; 三者独立连接, 不互斥。
- 区分 provider 能力: 有的只支持规则同步和基础配置同步, 有的支持小组规则, 有的还能承担 Remote Invoke relay。
- 把 GitHub Gist 这类公开可信开发者服务纳入个人规则同步, 同时保留现有 Bifrost Sync Server 协议。
- 在 ByteDance 内网自动检测可用 provider, 检测通过后自动推荐或配置 `https://bifrost.bytedance.net`。
- 当用户完全没有登录任何同步服务时, Web UI 首次进入 Sync 页面应主动弹出登录选择窗口。
- 让未来 Gitee, WebDAV, OneDrive, Google Drive 等 provider 可以按同一扩展点接入。

因此本方案把 Sync 从 "one remote URL" 升级为 "provider registry + provider instances + capability routing + sync lanes"。

## Goals

### Must implement

- 引入 `Provider Type`: 一类同步服务实现, 例如 `bifrost-server`, `github-gist`, `webdav`。
- 引入 `Provider Instance`: 用户实际启用的一个账号或端点, 例如 `byte-work`, `github-personal`。
- 引入 `Capability`: provider 能力声明, 区分规则同步、基础配置同步、小组规则、Remote Invoke relay、SSO、OAuth、端到端加密等。
- 引入 `Sync Lane`: 按数据/能力维度配置路由, 至少包含 `personal_rules`, `basic_config`, `group_rules`, `remote_invoke`。
- 支持多个 provider instance 同时 enabled, 但每条 lane 独立声明读写目标和优先级。
- 产品首屏支持三种内置同步服务卡片: ByteDance Internal, Bifrost Cloud, GitHub Gist。三者都可以独立连接, 可同时处于已登录状态。
- GitHub Gist provider 承担个人规则同步和基础配置同步, 不出现在小组规则和 Remote Invoke relay 路由中。
- Bifrost Cloud provider 继续兼容现有 Sync Server 协议, 并可声明小组规则和 Remote Invoke relay 能力。技术类型仍为 `bifrost-server`。
- ByteDance 内网 provider 通过探测自动发现, 检测成功后作为 managed provider 推荐启用。
- Remote Invoke 注册只允许 ByteDance Internal 或 Bifrost Cloud 这两类带 `remote_invoke_relay` capability 的 provider; 如果两者都已登录且启用, 本机服务要同时向两个 provider 注册。
- 基础配置同步只覆盖用户明确要求的 Settings 基础网络配置: 应用白名单、域名白名单、黑名单/排除列表; 其他配置暂不同步。
- 旧 `sync.remote_base_url` 迁移为默认 `bifrost-server` provider instance 的配置字段。

### Must not break

- 现有 `bifrost sync login --token --url` 仍可用, 映射到 `bifrost-server` provider。
- 现有 Admin API `/sync/status`, `/sync/config`, `/sync/login`, `/sync/run` 在兼容期继续返回旧字段。
- Remote Invoke relay 不再无条件读取规则同步 provider URL; 只有带 `remote_invoke_relay` capability 的 provider 才能被 lane 选中。
- Group Rules 不从 GitHub Gist/WebDAV 这类个人文件型 provider 读取。
- 基础配置同步不包含证书、密码、代理认证、Remote Invoke grant、IM provider secret、Agent 模型密钥或其它敏感配置。
- Provider token, OAuth access token, Gist token 不写入公开配置文件。

### Out of scope for first implementation

- 多人实时协同编辑规则。
- 跨 provider 自动三方合并冲突。
- GitHub Gist 承载小组规则或 Remote Invoke relay。
- 把 Remote Invoke call payload 存到文件型 provider。

## Concept Model

### Provider Type

Provider Type 是代码层扩展点, 每个 type 实现相同 trait, 并声明静态能力。

| Type | Description | First class use |
| --- | --- | --- |
| `bifrost-server` | 现有 Bifrost Sync Server 协议, 覆盖 ByteDance Internal、Bifrost Cloud、自建部署 | 规则、基础配置、小组、Remote Invoke |
| `github-gist` | GitHub OAuth + secret gist 文件 | 个人规则和基础配置同步 |
| `webdav` | WebDAV 文件存储 | 个人规则和基础配置同步或备份 |
| `gitee-repo` | Gitee/GitCode 私有仓库文件 | 个人规则和基础配置同步 |

### Provider Instance

Provider Instance 是用户配置或自动发现的实际目标。

```json
{
  "id": "github-personal",
  "type": "github-gist",
  "display_name": "Personal GitHub Gist",
  "enabled": true,
  "mode": "primary",
  "config": {
    "gist_id": "abc123",
    "filename": "bifrost-sync.json"
  }
}
```

`mode` 仅作为默认产品语义:

- `primary`: 可作为 lane 主要读写目标。
- `mirror`: 默认只写入备份, 手动拉取时才读。
- `disabled`: 保留配置和会话, 不参与同步。
- `managed`: 系统内置或自动检测目标, 用户可退出但不能编辑核心端点。

### Capability

能力声明是 UI, CLI, Admin API 和 Sync Engine 的共同约束。

| Capability | Meaning |
| --- | --- |
| `rules_sync` | 个人规则列表和规则内容快照读写 |
| `config_sync` | 基础配置快照读写, 仅覆盖当前允许同步的 Settings 子集 |
| `group_rules` | 小组列表和小组规则读写 |
| `remote_invoke_relay` | Remote Invoke relay / pairing / grants / calls |
| `identity_profile` | 可提供用户身份信息 |
| `server_conflict_resolution` | 服务端能提供远端对象级冲突信息 |
| `file_snapshot` | 以单文件快照存储, 需要客户端冲突控制 |
| `oauth_device_flow` | 支持 CLI/headless device flow |
| `sso_browser_flow` | 支持浏览器 SSO 登录 |
| `end_to_end_encryption_required` | 内容必须客户端加密后上传 |
| `auto_detected` | 可由本机环境探测自动推荐 |

### Sync Lane

Lane 是能力路由表, 决定每类数据从哪些 provider 读、写到哪些 provider。

```json
{
  "lanes": {
    "personal_rules": {
      "enabled": true,
      "read_from": ["byte-work"],
      "write_to": ["byte-work", "bifrost-cloud", "github-personal"],
      "conflict_policy": "ask",
      "mirror_policy": "best_effort"
    },
    "basic_config": {
      "enabled": true,
      "read_from": ["byte-work"],
      "write_to": ["byte-work", "bifrost-cloud", "github-personal"],
      "scope": ["app_allowlist", "domain_allowlist", "blacklist"]
    },
    "group_rules": {
      "enabled": true,
      "read_from": ["byte-work"],
      "write_to": ["byte-work"]
    },
    "remote_invoke": {
      "enabled": true,
      "providers": ["byte-work", "bifrost-cloud"],
      "registration": "all_eligible",
      "selection": "priority"
    }
  }
}
```

Default product rule:

- 普通用户默认只启用一个 primary provider。
- 多 provider 同时启用时, 第二个及之后默认作为 mirror, 防止多写冲突。
- 只有高级设置允许 multi-read / multi-write。

## Product Provider Matrix

| Product provider | Type | Rules sync | Config sync | Remote Invoke | Group rules | Identity | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| ByteDance Internal | `bifrost-server` managed | Yes | Yes | Yes | Yes | Byte SSO | 自动检测, 内网推荐, 可与其它 provider 同时连接 |
| Bifrost Cloud | `bifrost-server` custom URL | Yes | Yes | Yes | Optional | Bifrost token / OAuth2 | 官方云或自建同步服务, 能力由 server `/v4/capabilities` 返回 |
| GitHub Gist | `github-gist` | Yes | Yes | No | No | GitHub OAuth | secret gist + 客户端加密; 只同步个人规则和基础配置 |
| WebDAV | `webdav` | Yes | Yes | No | No | Basic/app password | 后续国内和自部署 fallback |
| Gitee Repo | `gitee-repo` | Yes | Yes | No | No | OAuth/PAT | 国内 Git provider 后续接入 |

UI 和 CLI 必须以此矩阵裁剪动作:

- 不支持 `remote_invoke_relay` 的 provider 不显示 Remote Invoke 入口。
- 不支持 `group_rules` 的 provider 不显示小组开关。
- 所有 provider 卡片都显示三类核心能力: `Remote Invoke`, `Rules Sync`, `Config Sync`。
- GitHub Gist 显示 `Rules Sync` 和 `Config Sync`, `Remote Invoke` 显示为不支持。
- 文件型 provider 默认显示 "Rules + Config only"。

## Storage And Config

### New config shape

`config.toml` 或 JSON internal model 需要支持 provider map 和 lanes。

```toml
[sync]
enabled = true
auto_sync = true
active_personal_rules_provider = "byte-work"

[sync.providers.byte-work]
type = "bifrost-server"
enabled = true
display_name = "ByteDance Sync"
managed = true

[sync.providers.byte-work.config]
remote_base_url = "https://bifrost.bytedance.net"

[sync.providers.bifrost-cloud]
type = "bifrost-server"
enabled = true
display_name = "Bifrost Cloud"
mode = "primary"

[sync.providers.bifrost-cloud.config]
remote_base_url = "https://sync.example.com"

[sync.providers.github-personal]
type = "github-gist"
enabled = true
display_name = "Personal GitHub Gist"
mode = "mirror"

[sync.providers.github-personal.config]
gist_id = "abc123"
filename = "bifrost-sync.json"

[sync.lanes.personal_rules]
enabled = true
read_from = ["byte-work"]
write_to = ["byte-work", "bifrost-cloud", "github-personal"]
conflict_policy = "ask"

[sync.lanes.basic_config]
enabled = true
read_from = ["byte-work"]
write_to = ["byte-work", "bifrost-cloud", "github-personal"]
scope = ["app_allowlist", "domain_allowlist", "blacklist"]

[sync.lanes.group_rules]
enabled = true
read_from = ["byte-work"]
write_to = ["byte-work"]

[sync.lanes.remote_invoke]
enabled = true
providers = ["byte-work", "bifrost-cloud"]
registration = "all_eligible"
selection = "priority"
```

### Backward compatibility

Existing:

```toml
[sync]
remote_base_url = "https://bifrost.bytedance.net"
```

Migrates at read time to:

```toml
[sync.providers.default-bifrost-server]
type = "bifrost-server"
enabled = true

[sync.providers.default-bifrost-server.config]
remote_base_url = "https://bifrost.bytedance.net"
```

Compat APIs still surface `remote_base_url` as the selected `bifrost-server` provider URL until UI/CLI migration completes.

### Secret storage

Provider session secrets must live outside normal config:

- macOS Keychain / Windows Credential Manager / Linux Secret Service when available.
- Fallback encrypted state file under `BIFROST_DATA_DIR`, never committed or exported.
- `sync-state.json` may keep non-secret metadata: provider user, gist id, remote revision, last sync result.

## Provider Trait

Rust trait sketch:

```rust
#[async_trait::async_trait]
pub trait SyncProvider: Send + Sync {
    fn provider_type(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn static_capabilities(&self) -> ProviderCapabilities;

    async fn detect(&self, ctx: &ProviderDetectContext) -> ProviderDetection;
    async fn probe(&self, instance: &ProviderInstance, session: Option<&ProviderSession>) -> ProviderHealth;
    async fn capabilities(&self, instance: &ProviderInstance, session: Option<&ProviderSession>) -> ProviderCapabilities;

    async fn auth_start(&self, instance: &ProviderInstance, options: AuthStartOptions) -> Result<AuthStartResult>;
    async fn auth_poll(&self, instance: &ProviderInstance, state: AuthState) -> Result<AuthPollResult>;
    async fn logout(&self, instance: &ProviderInstance, session: ProviderSession) -> Result<()>;

    async fn read_personal_rules(&self, instance: &ProviderInstance, session: &ProviderSession) -> Result<RemoteSnapshot>;
    async fn write_personal_rules(
        &self,
        instance: &ProviderInstance,
        session: &ProviderSession,
        snapshot: SyncEnvelope,
        precondition: WritePrecondition,
    ) -> Result<WriteResult>;

    async fn read_basic_config(&self, instance: &ProviderInstance, session: &ProviderSession) -> Result<RemoteSnapshot>;
    async fn write_basic_config(
        &self,
        instance: &ProviderInstance,
        session: &ProviderSession,
        snapshot: SyncEnvelope,
        precondition: WritePrecondition,
    ) -> Result<WriteResult>;
}
```

Group and Remote Invoke should use extension traits so file-only providers do not implement unrelated methods:

```rust
pub trait GroupRulesProvider {
    async fn list_groups(&self, ...);
    async fn read_group_rules(&self, ...);
    async fn write_group_rules(&self, ...);
}

pub trait RemoteInvokeRelayProvider {
    async fn start_pairing(&self, ...);
    async fn open_call(&self, ...);
    async fn stream_call_events(&self, ...);
}
```

## Sync Engine

### Personal rules lane

Sync Engine owns normalization, local snapshot generation, tombstones, conflicts, and fan-out writes.

Flow:

1. Load enabled provider instances for `personal_rules`.
2. Read from primary `read_from[0]`.
3. If advanced multi-read enabled, read additional providers and compare snapshot revision.
4. Compare remote snapshot with local rules/tombstones.
5. Apply local changes or remote changes according to conflict policy.
6. Write resulting canonical snapshot to all `write_to` providers.
7. If mirror write fails, mark provider degraded but do not fail primary sync unless lane policy says strict.

### Basic config lane

Basic config sync reuses the same provider fan-out model as personal rules, but the snapshot scope is intentionally narrow:

- Application allowlist/blocklist fields used by Settings.
- Domain allowlist/blocklist fields used by Settings.
- Blacklist/exclude lists that affect the same proxy rule and TLS selection surface.

The lane must not sync secrets, certificates, admin passwords, proxy credentials, model/API keys, Remote Invoke grants, IM provider secrets, traffic history, or any local machine identity. Adding a new config key to this lane requires an explicit allowlist entry and a human_tests case.

### Group rules lane

Only providers with `group_rules` capability may participate. For v1, use one primary group provider. Multi-provider group sync is deferred because membership and ACL semantics are provider-specific.

### Remote Invoke lane

Remote Invoke is not a file sync operation. It routes live relay traffic to providers with `remote_invoke_relay`.

Registration:

- `registration = all_eligible`: if ByteDance Internal and Bifrost Cloud are both connected and enabled, Bifrost registers this local instance with both relay providers.
- Registration failures are tracked per provider. A failure on one relay provider must not unregister the other.
- GitHub Gist, WebDAV, and other file providers are never eligible for Remote Invoke registration.
- The Settings page should show registration state per eligible provider: Registered, Needs sign in, Failed, Disabled.

Selection for new outbound Remote Invoke operations:

- `priority`: first healthy provider wins.
- `manual`: user explicitly selects one provider.
- Future `failover`: active calls stay pinned; new pairings can switch after provider health failure.

Remote Invoke must not silently fall back to GitHub Gist, WebDAV, or any file provider.

## GitHub Gist Provider

### Product positioning

GitHub Gist is for developer-trusted personal rule sync, basic config sync, and optional backup. It is not a team collaboration or Remote Invoke provider.

UI copy:

- Title: `GitHub Gist`
- Capability badges: `Rules Sync`, `Config Sync`
- Security badge: `Encrypted snapshot`
- Warning: `Secret gists are not end-to-end private by themselves. Bifrost encrypts rule data before upload.`

### Auth

Use GitHub OAuth device flow for CLI and desktop-friendly login.

State:

- `device_code`, `user_code`, `verification_uri`, `expires_at`, `interval`.
- Access token stored in secret storage.
- User identity fetched and stored as provider profile.

### Remote layout

Default gist:

- Description: `Bifrost Sync (encrypted)`
- Public: `false`
- Files:
  - `bifrost-rules-sync.json`
  - `bifrost-config-sync.json`

Payload:

```json
{
  "schema_version": 1,
  "format": "bifrost.sync.snapshot.v1",
  "provider_type": "github-gist",
  "collection_id": "uuid",
  "revision": "logical-revision",
  "updated_at": "2026-07-06T12:00:00Z",
  "device": {
    "id": "device-id",
    "name": "MacBook Pro"
  },
  "encryption": {
    "mode": "xchacha20-poly1305",
    "kdf": "argon2id",
    "salt": "base64"
  },
  "ciphertext": "base64",
  "checksum": "sha256"
}
```

### Encryption

Gist provider requires client-side encryption:

- First device creates a sync recovery key.
- Additional devices log in to GitHub and import recovery key.
- Recovery key export is a deliberate action in UI/CLI.
- No raw rule or config content is uploaded to GitHub.

### Conflict handling

Gist does not provide strong compare-and-swap for file update. Bifrost must use conservative preconditions:

- Record gist `updated_at`, latest history version, and payload `revision`.
- Before write, re-read metadata; if changed since last read, enter conflict.
- Conflict modal shows local timestamp, remote timestamp, remote device name, and provider.
- Actions: Pull remote, Push local, Save local as copy.

## ByteDance Internal Provider

### Detection

The `bytedance-internal` managed provider wraps `bifrost-server` type with canonical URL `https://bifrost.bytedance.net`.

Detection:

1. Probe `GET https://bifrost.bytedance.net/v4/sso/check`.
2. Use short timeout, default 800-1500ms.
3. Treat HTTP 200 or 401 as reachable.
4. Treat DNS failure, TLS failure, timeout, and 5xx as unavailable.
5. Cache result for a short TTL to avoid blocking Settings UI.

Behavior:

- If no provider is configured and detection succeeds, create managed instance `byte-work` and recommend it.
- If user already selected another provider, do not override. Show ByteDance as available.
- If detection fails, keep the catalog entry visible but not selected.

### Capabilities

ByteDance provider should request server capabilities from `/v4/capabilities` when available. Fallback capability set:

- `rules_sync`
- `config_sync`
- `group_rules`
- `remote_invoke_relay`
- `identity_profile`
- `sso_browser_flow`
- `auto_detected`

## Bifrost Cloud Provider

`Bifrost Cloud` is the product-facing name for a user-configured `bifrost-server` provider. It may point to an official cloud endpoint, a private deployment, or a custom self-hosted Sync Server URL.

Product behavior:

- It is independent from ByteDance Internal. Users can connect both.
- It supports `Rules Sync`, `Config Sync`, and, when server capability says so, `Remote Invoke`.
- If it supports `remote_invoke_relay` and is signed in, local Bifrost registers Remote Invoke service with it.
- If both ByteDance Internal and Bifrost Cloud are signed in, both get Remote Invoke registration.
- Server `/v4/capabilities` decides whether group rules and remote invoke are available; UI must not assume every custom URL supports them.

## Server-Side Config Sync Changes

Rules Sync already exists on both server implementations as the `/v4/env/sync` protocol. Basic Config Sync does not exist yet and must be added to both Bifrost Cloud and ByteDance Internal in the same delivery as the client provider work. The final implementation push should not ship a UI that advertises `Config Sync` until both server paths can store, return, and conflict-check the allowed config snapshot.

### Existing code inventory

| Service | Repo/path | Current rules sync surface | Current storage | Current gap |
| --- | --- | --- | --- | --- |
| ByteDance Internal | `/Users/eden_studio/work/github/bifrost-server-v4` | `app/idl/env.thrift`, `typings/app/idl/env.ts`, `app/controller/env.ts`, `app/service/env.ts` expose `/v4/env/sync` | `app/model/env.ts` and `db.sql` store `bifrost_server_env` rows | No config-sync IDL, controller, service, model, or DB table |
| Bifrost Cloud | `packages/bifrost-sync-server` | `src/routes/env.ts` exposes `/v4/env/sync` | `src/dao/{types,sqlite,mysql}.ts` and `sql/init-{sqlite,mysql}.sql` store `bifrost_envs` rows | No config-sync route, DAO, schema, or capability endpoint |

Both services also have Remote Invoke relay code, but Remote Invoke grants, pairings, clients, calls, and SSH claims must remain outside Config Sync.

### Shared Basic Config Sync protocol

Add a new server protocol, versioned under `/v4/config/sync`, instead of extending `/v4/env/sync`. Keeping the lane separate makes it testable and prevents accidental mixing of rules and local settings.

Request shape:

```ts
interface SyncBasicConfigReq {
  scope: 'basic_config';
  check_list: Array<{
    key: BasicConfigKey;
    update_time: string;
    hash: string;
  }>;
  update_list: Array<{
    key: BasicConfigKey;
    value_json: string;
    hash: string;
    update_time: string;
  }>;
  delete_list: Array<{
    key: BasicConfigKey;
    delete_time: string;
  }>;
}
```

Response shape:

```ts
interface SyncBasicConfigRes {
  code: number;
  message: string;
  data: {
    local_update_list: Array<{
      key: BasicConfigKey;
      value_json: string;
      hash: string;
      update_time: string;
    }>;
    local_delete_list: BasicConfigKey[];
    result_list: Array<{
      type: 0 | 1 | 3;
      status: 0 | 1;
      key?: BasicConfigKey;
      msg?: string;
      update_time?: string;
    }>;
  };
}
```

Allowed keys:

```ts
type BasicConfigKey =
  | 'app_allowlist'
  | 'domain_allowlist'
  | 'blacklist';
```

Server validation requirements:

- Reject unknown keys with 400/422 before writing.
- Reject values that contain secrets, certificates, tokens, local port, system proxy state, Remote Invoke grants, shell policy, traffic history, or local machine identifiers.
- Store `value_json` as canonical JSON with stable key ordering before calculating `hash`.
- Enforce per-user ownership through the existing login token/session. Basic config is user-scoped only in V1; it is not group-scoped.
- Preserve optimistic sync semantics matching `/v4/env/sync`: client sends local check/update/delete lists; server returns remote updates and delete markers.
- Use the same timestamp comparison style as rules sync, but include `hash` in conflict metadata so clients can detect same timestamp/different content.

### Bifrost Cloud service changes

Bifrost Cloud lives in the current repo under `packages/bifrost-sync-server`.

Required implementation work:

- Add `BasicConfigKey`, `BasicConfigSnapshot`, `SyncBasicConfigReq`, and `SyncBasicConfigRes` to `src/types.ts`.
- Add an `IConfigDao` to `src/dao/types.ts` and expose it from `IStorage`.
- Implement `SqliteConfigDao` in `src/dao/sqlite.ts` and `MysqlConfigDao` in `src/dao/mysql.ts`.
- Add schema to both `sql/init-sqlite.sql` and `sql/init-mysql.sql`:
  - table name: `bifrost_basic_configs`
  - columns: `user_id`, `config_key`, `value_json`, `hash`, `create_time`, `update_time`
  - unique key: `(user_id, config_key)`
  - index: `user_id`
- Add `src/routes/config.ts` with `POST /v4/config/sync`.
- Register `handleConfig` in `src/index.ts` before Remote Invoke routes.
- Add `GET /v4/capabilities` returning at least:
  - `rules_sync`
  - `config_sync`
  - `group_rules` when group routes are enabled
  - `remote_invoke_relay` when `remote_invoke.enabled` is true
- Update `README.md` and `config.example.yaml` to document the new capability and storage table.
- Add tests next to existing sync-server tests:
  - config sync create/update/delete/check path
  - unknown key rejection
  - sensitive field rejection
  - SQLite and MySQL DAO coverage where practical
  - `/v4/capabilities` reflects Remote Invoke enabled/disabled state

Compatibility:

- Existing `/v4/env/sync`, user auth, OAuth2, group, and Remote Invoke endpoints stay unchanged.
- Existing SQLite/MySQL deployments need additive migrations. Do not require dropping existing remote-invoke tables.
- Old clients that do not call `/v4/config/sync` continue to work.

### ByteDance Internal service changes

ByteDance Internal lives in `/Users/eden_studio/work/github/bifrost-server-v4`. This repo must be changed and submitted as a separate PR when implementation starts.

Required implementation work:

- Add config-sync thrift definitions in `app/idl/env.thrift` or a new `app/idl/config.thrift`, and include the service from `app/idl/index.thrift`.
- Regenerate/update `typings/app/idl/*.ts` for the new request/response types.
- Add a controller method, either `ConfigController.SyncBasicConfig` or `EnvController.SyncBasicConfig`, exposing `POST /v4/config/sync`.
- Add `app/service/config.ts` or a clearly separated config-sync section in `app/service/env.ts`.
- Add a Sequelize model, suggested `app/model/basicConfig.ts`, mapped to `bifrost_server_basic_config`.
- Add DDL to `db.sql`:
  - table name: `bifrost_server_basic_config`
  - columns: `id`, `user_id`, `config_key`, `value_json`, `hash`, `create_time`, `update_time`
  - unique key: `(user_id, config_key)`
  - index: `user_id`
- Reuse existing SSO/session middleware and `ctx.session.user_id`; V1 config sync is only for the current user, not group virtual users.
- Add `GET /v4/capabilities` through IDL/controller so client detection can stop relying on fallback capability guesses.
- Return `config_sync` only after the new DB table and endpoint are deployed.
- Keep Remote Invoke routes in `app/routes/remoteInvoke.ts` unchanged except capability reporting.
- Add repo-local tests:
  - `SyncBasicConfig` happy path and conflict/check path
  - unknown/sensitive key rejection
  - session isolation between two users
  - `/v4/capabilities` returns `config_sync` and `remote_invoke_relay`

Compatibility:

- Existing `SyncEnv` in `app/service/env.ts` remains the rules sync implementation.
- Existing `bifrost_server_env` data is not migrated into config rows.
- Existing Remote Invoke Redis/SQL state is never serialized into config sync.
- Deployment must be additive and backward-compatible for old clients.

### Cross-service rollout contract

Implementation should land as one coordinated feature across three PRs:

1. Bifrost client/admin/provider PR in this repo.
2. Bifrost Cloud service PR in this repo under `packages/bifrost-sync-server`.
3. ByteDance Internal service PR in `/Users/eden_studio/work/github/bifrost-server-v4`.

The feature is considered ready only when all three are implemented and validated together. UI may show provider cards early, but `Config Sync` must stay disabled or marked unavailable for a server until `/v4/capabilities` advertises `config_sync` and `/v4/config/sync` passes an authenticated smoke test.

## Admin API

New endpoints should be additive. Existing endpoints stay compatible.

| Endpoint | Purpose |
| --- | --- |
| `GET /api/sync/providers/catalog` | Available provider types and detected managed providers |
| `GET /api/sync/providers` | Configured provider instances with status and non-secret config |
| `POST /api/sync/providers` | Add provider instance |
| `PATCH /api/sync/providers/{id}` | Update non-secret config, enabled, mode |
| `DELETE /api/sync/providers/{id}` | Remove provider instance and local session |
| `POST /api/sync/providers/{id}/auth/start` | Start login |
| `POST /api/sync/providers/{id}/auth/poll` | Poll device flow or complete pending auth |
| `POST /api/sync/providers/{id}/logout` | Clear provider session |
| `GET /api/sync/lanes` | Get lane routing |
| `PATCH /api/sync/lanes/{lane}` | Update lane routing |
| `GET /api/sync/registrations/remote-invoke` | Per-provider Remote Invoke registration state |
| `POST /api/sync/registrations/remote-invoke/refresh` | Register with all eligible Remote Invoke providers now |
| `POST /api/sync/run` | Run all enabled lanes |
| `POST /api/sync/run?lane=personal_rules` | Run one lane |
| `POST /api/sync/run?lane=basic_config` | Run basic config lane |

Compat:

- `GET /api/sync/status` returns aggregate status plus legacy `remote_base_url`.
- `PUT /api/sync/config` can still update the default `bifrost-server` provider URL during migration.
- `POST /api/sync/login` without provider id targets the selected or default `bifrost-server` provider.

## CLI

Provider commands:

```bash
bifrost sync provider catalog
bifrost sync provider detect
bifrost sync provider list
bifrost sync provider add
bifrost sync provider login
bifrost sync provider add github-gist --name personal-gist
bifrost sync provider add bifrost-server --name bifrost-cloud --url https://sync.example.com
bifrost sync provider enable personal-gist
bifrost sync provider disable personal-gist
bifrost sync provider remove personal-gist
bifrost sync provider status personal-gist
```

`bifrost sync provider add` and `bifrost sync provider login` without arguments start an interactive picker:

1. Choose `ByteDance Internal`, `Bifrost Cloud`, or `GitHub Gist`.
2. Show capability preview before login: Remote Invoke, Rules Sync, Config Sync.
3. Start the matching auth flow.
4. Offer to enable lanes after login.

Lane commands:

```bash
bifrost sync lane list
bifrost sync lane set personal-rules --read byte-work --write byte-work,bifrost-cloud,github-personal
bifrost sync lane set basic-config --read byte-work --write byte-work,bifrost-cloud,github-personal
bifrost sync lane set group-rules --read byte-work --write byte-work
bifrost sync lane set remote-invoke --providers byte-work,bifrost-cloud --registration all-eligible
```

Auth and run:

```bash
bifrost sync login --provider github-personal
bifrost sync login --provider byte-work
bifrost sync login --token "$TOKEN" --url https://sync.example.com
bifrost sync status --providers
bifrost sync status --providers --json
bifrost sync run
bifrost sync run --lane personal-rules
bifrost sync run --lane basic-config
bifrost sync remote-invoke register
bifrost sync logout --provider github-personal
```

Encryption:

```bash
bifrost sync encryption export-recovery-key --provider github-personal
bifrost sync encryption import-recovery-key --provider github-personal
```

## Web UI

Settings Sync should become provider-first. The page also serves as the sign-in hub for all sync services. The UI must be operational, not a landing page: users should be able to see connected services, connect a new provider, run sync, inspect Remote Invoke registration, and resolve conflicts from the same tab.

### Information architecture

`Settings -> Sync` is a single tab with four full-width sections:

1. `Sync Overview`: global sync controls and aggregate state.
2. `Connected Services`: a responsive card grid that always shows every currently supported sync provider type.
3. `Capability Routing`: editable lane routing for Rules Sync, Config Sync, Group Rules, and Remote Invoke.
4. `Add Sync Service`: provider catalog and setup entry points.

The tab should remain dense and settings-like. Use compact rows, capability chips, icon buttons with tooltips, and inline status instead of marketing copy. Cards represent provider instances, not decorative page sections.

### Supported provider card grid

The first implementation supports exactly three product provider types. The `Connected Services` area must render all three as first-class cards even when they are not signed in:

1. `ByteDance Internal`
2. `Bifrost Cloud`
3. `GitHub Gist`

The grid is the main sync management surface. It replaces the legacy single `Remote Sync` panel shown in earlier builds.

Card grid requirements:

- Desktop and wide tablet: at most three cards in one row, one card per supported provider type.
- Medium width: cards may wrap into two columns plus one card on the next row.
- Narrow width: cards stack into one column.
- The grid must not create an empty fourth column or placeholder card in V1.
- Cards should share the same height within a row and keep action buttons aligned.
- Each card is independently connectable, configurable, disconnectable, and syncable.
- A provider that is not configured still remains visible with capability chips and a primary `Sign in` or `Configure` action.
- Provider order is stable: ByteDance Internal, Bifrost Cloud, GitHub Gist.
- Future provider types can be added through provider catalog metadata, but V1 acceptance tests should assert that only these three cards are rendered by default.
- The legacy bottom `Remote Sync` card must not render once the provider card grid is available. Sign-in, sign-out, endpoint configuration, and provider capability status live on the provider cards.
- `Bifrost Cloud` owns a user-editable Remote URL input inside its card. Typing into this field must not be reset by status polling; saving the field updates the configured Bifrost Sync Server URL used for Bifrost Cloud login/sync.
- `ByteDance Internal` keeps the managed internal endpoint read-only. `GitHub Gist` keeps its non-URL remote identity read-only and does not expose Remote Invoke actions.

Suggested CSS contract:

- Use a CSS grid container with `grid-template-columns: repeat(auto-fit, minmax(280px, 1fr))` and cap the container at three columns through max column width or a `--sync-provider-grid-max-columns: 3` token.
- Each card has a stable minimum height and a footer action row with fixed-position alignment inside the card.
- Long endpoint, user, gist id, or error text wraps or ellipsizes within the card body; it must not push the footer outside the card.

### Sync Overview

Controls and fields:

- Global sync enabled switch.
- Auto sync switch.
- Aggregate status tag: Ready, Degraded, Needs sign in, Conflict, Disabled.
- Last sync time and last lane result.
- Connected services summary: ByteDance Internal, Bifrost Cloud, GitHub Gist, each showing signed-in user and last sync/registration state.
- Primary action buttons: `Sync Now`, `Add Service`, `Resolve Conflict` when conflicts exist.
- Secondary icon actions: refresh status, open logs/details.

Interaction rules:

- `Sync Now` opens a small menu: All lanes, Rules Sync, Config Sync, Group Rules. Remote Invoke registration has its own `Register Remote Invoke` action because it is live service registration, not data sync.
- Disabled global sync keeps provider login sessions but pauses lane runs.
- Degraded status shows which provider/lane failed and keeps healthy providers usable.

### First-run sign-in modal

When the user opens Settings Sync and no sync provider has a valid session:

1. Show a modal instead of silently leaving the page in local-only state.
2. Explain that logging in can sync the rule list and the allowed basic settings across devices.
3. Offer three choices as equal cards:
   - `ByteDance Internal`: Remote Invoke, Rules Sync, Config Sync.
   - `Bifrost Cloud`: Remote Invoke if supported by server, Rules Sync, Config Sync.
   - `GitHub Gist`: Rules Sync, Config Sync.
4. Let the user dismiss the modal and continue local-only; do not block Bifrost local proxy usage.
5. Do not auto-login any provider when the modal appears. ByteDance auto-detection may preselect or highlight the internal provider, but SSO must start only after the user clicks `Start`.
6. The modal must remember dismissal for the current browser session, but Settings Sync still shows an `Add Service` call to action until at least one provider is connected.

Modal layout:

- Title: `Set up sync`.
- Three provider choice cards in a responsive grid.
- Each provider card shows icon, provider name, short endpoint/account hint, and capability chips.
- Buttons: `Continue local only`, `Start`, and a close icon.
- ByteDance card can show `Detected` or `Not on internal network`; Bifrost Cloud and GitHub Gist cards show setup hints. Provider login or setup starts only after the selected card's `Start` action.

Modal interaction state machine:

1. Initial state: modal opens with the recommended provider preselected only when detection has a clear recommendation; otherwise no provider is selected and `Start` is disabled.
2. Close icon or `Continue local only`: dismisses the modal, keeps the user on Settings Sync, leaves all providers disconnected, and does not start any auth flow.
3. Provider card click: selects exactly one provider card and updates the footer summary to show what will happen next.
4. `Start` with `ByteDance Internal`: starts the internal SSO/login flow or opens the existing ByteDance login prompt. If internal detection is unavailable, show an inline unavailable state and keep the modal open.
5. `Start` with `Bifrost Cloud`: advances to the Bifrost Cloud setup wizard, starting with server URL entry or saved endpoint selection, then capability probe and login.
6. `Start` with `GitHub Gist`: starts the GitHub auth/setup wizard, including gist create/import and recovery-key setup.
7. Failed auth or cancelled setup returns to the same modal/provider card state, with an inline error and no provider marked connected.
8. Successful login closes the modal, marks that provider card connected in Settings Sync, and runs the first eligible Rules Sync and Config Sync probe according to capability.

### Connected Services

Show the supported provider card grid with compact capability badges:

- `ByteDance Internal`: Remote Invoke, Rules Sync, Config Sync, Groups.
- `Bifrost Cloud`: Remote Invoke when server supports it, Rules Sync, Config Sync, optional Groups.
- `GitHub Gist`: Rules Sync, Config Sync, Encrypted.

Each row/card:

- Display name, provider type, account/user.
- Health: Reachable, Unauthorized, Degraded, Not configured.
- Capability badges: Remote Invoke, Rules Sync, Config Sync, Groups, Encrypted.
- Remote Invoke registration state where supported.
- Actions: Sign in, Sign out, Sync now, Configure, Remove.

Provider card states:

| State | User-visible behavior |
| --- | --- |
| Not configured | Shows provider capabilities and a primary `Sign in` or `Configure` action. |
| Signing in | Shows progress, polling state, and cancel action. |
| Connected | Shows account, last sync, enabled lanes, and action buttons. |
| Needs sign in | Keeps config visible; primary action is `Reconnect`. |
| Degraded | Shows the failing lane/provider detail with retry action. |
| Conflict | Shows conflict count and `Resolve` action. |
| Disabled | Muted card, session retained, no auto sync. |

Provider card content requirements:

- ByteDance Internal card: detection status, signed-in user, Remote Invoke registration badge, `Re-detect` action.
- Bifrost Cloud card: editable server URL, capability probe result, signed-in user, Remote Invoke registration badge if supported.
- GitHub Gist card: GitHub user, gist id, encryption/recovery-key status, Rules Sync and Config Sync badges, no Remote Invoke controls.
- Future providers inherit the same card shell and declare capabilities through catalog metadata.

### Capability Routing

Rows by lane:

- Personal Rules: read/write provider chips, conflict policy.
- Basic Config: read/write provider chips and explicit synced fields.
- Group Rules: server provider only.
- Remote Invoke: eligible relay providers and registration policy.

Invalid provider choices are disabled with explanation text, not silently hidden in advanced routing dialogs.

Routing editor interaction:

- Each lane row has `Read from`, `Write to`, and `Policy` columns.
- Provider chips show provider icon, name, health, and capability support.
- Unsupported provider chips appear disabled with tooltip, for example `GitHub Gist does not support Remote Invoke`.
- Rules Sync and Config Sync can write to multiple providers; default secondary providers are mirrors.
- Remote Invoke uses registration policy `all eligible`; the row shows ByteDance Internal and Bifrost Cloud registration states separately.
- Editing opens a drawer, not a nested card. The drawer includes provider checkboxes, capability warnings, and a dry-run summary.
- Saving invalid routing is blocked client-side and server-side.

### Add Sync Service

Add provider cards:

- ByteDance Internal: detected/recommended state.
- Bifrost Cloud: URL input + login.
- GitHub Gist: Sign in with GitHub.

V1 should not show WebDAV, Dropbox, Google Drive, or custom file-provider cards in the default product UI. They may be introduced later through the provider catalog without changing the card shell.

### GitHub setup wizard

Steps:

1. Sign in with GitHub.
2. Choose `Create new secret gist` or `Use existing gist ID`.
3. Create or import encryption recovery key.
4. Preview capability: `Rules Sync + Config Sync`, no Remote Invoke.
5. Enable as primary or mirror.

### Bifrost Cloud setup wizard

Steps:

1. Enter server URL or choose a saved/self-hosted endpoint.
2. Probe `/v4/capabilities`; show Rules Sync, Config Sync, Group Rules, and Remote Invoke support before login.
3. Sign in through browser or paste token.
4. If Remote Invoke is supported, ask whether to register this device immediately.
5. Confirm lane routing; default to Rules Sync + Config Sync enabled, Remote Invoke registered if supported.

### ByteDance setup flow

Steps:

1. Auto-detect internal endpoint in the background.
2. Show card as `Detected` when reachable.
3. Opening the card starts the existing internal SSO prompt.
4. After login, enable Rules Sync, Config Sync, Group Rules, and Remote Invoke registration by default.
5. If Bifrost Cloud is already connected, keep it connected and register Remote Invoke to both eligible providers.

### Status and error surfaces

Settings Sync should have three levels of error presentation:

- Card-level inline error for provider-specific failures.
- Lane-level warning for routing or conflict problems.
- Modal/drawer for conflicts that need user choice.

Conflict resolution modal:

- Shows lane, provider, local update time, remote update time, remote device name, and affected object count.
- Actions: Pull remote, Push local, Save local as copy, Cancel.
- GitHub Gist conflicts mention that the provider has weak CAS and Bifrost re-read remote metadata before write.

Remote Invoke registration errors:

- Per provider registration state appears on the provider card and Remote Invoke lane row.
- If ByteDance succeeds and Bifrost Cloud fails, the UI shows `Partial registration` rather than failing the entire Sync tab.
- `Register Remote Invoke` retries only eligible providers.

### Responsive and theme requirements

- Desktop: overview controls in a compact two-column grid; provider cards use a responsive grid with equal-height cards.
- Narrow viewport: provider cards stack in one column; routing rows remain readable with horizontal provider chips wrapping onto a second line.
- Dark theme: all status tags, warning panels, disabled provider chips, and modal surfaces must use theme tokens and remain readable.
- Long server URLs, gist ids, account names, and error messages must wrap or use ellipsis with tooltip; they must not overlap action buttons.
- All primary actions must have stable `data-testid` hooks for Playwright.

### Test ids

Recommended stable selectors:

- `settings-sync-overview`
- `settings-sync-first-run-modal`
- `settings-sync-provider-grid`
- `settings-sync-provider-card-bytedance`
- `settings-sync-provider-card-bifrost-cloud`
- `settings-sync-provider-card-github-gist`
- `settings-sync-provider-capability-remote-invoke`
- `settings-sync-provider-capability-rules`
- `settings-sync-provider-capability-config`
- `settings-sync-routing-row-personal-rules`
- `settings-sync-routing-row-basic-config`
- `settings-sync-routing-row-remote-invoke`
- `settings-sync-remote-registration-status`

## Product Defaults

### First run inside ByteDance network

- Auto-detect ByteDance provider.
- Show it as recommended.
- If no provider has a valid session, open the Sync sign-in modal with ByteDance Internal highlighted.
- Route personal rules, basic config, group rules, and remote invoke to ByteDance after login.
- GitHub Gist remains optional mirror.
- Bifrost Cloud remains independently connectable; connecting it does not disconnect ByteDance.

### First run outside ByteDance network

- If no provider has a valid session, open the Sync sign-in modal with Bifrost Cloud and GitHub Gist as top options.
- GitHub Gist can be primary for personal rules and basic config only.
- Group Rules and Remote Invoke remain unavailable until ByteDance Internal or Bifrost Cloud is configured and signed in.

### Multiple providers enabled

- One primary per lane by default.
- Additional file providers are mirrors.
- Multi-read/multi-write is an advanced setting.
- Conflicts require explicit user action.
- Remote Invoke registration is not primary/mirror. It fans out to every enabled, signed-in provider with `remote_invoke_relay`.

### Basic config sync scope

V1 syncs only the settings users explicitly expect to follow their rule setup:

- Application allowlist.
- Domain allowlist.
- Blacklist or exclude list fields that affect rule/TLS matching.

V1 does not sync:

- CA certificates or trust state.
- Admin auth, proxy auth, passwords, tokens, API keys, OAuth secrets.
- System proxy enablement, local port, host binding, data dir, traffic history.
- Remote Invoke grants, pairing state, shell/file policies.
- IM provider credentials, Agent model provider secrets, local runner settings.

## Implementation Phases

### Phase 1: Provider model without behavior change

- Add provider registry, provider instance model, capabilities, and lane config.
- Wrap existing `SyncHttpClient` as `bifrost-server` provider.
- Migrate `remote_base_url` read path to default provider instance.
- Keep existing API/UI behavior.

### Phase 2: Service-side config sync protocol

- Add shared `/v4/config/sync` protocol and `/v4/capabilities` contract.
- Implement Bifrost Cloud service changes in `packages/bifrost-sync-server`.
- Implement ByteDance Internal service changes in `/Users/eden_studio/work/github/bifrost-server-v4`.
- Keep `config_sync` disabled for any provider whose capability probe does not advertise support.
- Validate both services before enabling the UI `Config Sync` badge as active.

### Phase 3: ByteDance detection and routing

- Add managed `bytedance-internal` provider.
- Add catalog/detect APIs.
- Route personal/group/remote invoke lanes through selected provider.
- Split Remote Invoke relay URL from generic Sync URL.
- Add first-run sign-in modal trigger when no provider session exists.

### Phase 4: UI and CLI provider management

- Add provider catalog UI and lane routing UI.
- Add connected-services display to Settings Sync.
- Add CLI provider/lane commands, including interactive `provider add` and `provider login`.
- Keep old commands as aliases.

### Phase 5: GitHub Gist provider

- Implement OAuth device flow.
- Implement gist discovery/create/update.
- Implement encrypted snapshot format and recovery key UX.
- Add conflict detection and user conflict resolution.
- Support both rules snapshot and basic config snapshot.

### Phase 6: Remote Invoke multi-registration

- Register local Remote Invoke service with every enabled signed-in provider that has `remote_invoke_relay`.
- Reconcile Remote Invoke workers after provider login, provider logout, and provider URL changes, so runtime state matches the connected provider set without requiring a process restart.
- Show registration status per provider in Settings Sync and CLI status.
- Keep outbound Remote Invoke provider selection explicit or priority-based.

### Phase 7: Coordinated release gate

- Merge and deploy Bifrost Cloud config sync support.
- Merge and deploy ByteDance Internal config sync support.
- Land Bifrost client/admin changes with capability gating.
- Enable `Config Sync` by default only for ByteDance Internal and Bifrost Cloud once their deployed `/v4/capabilities` returns `config_sync`.
- Keep GitHub Gist config sync independent because it stores encrypted snapshots in the user's gist.

### Phase 8: Additional providers

- Add WebDAV or Gitee as new provider type without modifying Sync Engine.
- Extend provider catalog metadata and tests.

## Test Plan

### Unit tests

- Config migration from `remote_base_url` to default provider instance.
- Capability validation rejects invalid lane routing.
- Capability validation rejects Remote Invoke lanes that include GitHub Gist or file providers.
- Basic config allowlist rejects unknown or sensitive config keys.
- GitHub provider payload encryption never serializes raw rule or config content.
- Gist conflict detection when remote revision changes.
- ByteDance detection handles 200, 401, timeout, TLS failure, DNS failure.
- Bifrost Cloud `/v4/config/sync` rejects unknown keys, rejects sensitive values, syncs create/update/delete/check lists, and isolates users.
- ByteDance Internal `SyncBasicConfig` rejects unknown keys, rejects sensitive values, syncs create/update/delete/check lists, and isolates sessions.
- Both services' `/v4/capabilities` advertise `config_sync` only after the endpoint and table are available.

### E2E tests

- Existing Bifrost Server sync behavior unchanged.
- ByteDance detected provider configures lanes only when no user-selected provider exists.
- First Settings Sync visit with zero provider sessions opens the sign-in modal and offers ByteDance Internal, Bifrost Cloud, and GitHub Gist.
- Settings Sync shows all connected services and each card's Remote Invoke, Rules Sync, and Config Sync capabilities.
- ByteDance Internal and Bifrost Cloud run authenticated Basic Config Sync for app allowlist, domain allowlist, and blacklist without syncing secrets or local machine state.
- GitHub Gist sync writes encrypted personal rules and basic config snapshots and never exposes group/remote invoke actions.
- ByteDance Internal and Bifrost Cloud can both be connected; Remote Invoke registration is attempted for both.
- Remote Invoke fails with clear message when no `remote_invoke_relay` provider is configured.
- CLI interactive `bifrost sync provider add` and `bifrost sync provider login` can add/login each provider type.
- CLI `bifrost sync status --providers` lists all provider session, sync, and Remote Invoke registration states.

### Web UI validation

Add focused Playwright coverage in `web/tests/ui/admin-settings.spec.ts` or a dedicated `settings-sync-provider.spec.ts`:

| Case | Mock setup | Assertions |
| --- | --- | --- |
| First-run modal | `/sync/providers` returns no sessions; ByteDance detection returns reachable | Modal is visible, three provider cards appear, ByteDance card is highlighted, close icon and `Continue local only` dismiss only for current browser session, and no auth API is called |
| First-run modal start flow | No provider session; provider cards are rendered; auth endpoints are mocked per provider | Selecting ByteDance then `Start` calls the internal SSO flow; selecting Bifrost Cloud then `Start` opens URL/capability/login wizard; selecting GitHub Gist then `Start` opens GitHub gist auth/setup; failed/cancelled auth returns to modal without marking connected |
| Supported provider card grid | Provider catalog returns V1 product providers | Exactly three cards render in stable order: ByteDance Internal, Bifrost Cloud, GitHub Gist; no WebDAV/custom placeholder appears; unconfigured providers still show capability chips and sign-in/configure actions |
| Connected services | ByteDance, Bifrost Cloud, GitHub Gist all signed in | Three provider cards render simultaneously; each shows account, status, Rules Sync, Config Sync; GitHub Gist has no Remote Invoke action |
| Capability routing | Personal rules and basic config write to all three; Remote Invoke providers are ByteDance + Bifrost Cloud | Routing rows show correct provider chips; GitHub Gist chip is disabled for Remote Invoke with explanatory tooltip |
| Remote Invoke partial registration | ByteDance registered, Bifrost Cloud failed | UI shows partial registration, retry action targets Bifrost Cloud, ByteDance remains registered |
| Bifrost Cloud setup | Capabilities probe returns Rules Sync + Config Sync + Remote Invoke | Wizard shows capability preview before login and offers immediate Remote Invoke registration |
| GitHub Gist setup | GitHub auth succeeds and gist id is returned | Wizard shows encrypted snapshot status, recovery key step, Rules Sync + Config Sync preview, no Remote Invoke controls |
| Basic config scope | Routing drawer opens Basic Config lane | Drawer lists app allowlist, domain allowlist, blacklist/exclude list; no secrets, certificates, tokens, local port, or Remote Invoke grant fields appear |
| Degraded/conflict states | Provider status includes conflict and last_error | Card-level error and conflict modal show lane/provider/object metadata and Pull/Push/Save copy actions |
| Dark theme | `bifrost-theme=dark` before page load | Provider cards, modal, disabled chips, warning panels, and routing drawer use dark tokens and text remains readable |
| Responsive layout | Viewports 390px, 820px, and 1280px | At 1280px the three provider cards occupy one row with at most three columns; at 820px cards wrap without overlap; at 390px cards stack into one column; long URLs/gist ids wrap or ellipsize with tooltip |

Visual/DOM checks:

- Use computed bounding boxes to assert no provider card text overlaps action buttons.
- Use card bounding boxes to assert no more than three provider cards share a row.
- Assert the V1 product UI renders exactly the ByteDance Internal, Bifrost Cloud, and GitHub Gist cards by default.
- Assert disabled unsupported capability chips are visible and have tooltips.
- Assert first-run modal does not re-open after dismissing within the same browser session, but `Add Service` remains visible.
- Assert closing/dismissing the modal never calls provider auth/start endpoints.
- Assert `Start` is disabled until a provider is selected when there is no recommended provider.
- Assert polling refresh updates provider status without resetting an in-progress Bifrost Cloud URL draft.

Manual human_tests:

- `human_tests/webui-settings.md` should gain implementation-phase cases for the real Settings Sync page.
- `human_tests/sync-provider-architecture.md` remains the design-review gate until implementation starts.

### human_tests

- `human_tests/sync-provider-architecture.md` covers design review and future implementation acceptance cases.

## Review / Fix / Test Loop

### Round 1

- Re-read user goals: multi-provider enabled, capability distinction, GitHub Gist, ByteDance auto-detection, Bifrost Cloud, basic config sync, Remote Invoke registration fan-out, first-run sign-in modal, extensibility, UI and CLI.
- Review config migration and capability routing for accidental single-provider assumptions.
- Verify design doc contains concrete API, CLI, UI, and phase plan.

### Round 2

- Re-check GitHub Gist boundaries: personal rules and basic config only, encrypted, no group or remote invoke.
- Re-check Remote Invoke uses only `remote_invoke_relay` providers.
- Re-check human_tests index and acceptance cases match this design.
- Re-check config sync scope only includes allowlisted Settings basics and excludes secrets/local machine state.

## Risks

- Multi-provider fan-out can create hard-to-debug conflict loops. Default product mode should be primary + mirrors, not arbitrary multi-write.
- GitHub Gist OAuth `gist` permission is broad for all user gists. UI must explain scope, and data must be encrypted.
- Secret gist is not access control for sensitive rule content; encryption is mandatory.
- Splitting Sync provider URL and Remote Invoke relay URL touches existing mental model and docs. Migration must keep old fields working during transition.
- Group Rules ACL semantics are server-specific. Do not attempt file-provider group sync in v1.

## Documentation Updates Required During Implementation

- `docs/cli.md` and `docs/cli-quick-start.md`: provider/lane commands and old command aliases.
- `human_tests/api-sync.md`: new provider/lane API cases.
- `human_tests/webui-settings.md`: provider catalog and routing UI cases.
- `human_tests/remote-invoke.md`: relay provider capability routing cases.
- `packages/bifrost-sync-server/README.md`: server capabilities endpoint if added.

## Implementation Update - 2026-07-07

This implementation pass lands the release foundation for provider-aware sync:

- Bifrost Cloud sync server now exposes `/v4/capabilities` and `/v4/config/sync`.
- Bifrost Cloud stores `app_allowlist`, `domain_allowlist`, and `blacklist` in a dedicated basic-config table for SQLite and MySQL.
- Bifrost Cloud rejects unsupported config keys and JSON payloads containing sensitive key names such as token/password/secret/credential.
- ByteDance Internal service has the matching IDL/controller/service/model/DDL plan and implementation in `/Users/eden_studio/work/github/bifrost-server-v4`.
- The Bifrost client sync manager now tracks basic config sync metadata in `sync-state.json` and syncs the three allowed basic config keys after a successful rule sync.
- Settings Sync now renders a provider card grid for ByteDance Internal, Bifrost Cloud, and GitHub Gist, capped at three cards per row by layout width and responsive down to one column. Cards use a wider desktop column so provider metadata and the Bifrost Cloud URL editor remain readable.
- Settings Sync first-run modal is dismissible. Selecting ByteDance Internal or Bifrost Cloud and pressing `Start` saves the provider URL and opens the existing login flow.
- ByteDance Internal and Bifrost Cloud sessions are persisted as independent provider sessions in `sync-state.json`, so both cards can stay connected at the same time.
- GitHub Gist token sign-in is implemented as the first usable login path. Users provide a GitHub token with `gist` scope; Bifrost validates it against GitHub `/user` and stores an independent `github_gist` provider session.
- Remote Invoke registration targets are resolved from all eligible connected providers. ByteDance-only, Bifrost Cloud-only, and ByteDance+Bifrost Cloud dual-channel startup/runtime login all register with the matching relay URL and provider session token. The runtime keeps one worker per eligible relay URL and stops workers whose provider session was removed.
- CLI `bifrost sync status` now lists provider status and capability flags.

Known remaining implementation gaps before claiming the full original product target:

- GitHub Gist OAuth/device flow, encrypted gist storage, conflict handling, and recovery key UX are not yet implemented. OAuth device flow can be layered onto the token session path once a Bifrost GitHub OAuth App client id is configured.
