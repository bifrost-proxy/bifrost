# 启动终端移动端可用性检查

> 状态：已实现并随 foreground start 默认启用
> 更新时间：2026-06-16

## 背景

`bifrost start` 前台启动后会在终端展示 `SERVER STATUS`，其中包含网络、TLS/HTTPS 拦截、CA 证书、系统代理、CLI 代理等状态。移动端调试时，用户还需要知道手机是否能从局域网访问 Bifrost、是否已配置代理、是否已信任 Bifrost CA，以及扫码后是否真的有设备连进来。

Web 管理端的 `Settings -> Certificate -> Availability Check` 已经具备类似检查能力：通过二维码或链接让目标设备打开一个 trust probe 页面，并检查代理授权、探测端口可达性、HTTPS 证书信任和代理配置状态。本方案把这套能力扩展到前台启动终端中，作为 `SERVER STATUS` 下面的新增动态板块。

## 目标

- 在 `SERVER STATUS` 下默认新增 `MOBILE AVAILABILITY CHECK` 板块，不增加启用开关。
- 只展示有效的局域网或公网 IP，过滤 loopback、link-local、unspecified、虚拟网卡、隧道、VPN、容器桥接等不可供手机访问的地址。
- 展示一个或多个有效 IP 对应的扫码入口；如果有多个可用 IP，展示多个入口。
- 网络变化时刷新局域网 IP、二维码和链接。
- 手机扫码后，在二维码下方展示已建立连接的设备状态。
- 设备列表只展示最近活跃设备；页面关闭或一段时间无上报后自动消失，不持久化历史。
- 设备状态至少包含页面打开、来源 IP、最近活跃、代理授权、代理配置、网络可达、HTTPS CA 信任、证书/安装状态等信息；终端 `Recent devices` 每一行必须只展示该设备自己的状态，不能用 session 聚合状态兜底。
- 设备列表顺序必须稳定，WebView `Connected devices` 和终端 `Recent devices` 都按设备 IP 排序；不要按 `last_seen` 排序，否则刷新或心跳上报会导致设备行上下抖动。
- Access Control `pending` 时终端提供审批入口；如果 Web UI 已审批，终端状态自动变为 allowed，不再卡在 pending。
- HTTPS Trust Check 经 CONNECT 代理访问时，活跃 trust-probe 的 HTTPS 目标必须强制 CONNECT 直通，即使当前命中 `tlsIntercept://` 规则或应用 TLS 拦截策略，避免 Bifrost 自己拦截自己的探测端口后产生 502/UnknownIssuer。active trust-probe 的 HTTP absolute-form `netcheck/check` 请求不得经 Bifrost HTTP proxy 计为成功；除 `bifrost-proxy-check.invalid` 专用代理配置探针外，代理入口必须拒绝经代理送入的 active trust-probe 请求。
- 复用 WebView 证书页已有 trust probe 能力，避免新增第二套检测协议；USB/mobile devices 证书安装能力仍保留在既有 Web/CLI 入口。

## 非目标

- 不在本阶段重新设计 Web 证书页 Availability Check。
- 不改变默认访问控制策略；如果移动设备需要授权，终端只展示 `pending` 状态和引导信息。
- 不强制自动配置手机代理或自动安装手机证书；已有 USB/Configurator/ADB 能力仍由 Web 设置页和 `bifrost ca install --mobile` 承担。
- 不在 daemon、LaunchDaemon 或非 TTY 输出中展示动态二维码；重定向日志和 CI shell 运行默认不输出移动面板，避免污染日志文件和放大 E2E 资源压力。
- 不展示虚拟 IP、loopback、link-local、未指定地址和不可用于手机访问的本地隧道地址。

## 现有能力

### 启动状态输出

- 入口：`crates/bifrost-cli/src/commands/start.rs`
- 现状：`run_foreground` 在启动服务前后打印 `SERVER STATUS`，包括网络、TLS、CA、系统代理、CLI proxy 等静态信息。
- 限制：当前输出是一次性的，无法表达网络变化和扫码后设备状态变化。

