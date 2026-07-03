# 网络地址智能识别 设计方案

## 背景

Bifrost 需要在多个位置对外展示 "本机可访问的代理地址" 与 "证书下载地址"：

- Settings > Proxy 页展示所有可用局域网 IP + 端口，供其他设备配置代理。
- Settings > Certificate 页展示局域网 CA 下载 URL 与二维码。
- WebSocket push 通道 `proxy_address` / `cert_info` 用于实时刷新前端。

历史实现里，`handlers/proxy.rs`、`handlers/cert.rs`、`push.rs` 各自维护一份 `get_local_ips`，只做了 "是否为私有 IP" 的粗过滤，导致：

- Docker、veth、utun、vmnet 等虚拟接口的 IP 全部展示，用户根本猜不到该用哪个。
- 未激活 / 未运行的接口也进入列表。
- 没有 "推荐地址" 概念，多 IP 环境下用户体验极差。
- CGN（`100.64.0.0/10`，Tailscale/CGNAT/企业 VPN 常见）地址被误判成公网，反而不展示。
- 网络环境变化（VPN 上下线、Wi-Fi 切换）后，前端 IP 列表和访问控制的子网列表都会 stale。

本方案把 IP 收集、接口过滤、preferred 检测、子网快照收拢到统一模块 `crates/bifrost-admin/src/network.rs`，并在服务运行时持续刷新子网快照。

## 用户目标验证清单

### 必须实现

- 三处（proxy、cert、push）复用同一个 `get_local_ips` 入口，不再各写各的。
- 三层过滤：接口 flags（Unix `IFF_UP` + `IFF_RUNNING` 且非 loopback）+ 接口名前缀黑名单 + IP 地址语义过滤。
- 结果列表包含 `is_preferred: bool` 字段；preferred 项排在最前。
- Preferred 检测通过 UDP `connect 8.8.8.8:80`（不发数据）借操作系统路由表拿到默认出口 IP，再走 `is_effective_client_ip` 校验。
- 支持 CGN `100.64.0.0/10`：既在 `is_effective_client_ip` 保留，又在 `access_control::is_private_network` 命中同子网/私有段判定。
- `access_control` 通过 `get_local_subnets()` 拿本机所有接口 IP+netmask，构建同子网判定；`is_private_network` 优先走同子网，降级走硬编码私有段。
- `start_push_tasks` 后台 30 秒周期性刷新子网快照，检测到变化时热更新（`RwLock<Vec<IpNet>>`）+ 广播 `proxy_address` / `cert_info` scope。

### 必须不破坏

- 现有 `/api/proxy/address` 与 `/api/cert/info` 响应结构兼容；只是新增 `is_preferred`。
- 前端 `web/src/api/proxy.ts` 类型扩展向后兼容。
- 未安装 Docker/VPN 的干净物理机上，`get_local_ips` 返回值等价旧行为。
- Windows 平台走 `local_ip_address::list_afinet_netifas`，netmask 不可得时退化为 `/24`。
- `is_private_network` 对 RFC 1918、link-local 仍然直通。

### 必须真实验证

- 单元测试覆盖 docker/veth/bridge/vpn/vm/container_orchestration 六类虚拟接口名过滤。
- 单元测试覆盖 private / CGN / public / loopback / link-local / IPv6 / 文档段 / benchmark 段的 `is_effective_client_ip`。
- 单元测试覆盖同子网 CGN 精确匹配与运行时热更新后访问决策变化。
- 真实场景：VPN 上下线后 30 秒内前端 IP 列表自动更新，无需刷新页面。

## 产品语义

### 三层过滤策略

1. **接口 flags 过滤（仅 Unix，`is_usable_interface`）**
   - 要求 `IFF_UP` + `IFF_RUNNING`；排除 `IFF_LOOPBACK`。
2. **接口名前缀过滤（`is_virtual_interface_name`）**
   - 前缀黑名单：`docker`、`br-`、`veth`、`vnet`、`virbr`、`cni`、`flannel`、`calico`、`weave`、`cilium`、`lxc`、`lxd`、`podman`、`tun`、`tap`、`wg`、`tailscale`、`utun`、`ipsec`、`ppp`、`vmnet`、`vmware`、`vboxnet`、`bridge`、`dummy`。
   - 大小写不敏感。
