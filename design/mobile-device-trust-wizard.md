# Mobile Device Trust Wizard 设计方案

## 背景

Bifrost 桌面端已经把 CA 下载、二维码和证书状态封装在 `Settings > Certificate` 页面，但手机端信任 CA 一直是排障重灾区：

- Android 端 CA 需要用户在系统安全设置里手动信任，普通 ADB 无法读取用户证书库导致管理端只能给出模糊状态。
- iOS 端不仅需要下载 `.mobileconfig`，还必须去 `Settings > General > About > Certificate Trust Settings` 手动开启完全信任，中间步骤最少 4 个界面切换。
- 用户即使装了 CA，也常常不知道自己手机上的浏览器/App 是否真的能被 Bifrost 解密；缺少一个“真的能用吗”的验收入口。
- 局域网多网卡环境下，手机连的往往不是本机默认路由 IP，管理端给出的“建议地址”常常是错的。

Mobile Device Trust Wizard 把这些流程统一为一个向导，同时新增 Bifrost Availability Check（可用性检查）作为端到端排障入口，用真实 HTTPS 握手判断“当前扫码浏览器的 TLS 栈是否已经信任本机 Bifrost CA”。

产品边界：

- 普通个人手机（personal）：Bifrost 可以推送、可以打开安装入口、可以生成 profile 与二维码，但最终信任必须由用户手动确认；管理端不承诺静默安装/信任。
- 受管设备（managed）：Android Device Owner / Profile Owner / 委派证书安装器，或 iOS Apple Configurator / MDM / enrollment profile。本次落地 macOS + Apple Configurator (`cfgutil`) 的自动信任路径。

## 用户目标验证清单

### 必须实现

- Settings > Certificate 页展示统一向导：Availability Check、Local install、iOS devices、Android devices、Certificate downloads 五段左侧固定导航，右侧单列滚动。
- 全局挂载 `MobileDeviceTrustPrompt`，检测到 USB 设备时弹出确认窗口（多设备时列出名称/型号/ID/ECID）。
- Android USB guided install：`adb push` CA + 尝试 `am start` 证书安装入口 + fallback 到安全设置页。
- iOS `.mobileconfig` 下载和二维码（`com.apple.security.root` payload）。
- macOS Apple Configurator 自动安装：`cfgutil -e <ECID> install-profile`，支持多设备定向 ECID。
- 全局 Availability Check：短期 session + 公开 landing HTML + 双协议（HTTP netcheck + HTTPS trust probe）监听器；`.invalid` 域名探测 proxy 是否已配置。
- Push 推送：`trust_probe` 和 `mobile_devices` 两个 settings scope 通过 `/api/push` 实时更新，管理端不用轮询。
- CLI `bifrost ca install --mobile` / `--ios` / `--ios --configurator`。
- 本机 CA install API 也走确认字符串保护，等价于 `bifrost ca install`。

### 必须不破坏

- `bifrost ca install` 原本机 keychain 安装语义不变，只是同时通过 `/api/cert/install` 提供 WebView 入口。
- `/api/cert/info` 兼容旧字段，仅新增 `sha256_fingerprint`。
- Availability Check probe listener 在 60 秒空闲后自愈停止/重建，不与其它监听器冲突。
- 未启用移动设备场景的用户不会看到额外弹窗；`MobileDeviceTrustPrompt` 只在服务端周期性探测发现真实 USB 设备时推送。
- Ant Design token 化，不新增硬编码主题色，light/dark 由现有页面统一验证。

### 必须真实验证

- 局域网 IP 生成的二维码在真实手机（iPhone Safari / Android Chrome / WeChat WebView）上可打开、可推进状态。
- 双协议 probe listener 在 `admin_port + 2` 冲突时自动选空闲端口，session 顶层 `probePort` 与实际监听端口一致。
- Apple Configurator 未安装时，UI 提示禁用原因并提供 Mac App Store 跳转按钮，不静默安装。
- `.invalid` 探测请求经代理进入 Bifrost 后被识别为 `proxy_configured_ok`；HTTP netcheck 经代理进入必须返回失败，避免误判直连。
- 手机端关闭 Wi-Fi 代理后，公开页和管理端在 1-2 轮轮询内回落 `proxyConfigured=false`。

## 产品语义

### 三类身份

Bifrost 明确区分：