### 局域网地址发现

- 入口：`crates/bifrost-admin/src/network.rs`
- 能力：
  - `get_local_ips()` 返回可用 IPv4 局域网 IP，并标记 preferred IP。
  - `get_local_subnets()` 已被 access control 用于检测本机子网变化。
- 限制：
  - 当前只暴露 IP 字符串和 preferred 标记，终端展示后续可能需要 interface 名、地址类型、变化原因等扩展字段。

### 证书 / 代理二维码

- 入口：`crates/bifrost-admin/src/handlers/cert.rs`
- 现有 public endpoint：
  - `/_bifrost/public/cert/qrcode?ip=<ip>`
  - `/_bifrost/public/proxy/qrcode?ip=<ip>`
- 限制：
  - 单纯证书或代理二维码不能表达完整可用性检查状态。

### Trust Probe 可用性检查

- 入口：`crates/bifrost-admin/src/handlers/trust_probe.rs`
- 能力：
  - 创建 trust probe session。
  - 生成 landing URL、二维码 URL、CA 下载 URL、proxy QR URL。
  - 记录设备打开页面、网络可达、TLS trusted/failed、代理授权、代理配置等状态。
  - 支持多个 device 通过同一 session 上报，并保留 `deviceId`、`clientIp`、`userAgent`、`platformHint`、`lastSeen`、events。
  - 通过 push scope `trust_probe` 向 Web 实时广播 session 状态。
- 限制：
  - session 创建接口目前偏 Web API；终端侧需要内部复用，而不是通过 HTTP 请求自己调用自己。
  - trust probe 的 QR 是 SVG URL；终端需要把 URL 渲染成字符二维码或展示可复制链接。

### Mobile Devices 状态

- 入口：`crates/bifrost-admin/src/handlers/mobile_devices.rs`
- 能力：
  - ADB / iOS Configurator 设备发现。
  - 证书安装状态、trusted 状态、设备 connected/unauthorized/offline 等状态。
  - push scope `mobile_devices` 已每 3 秒广播给 Web。
- 限制：
- USB 发现到的设备和 trust probe 里通过浏览器打开页面的设备不一定能一一匹配；初版不做合并，避免误报。

## 建议体验

启动后，在现有 `SERVER STATUS` 下方增加：

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
       Auto refresh paused while waiting for y/n input
     - Android Chrome 192.168.1.46  last seen 2s ago
       Page opened ✓  Network ✓  Proxy access allowed  Proxy config detected  CA trust ✓
```

如果 CA 未就绪：

```text
📱 MOBILE AVAILABILITY CHECK
   Status:        CA not ready
   Action:        Bifrost needs a CA certificate before HTTPS trust probe QR can be generated.
   Proxy:         192.168.1.23:8800
```

如果非 TTY / daemon / launchd：

```text
📱 MOBILE AVAILABILITY CHECK
   TTY dynamic refresh disabled. A one-shot Availability Check summary is printed.
```

## 数据模型

新增内部快照类型，建议放在 `bifrost-admin`，由 CLI foreground start 调用：

```rust
pub struct MobileAvailabilitySnapshot {
    pub entries: Vec<MobileAvailabilityEntry>,
    pub devices: Vec<MobileAvailabilityDevice>,
    pub pending_authorizations: Vec<MobileAvailabilityPendingAuth>,
    pub access_mode: String,
}

pub struct MobileAvailabilityEntry {
    pub ip: String,
    pub is_preferred: bool,
    pub landing_url: Option<String>,
    pub terminal_qr: Option<String>,
    pub error: Option<String>,
}

pub struct MobileAvailabilityDevice {
    pub label: String,
    pub client_ip: Option<String>,
    pub source_host: String,
    pub user_agent: Option<String>,
    pub certificate_status: String,
    pub proxy_access_status: String,
    pub proxy_config_status: String,
    pub network_status: String,
    pub last_seen_seconds_ago: i64,
}