3. **IP 地址过滤（`is_effective_client_ip`）**
   - 保留：IPv4 私有段 (`10.x` / `172.16-31.x` / `192.168.x`)、CGN (`100.64.0.0/10`)、可路由公网 IPv4。
   - 排除：`127.x`、`169.254.x`、`0.x`、多播/广播、`192.0.2.0/24`（文档段）、`198.18.0.0/15`（benchmark 段）、`>=224.x`、全部 IPv6。

> 历史命名 `is_routable_private_ip` 只接受私有/CGN。当前统一为 `is_effective_client_ip`，语义放宽为 "对客户端有意义的 IPv4"，公网 IP 也保留。

### Preferred IP 检测

`detect_preferred_ip`（network.rs:101）：

1. UDP socket `connect("8.8.8.8:80")`（不实际发数据，仅让 OS 选路）。
2. 读 `local_addr()` 获得默认出口 IP。
3. 用 `is_effective_client_ip` 校验，特殊地址丢弃。

命中时列表中对应项 `is_preferred = true`，并排在最前。

### `is_private_network` 双路径判定

`crates/bifrost-core/src/access_control.rs`：

```
连接来源 IP  ──>  is_private_network()
                    │
                    ├── is_in_local_subnet() → 遍历本机子网列表，精确子网掩码匹配（优先）
                    │
                    └── is_private_range()  → RFC 1918 + CGN 100.64.0.0/10 + link-local（降级）
```

优先路径准确覆盖 "同网段公网 IP" 与 "CGN 子网"；降级路径覆盖 netmask 不可得或子网快照未加载完的场景。

### 子网快照热更新

- 启动时 `get_local_subnets()` 写入 `RwLock<Vec<IpNet>>`。
- `start_push_tasks` 起后台任务，每 30 秒重取一次；变化时：
  - 原子替换 `RwLock` 内容。
  - 广播 `proxy_address` 和 `cert_info` push scope，前端立即拿到新 IP 列表和 CA 下载地址。
  - info 日志记录 old/new 子网列表。
- rebind 等场景由于走同一 `start_push_tasks`，自动获得刷新能力。

## 技术细节

### 核心文件

- `crates/bifrost-admin/src/network.rs`（当前实现 ~400 行）
- `crates/bifrost-core/src/access_control.rs`（`is_private_network`、`get_local_subnets` 消费者、`RwLock<Vec<IpNet>>`）
- `crates/bifrost-admin/src/handlers/proxy.rs`
- `crates/bifrost-admin/src/handlers/cert.rs`
- `crates/bifrost-admin/src/push.rs`（`start_push_tasks` 内嵌 30 秒 refresh）
- `web/src/api/proxy.ts`
- `web/src/pages/Settings/tabs/ProxyTab.tsx`

### 关键符号（`network.rs`）

| 符号 | 行号 | 作用 |
| ---- | ---- | ---- |
| `get_local_ips` | 11 | 主入口，返回 `Vec<LocalIpInfo>` |
| `detect_preferred_ip` | 101 | UDP 借路由表选默认出口 IP |
| `is_effective_client_ip` | 115 | IP 语义过滤 |
| `is_usable_interface` | 144 | 接口 flags 过滤 |
| `is_virtual_interface_name` | 159 | 前缀黑名单 |
| `get_local_subnets` | 192 | 收集本机所有子网 |

### API 响应

`GET /api/proxy/address` 返回：

```json
{
  "addresses": [
    {
      "ip": "10.71.149.76",
      "address": "10.71.149.76:8800",
      "qrcode_url": "/_bifrost/public/proxy/qrcode?ip=10.71.149.76",
      "is_preferred": true
    }
  ]
}
```

`GET /api/cert/info` 返回的 `download_urls[]` 结构对齐同一份 IP 列表 + `is_preferred`。

### Push scope

