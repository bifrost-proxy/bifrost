# 移动端可用性终端面板测试用例

## 功能模块说明

本文档验证 `bifrost start` 前台启动后默认展示的移动端可用性检查面板。面板用于在终端中展示可扫码的移动端连通性检查入口、最近活跃移动设备状态，以及 Access Control 的终端审批入口。

核心预期：

- 前台启动后默认展示 `MOBILE AVAILABILITY CHECK` 面板，无需额外参数启用。
- stdout/stderr 被重定向的非 TTY 启动默认不输出移动面板，避免 CI、rules E2E 和日志文件被二维码刷屏；专项 E2E 可使用 `BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1` 强制验证一次性输出。
- 面板仅展示有效 LAN 或公网 IPv4，不展示 loopback、link-local、虚拟网卡、VPN、容器桥接等地址。
- 如果有效 IP 或网络状态变化，面板自动刷新，二维码随新的目标地址更新。
- 创建或复用 Availability Check session 时返回的 `probePort` 必须是已经实际绑定成功的端口；如果默认 `admin_port + 2` 被系统或测试客户端源端口占用，必须自动 fallback 到空闲端口并在 API 响应中体现，避免终端面板、公开页或 E2E 拿到 stale probe URL。已有 probe listener 健康时必须复用同一个端口，不因多设备并发打开或页面刷新而反复重启；只有旧 listener 已停止时才自愈重建。
- 终端二维码使用短公开入口 `/_bifrost/tp` 且不渲染额外 quiet zone，以减少终端占用；`Open` 行仍展示完整公开检测 URL。
- 扫码访问后，最近活跃设备展示网络、证书信任、代理访问、代理配置状态；设备不再活跃后从面板消失。
- HTTPS Trust Check 经由 CONNECT 代理访问时，即使当前命中 TLS 拦截规则或应用拦截策略，也不能被 Bifrost 自身拦截成 502；活跃 trust-probe HTTPS 目标必须保持 CONNECT 直通，让浏览器完成真实证书信任判断。active trust-probe 的 HTTP absolute-form `netcheck/check` 请求不得经 Bifrost HTTP proxy 计为成功，必须返回失败以避免误判；公开检测页收到该失败时必须提示 direct probe 被代理接管，而不是误报 probe port 不可达。
- 最近活跃设备状态在真实终端中使用颜色区分：通过态为绿色，等待/未确认态为黄色，拒绝/失败态为红色；重定向日志和 CI 输出保持纯文本。
- 最近活跃设备的 `network`、`certificate`、`proxy access`、`proxy config` 必须只表示该设备自己的检测结果；不能因为同一个 trust-probe session 内其它浏览器或设备状态已通过，就把当前设备显示为同样通过。
- 最近活跃设备的设备类型必须优先从 `platformHint` 和 User-Agent 推断，覆盖主流 OS、浏览器和应用容器；不要在能从 UA 识别出信息时显示 `unknown`。
- WebView `Connected devices` 和终端 `Recent devices` 的设备顺序必须稳定，按设备 IP 排序，不按最近活跃时间排序，避免心跳上报时列表上下抖动。
- 终端在 Access Control pending 时直接提供 `Yes: y | No: n` 审批入口；Web UI 已审批的客户端不再在终端保持 pending。
- Access Control pending 期间终端面板暂停同一 pending 列表的自动重绘，避免用户输入 `y`/`n` 时被定时刷新擦掉。
- 同一台移动设备刷新同一个检查页面时，只更新最近活跃时间和状态，不新增 `ios (2)` 这类重复设备。
- 启动期间控制台不应输出与移动面板无关的 Demo/进程检查噪声。

## 前置条件

1. 使用源码启动，且使用临时数据目录，避免污染正在运行的服务数据。
2. 除非本用例明确验证系统代理，启动命令必须带 `--no-system-proxy`。
3. 禁用 Sync 自动登录弹窗：
   ```bash
   export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   ```
4. 准备一个可扫码访问同一局域网的移动设备。
5. 准备一个未被占用端口，例如 `18890`。

## 测试用例

