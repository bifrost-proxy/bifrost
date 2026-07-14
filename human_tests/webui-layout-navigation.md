# Web UI 布局与导航测试用例

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/`

---

## 测试用例

### TC-WLN-01：侧边栏导航图标显示

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 观察页面左侧侧边栏

**预期结果**：
- 侧边栏宽度为 50px，垂直排列导航图标
- 默认显示以下导航项（从上到下）：
  - Network（GlobalOutlined 图标）— 对应 `/traffic`
  - Replay（ThunderboltOutlined 图标）— 对应 `/replay`
  - Rules（FileTextOutlined 图标）— 对应 `/rules`
  - Values（DatabaseOutlined 图标）— 对应 `/values`
  - Scripts（CodeOutlined 图标）— 对应 `/scripts`
  - DevTools（BugOutlined 图标）— 对应 `/devtools`
  - Settings（SettingOutlined 图标）— 对应 `/settings`
- 每个图标下方显示 9px 大小的文字标签
- DevTools 位于 Scripts 之后，不出现在 Network 后的高优先级位置
- 如果 Sync 功能已启用，在 DevTools 和 Settings 之间显示 Groups（UsergroupAddOutlined 图标）
- 侧边栏底部显示主题切换圆形按钮

---

### TC-WLN-02：侧边栏当前页面激活指示器

**操作步骤**：
1. 打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 观察 Network 图标的样式
3. 点击 Rules 图标

**预期结果**：
- 当前活动页面的图标高亮显示（使用 `colorPrimary` 主题色，背景为 `colorPrimaryBg`）
- 活动图标左侧显示 3px 宽的蓝色竖条指示器（borderRadius: "0 2px 2px 0"）
- 点击 Rules 后：
  - Rules 图标变为激活状态（高亮 + 左侧竖条）
  - Network 图标恢复为非激活状态（使用 `colorTextSecondary` 颜色）
  - 页面内容切换到 Rules 页面
  - URL 变为 `/_bifrost/rules`

---

### TC-WLN-03：侧边栏逐个导航验证

**操作步骤**：
1. 依次点击侧边栏所有导航图标：Network → Replay → Rules → Values → Scripts → DevTools → Settings

**预期结果**：
- 点击 Network：URL 变为 `/traffic`，显示流量列表页
- 点击 Replay：URL 变为 `/replay`，显示重放页面
- 点击 Rules：URL 变为 `/rules`，显示规则列表页
- 点击 Values：URL 变为 `/values`，显示 Values 页面
- 点击 Scripts：URL 变为 `/scripts`，显示脚本页面
- 点击 DevTools：URL 变为 `/devtools`，显示 DevTools 页面
- 点击 Settings：URL 变为 `/settings`，显示设置页面
- 每次切换都更新激活指示器到当前页面图标

---

### TC-WLN-04：三栏分割面板调整

**操作步骤**：
1. 打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 拖拽左侧 FilterPanel 和中间流量列表之间的分隔条
3. 拖拽中间流量列表和右侧详情面板之间的分隔条

**预期结果**：
- 左侧 FilterPanel 可调整宽度，最小 180px，最大 350px
- 中间流量列表区域最小宽度 400px
- 右侧详情面板最小宽度 350px
- 拖拽分隔条时面板大小实时响应
- 左侧 FilterPanel 宽度变更后刷新页面会保留设置

---

### TC-WLN-05：FilterPanel 折叠/展开

**操作步骤**：
1. 在 Traffic 页面 Toolbar 中点击左侧的 FilterOutlined 图标按钮
2. 再次点击该按钮

**预期结果**：
- 第一次点击：左侧 FilterPanel 折叠隐藏，中间列表区域扩展占满空间
- FilterOutlined 按钮颜色从 `colorPrimary` 变为 `colorTextSecondary`
- 第二次点击：左侧 FilterPanel 恢复展开
- FilterOutlined 按钮颜色恢复为 `colorPrimary`

---

### TC-WLN-06：详情面板折叠/展开

**操作步骤**：
1. 在 Traffic 页面 Toolbar 右侧点击 MenuFoldOutlined/MenuUnfoldOutlined 图标按钮
2. 再次点击该按钮

**预期结果**：
- 第一次点击：右侧详情面板折叠，图标变为 MenuUnfoldOutlined
- 第二次点击：右侧详情面板恢复展开，图标变为 MenuFoldOutlined
- Tooltip 提示文本根据状态变化："Hide detail panel" / "Show detail panel"

---

### TC-WLN-07：状态栏显示

**操作步骤**：
1. 在任意管理端页面，观察页面最底部的状态栏

**预期结果**：
- 状态栏高度 20px，位于页面最底部
- 状态栏从左到右依次显示以下信息：
  - Proxy 状态：绿色圆点 + "Proxy: Running"（或灰色圆点 + "Proxy: Stopped"）
  - Sync 状态：对应颜色圆点 + "Sync: Off/Syncing/Local/Sign in/Synced/Connected"
  - 分隔线
  - 上传速率：↑ 图标 + 速率值（如 "0 B/s"）
  - 下载速率：↓ 图标 + 速率值
  - 分隔线
  - 总流量：Total: + 流量值（如 "0 B"）
  - 分隔线
  - 活跃连接数：Conn: + 数字
  - 请求总数：Req: + 数字
  - 分隔线
  - 内存使用：Mem: + 值（如 "12.5 MB"）
  - CPU 使用率：CPU: + 百分比（如 "1.2%"）
  - 分隔线
  - 运行时长：Uptime: + 时间值（如 "5m"、"1h 30m"）
  - 右侧：版本号（如 "v0.x.x"），可点击
- 所有数值使用 monospace 字体

---

### TC-WLN-08：Toolbar 显示

**操作步骤**：
1. 在 Traffic 页面观察流量列表上方的 Toolbar 区域

**预期结果**：
- Toolbar 位于流量列表上方，水平布局
- 左侧包含：
  - FilterPanel 折叠/展开按钮（FilterOutlined 图标）
  - 分隔线
  - 清除流量下拉按钮（DeleteOutlined 图标 + DownOutlined），点击展开菜单含 "Clear all" 和 "Clear filtered (N)"
  - 分隔线
- 中间区域显示过滤标签组（Tag.CheckableTag）：
  - Rule 组：Hit Rule
  - Protocol 组：HTTP、HTTPS、WS、WSS、H3
  - Type 组：JSON、Form、XML、JS、CSS、Font、Doc、Media、SSE
  - Status 组：1xx、2xx、3xx、4xx、5xx、error
  - Imported 组：Imported
  - 各组之间有分隔线
- 右侧包含：
  - System Proxy 标签 + 开关
  - 分隔线
  - 详情面板折叠/展开按钮

---

### TC-WLN-09：深色/浅色主题切换

**操作步骤**：
1. 观察侧边栏底部的主题切换按钮
2. 如果当前是浅色主题，点击 MoonOutlined 图标
3. 如果当前是深色主题，点击 SunOutlined 图标

**预期结果**：
- 浅色主题下：按钮显示 MoonOutlined 图标，颜色 #64748b，背景 rgba(100,116,139,0.1)
- 点击后切换到深色主题：
  - 整个 UI 配色方案切换为深色（使用 Ant Design `darkAlgorithm`）
  - 按钮变为 SunOutlined 图标，颜色 #facc15，背景 rgba(250,204,21,0.12)
  - Tooltip 提示文本变为 "Switch to Light"
  - `document.documentElement` 的 `data-theme` 属性变为 "dark"
- 再次点击恢复浅色主题：
  - UI 恢复浅色配色方案（使用 Ant Design `defaultAlgorithm`）
  - 按钮恢复为 MoonOutlined
  - Tooltip 提示文本变为 "Switch to Dark"
  - `data-theme` 属性变为 "light"

---

### TC-WLN-10：响应式布局

**操作步骤**：
1. 在浏览器中打开管理端页面
2. 调整浏览器窗口宽度从 1440px 缩小到 800px
3. 观察各区域的布局变化

**预期结果**：
- 侧边栏始终保持固定宽度 50px，不随窗口宽度变化
- 状态栏始终保持在底部，高度 20px
- 内容区域（Outlet）自适应剩余空间
- Traffic 页面的三栏面板：
  - 中间区域最小宽度 400px 约束生效
  - 窗口过窄时可通过折叠左侧/右侧面板适应
- 整体布局使用 `100vh` 和 `100vw`，无外层滚动条

---

### TC-WLN-11：版本检查弹窗

**操作步骤**：
1. 在状态栏右侧点击版本号（如 "v0.x.x"）

**预期结果**：
- 弹出版本信息弹窗（VersionModal）
- 触发版本检查请求（`checkVersion({ forceRefresh: true })`）
- 弹窗中显示当前版本信息
- 如果有新版本可用：
  - 状态栏版本号左侧显示红色小圆点
  - 版本号右侧显示绿色向上箭头
  - 鼠标悬停版本号时 Tooltip 显示 "New version available: vX.X.X"
- 如果没有新版本：
  - 鼠标悬停版本号时 Tooltip 显示 "Click to view version info"

---

### TC-WLN-12：.bifrost 文件拖拽导入

**操作步骤**：
1. 准备一个 `.bifrost` 格式的文件（可通过导出功能获取）
2. 将该文件拖拽到浏览器窗口中
3. 在拖拽悬停时观察 UI 变化
4. 释放文件

**预期结果**：
- 拖拽文件进入浏览器窗口时：
  - 显示全屏遮罩层（className: `bifrost-drop-overlay`）
  - 遮罩层中央显示上传图标（UploadOutlined，48px）和文本 "释放以导入 .bifrost 文件"
- 释放文件后：
  - 遮罩层消失
  - 显示导入中 Modal（Spin 加载 + "正在导入..."）
  - 导入完成后显示 Toast 消息 "导入 {文件名} 成功"
  - 自动跳转到对应页面（规则文件跳转到 /rules，网络文件跳转到 /traffic 等）
  - 对应页面的数据自动刷新
- 拖拽非 `.bifrost` 文件时：
  - 显示警告 Toast "请拖入 .bifrost 格式的文件"

---

### TC-WLN-13：Settings 页面导航联动

**操作步骤**：
1. 在侧边栏点击 Settings 图标
2. 如果有待处理的证书认证请求，观察 Settings 图标

**预期结果**：
- Settings 图标上显示 Badge 徽标，数字为待处理请求数量
- Badge 使用 `size="small"` 和 `offset={[4, -4]}`
- 点击后进入 Settings 页面，激活指示器正确显示

---

### TC-WLN-14：默认路由重定向

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/`

