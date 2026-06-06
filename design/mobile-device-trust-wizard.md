# Mobile Device Trust Wizard

## 功能模块详细描述

Mobile Device Trust Wizard 把现有 CA 下载、二维码和证书状态能力扩展为手机证书安装向导。

产品边界分两类：

- 普通个人手机：Bifrost 可以检测 Android USB 设备、推送 CA、打开系统安装入口，或为 iOS 提供 `.mobileconfig` 下载/二维码；手机端仍必须由用户确认安装和信任。
- 企业/测试受管设备：只有 Android Device Owner/Profile Owner/委派证书安装器，或 iOS Apple Configurator/MDM/MDM enrollment profile 这类受管路径，才允许真正自动安装并启用信任。本次支持 macOS + Apple Configurator/cfgutil 的高级路径；普通网页/二维码下载仍必须手动启用完全信任。

新增 Bifrost Availability Check / 可用性检查，用于验证“目标设备是否能使用 Bifrost”。它不读取 iOS/Android 私有证书库，也不依赖 USB、ADB、Apple Configurator 或 MDM，而是让目标设备扫码打开 HTTP 落地页，自动检查代理访问授权、浏览器是否真的配置了 Bifrost 代理、探针端口可达性，再发起一次由当前 Bifrost CA 签发证书的真实 HTTPS 请求。HTTPS 请求成功表示当前设备浏览器 TLS 栈已经信任 Bifrost CA；失败则说明 CA 未安装、未启用完全信任、证书不匹配、设备时间异常或探针端口被拦截。

## 实现逻辑

### 1. 设备层 crate

新增 `crates/bifrost-device`：

- `model.rs` 定义 `MobilePlatform`、`DeviceTrustCapability`、`DeviceStatus`、`InstallMode`、`MobileDevice`、`InstallSession`。
- `adb.rs` 负责：
  - 检测 `BIFROST_ADB_PATH` 或 `PATH` 中的 `adb`。
  - 解析 `adb devices -l` 输出。
  - 普通 Android guided install：`adb push` CA 到 `/sdcard/Download/bifrost-ca.crt`，再尝试 `am start` 打开证书安装入口，失败时 fallback 到安全设置页。
  - Android CA 状态探针：对 connected 设备检查 `/sdcard/Download/bifrost-ca.crt` 是否与当前 CA 文件一致；对 root/emulator/test 设备尝试读取 `/data/misc/user/0/cacerts-added`，按当前 CA DER 的 SHA-256 指纹比对是否已经进入 Android 用户证书库。普通未 root 设备通常无法读取该私有证书库，因此状态会明确显示为“已推送但无法由普通 ADB 确认”或“未知”，不会误称已信任。
- `mobileconfig.rs` 负责：
  - 从 PEM/DER 证书文件提取 DER。
  - 生成 iOS `.mobileconfig`，PayloadType 为 `com.apple.security.root`，并明确写入手动启用完全信任的说明。
  - 生成 iOS Wi-Fi Proxy `.mobileconfig`，在同一个 profile 中包含 `com.apple.security.root` 和 `com.apple.wifi.managed`。Wi-Fi payload 写入服务端检测到或用户提供的当前 Wi-Fi `SSID_STR`、`ProxyType=Manual`、`ProxyServer` 和 `ProxyServerPort`，用于 POC 验证普通 iPhone 能否通过 managed Wi-Fi profile 修改同名 Wi-Fi 的手动代理。profile 不写入 `Password`、`Passphrase` 或其他 Wi-Fi 入网凭据，避免要求用户再次输入 Wi-Fi 密码；但 iOS 会把该 payload 作为 managed Wi-Fi 配置，卸载 profile 时可能移除该 Wi-Fi 网络条目，因此 UI 必须把手动 Wi-Fi 代理设置作为安全推荐，把 profile 标记为实验性并要求用户确认风险。
- `ios.rs` 负责：
  - macOS 上通过 `ioreg` 检测 USB iPhone/iPad。
  - 检测 Apple Configurator 的 `cfgutil` 是否可用。
  - 高级路径调用 `cfgutil -e <ECID> install-profile <profile.mobileconfig>`。Bifrost 通过 `cfgutil --format JSON list` 读取每台设备的自定义名称、UDID、ECID 和型号；多台 iPhone/iPad 同时连接时，页面和 CLI 均让用户选择目标设备，并按所选设备 ECID 定向安装，避免误装。

