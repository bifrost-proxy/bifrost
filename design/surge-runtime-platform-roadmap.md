# Surge Runtime Platform 未交付能力技术方案

## 状态

本文是评审用技术方案，覆盖 Surge 用户覆盖路线中尚未交付的 runtime/platform 能力。本文不把高影响 runtime 声明为已上线；截至本文档编写时，当前 PR 已具备 Profile 解析、兼容性报告、dry-run explain、禁用规则文件编译、WebUI Profile 工作台，以及 Bifrost Native Profile schema v1 的 `validate/effective` dry-run 基座。

已落地基座：

1. `RuntimePlanVersion`：包含 `plan_id`、`source_hash`、compiler version、mode、runtime plan、diagnostics 和 safety notes。
2. Bifrost Native Profile TOML schema v1：覆盖 `profile`、`policies`、`policy_groups`、`rules`、`dns`、`mitm` 和 `http_pipeline`。
3. CLI：`bifrost profile native validate <file>` 和 `bifrost profile native effective <file>`。
4. 当前 Native Profile 输出为 `dry-run-only`，不启用代理、不修改系统代理、不触发 DNS/MITM/TUN runtime。

待设计能力：

1. 动态 Policy Group runtime。
2. 真实 DNS / MITM / Rewrite / Script runtime。
3. Transparent Proxy / TUN / VIF。
4. UDP / QUIC / HTTP3 scheduling。
5. Team Profile。
6. Agent 自动迁移。

## 总体架构

```text
Profile Sources
  Surge / Bifrost Native / Team Profile / Managed URL
        |
        v
Profile IR + Compatibility Analyzer
        |
        v
Runtime Plan Compiler
        |
        +--> Policy Runtime Adapter
        +--> DNS Runtime Adapter
        +--> MITM Runtime Adapter
        +--> HTTP Pipeline Runtime Adapter
        +--> Transparent Network Adapter
        +--> Team Profile Sync Adapter
        +--> Agent Migration Adapter
        |
        v
Decision Timeline Store
        |
        v
CLI / Admin API / WebUI / Agent Tools
```

核心原则：

- Profile IR 是唯一入口。Surge 配置不得绕过 IR 直接写 runtime。
- 高影响能力默认 dry-run 或 disabled，必须有 preview、diff、rollback 和 scope。
- 每个 runtime decision 都写入 Traffic Decision Timeline，便于解释“为什么走这个代理/为什么没有走代理”。
- Surge-compatible mode 和 Bifrost-native mode 分开建模，不能把 Surge ordered semantics 静默降级成普通规则优先级。
- Team/Agent 能力只能编排 profile/runtime API，不能直接写底层文件绕过审计。

## 共同基础模块

### Runtime Plan Version

新增 `RuntimePlanVersion`，作为所有 adapter 的输入快照。当前已完成 dry-run 序列化基座，后续 activation/diff/rollback 会继续扩展字段：

- `plan_id`: 内容 hash + profile source + compiler version。
- `source_profile`: profile source、etag、last-modified、revision。
- `mode`: `surge-compatible` / `bifrost-native` / `team-managed`。
- `policy_graph`: policy、policy group、health config、selected override。
- `dns_plan`: host mapping、per-domain resolver、cache policy。
- `mitm_plan`: host/app/client/device scope、cert status requirement、quic policy。
- `http_pipeline_plan`: rewrite、map local、header/body operation、script binding。
- `network_plan`: system proxy / transparent proxy / tun / vif / udp / quic / h3。
- `safety`: required approvals、rollback token、dry-run notes。

`RuntimePlanVersion` 已可序列化并输出稳定 hash；仍需补齐可视化 diff、apply/activate/rollback token，并和 Traffic 记录建立引用关系。

### Decision Timeline

每条请求记录一条 normalized decision timeline：

```text
ingress -> dns -> rule -> policy -> mitm -> http_pipeline -> upstream -> traffic_record
```

建议结构：

- `request_id`
- `plan_id`
- `steps[]`
  - `stage`
  - `source_line`
  - `input`
  - `decision`
  - `reason`
  - `runtime_state`
  - `duration_ms`
  - `warnings`

WebUI Traffic、CLI `traffic get`、`profile explain` 和 Agent 都读取同一份 timeline。

