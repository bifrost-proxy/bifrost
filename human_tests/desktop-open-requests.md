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

### TC-DOR-08 桌面端 OpenURL 能打开外部链接

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
- 内部 `#/traffic/detail` 等桌面壳路由不被误当成外部 URL。
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