### TC-MAT-01：前台启动默认展示移动端可用性面板

**操作步骤**：
1. 启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-human-mobile BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 18890 --unsafe-ssl --no-system-proxy
   ```
2. 观察启动输出中的 `SERVER STATUS` 后续内容。

**预期结果**：
- 终端默认展示 `MOBILE AVAILABILITY CHECK` 面板。
- 面板中展示 `Target`、`Open`、`QR`、`Recent devices`、`Access Control`。
- `QR` 区域明显小于旧的大块二维码，扫码后进入同一个 `Bifrost Availability Check` 页面。
- 无需传入 `--mobile-check-panel` 或任何额外开关。

### TC-MAT-02：仅展示有效 LAN 或公网 IPv4

**操作步骤**：
1. 在启动终端观察 `MOBILE AVAILABILITY CHECK` 面板中的 `Target` 列表。
2. 在另一终端执行：
   ```bash
   ifconfig
   ```
3. 对比 `Target` 列表与本机网卡地址。

**预期结果**：
- `Target` 不包含 `127.0.0.1`。
- `Target` 不包含 `169.254.x.x`。
- `Target` 不包含 `utun*`、`tun*`、`tap*`、`docker*`、`bridge*`、`veth*`、`vmnet*` 等虚拟/隧道接口地址。
- 如果本机存在有效 LAN 或公网 IPv4，至少展示一个有效地址。

### TC-MAT-03：二维码随网络地址变化自动刷新

**操作步骤**：
1. 记录当前面板中 `Target` 和 `Open` URL。
2. 切换网络，例如连接或断开 Wi-Fi、切换热点，等待 3 到 10 秒。
3. 观察终端面板是否自动刷新。

**预期结果**：
- 网络地址变化后，`Target` 列表自动更新。
- 新地址对应的 `Open` URL 和 QR 内容同步更新。
- 不需要用户在终端执行刷新命令。

### TC-MAT-04：移动设备扫码后展示最近活跃设备状态且刷新不重复

**操作步骤**：
1. 使用移动设备扫描终端中的 QR。
2. 在移动设备浏览器中打开检查页面。
3. 等待页面完成网络和证书检查。
4. 刷新同一个移动设备浏览器页面一次。
5. 观察终端 `Recent devices` 区域。

**预期结果**：
- `Recent devices` 出现该移动设备。
- 设备信息包含来源 IP、访问目标地址、最近活跃时间。
- 状态行包含 `network`、`certificate`、`proxy access`、`proxy config`。
- 状态值在真实终端中按语义上色：`reachable`、`trusted`、`allowed`、`configured` 为绿色；`not trusted`、`pending approval`、`not confirmed`、`checking` 为黄色；`denied`、`failed` 为红色。
- 刷新同一个页面后仍只显示一条该设备记录，不出现 `ios (2)` 或同 IP 的重复设备。

### TC-MAT-05：页面关闭或停止活跃后设备自动消失

**操作步骤**：
1. 在 TC-MAT-04 已显示设备后，关闭移动设备浏览器页面。
2. 等待至少 75 秒。
3. 观察终端 `Recent devices` 区域。

**预期结果**：
- 已关闭页面的设备不再显示在 `Recent devices` 中。
- 面板不持久化历史设备。

### TC-MAT-06：终端 Access Control 审批允许 pending 客户端

**操作步骤**：
1. 使用未在白名单内的移动设备配置代理到 Bifrost 服务端口。
2. 触发一次代理访问，让客户端进入 Access Control pending。
3. 在终端观察 `Access Control` 区域，找到 pending IP。
4. 在同一个启动终端输入：
   ```text
   y
   ```
5. 再次从移动设备发起代理访问。

**预期结果**：
- 终端面板显示 `approve current device? Yes: y | No: n` 或等价 Yes/No 入口。
- pending 列表不变时，面板不会因 attempts 或 last seen 定时变化而清屏重绘，用户正在输入的 `y`/`n` 不会消失。
- 面板显示 `auto refresh: paused while waiting for y/n input` 或等价输入保护提示。
- 终端输出 `Access Control: allowed pending client <pending-ip>` 或等价允许结果。
- pending IP 从面板中消失。
- 移动设备后续代理访问通过。

### TC-MAT-07：Web UI 已审批后终端不再保持 pending

**操作步骤**：
1. 触发一个移动设备 pending。
2. 在 Web UI 的 Access Control 区域审批该 pending IP。
3. 返回终端观察 `Access Control` 区域。

**预期结果**：
- 终端面板自动刷新。
- 已由 Web UI 审批的 IP 不再展示为 pending。
- 设备状态中的 `proxy access` 最终显示为 `allowed`。

### TC-MAT-08：启动控制台无无关 Demo/进程检查噪声

**操作步骤**：
1. 使用 TC-MAT-01 的启动命令重新启动服务。
2. 观察从启动到面板首次展示期间的终端输出。

**预期结果**：
- 终端可见输出集中在启动帮助、`SERVER STATUS`、`MOBILE AVAILABILITY CHECK` 和必要的交互提示。
- 不出现与本面板无关的 Demo 进程检查、浏览器检查、ChatGPT Web 检查等后台诊断日志。

### TC-MAT-08B：非 TTY/CI 重定向启动不输出移动面板

**操作步骤**：
1. 使用 stdout/stderr 重定向方式启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-human-mobile-nontty BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 18891 --unsafe-ssl --no-system-proxy > /tmp/bifrost-nontty.log 2>&1
   ```