pub struct MobileAvailabilityPendingAuth {
    pub ip: String,
    pub attempt_count: u32,
    pub first_seen_seconds_ago: u64,
}
```

数据来源：

- `entries` 来自有效地址枚举 + trust probe public session。
- `devices` 以 trust probe session devices 为主（实现中字段名即 `devices`）。
- `pending_authorizations` 来自 access control pending list；审批后应立即从 pending 列表消失，并反映到 trust probe 设备的 `proxyAccessStatus=allowed`。
- USB 发现设备暂不合并到终端面板；终端面板以浏览器 trust probe 的最近活跃设备为准，避免把已断开的 USB 历史状态持久展示出来。

## 终端刷新模型

### TTY 模式

- 启动服务后创建一个后台 task，周期性生成 `MobileAvailabilitySnapshot`。
- 使用 ANSI 控制序列刷新 `MOBILE AVAILABILITY CHECK` 板块；终端输入 Access Control 命令后强制下一帧追加重绘，避免审批结果和旧面板互相覆盖。
- 刷新触发条件：
  - `get_local_ips()` 结果变化。
  - trust probe session 状态变化。
  - pending authorization 状态变化。
  - 固定心跳，例如 3 秒，用于 last seen 相对时间更新。
- 避免每次刷新重打整个 `SERVER STATUS`，减少终端抖动。

### 非 TTY 模式

- 默认不输出移动可用性面板，也不启动动态重绘 task。
- 如果默认日志输出改为文件，非 TTY 下也不应持续写二维码刷新日志。
- macOS LaunchDaemon / 系统守护进程场景不展示动态二维码。
- 专项 E2E 可通过 `BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1` 强制输出一次面板，用于验证默认展示内容；该变量不作为用户功能开关公开。

### IP 变化处理

- 新 IP 出现：创建或复用对应 host 的 trust probe session，并加入展示。
- IP 消失：下一次快照移除对应 entry，不持久化历史地址。
- preferred IP 变化：重新排序，并在标题处展示新的 preferred IP。
- 多 IP 展示：
  - 默认展示所有有效 IP 的扫码入口。
  - 过滤无效或虚拟地址，避免把 Docker、utun、VPN、bridge、link-local 等地址提供给手机。
  - 如果有效 IP 过多，仍只展示最近/优先的前几个二维码，剩余展示 URL，避免终端高度失控。

## Access Control 终端入口

- 终端 panel 监听 access control pending authorization 列表。
- 如果有 pending IP，显示短操作：
  - `y` / `yes`：允许当前最早 pending 设备。
  - `n` / `no`：拒绝当前最早 pending 设备。
  - `allow <ip>` / `deny <ip>` 保留为隐藏兼容命令，主要交互不展示给用户。
- 终端审批只在 TTY foreground 下启用；非 TTY 不监听 stdin。
- 如果 Web UI 已审批，access control pending list 会变化，终端下一次刷新自动移除 pending 提示，并通过 trust probe 事件显示 `Proxy access allowed`。
- 终端输入不能破坏 Ctrl+C 停止服务；使用非阻塞 stdin task，识别不到命令时提示一次。
- pending 列表未变化时暂停同一板块的自动重绘，避免用户正在输入 `y`/`n` 时被定时刷新擦除；pending 变化、网络变化或审批结果变化仍会强制刷新。

## Trust Probe 集成

建议把 `TrustProbeManager` 的部分能力从 private 调整为 crate 内可复用：

- `get_or_create_terminal_probe_session(state, host, ttl)`：供终端启动面板复用 trust probe session（实际导出名）。
- `list_active_sessions()`：已有 public function 可继续使用。
- `render_terminal_qr(url)`：新增终端二维码渲染 helper，避免 CLI 侧重复依赖和实现。终端二维码编码短公开入口 `http://<host>:<port>/_bifrost/tp`，并关闭 qrcode quiet zone，以减少终端面板占用；`Open` 行仍展示完整 `/_bifrost/public/trust-probe` URL。
- Router / public trust probe handler 将 `/_bifrost/tp` 映射到同一个 fixed landing page；security public path 检查同步允许该短入口。
- `is_active_trust_probe_target(host, port)`：供 CONNECT 隧道处理查询当前活跃 trust-probe HTTPS 目标；命中时跳过 TLS 拦截，保持直通，让浏览器或移动设备真实判断 CA 是否已信任。HTTP proxy 入口也用该状态拒绝 active trust-probe 的 absolute-form `netcheck/check` 请求，避免代理路径污染浏览器直连探测结果。
- probe listener 按 host/CA key 健康复用；如果旧 listener 已停止，下一次公开页或 session 轮询自愈重建。listener 在 60 秒没有新的公开页、session 轮询、report、proxy-access、netcheck 或 HTTPS check 流量后自动停止，避免多个设备并发或刷新页面时反复启动并占用端口。