### 2. Admin API

新增路由：

- `POST /_bifrost/api/cert/install`
- `GET /_bifrost/api/mobile-devices`
- `POST /_bifrost/api/mobile-devices/refresh`
- `POST /_bifrost/api/mobile-devices/{id}/install-ca`
- `GET /_bifrost/api/mobile-devices/install-sessions/{session_id}`
- `GET /_bifrost/public/mobile/ios-profile.mobileconfig`
- `GET /_bifrost/public/mobile/ios-profile.mobileconfig/qrcode`
- `GET /_bifrost/public/mobile/ios-wifi-proxy.mobileconfig?ssid=<ssid>&ip=<local_ip>&port=<proxy_port>`
- `GET /_bifrost/public/mobile/ios-wifi-proxy.mobileconfig/qrcode?ssid=<ssid>&ip=<local_ip>&port=<proxy_port>`
- `POST /_bifrost/api/mobile-devices/{id}/install-ios-proxy-profile`

安全边界：

- `/api/cert/install` 等价于 `bifrost ca install` 的本机系统信任安装流程，仅允许 loopback / 桌面 WebView 访问；请求体必须携带确认字符串 `install_local_ca_certificate`，避免远程 Admin 或裸 POST 误触发本机 keychain/sudo 安装提示。
- `/api/mobile-devices*` 仅允许 loopback / 桌面 WebView 访问，避免远程 Admin 触发本机 USB 操作。
- `/_bifrost/public/cert*` 与 `/_bifrost/public/mobile*` 是公开下载路径，必须允许任意 LAN 客户端访问，不要求交互式授权或白名单；否则手机扫码会拿到非 profile 响应并提示描述文件无效。
- `/_bifrost/public/proxy*` 同样是公开二维码路径，必须允许任意 LAN 客户端访问；否则可用性检查成功后的 `Open proxy QR code` 会被访问控制拦截成 403。
- `install-ca` 支持 Android `normal_guide` 和 iOS `managed_auto_trust`。请求体必须携带确认字符串 `push_and_open_mobile_certificate_installer`，并且只有本地 Admin/WebView 能触发。
- `install-ios-proxy-profile` 只允许本地 Admin/WebView 触发，必须携带确认字符串 `install_ios_wifi_proxy_profile`。该接口复用 Apple Configurator/cfgutil 定向安装路径，把 `CA + Wi-Fi Proxy` profile 发送到所选 iPhone；普通未监督 iPhone 仍可能要求手机端确认。
- iOS Wi-Fi Proxy public profile endpoint 免授权开放给 LAN 手机扫码下载，但 `ip` 必须是 Bifrost 检测到的本机局域网 IP 或 loopback，不能生成指向任意第三方代理的 profile。
- 安装操作记录 tracing audit log。
- `/api/cert/info` 新增 `sha256_fingerprint`，供用户在手机上核对 CA 指纹。

### 3. Web UI

在 Settings -> Certificate 的 Mobile Installation 卡片中新增：