2. 观察 `/tmp/bifrost-nontty.log`。

**预期结果**：
- 日志中不出现 `MOBILE AVAILABILITY CHECK`。
- 日志中不出现终端二维码块。
- 专项测试如果设置 `BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1`，可以强制输出一次面板用于断言。

### TC-MAT-09：HTTPS Trust Check 走代理且命中 TLS 拦截时不返回 502

**操作步骤**：
1. 启动带显式 TLS 拦截规则的测试服务，例如规则包含：
   ```text
   127.0.0.1 tlsIntercept://
   ```
2. 创建 trust-probe session，并记录返回的 `probePort` 与 `sessionId`。
3. 下载当前 Bifrost CA，并让测试客户端信任该 CA。
4. 通过 Bifrost HTTP 代理访问：
   ```text
   https://127.0.0.1:<probePort>/_bifrost/trust-probe/check?sid=<sessionId>&deviceId=<deviceId>
   ```

**预期结果**：
- 请求返回 HTTP 200，不返回 502。
- 代理日志不出现该 trust-probe check 请求对应的 `REQUEST_TLS_FAILED` / `UnknownIssuer` 上游证书错误。
- trust-probe 设备状态能够继续推进到 `browser HTTPS passed` 或根据客户端 CA 信任情况显示真实结果，而不是被代理内部拦截失败覆盖。
- 如果把 active trust-probe 的 HTTP `netcheck` 或 HTTPS `check` URL 作为 HTTP absolute-form 请求直接发给 Bifrost proxy，响应为失败状态，且不会把设备状态推进为 network/browser HTTPS 通过态；`bifrost-proxy-check.invalid` 仍然可以经代理标记 proxy config detected。

### TC-MAT-10：设备级状态不继承 session 聚合状态

**操作步骤**：
1. 使用同一个 trust-probe session 让某个浏览器或设备完成 `proxyConfigured` 检测。
2. 使用另一个未配置代理的设备打开同一个检测页面，并确认 WebView 证书页中该设备的 Proxy config 仍为未配置。
3. 观察终端 `Recent devices` 中该设备所在行。

**预期结果**：
- 未配置代理的设备行显示 `proxy config: not confirmed`。
- 如果该设备没有独立上报网络、证书或代理访问通过状态，对应行也不能继承 session 内其它设备的 `reachable`、`trusted`、`allowed` 状态。
- 终端状态与 WebView 证书页的设备级状态一致。

### TC-MAT-11：设备列表按 IP 稳定排序

**操作步骤**：
1. 在同一个 trust-probe session 中建立多个设备记录，例如 `10.71.219.32`、`10.4.224.68`、`127.0.0.1`。
2. 让其中一个设备持续刷新或上报心跳，改变它的 `lastSeen`。
3. 观察 WebView `Connected devices` 和终端 `Recent devices`。

