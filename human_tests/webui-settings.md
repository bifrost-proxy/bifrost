# Web UI Settings 页面测试用例

## 功能模块说明

Settings 页面是 Bifrost 管理端的系统设置中心，包含多个功能 Tab：Proxy（代理设置）、Certificate（证书管理）、TLS/Interception（TLS 拦截配置）、Performance（性能配置）、Access Control（访问控制）、Appearance（外观设置）、Metrics（指标监控）、Sync（同步功能）。本文档不包含 Remote Access Tab（已在 `remote-access-web-ui.md` 中覆盖）。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/settings`
3. 确保端口 8800 未被防火墙阻止

---

## 测试用例

### Proxy Tab

#### TC-WST-01：查看代理端口信息

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/settings?tab=proxy`

**预期结果**：
- Settings 页面显示 "Proxy" Tab 为当前活动标签
- 显示当前代理端口号（如 `8800`）
- 端口信息清晰可读

---

#### TC-WST-02：查看代理地址

**操作步骤**：
1. 在 Proxy Tab 中查看代理地址信息

**预期结果**：
- 显示代理服务器的监听地址（如 `127.0.0.1:8800` 或 `0.0.0.0:8800`）
- 显示局域网 IP 地址（如 `192.168.x.x:8800`），方便其他设备连接

---

#### TC-WST-03：查看代理二维码

**操作步骤**：
1. 在 Proxy Tab 中查找二维码区域

**预期结果**：
- 页面上显示代理配置的二维码
- 二维码可被手机扫描识别
- 二维码包含代理服务器地址和端口信息

---

### Certificate Tab

#### TC-WST-04：查看证书状态

**操作步骤**：
1. 点击 "Certificate" Tab 或打开 `http://127.0.0.1:8800/_bifrost/settings?tab=cert`

**预期结果**：
- 显示 CA 证书的当前状态（已安装/未安装）
- 显示证书的基本信息（如颁发者、有效期等）

---

#### TC-WST-05：下载 CA 证书

**操作步骤**：
1. 在 Certificate Tab 中点击 "Download" 或 "下载证书" 按钮

**预期结果**：
- 浏览器触发文件下载
- 下载的文件为 CA 证书文件（如 `.pem` 或 `.crt` 格式）
- 文件大小合理（非空文件）

---

#### TC-WST-06：查看证书二维码

**操作步骤**：
1. 在 Certificate Tab 中查找证书二维码区域

**预期结果**：
- 页面上显示证书下载链接的二维码
- 二维码可被手机扫描识别
- 扫描后可以在移动设备上下载证书

---

### TLS/Interception Tab

#### TC-WST-07：查看 TLS 拦截状态

**操作步骤**：
1. 点击 "TLS/Interception" Tab 或打开 `http://127.0.0.1:8800/_bifrost/settings?tab=tls`

**预期结果**：
- 显示 TLS 拦截功能的当前状态（开启/关闭）
- 显示 TLS 拦截的开关控件

---

#### TC-WST-08：启用 TLS 拦截

**操作步骤**：
1. 在 TLS/Interception Tab 中，如果 TLS 拦截当前为关闭状态
2. 点击 TLS 拦截的开关控件启用

**预期结果**：
- TLS 拦截状态变为启用
- 显示成功提示（Toast）
- 页面刷新后状态仍为启用

---

#### TC-WST-09：禁用 TLS 拦截

**前置条件**：TLS 拦截当前为启用状态

**操作步骤**：
1. 点击 TLS 拦截的开关控件关闭

**预期结果**：
- TLS 拦截状态变为禁用
- 显示成功提示（Toast）
- 页面刷新后状态仍为禁用

---

#### TC-WST-10：添加排除域名

**操作步骤**：
1. 在 TLS/Interception Tab 中找到排除域名（Exclude Domain）配置区域
2. 输入域名 `example.com`
3. 确认添加

**预期结果**：
- 排除域名列表中出现 `example.com`
- 该域名的 HTTPS 流量将不会被 TLS 拦截
- 显示成功提示

---

#### TC-WST-11：添加包含域名

**操作步骤**：
1. 在 TLS/Interception Tab 中找到包含域名（Include Domain）配置区域
2. 输入域名 `api.example.com`
3. 确认添加