## 迭代二：Bifrost Native Profile 推进方案

### 迭代目标

迭代二的目标是完成“替代 Surge”的日常使用闭环：用户可以把 Surge 配置迁移为 Bifrost Native Profile，并用 Bifrost 原生的 Policy、DNS、MITM、HTTP Pipeline 和 Traffic Decision Timeline 完成稳定代理工作流。

这一迭代不追求透明代理或团队 rollout 的平台化能力；重点是把当前 Bridge 的 dry-run / disabled preview 推进为可控启用的个人本地 runtime。

### 交付边界

必须交付：

- Bifrost Native Profile 文件格式、schema version、include、module、local override、managed profile。
- Native Profile effective dump、source diff、IR diff、runtime plan diff。
- Policy / Policy Group runtime：`direct`、`reject`、`proxy`、`select`、`fallback`、`url-test`。
- DNS Center：system/custom/per-domain DNS、local host mapping、DNS cache、flush、explain。
- MITM Center：host/app/client/device scope、证书 readiness、QUIC fallback/block 策略。
- HTTP Pipeline：URL rewrite、map local/mock、header/body rewrite、request/response script、decode/parser。
- Traffic Decision Timeline：每条请求记录 DNS、Rule、Policy、MITM、HTTP Pipeline 和 upstream decision。

暂不交付：

- Transparent Proxy / TUN / VIF。
- UDP / QUIC / HTTP3 完整代理调度。
- Team Profile 多人审批、灰度 rollout。
- Agent 自动迁移的自动 apply；迭代二只提供 migration draft 和 disabled apply。

### Native Profile 数据模型

```toml
version = 1
mode = "bifrost-native"

[metadata]
name = "personal-main"
description = "Daily proxy profile"

[[include]]
path = "./base.bifrost-profile"
required = true

[[module]]
path = "./modules/dev.bifrost-profile"
enabled = true

[override]
path = "./local.override.bifrost-profile"

[[policy]]
name = "ProxyA"
type = "proxy"
url = "http://127.0.0.1:8080"
capabilities = ["tcp", "http", "https"]

[[policy_group]]
name = "Auto"
type = "url-test"
candidates = ["ProxyA", "DIRECT"]
probe_url = "http://www.gstatic.com/generate_204"
interval_secs = 300

[[rule]]
match = { domain_suffix = "example.com" }
policy = "Auto"

[[dns.host]]
pattern = "api.local"
address = "127.0.0.1"

[[mitm.scope]]
host = "*.example.com"
action = "intercept"

[[http_pipeline]]
match = { url_regex = "^https://api\\.example\\.com/v1" }
phase = "request"
operation = { header = { set = { "X-Debug" = "1" } } }
```

### 编译与运行链路

```text
Native Profile source
  -> parser
  -> Profile IR
  -> compatibility/safety analyzer
  -> RuntimePlanVersion
  -> runtime adapters
  -> Decision Timeline
```

关键要求：

- Native parser 必须和 Surge parser 共享 Profile IR 的下游结构，避免两套 runtime。
- include/module/override 合并必须可解释，effective dump 要标注来源文件和行号。
- RuntimePlanVersion 一旦激活，不可被原地修改；所有变更产生新 plan_id。
- CLI/WebUI 激活 profile 前必须展示 safety summary，高影响能力默认需要显式确认。

### CLI / API / WebUI

CLI：

- `bifrost profile native validate <file>`
- `bifrost profile native effective <file>`
- `bifrost profile native diff <old> <new>`
- `bifrost profile native apply <file> --disabled`
- `bifrost profile native activate <name>`
- `bifrost profile policy list/select/probe`
- `bifrost profile dns explain/flush`
- `bifrost profile mitm explain/status`
- `bifrost profile pipeline preview/apply`

Admin API：

- `POST /api/profile/native/validate`
- `POST /api/profile/native/effective`
- `POST /api/profile/native/diff`
- `POST /api/profile/native/apply`
- `POST /api/profile/native/activate`
- `GET /api/profile/runtime/current`

WebUI：

- Profile Editor：source / effective / diff 三栏。
- Policy Center：策略组状态、手动切换、probe history。
- DNS Center：resolver、cache、flush、explain。
- MITM Center：scope、CA readiness、QUIC policy。
- Pipeline Center：rewrite/script/mock 列表、dry-run explain、启用状态。