session 生命周期：

- 终端 panel 创建的 session TTL 建议 30 分钟。
- 如果设备还在连接或 panel 仍活跃，到期前自动续期。
- Ctrl+C / 服务退出时不需要显式清理；已有 session 过期清理逻辑兜底。

## 设备状态展示规则

每台设备展示一行摘要 + 一行状态 tag：

- display name：
  - 从 platform hint / user agent 推断，例如 `iPhone Safari`、`Android Chrome`。当客户端上报 `unknown` 时，服务端继续从 User-Agent 推断主流 OS、浏览器和应用容器，例如 Edge、Chrome、Safari、WeChat、DingTalk、Lark、Samsung Browser。
  - 再兜底为短 device id。
- connection：
  - `Page opened`：trust probe 已收到 `page_opened`。
  - `last seen`：来自 trust probe device `lastSeen`。
- proxy access：
  - `allowed`：可直接访问代理。
  - `pending`：interactive 模式等待用户授权。
  - `denied`：本次 session 被拒绝。
  - `unavailable`：access control 状态不可判定。
- proxy config：
  - `detected`：设备已通过代理访问到 proxy-config check host。
  - `missing`：打开页面但未检测到代理配置。
  - `pending`：设备尚未执行到该检查。
- CA trust：
  - `trusted`：HTTPS probe 成功。
  - `failed`：HTTPS probe 失败，展示失败原因。
  - `pending`：未完成。
- certificate：
  - 初版由 trust probe HTTPS 结果展示 `trusted` / `not trusted` / `checking`。

## 实现步骤

1. 抽取 admin 内部 snapshot API：
   - 新增 mobile availability snapshot builder。
   - 复用有效 IP 枚举、trust probe public session 和 access control pending/temporary whitelist 状态。
2. 增加终端 QR 渲染：
   - 使用已有 `qrcode` crate。
   - 终端输出使用 block 字符或 ANSI-independent 文本，保证浅色/深色终端可读。
3. 在 `run_foreground` 中启动 terminal panel：
   - foreground 默认启用。
   - TTY 启用动态刷新和审批输入。
   - daemon / launchd / 非 TTY 禁用动态刷新；非 TTY 仅打印一次摘要。
4. 网络变化刷新：
   - 优先复用已有 access control subnet watcher 的网络变化判断。
   - 初版可用周期 polling，后续再优化为事件驱动。
5. Web/API 兼容检查：
   - 保持现有 Web Availability Check API 不变。
   - 仅增加内部复用函数或补充只读 API，不破坏现有响应结构。

## 验证计划

### 单元测试

- `network::is_effective_client_ip()` 覆盖 private、CGNAT、公网、loopback、link-local、IPv6、documentation、benchmark、multicast 等地址。
- `render_mobile_availability_panel()` 能渲染无有效 IP、无 pending 的空状态。
- `parse_access_control_command()` 覆盖 `y`/`n`、allow/deny/help、缺失 IP、非法 IP 和多余参数。
- 终端设备列表按同一 `source_host + client_ip` 去重；同一手机刷新页面只更新最近活跃时间和状态，不追加 `ios (2)` 这类重复设备。
- `should_render_panel_update()` 在同一组 pending authorization 存在时抑制定时自动重绘，避免普通终端行输入中的 `y` / `n` 被动态面板清屏擦掉；pending IP 变化、用户提交命令后的 force render、非 TTY 一次性输出仍正常刷新。
- `render_status_value()` 仅在真实 TTY 输出中对状态值加 ANSI 颜色：通过态（reachable/trusted/allowed/configured）绿色，失败/拒绝态（failed/denied）红色，其余等待或未确认状态黄色；非 TTY、日志文件和 CI 输出保持纯文本。