**预期结果**：
- 包含域名列表中出现 `api.example.com`
- 仅该域名的 HTTPS 流量会被 TLS 拦截（当使用包含模式时）
- 显示成功提示

---

#### TC-WST-12：添加排除应用

**操作步骤**：
1. 在 TLS/Interception Tab 中找到应用排除（App Exclude）配置区域
2. 输入应用名称或进程名
3. 确认添加

**预期结果**：
- 排除应用列表中出现添加的应用
- 该应用的流量将不会被 TLS 拦截
- 显示成功提示

---

#### TC-WST-13：添加包含应用

**操作步骤**：
1. 在 TLS/Interception Tab 中找到应用包含（App Include）配置区域
2. 输入应用名称或进程名
3. 确认添加

**预期结果**：
- 包含应用列表中出现添加的应用
- 仅该应用的流量会被 TLS 拦截（当使用包含模式时）
- 显示成功提示

---

#### TC-WST-14：Unsafe SSL 开关

**操作步骤**：
1. 在 TLS/Interception Tab 中找到 "Unsafe SSL" 开关
2. 切换开关状态

**预期结果**：
- 开关状态正确切换
- 显示成功提示（Toast）
- 启用后，代理不验证上游服务器的 SSL 证书
- 页面刷新后状态保持一致

---

#### TC-WST-15：配置变更时断开连接开关

**操作步骤**：
1. 在 TLS/Interception Tab 中找到 "Disconnect on Config Change"（配置变更时断开连接）开关
2. 切换开关状态

**预期结果**：
- 开关状态正确切换
- 显示成功提示（Toast）
- 启用后，TLS 配置变更时会主动断开现有连接
- 页面刷新后状态保持一致

---

### Performance Tab

#### TC-WST-16：查看性能配置

**操作步骤**：
1. 点击 "Performance" Tab 或打开 `http://127.0.0.1:8800/_bifrost/settings?tab=performance`

**预期结果**：
- 显示当前的性能配置参数
- 包含最大记录数、最大数据库大小、Body 大小限制等配置项
- 各配置项显示当前值

---

#### TC-WST-17：修改最大记录数

**操作步骤**：
1. 在 Performance Tab 中找到最大记录数（Max Records）配置项
2. 将值修改为一个新的数字（如 `5000`）
3. 保存/确认修改

**预期结果**：
- 配置值更新为 `5000`
- 显示成功提示（Toast）
- 页面刷新后值保持为修改后的 `5000`

---

#### TC-WST-18：修改最大数据库大小

**操作步骤**：
1. 在 Performance Tab 中找到最大数据库大小（Max DB Size）配置项
2. 将值修改为一个新的大小值
3. 保存/确认修改

**预期结果**：
- 配置值更新为新值
- 显示成功提示（Toast）
- 页面刷新后值保持为修改后的值

---

#### TC-WST-19：修改 Body 大小限制

**操作步骤**：
1. 在 Performance Tab 中找到 Body 大小限制（Body Size Limit）配置项
2. 将值修改为一个新的大小值
3. 保存/确认修改

**预期结果**：
- 配置值更新为新值
- 显示成功提示（Toast）
- 超过该大小的请求/响应 Body 将不被存储
- 页面刷新后值保持一致

---

#### TC-WST-20：清除缓存

**操作步骤**：
1. 在 Performance Tab 中找到 "Clear Cache"（清除缓存）按钮
2. 点击按钮

**预期结果**：
- 弹出确认对话框或直接执行清除操作
- 清除成功后显示提示（Toast）
- 缓存数据被清理

---

#### TC-WST-21：查看 Body Store 统计信息

**操作步骤**：
1. 在 Performance Tab 中查看 Body Store 相关的统计信息

**预期结果**：
- 显示 Body Store 的当前大小
- 显示已存储的 Body 数量
- 信息数值合理，与实际使用量一致

---

#### TC-WST-22：查看 Traffic Store 统计信息

**操作步骤**：
1. 在 Performance Tab 中查看 Traffic Store 相关的统计信息

**预期结果**：
- 显示 Traffic Store 的当前大小
- 显示已存储的流量记录数量
- 信息数值合理，与实际使用量一致

---