### 迭代二里程碑

1. Native Profile schema + parser + effective dump。
2. RuntimePlanVersion apply/activate/rollback。
3. PolicyRuntime select/fallback/url-test。
4. HTTP Pipeline runtime activation。
5. DNS Center runtime。
6. MITM Center runtime。
7. Traffic Decision Timeline 打通 CLI/WebUI。
8. Surge -> Native migration draft + disabled apply。

### 迭代二验收标准

- 用户可以用一个 Native Profile 启动日常代理，不依赖 Surge 原文件。
- `profile native diff` 能看出 source、IR、runtime plan 三层差异。
- Policy Group 切换和健康探测真实影响请求。
- DNS/MITM/Pipeline 的真实执行结果都进入 Traffic Decision Timeline。
- 高影响能力未确认时不能自动启用。

## 迭代三：Bifrost Proxy Platform 推进方案

### 迭代目标

迭代三的目标是完成“反超 Surge”的平台化能力：Bifrost 不只是 profile runtime，而是跨设备、透明代理、团队协作、远程诊断和 Agent 自动化的代理平台。

这一迭代建立在迭代二的 Native Profile runtime 之上，不再直接扩展单机配置文件语义，而是扩展网络入口、传输协议、团队治理和自动化能力。

### 交付边界

必须交付：

- Transparent Proxy / TUN / VIF。
- UDP / QUIC / HTTP3 policy scheduling。
- Multi-device Proxy Fabric。
- Team Profile revision、diff、approval、rollback、audit、rollout target。
- Agent Automation：迁移、解释、生成、验证、远程诊断。
- 平台级 observability：跨设备 timeline、runtime health、policy health、network health。

暂不交付或需单独评审：

- OS kernel extension 级深度集成默认开启。
- 未审批的团队强制 MITM。
- Agent 自动启用高风险能力。
- 跨设备私钥/证书自动分发。

### 平台架构

```text
Device Runtime
  HTTP/SOCKS ingress
  Transparent/TUN/VIF ingress
  UDP/QUIC/H3 ingress
        |
        v
RuntimePlanVersion
        |
        v
Policy/DNS/MITM/Pipeline/Transport Runtime
        |
        v
Local Timeline + Health
        |
        v
Team Control Plane / Remote Invoke / Agent Automation
```

### Control Plane 数据模型

```rust
struct PlatformDevice {
    device_id: String,
    owner: String,
    labels: BTreeMap<String, String>,
    capabilities: Vec<DeviceCapability>,
    active_plan_id: Option<String>,
    health: DeviceHealth,
}

struct RolloutTarget {
    selector: BTreeMap<String, String>,
    percentage: Option<u8>,
    require_approval: bool,
}

struct PlatformRuntimeEvent {
    event_id: String,
    device_id: String,
    plan_id: String,
    category: String,
    severity: String,
    message: String,
    created_at_ms: i64,
}
```

### 关键能力拆分

Transparent / TUN / VIF：

- 负责捕获未显式配置代理的应用流量。
- 必须有 rollback plan、stale state recovery、platform-specific adapter。
- 初期建议 Linux TUN 实验优先，macOS Network Extension 和 Windows WinTun 分别做 prototype。

UDP / QUIC / HTTP3 scheduling：

- Transport metadata 进入 PolicyRuntime。
- 候选 policy 需要声明 transport capabilities。
- QUIC 与 MITM 冲突时默认 diagnose，用户显式选择 block/fallback。

Multi-device Proxy Fabric：

- 设备注册、能力上报、健康心跳。
- Profile rollout 按 device label/owner/env 灰度。
- Remote Invoke 复用现有远程执行与流量诊断能力。

Team Profile：

- Team Profile 是控制平面的配置治理模型。
- 所有 revision 都绑定 RuntimePlanVersion hash。
- 高影响能力必须 approval。
- rollout/rollback 必须记录 audit。

Agent Automation：

- Agent 只生成 migration task、draft、disabled apply 和 verification evidence。
- Agent 可通过 Remote Invoke 对远端设备收集 evidence。
- 高风险变更必须进入 Team approval。

