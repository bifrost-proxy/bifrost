# 桌面端代理监听所有网络接口

## 背景

Bifrost 的核心价值之一是把开发者的电脑变成整个局域网设备的代理入口——手机、模拟器、平板、另一台电脑都可以指向这台机器的 IP，让流量落到 Bifrost 拦截、录制、修改、重放。

CLI 模式（`bifrost start` / `bifrost daemon`）默认绑定 `0.0.0.0`，任何局域网内可达的地址都能连上代理端口。桌面端历史实现却硬编码了 `--host 127.0.0.1`：

- Tauri 侧调用 `Command::new(binary_path).args(["start", "--host", "127.0.0.1", ...])`，导致内嵌 core 只监听回环地址；
- 结果桌面用户看到的“代理端口 9900” 只在本机可用，其他设备根本连不上；
- 这与 Bifrost 的核心用例（Mobile / IoT 抓包）完全冲突，也让 “Network Address” 面板显示的 LAN IP 变成误导——那些 IP 从外部访问会连接被拒。

同时又不能简单把桌面端也 blanket 绑定 `0.0.0.0`：桌面壳层还要保留 `127.0.0.1` 作为 admin API 的调用地址与日志显示地址，否则安全语义会退化。

本文覆盖桌面端如何拆分“绑定地址”与“admin/日志地址”常量、如何保证 admin API 不因绑定 `0.0.0.0` 而暴露、以及 LAN IP 展示与端口探测口径。相关：`desktop-launcher-startup.md`、`desktop-runtime-port-switch.md`（rebind 后仍走 `0.0.0.0`）、`crates/bifrost-admin/src/security.rs`（loopback-only 校验）。

## 用户目标验证清单

### 必须实现

- 桌面端启动 sidecar 时代理端口绑定 `0.0.0.0`，与 CLI 默认行为对齐。
- 手机 / 其他电脑通过桌面端所在机器的 LAN IP + 端口能连上代理。
- 桌面壳层调用 admin API 时仍走 `127.0.0.1:{port}`，不受绑定地址影响。
- 端口可用性检测（`is_port_available`）与实际绑定使用同一 host（`0.0.0.0`），避免出现“探测通过但真实绑定失败”或“探测失败但其实可绑”。
- 日志中显示 `http://127.0.0.1:{port}` 作为面向用户的 backend URL；LAN IP 通过 `bifrost_admin::network::get_local_ips()` 单独列出。
- Admin API 依然仅接受 loopback peer，即使代理监听 `0.0.0.0`，外部请求也会在 admin 侧被 401/403。

### 必须不破坏

- Handoff / launcher / cert bootstrap / watchdog / port switch 语义。
- Admin API 安全模型（`is_valid_admin_request` + `AdminSecurityConfig::allowed_hosts` = loopback）。
- `desktop-config.json` 的 `proxy_port` 字段结构。
- CLI 默认行为（`--host 0.0.0.0` 默认值）。
- 现有 `bifrost-admin::network` 单元测试与调用者。

### 必须真实验证

- 桌面端启动后 `lsof -i :9900` 展示 `*:9900`（即绑定所有接口）而非 `127.0.0.1:9900`。
- 手机 / 另一台电脑设置代理为 `<Mac LAN IP>:9900` 能命中 Bifrost 并抓到流量。
- 外部尝试访问 `http://<Mac LAN IP>:9900/_bifrost/api/proxy/system/support` 得到 admin 401/403，而不是被响应。
- 桌面壳层依旧能通过 `http://127.0.0.1:9900/_bifrost/api/config/server` 走通 rebind。

## 产品语义

### 两个语义完全不同的“host” 常量

原代码只有一个 `BACKEND_HOST = "127.0.0.1"`，混用在四个语义上：

| 用途 | 需要值 | 说明 |
| --- | --- | --- |
| 代理服务绑定地址 | `0.0.0.0` | 让 LAN 上其他设备可访问 |
| Admin API 调用地址 | `127.0.0.1` | 桌面壳层走本机 admin |
| 端口可用性检测 | `0.0.0.0` | 与实际绑定一致，避免误判 |
| 面向用户的 URL 显示 | `127.0.0.1` | 用户看到 loopback，代表 admin 入口 |

拆成两个常量：

```rust
const BACKEND_BIND_HOST: &str = "0.0.0.0";
const BACKEND_ADMIN_HOST: &str = "127.0.0.1";
```

- `BACKEND_BIND_HOST`：`start_backend` 传给 sidecar 的 `--host`；`is_port_available` 用它调 `TcpListener::bind`。
- `BACKEND_ADMIN_HOST`：`probe_backend_health` 组装 URL；`wait_for_backend` 与 `request_backend_port_transition` 都用它。
- `desktop-bootstrap.log` 中的 backend URL 使用 `http://{BACKEND_ADMIN_HOST}:{port}` 输出。

### Admin API 依然仅接受 loopback

即使代理 socket 绑定 `0.0.0.0`，admin API 的安全模型仍然要求 peer 是 loopback：

