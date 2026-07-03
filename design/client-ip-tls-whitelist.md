# Client IP TLS Whitelist 基于客户端 IP 的 TLS 解包控制设计方案

## 背景

Bifrost 的 TLS 拦截（解包）能力此前只支持三个维度：全局开关、域名 include/exclude、应用（进程名）include/exclude。这三个维度都以“流量目标”或“本地发起进程”为主键，无法回答“来自某台远端设备的所有流量”这类客户端视角的问题。

在真实使用场景中，用户经常希望：

- 电脑给公司里其他同学 / 手机 / 平板做代理，只对指定的几个 client IP 做 TLS 解包，其他 client IP 走透传，避免全网被拦截。
- 内部测试时，把某个 CI runner 或者手机的固定 IP 加入 include，让其他远端设备默认不解包。
- 从代理 CONNECT 流量的 Response 空状态里，一键把当前请求的 `client_ip` 加入解包名单，而不用回到 Settings 手动输入。

同时，如果远端 IP 首次出现（例如新手机第一次连代理），用户没有任何提示，容易误以为 TLS 拦截没生效。本方案引入 **client IP 维度**，并配套一个交互式“新 IP 决策弹窗”，让用户在第一次看到未决策的 IP 时选择“启用 TLS 解包”或“跳过”。

本方案实现在以下位置：

