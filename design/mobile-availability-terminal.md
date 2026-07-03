# 启动终端移动可用性面板设计方案（Mobile Availability Terminal）

## 背景

`bifrost start` 前台启动会在终端输出 `SERVER STATUS`：网络、TLS/HTTPS 拦截、CA 证书、系统代理、CLI 代理等静态状态。但移动端调试的用户还需要三件事：

1. 是否能从局域网/公网访问 Bifrost；
2. 手机是否已配置代理、是否已信任 Bifrost CA；
3. 扫码之后到底有没有设备连进来，代理授权是不是通过了。

Web 管理端 `Settings → Certificate → Availability Check` 已经具备 trust probe 能力：生成扫码入口、检查代理授权、探测端口可达、验证 HTTPS/CA 信任、返回代理配置状态、实时列出连入设备。本方案把这套能力搬到前台终端，作为 `SERVER STATUS` 下方新增的 `MOBILE AVAILABILITY CHECK` 板块，默认开启、TTY 下动态刷新、非 TTY 下静默/一次性输出。

## 用户目标验证清单

### 必须实现

- `bifrost start` foreground 默认在 `SERVER STATUS` 下方展示 `MOBILE AVAILABILITY CHECK`，无 flag 开关。
- 只展示对手机真实可达的地址（有效 LAN/公网 IPv4），过滤 loopback / link-local / unspecified / utun / VPN / bridge / Docker / 容器。
- 每个可用 IP 生成 trust probe URL + 短入口 `/_bifrost/tp` 编码为终端 QR；`Open` 行展示完整 URL 便于手动复制。
- 网络变化（IP 出现/消失/preferred 变化）触发面板刷新；同时按 3 秒心跳更新 `last seen`。
- 手机扫码后设备出现在 `Connected devices`；同一手机刷新页面不产生重复行；设备按 IP 稳定排序。
- 设备状态包含：page opened、client IP、last seen、proxy access、proxy config、network reachable、CA trusted、certificate status；每行仅展示该设备自己的状态，不用 session 聚合值兜底。
- Access Control pending 时终端提供 `y` / `n` 审批输入；Web 侧同步审批后终端自动切换为 allowed。
- Active trust-probe 的 HTTPS 目标即使命中 `tlsIntercept://` 规则也强制 CONNECT 直通；HTTP absolute-form `netcheck/check` 不允许经代理成功。
- 非 TTY（daemon / launchd / 重定向 / CI）默认不展示动态面板，避免污染日志与 E2E；可用 `BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1` 强制打印一次。
- 终端 QR 使用短链接 + 关闭 quiet zone 缩小占用；多 IP 场景仅展示 preferred + 折叠其它。

### 必须不破坏

- 现有 Web `Availability Check` API / 响应结构不变。
- 默认访问控制策略不变；未授权设备只显示 pending。
- USB / iOS Configurator / ADB / `bifrost ca install --mobile` 保持既有能力，不合并展示（避免误报）。
- 终端输入不干扰 `Ctrl-C` 停止服务；stdin 采用非阻塞任务。
- 面板刷新不重打 `SERVER STATUS`，避免终端抖动。

### 必须真实验证

- Foreground 启动看到 `MOBILE AVAILABILITY CHECK`。
- 手机扫码后设备行出现且状态推进。
- Access Control `y/n` 审批可用；Web 审批后终端同步 allowed。
- 非 TTY 默认无面板；`BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1` 打印一次快照。
- HTTPS 探测在命中 `tlsIntercept://` 时仍能拿到浏览器直连的 CA 判断，不出现 502/UnknownIssuer。

## 产品语义

### 面板样例

```text
📱 MOBILE AVAILABILITY CHECK
   Status:        Ready
   Preferred IP:  192.168.1.23
   Proxy:         192.168.1.23:8800
   Check URL:     http://192.168.1.23:8800/_bifrost/public/trust-probe/check?...
   QR:            scan to check proxy + CA trust
                  <terminal QR>
                  (compact QR encodes /_bifrost/tp)

   Other addresses:
     - 192.168.8.12:8800  <short check URL or QR collapsed>

   Connected devices:
     - iPhone Safari  192.168.1.45  last seen 12s ago
       Page opened ✓  Network ✓  Proxy access pending  Proxy config missing  CA trust pending
       Action: approve current device? Yes: y | No: n
     - Android Chrome 192.168.1.46  last seen 2s ago
       Page opened ✓  Network ✓  Proxy access allowed  Proxy config detected  CA trust ✓
```