**预期结果**：
- 自动重定向到 `http://127.0.0.1:8800/_bifrost/traffic`
- 侧边栏 Network 图标处于激活状态
- 显示 Traffic 流量列表页面

---

### TC-WLN-15：小窗口下左侧导航栏支持滚动

**前置条件**：使用临时数据目录启动 Bifrost，且窗口高度调整到约 360px，确保左侧导航项总高度超过可视区域。

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 观察左侧一级导航栏中 Network、Replay、Rules、Values、Scripts、DevTools、Notify、Settings 等 Tab 项
3. 在左侧导航栏内向下滚动
4. 点击滚动后露出的 `Settings` Tab
5. 点击底部主题切换按钮，切换亮色/暗色主题后重复第 3 步

**预期结果**：
- 每个一级 Tab item 都保持稳定最小高度，不因窗口变矮被压缩或文字重叠
- 当导航项总高度超过窗口高度时，左侧导航项区域出现可滚动行为
- 向下滚动后靠下的 `Notify`、`Settings` 等 Tab 可见且可点击
- 点击 `Settings` 后 URL 变为 `/_bifrost/settings`，Settings Tab 激活指示器正确显示
- 底部主题切换按钮保持在侧边栏底部，不随导航项一起滚动
- 亮色和暗色主题下滚动区域、激活背景、文字和图标均清晰可读