**预期结果**：
- 设备列表按 IP 排序，顺序不会因为 `lastSeen` 更新而变化。
- WebView 和终端都不再使用 `lastSeen` 作为主排序字段。
- 没有 IP 的设备排在有 IP 的设备后面，并使用设备 id/label 稳定兜底。

### TC-MAT-12：WebView Availability Check 不展示 probe event 列表

**操作步骤**：
1. 打开 `/_bifrost/settings?tab=certificate`。
2. 在 Availability Check 中生成或复用一个 session，让设备打开检测页并产生 `netcheck_ok`、`tls_ok`、`proxy_config_failed` 等事件。
3. 观察黄色 `Experimental managed Wi-Fi profile` 提示下方区域。

**预期结果**：
- 顶部 `Connected devices` 中仍展示每个设备的 Page、Network、Browser HTTPS、Access、Proxy 状态。
- 黄色提示下方不再展示 `proxy_config_failed`、`tls_ok`、`netcheck_ok` 等 probe event 列表。
- 后端仍保留事件数据，前端只是不在该面板中渲染事件流。

### TC-MAT-13：probe listener 复用、自愈与代理接管提示回归

**操作步骤**：
1. 启动测试服务并打开 `http://<lan-ip>:<port>/_bifrost/public/trust-probe`。
2. 记录公开页生成的 `probePort`，确认本机 `lsof -nP -iTCP:<probePort> -sTCP:LISTEN` 能看到对应 Bifrost listener。
3. 使用多个设备或同一浏览器多次刷新同一个公开检测页。
4. 再次检查 `probePort` listener 数量和端口号。
5. 在本机浏览器配置 HTTP 代理为 Bifrost 服务端口后，打开同一个公开检测页，并观察 probe port 检查提示。
6. 使用 curl 验证 direct netcheck 的两条路径：
   ```bash
   curl -sS "http://<lan-ip>:<probePort>/_bifrost/trust-probe/netcheck?sid=<sessionId>&deviceId=direct"
   curl -sS -x "http://127.0.0.1:<port>" --noproxy "" "http://<lan-ip>:<probePort>/_bifrost/trust-probe/netcheck?sid=<sessionId>&deviceId=proxied" -i
   ```

**预期结果**：
- 多设备并发打开和页面刷新仍复用同一个健康 `probePort`，不会反复启动新 listener 或占用多个端口。
- 如果旧 listener 已停止，再次打开公开检测页会自动重建 probe listener，并更新 session 中的 `probePort`。
- direct netcheck 直连返回 200。
- 通过 Bifrost 代理发送的 active trust-probe absolute-form netcheck 返回 409，错误为 `trust_probe_must_bypass_proxy`。
- 公开检测页在该 409 场景显示 `Direct probe request went through the configured proxy` 或等价提示，不显示 `Probe port is not reachable`。

### TC-MAT-14：设备类型从 User-Agent 推断，避免 unknown

**操作步骤**：
1. 使用 macOS/Windows 桌面浏览器、Android 浏览器、iOS Safari 或常见内置浏览器/应用容器打开公开 Availability Check 页面。
2. 观察终端 `Recent devices` 和 WebView `Connected devices` 的设备类型标签。
3. 对本机桌面浏览器配置代理后再次打开公开检测页，确认即使客户端上报 `platformHint=unknown`，服务端仍能从 User-Agent 推断类型。

**预期结果**：
- macOS Edge/Chrome/Safari、Windows Chrome/Edge、iOS Safari、Android Chrome 等主流浏览器展示为 `macos edge`、`windows chrome`、`ios safari`、`android chrome` 或等价 OS + 浏览器标签。
- WeChat、Alipay、DingTalk、Lark、QQBrowser、Samsung/Huawei/MIUI/UC/Quark/Baidu/Sogou 等常见应用或浏览器容器能展示对应 app/browser 名称。
- 只有 UA 为空或完全无法识别时才退回 device id 或通用 `browser`，不应在有可解析 UA 时显示 `unknown`。