- 数据模型：`crates/bifrost-storage/src/unified_config.rs::TlsConfig`
- 决策链：`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 的 `is_ip_included` / `is_ip_excluded`
- Pending 管理：`crates/bifrost-admin/src/ip_tls_pending.rs`
- API：`crates/bifrost-admin/src/handlers/config.rs` 的 `/api/config/ip-tls/pending*` 路由
- Web 端：`web/src/components/PendingIpTlsModal`、`web/src/stores/usePendingIpTlsStore.ts`、`web/src/pages/Settings/tabs/ProxyTab.tsx`

## 用户目标验证清单

### 必须实现

- `TlsConfig` 增加 `ip_intercept_include` 和 `ip_intercept_exclude` 两个字段，落盘于统一的 `unified_config` 存储。
- TLS 决策链中新增 client IP 维度，优先级 **高于** 全局开关、**低于** 规则级 `tls_intercept`、Host 重写、应用 include/exclude、域名 include/exclude。
- 支持单 IP、CIDR、IPv6 以及 IPv6 CIDR。
- CONNECT 阶段，如果客户端为非 loopback 且未落在 include/exclude 中，且未在 session 内做过决策，把 IP 推入 `IpTlsPendingManager`。
- WebSocket / SSE 推送 pending 事件，前端弹窗展示 `Enable TLS` 与 `Skip` 两个操作。
- Approve 把 IP 加入 include 并持久化，Skip 加入 exclude；两个操作互斥（另一侧同名 IP 会被自动移除）。
- Traffic 详情 Response 空状态提供 `Intercept this client` 一键写入 include 的快捷入口。
- Settings > Proxy Tab 提供 “Force Intercept” / “Skip Intercept” 两张 IP / CIDR 卡片。

### 必须不破坏

- 规则级 `tls_intercept`、Host 重写强制解包语义不变，仍在 IP 维度之前生效。
- 域名和应用维度不变，只在这两级都没有命中时才落到 IP 维度。
- 全局 `enable_tls_interception` 兜底行为保持：所有维度都没有决定时按全局值走。
- Loopback client（`127.0.0.0/8`、`::1`）行为不变，不触发 pending，也不参与新 IP 弹窗。
- Access control whitelist（`/api/whitelist`）与 TLS IP whitelist 是两回事，二者互不影响。
- 现有 traffic 展示、mock、`bifrost config` 客户端命令继续可用，只是新增两组字段。

### 必须真实验证

- 用两台真实设备验证：远端 IP 第一次 CONNECT 时前端弹窗出现，Approve 后后续请求解包，Skip 后后续请求直通。
- 修改 include / exclude 后重启 daemon，配置持久化且 pending session 被重置。
- Traffic 详情 Response 空状态点击 `Intercept this client` 后，Settings 页面看到 IP 已进入 Force Intercept 列表。
- `is_ip_included` / `is_ip_excluded` 单测覆盖单 IP、CIDR、IPv6 三类；决策链单测覆盖 `test_ip_tls_priority_below_domain`、`test_ip_tls_priority_above_global`、include vs exclude 冲突场景。

## 产品语义

### client IP 维度到底是什么

client IP 指的是发起 CONNECT / HTTP 请求的 socket 对端地址（`peer_addr.ip()`），不是目标域名解析出来的服务器 IP。含义是“谁在使用我的代理”，用于回答“把某台远端设备的所有 TLS 流量解开来看”这个需求。它和 access control whitelist（谁能连上代理）在语义上正交：access control 决定“能否连”，IP TLS 白名单决定“连上后要不要解包”。

### 优先级（从高到低）

1. 无 CA 证书 → 不解包。
2. 规则级 `tls_intercept` 字段（rule level override）。
3. Host 重写要求解包 → 强制解包。
4. 应用（进程名）include / exclude —— 仅对本地客户端有效，远端 IP 天然无进程名，落到下一级。
5. 域名 include / exclude。
6. **client IP include**：命中即强制解包。
7. **client IP exclude**：命中即强制不解包。
8. 全局 `enable_tls_interception` 兜底。

第 6/7 步的顺序意味着：如果一个 IP 同时出现在 include 和 exclude 里，include 生效。这也是 approve/skip 互斥入库的原因——避免出现“两边都有”的模糊状态。

### 新 IP 决策弹窗（PendingIpTlsModal）

- 首次出现的非 loopback client IP，如果既不在 include 也不在 exclude、且 session 内没决策过，会进入 `IpTlsPendingManager::pending`，并广播 `event_type=new` 事件。
- 前端 `usePendingIpTlsStore` 订阅事件，`App.tsx` 挂载 `PendingIpTlsModal`；有 pending 时自动弹出。
- 用户点击 `Enable TLS` 走 approve API；点击 `Skip` 走 skip API；两次操作都会在 `session_decided` 里记录，重启前不会再次弹窗提醒同一个 IP。
- 支持 `Clear All` 一键清空，以及跳转 Settings 页面手动编辑名单。
- 弹窗展示的 `attempt_count` 是同一 pending 周期内该 IP 累计触发次数，用于让用户判断“这个 IP 一直在敲门吗”。

## 技术细节

### 数据模型

```rust
// crates/bifrost-storage/src/unified_config.rs
pub struct TlsConfig {
    pub enabled: bool,
    pub intercept_include: Vec<String>,
    pub intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub ip_intercept_include: Vec<String>,   // NEW
    pub ip_intercept_exclude: Vec<String>,   // NEW
    // ...
}

pub struct TlsConfigUpdate {
    // ...
    pub ip_intercept_include: Option<Vec<String>>,
    pub ip_intercept_exclude: Option<Vec<String>>,
}
```

`RuntimeConfig`、`TlsInterceptConfig`、`ProxyConfig` 在启动路径把这两组字段透传到 tunnel 层。反序列化时缺字段视为空数组，保证老 unified config 文件平滑升级。

### IP 匹配实现

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`：

```rust
fn parse_client_ip(raw: &str) -> Option<IpAddr> { /* ... */ }

fn ip_matches_pattern(ip: &str, pattern: &str) -> bool {
    let target = match parse_client_ip(ip) { Some(v) => v, None => return false };
    if let Ok(network) = pattern.parse::<ipnet::IpNet>() {
        return network.contains(&target);
    }
    if let Ok(single) = pattern.parse::<IpAddr>() {
        return single == target;
    }
    false
}

fn is_ip_included(ip: &str, list: &[String]) -> bool {
    list.iter().any(|p| ip_matches_pattern(ip, p))
}
fn is_ip_excluded(ip: &str, list: &[String]) -> bool {
    list.iter().any(|p| ip_matches_pattern(ip, p))
}
```

