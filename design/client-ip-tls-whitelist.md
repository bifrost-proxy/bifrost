# Client IP TLS Whitelist — 基于客户端 IP 的 TLS 解包控制

## 功能概述

新增基于 Client IP 地址的 TLS 拦截（解包）控制维度，支持白名单（include）和黑名单（exclude）。其优先级 **低于** 规则级、域名级、应用级配置，但 **高于** TLS 全局开关。

同时引入"新 IP 连接通知"机制：当首次出现一个未在任何 IP 列表中的远程 IP 发起 TLS 连接时，通过 WebSocket 推送通知到 WebUI，弹出交互提示让用户选择"启用 TLS 解包"或"跳过"。用户选择后，该 IP 自动加入对应列表并持久化。

## 优先级设计

TLS 拦截决策链（从高到低）：

1. 无 CA 证书 → 不拦截
2. 规则级 `tls_intercept` 字段 → 最高优先级
3. Host 重写需要拦截 → 强制拦截
4. 应用包含/排除列表（仅本地客户端）
5. 域名包含/排除列表
6. **【新增】IP 包含列表（ip_intercept_include）→ 拦截**
7. **【新增】IP 排除列表（ip_intercept_exclude）→ 不拦截**
8. 全局开关 `enable_tls_interception` → 兜底

## 数据模型

### TlsConfig（持久化层 - unified_config.rs）

```rust
pub struct TlsConfig {
    // ... 现有字段 ...
    pub ip_intercept_exclude: Vec<String>,   // IP 黑名单（不解包）
    pub ip_intercept_include: Vec<String>,   // IP 白名单（强制解包）
}
```

### TlsConfigUpdate

```rust
pub struct TlsConfigUpdate {
    // ... 现有字段 ...
    pub ip_intercept_exclude: Option<Vec<String>>,
    pub ip_intercept_include: Option<Vec<String>>,
}
```

### RuntimeConfig / TlsInterceptConfig / ProxyConfig

同步新增 `ip_intercept_exclude` 和 `ip_intercept_include` 字段。

## IP 匹配规则

复用项目中已有的 `IpNet` 模式（参考 access_control.rs）：
- 支持单 IP：`192.168.1.100`
- 支持 CIDR：`192.168.1.0/24`
- 支持 IPv6：`::1`、`fe80::/10`

## 新 IP TLS 通知机制

### 触发条件

在 `handle_connect` 中，当：
1. 客户端为非本地 IP（`!is_loopback()`）
2. 该 IP 不在 `ip_intercept_include` 或 `ip_intercept_exclude` 中
3. 该 IP 不在已知的 pending/session-decided 列表中

→ 触发通知推送。

### 后端实现

在 `AdminState` 中新增 `IpTlsPendingManager`：
- `pending: RwLock<Vec<(IpAddr, u64, u32)>>` — 待决定的 IP 列表
- `session_decided: RwLock<HashSet<IpAddr>>` — 本次会话已决定的 IP（避免重复弹窗）
- `event_sender: broadcast::Sender<PendingIpTlsEvent>` — 事件广播

### 推送消息

```json
{
  "event_type": "new",
  "ip": "192.168.1.100",
  "first_seen": 1712345678,
  "attempt_count": 1,
  "total_pending": 1
}
```

### API 端点

| 路径 | 方法 | 功能 |
|------|------|------|
| `/api/config/ip-tls/pending` | GET | 获取待决定的 IP 列表 |
| `/api/config/ip-tls/pending/stream` | GET | SSE 流订阅新 IP 事件 |
| `/api/config/ip-tls/pending/approve` | POST | 审批 IP（加入 include 列表） |
| `/api/config/ip-tls/pending/skip` | POST | 跳过 IP（加入 exclude 列表） |
| `/api/config/ip-tls/pending` | DELETE | 清空 pending 列表 |

## 前端组件

### PendingTlsIpModal

参照现有 `PendingAuthModal` 的交互模式：
- 当有 pending IP 时自动弹出模态框
- 列出每个 IP，提供 "Enable TLS" 和 "Skip" 两个操作按钮
- Enable → 调用 approve API → IP 加入 ip_intercept_include
- Skip → 调用 skip API → IP 加入 ip_intercept_exclude
- 支持 Clear All 批量操作
- 支持跳转到 Settings 页面

### Settings 页面扩展

在 TLS Config 区域新增 "IP TLS Whitelist" section，展示和管理 `ip_intercept_include` / `ip_intercept_exclude` 列表。

## 测试方案

### 单元测试

- `test_ip_intercept_include_match`：验证 IP 在 include 列表中 → 返回拦截
- `test_ip_intercept_exclude_match`：验证 IP 在 exclude 列表中 → 返回不拦截
- `test_ip_intercept_cidr_match`：验证 CIDR 匹配正确
- `test_ip_tls_priority_below_domain`：验证域名 include 优先级高于 IP exclude
- `test_ip_tls_priority_above_global`：验证 IP include 优先级高于全局关闭

### E2E 测试

- 验证新 IP 连接触发通知推送
- 验证 approve API 将 IP 加入 include 列表
- 验证 skip API 将 IP 加入 exclude 列表
- 验证配置持久化和重启后恢复

### 真实场景测试

- 启动服务，从远端 IP 发起 HTTPS 请求，观察 WebUI 弹窗
- 点击 Enable TLS 后确认后续请求被解包
- 点击 Skip 后确认后续请求直通
