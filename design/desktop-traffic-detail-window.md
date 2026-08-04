# Desktop Traffic 独立详情窗口

## 背景与问题

Traffic 页面右上角的 “Open in new window” 目前统一调用浏览器
`window.open(..., "popup")`。普通浏览器会创建 popup，但 Tauri 桌面 WebView 不会据此创建
操作系统窗口，调用返回 `null`，随后页面显示 `Failed to open detail window`。

问题只发生在窗口创建边界。详情路由 `/traffic/detail`、详情读取、浏览器 popup 与
`useTrafficDetailWindowStore` 的挂回状态已经存在，修复应复用这些能力，不复制详情页面。

## 用户目标验证清单

- 桌面端点击独立窗口按钮后创建并聚焦真正的原生详情窗口。
- 再次选择流量并打开时复用同一详情窗口，更新其记录而不是累计新窗口。
- 点击 `Attach Back` 或手动关闭原生窗口后，主窗口恢复内嵌详情。
- 普通浏览器继续使用现有 popup，关闭检测和挂回行为保持不变。
- 空 ID、超长 ID 和包含 URL 特殊字符的 ID 不得产生无效或可注入的窗口路由。

## 设计

### 桌面原生命令

`desktop/src-tauri/src/traffic_detail_window.rs` 提供两个异步命令，并由
`desktop/src-tauri/src/main.rs` 注册：

- `open_traffic_detail_window(record_id, popup_id)`：校验并编码参数，创建或复用标签为
  `traffic-detail` 的 `WebviewWindow`，加载
  `index.html#/traffic/detail?detached=1&popupId=...&id=...`，然后显示并聚焦。
- `close_traffic_detail_window()`：关闭已经存在的详情窗口；窗口不存在时幂等成功。

窗口使用稳定标签以保证同一时刻只有一个详情窗口。窗口标签加入 Tauri capability，详情页仍能
调用现有 core invoke。命令必须是异步命令，避免 Windows 上在同步 command 中创建 WebView
可能产生的事件循环死锁。

窗口被操作系统关闭（包括 macOS `Command+W`）时，Rust 在 `Destroyed` 生命周期事件中向标签为
`main` 的主 WebView 派发
`desktop://traffic-detail-closed` DOM `CustomEvent`。主页面收到事件后清除 detached 状态，恢复
内嵌详情。主动 `Attach Back` 同样先恢复 store，再调用关闭命令；关闭命令幂等，因此两个路径
发生竞态也不会留下错误状态。

原生顶层壳窗口标签是 `host`，其自身不承载 React DOM；关闭通知不得向 `host` 查找 WebView，
否则窗口虽已销毁，主页面仍会停留在 detached 状态，后续选择记录也不会展示内嵌详情。
通知失败会写入 `desktop-bootstrap.log`，但不阻止操作系统完成窗口销毁。

### Web 兼容分支

Traffic 页面根据 `isDesktopShell()` 分流：

- 桌面：先写入 detached store，再调用原生打开命令。创建失败时回滚 store，并显示原有错误提示。
- 浏览器：继续调用 `window.open`，保留 popup 引用、关闭轮询、复用和聚焦行为。

桌面窗口没有可用的浏览器 `Window` 引用，因此浏览器 popup 的 400ms 关闭轮询在桌面原生路径
必须跳过，桌面关闭由原生事件驱动。

详情页的 `Attach Back` 和 detached 状态自动关闭也按运行环境分流：桌面调用原生关闭命令，
浏览器调用 `window.close()`。浏览器 `beforeunload` 同步逻辑保持不变；桌面状态以主窗口收到的
原生销毁事件为准。

## 安全与边界

- `record_id` 和 `popup_id` 必须去除首尾空白后非空。
- `record_id` 最长 512 字符，`popup_id` 最长 128 字符；超限直接拒绝。
- 两个参数均使用 URL query 百分号编码后拼入应用内路由。
- 窗口只允许加载编译产物内的固定 Traffic 详情路径，不接受任意 URL。
- 原生窗口不存在时 close 成功返回，避免重复关闭造成用户可见错误。

## 验证方案

- Rust 单元测试：正常路由、特殊字符编码、空 ID、超长 ID。
- Web 单元测试：Tauri open/close invoke 参数；桌面打开策略不调用 `window.open`；浏览器 popup
  正常与被阻止两条路径。
- E2E：执行桌面详情窗口契约脚本，静态阻止关闭通知误投到 `host`；复跑 Traffic 浏览器 popup 的 Playwright 用例，验证新窗口、
  `Attach Back` 与关闭行为。
- human test：在真实桌面应用中打开详情窗口、切换记录复用、点击 `Attach Back`、手动关闭；在
  浏览器中复测 popup 兼容路径。
- 收尾验证依次执行 e2e-test、rust-project-validate、workspace all-features 和远端 90% 覆盖率门禁。

## 非目标

- 不改变详情内容、Traffic API、流量存储格式或主窗口 chrome。
- 不支持同一时刻打开多个 Traffic 详情窗口。
- 不将任意外部 URL 接入该命令。