- 支持 `192.168.1.100`、`10.0.0.0/8`、`::1`、`fe80::/10` 等格式。
- 非法字符串直接匹配失败，不 panic。
- pattern 解析结果不缓存，靠 `TlsIntercept` 每次 CONNECT 拿到的最新配置。

### CONNECT 触发点

```rust
// tunnel/mod.rs handle_connect()
if !peer_addr.ip().is_loopback() {
    let client_ip_str = peer_addr.ip().to_string();
    if !is_ip_included(&client_ip_str, &tls_intercept_config.ip_intercept_include)
        && !is_ip_excluded(&client_ip_str, &tls_intercept_config.ip_intercept_exclude)
    {
        if let Some(ip_tls_mgr) = admin_state.ip_tls_pending_manager.as_ref() {
            ip_tls_mgr.check_and_add_pending(peer_addr.ip());
        }
    }
}
```

`check_and_add_pending` 内部：如果 session 已决策 → 直接返回 false；如果已经在 pending 列表 → attempt_count += 1；否则 push 并广播 `new` 事件。

### IpTlsPendingManager

- 数据结构：`RwLock<Vec<(IpAddr, first_seen_secs, attempt_count)>>`。
- `session_decided: RwLock<HashSet<IpAddr>>` 存活期与进程一致，daemon 重启后重置，用户下次连接会再次弹窗。
- `broadcast::Sender<PendingIpTlsEvent>` channel 容量 64，事件类型 `new` / `approved` / `skipped` / `cleared`。
- `approve` / `skip` 从 pending 移除、写入 session_decided、广播；不直接写 `TlsConfig`——由 handler 层调用统一的配置更新入口保证事务一致性。

### API 端点

| 路径 | 方法 | 功能 |
|------|------|------|
| `/api/config/ip-tls/pending` | GET | 获取当前 pending IP 列表 |
| `/api/config/ip-tls/pending/stream` | GET | SSE 流订阅 pending 事件 |
| `/api/config/ip-tls/pending/approve` | POST | body `{ "ip": "..." }`，Approve 一个 IP |
| `/api/config/ip-tls/pending/skip` | POST | Skip 一个 IP |
| `/api/config/ip-tls/pending` | DELETE | 清空 pending 列表（不改写 include/exclude） |

推送事件结构（`PendingIpTlsEvent`）：

```json
{
  "event_type": "new",
  "pending": { "ip": "192.168.1.100", "first_seen": 1712345678, "attempt_count": 1 },
  "total_pending": 1
}
```

Approve/Skip handler 会同时：

1. 更新 `TlsConfig.ip_intercept_include` 或 `ip_intercept_exclude`（写入前去重，且从另一侧移除同名 IP 以维持互斥）。
2. 调用 `IpTlsPendingManager::approve/skip` 把该 IP 移出 pending、加入 session_decided。
3. 广播 `push_manager` 通知，让所有 Web 客户端刷新 Settings 与 pending 列表。

`PUT /api/config` 更新完整 unified config 时，如果传入的 include/exclude 与旧值不同，需要清理 `session_decided` 中对应条目——否则用户手动把 IP 从 include 中删除、又想让弹窗再次出现，会被 session 缓存阻断。

## CLI 与 Web

### CLI

`bifrost config set` 支持 `tls.ip_intercept_include` / `tls.ip_intercept_exclude` 两个 key（数组）；`bifrost config get tls` 输出会打印这两组值；`bifrost config` display 增加对应段展示。CLI 层不实现 pending 决策的交互命令，因为决策强依赖 WebSocket 推送与前端弹窗；CLI 侧只提供“配置读写”，让 headless 用户能通过 API/CLI 显式维护名单。

### Web UI