1. `personal`：Bifrost 只能推送/打开安装入口，最终信任由用户完成。
2. `managed_auto_trust`：Apple Configurator/MDM 路径，可静默安装并信任。
3. `availability_check`：不做任何安装，仅通过真实 HTTPS 请求验证当前浏览器 TLS 是否信任本机 CA。

### 状态机（Availability Check session）

| 状态 | 含义 |
| ---- | ---- |
| `created` | 管理端生成二维码，等待设备扫码 |
| `page_opened` | 目标设备已打开 HTTP 落地页 |
| `proxy_access_allowed` / `_pending` / `_denied` / `_unavailable` | 代理访问授权检查结果 |
| `proxy_configured_ok` / `proxy_config_failed` | 目标设备浏览器是否把 HTTP 请求发到 Bifrost |
| `network_reachable` | probe 端口 HTTP netcheck 可达 |
| `tls_trusted` | 直连或 CONNECT 后 HTTPS probe 成功 |
| `tls_failed` | netcheck 成功但 HTTPS 握手/请求失败 |
| `network_failed` | landing 已打开但 probe 端口不可达 |
| `expired` | 会话过期 |

`proxy_config_failed` 必须把之前的 `proxy_configured_ok` 回落为 false，保证“手机代理被关掉”和“证书被卸载”这两种回退能被观察到。

### 准确性边界

- 成功只说明“当前扫码设备的当前浏览器 TLS 链路能完成 Bifrost HTTPS probe”。不代表全平台所有 App 都能被解密。
- 失败也不等价于“证书一定没安装”；可能是 firewall 拦截 probe 端口、时间错、扫码设备不在同一 LAN、IP 错、浏览器缓存旧信任判断。
- 实验性 iOS Wi-Fi Proxy Profile 已从系统整体移除；旧 endpoint 应返回 404。理由是 Apple managed Wi-Fi payload 把代理绑到受管 Wi-Fi 配置，稳定性和清理体验不可控。

## 技术细节

### 设备层 crate `crates/bifrost-device`

- `src/model.rs`：`MobilePlatform`、`DeviceTrustCapability`、`DeviceStatus`、`InstallMode`、`MobileDevice`、`InstallSession`。
- `src/adb.rs`：
  - 检测 `BIFROST_ADB_PATH` 或 `PATH` 中的 `adb`。
  - 解析 `adb devices -l`。
  - guided install：`adb push` 到 `/sdcard/Download/bifrost-ca.crt` + `am start` 证书安装入口，失败 fallback 安全设置。
  - CA 状态探针：读取 `/sdcard/Download/bifrost-ca.crt`；root/emulator 尝试 `/data/misc/user/0/cacerts-added` 并按 DER SHA-256 指纹匹配。未 root 设备显示 `pushed_to_device` 或 `unknown`，不误报 `installed`。
- `src/mobileconfig.rs`：PEM/DER 提取 + iOS `.mobileconfig` 生成，PayloadType `com.apple.security.root`。
- `src/ios.rs`：`ioreg` 检测 USB、检测 `cfgutil`、`cfgutil --format JSON list` 读取自定义名/UDID/ECID/型号、`cfgutil -e <ECID> install-profile` 定向安装。

### Admin API 路由

由 `crates/bifrost-admin/src/handlers/mobile_devices.rs` 和 `handlers/trust_probe.rs` 提供：

