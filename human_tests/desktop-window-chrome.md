# Desktop Window Chrome 真实场景测试

## 功能模块说明

验证 Bifrost 桌面端窗口 chrome、拖拽区域、窗口控制按钮、启动方式和托管 core server 启动链路。重点覆盖 Windows 自绘 chrome 回归：交互控件不能被 Tauri drag region 遮挡，桌面快捷方式启动不能出现 console 黑框，桌面壳必须自动启动内置 core server。

## 前置条件

- Windows VM 项目路径为 `C:\Users\eden_studio\work\github\bifrost-windows-desktop-hit-test`。
- Windows VM 中已从当前分支构建 `target-desktop-hit-test\debug\bifrost-desktop.exe`。
- Windows VM 中已准备 sidecar：`desktop\src-tauri\resources\bin\bifrost.exe`。
- 启动真实桌面壳前，使用临时数据目录，避免污染用户真实配置：
  ```cmd
  set BIFROST_DATA_DIR=%TEMP%\bifrost-desktop-window-chrome
  set BIFROST_DESKTOP_BIN=C:\Users\eden_studio\work\github\bifrost-windows-desktop-hit-test\desktop\src-tauri\resources\bin\bifrost.exe
  set BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
  set BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
  ```
- 除非用例明确要求观察 GUI，否则可通过 Parallels `prlctl exec "Windows 11" --current-user ...` 执行命令。

## 测试用例列表

### TC-DWC-01 Windows 右上角窗口控制按钮可点击

操作步骤：

1. 在 Windows 用户桌面会话启动 `bifrost-desktop.exe`。
2. 鼠标移动到右上角最小化、最大化、关闭按钮，观察 hover 状态。
3. 分别点击最小化、最大化/还原、关闭按钮。

预期结果：

- hover 时按钮有可见反馈。
- 单击最小化会最小化窗口。
- 单击最大化/还原会切换窗口状态。
- 单击关闭会关闭或按产品关闭策略隐藏窗口。
- 单击按钮时不会触发窗口拖拽或双击最大化副作用。

### TC-DWC-02 Windows 左侧 tab 与底部主题切换可点击

操作步骤：

1. 启动 Windows 桌面端并进入主界面。
2. 依次点击左侧 Rules、Traffic、Settings 等 tab。
3. 点击左侧 OpenAPI 入口。
4. 点击底部亮暗主题切换按钮。

预期结果：

- 左侧 tab 每次点击都能切换到对应页面，不需要多次点击。
- OpenAPI 入口可正常响应。
- 主题切换按钮可切换亮暗主题。
- 这些控件所在区域不会拖拽窗口，也不会因为 drag region 抢占点击导致无响应。

### TC-DWC-03 Windows 顶部空白区域仍可拖拽和双击最大化

操作步骤：

1. 启动 Windows 桌面端。
2. 在顶部非交互空白区域按住鼠标拖动窗口。
3. 在同一区域双击窗口。
4. 在左侧 tab、底部主题按钮、右上角窗口按钮上重复点击，确认不会被当作拖拽区域。

预期结果：

- 顶部空白区域可以拖动窗口。
- 顶部空白区域双击可以最大化/还原窗口。
- 交互控件不会触发拖拽区域行为。

### TC-DWC-04 Windows 启动不出现 console 黑框

操作步骤：

1. 构建 Windows debug 桌面端。
2. 使用 PE header 检查桌面 exe subsystem。
3. 在 Windows 用户会话从桌面快捷方式或 `start` 命令启动桌面端。

预期结果：

- PE subsystem 为 Windows GUI。
- 启动时不出现 PowerShell、cmd 或 console 黑框。
- 桌面端窗口正常显示。

### TC-DWC-05 Windows 桌面壳自动启动 core server

操作步骤：

1. 删除或换用全新的临时 `BIFROST_DATA_DIR`。
2. 启动 Windows 桌面端。
3. 检查 `bifrost-desktop.exe` 进程存在。
4. 检查内置 `bifrost.exe` sidecar 进程存在。
5. 请求 `http://127.0.0.1:<desktop-config.json 中 proxy_port>/_bifrost/api/proxy/address`。

预期结果：

- 首次启动会自动创建 `desktop-config.json`。
- 桌面壳 setup 不会因为配置目录不存在而 panic。
- 内置 core server 自动监听配置端口。
- Admin API 返回包含代理地址的 JSON 响应。

## 清理步骤

1. 关闭 Windows 桌面端。
2. 结束本用例启动的 `bifrost-desktop.exe` 和对应临时 `BIFROST_DATA_DIR` 下的 sidecar `bifrost.exe`。
3. 删除 `%TEMP%\bifrost-desktop-window-chrome` 或本次用例使用的临时数据目录。