### CLI / API / WebUI

CLI：

- `bifrost platform device list/status`
- `bifrost platform rollout create/status/rollback`
- `bifrost transparent status/enable/disable`
- `bifrost transport explain <host> --protocol udp|quic|h3`
- `bifrost team profile create/diff/approve/rollout/rollback`
- `bifrost profile migrate analyze/plan/apply --disabled`

Admin API：

- `GET /api/platform/devices`
- `POST /api/platform/rollouts`
- `POST /api/platform/rollouts/:id/rollback`
- `GET /api/platform/events`
- `POST /api/team-profiles/:id/revisions`
- `POST /api/team-profiles/:id/revisions/:rev/approve`
- `POST /api/profile/migration/plan`

WebUI：

- Platform Devices：设备、能力、健康、当前 plan。
- Rollout Center：灰度、审批、回滚。
- Transparent Network Center：平台入口状态和 rollback。
- Transport Diagnostics：UDP/QUIC/H3 timeline。
- Agent Migration Workbench：任务、证据、阻塞项、apply-disabled。

### 迭代三里程碑

1. Platform device registry + health。
2. Team Profile local revision/diff/audit。
3. Rollout target + rollback。
4. Transparent/TUN experimental ingress。
5. UDP relay + QUIC/H3 metadata scheduling。
6. Multi-device timeline aggregation。
7. Agent migration task system。
8. Agent remote diagnosis and verification loop。

### 迭代三验收标准

- 多设备能上报 active plan、capabilities、health。
- Team Profile revision 可审批、灰度、回滚。
- 透明代理入口启停具备 rollback，不污染系统网络。
- UDP/QUIC/H3 请求有 transport-aware policy decision。
- Agent 迁移生成的每个动作都有 evidence，且高风险动作不能绕过审批。

## 1. 动态 Policy Group Runtime

### 目标

让 Surge `select`、`fallback`、`url-test`、未来 `load-balance` 等策略组从 dry-run graph 进入真实 runtime：

- `select`: 用户可手动切换，切换即时生效并持久化。
- `fallback`: 按健康状态选择第一个可用 policy。
- `url-test`: 周期性测速，选择 latency 最低 policy。
- group 嵌套、缺失成员、cycle、健康状态都可解释。

### 模块设计

- `bifrost-core::profile::policy_graph`: 纯数据图，已由 Profile compiler 生成。
- `bifrost-proxy::policy_runtime`: 请求时解析 terminal upstream。
- `bifrost-admin::policy_state`: 管理 selected policy、health snapshot、override。
- `bifrost-admin::policy_probe`: 周期探测 url-test/fallback。
- `web/src/pages/ProfilePolicy`: 策略组状态、手动切换、探测历史。

### 数据模型

```rust
struct PolicyRuntimeState {
    plan_id: String,
    groups: BTreeMap<String, PolicyGroupState>,
}

struct PolicyGroupState {
    name: String,
    group_type: String,
    selected: Option<String>,
    candidates: Vec<PolicyCandidateState>,
    last_decision: Option<PolicyDecisionSnapshot>,
    health: Vec<PolicyHealthSnapshot>,
}

struct PolicyHealthSnapshot {
    policy: String,
    status: HealthStatus,
    latency_ms: Option<u64>,
    checked_at_ms: i64,
    error: Option<String>,
}
```

### 执行链路

1. Proxy 收到请求并命中 rule。
2. Rule 得到 requested policy。
3. `PolicyRuntime::resolve(plan_id, requested_policy, request_context)` 返回 terminal policy。
4. 若 terminal policy 是 proxy endpoint，交给 upstream connector。
5. resolver 记录 policy decision timeline。

### API / CLI

- `GET /api/profile/policy-groups`
- `GET /api/profile/policy-groups/:name`
- `POST /api/profile/policy-groups/:name/select`
- `POST /api/profile/policy-groups/:name/probe`
- `bifrost profile policy list`
- `bifrost profile policy select <group> <policy>`
- `bifrost profile policy probe <group>`

### 迭代拆分

1. `select` runtime：手动选择、持久化、timeline。
2. `fallback` runtime：健康状态 + 失败切换。
3. `url-test` runtime：周期探测 + latency sort。
4. group 嵌套和 cycle 的 runtime hardening。
5. WebUI policy center。

