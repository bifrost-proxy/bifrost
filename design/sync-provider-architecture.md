# Sync Provider Architecture

## Background

Bifrost 当前 Sync 体系把远端同步服务建模为一个全局 `remote_base_url`。这个模型能覆盖官方或自建 Bifrost Sync Server, 但无法表达以下新目标:

- 同时启用多个同步目标, 例如 ByteDance 内网服务 + GitHub Gist 个人备份。
- 区分 provider 能力: 有的只支持个人规则配置同步, 有的支持小组规则, 有的还能承担 Remote Invoke relay。
- 把 GitHub Gist 这类公开可信开发者服务纳入个人规则同步, 同时保留现有 Bifrost Sync Server 协议。
- 在 ByteDance 内网自动检测可用 provider, 检测通过后自动推荐或配置 `https://bifrost.bytedance.net`。
- 让未来 Gitee, WebDAV, OneDrive, Google Drive 等 provider 可以按同一扩展点接入。

因此本方案把 Sync 从 "one remote URL" 升级为 "provider registry + provider instances + capability routing + sync lanes"。

## Goals

### Must implement

- 引入 `Provider Type`: 一类同步服务实现, 例如 `bifrost-server`, `github-gist`, `webdav`。
- 引入 `Provider Instance`: 用户实际启用的一个账号或端点, 例如 `byte-work`, `github-personal`。
- 引入 `Capability`: provider 能力声明, 区分个人规则、小组规则、Remote Invoke relay、SSO、OAuth、端到端加密等。
- 引入 `Sync Lane`: 按数据/能力维度配置路由, 至少包含 `personal_rules`, `group_rules`, `remote_invoke`。
- 支持多个 provider instance 同时 enabled, 但每条 lane 独立声明读写目标和优先级。
- GitHub Gist provider 只承担个人规则配置同步, 不出现在小组规则和 Remote Invoke relay 路由中。
- Bifrost Server provider 继续兼容现有 Sync Server 协议, 并可声明小组规则和 Remote Invoke relay 能力。
- ByteDance 内网 provider 通过探测自动发现, 检测成功后作为 managed provider 推荐启用。
- 旧 `sync.remote_base_url` 迁移为默认 `bifrost-server` provider instance 的配置字段。

### Must not break

