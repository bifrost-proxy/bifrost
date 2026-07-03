# TLS 客户端信任自动检测

## 背景

Bifrost 启用 TLS 拦截后使用自研 CA 为目标域名动态签发叶子证书。若客户端未把 Bifrost CA 加入信任存储，TLS 握手会失败并抛 `ERR_CERT_AUTHORITY_INVALID` 类错误。已有的两条能力不能填补这个盲区：

- `CertInstaller::check_status()` 只查服务端本机系统信任链，不代表 Firefox / Node.js / Java / 移动 App 等使用独立 trust store 的客户端。
- `IpTlsPendingManager` 只处理"待用户批准"的远程 IP，不知道客户端是否真正握手成功过。

本方案在 TLS 握手失败点（`TlsAcceptor::accept()`）分析 rustls 错误信号，被动统计每个客户端 (IP / 应用) 的握手成功/失败次数与失败原因，实时评估其信任状态（Trusted / NotTrusted / PossiblyNotTrusted / LikelyUntrusted / Unknown），推送到 Admin API + Web 通知面板，让用户能直接看到"哪个客户端还没装 CA"。

## 用户目标验证清单

### 必须实现

- `TlsAcceptFailureReason` 枚举：`ClientDoesNotTrustCa` / `ProbablyClientDoesNotTrustCa` / `DecryptionError` / `CertificateExpired` / `ProtocolIncompatible` / `ConnectionReset` / `Unknown`。
- `classify_tls_accept_error(&std::io::Error) -> TlsAcceptFailureReason`：字符串模糊匹配 rustls 格式化的 alert 描述（大小写/下划线/空格三形式都覆盖）。
- `ClientTrustRecord` per-client：`first_seen/last_seen`、`handshake_success/fail_definite_untrust/fail_probable_untrust/fail_other`、`last_failure_reason/domain`、`failed_domains: HashMap<String, DomainFailureDetail>`、`success_domains: HashSet<String>`。
- `evaluate_trust(&ClientTrustRecord) -> ClientTrustStatus`：`Unknown`（无样本） / `Trusted`（近期只有成功） / `NotTrusted { reason }`（`definite ≥ MIN_DEFINITE_FOR_NOT_TRUSTED` 且无成功） / `PossiblyNotTrusted { reason }` / `LikelyUntrusted { confidence, sample_count }`（有一定量样本但混合成功）。
- `ClientTlsTrustTracker`：`record_success(client, domain)` / `record_failure(client, domain, reason)` / `get(client)` / `list_all()` / `summary(client)`；每次写入后调用 `evaluate_trust` 生成前/后 status，用于事件推送。
- `ClientTrustEvent`：状态迁移事件（老 status → 新 status，含 reason）。
- Admin API `GET /api/notifications/client-trust`：返回 `ClientTrustSummary[]`；只在 `state.client_trust_tracker` 存在时激活。
- Notifications 页面 Client Trust 子表：展示 client、trust_status、last_failure_reason、last_failure_domain、handshake success/fail 计数、first_seen/last_seen；`untrustedCount` 参与页面通知徽章。

### 必须不破坏

- 不影响 TLS 握手性能：`classify_tls_accept_error` 只在握手已失败路径运行；成功路径仅一次 hashmap set 插入。
- 不改变现有 `CertInstaller` / `IpTlsPendingManager` 行为，二者与本模块正交。
- Tracker 在 `AdminState::client_trust_tracker` 为 `None` 时（如禁用 TLS 拦截）全部路径退化为 no-op；`/api/notifications/client-trust` 返回空 items 或 501。
- 不写入 `BIFROST_DATA_DIR`；tracker 只驻内存，进程重启后从零重建（历史失败在下一次握手时自然收敛）。
- 不因 `Unknown` 分类误报"不信任"：只有 `NotTrusted` / `LikelyUntrusted` 才计入 `untrustedCount`。

### 必须真实验证