### 验收标准

- 切换 select 后，同一 host 下一次请求走新 policy。
- fallback 候选不可达时自动跳到下一个可用 policy。
- url-test 探测结果变化后 terminal policy 可变化。
- Traffic timeline 能解释 selected/health/latency/cycle/missing member。

## 2. 真实 DNS / MITM / Rewrite / Script Runtime

### 目标

把当前 dry-run DNS/MITM/HTTP Pipeline explain 推进为真实执行，但保持安全默认：

- DNS: host mapping、per-domain resolver、cache、flush、explain。
- MITM: host/app/client/device scope、证书状态、QUIC fallback/block。
- Rewrite: URL rewrite、map local、header/body rewrite、mock。
- Script: request/response/decode/parser 脚本绑定、权限、日志和回滚。

### DNS Runtime

新增 `DnsRuntimeAdapter`：

- 输入：`dns_plan` + request host + client context。
- 输出：resolved IP / resolver used / cache hit / fallback reason。
- 支持 host mapping 优先级：
  1. Profile `[Host]` exact / wildcard。
  2. Bifrost Native per-domain resolver。
  3. system/custom default resolver。
  4. fallback resolver。

API：

- `GET /api/profile/dns/status`
- `POST /api/profile/dns/flush`
- `POST /api/profile/dns/explain`
- `bifrost profile dns explain <host>`
- `bifrost profile dns flush`

### MITM Runtime

新增 `MitmRuntimeAdapter`：

- 输入：`mitm_plan` + TLS client hello + app/client/device context。
- 输出：intercept / passthrough / blocked-quic / fallback-to-tcp。
- 强制检查 CA readiness，未信任 CA 时禁止自动开启拦截。
- QUIC policy：
  - `allow`: 不拦截 QUIC。
  - `block`: 阻断 QUIC，迫使 HTTP/3 fallback 到 TCP/TLS。
  - `diagnose`: 仅记录建议，不阻断。

API：

- `GET /api/profile/mitm/status`
- `POST /api/profile/mitm/explain`
- `POST /api/profile/mitm/scope`
- `bifrost profile mitm explain <host>`

### HTTP Pipeline Runtime

新增 `HttpPipelineRuntimeAdapter`：

- `url_rewrite`: 请求前改 URL 或直接 redirect。
- `map_local`: 直接返回本地文件。
- `header_rewrite`: request/response header 操作，必须明确 phase。
- `body_rewrite`: request/response body 操作，受 body size strategy 限制。
- `script`: request/response/decode/parser，必须引用已导入脚本。

Surge 到 Bifrost 的激活策略：

- 安全集自动编译为 disabled rule，用户启用后生效。
- 高风险项进入 migration task，不直接启用：
  - Header Rewrite phase 不明确。
  - Script 文件未导入或权限未知。
  - Body rewrite 可能改变二进制/大 body。
  - MITM 依赖未满足。

API：

- `POST /api/profile/pipeline/preview`
- `POST /api/profile/pipeline/apply`
- `POST /api/profile/pipeline/explain`
- `bifrost profile pipeline preview <profile>`
- `bifrost profile pipeline apply <profile> --scope disabled|enabled`

### Script Runtime

脚本迁移不能只写规则引用，还要管理脚本资产：

- 解析 Surge `script-path` / `script-update-interval` / `requires-body`。
- 本地路径脚本导入到 `scripts/request` 或 `scripts/response`。
- 远程脚本下载并记录 etag/hash。
- 扫描脚本能力：file/net/body/headers/status。
- 给出 sandbox permission diff。
- 脚本运行失败写入 timeline，不应让代理进程崩溃。

### 验收标准

- DNS host mapping 真实影响上游连接，并可在 timeline 查看。
- MITM scope 改动只影响匹配 host/app/client，CA 未 ready 时不能启用。
- URL Rewrite / Map Local / Header Rewrite / Script 均能在真实请求中生效并记录 timeline。
- Script 导入失败、权限不足、运行异常都有可解释错误。

## 3. Transparent Proxy / TUN / VIF

### 目标

让 Bifrost 覆盖不显式配置 HTTP/SOCKS 代理的应用流量，并支持多设备/系统级代理场景。

