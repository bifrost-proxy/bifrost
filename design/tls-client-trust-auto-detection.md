# TLS 客户端信任自动检测

## 功能模块详细描述

当 Bifrost 启用 TLS 拦截后，代理会使用自定义 CA 为目标域名动态签发叶子证书。如果客户端未将 Bifrost CA 添加到信任存储中，TLS 握手将失败，客户端会看到 `ERR_CERT_AUTHORITY_INVALID` 或类似错误。

目前 Bifrost 已经具备：
- 服务端本机的证书安装状态检测（`CertInstaller::check_status()` → `CertStatus` 三态）
- 远程 IP 客户端的 TLS 拦截待确认机制（`IpTlsPendingManager`）

**但缺少一个关键能力：在 TLS 拦截启用的运行时环境中，自动检测某个客户端（本机或远程）是否真正信任了 Bifrost 自定义 CA 证书。**

本方案旨在设计一套「TLS 客户端信任自动检测」机制，使 Bifrost 能在 TLS 拦截运行时自动评估客户端的证书信任状态，而不是仅依赖服务端本机的系统信任链检查。

## 背景与问题分析

### 当前架构中的信任检测盲区

```
┌─────────────┐     CONNECT      ┌─────────────┐     TLS     ┌──────────────┐
│   Client     │ ───────────────► │   Bifrost    │ ──────────► │  Target Host │
│ (浏览器/App) │ ◄─── 200 OK ─── │   Proxy      │ ◄────────── │              │
│              │                  │              │             │              │
│  TLS握手开始  │ ◄─ ServerHello ─ │ (动态叶子证书) │             │              │
│  验证证书... │                  │              │             │              │
│              │                  │              │             │              │
│  ❌ 不信任CA  │ ─ Alert(48) ──► │  握手失败!    │             │              │
│  或直接断开   │                  │  但错误信息   │             │              │
│              │                  │  不够精确     │             │              │
└─────────────┘                  └─────────────┘             └──────────────┘
```

当前的问题：
1. **本机检测不代表运行时信任**：`CertInstaller` 检测的是操作系统级别的信任存储，但某些应用（如 Firefox、Node.js、Java）使用独立信任存储，系统级别的"已信任"并不意味着所有客户端都信任。
2. **远程客户端完全无法检测**：远程设备（手机、其他电脑）是否安装信任了 CA，当前只能靠用户手动确认。
3. **握手失败后信息不透明**：`TLS accept failed` 错误信息被统一吞掉，无法区分"客户端不信任 CA"和"其他 TLS 错误"。

### TLS 握手失败的可识别信号

当客户端不信任代理签发的证书时，在 TLS 握手阶段会产生以下可辨识的信号：

| 信号来源 | 信号内容 | 可靠度 | 说明 |
|---------|---------|--------|------|
| TLS Alert `unknown_ca`(48) | 客户端发送 Fatal Alert | ★★★★★ | 明确表示客户端不认识签发 CA，最可靠的信号 |
| TLS Alert `bad_certificate`(42) | 客户端发送 Fatal Alert | ★★★★☆ | 客户端认为证书无效（可能是信任问题或格式问题） |
| TLS Alert `certificate_unknown`(46) | 客户端发送 Fatal Alert | ★★★☆☆ | 通用证书错误，需结合上下文判断 |
| rustls `DecryptError` | 对端未加密发送 alert | ★★★☆☆ | OpenSSL 客户端在未完成握手时发送未加密 alert，rustls 表现为 DecryptError |
| TCP 连接重置 / 关闭 | 对端直接断开 | ★★☆☆☆ | 部分客户端直接关闭连接而不发送 alert |
| 握手超时 | 客户端无响应 | ★☆☆☆☆ | 可能是网络问题也可能是客户端挂起 |

## 可行性分析

### 方案一：TLS 握手失败错误分析（被动检测） — ⭐ 推荐

**原理**：在 `TlsAcceptor::accept()` 失败时，分析错误类型，判断是否属于"客户端不信任证书"类故障。

**rustls 的错误类型映射**：

```rust
// rustls::Error 的关键变体
enum Error {
    AlertReceived(AlertDescription),  // 收到客户端发送的 TLS Alert
    DecryptError,                     // 客户端发送了未加密的 Alert（OpenSSL 行为）
    PeerIncompatible(..),            // 协议不兼容
    PeerMisbehaved(..),              // 对端行为异常
    // ...
}
```

当客户端不信任证书时，rustls 服务端通常收到：
- `Error::AlertReceived(AlertDescription::UnknownCA)` — 最直接的信号
- `Error::AlertReceived(AlertDescription::BadCertificate)` — 证书验证失败
- `Error::AlertReceived(AlertDescription::CertificateUnknown)` — 通用证书错误
- `Error::DecryptError` — OpenSSL/LibreSSL 客户端在握手早期发送未加密 alert