#### TC-WST-43：Max Body Probe Size 相邻刻度文本不重叠

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/settings?tab=performance`
2. 在 Performance Tab 中找到 `Max Body Probe Size` 配置项
3. 查看滑块刻度 `Off`、`16KB`、`64KB`、`256KB`、`1MB`
4. 切换为深色主题后再次查看同一滑块刻度

**预期结果**：
- `Off` 和 `16KB` 两个左侧相邻刻度文本保持至少可读间距，不出现重叠或贴压
- 其余刻度文本仍位于对应滑块位置附近，滑块最小值、最大值和步进不变
- 亮色和暗色主题下刻度均清晰可读

**执行记录**：
- 2026-08-21：PASS。执行 `WEB_PORT=19112 BACKEND_PORT=19111 BIFROST_UI_TEST_PORT=19111 ADMIN_API_BASE=http://127.0.0.1:19111/_bifrost/api PROXY_URL=http://127.0.0.1:19111 PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH="..." pnpm --dir web exec playwright test --config=playwright.frontend.config.ts admin-settings.spec.ts -g "Max Body Probe Size 相邻刻度文本不重叠"`，使用前端-only Playwright 配置和临时 Admin API mock，真实 Chromium 打开 Settings Performance 页面，分别在亮色和暗色主题下量测 `Off`、`16KB`、`64KB`、`256KB`、`1MB` 全部相邻刻度文本边界，1/1 PASS。

---

### Access Control Tab

#### TC-WST-23：查看访问控制模式

**操作步骤**：
1. 点击 "Access Control" Tab 或打开 `http://127.0.0.1:8800/_bifrost/settings?tab=access`

**预期结果**：
- 显示当前的访问控制模式（白名单模式选择）
- 显示 IP 白名单列表
- 显示 Allow LAN（允许局域网）开关状态
- 显示待授权列表（Pending Authorizations）

---

#### TC-WST-24：切换白名单模式

**操作步骤**：
1. 在 Access Control Tab 中切换白名单模式

**预期结果**：
- 白名单模式切换成功
- 显示成功提示（Toast）
- 页面刷新后模式保持一致

---

#### TC-WST-25：添加 IP 到白名单

**操作步骤**：
1. 在 Access Control Tab 中找到 IP 白名单配置区域
2. 输入 IP 地址 `192.168.1.100`
3. 确认添加

**预期结果**：
- IP 白名单列表中出现 `192.168.1.100`
- 该 IP 被允许访问代理服务
- 显示成功提示

---

#### TC-WST-26：从白名单中移除 IP

**前置条件**：白名单中已有 `192.168.1.100`

**操作步骤**：
1. 在 IP 白名单列表中找到 `192.168.1.100`
2. 点击删除按钮移除该 IP

**预期结果**：
- `192.168.1.100` 从白名单列表中消失
- 该 IP 不再被允许访问代理服务
- 显示成功提示

---

#### TC-WST-27：切换允许局域网访问开关

**操作步骤**：
1. 在 Access Control Tab 中找到 "Allow LAN"（允许局域网）开关
2. 切换开关状态

**预期结果**：
- 开关状态正确切换
- 显示成功提示（Toast）
- 启用时，同一局域网内的设备可以访问代理服务
- 禁用时，仅白名单中的 IP 可以访问
- 页面刷新后状态保持一致

---

#### TC-WST-28：查看待授权请求

**操作步骤**：
1. 在 Access Control Tab 中找到 "Pending Authorizations"（待授权）区域

**预期结果**：
- 显示待授权的连接请求列表
- 如果没有待授权请求，显示空状态
- 如果有待授权请求，显示请求来源 IP 和时间
- 可以批准或拒绝待授权请求

---

### Appearance Tab

#### TC-WST-29：切换深色主题

**操作步骤**：
1. 点击 "Appearance" Tab 或打开 `http://127.0.0.1:8800/_bifrost/settings?tab=appearance`
2. 选择 "Dark"（深色）主题

**预期结果**：
- 页面立即切换为深色主题
- 背景变为深色，文字变为浅色
- 所有 UI 组件正确适配深色主题
- 页面刷新后主题保持为深色

---

#### TC-WST-30：切换浅色主题

**前置条件**：当前为深色主题

**操作步骤**：
1. 在 Appearance Tab 中选择 "Light"（浅色）主题