- `PendingIpTlsModal`：全局挂载于 `App.tsx`，`usePendingIpTlsStore` 订阅 SSE，pending > 0 时弹出。每行展示 IP、首次出现时间、attempt_count；提供 `Enable TLS` / `Skip` / `Clear all` / `Manage in Settings` 四个入口。
- `Layout/index.tsx` 顶部状态区在 pending > 0 时展示 badge，避免用户误关弹窗后失去入口。
- Settings > Proxy Tab：新增两张卡片 **Force Intercept** 与 **Skip Intercept**，分别绑定 include / exclude。UX 细节：输入框占位符 `192.168.1.100 or 10.0.0.0/8`；Add 前做前端 IP/CIDR 语法校验；已配置项以可关闭 Tag 展示；从一侧新增 IP 时自动从另一侧移除，前后端一致互斥。
- Traffic 详情 Response 空状态：新增 `Intercept this client` 按钮，仅在 `client_ip` 存在且非 loopback 时展示；点击后调用 approve API，并给出统一的“目标应用需要重连后新的 CONNECT 才生效”提示。同一入口还提供 `Intercept this domain`、`Intercept this app`、`Allow this client`（写入 access whitelist）。

### Admin API 权限

- `/api/config/ip-tls/pending/*` 与 `/api/config` 同权限（受 CSRF 保护、需要 admin token）。
- SSE stream 支持长连接，客户端断开后重连即可获取最新 pending list（前端 store 有 fallback：断开后重新拉一次 GET）。

## Sync 边界

- IP include/exclude 属于本机 client 视角配置，与其他设备的 client IP 分布无关，**默认不进入 rule sync / group sync**。
- unified_config 的整体同步策略仍在讨论中；本方案要求：即使未来 unified_config 加入同步，也必须对 `ip_intercept_include/exclude` 显式过滤掉，避免多台设备互相覆盖。
- Group 规则不承载 client IP 语义。

## 实现切分

### Phase 1：核心决策链与持久化

- `TlsConfig`、`TlsConfigUpdate`、`RuntimeConfig`、`TlsInterceptConfig`、`ProxyConfig` 增加两组字段。
- `is_ip_included` / `is_ip_excluded` / `ip_matches_pattern`。
- CONNECT 决策链插入 IP 维度（优先级第 6/7 步）。
- 单测：单 IP、CIDR、IPv6、include vs exclude、优先级低于域名、优先级高于全局。

### Phase 2：Pending Manager 与 API

- `IpTlsPendingManager`、`PendingIpTls`、`PendingIpTlsEvent`。
- 5 个 REST 端点 + SSE stream。
- Session decided 集合、广播机制、handler 与配置写入原子性。
- 单测：manager 缺失时 handler 返回 503、清空后 pending 为空、approve/skip 从 pending 移除并写 session_decided。

### Phase 3：前端集成

- `usePendingIpTlsStore` + SSE 订阅。
- `PendingIpTlsModal` 组件与 `App.tsx` 挂载。
- Settings Proxy Tab 两张卡片（Force / Skip Intercept）。
- Traffic Response 空状态四个快捷入口。
- Playwright 覆盖 pending 弹窗、Settings 卡片、Response 空状态入口。

### Phase 4：文档与人工回归

- `human_tests/api-config.md` 与 `human_tests/webui-traffic.md` 补 client IP 场景。
- 新建或补齐 e2e 脚本（如 `test_ip_tls_client_whitelist.sh`），覆盖：非本机 client 触发 pending、approve/skip 落盘、优先级验证。
- `docs/` 增加 “Client IP TLS Whitelist” 段落，说明与 access whitelist 的区别。

## 测试方案

### 单元测试（proxy）

- `test_ip_intercept_include_match`
- `test_ip_intercept_exclude_match`
- `test_ip_intercept_cidr_match`
- `test_ip_intercept_ipv6`
- `test_ip_tls_priority_below_domain`：域名 include 命中时忽略 IP exclude。
- `test_ip_tls_priority_above_global`：IP include 在全局关闭时仍强制解包；IP exclude 在全局开启时仍不解包。
- `test_ip_tls_include_and_exclude_conflict`：同一 IP 同时出现在两侧时 include 优先。

### 单元测试（admin）