| 路由 | 用途 | 访问边界 |
| ---- | ---- | ---- |
| `POST /_bifrost/api/cert/install` | 本机 CA 系统信任安装 | loopback/WebView，body 需 `install_local_ca_certificate` |
| `GET /_bifrost/api/mobile-devices` | 列出 USB/ADB/cfgutil 设备 | loopback/WebView |
| `POST /_bifrost/api/mobile-devices/refresh` | 强制刷新 | loopback/WebView |
| `POST /_bifrost/api/mobile-devices/{id}/install-ca` | Android guided / iOS Configurator 安装 | loopback/WebView，body 需 `push_and_open_mobile_certificate_installer` |
| `GET /_bifrost/api/mobile-devices/install-sessions/{session_id}` | 查询安装进度 | loopback/WebView |
| `POST /_bifrost/api/trust-probe/sessions` | 创建 Availability Check | loopback/WebView，`host` 必须是本机发现 IP |
| `GET /_bifrost/api/trust-probe/sessions/{session_id}` | 查询 session（含 `devices[]`） | loopback/WebView |
| `GET /_bifrost/public/trust-probe` | 手机落地 HTML | LAN 公开，不带 token |
| `GET /_bifrost/public/trust-probe/qrcode?host=<ip>` | landing 二维码 SVG | LAN 公开 |
| `GET /_bifrost/public/trust-probe/{session_id}/session?deviceId=<id>` | 手机端轮询 | LAN 公开 |
| `POST /_bifrost/public/trust-probe/{session_id}/report?deviceId=<id>` | 上报 page_opened/network_failed/tls_failed/proxy_config_failed | LAN 公开 |
| `GET /_bifrost/public/trust-probe/{session_id}/proxy-access?deviceId=<id>` | 借访问控制模块检查授权 | LAN 公开，触发 pending |
| `GET /_bifrost/public/mobile/ios-profile.mobileconfig` | iOS profile 下载 | LAN 公开 |
| `GET /_bifrost/public/mobile/ios-profile.mobileconfig/qrcode` | iOS profile 二维码 | LAN 公开 |
| `GET /_bifrost/public/mobile/ios-wifi-proxy.mobileconfig(/qrcode)` | 实验性 Wi-Fi Proxy Profile | 返回 404（已移除） |
| `GET /_bifrost/public/proxy/qrcode` | 已知代理二维码 | LAN 公开 |

`/api/cert/info` 新增 `sha256_fingerprint`（DER 证书 SHA-256），供手机端核对。

### Push 通道

`crates/bifrost-admin/src/push.rs` 支持两个 settings scope：

- `trust_probe`：创建/复用 session、公开 landing/qrcode、report、proxy-access 时广播；全局右上角通知中心订阅。
- `mobile_devices`：仅当存在订阅者时服务端周期性探测 USB/ADB/cfgutil 并推送快照，替代前端定时轮询。

新 WebSocket 连接或动态订阅时立即补发当前快照，避免首次打开必须等下一次事件。

### 双协议 probe server

- 创建/复用 session 前必须先 bind probe listener，`probePort` 写入 session 返回体。
- listener 默认尝试 `admin_port + 2`；端口冲突自动选空闲端口；同 host/admin/CA 下的其它 active session 一起更新为实际端口。
- listener 按 `host/CA key` 管理：`127.0.0.1` 本机预览与 `10.x.x.x` 手机检查可以共存。
- 60 秒 idle 自愈停止，下一次流量到来时重建。
- 同一 TCP 端口通过 `peek` 首字节分流 HTTP vs TLS：HTTP `/_bifrost/trust-probe/netcheck`，HTTPS `/_bifrost/trust-probe/check`。
- HTTPS 证书由当前 Bifrost CA 给所选 IP 动态签发，IP 写入 SAN。
- HTTP netcheck 请求若经 Bifrost proxy 送入必须返回失败（避免误判直连），由 `bifrost_admin::is_active_trust_probe_target` 在 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 中判定。

### 代理配置探测

手机页请求 `http://bifrost-proxy-check.invalid/_bifrost/trust-probe/proxy-configured?sid=<sid>&deviceId=<id>`。`.invalid` 域名只有在浏览器已配置 HTTP proxy 时才会被送到 Bifrost；Bifrost 代理入口截获该请求，记录 `proxy_configured_ok`，广播 `trust_probe` push。

### 手机公开页稳定性

- 手机页每秒重跑 proxy access、HTTPS probe、proxy configured；只在结果真的变化时更新 DOM，否则保持稳定，避免窄屏抖动。
- `Browser HTTPS probe failed`、`Download Bifrost CA`、下一步说明这三块在未信任状态下必须 DOM 稳定。
- localStorage 写入 `bifrostAvailabilityDeviceId`，后续所有请求携带 `deviceId`；不作为权限凭据。
- 多 IP 环境下，手机页顶部推荐使用 `window.location.hostname` 作为代理 host，不用服务端 preferred IP，因为已经被目标设备实际连通。

## CLI

### `bifrost ca install`（保留）

本机 keychain / 系统信任安装，语义不变。

### `bifrost ca install --mobile`

Android USB guided install。

- `--device <serial>` 指定设备。
- `--yes` 非交互；只有一个 ready 设备时自动选择，多个 ready 设备必须显式 `--device`。
- unauthorized/offline 设备直接拒绝。

### `bifrost ca install --ios`