**本次执行记录（2026-05-09）**：
- 通过。执行 `pnpm --dir web exec playwright test admin-settings.spec.ts --grep "左侧一级导航在小窗口下可滚动"`，在 900x360 viewport 下验证导航滚动容器 `scrollHeight > clientHeight`、`overflow-y: auto`、Tab item `min-height: 64px`，滚动后可点击 `Settings` 并保持主题按钮可见。

---

### TC-WLN-16：版本更新弹窗 Upgrade Command Copy 真实写入剪贴板

**前置条件**：使用临时数据目录启动 Bifrost，端口避开 `9900`，启动参数包含 `--no-system-proxy`。测试前先清空系统剪贴板内容。

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 让版本检查接口返回有新版本，例如 current `0.0.62`、latest `0.0.73`
3. 点击状态栏右侧版本号，打开 `New Version Available` 弹窗
4. 点击 `Upgrade Command` 区域右侧的 `Copy` 按钮
5. 读取真实剪贴板内容

**预期结果**：
- 页面显示成功提示 `Command copied to clipboard`
- 剪贴板内容精确等于 `bifrost upgrade`
- 若复制失败，页面必须显示失败提示，不允许出现成功提示但剪贴板为空的假阳性
- 亮色和暗色主题下弹窗内容、Copy 按钮和提示均清晰可读