**实现思路**：

```rust
fn classify_tls_accept_error(error: &std::io::Error) -> TlsAcceptFailureReason {
    let error_str = error.to_string();

    // rustls 将 AlertDescription 格式化为字符串：
    // "peer sent fatal alert: UnknownCA"
    // "peer sent fatal alert: BadCertificate"
    // "peer sent fatal alert: CertificateUnknown"
    if error_str.contains("UnknownCA")
        || error_str.contains("BadCertificate")
        || error_str.contains("CertificateUnknown")
    {
        return TlsAcceptFailureReason::ClientDoesNotTrustCa;
    }

    // OpenSSL 客户端在握手早期发送未加密 alert
    if error_str.contains("decrypt error") || error_str.contains("DecryptError") {
        return TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa;
    }

    // 证书过期
    if error_str.contains("CertificateExpired") {
        return TlsAcceptFailureReason::CertificateExpired;
    }

    // 协议不兼容
    if error_str.contains("HandshakeFailure") || error_str.contains("ProtocolVersion") {
        return TlsAcceptFailureReason::ProtocolIncompatible;
    }

    TlsAcceptFailureReason::Unknown
}
```

**优势**：
- 零额外网络开销，纯粹利用已有的 TLS 握手失败信息
- 不需要客户端配合，对客户端完全透明
- 可以精确关联到具体的域名、客户端 IP、客户端应用
- 实时性强，握手失败后立即可知

**劣势**：
- 依赖 rustls 错误信息的字符串格式（格式可能随版本变化）
- 部分客户端直接关闭 TCP 连接而不发送 TLS Alert，此时无法识别
- 只能在握手失败后检测（被动检测），无法在握手前预判

**可靠度评估**：★★★★☆（对大部分主流浏览器和 HTTP 客户端有效）

### 方案二：主动探测请求（TLS Trust Probe）

**原理**：当检测到新的客户端 IP（或本机首次启用 TLS 拦截时），代理主动发起一个经过 TLS 拦截的探测请求，检查 TLS 握手是否成功。

**实现思路**：

```
┌────────────┐                    ┌────────────────┐
│   Bifrost   │ ─── 1. 生成探测  ─► │  Probe Endpoint │
│   Proxy     │     请求给自己    │  (代理内部虚拟  │
│             │                   │   HTTPS 服务)   │
│             │ ◄── 2. TLS握手 ── │                 │
│             │     使用自定义CA  │                 │
│             │                   │                 │
│  3. 检查结果 │                   │                 │
│  握手成功 → │                   │                 │
│  客户端信任  │                   │                 │
└────────────┘                    └────────────────┘
```

具体流程：
1. Bifrost 在管理端暴露一个内部 HTTPS 端点（如 `https://bifrost-trust-probe.internal/_probe`）
2. 该端点使用 Bifrost CA 签发的证书
3. 当需要检测某客户端的信任状态时，构造一个 HTTPS 请求通过代理发送到该端点
4. 如果客户端（或系统代理环境）信任 CA，请求成功；否则 TLS 握手失败

**但此方案存在根本性困难**：
- 探测请求是代理自己发给自己的，不经过客户端的 TLS 验证栈 — 无法代表客户端的信任状态
- 如果要让客户端来发探测请求，需要客户端配合（嵌入 JavaScript、安装 Agent 等），大幅增加复杂度
- 对远程客户端（手机等）几乎无法实现无侵入的主动探测

**变体方案：Web UI 内嵌探测**

在 Bifrost 管理端 Web UI 中嵌入一个隐藏的 `<img>` 或 `fetch()` 请求，目标为一个必须经过 TLS 拦截的 HTTPS URL。如果加载成功，说明当前浏览器信任 CA；如果失败，说明未信任。

```javascript
// Web UI 中的探测逻辑
async function probeTlsTrust() {
    try {
        // 请求一个必须经过 MITM 的外部 HTTPS URL
        // 这个请求会经过代理 → 使用 Bifrost CA 签发的证书
        const resp = await fetch('https://bifrost-trust-check.test/_probe', {
            signal: AbortSignal.timeout(5000),
        });
        return resp.ok; // 成功 = 浏览器信任 CA
    } catch {
        return false;    // 失败 = 浏览器不信任 CA
    }
}
```

**优势**：
- 可以检测当前浏览器是否信任 CA（Web UI 场景）
- 用户打开管理界面时自动完成检测

**劣势**：
- 仅适用于 Web 浏览器客户端
- 需要代理处于拦截模式下才有意义
- 需要一个可控的域名或 IP 来触发 TLS 拦截
- 浏览器安全策略（混合内容、CORS）可能阻止检测
- 无法覆盖 CLI 工具、移动端 App 等非浏览器客户端

