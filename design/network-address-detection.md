# 网络地址智能识别

## 功能描述

解决当机器存在多个网络接口（物理网卡、VPN 隧道、Docker 桥接、虚拟机网络等）时，代理地址和证书下载 URL 展示了过多无效 IP 的问题。

## 问题分析

原有实现（`proxy.rs`、`cert.rs`、`push.rs` 各自独立的 `get_local_ips` 函数）只过滤了"是否为私有 IP"，未考虑：
1. 虚拟网络接口（docker0、veth、utun、vmnet 等）的 IP 对用户无意义
2. 接口状态（未激活、未运行的接口不应展示）
3. 用户无法区分哪个 IP 是主要可用的

## 实现方案

### 统一模块：`bifrost-admin/src/network.rs`

提取公共 IP 获取逻辑为独立模块，三处调用方统一复用。

### 三层过滤策略

1. **接口 flags 过滤**（仅 Unix）
   - 要求 `IFF_UP` + `IFF_RUNNING`：接口必须已激活且正在运行
   - 排除 `IFF_LOOPBACK`：排除回环接口

2. **接口名称过滤**
   - 过滤已知虚拟接口前缀：`docker`、`br-`、`veth`、`vnet`、`virbr`、`cni`、`flannel`、`calico`、`weave`、`cilium`、`lxc`、`lxd`、`podman`、`tun`、`tap`、`wg`、`tailscale`、`utun`、`ipsec`、`ppp`、`vmnet`、`vmware`、`vboxnet`、`bridge`、`dummy`
   - 大小写不敏感匹配

3. **IP 地址过滤**（`is_effective_client_ip`）
   - 保留 IPv4 私有地址（`10.x`、`172.16-31.x`、`192.168.x`）
   - 保留 CGN 地址（`100.64.0.0/10`，即 `100.64-127.x`，RFC 6598）
   - 保留可路由的公网 IPv4（用于在公网可达接口上对外暴露代理）
   - 排除回环（`127.x`）、链路本地（`169.254.x`）、未指定（`0.x`）、多播 / 广播、文档段（`192.0.2.0/24`）、benchmark 段（`198.18.0.0/15`）、`>=224.x` 的多播/保留段，以及全部 IPv6

   > 注：早期实现叫 `is_routable_private_ip`，只接受私有/CGN 地址；当前实现已更名为 `is_effective_client_ip`，语义放宽为对客户端有意义的 IPv4 地址，公网 IP 会被一并保留。

### Preferred IP 检测

通过 UDP socket 连接 `8.8.8.8:80`（不实际发送数据），获取操作系统路由表选择的默认出口 IP，经过 `is_effective_client_ip` 校验后标记为 `is_preferred`。回环 / 未指定 / 链路本地 / 多播 / 文档段 / benchmark 段等特殊地址会被丢弃；私有、CGN、公网可路由地址都会被保留。

### API 变更

`ProxyAddress` 结构体新增 `is_preferred: bool` 字段：

```json
{
  "ip": "10.71.149.76",
  "address": "10.71.149.76:8800",
  "qrcode_url": "/_bifrost/public/proxy/qrcode?ip=10.71.149.76",
  "is_preferred": true
}
```

结果列表按 preferred 优先排序。

### 前端展示

preferred IP 的地址卡片上展示绿色 "Recommended" 标签。

## 关键文件

- `crates/bifrost-admin/src/network.rs` — 核心实现（IP 获取、接口过滤、preferred 检测）
- `crates/bifrost-core/src/access_control.rs` — 访问控制（`is_private_network()` 用于 allow_lan 判定）
- `crates/bifrost-admin/src/handlers/proxy.rs` — 代理地址 API
- `crates/bifrost-admin/src/handlers/cert.rs` — 证书信息 API
- `crates/bifrost-admin/src/push.rs` — WebSocket 推送
- `web/src/api/proxy.ts` — 前端类型定义
- `web/src/pages/Settings/tabs/ProxyTab.tsx` — 前端展示

## CGN 地址段支持（RFC 6598）

### 背景

部分用户的机器 IP 为 `100.64.0.0/10`（CGN，Carrier-Grade NAT）地址段，常见于：
- ISP 的 CGN/CGNAT 网络
- 企业 VPN（如 Tailscale、WireGuard）
- 云服务商内网

该地址段不属于 RFC 1918 私有地址，但也不是公网可路由地址，属于 IANA 为运营商级 NAT 保留的共享地址空间。

### 修复内容

1. **`access_control.rs` 的 `is_private_network()`**：采用双重判定策略：
   - **优先**：同子网判定 — 检查连接 IP 是否与本机任一网络接口在同一子网（通过 `IpNet::contains()`）
   - **降级**：私有网段判定 — 匹配 RFC 1918 + CGN `100.64.0.0/10` + link-local
   - 启动时通过 `get_local_subnets()` 获取本机所有网络接口的 IP + netmask，计算子网列表