- 现有 `bifrost sync login --token --url` 仍可用, 映射到 `bifrost-server` provider。
- 现有 Admin API `/sync/status`, `/sync/config`, `/sync/login`, `/sync/run` 在兼容期继续返回旧字段。
- Remote Invoke relay 不再无条件读取规则同步 provider URL; 只有带 `remote_invoke_relay` capability 的 provider 才能被 lane 选中。
- Group Rules 不从 GitHub Gist/WebDAV 这类个人文件型 provider 读取。
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
| `bifrost-server` | 现有 Bifrost Sync Server 协议, 覆盖官方、内网、自建部署 | 规则、小组、Remote Invoke |
| `github-gist` | GitHub OAuth + secret gist 文件 | 个人规则配置同步 |
| `webdav` | WebDAV 文件存储 | 个人规则配置同步或备份 |
| `gitee-repo` | Gitee/GitCode 私有仓库文件 | 个人规则配置同步 |

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
| `personal_rules_config_sync` | 个人规则配置快照读写 |
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
      "write_to": ["byte-work", "github-personal"],
      "conflict_policy": "ask",
      "mirror_policy": "best_effort"
    },
    "group_rules": {
      "enabled": true,
      "read_from": ["byte-work"],
      "write_to": ["byte-work"]
    },
    "remote_invoke": {
      "enabled": true,
      "providers": ["byte-work"],
      "selection": "priority"
    }
  }
}
```

Default product rule:

- 普通用户默认只启用一个 primary provider。
- 多 provider 同时启用时, 第二个及之后默认作为 mirror, 防止多写冲突。
- 只有高级设置允许 multi-read / multi-write。

## Capability Matrix

| Provider | Personal rules | Group rules | Remote Invoke | Identity | Notes |
| --- | --- | --- | --- | --- | --- |
| ByteDance Internal | Yes | Yes | Yes | Byte SSO | 自动检测, managed, 默认推荐 |
| Bifrost Server | Yes | Optional | Optional | Bifrost token / OAuth2 | 官方或自建, 能力由 server `/v4/capabilities` 返回 |
| GitHub Gist | Yes | No | No | GitHub OAuth | secret gist + 客户端加密; 只同步个人规则 |
| WebDAV | Yes | No | No | Basic/app password | 国内和自部署 fallback |
| Gitee Repo | Yes | No | No | OAuth/PAT | 国内 Git provider 后续接入 |

UI 和 CLI 必须以此矩阵裁剪动作:

- 不支持 `remote_invoke_relay` 的 provider 不显示 Remote Invoke 入口。
- 不支持 `group_rules` 的 provider 不显示小组开关。
- 文件型 provider 默认显示 "Rules only"。

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
write_to = ["byte-work", "github-personal"]
conflict_policy = "ask"

[sync.lanes.group_rules]
enabled = true
read_from = ["byte-work"]
write_to = ["byte-work"]

[sync.lanes.remote_invoke]
enabled = true
providers = ["byte-work"]
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

### Group rules lane

Only providers with `group_rules` capability may participate. For v1, use one primary group provider. Multi-provider group sync is deferred because membership and ACL semantics are provider-specific.

### Remote Invoke lane

Remote Invoke is not a file sync operation. It routes live relay traffic to providers with `remote_invoke_relay`.

Selection:

- `priority`: first healthy provider wins.
- `manual`: user explicitly selects one provider.
- Future `failover`: active calls stay pinned; new pairings can switch after provider health failure.

Remote Invoke must not silently fall back to GitHub Gist, WebDAV, or any file provider.

## GitHub Gist Provider

### Product positioning

GitHub Gist is for developer-trusted personal rule sync and optional backup. It is not a team collaboration or Remote Invoke provider.

UI copy:

- Title: `GitHub Gist`
- Capability badge: `Rules only`
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
- File: `bifrost-sync.json`

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
- No raw rule content is uploaded to GitHub.

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

- `personal_rules_config_sync`
- `group_rules`
- `remote_invoke_relay`
- `identity_profile`
- `sso_browser_flow`
- `auto_detected`

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
| `POST /api/sync/run` | Run all enabled lanes |
| `POST /api/sync/run?lane=personal_rules` | Run one lane |

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
bifrost sync provider add github-gist --name personal-gist
bifrost sync provider add bifrost-server --name self-hosted --url https://sync.example.com
bifrost sync provider enable personal-gist
bifrost sync provider disable personal-gist
bifrost sync provider remove personal-gist
bifrost sync provider status personal-gist
```

Lane commands:

```bash
bifrost sync lane list
bifrost sync lane set personal-rules --read byte-work --write byte-work,github-personal
bifrost sync lane set group-rules --read byte-work --write byte-work
bifrost sync lane set remote-invoke --providers byte-work
```

Auth and run:

```bash
bifrost sync login --provider github-personal
bifrost sync login --provider byte-work
bifrost sync login --token "$TOKEN" --url https://sync.example.com
bifrost sync status --providers
bifrost sync run
bifrost sync run --lane personal-rules
bifrost sync logout --provider github-personal
```

Encryption:

```bash
bifrost sync encryption export-recovery-key --provider github-personal
bifrost sync encryption import-recovery-key --provider github-personal
```

## Web UI

Settings Sync should become provider-first.

### Section 1: Overview

- Global sync enabled switch.
- Auto sync switch.
- Aggregate status: Ready, Degraded, Needs sign in, Conflict, Disabled.
- Last sync time and last lane result.

### Section 2: Sync Targets

List enabled provider instances with compact capability badges:

- `ByteDance Sync`: Rules, Groups, Remote Invoke.
- `GitHub Gist`: Rules only, Encrypted.
- `Bifrost Server`: Rules, optional Groups, optional Remote Invoke.
- `WebDAV`: Rules backup.