生成 `bifrost-ca.mobileconfig`，输出 iPhone 手动安装 + Certificate Trust Settings 步骤，不控制手机。

### `bifrost ca install --ios --configurator`

macOS + Apple Configurator 路径：检测 `cfgutil` + USB iOS 设备，调用 `cfgutil -e <ECID> install-profile`。多设备需要 `--device <id-or-ecid>`。

Android CLI 在安装前后输出 `Android CA status`：普通设备通常显示 `pushed to device` 或 `unknown`，root/emulator 可通过用户证书库指纹显示 `installed`。

## Web UI

### 全局

- `MobileDeviceTrustPrompt`（`web/src/components/MobileDeviceTrustPrompt/index.tsx`）挂载在 `Layout/index.tsx`，订阅 `mobile_devices` scope。
- 多设备弹窗：自定义名称、型号、ID、ECID，`Install Selected` 直接安装 or `Open Certificate Setup` 跳转（URL 带 `mobile_device` / `mobile_platform`）。
- 右上角 `AvailabilityCheckNotificationCenter` 订阅 `trust_probe` scope。

### Certificate 页

`web/src/pages/Settings/tabs/CertificateTab.tsx` 使用左侧固定导航 + 右侧单列，五段固定顺序：

1. Availability Check
2. Local install
3. iOS devices
4. Android devices
5. Certificate downloads

### Local Install

未安装或未信任时显示 `Install and Trust CA` / `Trust CA`；点击后调用 `/api/cert/install`（等价 `bifrost ca install`）。macOS 状态传播有短暂延迟，前端持续轮询 `/api/cert/info`。

### iOS 区块

- 顶部展示统一流程：送 profile → Settings 安装 → Certificate Trust Settings 打开完全信任。
- 送达方式两选一：Apple Configurator（自动）/ 手动扫码 or LAN profile QR。
- Configurator 每台设备一个按钮，按 ECID 定向；未检测到 `cfgutil` 时禁用按钮 + Mac App Store 跳转。
- `cfgutil` 返回 `ConfigurationUtilityKit.error Code: 625`（需要设备端交互）视为待确认，不判失败。
- 送达方式之后统一步骤图 `ios_1.png` ~ `ios_7.png`，手动扫码步骤展示 `ios_qr_1.jpeg` 和 `ios_qr_2.jpeg`。
- 实验性 iOS Wi-Fi Proxy Profile 已从 UI 完全移除，只留手动 Wi-Fi HTTP Proxy 提示。

### Android 区块

- 展示 ADB/设备授权状态和 CA 状态：`unknown` / `not_installed` / `pushed_to_device` / `installed`。
- `installed` 仅在证书库可读且指纹匹配时显示；否则只显示 `pushed_to_device` 并提示继续在手机上确认。

### Availability Check 卡片

- 展示已连接、需要检查的移动设备（合并 `/api/mobile-devices` USB/ADB/cfgutil 结果）。
- session `devices[]` 每台设备展示：localStorage id、平台 hint、client IP、最近活跃时间、page_opened、probe 端口、HTTPS probe、proxy access、proxy configured。
- 顶层 `status` 保留聚合视图兼容旧 UI/测试。

### 代理交互式授权弹窗

只展示 pending 请求列表和 Allow/Deny/Clear All；不嵌入二维码/链接/Wi-Fi profile 风险说明。可用性检查入口保留在 Certificate 顶部。

## Sync 边界

- 本地 CA 是每台设备的私有信任状态，不参与任何 sync。
- Availability Check session 不 sync，只在本机内存/短期 store。
- `mobile_devices` push 只推给本机 WebSocket 订阅者，远程 Admin 访问 `/api/mobile-devices*` 返回 403，避免远程页面感知本机 USB。

## Phase 拆分

### Phase 1：设备层与 API 骨架

- 新增 `crates/bifrost-device`（adb/ios/mobileconfig/model）。
- `handlers/mobile_devices.rs`、`handlers/cert.rs` `sha256_fingerprint`、confirmation string 保护。
- CLI `--mobile` / `--ios` / `--ios --configurator`。

### Phase 2：Availability Check 服务

- `handlers/trust_probe.rs` + 公开 landing HTML + 双协议 listener。
- `.invalid` proxy 探测入口在 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 判定并回写。
- `crates/bifrost-admin/src/mobile_availability.rs` 聚合 session/devices/推送。