**本次执行记录（2026-05-09）**：
- 通过。先执行 `printf '' | pbcopy` 清空系统剪贴板，再执行 `pnpm --dir web exec playwright test admin-settings.spec.ts --grep "版本更新弹窗 Upgrade Command Copy 写入剪贴板" --headed`，headed Chromium 打开真实 WebUI，mock 版本检查为 `0.0.62 -> 0.0.73`，点击状态栏版本号打开更新弹窗并点击 `Copy`。
- 通过。测试结束后执行 `pbpaste`，真实系统剪贴板内容为 `bifrost upgrade`，与预期完全一致。

---

### TC-WLN-17：底部 Sync 状态栏跳转到 Settings Sync

**前置条件**：使用临时数据目录启动 Bifrost，端口避开 `9900`，启动参数包含 `--no-system-proxy`。

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 点击底部状态栏中的 `Sync: ...` 区域
3. 回到 Traffic 页后，使用键盘聚焦 `Sync: ...` 区域并按 Enter 或 Space

**预期结果**：
- 点击后 URL 变为 `/_bifrost/settings?tab=sync`
- Settings 页面中 `Sync` Tab 处于选中状态
- 状态栏 Sync 区域有 hover 反馈，并保持原有 Tooltip 状态说明
- Enter 和 Space 键触发与鼠标点击相同的跳转行为
- 亮色和暗色主题下状态栏文本、圆点和 hover 背景均清晰可读

**本次执行记录（2026-06-03）**：
- 通过。执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "底部 Sync 状态栏点击后跳转到 Settings Sync"`，真实 Chromium 打开 Traffic 页面，点击 `data-testid=statusbar-sync` 后 URL 变为 `/_bifrost/settings?tab=sync`，且 `Sync` Tab 的 `aria-selected=true`；随后回到 Traffic 页面，分别聚焦状态栏 Sync 区域并按 Enter、Space，均跳转到 `/_bifrost/settings?tab=sync`。

---

### TC-WLN-18：Safari 左侧导航标签完整显示回归

**前置条件**：

1. 执行 `pnpm --dir web run build`，再执行 `cargo build --bin bifrost`，确保最新 WebUI 已嵌入测试二进制。
2. 使用独立目录启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-webkit-human \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   target/debug/bifrost start --host 127.0.0.1 -p 18990 \
     --unsafe-ssl --no-system-proxy --skip-cert-check --access-mode allow_all
   ```
3. 使用 `lsof -nP -iTCP:18990 -sTCP:LISTEN` 和 `curl -sS http://127.0.0.1:18990/_bifrost/api/proxy/address` 确认最新进程与 Admin API ready。

**操作步骤**：

1. 在 Safari 打开 `http://127.0.0.1:18990/_bifrost/activity`。
2. 从上到下观察 `Activity`、`Network`、`Replay`、`Rules`、`Values`、`Scripts`、`AI`、`DevTools`、`Notify`、`Settings` 的图标和英文标签。
3. 检查侧栏底部 `OpenAPI` 与主题切换按钮。
4. 点击主题切换按钮进入暗色主题，再重复第 2、3 步。
5. 缩小 Safari 窗口高度直到导航列表发生纵向溢出，在侧栏内向下滚动并点击 `Settings`。

**预期结果**：

- 侧栏外宽保持 50px，内容区不因修复发生整体横向偏移。
- 每个图标和标签都完整显示；`Network`、`DevTools`、`Settings` 不缺末尾字符，也不越过右侧边框。
- 亮色和暗色主题的文字、激活背景与图标均清晰可读。
- 小窗口下导航列表仍可纵向滚动，滚动条不占用窄栏横向内容宽度。
- 点击 `Settings` 后进入 `/_bifrost/settings`，底部 OpenAPI 和主题按钮不随列表滚动。

---

### TC-WLN-19：macOS Tauri WebView 左侧导航回归

**前置条件**：