CA 未就绪：

```text
📱 MOBILE AVAILABILITY CHECK
   Status:        CA not ready
   Action:        Bifrost needs a CA certificate before HTTPS trust probe QR can be generated.
   Proxy:         192.168.1.23:8800
```

非 TTY：

```text
📱 MOBILE AVAILABILITY CHECK
   TTY dynamic refresh disabled. A one-shot Availability Check summary is printed.
```

### 状态字段语义

| 字段 | 值 | 含义 |
| --- | --- | --- |
| Page opened | ✓ / — | trust probe 收到 `page_opened` |
| Network | ✓ / — | 设备可从局域网连到 admin 端口 |
| Proxy access | allowed / pending / denied / unavailable | access control 判定 |
| Proxy config | detected / missing / pending | 是否经代理访问到 config check host |
| CA trust | trusted / failed / pending | HTTPS probe 成功、失败原因或未完成 |
| Certificate | trusted / not trusted / checking | HTTPS 结果推导 |

## 技术细节

### 主要文件

- `crates/bifrost-admin/src/mobile_availability.rs`
  - `MobileAvailabilitySnapshot`、`MobileAvailabilityEntry`、`MobileAvailabilityDevice`、`MobileAvailabilityPendingAuth`（`mobile_availability.rs:44+`）。
  - `render_mobile_availability_panel(snapshot)` / `render_mobile_availability_panel_for_terminal(snapshot, is_terminal)`（`mobile_availability.rs:452-540`）。
  - TTY 判定：`stdout_is_terminal || BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE`（`mobile_availability.rs:116`）。
  - `pending_authorization_signature()` 用于抑制 pending 输入期间的定时重绘。