**可靠度评估**：★★★☆☆（仅适用于 Web UI 场景，覆盖面有限）

### 方案三：统计分析模型（握手成功率推断）

**原理**：通过持续统计某个客户端 IP / 客户端应用的 TLS 握手成功率，推断其是否信任自定义 CA。

**统计维度**：
- 按客户端 IP 聚合：同一 IP 的握手成功率
- 按客户端应用聚合（本机）：如 `Safari` / `Chrome` / `curl` / `node` 各自的握手成功率
- 按域名聚合：排除因目标服务器问题导致的失败

**推断逻辑**：

```
IF  某客户端在最近 N 次 TLS 拦截握手中：
    - 全部失败 → 高概率不信任 CA
    - 全部成功 → 信任 CA
    - 部分失败 → 需要结合错误类型进一步分析
```

**优势**：
- 无需额外机制，纯粹利用运行时统计数据
- 可以按应用粒度识别（如"Chrome 已信任，但 Firefox 未信任"）
- 持续运行时状态可以及时感知变化（如用户在运行中安装了 CA）

**劣势**：
- 需要积累一定量的样本才能做出判断（冷启动问题）
- 可能被网络波动、目标服务器故障等因素干扰
- 需要在所有 TLS 握手路径（CONNECT + SOCKS5）上埋点

**可靠度评估**：★★★★☆（样本量足够时非常可靠）

## 推荐方案：方案一 + 方案三组合

### 整体架构

```
                         TLS 拦截握手
                              │
                              ▼
                    ┌──────────────────┐
                    │ TlsAcceptor      │
                    │   .accept()      │
                    └────┬────────┬────┘
                         │        │
                    成功  │        │ 失败
                         │        │
                         ▼        ▼
              ┌──────────────┐  ┌───────────────────┐
              │ 记录握手成功  │  │ classify_tls_error │
              │              │  │ (方案一：错误分类)  │
              └──────┬───────┘  └────────┬──────────┘
                     │                   │
                     ▼                   ▼
              ┌─────────────────────────────────────┐
              │     ClientTlsTrustTracker            │
              │  (方案三：统计分析)                    │
              │                                     │
              │  - 按 client_ip 聚合                 │
              │  - 按 client_app 聚合 (本机)          │
              │  - 按 domain 分组                    │
              │  - 计算信任置信度                     │
              └──────────────┬──────────────────────┘
                             │
                             ▼
              ┌─────────────────────────────────────┐
              │     ClientTrustStatus                │
              │                                     │
              │  Trusted        - 信任 CA            │
              │  NotTrusted     - 不信任 CA (确认)    │
              │  LikelyUntrusted - 可能不信任 (推测)  │
              │  Unknown        - 样本不足           │
              └──────────────────────────────────────┘
                             │
                     ┌───────┴────────┐
                     ▼                ▼
              ┌────────────┐   ┌──────────────┐
              │  Admin API  │   │  Push 通知    │
              │  展示状态   │   │  实时告警     │
              └────────────┘   └──────────────┘
```

### 核心数据结构

```rust
/// TLS 握手失败的原因分类
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsAcceptFailureReason {
    /// 客户端明确发送了 unknown_ca / bad_certificate / certificate_unknown alert
    /// → 确定不信任 CA
    ClientDoesNotTrustCa,

    /// 客户端发送了未加密的 alert（OpenSSL 行为），大概率是不信任 CA
    ProbablyClientDoesNotTrustCa,

    /// 证书过期（非信任问题，是证书管理问题）
    CertificateExpired,

    /// 协议不兼容（非信任问题，是 TLS 配置问题）
    ProtocolIncompatible,

    /// 客户端断开连接但未发送 alert（原因不确定）
    ConnectionReset,

    /// 其他/无法识别的错误
    Unknown,
}

/// 客户端对自定义 CA 的信任状态
#[derive(Debug, Clone, Serialize)]
pub enum ClientTrustStatus {
    /// 已确认信任（连续多次握手成功）
    Trusted,

    /// 已确认不信任（收到明确的证书拒绝 alert）
    NotTrusted { reason: String },

    /// 推测不信任（握手失败率高，但未收到明确 alert）
    LikelyUntrusted { confidence: f32, sample_count: u32 },

    /// 样本不足，无法判断
    Unknown,
}

/// 每个客户端的信任追踪记录
#[derive(Debug, Clone)]
struct ClientTrustRecord {
    /// 首次观测时间
    first_seen: u64,
    /// 最后一次观测时间
    last_seen: u64,
    /// TLS 握手成功次数
    handshake_success: u32,
    /// TLS 握手失败次数（区分原因）
    handshake_fail_untrust: u32,   // 因不信任 CA 失败
    handshake_fail_other: u32,     // 其他原因失败
    /// 最近一次失败的原因
    last_failure_reason: Option<TlsAcceptFailureReason>,
    /// 最近一次失败的域名
    last_failure_domain: Option<String>,
}
```