- 客户端未装 CA 时抓 `curl https://example.com --resolve example.com:443:127.0.0.1 -k=false`，Bifrost 侧 tracker 出现 `ClientDoesNotTrustCa`；Web Notifications 页面 Client Trust 表出现新行 `NotTrusted`。
- 装 CA 后再握手成功，tracker 迁移到 `Trusted`。
- OpenSSL 客户端（`openssl s_client`）在握手早期发送未加密 alert，tracker 记为 `DecryptionError` / `ProbablyClientDoesNotTrustCa`（视错误串）。
- 多个域名混合成功/失败时 `LikelyUntrusted { confidence, sample_count }` 数值符合直觉。

## 产品语义

### 被动检测优先

方案一（被动分析握手错误）是唯一落地方案：不需要主动向客户端发起测试连接，也不要求客户端配合，仅在真实流量的握手失败时统计。相较主动探测：0 侵入、0 额外流量、隐私友好。

### 五态信任判定

| 状态 | 触发条件 | 语义 |
| --- | --- | --- |
| `Trusted` | 近期只有 handshake_success，且最近一次成功晚于最近一次失败 | 客户端已信任 CA |
| `NotTrusted { reason }` | `handshake_fail_definite_untrust ≥ MIN_DEFINITE_FOR_NOT_TRUSTED` 且 `handshake_success == 0` | 明确未信任，reason 来自最近一次失败 |
| `PossiblyNotTrusted { reason }` | 只见到 probable 失败，无 definite / 无成功 | 疑似未信任 |
| `LikelyUntrusted { confidence, sample_count }` | 有一定量样本，untrust 占比高但仍有成功 | 部分客户端组件不信任（如 App 内嵌 WebView） |
| `Unknown` | 无样本 | 尚未观察到握手 |

`MIN_DEFINITE_FOR_NOT_TRUSTED` 是消抖阈值（默认 1~2），避免偶发 alert 误判。

### Alert 到 reason 的映射（`classify_tls_accept_error`）

- `UnknownCA` / `unknown_ca` / `unknown ca` → `ClientDoesNotTrustCa`（definite）
- `BadCertificate` / `CertificateUnknown`（三形式） → `ProbablyClientDoesNotTrustCa`（probable）
- 含 `decrypt` → `DecryptionError`（OpenSSL 早期未加密 alert）
- `CertificateExpired` → `CertificateExpired`
- `HandshakeFailure` / `ProtocolVersion` → `ProtocolIncompatible`
- `connection reset` / `broken pipe` / `unexpected eof` → `ConnectionReset`
- 兜底 → `Unknown`

### 每客户端 + 每域名双维度

Tracker 顶层按 client key（IP 或 IP:port）聚合，`ClientTrustRecord` 内部再按 domain 分桶（`failed_domains: HashMap<String, DomainFailureDetail>`），支持 UI 上钻"这个客户端在哪些域名上握手失败"。

## 技术细节

### 关键源码

| 文件 | 责任 |
| --- | --- |
| `crates/bifrost-admin/src/client_trust_tracker.rs` | `TlsAcceptFailureReason` / `ClientTrustStatus` / `ClientTrustRecord` / `ClientTrustSummary` / `ClientTrustEvent` / `ClientTlsTrustTracker` / `classify_tls_accept_error` / `evaluate_trust` |
| `crates/bifrost-admin/src/state.rs` | `AdminState.client_trust_tracker: Option<Arc<ClientTlsTrustTracker>>` |
| `crates/bifrost-admin/src/handlers/notification.rs` | `GET /api/notifications/client-trust` → `handle_client_trust` |
| `crates/bifrost-proxy/src/tls_accept.rs` | TLS accept 失败路径调用 `classify_tls_accept_error` + `tracker.record_failure(...)`；成功路径调用 `tracker.record_success(...)` |
| `web/src/api/notifications.ts` | `ClientTrustSummary` / `ClientTrustResponse` / `getClientTrust()` → `/notifications/client-trust` |
| `web/src/stores/useNotificationStore.ts` | `clientTrust` state + `fetchClientTrust()` action + `untrustedCount` computed |
| `web/src/pages/Notifications/index.tsx` | Client Trust 子表；`trustStatusTag` 渲染 |

