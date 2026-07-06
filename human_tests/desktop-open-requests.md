# Desktop Open Requests

## 功能模块说明

验证 tray 的 `Open Traffic` / `Open Rules` / `Open Settings` 优先打开已安装桌面端 App，并通过 `bifrost://` 跳转到对应页面；未安装 App 或协议不可用时回退到 Web UI。验证桌面端严格单实例、`bifrost://` 协议注册，以及 `.bifrost` 文件关联打开后复用导入逻辑，支持抓包查看和 rules 导入。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 执行命令前先准备 debug sidecar：
  ```bash
  cargo build -p bifrost-cli
  node scripts/prepare-tauri-sidecar.mjs debug
  ```
- 桌面编译验证需要先构建桌面 Web 产物：
  ```bash
  pnpm --dir web run build:desktop
  ```
- 涉及真实桌面 GUI 的用例需要在 macOS 或 Windows 桌面会话中执行。
- 所有服务启动测试必须使用临时 `BIFROST_DATA_DIR`，并设置：
  ```bash
  export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
  export BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
  export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
  ```

## 测试用例列表

### TC-DOR-01 tray 菜单优先打开桌面端路由

操作步骤：

1. 启动 Bifrost，使 tray 处于 Running 状态。
2. 点击 tray 菜单中的 `Open Traffic`。
3. 点击 tray 菜单中的 `Open Rules`。
4. 点击 tray 菜单中的 `Open Settings`。
5. 查看桌面端主窗口和当前路由。

预期结果：

- 三个菜单项先尝试打开 `bifrost://open/traffic`、`bifrost://open/rules`、`bifrost://open/settings`。
- 已安装桌面端时不会打开浏览器 Web UI，而是恢复现有桌面端主窗口并跳到对应页面。
- 点击不会启动第二个桌面端后端进程。

### TC-DOR-02 未安装桌面端时回退 Web UI

操作步骤：

1. 在没有安装或临时移除桌面端协议注册的环境中启动 Bifrost tray。
2. 点击 `Open Traffic`。
3. 点击 `Open Rules`。
4. 点击 `Open Settings`。

预期结果：

- OS opener 无法解析 `bifrost://` 时，tray 自动打开当前 Admin Web UI fallback URL。
- fallback URL 分别为 `/_bifrost/traffic`、`/_bifrost/rules`、`/_bifrost/settings`。
- tray 不报错退出，服务继续运行。

### TC-DOR-03 bifrost:// 协议跳转复用单实例

操作步骤：

1. 启动桌面端 App。
2. 再执行：
   ```bash
   open 'bifrost://open/traffic'
   open 'bifrost://open/rules'
   open 'bifrost://open/settings'
   ```
3. 检查进程数量和桌面端窗口路由。

预期结果：

- 只有一个 `bifrost-desktop` 主实例。
- 后续 `open bifrost://...` 请求被转发给已有实例。
- 桌面端窗口被恢复到前台，并切换到对应页面。

### TC-DOR-04 打开抓包 .bifrost 文件进入 Traffic

操作步骤：

1. 准备一个类型为 `network` 的 `.bifrost` 文件。
2. 双击文件，或执行：
   ```bash
   open /path/to/capture.bifrost
   ```
3. 查看桌面端当前页面和导入结果。

预期结果：

- OS 将 `.bifrost` 文件交给 Bifrost 桌面端。
- 已运行桌面端时请求被转发给单实例；未运行时启动桌面端后处理文件。
- 文件按现有 Bifrost import parser 解析，导入成功后跳转到 Traffic 页面。

### TC-DOR-05 打开 rules .bifrost 文件进入 Rules

操作步骤：

1. 准备一个类型为 `rules` 的 `.bifrost` 文件。
2. 双击文件，或执行：
   ```bash
   open /path/to/rules.bifrost
   ```
3. 查看桌面端当前页面和规则列表。

预期结果：

- 文件按现有 Bifrost import parser 解析。
- Rules store 刷新，页面跳转到 Rules。
- 已运行桌面端时不创建第二个实例。

### TC-DOR-06 契约级自动化回归

操作步骤：

1. 执行：
   ```bash
   bash e2e-tests/tests/test_desktop_open_requests_contract.sh
   ```
2. 查看脚本输出。

预期结果：

- Tauri config 注册 `bifrost://` 和 `.bifrost`。
- desktop Cargo 依赖包含 `tauri-plugin-deep-link` 和 `tauri-plugin-single-instance`。
- tray 菜单动作使用 `OpenAppRoute`，并保留 Web UI fallback。
- focused Rust tests 通过。
- Linux runner 缺少 Tauri GTK/GObject 开发依赖时，仅跳过 desktop crate `open_requests` Rust test；静态契约检查仍执行，macOS 或依赖完整的 Linux 环境仍执行该 Rust test。

### TC-DOR-07 .bifrost 导入导出带 Admin CSRF

操作步骤：

1. 在桌面端或 Web UI 中打开 Traffic 页面，选中一条请求，点击 `Export as .bifrost`。
2. 在 Rules 页面选择或创建一条规则，执行 rules `.bifrost` 导出。
3. 将导出的 network `.bifrost` 和 rules `.bifrost` 文件重新拖入页面或通过文件关联打开。
4. 查看浏览器 DevTools Network 或桌面端控制台错误。

预期结果：