### 平台分层

- macOS:
  - Network Extension / Packet Tunnel Provider 作为长期方向。
  - 初期可用 PF rdr + loopback capture 做受控实验，但必须隔离权限和 rollback。
- Windows:
  - WinTun / WFP callout 分阶段。
  - 初期优先 WinTun 用户态 adapter。
- Linux:
  - TUN + iptables/nftables mark/routing table。

### 模块设计

- `bifrost-netif`: 跨平台 network interface abstraction。
- `bifrost-tun`: TUN packet capture、TCP/UDP sessionization。
- `bifrost-transparent`: system route/firewall integration。
- `bifrost-proxy::transparent_ingress`: 把 L3/L4 flow 转成 proxy request context。
- `bifrost-admin::network_profile`: 保存 VIF/TUN 配置、状态、rollback token。

### 数据模型

```rust
struct TransparentProfile {
    id: String,
    enabled: bool,
    platform: PlatformKind,
    mode: TransparentMode,
    include_apps: Vec<String>,
    exclude_apps: Vec<String>,
    include_cidrs: Vec<String>,
    exclude_cidrs: Vec<String>,
    dns_capture: bool,
    udp_capture: bool,
    rollback: RollbackPlan,
}
```

### 安全策略

- 默认不开启。
- 启用前必须生成 rollback plan。
- 每次启动检查 stale route/firewall state。
- 失败时自动回滚，不得留下系统网络不可用状态。
- human_tests 必须包含“崩溃后恢复”和“用户取消授权”。

### 迭代拆分

1. Linux TUN 实验实现，验证 packet sessionization。
2. macOS/Windows 只做状态面和 rollback 设计，不启用默认入口。
3. macOS Network Extension 原型。
4. Windows WinTun 原型。
5. 统一 WebUI Transparent Proxy Center。

### 验收标准

- 不配置浏览器代理时，目标应用流量可进入 Bifrost。
- 启停不会污染系统代理、路由、DNS。
- Traffic 能标记 ingress 为 `transparent/tun/vif`。
- 崩溃重启能自动清理或恢复透明代理状态。

## 4. UDP / QUIC / HTTP3 Scheduling

### 目标

让 Policy Engine 能处理 UDP、QUIC 和 HTTP/3，不再只服务 HTTP/1/2/TCP 代理链路。

### 核心难点

- UDP 是无连接语义，需要 flow idle timeout。
- QUIC 加密后只能基于 SNI/ALPN/5-tuple/ClientHello-like metadata 做策略。
- HTTP/3 upstream 需要连接池、0-RTT 策略和 fallback。
- MITM 与 QUIC 天然冲突，必须支持 block/fallback/diagnose。

### 模块设计

- `bifrost-proxy::udp`: UDP ingress/egress relay。
- `bifrost-proxy::quic`: QUIC metadata extraction and routing。
- `bifrost-proxy::http3`: h3 upstream connector。
- `PolicyRuntime::resolve_transport`: 输入 transport metadata。
- `QuicPolicy`: allow/block/fallback/diagnose。

### Scheduling 策略

- TCP/HTTP policy 和 UDP/QUIC policy 使用同一 policy graph，但 transport capability 必须参与过滤。
- 若 terminal proxy 不支持 UDP，fallback 到下一个支持 UDP 的 candidate。
- 若 MITM scope 命中 HTTP/3，按 plan 执行：
  - block QUIC -> TCP fallback。
  - passthrough QUIC -> 不解密，仅记录。
  - diagnose -> 记录建议，不干预。

### API / CLI

- `GET /api/profile/transports`
- `POST /api/profile/transports/explain`
- `bifrost profile transport explain <host> --protocol udp|quic|h3`
- `bifrost traffic list --protocol h3`

### 验收标准

- UDP flow 可以按 policy group 选择支持 UDP 的 upstream。
- HTTP/3 请求可被记录为 h3，并展示 policy decision。
- QUIC block/fallback 能触发 TCP 重试并可解释。
- 不支持 UDP 的 policy 不会被错误选择。

## 5. Team Profile

### 目标

把个人 profile 文件升级为团队代理配置平台，支持 revision、diff、approval、rollback、audit 和 rollout target。

### 数据模型