- 普通设备提示：Bifrost 只能推送/打开安装流程，手机端仍需确认。
- Local Certificate Install 区块在本机 CA 未安装或已安装但未信任时展示 `Install and Trust CA` / `Trust CA` 按钮；点击后调用 `/api/cert/install`，行为等价于 `bifrost ca install`，成功后立即刷新本机证书状态。由于 macOS 安装证书和设为信任可能存在短暂状态传播延迟，前端会继续轮询 `/api/cert/info`，直到状态变为 `Installed and trusted` 或超时。
- Android USB 区块：检测设备、展示 ADB/设备授权状态和当前 CA 状态；对 connected 设备执行 guided install。CA 状态分为 `unknown`、`not_installed`、`pushed_to_device`、`installed`。其中 `installed` 仅在 Bifrost 能读取 Android 用户证书库并匹配当前 CA 指纹时显示；普通个人手机若只完成了 push/open installer，则 UI 只显示 `pushed_to_device` 并提示继续在手机上确认。
- 全局设备监听提示：管理端主布局挂载 `MobileDeviceTrustPrompt`，用户在任意页面时每 3 秒静默轮询本地 `/api/mobile-devices/refresh`；检测到 connected Android 或 iOS 设备时弹出确认窗口。若同时发现多台设备，弹窗列出每台设备的自定义名称、型号、ID 和 ECID，用户可以选择目标设备后直接点击 `Install Selected`，也可以点击 `Open Certificate Setup` 跳转到 `Settings > Certificate`。跳转时 URL 携带 `mobile_device` / `mobile_platform`，Certificate 页拿到目标设备后自动滚动到对应卡片，高亮该卡片，并让对应安装按钮播放脉冲动画，确保用户知道从哪里继续操作；用户选择 Not now 后只在当前页面会话内记录设备 id，避免旧 localStorage 记录导致后续连接永远不弹。远程 Admin 访问本地 USB API 会得到 403，轮询静默忽略，不打扰远程页面。
- Certificate 页自身仍每 3 秒刷新设备列表，负责展示 Android ADB、Android CA 状态、iOS profile、Apple Configurator 和安装 session；它不再弹局部重复提示。Android 用户在手机端完成安装后，如果设备是 root/emulator 且证书库可读，页面会自动刷新为 `CA installed`；普通设备仍提示系统证书库不可由普通 ADB 验证。
- Certificate 页使用左侧固定导航和右侧单列章节，不再把 Android 和 iOS 做成并列卡片。导航和右侧内容顺序为 Availability Check、Local install、iOS devices、Android devices、Certificate downloads；可用性检查是手机/跨设备排障的最高优先级入口，iOS 设备安装必须在 Android 之前，证书文件下载和二维码下载放在最后。
- 代理交互式授权弹窗同步展示 Availability Check 紧凑入口。未授权设备触发 `Pending Authorization Requests` 时，弹窗中部自动生成一组可用性检查二维码和链接，提示用户遇到证书、代理授权或局域网连通问题时可直接用目标设备扫码检查；审批列表的 Allow/Deny 操作保持不变。
- iPhone/iPad 区块：
  - 顶部先展示统一 iOS 流程：把 profile 送到 iPhone -> 在 Settings 安装描述文件 -> Settings > General > About > Certificate Trust Settings -> 打开 Bifrost CA 完全信任。Apple Configurator 和手动扫码/文件安装只作为“送达 profile”的两种入口，不在 UI 上拆成互相割裂的两套模式。
  - 统一流程之后先展示“选择 profile 送达方式”。Apple Configurator/cfgutil 检测状态、每台 iOS 设备的自定义名称/型号/ID/ECID 和 `Configurator Install` 按钮作为自动送达入口；手动扫码/下载 `.mobileconfig` / LAN profile QR 作为手动送达入口。点击某一台的 Configurator 按钮时，Bifrost 通过该设备 ECID 从电脑侧定向发送 profile；如果 iPhone 仍要求屏幕确认，则继续按同一套 Settings 安装和信任步骤操作。
  - iOS 区块额外提供 Wi-Fi Proxy Profile POC：页面优先显示服务端检测到的当前 Mac Wi-Fi 名称和 `Proxy address`；如果 macOS 隐私/TCC 返回 `<redacted>` 或检测失败，管理端允许用户输入 iPhone 当前 Wi-Fi 名称作为兜底。所有 Wi-Fi 名称配置入口（Certificate 页 iOS 区块、Availability Check 卡片、代理交互式授权弹窗的 compact Availability Check、手机公开检测页）必须在输入区域下方直接展示 managed Wi-Fi profile 风险说明，强调 profile 不包含 Wi-Fi 密码但卸载可能移除 iOS managed Wi-Fi 网络条目。每台 connected iPhone 的设备行增加 `Proxy Config` 按钮，与 `Configurator Install` 并列；点击后通过 `cfgutil -e <ECID> install-profile` 定向下发包含 CA 和 Wi-Fi 代理 payload 的 profile。手动路径提供实验性 Wi-Fi proxy profile 下载和对应 QR，手机扫码/下载后仍需要确认安装 profile。由于卸载该 profile 可能移除 managed Wi-Fi 条目，管理端必须先要求用户勾选风险确认，未确认时禁用 `Proxy Config`、下载按钮和二维码。
  - 手动扫码送达入口使用 `web/src/assets/ios/ios_qr_1.jpeg` 和 `web/src/assets/ios/ios_qr_2.jpeg` 展示唯一区别步骤：用 iPhone Camera 扫 LAN QR 并点击黄色链接，然后允许下载 configuration profile。
  - 送达方式之后展示共享步骤 `ios_1.png` 到 `ios_7.png`；每一步用图片文件名作为步骤标识，图下文案明确说明：选择 iPhone、确认 profile 已到达、进入 Settings 的 Downloaded Profile、安装 Bifrost CA profile、接受未签名 profile 警告、进入 Settings > General > About、最后在 Certificate Trust Settings 打开 Bifrost CA 完全信任。
  - `cfgutil` 返回 `ConfigurationUtilityKit.error Code: 625` / “需要用户在设备上交互” 时，Bifrost 将其视为已把安装流程交给 iPhone 的待确认状态，而不是硬失败。
  - MDM/监督设备路径只做说明，不把普通下载、扫码或普通 USB 连接误描述成静默自动信任。