**预期结果**：
- 页面立即切换为浅色主题
- 背景变为白色/浅色，文字变为深色
- 所有 UI 组件正确适配浅色主题
- 页面刷新后主题保持为浅色

---

### Metrics Tab

#### TC-WST-31：查看 CPU 和内存图表

**操作步骤**：
1. 点击 "Metrics" Tab 或打开 `http://127.0.0.1:8800/_bifrost/settings?tab=metrics`

**预期结果**：
- 显示 CPU 使用率图表，图表有时间轴和百分比轴
- 显示内存使用图表，图表有时间轴和容量轴
- 图表数据实时更新
- 图表渲染正常，无空白或报错

---

#### TC-WST-32：查看 QPS 图表

**操作步骤**：
1. 在 Metrics Tab 中查看 QPS（每秒查询数）图表

**预期结果**：
- 显示 QPS 图表，图表有时间轴和请求数轴
- 当有流量通过代理时，图表显示相应的 QPS 数据
- 无流量时图表显示零值
- 图表渲染正常

---

#### TC-WST-33：查看应用指标

**操作步骤**：
1. 在 Metrics Tab 中查看应用指标（App Metrics）区域

**预期结果**：
- 显示按应用/进程分组的指标数据
- 包含各应用的请求数量、流量大小等信息
- 数据呈现方式清晰（表格或图表）

---

#### TC-WST-34：查看域名指标

**操作步骤**：
1. 在 Metrics Tab 中查看域名指标（Host Metrics）区域

**预期结果**：
- 显示按域名分组的指标数据
- 包含各域名的请求数量、流量大小等信息
- 数据呈现方式清晰（表格或图表）
- 可以查看最活跃的域名排行

---

### Sync Tab

#### TC-WST-35：查看同步登录状态

**操作步骤**：
1. 点击 "Sync" Tab 或打开 `http://127.0.0.1:8800/_bifrost/settings?tab=sync`

**预期结果**：
- 显示同步功能的当前状态
- 如果未登录，显示登录入口（登录按钮或二维码）
- 如果已登录，显示当前登录的账户信息

---

#### TC-WST-36：同步登录

**操作步骤**：
1. 在 Sync Tab 中点击登录按钮或扫描二维码进行登录

**预期结果**：
- 显示登录流程（扫码/输入凭据）
- 登录成功后显示账户信息
- 同步状态更新为已连接
- 显示成功提示

---

#### TC-WST-37：查看同步状态

**前置条件**：已通过 TC-WST-36 完成同步登录

**操作步骤**：
1. 在 Sync Tab 中查看同步状态信息

**预期结果**：
- 显示当前同步连接状态（已连接/断开）
- 显示上次同步时间
- 显示同步的数据概况
- 状态信息实时更新

---

#### TC-WST-38：执行同步

**前置条件**：已通过 TC-WST-36 完成同步登录

**操作步骤**：
1. 在 Sync Tab 中点击 "Sync" 或 "Run Sync"（执行同步）按钮

**预期结果**：
- 同步过程开始，显示同步进度或加载状态
- 同步完成后显示成功提示
- 上次同步时间更新为当前时间
- 配置数据与云端保持一致

---

#### TC-WST-39：Bifrost Cloud URL 在 Provider 卡片内可编辑且旧 Remote Sync 面板不再显示

**操作步骤**：
1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=sync`
2. 等待 Sync 页面显示三张 Provider 卡片：ByteDance Internal、Bifrost Cloud、GitHub Gist
3. 确认页面底部不再显示旧的 `Remote Sync` 卡片或旧的全局 Remote URL 输入框
4. 在 Bifrost Cloud 卡片的 Remote 输入框中输入 `http://127.0.0.1:61580/custom/`
5. 停留至少 3 秒，等待页面完成一次 Sync 状态轮询刷新
6. 点击 Bifrost Cloud Remote 输入框右侧的 "Save" 按钮

**预期结果**：
- Sync 页面只有 Provider 卡片作为主要管理入口，不出现旧 `Remote Sync` 面板
- Bifrost Cloud 的 Remote 输入框是可输入状态，ByteDance Internal 和 GitHub Gist 仍按各自能力展示只读 Remote 信息
- 等待轮询刷新期间，Bifrost Cloud 输入框内容保持为 `http://127.0.0.1:61580/custom/`，不会回滚到旧的默认地址
- 点击 Save 后，提交的是当前输入框内容
- 保存成功后 Bifrost Cloud 输入框显示后端返回的 Remote URL，且仍与刚提交的当前输入保持一致