- `crates/bifrost-admin/src/security.rs`：`is_valid_admin_request()` 校验 `peer_addr.ip().is_loopback()`。
- `AdminSecurityConfig::allowed_hosts` 默认只包含 `127.0.0.1` / `localhost`（含带端口形式）。
- 因此外部设备可以走代理端口，但无法调 `/api/*` 后台 API；桌面壳层与外部设备权限层次清晰。

代理端口本身（HTTP CONNECT / TLS MITM / SOCKS 等）是允许 LAN 访问的，这是 Bifrost 的产品能力。

### LAN IP 展示与推荐

桌面端“Network Address”面板 / “Mobile device trust wizard”依赖 `crates/bifrost-admin/src/network.rs::get_local_ips()` 列举本机 IP。本次改造复用既有实现，不新增字段：

- `get_local_ips() -> Vec<LocalIpInfo>`：使用 `local_ip_address::list_afinet_netifas` 枚举所有 IPv4/IPv6 接口，过滤 loopback、link-local、ipv6 等，按 preferred 排序。
- 单元测试 (`test_get_local_ips_*`) 保证结果非空、preferred 排第一、无重复、每条都是有效地址。

## 关键代码入口

- `desktop/src-tauri/src/main.rs`
  - `const BACKEND_BIND_HOST: &str = "0.0.0.0";`
  - `const BACKEND_ADMIN_HOST: &str = "127.0.0.1";`
  - `start_backend()` 使用 `BACKEND_BIND_HOST` 作为 `--host` 参数。
  - `is_port_available(port)` 使用 `TcpListener::bind((BACKEND_BIND_HOST, port))`。
  - `probe_backend_health(port)` / `wait_for_backend(port, timeout)` / `wait_for_backend_shutdown` / `request_backend_port_transition` 使用 `BACKEND_ADMIN_HOST`。
  - `append_desktop_bootstrap_log` 在 sidecar ready / rebind 日志中显示 `http://{BACKEND_ADMIN_HOST}:{port}` 作为规范 URL。
- `crates/bifrost-admin/src/network.rs`
  - `get_local_ips() -> Vec<LocalIpInfo>`：既有实现。
  - `get_effective_local_ips()`：内部枚举 + 排序。
  - 单元测试 `test_get_local_ips_returns_non_empty` / `_preferred_is_first` / `_no_duplicates` / `_all_entries_are_valid_addresses`。
- `crates/bifrost-admin/src/security.rs`
  - `is_valid_admin_request()`：loopback + host allowlist。
  - `AdminSecurityConfig::allowed_hosts`：默认 loopback 域名 / IP。
- `crates/bifrost-cli/src/commands/start.rs` 保持 `--host 0.0.0.0` 默认，桌面端与 CLI 语义对齐。

## 启动命令片段

```rust
Command::new(binary_path)
    .args([
        "start",
        "--host",
        BACKEND_BIND_HOST,            // "0.0.0.0"
        "--port",
        &port,
        "--skip-cert-check",
    ])
    .env("BIFROST_DATA_DIR", data_dir)
    .stdout(Stdio::from(stdout_log))
    .stderr(Stdio::from(stderr_log))
    .spawn()
```

- `--skip-cert-check` 是桌面端启动 sidecar 时的偏好（cert bootstrap 由桌面壳层独立完成，见 `desktop-core-cert-bootstrap.md`）。
- `BIFROST_DATA_DIR` 与桌面壳层保持一致（`bifrost_storage::data_dir()`）。

## 依赖项

- 桌面壳层：`desktop/src-tauri/src/main.rs`
- Admin 侧：`crates/bifrost-admin/src/network.rs`、`crates/bifrost-admin/src/security.rs`
- 代理侧：`crates/bifrost-proxy/src/server.rs`（真正 listen 逻辑保持不变；本次只影响它拿到的 host 参数）
- CLI 侧：`crates/bifrost-cli/src/commands/start.rs`（作为语义参考，无需修改）

## CLI / 环境变量表面

无新 CLI，无新环境变量。相关的旧变量：

- `BIFROST_DATA_DIR`：影响 sidecar 数据目录与端口探测。
- `BIFROST_ADMIN_ALLOWED_HOSTS`（若存在）：不做改动，继续控制 admin allow list。

## Web / Admin API 表面

无新 API。相关已有：

- `GET /_bifrost/api/proxy/network/local-ips`（或对应路径）返回 `get_local_ips()` 结果，用于 UI 展示 LAN IP。前端 Mobile Trust Wizard、Network Address 面板复用。
- `GET /_bifrost/api/proxy/system/support`：桌面壳层的 backend 就绪探针（`probe_backend_health` 使用）。
- `PUT /_bifrost/api/config/server`：端口 rebind 入口（详见 `desktop-runtime-port-switch.md`）。

## Sync 边界

- 绑定地址是本机运行时属性，不通过 sync。
- LAN IP 列表按机器变化，也不同步。

## 实现切分

### Phase 1：常量拆分（已完成）