- 保留原证书文件 QR，支持手动下载 CA。
- 新增 UI 使用 Ant Design token，不新增硬编码主题色；light/dark 主题由现有 Settings 页面统一验证。

### 4. CLI

保留 `bifrost ca install` 的原有本机系统信任语义；新增移动设备路径：

- `bifrost ca install --mobile`：进入 Android USB guided install。
- `bifrost ca install --mobile --device <serial>`：指定 ADB serial，适合脚本执行。
- `bifrost ca install --mobile --yes`：非交互模式；只有一个 ready 设备时自动选择，有多个 ready 设备时要求显式 `--device`。
- `bifrost ca install --ios`：生成 `bifrost-ca.mobileconfig` 并输出 iPhone/iPad 手动安装和完全信任步骤，不尝试静默控制手机。
- `bifrost ca install --ios --configurator`：macOS + Apple Configurator 高级路径；检测 `cfgutil` 和 USB iOS 设备后调用 `cfgutil -e <ECID> install-profile`。单台设备自动选择；多台设备在交互终端中显示自定义名称/型号/ID 并让用户选择；非交互模式需要传 `--device <id-or-ecid>`。

CLI 不承诺普通手机自动启用根 CA 信任；iOS 的自动 SSL/TLS 信任只归属于 Apple Configurator/MDM/监督设备路径。
Android CLI 会在安装前后输出 `Android CA status`。普通设备通常只能显示 `pushed to device` 或 `unknown`；root/emulator/test 设备可通过用户证书库指纹比对显示 `installed`。

### 5. Availability Check

可用性检查由三个部分组成：

  - Admin API：
  - `POST /_bifrost/api/trust-probe/sessions` 创建短期探针会话。请求中的 `host` 必须是 Bifrost 发现到的本机 IP，不能传任意域名或公网地址。
  - `GET /_bifrost/api/trust-probe/sessions/{session_id}` 查询实时状态。
  - `PATCH /_bifrost/api/trust-probe/sessions/{session_id}` 更新 Wi-Fi 名称等可配置字段。管理端 Availability Check 卡片使用该接口把用户输入的 SSID 写入 probe manager，并同步到所有未过期的 active session，避免顶部卡片、代理交互式授权弹窗和手机公开页各自持有不同 session 时 SSID 互相不可见。手机公开页通过 token 化 public session 轮询实时同步。
- Public landing：
  - `GET /_bifrost/public/trust-probe/{session_id}?t=<token>` 返回自包含 HTML 检测页，不依赖登录和主 Web UI bundle。
  - `GET /_bifrost/public/trust-probe/{session_id}/session?t=<token>` 返回该公开页面可用的最小 session 配置，包括 `suggestedWifiSsid`、SSID 提示和代理配置检测结果。手机页每秒轮询该接口；用户也可以直接在手机页输入 Wi-Fi 名称并通过 `report` 回写，后端会把该 SSID 同步到所有未过期的 active Availability Check session。
  - `GET /_bifrost/public/trust-probe/{session_id}/qrcode?t=<token>` 返回扫码二维码。
  - `POST /_bifrost/public/trust-probe/{session_id}/report?t=<token>` 接收手机页面通过 HTTP 回报的 `page_opened`、`network_failed`、`tls_failed` 等事件。
  - `GET /_bifrost/public/trust-probe/{session_id}/proxy-access?t=<token>` 使用访问控制模块检查当前客户端 IP 是否已被允许使用代理；待授权时会写入 pending authorization，让管理端能继续审批。
  - 手机页会每秒自动重跑 proxy access、HTTPS trust 和 proxy configured 三项检查，不要求用户刷新页面才能看到授权、信任或代理配置状态变化；管理端 Availability Check 卡片同样持续轮询 session 状态直到过期。
  - 手机页会请求 `http://bifrost-proxy-check.invalid/_bifrost/trust-probe/proxy-configured?sid=<session_id>&t=<token>`。该 `.invalid` 域名只有在浏览器已经配置 HTTP proxy 时才会被送到 Bifrost；Bifrost 在代理入口截获该请求并记录 `proxy_configured_ok`。如果请求失败，手机页通过 HTTP report 回写 `proxy_config_failed`，并优先提示用户手动配置 Wi-Fi 代理：`Settings > Wi-Fi > current network > Configure Proxy > Manual`。iOS Wi-Fi Proxy Profile 作为实验选项，下载链接中的 SSID 来自 Bifrost 服务端检测、管理端输入或手机页输入；profile 文案明确说明不包含 Wi-Fi 密码或 join credentials，但卸载 profile 可能移除 managed Wi-Fi 网络条目，因此必须勾选风险确认后才能下载。