- `crates/bifrost-admin/src/handlers/trust_probe.rs`
  - `get_or_create_terminal_probe_session(state, ip)`（`trust_probe.rs:131`）
  - `is_active_trust_probe_target(host, port)`（`trust_probe.rs:51`）
  - `list_active_sessions()`
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs:702`
  - `let force_trust_probe_passthrough = bifrost_admin::is_active_trust_probe_target(&host, port);`
  - CONNECT 命中 active trust-probe 时强制直通，绕过 TLS 拦截。
- `crates/bifrost-admin/src/handlers/cert.rs`
  - 已有 `/_bifrost/public/cert/qrcode`、`/_bifrost/public/proxy/qrcode`。
- `crates/bifrost-admin/src/network.rs`
  - `get_local_ips()` / `get_local_subnets()` + `is_effective_client_ip()`。
- `crates/bifrost-cli/src/commands/start.rs`
  - `run_foreground` 中启动 mobile availability 后台 task 与刷新循环。
- `web/src/pages/Settings/Certificate/AvailabilityCheck*.tsx`
  - Web 侧 Availability Check（复用同一 trust probe manager）。

### 面板刷新模型

- TTY：后台 task 周期生成 snapshot；触发条件：`get_local_ips()` 变化、trust probe session 变化、pending authorization 变化、3s 心跳。使用 ANSI 控制序列局部重绘，不重打 `SERVER STATUS`。pending 输入未变化时抑制自动重绘，避免擦掉用户输入。
- 非 TTY：不启动后台 task；打印一次面板或完全静默；`BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1` 用于 E2E 强制输出。
- Ctrl-C：面板 ready 前必须已安装 shutdown 信号监听（human_tests TC 已断言）。

### Trust probe listener 生命周期

- 复用 `TrustProbeManager`：终端 panel 通过 `get_or_create_terminal_probe_session()` 拿或建 session（TTL 30 分钟，活跃自动续期）。
- listener 在 60s 没有新的公开页 / session 轮询 / report / proxy-access / netcheck / HTTPS check 流量后自动停止，避免多设备并发反复启停。
- CONNECT/HTTP 入口通过 `is_active_trust_probe_target` 保护 active trust-probe 目标；HTTP proxy 拒绝经代理送入的 active trust-probe absolute-form `netcheck/check`。

### Access Control 终端入口

- 监听 pending authorization 列表。
- `y` / `yes`：允许最早 pending 设备；`n` / `no`：拒绝最早 pending 设备；隐藏兼容 `allow <ip>` / `deny <ip>`。
- 非 TTY 不监听 stdin。
- Web 审批同步 pending 列表变化 → 下一次面板自动刷新为 allowed。

## CLI / Web / Admin API

### CLI

- `bifrost start`（foreground）：默认展示 `MOBILE AVAILABILITY CHECK`；TTY 支持 `y` / `n` 审批。
- `BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1 bifrost start ...`：非 TTY 场景强制打印一次面板（供 E2E）。
- `bifrost ca install --mobile`：既有 USB/Configurator/ADB 能力保持不变。

### Web

- `Settings → Certificate → Availability Check`：既有 Web 面板保留；与终端面板共享同一 trust probe manager，Web 审批同步到终端 pending 列表。

### Admin API

- `/_bifrost/public/trust-probe/*`：既有 landing / check / report / netcheck / QR。
- `/_bifrost/tp`：短入口，重定向到 fixed landing。security public path allowlist 需放行。
- `/_bifrost/public/cert/qrcode`、`/_bifrost/public/proxy/qrcode`：既有 QR 端点。

无新增用户可调 CLI 子命令。

## Sync 边界

- Trust probe session / mobile availability snapshot 均属本机运行时数据，不跨设备 sync。
- Access control pending 列表本机可见；Web 与终端共享 in-process 状态，不通过 sync 传播。
- Trust probe 短入口路径 `/_bifrost/tp` 是本机 admin 路由，非 sync 内容。

## Phase 1-4

### Phase 1：Admin snapshot + trust probe 内部复用

1. 抽取 `MobileAvailabilitySnapshot` 与相关结构（`mobile_availability.rs`）。
2. `TrustProbeManager` 提供 `get_or_create_terminal_probe_session`、`list_active_sessions`、`is_active_trust_probe_target`。
3. 网络地址过滤 `is_effective_client_ip()`。

### Phase 2：终端渲染 + QR

1. `render_mobile_availability_panel_for_terminal` 支持 TTY / 非 TTY 分支。
2. 终端 QR 用 `qrcode` crate + block 字符；关闭 quiet zone；编码 `/_bifrost/tp`。
3. 状态颜色仅在 TTY 输出 ANSI；重定向输出保持纯文本。

### Phase 3：Foreground 启动集成

1. `run_foreground` 启动后台 task 生成 snapshot + 刷新面板。
2. TTY 下开 stdin 非阻塞 task，识别 `y` / `n` / `allow` / `deny`。
3. shutdown 信号在 ready 之前注册。
4. `BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE` 用于 E2E。

### Phase 4：CONNECT/HTTP 保护 + 稳定性

1. HTTP tunnel 命中 active trust-probe 强制直通（`tunnel/mod.rs:702`）。
2. HTTP proxy 拒绝经代理的 active trust-probe `netcheck/check`。
3. listener 60s 自愈重建 + 空闲自停。
4. 设备去重按 `source_host + client_ip`；同一手机刷新页不追加。

## 测试方案

### 单元测试（`mobile_availability.rs`）

| 测试 | 断言 |
| --- | --- |
| `render_mobile_availability_panel` 空快照 | 输出包含 `MOBILE AVAILABILITY CHECK` 与 empty state（`mobile_availability.rs:840-846`） |
| `render_mobile_availability_panel` 常规 | 展示 IP / QR / 设备行（`mobile_availability.rs:853-863`） |
| `render_mobile_availability_panel_for_terminal(true)` vs `false` | TTY 输出带 ANSI 颜色；非 TTY 保持纯文本（`mobile_availability.rs:872-893`） |
| `pending_authorization_signature` | pending 未变化时返回相同签名，抑制自动重绘 |
| `BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE` guard | 环境变量强制启用面板（`mobile_availability.rs:950-954`） |
| `network::is_effective_client_ip` | 过滤 loopback / link-local / IPv6 doc / benchmark / multicast / 保留 CGNAT / 公网 |

### E2E 测试

- 断言 foreground 启动输出 `MOBILE AVAILABILITY CHECK` 与 `y` / `n` 审批入口。
- 断言 target 不含 `127.0.0.1`。
- 断言启动输出无 Demo / process check / ChatGPT Web startup auth 噪声。
- 断言 HTTPS 探测在命中 `tlsIntercept://` 时 200，不出现 502/UnknownIssuer。

### human_tests（`human_tests/mobile-availability-terminal.md`）

| 用例 | 断言 |
| --- | --- |
| TC-MAT-01 | 前台启动 `SERVER STATUS` 下方出现 `MOBILE AVAILABILITY CHECK` |
| TC-MAT-02 | 单 IP 展示完整 URL + 短 QR |
| TC-MAT-03 | 网络变化后 target / URL / QR 自动刷新 |
| TC-MAT-04 | 手机扫码后设备出现并状态推进；刷新不产生重复；ANSI 颜色只在 TTY；重定向输出纯文本 |
| TC-MAT-05 | 手机页面关闭或停报后仅保留最近活跃设备，超时移除 |
| TC-MAT-06 | Access Control pending 用 `y` / `n` 审批；输入保护抑制自动重绘 |
| TC-MAT-07 | Web UI 审批同步为 allowed |
| TC-MAT-08 | 启动控制台无 Demo / 进程检查噪声 |
| TC-MAT-09 | HTTPS Trust Check 经代理命中 `tlsIntercept://` 时 200 |

## Review / Fix / Test 闭环

- **第 1 轮**：核对 `mobile_availability.rs`、`trust_probe.rs`、`tunnel/mod.rs:702` 与文档一致；跑 `cargo test -p bifrost-admin mobile_availability::`、`trust_probe::`；跑 `cargo test -p bifrost-proxy tunnel::`。
- **第 2 轮**：基于最新 diff 复查 `human_tests/mobile-availability-terminal.md` 与索引；跑 E2E 前台启动脚本；确认非 TTY 环境静默。
- **第 3 轮（按需）**：如出现 TTY 抖动、pending 输入被擦、Web / 终端审批不同步等回归，追加轮次。

## 校验要求

- 优先执行：
  - `cargo test -p bifrost-admin mobile_availability:: trust_probe::`
  - `cargo test -p bifrost-proxy tunnel::`
- 再执行 `rust-project-validate`：fmt / clippy / `cargo test --workspace --all-features`。
- `scripts/ci/local-ci.sh` 仅在最终范围需要完整本地 CI 时执行。

## 风险与决策

| 风险 | 决策 |
| --- | --- |
| 动态终端刷新与常规日志互相干扰 | 依赖"默认日志写文件"改造；非 TTY / CI 重定向静默 |
| TUI 组件依赖膨胀（ratatui/crossterm） | 不引入；仅 Yes/No 审批 + pending 输入保护 |
| Demo / process check 子进程污染 panel | 改为捕获输出或写日志文件 |
| 多网卡展示导致高度失控 | 短链 QR + 关闭 quiet zone；只完整展示 preferred |
| USB 设备与浏览器设备无法一一对应 | 初版不合并 USB discovery，避免误报 |
| 网络变化 polling 延迟 | 使用 3s polling；后续可接入平台事件 |
| Trust probe 需要 CA key，CA 未就绪 | 面板 fallback 展示 "CA not ready" 与修复引导 |
| Active trust-probe 被 TLS 拦截污染 | CONNECT 强制直通；HTTP proxy 拒绝 absolute-form probe |

## 已确认决策

- 活跃设备窗口 75 秒，暂不配置化。
- 公网 IPv4 与 LAN IPv4 均默认展示；虚拟 / 隧道 / 特殊地址不展示。
- 终端审批命令直接执行；`allow <ip>` 可审批 pending，也可加入临时允许列表。

## 文档更新要求

- 更新 `human_tests/mobile-availability-terminal.md`（9 个 TC）。
- 更新 `human_tests/readme.md` 索引。
- README / 协议 / Hook 文档：本次不新增外部 CLI 或 API 字段，不需要额外更新。