### 实现逻辑

#### 1. TLS 握手错误分类器（方案一）

修改 `tls_intercept_tunnel_with_cancel()` 和 `tls_intercept_tunnel()` 中的 `TlsAcceptor::accept()` 错误处理：

```rust
// 改造前的代码（已替换，当前实现已按改造后方案执行，见 mod.rs:1034 和 1161）
let mut client_tls = acceptor
    .accept(TokioIo::new(upgraded))
    .await
    .map_err(|e| BifrostError::Tls(format!("TLS accept failed: {e}")))?;

// 改造后
let mut client_tls = match acceptor.accept(TokioIo::new(upgraded)).await {
    Ok(tls) => tls,
    Err(e) => {
        let reason = classify_tls_accept_error(&e);

        // 上报到 ClientTlsTrustTracker
        if let Some(ref state) = admin_state {
            if let Some(ref tracker) = state.client_trust_tracker {
                tracker.record_handshake_failure(
                    client_ip,
                    client_app.as_deref(),
                    original_host,
                    &reason,
                );
            }
        }

        // 根据失败原因生成更精确的日志
        match &reason {
            TlsAcceptFailureReason::ClientDoesNotTrustCa => {
                warn!(
                    "[{}] TLS handshake failed: client does not trust Bifrost CA \
                     (host={}, client_ip={}, client_app={:?})",
                    req_id, original_host, client_ip, client_app
                );
            }
            TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa => {
                warn!(
                    "[{}] TLS handshake failed: client likely does not trust Bifrost CA \
                     (host={}, client_ip={}, client_app={:?})",
                    req_id, original_host, client_ip, client_app
                );
            }
            _ => {
                debug!("[{}] TLS handshake failed: {:?} ({})", req_id, reason, e);
            }
        }

        return Err(BifrostError::Tls(format!("TLS accept failed: {e}")));
    }
};
```

错误分类的核心函数：

```rust
fn classify_tls_accept_error(error: &std::io::Error) -> TlsAcceptFailureReason {
    let msg = error.to_string();
    let lower = msg.to_ascii_lowercase();

    // 1. 精确匹配 TLS Alert — 最可靠
    if lower.contains("unknownca") || lower.contains("unknown_ca") || lower.contains("unknown ca") {
        return TlsAcceptFailureReason::ClientDoesNotTrustCa;
    }
    if lower.contains("badcertificate") || lower.contains("bad_certificate") || lower.contains("bad certificate") {
        return TlsAcceptFailureReason::ClientDoesNotTrustCa;
    }
    if lower.contains("certificateunknown") || lower.contains("certificate_unknown") || lower.contains("certificate unknown") {
        return TlsAcceptFailureReason::ClientDoesNotTrustCa;
    }

    // 2. DecryptError — OpenSSL 客户端发送未加密 alert
    if lower.contains("decrypt") {
        return TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa;
    }

    // 3. 证书过期
    if lower.contains("certificateexpired") || lower.contains("certificate expired") {
        return TlsAcceptFailureReason::CertificateExpired;
    }

    // 4. 协议不兼容
    if lower.contains("handshakefailure") || lower.contains("protocolversion") {
        return TlsAcceptFailureReason::ProtocolIncompatible;
    }

    // 5. 连接重置
    if lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
    {
        return TlsAcceptFailureReason::ConnectionReset;
    }

    TlsAcceptFailureReason::Unknown
}
```

#### 2. 客户端信任追踪器（方案三）

```rust
pub struct ClientTlsTrustTracker {
    /// 按 client_ip 聚合
    by_ip: RwLock<HashMap<IpAddr, ClientTrustRecord>>,
    /// 按 client_app 聚合（仅本机客户端）
    by_app: RwLock<HashMap<String, ClientTrustRecord>>,
    /// 事件广播
    event_sender: broadcast::Sender<ClientTrustEvent>,
}
```

关键方法：

- `record_handshake_success(client_ip, client_app, domain)` — 握手成功时调用
- `record_handshake_failure(client_ip, client_app, domain, reason)` — 握手失败时调用
- `get_trust_status_by_ip(ip) -> ClientTrustStatus` — 查询某 IP 的信任状态
- `get_trust_status_by_app(app) -> ClientTrustStatus` — 查询某应用的信任状态
- `get_all_statuses() -> Vec<ClientTrustSummary>` — 列出所有客户端的信任状态
- `subscribe() -> Receiver<ClientTrustEvent>` — 订阅信任状态变更事件