- `.bifrost` 导出 POST 请求带 `X-Bifrost-CSRF`，不会返回 `403 Missing or invalid admin CSRF token`。
- `.bifrost` 导入 POST 请求带 `X-Bifrost-CSRF`，network 导入后跳转 Traffic，rules 导入后跳转 Rules。
- 同类 `.bifrost-file` detect/import/export 接口统一使用 CSRF-aware API client。

### TC-DOR-08 .bifrost 导入前预览并确认

操作步骤：

1. 准备一个类型为 `rules` 的 `.bifrost` 文件，通过拖入页面、文件选择器或双击文件打开。
2. 查看弹出的导入预览窗口。
3. 点击 `Cancel`，确认不会导入规则；再次打开同一文件后点击 `Import`。
4. 准备一个包含多条请求的 `network` `.bifrost` 文件，通过拖入页面、文件选择器或双击文件打开。
5. 查看弹出的导入预览窗口，然后点击 `Import`。
6. 准备一个只包含一条请求的 `network` `.bifrost` 文件，通过拖入页面、文件选择器或双击文件打开。
7. 查看弹出的导入预览窗口，然后点击 `Import`。

预期结果：

- rules 文件导入前展示规则名称、启用状态、描述、有效行数和规则详情，点击确认后才导入并跳转 Rules。
- 多请求 network 文件导入前展示请求数量、域名标签和请求列表，点击确认后才导入并跳转 Traffic。
- 单请求 network 文件导入前直接使用 Network 详情组件预览请求/响应详情，点击确认后才导入并跳转 Traffic。
- 拖拽导入、文件选择器导入和 OS 文件关联打开都复用同一套预览确认流程。
- 点击取消不会调用 `/api/bifrost-file/import`，不会改写当前 Rules 或 Network 数据。

### TC-DOR-09 桌面端 OpenURL 能打开外部链接

操作步骤：

1. 启动桌面端 App。
2. 在桌面端点击 `Open Docs`、OpenAPI、Availability Check 链接、Apple Configurator App Store 链接等会打开新窗口或外部页面的入口。
3. 在桌面端控制台或系统默认浏览器中观察打开结果。
4. 执行：
   ```bash
   open 'bifrost://open/settings'
   ```

预期结果：

- 桌面壳拦截外部 `http(s)`、`mailto`、`macappstore` 和 `bifrost://` URL，并通过原生 OpenURL 打开。
- `/_bifrost/...`、`/api/...`、`/public/...` 这类后端相对路径会被解析到当前桌面后端端口后再打开，不停留在 Tauri WebView 的静态资源 origin。
- 内部 `#/traffic/detail`、`#certificate-*`、`/#/...` 等桌面壳路由不被误当成外部 URL。
- `bifrost://open/settings` 仍复用单实例并跳转 Settings。

## 清理步骤

- 关闭桌面端 App。
- 停止测试用 Bifrost 服务。
- 删除临时 `BIFROST_DATA_DIR`。
- 删除测试用 `.bifrost` 文件。

## 执行记录

| 日期 | 用例 | 实际结果 |
| --- | --- | --- |
| 2026-07-06 | TC-DOR-06 | PASS：`bash e2e-tests/tests/test_desktop_open_requests_contract.sh` 通过，覆盖协议/文件关联配置、single-instance/deep-link 依赖、tray app route fallback 契约、desktop open request parser。 |
| 2026-07-06 | TC-DOR-01 / TC-DOR-03 / TC-DOR-04 / TC-DOR-05 的核心代码路径 | PASS：`cargo test -p bifrost-cli tray::` 通过，`cargo test --manifest-path desktop/src-tauri/Cargo.toml open_requests` 通过，`pnpm --dir web run build:desktop` 通过，`pnpm --dir web test:unit src/api/bifrost-file.test.ts` 通过。真实已安装 App 的 LaunchServices/文件双击操作仍需发布包安装环境复测。 |
| 2026-07-06 | TC-DOR-07 | PASS：`pnpm --dir web test:unit src/api/bifrost-file.test.ts` 覆盖 import/export POST 请求带 `X-Bifrost-CSRF`；`rg` 扫描确认前端直接 `axios.post/put/delete/patch` 只剩全局 CSRF-aware client 和错误格式化使用。 |
| 2026-07-06 | TC-DOR-08 | PASS：`pnpm --dir web test:unit src/api/bifrost-file.test.ts` 覆盖 preview POST 请求带 `X-Bifrost-CSRF`；`pnpm --dir web exec eslint ...` 与 `pnpm --dir web run build:desktop` 覆盖 rules、多请求 network、单请求 network 预览确认 UI 编译通过；`cargo test -p bifrost-admin bifrost_file --lib` 覆盖 preview 后端解析与单请求详情数据。 |
| 2026-07-06 | TC-DOR-09 | PASS：`pnpm --dir web test:unit src/desktop/openTarget.test.ts` 通过，覆盖同源 hash 与 `/#/...` 不外跳、后端相对路径转桌面后端 URL、`https`/`mailto`/`macappstore`/`bifrost` 外部 scheme 原生打开，以及 custom protocol 下 `mailto`/`bifrost` 不被 `origin=null` 误拦截。 |
| 2026-07-06 | TC-DOR-06 Linux CI 依赖边界回归 | PASS：`bash e2e-tests/tests/test_desktop_open_requests_contract.sh` 在 macOS 本机执行完整 desktop `open_requests` Rust test；脚本在 Linux 且缺少 `gobject-2.0` 时只跳过该 Tauri desktop test，避免 E2E Shell runner 因系统 GUI 开发依赖缺失误失败。 |