### Phase 3：Web UI + Push

- `pushService.ts` `trust_probe` / `mobile_devices` scope。
- `CertificateTab.tsx` 五段导航 + 卡片。
- `MobileDeviceTrustPrompt` 全局挂载 + 目标设备跳转。
- `AvailabilityCheckPanel` + `AvailabilityCheckNotificationCenter`。

### Phase 4：文档 + 迁移边界

- README、`docs/getting-started.md`、`site/src/content/docs/getting-started/cli-quick-start.md`、`site/src/content/docs/reference/cli.md` 把可用性检查作为高优先级排障入口。
- `human_tests/readme.md` 索引更新。
- 实验性 iOS Wi-Fi Proxy Profile 回归清理。

## 测试方案

### 单元测试

- `bifrost-device`：
  - `parse_adb_devices` connected/unauthorized/offline。
  - `generate_ios_mobileconfig` 包含 `com.apple.security.root` 和手动信任提示。
  - PEM 证书可提取 DER。
  - `cfgutil_install_profile_args` 使用 `install-profile <profile.mobileconfig>`；多设备时 `-e <ECID> install-profile <profile.mobileconfig>`。
  - Android user CA store PEM 指纹匹配/不匹配路径。
- `bifrost-admin`：
  - mobile devices API loopback 限制逻辑。
  - CertInfo SHA-256 指纹由 DER 生成。
  - Availability Check token hash、状态机流转、TLS 成功覆盖历史失败、proxy access 状态记录。
  - probe listener 健康复用、stale 自愈、60 秒 idle 停止。
  - UA 平台推断：Edge、Chrome、iOS Safari、Android WebView/浏览器、WeChat/常见应用容器。
- `bifrost-cli`：
  - `ca install --mobile` 单个 connected Android 自动选择。
  - 多个 connected Android + `--yes` 时必须传 `--device`。
  - 指定 unauthorized/offline 设备时拒绝。
  - `ca install --ios --configurator` 参数解析。

### E2E 测试（`crates/bifrost-e2e/src/tests/admin_api.rs`）

- `admin_api_mobile_devices_lists_android_ios_discovery`
- `admin_public_ios_mobileconfig_uses_current_ca`
- `admin_public_ios_wifi_proxy_mobileconfig_removed`
- `admin_api_mobile_install_requires_explicit_confirmation`
- `admin_api_local_ca_install_requires_explicit_confirmation`
- `admin_trust_probe_verifies_https_trust_with_current_ca`：断言 `probePort` 是实际监听端口；打开 landing/qrcode/proxy-access；配好代理的客户端访问 `.invalid` 探针，断言 `proxyConfigured=true`；HTTP netcheck 经代理仍返回 409；HTTPS check 直连和代理路径均成功；`proxy_config_failed` 上报后 public/管理端都回落为 false。
- `admin_trust_probe_public_landing_stable_failed_state`：iPhone 窄屏在未信任状态多轮轮询，`Browser HTTPS probe failed`、`Download Bifrost CA`、下一步说明 DOM 稳定。
- `proxy_rejects_proxy_routed_active_trust_probe_target`（planned，2026-06-16 尚未落地）：目前由 `admin_trust_probe_verifies_https_trust_with_current_ca` 一并覆盖。
- `cli_ca_install_mobile_single_device_fake_adb`
- `cli_ca_install_mobile_multiple_devices_requires_device_fake_adb`
- `CLI iOS guide`：真实场景验证 `ca install --ios` 输出 profile 路径、手动步骤、Certificate Trust Settings、Configurator 后续命令。

### 真实场景测试 `human_tests/mobile-device-trust.md`