#### 信任状态判定算法

```rust
fn evaluate_trust(record: &ClientTrustRecord) -> ClientTrustStatus {
    let total = record.handshake_success + record.handshake_fail_untrust + record.handshake_fail_other;

    if total == 0 {
        return ClientTrustStatus::Unknown;
    }

    // 如果有过明确的不信任 alert 且没有后续成功握手 → 确定不信任
    if record.handshake_fail_untrust > 0 && record.handshake_success == 0 {
        return ClientTrustStatus::NotTrusted {
            reason: format!("{:?}", record.last_failure_reason),
        };
    }

    // 如果有过明确的不信任 alert，但后续也有成功握手 → 说明中途安装了证书
    if record.handshake_fail_untrust > 0 && record.handshake_success > 0 {
        // 看最近的事件：如果最后一次是成功，说明现在已信任
        // 简化：只要有成功握手，且失败只在早期出现，视为已信任
        if record.last_seen > record.first_seen {
            return ClientTrustStatus::Trusted;
        }
    }

    // 只有成功，从没有不信任失败 → 信任
    if record.handshake_fail_untrust == 0 && record.handshake_success > 0 {
        return ClientTrustStatus::Trusted;
    }

    // 混合情况：计算置信度
    let fail_ratio = record.handshake_fail_untrust as f32 / total as f32;
    if fail_ratio > 0.8 {
        return ClientTrustStatus::LikelyUntrusted {
            confidence: fail_ratio,
            sample_count: total,
        };
    }

    ClientTrustStatus::Unknown
}
```

#### 3. 管理端 API

新增 API 端点：

```
GET /api/tls/client-trust          → 列出所有客户端的信任状态
GET /api/tls/client-trust/stream   → SSE 推送信任状态变更
```

响应示例：

```json
{
  "clients": [
    {
      "identifier": "192.168.1.100",
      "identifier_type": "ip",
      "trust_status": "not_trusted",
      "reason": "ClientDoesNotTrustCa",
      "handshake_success": 0,
      "handshake_fail_untrust": 12,
      "handshake_fail_other": 0,
      "first_seen": 1713360000,
      "last_seen": 1713361200,
      "last_failure_domain": "example.com"
    },
    {
      "identifier": "Safari",
      "identifier_type": "app",
      "trust_status": "trusted",
      "reason": null,
      "handshake_success": 45,
      "handshake_fail_untrust": 0,
      "handshake_fail_other": 1,
      "first_seen": 1713350000,
      "last_seen": 1713361000,
      "last_failure_domain": null
    },
    {
      "identifier": "Firefox",
      "identifier_type": "app",
      "trust_status": "not_trusted",
      "reason": "ClientDoesNotTrustCa",
      "handshake_success": 0,
      "handshake_fail_untrust": 8,
      "handshake_fail_other": 0,
      "first_seen": 1713355000,
      "last_seen": 1713361100,
      "last_failure_domain": "github.com"
    }
  ]
}
```

#### 4. Web UI 集成

在 Settings → Certificate 页面中，新增"客户端信任状态"区域：

```
┌─────────────────────────────────────────────────────────────┐
│ Certificate Trust Status                                     │
│                                                              │
│ 🖥️ This Machine                                             │
│   ├── System: ✅ Installed and trusted                       │
│   ├── Safari: ✅ Trusted (45 successful handshakes)          │
│   ├── Firefox: ❌ Not Trusted (uses own certificate store)   │
│   └── curl: ✅ Trusted (12 successful handshakes)            │
│                                                              │
│ 🌐 Remote Clients                                            │
│   ├── 192.168.1.100: ❌ Not Trusted (12 failed handshakes)  │
│   │   └── Last failure: example.com — unknown_ca             │
│   └── 192.168.1.101: ✅ Trusted (30 successful handshakes)  │
│                                                              │
│ ⚠️ Tip: Firefox 使用独立证书存储，需在 Firefox 设置中          │
│   单独导入 CA 证书。                                          │
└─────────────────────────────────────────────────────────────┘
```

#### 5. 与现有 IpTlsPendingManager 的关系

`ClientTlsTrustTracker` 与现有的 `IpTlsPendingManager` 功能互补但不重叠：

| 维度 | IpTlsPendingManager | ClientTlsTrustTracker |
|------|---------------------|-----------------------|
| 作用时机 | TLS 拦截决策之前（是否应该拦截） | TLS 拦截执行之后（拦截是否成功） |
| 目标问题 | 远程 IP 是否应该被拦截 | 客户端是否信任了 CA |
| 触发条件 | 新 IP 的 CONNECT 请求到来 | TLS 握手成功或失败 |
| 决策影响 | 控制是否启用 TLS 拦截 | 提供用户可见的诊断信息 |