- 引入 `BACKEND_BIND_HOST` / `BACKEND_ADMIN_HOST`。
- `start_backend`、`is_port_available` 切到 `BACKEND_BIND_HOST`；`probe_backend_health`、`wait_for_backend`、rebind API 切到 `BACKEND_ADMIN_HOST`。
- 日志字符串统一使用 `BACKEND_ADMIN_HOST` 显示 URL。

### Phase 2：Admin 安全语义确认（已完成）

- `security.rs` 保持 loopback-only；不做任何放宽。
- 复核所有对 `BACKEND_ADMIN_HOST` 的引用是否只在桌面壳层内部使用，未泄漏到对外文档 / UI。

### Phase 3：Network Address 展示复用（已完成）

- 复用 `network.rs::get_local_ips()`；不引入新 endpoint。
- UI（Mobile Trust Wizard / Network Address 面板）不变。

### Phase 4：文档与人工测试

- 本文 + `desktop-runtime-port-switch.md` + `network-address-detection.md` 边界清晰。
- Human_tests 覆盖 LAN 访问 & admin 拒外。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/network.rs::tests`
  - `test_get_local_ips_returns_non_empty`
  - `test_get_local_ips_preferred_is_first`
  - `test_get_local_ips_no_duplicates`
  - `test_get_local_ips_all_entries_are_valid_addresses`
- `desktop/src-tauri/src/main.rs::tests`
  - 现有 close / port / recovery 单元测试隐式保证 `BACKEND_BIND_HOST` / `BACKEND_ADMIN_HOST` 常量被正确注入端口探测与 rebind 路径（`parses_snake_case_port_update_response` 等）。
  - 可选新增：`bind_host_is_wildcard` 断言 `BACKEND_BIND_HOST == "0.0.0.0"`；`admin_host_is_loopback` 断言 `BACKEND_ADMIN_HOST == "127.0.0.1"`——常量化后作为回归防护。

### E2E / 真实场景（`human_tests/desktop-proxy-bind-all-interfaces.md`）

- TC-DPB-01：桌面端启动后 `lsof -nP -iTCP:9900` 展示 `*:9900 (LISTEN)`。
- TC-DPB-02：手机 Wi-Fi 与 Mac 同网段，设置 HTTP 代理为 `<Mac LAN IP>:9900`，浏览网页能命中 Bifrost 流量列表。
- TC-DPB-03：外部电脑 `curl -x http://<Mac LAN IP>:9900 https://example.com` 成功；但 `curl http://<Mac LAN IP>:9900/_bifrost/api/proxy/system/support` 返回 401/403 或空响应。
- TC-DPB-04：桌面端本机 `curl http://127.0.0.1:9900/_bifrost/api/proxy/system/support` 返回 200。
- TC-DPB-05：Rebind 到新端口（Settings → change port）后仍是 `*:{new_port}` 绑定，重复 TC-DPB-01 / TC-DPB-02 用新端口。
- TC-DPB-06：非 macOS（Linux/Windows）桌面端启动后同样绑定 `0.0.0.0`。

启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin network`
- `cargo test -p bifrost-desktop --tests`
- `rust-project-validate`
- 本机 no-local-coverage。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标：绑定 `0.0.0.0`、admin 仍 loopback、探测/绑定同 host、日志显示 `127.0.0.1`。
- 复核 diff：所有 `BACKEND_HOST` 引用点是否已改到正确常量；`start_backend` `--host` 参数是否指向 `BACKEND_BIND_HOST`；rebind API URL 是否用 `BACKEND_ADMIN_HOST`。
- 重点 review：`is_port_available` 与 `start_backend` 使用同一 host，避免探测通过但绑定失败；`probe_backend_health` 使用 loopback 保证受 admin 安全模型保护。
- 复测：cargo tests；手机 + 另一台电脑真实抓包。

### 第 2 轮

- 复核第 1 轮发现的问题修复。
- 检查 `git diff`。
- 重点 review：Rebind 后 sidecar 是否仍以 `--host 0.0.0.0` 启动（`launch_backend_on_available_port` → `start_backend`）。
- 复测：外部 admin 请求真的被拒。

## 风险与决策点

- 绑定 `0.0.0.0` 会让代理端口在 LAN 完全可见，弱网络环境下可能被非预期设备连上。缓解：
  - Admin API 依旧 loopback-only；
  - 用户在陌生 Wi-Fi 环境需要自行判断风险；
  - 未来可加 “LAN allow list” 配置或 “仅回环” 一键开关，但第一版不做。
- 不做 IPv6-only 绑定：`0.0.0.0` 只是 IPv4 通配符；IPv6 场景若要覆盖，可在后续 `--host ::` 支持里做，目前 CLI 也未做双栈。
- 端口探测 host 与绑定 host 一致：避免 macOS 上 `127.0.0.1:P` 未被占用但 `0.0.0.0:P` 已被占用的假阳性。
- `BACKEND_ADMIN_HOST` 硬编码 `127.0.0.1`：若用户将系统 loopback alias 改动（罕见），无兼容；这是可接受权衡。
- Mobile Trust Wizard 与 Network Address 面板依赖 `get_local_ips()`，其推荐结果（preferred）仍由 admin 侧决定，桌面壳层不额外覆盖。