- `check_and_add_pending_returns_true_for_new_ip`
- `check_and_add_pending_returns_false_when_decided`
- `approve_moves_ip_out_of_pending_and_broadcasts_event`
- `skip_moves_ip_out_of_pending_and_broadcasts_event`
- `clear_ip_tls_pending_clears_manager_list`
- `get_ip_tls_pending_returns_empty_list_when_manager_missing`
- `ip_tls_pending_stream_returns_service_unavailable_without_manager`

### E2E

- `e2e-tests/tests/test_ip_tls_client_whitelist.sh`（新增）：
  - 起临时 daemon，从非 loopback IP 触发 CONNECT，验证 `/api/config/ip-tls/pending` 出现该 IP。
  - Approve 后 `TlsConfig.ip_intercept_include` 持久化，重启后仍存在。
  - Skip 后 `ip_intercept_exclude` 持久化。
  - CIDR 匹配：include 中写 `10.0.0.0/8`，另一台设备 `10.0.0.55` CONNECT 不再触发 pending。
- Playwright：
  - Settings 卡片增删 IP 与互斥。
  - Traffic Response 空状态点击 `Intercept this client` 后 Toast + Settings 同步。
  - PendingIpTlsModal 出现、Approve/Skip、Clear all。

### 真实场景测试（human_tests/api-config.md / webui-traffic.md）

- TC-IPTLS-01：新远端 IP 触发弹窗，Approve 后后续请求可解包。
- TC-IPTLS-02：Skip 后同一 IP 不再弹窗，且请求走透传。
- TC-IPTLS-03：Clear All 清空 pending，session 内该 IP 再来时重新弹窗（因为 session_decided 只在 approve/skip 时写入）。
- TC-IPTLS-04：手动从 Settings 删除 IP → 目标设备重连能否重新弹窗（依赖配置更新时清理 session_decided）。
- TC-IPTLS-05：Response 空状态 `Intercept this client` 与 `Allow this client` 分别落在 IP 解包白名单和 access whitelist。

启动服务统一使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核决策链：IP 维度是否严格夹在“域名”和“全局”之间；规则级 `tls_intercept` 与 Host 重写不受影响。
- 复核 pending 生命周期：session_decided 何时清空、何时保留；用户手动从 Settings 删 IP 后 session_decided 是否同步清理。
- 复核 approve/skip 的互斥入库；unified_config 反序列化缺字段兼容性。
- 跑 proxy tunnel 单测、admin handler 单测、E2E 脚本。

### 第 2 轮

- 复核前端：弹窗关掉后是否有 badge 入口；SSE 断线重连的 fallback；卡片与 pending 事件的一致性。
- 复核错误消息：pending manager 未启用时返回 503 而不是 500；非法 IP 返回 400 且不落盘。
- 复跑关键测试并抽查 Traffic Response 空状态。

## 风险与决策点

- **Loopback 判定**：`peer_addr.ip().is_loopback()` 未处理经过反向代理转发的场景。当前假设 Bifrost 直连客户端；若上游有透明代理需要用 X-Forwarded-For 提取 client IP，视为后续增强。
- **session_decided 与配置漂移**：session_decided 只在 approve/skip 时写；如果用户绕过 pending 直接编辑配置，session_decided 可能与 include/exclude 不一致。当前策略：配置更新入口同步清理 session_decided 中不再落在两个列表里的 IP。
- **CIDR 广域 include**：允许 `0.0.0.0/0` 会让全部 client 都被解包，等价于强制全局解包。第一版不阻断，但在 Settings 卡片给出 warning。
- **IPv6 dual stack**：`::ffff:192.168.1.1` 与 `192.168.1.1` 在匹配层需要归一，`ip_matches_pattern` 依赖 `ipnet` crate 的默认行为；单测覆盖 IPv4-mapped IPv6 场景。
- **与 access whitelist 混淆**：用户经常把“允许某台机器连代理”和“对某台机器解包”混淆。文档与 UI 必须始终并列出现，Response 空状态两个按钮的 tooltip 明确职责。
- **同步策略**：unified_config 未来若引入远程同步，必须显式过滤 client IP 白名单，避免不同设备互相覆盖。