```rust
struct TeamProfile {
    team_id: String,
    profile_id: String,
    name: String,
    current_revision: String,
    draft_revision: Option<String>,
    visibility: TeamProfileVisibility,
    rollout: RolloutPolicy,
}

struct TeamProfileRevision {
    revision_id: String,
    parent_revision_id: Option<String>,
    author: String,
    content_hash: String,
    ir_hash: String,
    runtime_plan_hash: String,
    compatibility_summary: CompatibilitySummary,
    created_at_ms: i64,
}

struct TeamProfileAuditEvent {
    event_id: String,
    actor: String,
    action: String,
    revision_id: String,
    diff_summary: String,
    created_at_ms: i64,
}
```

### 功能设计

- Draft 编辑：上传 profile、修改 Native profile、生成 runtime plan preview。
- Diff：source diff、IR diff、runtime plan diff、compatibility diff。
- Approval：高影响能力需要审批，例如 MITM/TUN/System DNS。
- Rollout：按设备、用户、环境、百分比灰度。
- Rollback：按 revision 回滚 runtime plan。
- Audit：所有变更和启用动作写审计日志。

### API / CLI

- `GET /api/team-profiles`
- `POST /api/team-profiles`
- `POST /api/team-profiles/:id/revisions`
- `GET /api/team-profiles/:id/revisions/:rev/diff`
- `POST /api/team-profiles/:id/revisions/:rev/approve`
- `POST /api/team-profiles/:id/rollout`
- `POST /api/team-profiles/:id/rollback`
- `bifrost team profile diff <id> <rev-a> <rev-b>`
- `bifrost team profile rollout <id> --target <selector>`

### 权限边界

- Profile author 不能直接启用高影响 runtime。
- Reviewer 不能审批自己提交的高影响变更。
- Device agent 只能拉取被授权的 rollout target。
- 本地用户可以临时 disable team profile，但要写 audit。

### 验收标准

- 同一个 profile 的 source diff 和 runtime plan diff 可审阅。
- 审批前不能 rollout MITM/TUN 类高影响变更。
- 设备按 target 拉取正确 revision。
- rollback 后新请求使用旧 runtime plan，timeline 引用旧 plan_id。

## 6. Agent 自动迁移

### 目标

让 Agent 成为 Surge -> Bifrost 迁移助手，但所有改动都必须可解释、可验证、可回滚。

### Agent 能力边界

Agent 可以：

- 分析 compatibility report。
- 生成迁移计划和风险分级。
- 生成 Bifrost Native Profile 草稿。
- 生成 disabled rules / scripts / values。
- 调用 explain、E2E、traffic replay 验证。
- 给出差异报告和建议。

Agent 不可以：

- 默认启用 MITM/TUN/System proxy。
- 未经用户确认导入或执行远程脚本。
- 绕过 Team Profile approval。
- 静默删除 Surge 不支持项。

### 工作流

```text
analyze -> plan -> generate draft -> simulate -> verify -> propose apply -> apply disabled -> observe -> promote
```

1. `analyze`: 读取 profile、compat report、traffic samples。
2. `plan`: 生成 migration tasks。
3. `generate draft`: 输出 Native Profile / disabled rules / script import plan。
4. `simulate`: 对关键 URL 做 `profile explain` 对比。
5. `verify`: 运行 E2E 或 replay。
6. `propose apply`: 给用户/团队审批。
7. `apply disabled`: 默认只保存 disabled。
8. `observe`: 启用后观察 traffic decision timeline。
9. `promote`: 通过后进入 Team Profile rollout。

### 数据模型

```rust
struct MigrationTask {
    id: String,
    source_line: Option<usize>,
    capability: String,
    risk: MigrationRisk,
    action: MigrationAction,
    status: MigrationStatus,
    evidence: Vec<MigrationEvidence>,
}

struct MigrationEvidence {
    kind: String,
    command: String,
    result: String,
    artifact: Option<String>,
}
```

### API / CLI

- `POST /api/profile/migration/analyze`
- `POST /api/profile/migration/plan`
- `POST /api/profile/migration/apply-disabled`
- `POST /api/profile/migration/verify`
- `bifrost profile migrate analyze <profile>`
- `bifrost profile migrate plan <profile>`
- `bifrost profile migrate apply <plan> --disabled`