未来可以考虑联动：当 `ClientTlsTrustTracker` 检测到某远程 IP 持续不信任 CA，可以建议用户为该 IP 关闭 TLS 拦截（通过 `IpTlsPendingManager` 的 skip 机制）。

## 涉及的代码修改

### 新增文件

| 文件 | 说明 |
|------|------|
| `crates/bifrost-admin/src/client_trust_tracker.rs` | `ClientTlsTrustTracker` 实现 |

### 修改文件

| 文件 | 修改内容 |
|------|---------|
| `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` | 在 `TlsAcceptor::accept()` 失败路径添加错误分类和上报；在成功路径添加成功上报 |
| `crates/bifrost-proxy/src/proxy/socks/tcp.rs` | SOCKS5 TLS 拦截路径同样添加错误分类和上报 |
| `crates/bifrost-admin/src/state.rs` | `AdminState` 新增 `client_trust_tracker` 字段 |
| `crates/bifrost-admin/src/lib.rs` | 导出新模块 |
| `crates/bifrost-admin/src/handlers/config.rs` | 新增 `/api/tls/client-trust` 端点 |
| `crates/bifrost-admin/src/push.rs` | 集成信任状态变更的 SSE 推送 |
| `crates/bifrost-cli/src/commands/start.rs` | 初始化 `ClientTlsTrustTracker` |
| `web/src/api/cert.ts` | 新增客户端信任状态 API 调用 |
| `web/src/pages/Settings/tabs/CertificateTab.tsx` | 新增客户端信任状态展示区域 |

## 依赖项

- 复用现有 `parking_lot` / `tokio::sync::broadcast`
- 复用现有 `serde` 序列化
- 无需新增外部依赖

## 测试方案

### 单元测试

- `classify_tls_accept_error` 函数的错误分类准确性：
  - 输入 `"peer sent fatal alert: UnknownCA"` → 返回 `ClientDoesNotTrustCa`
  - 输入 `"peer sent fatal alert: BadCertificate"` → 返回 `ClientDoesNotTrustCa`
  - 输入 `"decrypt error"` → 返回 `ProbablyClientDoesNotTrustCa`
  - 输入 `"peer sent fatal alert: HandshakeFailure"` → 返回 `ProtocolIncompatible`
  - 输入 `"connection reset by peer"` → 返回 `ConnectionReset`
  - 输入 `"unknown error xyz"` → 返回 `Unknown`
- `ClientTlsTrustTracker` 的记录与查询：
  - 连续记录成功 → 状态为 Trusted
  - 连续记录不信任失败 → 状态为 NotTrusted
  - 先失败后成功 → 状态转为 Trusted
  - 无记录 → 状态为 Unknown
- `evaluate_trust` 的边界条件：
  - 零样本 → Unknown
  - 混合成功/失败 → 根据比例判定

### 端到端测试（E2E）

- 启动代理（启用 TLS 拦截），使用**不信任 CA**的客户端发起 HTTPS 请求：
  - 验证代理日志中出现 `"client does not trust Bifrost CA"` 日志
  - 验证 `/api/tls/client-trust` 返回对应客户端的 NotTrusted 状态
- 启动代理（启用 TLS 拦截），使用**信任 CA**的客户端发起 HTTPS 请求：
  - 验证 `/api/tls/client-trust` 返回对应客户端的 Trusted 状态

### 真实场景测试（human_tests）

- 在 `human_tests/` 创建 `tls-client-trust-detection.md`：
  - TC-TCTD-01：启用 TLS 拦截，使用 Chrome（已安装 CA）访问 HTTPS 网站，验证管理界面显示 Chrome 为 Trusted
  - TC-TCTD-02：启用 TLS 拦截，使用 Firefox（未导入 CA）访问 HTTPS 网站，验证管理界面显示 Firefox 为 NotTrusted
  - TC-TCTD-03：远程设备（手机）未安装 CA 时，通过代理访问 HTTPS，验证管理界面显示该 IP 为 NotTrusted
  - TC-TCTD-04：在 Firefox 中手动导入 CA 后重新访问，验证状态从 NotTrusted 变为 Trusted
  - TC-TCTD-05：验证 TLS 拦截关闭时，不产生任何信任检测记录

## 校验要求（含 rust-project-validate）