- 双协议 probe server：
  - 默认尝试监听 `admin_port + 2`，端口冲突时自动选择空闲端口，并在 session 响应中返回实际 `probePort`。
  - 同一端口通过 TCP `peek` 首字节区分 HTTP 与 TLS。HTTP 路径提供 `/_bifrost/trust-probe/netcheck`，HTTPS 路径提供 `/_bifrost/trust-probe/check`。
  - HTTPS 证书由当前 Bifrost CA 给所选 IP 动态签发，IP 写入 SAN。设备浏览器能成功 `fetch(https://ip:probePort/...)` 时，Bifrost 将 session 标记为 `tls_trusted`。
  - 检测成功后，手机页展示可点击复制的代理地址 `<host>:<adminPort>`，并提供公开 proxy QR 链接。

状态机：

- `created`：管理端生成二维码，等待设备扫码。
- `page_opened`：目标设备已打开 HTTP 落地页。
- `proxy_access_allowed` / `proxy_access_pending` / `proxy_access_denied` / `proxy_access_unavailable`：目标设备代理访问授权检查结果，作为 session event 与 view 字段展示。
- `proxy_configured_ok` / `proxy_config_failed`：目标设备浏览器是否真的把 HTTP 请求发到了 Bifrost 代理。该状态独立于代理授权；授权通过但未配置代理时，页面会提示安装 Wi-Fi Proxy Profile 或手动配置当前 Wi-Fi 的 HTTP Proxy。
- `network_reachable`：目标设备能访问 probe 端口的 HTTP netcheck。
- `tls_trusted`：目标设备完成 HTTPS trust check。
- `tls_failed`：netcheck 成功但 HTTPS 握手/请求失败。
- `network_failed`：HTTP 落地页已打开但 probe 端口不可达。
- `expired`：会话过期。

准确性边界：

- 成功只说明当前扫码设备的当前浏览器 TLS 链路信任 Bifrost CA。
- 不承诺所有 App 都一定能被 Bifrost 解密；Android App 可能默认不信任用户 CA，部分 App 有 certificate pinning 或自定义 TLS 栈。
- 失败也不等价于“证书一定没安装”；还可能是探针端口被防火墙拦截、设备时间错误、扫码设备和电脑不在同一网络、IP 选择错误，或浏览器在安装/信任 CA 后仍缓存旧的证书信任判断，需要完整重启浏览器后再重试。
- Wi-Fi Proxy Profile 是 POC 能力：Apple profile schema 支持 Wi-Fi manual proxy payload，但它是 managed Wi-Fi 网络配置，不是“只 patch 现有 Wi-Fi proxy 字段”。普通 iPhone 删除该 profile 时可能同时删除对应 Wi-Fi 网络条目，导致用户需要重新连接 Wi-Fi。安全默认路径必须是手动配置 Wi-Fi HTTP Proxy 并手动改回 Off；profile 只能作为实验性便捷路径。

## 依赖项

- `crates/bifrost-device`
- `crates/bifrost-admin/src/handlers/mobile_devices.rs`
- `crates/bifrost-admin/src/handlers/cert.rs`
- `crates/bifrost-admin/src/handlers/trust_probe.rs`
- `web/src/api/cert.ts`
- `web/src/pages/Settings/tabs/CertificateTab.tsx`
- `crates/bifrost-cli/src/commands/ca.rs`
- `crates/bifrost-e2e/src/tests/admin_api.rs`