Each row/card:

- Display name, provider type, account/user.
- Health: Reachable, Unauthorized, Degraded, Not configured.
- Capability badges.
- Actions: Sign in, Sign out, Sync now, Configure, Remove.

### Section 3: Capability Routing

Rows by lane:

- Personal Rules: read/write provider chips, conflict policy.
- Group Rules: server provider only.
- Remote Invoke: relay provider priority.

Invalid provider choices are disabled with explanation text, not silently hidden in advanced routing dialogs.

### Section 4: Provider Catalog

Add provider cards:

- ByteDance Sync: detected/recommended state.
- Bifrost Sync Server: URL input + login.
- GitHub Gist: Sign in with GitHub.
- WebDAV: URL + username + app password.

### GitHub setup wizard

Steps:

1. Sign in with GitHub.
2. Choose `Create new secret gist` or `Use existing gist ID`.
3. Create or import encryption recovery key.
4. Preview capability: `Personal rules only`.
5. Enable as primary or mirror.

## Product Defaults

### First run inside ByteDance network

- Auto-detect ByteDance provider.
- Show it as recommended.
- Route personal rules, group rules, and remote invoke to ByteDance.
- GitHub Gist remains optional mirror.

### First run outside ByteDance network

- Show Bifrost Server and GitHub Gist as top options.
- GitHub Gist can be primary for personal rules only.
- Group Rules and Remote Invoke remain unavailable until a Bifrost Server provider is configured.

### Multiple providers enabled

- One primary per lane by default.
- Additional file providers are mirrors.
- Multi-read/multi-write is an advanced setting.
- Conflicts require explicit user action.

## Implementation Phases

### Phase 1: Provider model without behavior change

- Add provider registry, provider instance model, capabilities, and lane config.
- Wrap existing `SyncHttpClient` as `bifrost-server` provider.
- Migrate `remote_base_url` read path to default provider instance.
- Keep existing API/UI behavior.

### Phase 2: ByteDance detection and routing

- Add managed `bytedance-internal` provider.
- Add catalog/detect APIs.
- Route personal/group/remote invoke lanes through selected provider.
- Split Remote Invoke relay URL from generic Sync URL.

### Phase 3: UI and CLI provider management

- Add provider catalog UI and lane routing UI.
- Add CLI provider/lane commands.
- Keep old commands as aliases.

### Phase 4: GitHub Gist provider

- Implement OAuth device flow.
- Implement gist discovery/create/update.
- Implement encrypted snapshot format and recovery key UX.
- Add conflict detection and user conflict resolution.

### Phase 5: Additional providers

- Add WebDAV or Gitee as new provider type without modifying Sync Engine.
- Extend provider catalog metadata and tests.

## Test Plan

### Unit tests

- Config migration from `remote_base_url` to default provider instance.
- Capability validation rejects invalid lane routing.
- GitHub provider payload encryption never serializes raw rule content.
- Gist conflict detection when remote revision changes.
- ByteDance detection handles 200, 401, timeout, TLS failure, DNS failure.

### E2E tests

- Existing Bifrost Server sync behavior unchanged.
- ByteDance detected provider configures lanes only when no user-selected provider exists.
- GitHub Gist sync writes encrypted personal rules snapshot and never exposes group/remote invoke actions.
- Remote Invoke fails with clear message when no `remote_invoke_relay` provider is configured.

### human_tests

- `human_tests/sync-provider-architecture.md` covers design review and future implementation acceptance cases.

## Review / Fix / Test Loop

### Round 1

- Re-read user goals: multi-provider enabled, capability distinction, GitHub Gist, ByteDance auto-detection, extensibility, UI and CLI.
- Review config migration and capability routing for accidental single-provider assumptions.
- Verify design doc contains concrete API, CLI, UI, and phase plan.

### Round 2

- Re-check GitHub Gist boundaries: personal rules only, encrypted, no group or remote invoke.
- Re-check Remote Invoke uses only `remote_invoke_relay` providers.
- Re-check human_tests index and acceptance cases match this design.

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