### 关键数据结构

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsAcceptFailureReason {
    ClientDoesNotTrustCa,
    ProbablyClientDoesNotTrustCa,
    DecryptionError,
    CertificateExpired,
    ProtocolIncompatible,
    ConnectionReset,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClientTrustStatus {
    Trusted,
    NotTrusted { reason: String },
    PossiblyNotTrusted { reason: String },
    LikelyUntrusted { confidence: f32, sample_count: u32 },
    Unknown,
}

pub struct ClientTrustRecord {
    pub first_seen: u64,
    pub last_seen: u64,
    pub last_success_at: Option<u64>,
    pub last_failure_at: Option<u64>,
    pub handshake_success: u32,
    pub handshake_fail_definite_untrust: u32,
    pub handshake_fail_probable_untrust: u32,
    pub handshake_fail_other: u32,
    pub last_failure_reason: Option<TlsAcceptFailureReason>,
    pub last_failure_domain: Option<String>,
    failed_domains: HashMap<String, DomainFailureDetail>,
    success_domains: HashSet<String>,
}

pub struct ClientTrustSummary {
    pub client: String,
    pub trust_status: ClientTrustStatus,
    pub last_failure_reason: Option<String>,
    pub last_failure_domain: Option<String>,
    pub handshake_success: u32,
    pub handshake_fail_definite: u32,
    pub handshake_fail_probable: u32,
    pub handshake_fail_other: u32,
    pub first_seen: u64,
    pub last_seen: u64,
}

pub struct ClientTrustEvent {
    pub client: String,
    pub old_status: ClientTrustStatus,
    pub new_status: ClientTrustStatus,
    pub reason: Option<String>,
}
```

### 分类函数（真实实现）

```rust
pub fn classify_tls_accept_error(error: &std::io::Error) -> TlsAcceptFailureReason {
    let lower = error.to_string().to_ascii_lowercase();

    if lower.contains("unknownca") || lower.contains("unknown_ca") || lower.contains("unknown ca") {
        return TlsAcceptFailureReason::ClientDoesNotTrustCa;
    }
    if lower.contains("badcertificate") || lower.contains("bad_certificate")
        || lower.contains("bad certificate")
        || lower.contains("certificateunknown") || lower.contains("certificate_unknown")
        || lower.contains("certificate unknown")
    {
        return TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa;
    }
    if lower.contains("decrypt") { return TlsAcceptFailureReason::DecryptionError; }
    if lower.contains("certificateexpired") || lower.contains("certificate expired") {
        return TlsAcceptFailureReason::CertificateExpired;
    }
    if lower.contains("handshakefailure") || lower.contains("protocolversion") {
        return TlsAcceptFailureReason::ProtocolIncompatible;
    }
    if lower.contains("connection reset") || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
    {
        return TlsAcceptFailureReason::ConnectionReset;
    }
    TlsAcceptFailureReason::Unknown
}
```

### 集成点

TLS accept 入口（`crates/bifrost-proxy/src/tls_accept.rs` 或等价路径）在 `TlsAcceptor::accept()` 结果分支：

```rust
match acceptor.accept(stream).await {
    Ok(tls_stream) => {
        if let Some(t) = &state.client_trust_tracker { t.record_success(&client, &sni); }
        // 继续正常处理
    }
    Err(err) => {
        if let Some(t) = &state.client_trust_tracker {
            t.record_failure(&client, &sni, classify_tls_accept_error(&err));
        }
        return Err(err.into());
    }
}
```

## CLI + Web + Admin API

### CLI

不新增专属 CLI；调试可通过 `curl http://127.0.0.1:9900/api/notifications/client-trust` 查询。

### Web UI

- `Notifications` 页面新增 Client Trust 子表（列：Client / Trust Status / Reason / Last Failure Domain / Success / Failures / First Seen / Last Seen）。
- 顶部通知徽章 `untrustedCount` 汇总 `NotTrusted + LikelyUntrusted` 客户端数。
- `trustStatusTag(status)` 按状态渲染彩色 tag（绿 Trusted / 红 NotTrusted / 黄 PossiblyNotTrusted / 橙 LikelyUntrusted / 灰 Unknown）。
- 页面挂载时 `fetchClientTrust()`；后续按 polling 或用户手动刷新。

### Admin API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/notifications/client-trust` | 返回 `{ items: ClientTrustSummary[] }`；`state.client_trust_tracker` 缺失时返回空 items |

注意：设计初版曾用 `/api/tls/client-trust`，落地时统一到通知子路由 `/api/notifications/client-trust`。同一 handler `handle_client_trust` 由 `handlers/notification.rs` 分派。

## Sync 边界

Tracker 是本机运行时统计，不参与 Sync / 导入导出 / 分享。跨设备信任状态由各自 Bifrost 实例独立收敛；不同实例间的信任判定完全独立。

## Phase 1-4

### Phase 1：错误分类与数据结构

- 落 `TlsAcceptFailureReason` / `ClientTrustStatus` / `ClientTrustRecord`。
- 落 `classify_tls_accept_error`：三形式字符串匹配（大小写 / 下划线 / 空格分隔）。

### Phase 2：Tracker + 状态迁移事件

- `ClientTlsTrustTracker` 内部 `HashMap<String, ClientTrustRecord>`。
- `record_success` / `record_failure` 计算 old/new status，emit `ClientTrustEvent`。
- `evaluate_trust` 五态判定 + `MIN_DEFINITE_FOR_NOT_TRUSTED` 消抖。

### Phase 3：Admin API + AdminState 注入

- `AdminState.client_trust_tracker: Option<Arc<ClientTlsTrustTracker>>` 在 TLS 拦截开启时构造。
- `handlers/notification.rs` 分派 `/notifications/client-trust` → `handle_client_trust`。
- TLS accept 路径接入 `record_success` / `record_failure`。

### Phase 4：Web Notifications 集成

- `web/src/api/notifications.ts` 加 `getClientTrust()` + `ClientTrustSummary` 类型。
- `useNotificationStore` 加 `clientTrust` state + `fetchClientTrust` action + `untrustedCount`。
- `pages/Notifications/index.tsx` 新增 Client Trust 表格 + `trustStatusTag` 渲染。

## 测试方案

### 单元测试（`cargo test -p bifrost-admin client_trust_tracker`）

- `test_classify_unknown_ca_alert` / `test_classify_bad_certificate_alert` / `test_classify_certificate_unknown_alert` — 三形式（下划线 / 空格 / camel）都识别。
- `test_classify_decrypt_error` — OpenSSL 未加密 alert 命中 `DecryptionError`。
- `test_classify_certificate_expired` / `test_classify_handshake_failure` / `test_classify_protocol_version` / `test_classify_connection_reset` / `test_classify_broken_pipe` / `test_classify_unexpected_eof`。
- `test_classify_unknown_fallback` — 兜底 `Unknown`。
- `test_evaluate_trust_no_data` — 空 record → `Unknown`。
- `test_evaluate_trust_all_success` — 只有成功 → `Trusted`。
- `test_evaluate_trust_definite_not_trusted_no_success` — 达阈值且无成功 → `NotTrusted { reason }`。
- `test_evaluate_trust_probable_only` → `PossiblyNotTrusted { reason }`。
- `test_evaluate_trust_mixed_high_untrust_ratio` → `LikelyUntrusted { confidence, sample_count }`。
- `test_evaluate_trust_recovered` — 先失败后成功晚于失败 → `Trusted`。
- `test_record_failure_emits_transition_event` — old/new status 差异触发 event。
- `test_record_success_marks_success_domain`。
- `test_record_failure_bumps_failed_domain_detail`。
- `test_summary_returns_current_evaluation`。

### 集成测试

- `crates/bifrost-admin` handler 测试：`GET /api/notifications/client-trust` 在 tracker 存在/不存在时的行为。
- `crates/bifrost-proxy` TLS accept 测试：mock rustls Error → 断言 tracker 计数与 status 迁移。

### E2E 测试

- 未信任 CA 的 curl 请求 → tracker 出现 `NotTrusted`；已信任后 → `Trusted`。
- OpenSSL `s_client -showcerts` 提前中断 → `DecryptionError`。
- 多域名混合成功/失败 → `LikelyUntrusted` 数值符合直觉。

### human_tests

- `human_tests/tls-client-trust.md`（如有）：Web Notifications 页面 Client Trust 表交互；`untrustedCount` 徽章。
- Firefox（独立 trust store）在未导入 CA 时握手 → 页面出现新客户端 `NotTrusted`。
- 移动端（未装 profile）握手 → 页面按 IP 聚合出现 `NotTrusted`。

### 项目校验

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p bifrost-admin client_trust_tracker
cargo test --workspace --all-features
pnpm --dir web test
pnpm --dir web build
```

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 diff：`client_trust_tracker.rs`（分类完备性 / 五态迁移正确）、`handlers/notification.rs`（tracker 缺失兜底）、`state.rs`（`Option<Arc<...>>` 生命周期）、TLS accept 集成点、Web `Notifications` 表。
- 重点 review：`classify_tls_accept_error` 是否覆盖大小写/下划线/空格三形式；`MIN_DEFINITE_FOR_NOT_TRUSTED` 是否合理避免抖动；tracker 是否线程安全（`Arc<Mutex<...>>` 或 `RwLock`）。
- 复测：单元测试全绿；`curl -k` 与 `curl` 两种模式在 tracker 中体现。

### 第 2 轮

- 复核第 1 轮修复。
- 再次 `git diff`；Web store 是否正确聚合 `untrustedCount`；`trustStatusTag` 是否覆盖五态。
- 重点 review：`LikelyUntrusted.confidence` 计算是否稳定（避免 NaN / >1）；`success_domains` / `failed_domains` 内存占用是否有上限（未来风险）。
- 复测：真实浏览器 / OpenSSL / 移动端触发不同 alert；Web Notifications 表实时刷新。

## 风险与决策

- **API 路径**：初版设计 `/api/tls/client-trust`，落地时统一到 `/api/notifications/client-trust`，与其他通知类信息（cert_status / ip_pending）同源。文档以落地版本为准。
- **UI 挂载点**：初版设计放 Certificate 页面，落地时改到 Notifications 页面，作为 tab 表格。原因：Client Trust 是运行时观测型信息，与 CA 状态是两类事情。
- **字符串匹配脆弱**：rustls 错误字符串格式可能随版本变化；三形式匹配（大小写 / 下划线 / 空格）是当前折中；未来 rustls 若提供结构化错误应优先。
- **DecryptionError 分类**：命名与初版 `DecryptError` 不同（落地代码定为 `DecryptionError`）；文档已对齐。
- **消抖阈值**：`MIN_DEFINITE_FOR_NOT_TRUSTED` 太低会误报（网络抖动可能触发一次 alert），太高会漏报；默认 1~2，随线上观察调整。
- **内存无上限**：`HashMap<client, ClientTrustRecord>` + `HashMap<domain, DomainFailureDetail>` 未做 LRU；长期运行的高并发代理可能累积大量 client；后续加 LRU 或按时间 evict。
- **无落盘**：进程重启后 tracker 从零重建；接受"重启后首次握手失败前状态未知"，避免磁盘 IO 与隐私风险。
- **主动探测未启用**：方案二（服务端主动向客户端发起测试握手）不落地：需要客户端配合 / 引入额外流量 / 干扰用户；仅保留在设计文档作为备选。

## 依赖项

- `rustls` 错误字符串格式（`unknown_ca` / `bad_certificate` / `certificate_unknown` / `decrypt error` / `handshake_failure` / `protocol_version`）。
- `AdminState.client_trust_tracker: Option<Arc<ClientTlsTrustTracker>>`。
- `handlers/notification.rs` 分派 `/notifications/client-trust`。
- Web `useNotificationStore` / `Notifications` page。
- `serde` (`#[serde(rename_all = "snake_case")]` / `#[serde(tag = "status")]`)。