`/api/push` 支持两个 settings scope：

- `proxy_address` — 30 秒周期性检测子网变化时广播；WebSocket 新连接立即补发当前快照。
- `cert_info` — 同上，同时也会在证书本身变化时触发。

前端 `web/src/services/pushService.ts` 订阅上述 scope，收到消息后刷新 ProxyTab / CertificateTab 展示。

## CLI + Web + Admin API

### CLI

无新增子命令。`bifrost status` 输出的 admin URL 也走 `get_local_ips`，preferred 项排在最前。

### Web UI

- ProxyTab (`web/src/pages/Settings/tabs/ProxyTab.tsx`) 展示所有地址；preferred 项显示 "Recommended" 绿色标签。
- 二维码链接使用 `qrcode_url`。
- 前端不再本地判断 preferred，完全信任服务端 `is_preferred`。

### Admin API

- `GET /api/proxy/address`：返回带 `is_preferred` 的 IP 列表。
- `GET /api/cert/info`：download URLs 复用同一列表 + `is_preferred`。
- `/api/push` `proxy_address` / `cert_info` scope 支持首次订阅补发 + 子网变化广播。

## Sync 边界

- IP 列表、子网快照、preferred 结果都是每台设备的本地网络状态，不参与任何 sync。
- `access_control` 子网快照是本机运行时状态，也不 sync。
- Push 通道只推给本机 WebSocket 订阅者，远程 Admin 不感知本机 VPN 上下线。

## Phase 拆分

### Phase 1：统一 network 模块

- 抽出 `bifrost-admin/src/network.rs`，实现 `get_local_ips` + 三层过滤 + `detect_preferred_ip` + `is_effective_client_ip`。
- `handlers/proxy.rs`、`handlers/cert.rs`、`push.rs` 全部迁到统一入口。
- 单元测试覆盖虚拟接口名、IP 语义过滤、preferred 排序。

### Phase 2：CGN 支持与同子网判定

- `is_effective_client_ip` 加 CGN。
- `access_control` 引入 `is_private_network` 双路径 + `get_local_subnets()` + `RwLock<Vec<IpNet>>`。
- 单元测试覆盖 CGN allow_lan 直通与 prompt、同子网精确匹配。

### Phase 3：子网热更新

- `start_push_tasks` 起 30 秒 refresh 任务。
- 变更检测：diff 当前 vs 新快照，仅在变化时更新。
- 广播 `proxy_address` / `cert_info` push scope。
- 单元测试覆盖热更新后访问决策实时变化。

### Phase 4：Web + 文档