### TC-MAT-16：前台 Ctrl-C 停止不需要额外回车

**操作步骤**：
1. 使用真实 PTY 前台启动 Bifrost，且禁用系统代理和托盘：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=0 e2e-tests/tests/test_cli_foreground_ctrlc_no_enter.sh
   ```
2. 等待输出出现 `MOBILE AVAILABILITY CHECK`。
3. 向前台进程发送 `Ctrl-C`，但不再向 stdin 写入回车或其它字符。

**预期结果**：
- Bifrost 在 8 秒内优雅退出，脚本输出 `PASS: foreground Ctrl-C exits without an extra Enter`。
- 退出输出包含 `Bifrost proxy stopped.`。
- 测试脚本在 CI 中复用外层预构建的 `BIFROST_BIN`，不重新编译 debug binary，也不使用固定端口。

## 清理步骤

1. 在启动终端按 `Ctrl+C` 停止服务。
2. 清理临时数据目录：
   ```bash
   rm -rf ./.bifrost-human-mobile
   ```

## 本次执行记录

| 日期 | 用例 | 执行方式 | 结果 |
| --- | --- | --- | --- |
| 2026-06-08 | TC-MAT-01 / TC-MAT-02 / TC-MAT-06 / TC-MAT-08 / TC-MAT-08B | 执行 `e2e-tests/tests/test_mobile_availability_terminal_panel.sh`，使用临时数据目录、`BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1` 和真实 `bifrost start -p <random> --unsafe-ssl --skip-cert-check --no-system-proxy` 重定向启动，断言强制输出 `MOBILE AVAILABILITY CHECK`、不展示 loopback target、Access Control 显示 `shortcut: y = allow current, n = deny current`、启动控制台不包含 Demo/process check/ChatGPT Web 噪声；执行 `cargo test -p bifrost-admin mobile_availability --lib` 覆盖非 TTY 默认跳过、force env 才输出；使用当前 `target/release/bifrost` 真实重定向启动并 grep 日志，确认未设置 force 时不输出 `MOBILE AVAILABILITY CHECK` 或二维码块 | 通过 |
| 2026-06-08 | TC-MAT-01 / TC-MAT-04 / TC-MAT-06 二维码、刷新去重、颜色与输入保护回归 | 执行 `cargo test -p bifrost-admin mobile_availability --lib`，覆盖 `terminal_qr_uses_compact_short_url_without_quiet_zone`、`dedupe_recent_devices_collapses_refreshes_from_same_client_ip_and_host`、`render_panel_colors_status_values_only_for_terminal_output` 和 `pending_panel_suppresses_automatic_rerender_with_same_pending_ip`：终端二维码使用短入口且宽高小于长 URL 二维码；同一 host 下同一 client IP 的两条 iOS 最近设备记录合并为一条；TTY 输出状态值上色而普通输出无 ANSI；同一 pending IP 存在时自动重绘被抑制，force render 或 pending IP 变化仍会刷新 | 通过 |
| 2026-06-08 | TC-MAT-10 | 执行 `cargo test -p bifrost-admin recent_device_status_texts_are_device_scoped --lib`，覆盖设备级 certificate/network/proxy access/proxy config 状态函数只按设备状态输出，不使用 session 聚合状态兜底 | 通过 |
| 2026-06-08 | TC-MAT-11 | 执行 `cargo test -p bifrost-admin device_status_is_tracked_per_local_storage_device_id --lib`，覆盖 WebView `session.devices` 按 `192.168.1.20`、`192.168.1.21` IP 顺序输出；执行 `cargo test -p bifrost-admin recent_devices_are_sorted_by_client_ip_not_last_seen --lib`，覆盖终端 `Recent devices` 不按 `last_seen_seconds_ago` 排序 | 通过 |
| 2026-06-08 | TC-MAT-12 | 执行 `pnpm -C web exec eslint src/components/AvailabilityCheckPanel/index.tsx` 通过，确认移除 `showEvents` prop 和 probe event `List` 渲染后组件无 lint 错误；本地 `pnpm exec prettier` 不可用，等待远端 CI 的前端 build/format 覆盖 | 通过 |
| 2026-06-08 | TC-MAT-13 | 执行 `cargo test -p bifrost-admin trust_probe --lib`，覆盖健康 listener 直接复用且不加载 CA、不重新 bind，stale listener 会被移除后进入重建路径，60 秒 idle reaper 只关闭匹配的旧 listener 且不误关新 listener；执行 `cargo run -p bifrost-e2e -- --test admin_trust_probe_verifies_https_trust_with_current_ca --test-timeout 180`，覆盖公开检测页包含 `trust_probe_must_bypass_proxy` 识别分支和代理接管提示；执行真实 curl 直连/代理路径验证：直连 netcheck 返回 200，经代理 absolute-form netcheck 返回 409 `trust_probe_must_bypass_proxy`；严格 HTTPS curl 不信任当前 CA，`curl -k` 和转换后的 CA PEM `--cacert` 均返回 200，证明截图中的 Browser HTTPS probe failed 是浏览器 CA 信任问题，不是 probe 端口不可达 | 通过 |
| 2026-06-08 | TC-MAT-13 Windows CI 回归 | 针对 Windows CI 中 `admin_trust_probe_verifies_https_trust_with_current_ca` 失败补充验证：当 preferred probe port 不可用并 fallback 后，代理客户端访问 stale absolute-form direct probe URL 不应转发成 502，而应直接返回 409 `trust_probe_must_bypass_proxy`。执行 `cargo test -p bifrost-proxy test_proxy_request_to_trust_probe_direct_target_rejects_stale_probe_ports --lib` 通过；执行 `cargo run -p bifrost-e2e -- --test admin_trust_probe_verifies_https_trust_with_current_ca --test-timeout 180` 通过，日志显示 `Rejected proxy-routed availability probe`，用例 1/1 PASS | 通过 |
| 2026-06-08 | TC-MAT-14 | 执行 `cargo test -p bifrost-admin infer_device_platform_hint_covers_common_os_browser_and_apps --lib` 和 `cargo test -p bifrost-admin recent_device_label_infers_platform_from_user_agent_when_hint_is_unknown --lib`，覆盖 macOS Edge、Windows Chrome、iOS Safari、Android WeChat、Android Samsung Browser、未知自定义 UA fallback，以及终端 label 从 `platformHint=unknown` 回退到 UA 推断 | 通过 |
| 2026-06-10 | TC-MAT-15 create session 返回实际 probePort 回归 | 针对 main CI Windows ARM 中 `admin_trust_probe_verifies_https_trust_with_current_ca` 失败补充验证：当 `admin_port + 2` 与测试客户端本地源端口或其它 listener 冲突时，创建/复用 Availability Check session 必须先确保 probe listener 实际绑定，并把同一 host/admin port/CA 下的 active session `probePort` 写回真实端口。执行 `cargo test -p bifrost-admin update_probe_port_for_group_updates_only_matching_active_sessions --lib` 通过；执行 `cargo test -p bifrost-admin trust_probe --lib` 通过 21 个相关测试；执行 `cargo run -p bifrost-e2e -- --test admin_trust_probe_verifies_https_trust_with_current_ca --test-timeout 180` 通过，日志显示代理 absolute-form direct probe 仍返回 `trust_probe_must_bypass_proxy`，且直连 netcheck/HTTPS check 成功 | 通过 |
| 2026-06-11 | TC-MAT-16 前台 Ctrl-C 不需要额外回车 | 针对 PR CI run `27299005413` 的 Linux shard 1 失败补充验证：失败日志显示 `test_cli_foreground_ctrlc_no_enter.sh` 在重新构建 debug binary 后耗尽 240s per-test budget，并以 `OSError: [Errno 5] Input/output error` 退出。修复脚本后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=0 e2e-tests/tests/test_cli_foreground_ctrlc_no_enter.sh`，复用 release binary、动态端口和临时数据目录，输出 `PASS: foreground Ctrl-C exits without an extra Enter` | 通过 |
