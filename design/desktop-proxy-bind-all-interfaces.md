# 桌面端代理监听所有网络接口

## 问题

桌面端 (Tauri) 启动后端时硬编码 `--host 127.0.0.1`，导致代理服务只监听本地回环地址。
外部设备（手机、其他电脑）无法通过局域网 IP 连接代理，丧失了代理工具的核心能力。

CLI 模式默认绑定 `0.0.0.0`，桌面端应保持同等能力。

## 实现方案

### 常量拆分

原始代码只有一个 `BACKEND_HOST = "127.0.0.1"` 混用于：
1. 代理服务绑定地址 → 应该是 `0.0.0.0`
2. Admin API 访问地址 → 必须是 `127.0.0.1`
3. 端口可用性检测 → 应该是 `0.0.0.0`
4. 日志/错误信息显示 → 保持 `127.0.0.1`

拆分为两个常量：
- `BACKEND_BIND_HOST = "0.0.0.0"` — 代理服务绑定地址 + 端口可用性检测
- `BACKEND_ADMIN_HOST = "127.0.0.1"` — Admin API 调用 + 日志显示

### 安全性

Admin API 安全机制不受影响（位于 `crates/bifrost-admin/src/security.rs`）：
- `is_valid_admin_request()` 校验 `peer_addr.ip().is_loopback()`
- `AdminSecurityConfig::allowed_hosts` 仅允许 `127.0.0.1` / `localhost`（含带端口形式）
- 即使代理绑定 `0.0.0.0`，外部请求的 peer_addr 不是 loopback，会被拒绝

### 变更文件

- `desktop/src-tauri/src/main.rs` — 拆分常量 + 启动参数改用 `BACKEND_BIND_HOST`
- `crates/bifrost-admin/src/network.rs` — `get_local_ips()` 及其单元测试（既有实现，本次方案直接复用）

## 测试方案

### 单元测试
- `test_get_local_ips_returns_non_empty`: 验证 `get_local_ips()` 至少返回一个条目
- `test_get_local_ips_preferred_is_first`: 验证首选 IP 排在第一位
- `test_get_local_ips_no_duplicates`: 验证无重复条目
- `test_get_local_ips_all_entries_are_valid_addresses`: 验证所有返回条目均为有效地址

### E2E 测试
- 验证代理服务绑定 `0.0.0.0` 后可通过非 loopback 地址访问

### 真实场景测试
- 启动桌面端，通过局域网 IP 连接代理服务，验证外部设备可用