- `ProxyTab` `Recommended` 标签。
- `/api/proxy/address` 响应结构文档同步。
- README / 用户文档说明多 IP 展示与推荐逻辑。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/network.rs` 内嵌 tests）

- `test_is_virtual_interface_name_filters_docker`
- `test_is_virtual_interface_name_filters_veth`
- `test_is_virtual_interface_name_filters_bridge`
- `test_is_virtual_interface_name_filters_vpn`
- `test_is_virtual_interface_name_filters_vm`
- `test_is_virtual_interface_name_filters_container_orchestration`
- `test_is_virtual_interface_name_allows_physical`
- `test_is_virtual_interface_name_case_insensitive`
- `test_is_effective_client_ip_accepts_private`
- `test_is_effective_client_ip_accepts_cgn`
- `test_is_effective_client_ip_accepts_public`
- `test_is_effective_client_ip_rejects_loopback`
- `test_is_effective_client_ip_rejects_link_local`
- `test_is_effective_client_ip_rejects_ipv6`
- `test_is_effective_client_ip_rejects_special_public_ranges`
- `test_get_local_ips_returns_non_empty`
- `test_get_local_ips_preferred_is_first`
- `test_get_local_ips_no_duplicates`
- `test_get_local_ips_all_entries_are_valid_addresses`

### 单元测试（`crates/bifrost-core/src/access_control.rs`）

- `test_private_network_detection` — RFC 1918 + CGN + link-local。
- `test_cgn_address_allowed_with_allow_lan` — CGN + allow_lan=true 直通。
- `test_cgn_address_prompts_without_allow_lan` — CGN + allow_lan=false 触发 prompt。
- `test_local_subnet_detection_allows_same_subnet` — 同子网公网 IP 视为局域网。
- `test_local_subnet_detection_any_public_ip_in_same_subnet` — CGN 子网精确匹配。
- `test_subnet_hot_update_changes_access_decision` — 运行时更新子网后访问决策实时变化。

### E2E 测试

- `admin_api_proxy_address_with_preferred_ip`（planned，2026-06-16 尚未落地）：验证 API 返回含 `is_preferred` 字段、preferred IP 排在首位。

### 真实场景测试

- 启动服务后 `GET /api/proxy/address`，对比 `ifconfig` / `ip addr` 输出验证虚拟接口被正确过滤。
- 打开 Web UI ProxyTab，观察 Recommended 标签指向的是当前默认路由出口 IP。
- 手动切换 Wi-Fi / 连接断开 VPN / 关闭 Docker Desktop，最多 30 秒内前端 IP 列表自动更新，无需刷新页面。
- CGN 环境（例如 Tailscale）下发起局域网访问，`is_private_network` 命中 CGN，`/api/proxy/address` 展示 `100.x.y.z`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：三处统一入口、三层过滤、preferred、CGN、同子网、热更新。
- 复核 diff：`network.rs` 抽取、`access_control` 双路径、`push.rs` refresh 任务、前端 `is_preferred`。
- 重点 review：
  - `is_effective_client_ip` 是否覆盖了 IANA 特殊段（文档、benchmark、多播、保留）；
  - `detect_preferred_ip` 是否真的走 UDP 无副作用；
  - `RwLock` 热更新与请求判定路径的读锁开销；
  - Windows 分支 netmask 缺失时退化 `/24` 的正确性；
  - push scope 首次订阅补发时的快照 vs 后续增量一致性。
- 复测：`cargo test -p bifrost-admin network`、`cargo test -p bifrost-core access_control`、相关 E2E。

### 第 2 轮

- 基于最新 diff 复查前端展示、handler 响应结构、`start_push_tasks` refresh 循环。
- 重点 review：
  - VPN 上下线时是否有短暂窗口内 push 拿到旧快照；
  - `is_private_network` 降级路径在子网未加载完时是否会误拒同网段设备；
  - Docker 桥接接口名黑名单是否覆盖 Windows 下的 `vEthernet (WSL)` 命名。
- 若发现新增虚拟接口前缀漏掉、CGN 未覆盖某具体 ISP、preferred 检测失败无 fallback，追加第 3 轮。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin network`
- `cargo test -p bifrost-core access_control`
- 相关 E2E（proxy address API）
- `cargo test --workspace --all-features`（时间允许时）
- 本机 no-local-coverage 约定时不跑 `make coverage`。

## 风险与决策点

- **虚拟接口前缀黑名单维护**：新的容器/VPN 工具引入时需要补前缀。回归测试 `test_is_virtual_interface_name_*` 保证不误伤物理接口。
- **公网 IP 展示**：`is_effective_client_ip` 现在保留公网 IPv4，方便真实公网可达代理场景；这也意味着如果用户不希望暴露公网 IP，需要用 `access_control` 或防火墙限制。
- **CGN 判定策略**：CGN 段可能被 ISP 分给非同子网设备，仅靠段判可能误放。同子网精确匹配是主路径，降级段判仅作 fallback。
- **热更新窗口**：30 秒轮询是 idle vs 抖动的折中。若需要更快感知，可引入 `netlink` (Linux) / `SCNetworkReachability` (macOS) 事件订阅，但当前不做。
- **IPv6**：全部排除。理由是当前代理链路、CA URL、二维码消费方（手机 Wi-Fi 配置界面）大多不适合 IPv6 呈现；未来可按 opt-in 支持。
- **`detect_preferred_ip` 失败**：无路由或无网络时 UDP connect 失败；返回值为 `None`，列表中无 `is_preferred=true` 项，前端展示 "No recommended IP"，不 crash。