## 测试方案

### 单元测试

- `bifrost-device`：
  - `parse_adb_devices` 正确解析 connected/unauthorized/offline 状态。
  - `generate_ios_mobileconfig` 包含 `com.apple.security.root` 和手动信任提示。
  - `generate_ios_wifi_proxy_mobileconfig` 包含 `com.apple.security.root`、`com.apple.wifi.managed`、`SSID_STR`、`ProxyType=Manual`、`ProxyServer`、`ProxyServerPort`，并正确 XML escape SSID。
  - PEM 证书可提取 DER。
  - `cfgutil_install_profile_args` 使用 `install-profile <profile.mobileconfig>` 子命令；指定设备时使用 `-e <ECID> install-profile <profile.mobileconfig>`。
- `bifrost-admin`：
  - mobile devices API loopback 限制逻辑。
  - CertInfo SHA-256 指纹按 DER 证书生成。
  - Availability Check token hash、状态机流转、TLS 成功覆盖历史失败、proxy access 状态记录。
- `bifrost-cli`：
  - `ca install --mobile` 单个 connected Android 自动选择。
  - 多个 connected Android 且 `--yes` 时必须传 `--device`。
  - 指定 unauthorized/offline 设备时拒绝安装。
  - `ca install --ios --configurator` 参数解析正确。

### E2E 测试

在 `crates/bifrost-e2e/src/tests/admin_api.rs` 中新增：

- `admin_api_mobile_devices_lists_android_discovery`：验证设备发现 API 返回 ADB 状态和普通手机确认提示。
- `admin_public_ios_mobileconfig_uses_current_ca`：验证 iOS mobileconfig 和二维码 public endpoint。
- `admin_public_ios_wifi_proxy_mobileconfig_contains_proxy_payload`：验证 iOS Wi-Fi Proxy mobileconfig 和二维码 public endpoint 免授权可访问，profile 同时包含 CA payload 和 Wi-Fi manual proxy payload。
- `LAN public mobile profile`：真实场景用 LAN 地址验证 profile endpoint 不触发交互式授权，返回 `application/x-apple-aspen-config` 且 plist 有效。
- `admin_api_mobile_install_requires_explicit_confirmation`：验证 Android install 操作必须显式确认。
- `admin_api_local_ca_install_requires_explicit_confirmation`：验证本机 CA install 操作必须显式确认，自动测试不误触系统证书安装。
- `admin_trust_probe_verifies_https_trust_with_current_ca`：创建 Availability Check 会话，打开 public landing/qrcode，访问 public proxy QR、proxy-access；使用配置了 Bifrost HTTP proxy 的客户端访问 `bifrost-proxy-check.invalid` 专用探针，断言 `proxyConfigured=true`；再访问 HTTP netcheck，并用当前 Bifrost CA 作为 root CA 访问 HTTPS check，最后断言 session 状态为 `tls_trusted`。
- `bifrost-device` 单元测试：验证 Android user CA store PEM 指纹匹配和不匹配路径。
- `cli_ca_install_mobile_single_device_fake_adb`：通过 fake ADB 验证 `ca install --mobile --yes` 单设备自动选择并执行 push/open。
- `cli_ca_install_mobile_multiple_devices_requires_device_fake_adb`：通过 fake ADB 验证多个 ready 设备的非交互选择边界。
- `CLI iOS guide`：真实场景验证 `bifrost ca install --ios` 输出 profile 路径、手动安装步骤、Certificate Trust Settings、Configurator 后续命令。

### 真实场景测试

新增 `human_tests/mobile-device-trust.md`：