**执行记录**：
- 2026-07-07：PASS。执行 `pnpm --dir web run test:ui tests/ui/admin-settings.spec.ts --grep "Settings Sync"`，真实 Chromium 打开 Settings Sync 页面，7/7 PASS。断言旧 `Remote Sync` 面板不存在，Bifrost Cloud 卡片内 Remote URL 输入框可编辑，状态轮询不会覆盖输入，点击 Save 后提交当前 URL 并保持后端返回值。随后在 2048px 视口用 Playwright 量测三张 provider 卡片，三张同排且单卡宽度均为 469px，满足宽屏下更宽卡片且最多一排三张。

#### TC-WST-40：GitHub Gist Provider 支持 token 登录

**操作步骤**：
1. 打开 `Settings -> Sync`。
2. 确认 `GitHub Gist` 卡片展示生成 token 的引导，并提供 `Generate Token` 链接。
3. 点击 `Generate Token`，确认跳转 GitHub token 生成页，URL 预填 `scopes=gist`。
4. 回到 Bifrost，点击 `GitHub Gist` 卡片中的 `Sign In`。
5. 在 `Sign in to GitHub Gist` 弹窗中确认也有 `Generate Token` 链接，然后输入 GitHub token。
6. 点击弹窗 `Sign In`。

**预期结果**：
- `GitHub Gist` 登录按钮可点击，不再是 disabled。
- 卡片和弹窗都展示 `Generate Token` 链接，目标为 `https://github.com/settings/tokens/new?description=Bifrost%20Sync&scopes=gist`。
- 弹窗只展示 GitHub token 登录，不展示本机 callback、OAuth device flow、`Continue with GitHub` 或其它需要 Bifrost 维护 GitHub App 的入口。
- 弹窗提示 token 需要 `gist` scope。
- GitHub 不支持让第三方页面自动读取 token 生成结果，因此发布版本不要求自动回填；用户复制 token 后粘贴到 Bifrost。
- 前端向 `POST /_bifrost/api/sync/login` 发送 `provider_id=github_gist` 和 token。
- 后端验证 token 成功后，`GitHub Gist` 卡片显示 `Connected` 和 GitHub 用户。
- `GitHub Gist` 仍显示 `Remote Invoke: Not supported`，不会进入 Remote Invoke 双通道注册。

**执行记录**：
- 2026-07-07：PASS。执行 `cargo test -p bifrost-sync github_gist -- --nocapture`，确认 `github_gist` provider session 会让卡片进入 Connected 且 Remote Invoke 保持不支持。执行 `pnpm --dir web run test:ui tests/ui/admin-settings.spec.ts --grep "GitHub Gist"`，真实 Chromium 验证 GitHub Gist 登录按钮可点击、卡片和弹窗都有 `scopes=gist` 的 GitHub token 生成链接、token 弹窗展示、提交 `/sync/login` payload 为 `provider_id=github_gist` + token、成功后卡片显示 Connected 和 GitHub 用户。

#### TC-WST-41：GitHub Gist Provider 真实同步规则新增、编辑、删除和基础配置更新

**前置条件**：
1. 使用临时数据目录启动 Bifrost，并禁用 ByteDance Internal 自动登录，避免内网 session 污染 GitHub-only 场景：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DATA_DIR="$(mktemp -d)" \
   target/debug/bifrost start --port 9914 --host 127.0.0.1 --daemon --skip-cert-check --no-system-proxy --access-mode local_only --no-tray
   ```
2. 准备一个只用于测试的 GitHub token，scope 仅选择 `gist`。

**操作步骤**：
1. 调用 `POST /_bifrost/api/sync/login`，payload 包含 `provider_id=github_gist` 和测试 token。
2. 调用 `GET /_bifrost/api/sync/status`，确认 `GitHub Gist` provider 为 `connected=true`，`remote_invoke_registered=false`。
3. 通过 Rules Admin API 新增一条个人规则，例如 `codex-gist-sync-<timestamp>`，内容为 `example.com host://127.0.0.1:3000`。
4. 更新 Settings 基础配置中的三类允许同步字段：
   - `domain_allowlist`: `["gist-sync.example.com"]`
   - `app_allowlist`: `["BifrostTestApp"]`
   - `blacklist`: `["skip.gist-sync.example.com"]`