- 先执行本次修改相关测试和 E2E
- 再执行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --all-features`
  - `cargo build --all-targets --all-features`

## 文档更新要求

- `README.md`：无需更新（此功能不改变用户可见的 CLI / 配置行为）
- 后续如果添加了用户可感知的 UI 变更或 API，再同步更新文档

## 风险与注意事项

1. **rustls 错误格式稳定性**：错误分类依赖 rustls 的 `Display` 输出格式。建议添加回归测试覆盖主要错误格式，当 rustls 升级时检查是否需要适配。
2. **内存占用**：`ClientTlsTrustTracker` 按 IP / App 聚合存储，长时间运行可能累积大量记录。建议设置容量上限（如最多 1000 条 IP 记录）并定期清理过期记录。
3. **误判风险**：`DecryptError` 可能不是因为不信任 CA（可能是网络干扰），因此归类为 `ProbablyClientDoesNotTrustCa` 而非 `ClientDoesNotTrustCa`，降低误判影响。
4. **隐私考虑**：追踪客户端 IP 和应用名可能涉及隐私。建议记录的生命周期仅限于当前 Bifrost 运行周期，重启后清空，不持久化到磁盘。

## 实现优先级建议

| 阶段 | 内容 | 价值 | 工作量 |
|------|------|------|--------|
| P0 | TLS 握手错误分类 + 精准日志 | 立即可用的调试信息 | 小 |
| P1 | ClientTlsTrustTracker + Admin API | 完整的信任状态查询 | 中 |
| P2 | Web UI 信任状态展示 | 用户友好的可视化 | 中 |
| P3 | SSE 实时推送 + IpTlsPendingManager 联动 | 自动化运维 | 小 |

---

## TLS 不信任域名交互式 Passthrough

### 功能描述

当 `ClientTlsTrustTracker` 检测到客户端不信任某个域名的 TLS 证书时，系统会创建一条 `tls_trust_change` 类型的通知记录。本功能在两个 UI 层面提供交互式处理入口，让用户可以一键将不信任域名加入 TLS Passthrough 列表（即跳过该域名的 TLS 拦截）：

1. **Layout 全局 Toast 通知**：当有新的未读 `tls_trust_change` 通知产生时，`AppLayout` 组件会在页面右上角弹出 `notification.warning` Toast，包含域名信息及两个操作按钮：
   - **Passthrough**：调用 `useTlsConfigStore.addDomainToPassthrough(domain)` 将域名加入 `intercept_exclude` 列表，同时将通知标记为 `read` + `action_taken: "passthrough"`
   - **Ignore**：将通知标记为 `dismissed` + `action_taken: "ignored"`，不做配置变更

2. **Notifications 表格行内操作**：在 `/notifications` 页面的 `NotificationsTable` 中，`tls_trust_change` 类型记录的 Actions 列会根据当前状态渲染不同内容：
   - 未处理时：显示 **Passthrough** 和 **Ignore** 两个按钮（逻辑同 Toast）
   - 已执行 Passthrough：显示绿色 `Passthrough ✓` 标签
   - 已忽略/已 Dismiss：显示灰色 `Ignored` 标签

两个入口共享同一套数据流：`notification.metadata` 中的 JSON 字段 `{ "domain": "<域名>" }` 提供目标域名，`useTlsConfigStore.addDomainToPassthrough` 提供配置变更能力。

### 实现方案

#### Layout Toast 增强（`web/src/components/Layout/index.tsx`）

`AppLayout` 通过轮询 `useNotificationStore.unreadCount` 变化触发 `handleNotificationToast` 回调：

1. 当 `unreadCount` 增加时，调用 `getNotifications({ status: "unread", limit: 20 })` 获取最新未读通知
2. 对每条通知进行去重检查（`shownToastIdsRef`），过滤已弹出的 Toast
3. 对 `notification_type === "tls_trust_change"` 的通知，解析 `JSON.parse(item.metadata)` 提取 `meta.domain`
4. 弹出 `notification.warning` Toast（`duration: 0` 不自动关闭），包含：
   - 描述文本：`Domain <strong>{domain}</strong> is not trusted by the client.`
   - Passthrough 按钮：`await addDomainToPassthrough(domain)` → `updateNotificationStatus(id, "read", "passthrough")` → `fetchUnreadCount()` → `notification.destroy(key)`
   - Ignore 按钮：`updateNotificationStatus(id, "dismissed", "ignored")` → `fetchUnreadCount()` → `notification.destroy(key)`
5. 非 TLS 类通知累计到 `genericCount`，统一弹出通用 Toast

关键依赖：
- `useTlsConfigStore` 的 `addDomainToPassthrough` 和 `fetchConfig`
- `useNotificationStore` 的 `unreadCount` 和 `fetchUnreadCount`
- `api/notifications` 的 `getNotifications` 和 `updateNotificationStatus`

#### Notifications 表格操作（`web/src/pages/Notifications/index.tsx`）

`NotificationsTable` 组件中：

1. 通过 `useTlsConfigStore((s) => s.addDomainToPassthrough)` 获取 passthrough 操作方法
2. 定义 `handlePassthrough(id, domain)` 回调：先 `addDomainToPassthrough(domain)`，成功后 `handleUpdateStatus(id, "read", "passthrough")`
3. 定义 `handleIgnore(id)` 回调：`handleUpdateStatus(id, "dismissed", "ignored")`
4. 在 Actions 列的 `render` 函数中，对 `tls_trust_change` 类型记录：
   - 从 `JSON.parse(record.metadata)` 提取 `domain`
   - 根据 `record.action_taken` 判断已处理状态，渲染对应标签
   - 未处理时渲染 Passthrough / Ignore 按钮对

Domain 列同样通过 `JSON.parse(metadata)` 提取 `parsed.domain` 进行展示。

#### `useTlsConfigStore.addDomainToPassthrough` 复用（`web/src/stores/useTlsConfigStore.ts`）

此方法已存在且被 Toast 和 Notifications 表格共同复用，核心逻辑：

1. 检查 `config.intercept_exclude` 是否已包含该域名（幂等）
2. 将域名追加到 `intercept_exclude` 列表
3. 如果域名同时存在于 `intercept_include` 列表中，自动将其移除（互斥保证）
4. 调用 `updateTlsConfig({ intercept_exclude: newList, intercept_include: includeList })` 持久化
5. 更新本地 store 状态

### 测试方案

#### 单元测试

- `test_parse_tls_trust_notification_metadata_domain`：验证从 `metadata: '{"domain":"example.com"}'` 中正确解析出域名
- `test_parse_tls_trust_notification_metadata_null`：验证 `metadata: null` 时返回 `-`
- `test_parse_tls_trust_notification_metadata_invalid_json`：验证非法 JSON 时返回 `-`
- `test_parse_tls_trust_notification_metadata_missing_domain`：验证 `metadata: '{"foo":"bar"}'`（无 domain 字段）时返回 `-`
- `test_add_domain_to_passthrough_idempotent`：验证重复添加同一域名时直接返回 `true` 且不重复请求 API
- `test_add_domain_to_passthrough_removes_from_intercept`：验证域名从 `intercept_include` 自动移除

#### 端到端测试（E2E）

- 启动代理（启用 TLS 拦截），使用不信任 CA 的客户端访问 HTTPS 目标域名，验证：
  - `/api/notifications` 返回包含 `tls_trust_change` 类型、`metadata` 含目标域名的记录
  - 调用 Passthrough 操作后，`/api/config/tls` 返回的 `intercept_exclude` 包含该域名
  - 再次访问该域名时，代理对其执行 passthrough（不拦截 TLS）
- 验证 Notifications 页面 Actions 列状态流转：
  - 初始状态显示 Passthrough / Ignore 按钮
  - 点击 Passthrough 后显示绿色 `Passthrough ✓`
  - 点击 Ignore 后显示灰色 `Ignored`
- 验证 Toast 行为：
  - 新 `tls_trust_change` 通知触发 Toast 弹出
  - Toast 中点击 Passthrough 后 Toast 关闭，域名加入 passthrough 列表
  - Toast 中点击 Ignore 后 Toast 关闭，通知标记为 dismissed

#### 真实场景测试（human_tests）

在 `human_tests/` 创建 `tls-passthrough-interactive.md`，包含以下用例：

- **TC-TPI-01**：启用 TLS 拦截，使用 Chrome（未安装 CA）访问 `https://httpbin.org`，等待 Toast 弹出，确认 Toast 显示域名 `httpbin.org` 及 Passthrough / Ignore 按钮
- **TC-TPI-02**：在 TC-TPI-01 的 Toast 中点击 Passthrough，验证：Toast 消失；Settings → TLS 的 Exclude 列表中出现 `httpbin.org`；重新访问 `https://httpbin.org` 不再触发 TLS 拦截错误
- **TC-TPI-03**：在 TC-TPI-01 的 Toast 中点击 Ignore，验证：Toast 消失；Settings → TLS 的 Exclude 列表中不出现该域名；Notifications 页面对应记录显示 `Ignored`
- **TC-TPI-04**：打开 `/notifications?tab=tls_trust_change` 页面，找到未处理的 `tls_trust_change` 记录，验证 Domain 列正确显示域名，Actions 列显示 Passthrough 和 Ignore 按钮
- **TC-TPI-05**：在 Notifications 表格中点击 Passthrough 按钮，验证：Actions 列变为绿色 `Passthrough ✓` 标签；TLS 配置已更新
- **TC-TPI-06**：在 Notifications 表格中点击 Ignore 按钮，验证：Actions 列变为灰色 `Ignored` 标签；TLS 配置未变更