### E2E 测试

- 启动真实 Bifrost 前台服务，断言默认输出 `MOBILE AVAILABILITY CHECK`。
- 断言启动输出包含 Access Control 终端 `y`/`n` 审批入口。
- 断言启动输出不把 `127.0.0.1` 作为移动端 target。
- 断言启动输出不包含 Demo/process check/ChatGPT Web startup auth 等无关控制台噪声。

### human_tests

新增 `human_tests/mobile-availability-terminal.md`：

- TC-MAT-01：前台启动后，`SERVER STATUS` 下方出现 `MOBILE AVAILABILITY CHECK`。
- TC-MAT-02：有一个局域网 IP 时，终端展示完整 Availability Check URL 和更小的短链接二维码。
- TC-MAT-03：网络变化后，终端 target、URL 和二维码自动刷新。
- TC-MAT-04：手机扫码后，终端设备列表出现该设备，状态从 `page opened` 推进到网络/代理/CA 检查结果；刷新同一页面不产生重复设备。
- TC-MAT-04 额外验证：真实终端中设备状态值按通过/等待/失败语义上色；重定向输出不包含 ANSI 控制符。
- TC-MAT-05：手机页面关闭或停止上报后，终端设备列表只保留最近活跃设备，超时后自动移除。
- TC-MAT-06：Access Control pending 时可在终端用 `y` 允许当前设备，或用 `n` 拒绝当前设备。
- TC-MAT-06 额外验证：pending 列表未变化时，面板暂停自动重绘并显示输入保护提示，避免定时刷新擦除正在输入的 Yes/No。
- TC-MAT-07：Access Control pending 在 Web UI 审批后终端自动变为 allowed。
- TC-MAT-08：启动控制台无无关 Demo/进程检查噪声。
- TC-MAT-09：HTTPS Trust Check 经由 Bifrost 代理访问且命中 `tlsIntercept://` 时返回 200，不出现 502/UnknownIssuer。

## 风险与取舍

- 动态终端刷新可能与普通日志输出互相干扰；本功能应依赖“默认日志写文件”的改造，避免 console tracing 抢占终端；非 TTY/CI 重定向路径必须跳过移动面板，避免 rules E2E 每次启动都输出二维码导致日志膨胀和资源压力。
- 完整 TUI 可以进一步支持方向键选择、固定输入栏和多设备列表焦点，但当前审批动作只有 Yes/No。先采用 pending 输入保护，不为 admin crate 引入 ratatui/crossterm 依赖；如果后续需要复杂选择或多面板操作，再升级为 TUI。
- 启动阶段如果有 demo/process 检查子进程直接输出到 stdout/stderr，需要改为捕获输出或写入日志文件，避免污染动态 panel。
- 终端二维码改用短链接和无 quiet zone 降低高度；如果多网卡过多，后续仍可改成只完整展示 preferred IP。
- 手机设备与 USB discovery 设备无法总是精确匹配；初版不合并 USB discovery，避免误报证书状态。
- 网络变化检测用 polling 简单可靠，但刷新可能有几秒延迟；如果后续需要更实时，可接入平台网络事件。
- Trust probe session 需要 CA key；如果用户跳过证书生成或 CA 不完整，只能展示代理地址和修复引导。

## 已确认决策

- 活跃设备窗口使用 75 秒，暂不配置化。
- 公网 IPv4 与 LAN IPv4 均默认展示，虚拟/隧道/特殊地址不展示。
- 终端审批命令直接执行；`allow <ip>` 可审批 pending，也可直接加入临时允许列表。