5. 触发一次 sync，读取 GitHub Gist API，确认创建或更新 `Bifrost Sync Snapshot` private gist，文件名为 `bifrost-sync-snapshot.json`。
6. 编辑同一条规则为 `example.com host://127.0.0.1:3001`，并把三类基础配置改为 `gist-sync-updated.example.com`、`BifrostTestAppUpdated`、`skip-updated.gist-sync.example.com`。
7. 再次触发 sync，确认 Gist snapshot 内规则内容和三类基础配置均更新。
8. 删除该规则并再次触发 sync，确认 Gist snapshot 内不再包含这条规则，但基础配置保留最新值。
9. 删除测试期间创建的 GitHub Gist，停止 Bifrost，并删除临时数据目录。

**预期结果**：
- GitHub Gist 连接只影响 `github_gist` provider，不会注册 Remote Invoke。
- 首次同步会创建 private gist，后续同步复用并 PATCH 同一个 snapshot gist。
- 规则新增、编辑、删除都会同步到 `bifrost-sync-snapshot.json`。
- 基础配置只同步 `app_allowlist`、`domain_allowlist`、`blacklist`，不包含 token、证书、Remote Invoke 授权、本地端口、流量历史或本机标识。
- 清理步骤删除测试 gist 后，测试 GitHub 账号不会残留 Bifrost 测试数据。

**执行记录**：
- 2026-07-07：PASS。使用用户提供的临时 GitHub token 在独立 `BIFROST_DATA_DIR`、禁用 ByteDance 自动登录的 9914 端口环境完成真实 GitHub API 验证。`/sync/status` 返回 `github_gist` connected，用户标识为 GitHub 账号；新增规则后 snapshot 包含该规则和三类基础配置；编辑规则和配置后 snapshot 更新为 `host://127.0.0.1:3001`、`gist-sync-updated.example.com`、`BifrostTestAppUpdated`、`skip-updated.gist-sync.example.com`；删除规则后 snapshot 规则列表为空且基础配置保持最新值。测试结束已删除临时测试 gist、停止 9914 端口服务并清理临时数据目录。

#### TC-WST-42：多同步服务同时连接时不互相覆盖或震荡

**操作步骤**：
1. 准备一个同时存在 Bifrost Server provider session 和 GitHub Gist provider session 的临时 `sync-state.json`。
2. 创建一条已有 server 同步元数据的规则，`remote_id=server-env-1`。
3. 执行一次 GitHub Gist mirror 同步。
4. 检查本地规则同步元数据。
5. 创建一条只带 `gist:` remote id 的规则，然后执行 Bifrost Server sync。
6. 检查 server sync 行为和本地规则同步元数据。
7. 检查基础配置 sync metadata。

**预期结果**：
- GitHub Gist mirror 只更新 private gist snapshot，不调用本地规则 `mark_synced`，不覆盖 `server-env-1`、`remote_user_id`、`remote_updated_at` 或规则 sync status。
- Bifrost Server sync 忽略 `gist:` remote id 和 Gist tombstone，不会把 `gist:` id 当作 `/v4/env/{id}` 去 PATCH/DELETE。
- 当 server 远端没有该规则时，Bifrost Server sync 会创建自己的 server env，并把本地规则切换到 server remote id。
- GitHub Gist 的基础配置 meta key 使用 `github_gist:<config_key>`，不会覆盖 server provider 使用的 `<config_key>` hash 槽位。
- 手动同步和后台自动同步的 provider 顺序一致：server sync first，GitHub Gist mirror second；没有 server session 时才允许 GitHub Gist 双向同步。

**执行记录**：
- 2026-07-07：PASS。执行 `cargo test -p bifrost-sync -- --nocapture`，其中 `github_gist_mirror_does_not_overwrite_server_rule_metadata` 验证 Gist mirror 不改 server rule metadata，`server_sync_ignores_github_gist_remote_metadata` 验证 server sync 忽略 `gist:` id 并创建自己的 server remote，`github_gist_snapshot_syncs_basic_config_updates` 验证 Gist basic-config metadata 使用 `github_gist:` 命名空间。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