1. 执行 `pnpm --dir web run build:desktop`。
2. 执行 `cargo build --manifest-path desktop/src-tauri/Cargo.toml`。
3. 使用独立 `BIFROST_DATA_DIR`、`BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1` 和禁用系统代理的测试配置启动 `desktop/src-tauri/target/debug/bifrost-desktop`。

**操作步骤**：

1. 等待 Tauri 启动页 handoff 到主界面 Activity 页。
2. 从上到下检查所有可见一级导航图标和标签，重点检查 `Network`、`DevTools`、`Settings`。
3. 切换暗色主题后重复检查。
4. 缩小窗口高度并在导航区域向下滚动，确认 Settings 可见、可点击。
5. 在导航项附近拖动窗口并点击导航，确认本次布局修复没有破坏桌面 drag region 与点击行为。

**预期结果**：

- Tauri 系统 WebView 与 Safari 一致：所有导航图标和标签完整，不发生横向裁切。
- 侧栏保持 50px，macOS 顶部 38px window control spacer 和 35px 顶部 drag strip 不变。
- 亮色、暗色、小窗口滚动、导航点击和窗口拖动均正常。
- 不启用系统代理，不影响正式 9900 服务和默认用户数据目录。

---

### TC-WLN-20：Chromium 侧栏对照回归

**前置条件**：复用 TC-WLN-18 的独立 18990 服务。

**操作步骤**：

1. 在 Chrome/Chromium 打开 `http://127.0.0.1:18990/_bifrost/activity`。
2. 检查所有一级导航图标、标签、OpenAPI 与主题按钮的横向边界。
3. 切换暗色主题重复检查。
4. 缩小窗口高度，滚动到 Settings 并点击。

**预期结果**：

- Chromium 中导航布局与修复前保持一致，没有因 WebKit 兼容修复产生横向偏移或文字裁切。
- 亮色、暗色、小窗口纵向滚动和 Settings 导航均通过。
- Playwright Chromium 几何断言与真实 Chrome 观察结果一致。

### 2026-07-14 WebKit 侧栏兼容修复执行记录

| 用例 | 执行命令 / 真实证据 | 实际结果 |
| --- | --- | --- |
| TC-WLN-18 | `BIFROST_DATA_DIR=./.bifrost-webkit-human ... target/debug/bifrost start -p 18990 --no-system-proxy`；Safari 真实打开 `http://127.0.0.1:18990/_bifrost/activity`，使用辅助功能树与截图逐项检查亮色、暗色；将窗口缩小后点击视口外的 `Settings`。 | 通过。所有一级导航标签、OpenAPI 和主题按钮完整显示；小窗口中导航列表自动滚动并进入 `/_bifrost/settings`；18990 测试实例已停止。 |
| TC-WLN-19 | `pnpm run desktop:dev` 验证标准 dev 启动链；随后执行 `pnpm exec tauri build --debug --bundles app --no-sign --config desktop/src-tauri/tauri.conf.json` 生成可被 macOS 辅助功能识别的本地 debug `.app`。以 `BIFROST_DATA_DIR=./.bifrost-webkit-desktop`、`proxy_port=18991`、`BIFROST_DESKTOP_NO_SYSTEM_PROXY=1`、`BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1` 启动；Computer Use 实际检查亮色、暗色、缩小窗口、Settings 点击和顶部 drag region。 | 通过。WebView URL 为 `tauri://localhost#/activity` / `#/settings`，System Proxy 卡片显示 `http://127.0.0.1:18991`；所有导航标签完整；缩小窗口后 Settings 可滚动并点击；拖动窗口后 Activity 仍可点击。debug App 与 18991 sidecar 已退出，正式 9900 监听未被停止或改写。 |
| TC-WLN-20 | 真实 Chrome 打开 18990；DOM 几何实测侧栏外宽 50px、内部可用宽度 49px、`scrollbar-gutter: auto`，逐项断言所有标签左右边界均在侧栏内；切换暗色，将视口设为 900×360，滚动到底并点击 Settings，最后重置视口并关闭测试标签页。 | 通过。亮色、暗色和小窗口下均无横向裁切；小窗口 `scrollHeight=704`、`clientHeight=266`、`scrollTop=438`，点击后进入 `/_bifrost/settings`。 |

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test .bifrost-webkit-human .bifrost-webkit-desktop
```