### 验收标准

- Agent 输出的每个迁移动作都能追溯到 source line 或 compatibility item。
- 所有高风险动作默认 disabled 或 pending approval。
- Agent 生成的 Native Profile 可 diff、可 explain、可回滚。
- 迁移验证失败时不会削弱断言，而是生成 blocker。

## 推荐交付顺序

### Milestone A：Runtime Plan Activation Foundation

- RuntimePlanVersion dry-run 基座。已完成：Native Profile `validate/effective` 输出稳定 plan id、diagnostics 和 safety summary。
- RuntimePlanVersion apply/activate/rollback。
- Decision Timeline Store。
- PolicyRuntime select。
- HTTP Pipeline URL Rewrite / Map Local activation。

价值：把当前 Bridge 从“审阅工具”推进到“可控启用”。

### Milestone B：Policy + HTTP Pipeline Replacement

- fallback/url-test health store。
- Header Rewrite 精确映射。
- Script import and sandbox check。
- WebUI Policy/Pipeline Center。

价值：覆盖 Surge 日常代理配置主路径。

### Milestone C：DNS/MITM Runtime

- DNS Center。
- MITM Center。
- QUIC fallback/block policy。
- CA readiness and scope guard。

价值：覆盖 Surge 高价值能力，但保持安全边界。

### Milestone D：Transparent + UDP/H3

- Linux TUN experimental。
- UDP relay and policy scheduling。
- HTTP/3 upstream connector。
- macOS/Windows adapter prototype。

价值：进入平台级代理能力。

### Milestone E：Team + Agent

- Team Profile revision/diff/approval/rollout。
- Agent migration tasks。
- Agent verification loop。

价值：从个人工具升级为团队代理平台。

## 测试策略

### 单元测试

- Policy graph resolution: select/fallback/url-test/cycle/missing member。
- RuntimePlanVersion diff/hash/rollback。
- DNS host mapping and resolver precedence。
- MITM scope matching and QUIC policy。
- HTTP pipeline operation compilation and phase matching。
- Team Profile revision/diff/audit。
- MigrationTask risk classification。

### E2E 测试

- 真实请求走 select 切换后的 policy。
- fallback 候选失败后切换。
- url-test 低延迟候选获选。
- DNS host mapping 影响上游连接。
- MITM scope 只影响目标 host/app。
- URL Rewrite / Map Local / Header Rewrite / Script 在真实请求中生效。
- UDP flow 选择支持 UDP 的 policy。
- HTTP/3 timeline 记录 h3。
- Team Profile rollout / rollback。
- Agent migration apply-disabled + verify。

### human_tests

- 高影响开关默认 disabled。
- 启用 MITM/TUN 前有明确 scope 和 rollback。
- Team approval 阻止未审批高风险 rollout。
- Agent 自动迁移不能静默启用高风险能力。
- Traffic 页面能解释完整 decision timeline。

## 风险与决策点

| 风险 | 决策点 | 推荐策略 |
| --- | --- | --- |
| TUN/VIF 平台差异大 | 是否先做跨平台 | 先 Linux 实验，再 macOS/Windows 原型 |
| MITM/QUIC 冲突 | 默认 block 还是 diagnose | 默认 diagnose，用户显式 block |
| Script 权限风险 | 是否自动导入启用 | 自动导入草稿，默认 disabled |
| Team Profile 权限复杂 | 是否先做本地团队模型 | 先本地 audit/revision，再接远端 sync |
| Agent 误操作 | 是否允许自动 apply | 只允许 apply-disabled，高风险需审批 |
| Policy health 抖动 | 切换阈值 | latency hysteresis + cooldown |

## 评审问题

1. Policy Group runtime 是否优先支持 Surge-compatible mode，还是直接定义 Bifrost Native Policy 作为唯一 runtime？
2. MITM Center 是否必须在 Team Profile 前上线，还是 Team Profile 先提供审批框架？
3. TUN/VIF 第一平台选择 Linux、macOS 还是 Windows？
4. Agent 自动迁移是否允许在本地个人模式下自动启用低风险规则？
5. Team Profile 是否依赖现有 Sync 服务，还是先实现本地 revision/audit store？