- TC-MDT-01：API 设备发现返回普通手机确认边界。
- TC-MDT-02：iOS mobileconfig 下载包含 root payload 与 Certificate Trust Settings 提示。
- TC-MDT-03：iOS profile QR endpoint 返回 SVG。
- TC-MDT-04：Android install 缺少确认时拒绝。
- TC-MDT-05：Settings Certificate UI 不承诺自动信任，并展示左侧固定导航；右侧单列按 Availability Check、Local install、iOS、Android、Certificate downloads 顺序组织。
- TC-MDT-06：浅色/暗色主题下手机安装向导可读可操作。
- TC-MDT-07：任意管理端页面检测到 connected Android/iOS 后弹出全局确认窗口；多设备时弹窗展示设备名称/型号/ID/ECID，支持选择目标设备直接安装或跳转 Certificate 页面；Certificate 页设备列表继续自动刷新，并自动滚动、高亮目标设备卡片和脉冲提示安装按钮。
- TC-MDT-08：CLI `ca install --mobile --yes` 在 fake ADB 单设备场景自动选择并执行 guided install。
- TC-MDT-09：CLI `ca install --mobile --yes` 在 fake ADB 多设备场景要求 `--device`。
- TC-MDT-10：CLI `ca install --ios` 生成 profile 并输出手动信任步骤。
- TC-MDT-11：Settings iPhone/iPad 区域先展示统一流程概览，再展示 Apple Configurator 和手动扫码/文件两种 profile 送达方式；扫码方式展示 `ios_qr_1` / `ios_qr_2`；按 cfgutil 可用状态启用或禁用每台设备的安装按钮；多台 iOS 设备同时显示自定义名称、型号、ID、ECID，并按所选设备 ECID 定向安装。
- TC-MDT-12：Apple Configurator 返回需要手机端交互时，页面显示待用户确认而非失败。
- TC-MDT-13：Settings iPhone/iPad 在送达方式之后展示 `ios_1` 到 `ios_7` 共享图文步骤，明确 Configurator 和扫码/文件安装只差在送达 profile，后续 profile 安装与 Certificate Trust Settings 完全信任是同一条流程。
- TC-MDT-14：Android 设备卡片展示当前 CA 状态；普通 ADB 不承诺能验证 user CA store，root/emulator 可通过 `/data/misc/user/0/cacerts-added` 指纹匹配显示已安装。
- TC-MDT-15：Availability Check 使用局域网 IP 生成二维码；扫码设备依次检查代理访问授权、页面已打开、probe 端口可达、HTTPS 信任检查通过/失败，再检查代理是否已经配置；成功后展示可点击复制的代理配置和公开 proxy QR，失败时展示 iOS/Android 下一步安装和信任指引；代理未配置时明确提示下载按服务端检测或用户输入 Wi-Fi 名称生成的 iOS Wi-Fi Proxy Profile，或手动配置当前 Wi-Fi 的 HTTP Proxy。
- TC-MDT-16：公开 landing、Availability Check QR、公开 proxy QR 和 proxy-access endpoint 不受交互式访问控制误拦截；未授权局域网设备会被记录到 pending authorization。
- TC-MDT-17：代理交互式授权弹窗展示 Availability Check 二维码和链接，打开弹窗后自动生成 session，轮询状态后二维码/链接仍保留 `?t=<token>` 且二维码不出现预览遮罩白屏，目标设备可扫码进入检查页，Allow/Deny 审批流程不受影响。
- TC-MDT-18：iOS Wi-Fi Proxy Profile POC。验证扫码下载路径返回 `CA + Wi-Fi proxy` profile，iOS 设备列表的 `Proxy Config` 按钮能通过 Apple Configurator 定向下发到所选 iPhone；手机确认安装后，使用 Availability Check 验证代理配置、代理授权、probe 端口和 HTTPS 信任状态是否改善，并记录是否需要断开重连 Wi-Fi。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标、普通/受管模式边界、API 安全限制、UI 文案。
- 执行 `git status --short`、`git diff`。
- 运行 `cargo test -p bifrost-device`、`cargo test -p bifrost-admin mobile_devices cert`、相关 E2E、human_tests。
- 修复发现的问题并复跑失败路径。

第 2 轮：

- 基于最新 diff 复查新增 crate、Admin handler、Web UI、design、human_tests 索引。
- 复跑受影响测试和格式检查。
- 若发现 UI 文案误导、测试缺口或接口状态不一致，继续追加第 3 轮。

## 校验要求

- 先执行本次相关 E2E 和 human_tests。
- 最后执行 rust-project-validate：
  - `cargo fmt --all -- --check`
  - `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-device`
- `cargo test -p bifrost-cli ca_install_mobile`
- `cargo test -p bifrost-admin`
- `cargo test -p bifrost-e2e admin_api`
  - `cargo test --workspace --all-features`
  - 需要时执行 `scripts/ci/local-ci.sh`

## 文档更新要求

- 更新 `human_tests/readme.md`。
- 更新 README 和 `docs/getting-started.md`，把设备可用性检查作为手机/跨设备排障的高优先级入口说明。