- TC-MDT-01：API 设备发现返回普通手机确认边界。
- TC-MDT-02：iOS mobileconfig 下载包含 root payload 与 Certificate Trust Settings 提示。
- TC-MDT-03：iOS profile QR endpoint 返回 SVG。
- TC-MDT-04：Android install 缺少确认时拒绝。
- TC-MDT-05：Certificate UI 不承诺自动信任，左侧固定导航 + 右侧五段单列顺序。
- TC-MDT-06：light/dark 主题下可读可操作。
- TC-MDT-07：全局 `MobileDeviceTrustPrompt` 多设备弹窗、Install Selected 与 Open Certificate Setup 跳转、目标设备高亮和脉冲。
- TC-MDT-08：CLI `ca install --mobile --yes` fake ADB 单设备自动选择。
- TC-MDT-09：CLI `ca install --mobile --yes` fake ADB 多设备要求 `--device`。
- TC-MDT-10：CLI `ca install --ios` 生成 profile 并输出手动信任步骤。
- TC-MDT-11：Settings iPhone/iPad 统一流程 + Configurator/扫码两种送达；多台 iOS 定向 ECID。
- TC-MDT-12：Apple Configurator 需要手机端交互时页面显示待用户确认而非失败。
- TC-MDT-13：`ios_1` ~ `ios_7` 共享图文步骤。
- TC-MDT-14：Android CA 状态语义（`pushed_to_device` vs `installed`）。
- TC-MDT-15：Availability Check 端到端：扫码、代理授权、page_opened、probe 端口、HTTPS 信任、代理已配置、成功后代理配置 QR、失败下一步指引；管理端和公开页靠 push 自动刷新；多 IP 时手机页推荐 URL 中 IP。
- TC-MDT-16：公开 endpoint 不受交互式访问控制误拦截；未授权设备写入 pending。
- TC-MDT-17：代理授权弹窗简洁；Availability Check 入口在 Certificate 顶部。
- TC-MDT-18：实验性 iOS Wi-Fi Proxy Profile 移除回归（旧下载 404，UI 无 Wi-Fi 输入/风险确认/入口）。
- TC-MDT-19：手机公开页窄屏稳定性（未信任状态 5 秒内 DOM 稳定）。
- 附加：`human_tests/mobile-availability-terminal.md` 与本文档共享 push 通道，回归时一并跑 `e2e-tests/tests/test_mobile_availability_terminal_panel.sh`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标与边界（personal / managed_auto_trust / availability_check）。
- `git status --short` + `git diff`。
- 运行 `cargo test -p bifrost-device`、`cargo test -p bifrost-admin mobile_devices cert trust_probe`、相关 E2E、human_tests。
- 重点 review：本机 CA install / mobile install 两条路径的确认字符串保护；probe listener 端口回填；`.invalid` 分流；push scope 首次快照补发。
- 修复后复跑失败路径。

### 第 2 轮

- 基于最新 diff 复查 `bifrost-device`、Admin handler、Web UI（`CertificateTab.tsx`、`MobileDeviceTrustPrompt`、`AvailabilityCheckPanel`、`AvailabilityCheckNotificationCenter`）、design、human_tests 索引。
- 复跑受影响测试和格式检查。
- 若发现 UI 文案误导（如误称自动信任）、多 IP 提示错误、`proxy_config_failed` 未回落、窄屏抖动，追加第 3 轮。

## 校验要求

- 先跑本次相关 E2E 与 human_tests。
- 最后执行 rust-project-validate：
  - `cargo fmt --all -- --check`
  - `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test -p bifrost-device`
  - `cargo test -p bifrost-cli ca_install_mobile`
  - `cargo test -p bifrost-admin`
  - `cargo test -p bifrost-e2e admin_api`
  - `cargo test --workspace --all-features`
  - 需要时执行 `scripts/ci/local-ci.sh`。

## 风险与决策点

- **多网卡 preferred IP 误判**：服务端只做启发式默认；一旦手机页真的用某 IP 打开，公开页顶部展示的目标代理服务与 proxy QR 必须以 URL hostname 为准。
- **手机 App 信任不等于 TLS 解密全成功**：Android App 可能默认不信任用户 CA、部分 App 有 pinning；Availability Check 只承诺当前浏览器 TLS 链路，不承诺全平台 App。
- **iOS Wi-Fi Proxy Profile 已废弃**：Apple managed Wi-Fi payload 稳定性差，改为始终手动配置 Wi-Fi HTTP Proxy，完成后再手动改回 Off。
- **Apple Configurator `cfgutil` 缺失**：不静默安装 App Store 应用，由 macOS 弹窗让用户确认；安装后如仍无 `cfgutil`，提示在 Configurator 内安装 Automation Tools。
- **push scope 空转开销**：`mobile_devices` 只在存在订阅者时周期性探测 USB/ADB/cfgutil，避免无用户时也占用 ADB/cfgutil 子进程。
- **文档更新范围**：README、`docs/getting-started.md`、`site/src/content/docs/**/cli*.md` 必须一起更新，避免 CLI help、docs 站、README 说法不一致。