2. **`network.rs` 的 `is_effective_client_ip()`**（原 `is_routable_private_ip`，已重命名并放宽语义）：新增 CGN 地址段支持，使 CGN IP 能被正确展示在本机 IP 列表中；同时也保留公网可路由地址
3. **`network.rs` 的 `detect_preferred_ip()`**：增加 `is_effective_client_ip()` 校验，丢弃回环 / 链路本地 / 多播 / 文档段 / benchmark 段等特殊地址
4. **`network.rs` 的 `get_local_subnets()`**：新增函数，通过 `nix::ifaddrs` 获取本机所有网络接口的 IP 和 netmask，计算并返回去重后的子网列表（Windows 走 `local_ip_address::list_afinet_netifas`，netmask 不可得时退化为 `/24`）

### 同子网判定架构

```
连接来源 IP  ──>  is_private_network()
                    │
                    ├── is_in_local_subnet() ──> 遍历本机子网列表，子网掩码匹配
                    │     (优先路径：精确的同网段判定)
                    │
                    └── is_private_range() ──> 硬编码的私有网段列表
                          (降级路径：覆盖无法获取 netmask 的场景)
```

### 子网信息周期刷新

启动时通过 `get_local_subnets()` 获取本机子网快照并设置到 `access_control` 中。服务运行过程中，如果本地网络环境发生变化（VPN 连接/断开、WiFi 切换、网卡 IP 变更等），子网快照会过时，导致访问控制判定和 WebUI IP 列表展示可能不准确。

为此，在 `start_push_tasks` 中添加了子网周期刷新后台任务：

- **刷新周期**：每 30 秒检测一次
- **变更检测**：对比当前子网快照与最新 `get_local_subnets()` 结果，仅在发生变化时更新
- **热更新**：通过 `RwLock<Vec<IpNet>>` 实现运行时原子替换，对正在处理的请求无影响
- **WebUI 实时推送**：检测到子网变化后，自动广播 `proxy_address` 和 `cert_info` 两个 settings scope，已连接的 WebUI 客户端会立即收到最新的 IP 列表和证书下载地址，无需手动刷新页面
- **日志记录**：子网变化时输出 info 级别日志，包含 old/new 子网列表
- **自动覆盖**：由于在 `start_push_tasks` 中实现，rebind 等重启路径也自动获得刷新能力

## 测试方案

### 单元测试（`crates/bifrost-admin/src/network.rs`）
- `test_is_virtual_interface_name_filters_*` — 验证各类虚拟接口名被正确识别（docker / veth / bridge / vpn / vm / container_orchestration）
- `test_is_virtual_interface_name_allows_physical` — 验证物理接口不被误过滤
- `test_is_virtual_interface_name_case_insensitive` — 验证大小写不敏感匹配
- `test_is_effective_client_ip_accepts_private` / `test_is_effective_client_ip_accepts_cgn` / `test_is_effective_client_ip_accepts_public` — 验证私有 / CGN / 公网 IP 都被保留
- `test_is_effective_client_ip_rejects_loopback` / `test_is_effective_client_ip_rejects_link_local` / `test_is_effective_client_ip_rejects_ipv6` / `test_is_effective_client_ip_rejects_special_public_ranges` — 验证特殊地址段被排除（文档段、benchmark 段、多播等）
- `test_get_local_ips_returns_non_empty` / `test_get_local_ips_preferred_is_first` / `test_get_local_ips_no_duplicates` / `test_get_local_ips_all_entries_are_valid_addresses` — 验证返回值非空、preferred 排序、无重复、地址合法

### 单元测试（`crates/bifrost-core/src/access_control.rs`）
- `test_private_network_detection` — 验证 `is_private_network` 涵盖 RFC 1918 + CGN + link-local
- `test_cgn_address_allowed_with_allow_lan` — 验证 CGN IP 在 allow_lan=true 时直接 Allow
- `test_cgn_address_prompts_without_allow_lan` — 验证 CGN IP 在 allow_lan=false 时触发 Prompt
- `test_local_subnet_detection_allows_same_subnet` — 验证同子网的公网 IP 被视为局域网
- `test_local_subnet_detection_any_public_ip_in_same_subnet` — 验证 CGN 子网精确匹配
- `test_subnet_hot_update_changes_access_decision` — 验证运行时更新子网后访问决策实时变化

### E2E 测试
- `admin_api_proxy_address_with_preferred_ip` — 验证 API 返回含 `is_preferred` 字段、preferred IP 排在首位 (planned, not yet shipped as of 2026-06-16)

### 真实场景测试
- 启动服务后调用 `GET /api/proxy/address`，对比 `ifconfig` 输出验证虚拟接口被正确过滤
