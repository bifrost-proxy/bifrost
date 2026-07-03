# Remote Invoke Computer Use API 设计方案

> **状态**：RFC / planned — not yet shipped as of 2026-06-16。
> **关联**：`design/remote-invoke-file-api.md`、`design/remote-invoke-shell-e2e-regressions.md`、`design/grant-file-access.md`。

## 背景

Bifrost Remote Invoke 目前覆盖三类能力：

1. **只读查询** (`query.readonly`)：`status`、`search.stream`、`traffic.list`、`traffic.get`；
2. **Shell 控制** (`shell.exec`)：在 Shell Access policy 下执行命令；
3. **文件操作** (`file.*`)：读写、编辑、搜索、apply_patch、outline、read_many、scratch-dir 等。

代码校验（2026-06-16）：

- `crates/bifrost-admin/src/remote_invoke/types.rs` 中 `GrantScope` 当前枚举值为 `RemoteQuery` / `RemoteShellExec` / `RemoteShellInteractive` / `RemotePowerMgmt` / `RemoteImGateway`，**未包含** `RemoteComputerRead` / `RemoteComputerWrite`。
- 同文件 `CommandKind` 当前枚举值为 `QueryReadonly` / `ShellExec` / `File` / `PowerMgmt` / `ImGateway`，**未包含** `Computer` 变体。
- `crates/bifrost-cli` 不存在 `bifrost remote computer *` 子命令；无 `computer.*` 方法分发。
- 不存在 `bifrost-screen-agent` 二进制、`computer_ops` 模块、`ComputerAccessPolicy` 类型或 `computer_access.toml` 配置。

因此本文档所有 method / scope / policy / CLI / Phase 路径仍属设计预案，落地时应按当前 enum 变体做增量改动，并同步更新 `design/remote-invoke-file-api.md` scope 矩阵。

Agent 在远端设备上遇到 GUI 场景时完全无能为力：无法截图、无法看到屏幕内容、无法点击按钮或输入文本。典型痛点：

- 验证 UI 渲染（Agent 改前端代码后看不到实际效果）；
- 操作无 CLI 的应用（GUI-only 工具）；
- 填写网页表单，不依赖 Selenium/Puppeteer；
- 处理确认对话框、崩溃弹窗。

Claude Computer Use 已证明此能力对 Agent 的价值：模型通过"截图→推理→操作→再截图验证"循环，可以像人一样使用计算机。本方案在 Bifrost Remote Invoke 体系中新增 **Computer Use API** 作为第四类 remote 能力，复用现有 relay + encrypted channel + grant 授权体系。

## 用户目标验证清单

### 必须实现

- 新增 `CommandKind::Computer` 变体；`file.*` 与 `shell.*` 路径不受影响。
- 新增 `GrantScope::RemoteComputerRead` / `RemoteComputerWrite`，与 `RemoteFileRead/Write`、`RemoteShellExec/Interactive` 保持正交。
- 新增 `ComputerAccessPolicy`（类似 `FileAccessPolicy`）与 `computer_access.toml` 存储，支持 allowed_apps / denied_apps / no_control_apps / 分辨率上限 / 频率上限 / 用户确认 / 信任时长。
- Target 端引入常驻 `bifrost-screen-agent` 进程（GUI session 上下文），提供截图与输入。
- CLI 引入 `bifrost remote computer <subcmd>` 全套子命令。
- 所有写操作（click/type/key_press/scroll/drag）默认需要用户逐次确认，可通过 policy 打开信任窗口。
- 敏感应用（1Password、Keychain Access、SecurityAgent 等）默认 denied，截图排除。
- Retina 坐标系：截图返回 `scale_factor`，输入统一用逻辑坐标，CLI 自动处理转换。

### 必须不破坏

- 现有 `file.*` / `shell.exec` / `query.readonly` 权限模型和 policy 存储不受影响。
- Relay 侧不引入对 computer 命令的语义感知（保持端到端加密透传）。
- Grant 授权流程增量支持 `remote_computer_read/write`，不改动 pairing / QR 流程本身。

### 必须真实验证

- 权限缺失（Screen Recording / Accessibility）在启动时明确报错并引导。
- 敏感应用被排除后截图确实不包含窗口内容。
- 频率限制触发后返回 `computer.rate_limited`。
- Retina 显示器上点击坐标与截图坐标对齐。
- E2E：`bifrost-screen-agent` 崩溃后 admin 端能拒绝新 computer 请求而不是挂起。

## 产品语义

### 权限分层（三级）

| 级别 | 能力 | 授权方式 |
|------|------|----------|
| 只读 | screenshot / a11y-tree / active-window / wait_for_change | grant scope `remote_computer_read` + macOS Screen Recording 权限 |
| 操作 | click / type / key_press / scroll / drag | grant scope `remote_computer_write` + macOS Accessibility 权限 + 二次确认 |
| 危险 | 系统偏好设置 / 终端 / 密码管理器 | 默认 denied_apps 拒绝；需显式 allowed_apps 放行 |

### 与 `file.*` / `shell.exec` 的分工原则

- 能用 `file.*` 解决的（读写文件、edit、apply_patch），不用 `computer.*`。
- 能用 `shell.exec` 解决的（`git`、`cargo build`、执行脚本），不用 `computer.*`。
- Computer Use 是**最后手段**，适用于只有 GUI 才能触达的操作（UI 渲染验证、GUI-only 应用、浏览器表单）。

### 敏感应用保护策略

截图遇到 `denied_apps`：

- 优先使用 `CGWindowListCreateImage` 的 `exclude_windows` 参数硬排除；
- 若窗口枚举失败，退化为在截图后用黑色矩形遮挡；
- 输入操作遇到 `no_control_apps` 返回 `computer.app_no_control`（截图仍允许）。

## 技术细节（planned）

### 能力矩阵

| method | 语义 | 必需 scope |
|--------|------|------------|
| `computer.screenshot` | 截取屏幕/窗口图像 | `remote_computer_read` |
| `computer.mouse_move` | 移动鼠标到指定坐标 | `remote_computer_write` |
| `computer.click` | 点击鼠标（左/右/中键，单/双击） | `remote_computer_write` |
| `computer.type` | 输入文本 | `remote_computer_write` |
| `computer.key_press` | 按键/组合键 | `remote_computer_write` |
| `computer.scroll` | 滚动 | `remote_computer_write` |
| `computer.drag` | 拖拽 | `remote_computer_write` |
| `computer.get_accessibility_tree` | 获取 UI 无障碍树（结构化） | `remote_computer_read` |
| `computer.get_active_window` | 获取当前活跃窗口信息 | `remote_computer_read` |
| `computer.wait_for_change` | 等待屏幕变化（减少轮询） | `remote_computer_read` |

### 架构分层

```
┌──────────────┐   computer.screenshot/click/type   ┌────────────┐
│ Caller (CLI) │ ─────────────────────────────────▶ │   Relay    │
└──────────────┘                                    └─────┬──────┘
                                                          ▼
                                                ┌──────────────────┐
                                                │ Target Client    │
                                                │ bifrost-admin    │
                                                │  executor.rs     │
                                                └────────┬─────────┘
                                                         ▼
                                                ┌──────────────────┐
                                                │ ComputerAccess   │
                                                │ Policy Guard     │
                                                └────────┬─────────┘
                                                         ▼
                                          ┌──────────────┴──────────────┐
                                          ▼                             ▼
                                  ┌──────────────┐            ┌──────────────┐
                                  │ Screenshot   │            │ Input        │
                                  │ Engine       │            │ Engine       │
                                  │(ScreenCapture│            │(CGEvent /    │
                                  │ Kit / CGWin) │            │ Accessibility│
                                  └──────────────┘            └──────────────┘
                                         via bifrost-screen-agent (GUI session)
```

- Caller 端新增 `bifrost remote computer <subcmd>` 打包请求走 relay。
- Relay 仅路由，不理解 computer API 内容。
- Target 端 `bifrost-admin` 新增 `remote_invoke::computer_ops` 模块：（a）依据 grant 拒绝无权请求；（b）依据 `ComputerAccessPolicy` 做应用/操作类型细粒度控制；（c）通过 admin HTTP 上的一个 `screen-agent` endpoint 转发到 `bifrost-screen-agent` 常驻进程。

### 常驻 `bifrost-screen-agent`

**问题**：`screencapture` / `CGEvent` 从 remote shell (`launchd` daemon session) 无法访问 GUI session。

**方案**：`bifrost start` 时通过 `launchctl asuser $(id -u)` 或 `LaunchAgent` 方式在用户 GUI session 上下文启动 `bifrost-screen-agent` 常驻进程，监听 admin HTTP 端口上的 IPC。admin executor 把 screenshot / input 请求转发到此进程。

- macOS：优先 `ScreenCaptureKit`（macOS 12.3+）；降级 `CGWindowListCreateImage`；最后降级 `screencapture` CLI。
- Linux：`xdotool` + PipeWire / X11 `import`。

启动时检测 Screen Recording / Accessibility 权限；缺失时明确报错并引导用户开启。

### 授权模型（新增）

```rust
// crates/bifrost-admin/src/remote_invoke/types.rs（planned diff）

pub enum GrantScope {
    // ... existing ...
    RemoteComputerRead,   // 新增
    RemoteComputerWrite,  // 新增
}

pub enum CommandKind {
    QueryReadonly, ShellExec, File, PowerMgmt, ImGateway,
    Computer,             // 新增
}

pub fn scope_allows_command(grant_scope, file_access, kind) -> bool {
    match kind {
        CommandKind::Computer => matches!(grant_scope,
            GrantScope::RemoteComputerRead | GrantScope::RemoteComputerWrite),
        // ... existing arms ...
    }
}

pub fn computer_scope_allows_method(scope: GrantScope, method: &str) -> bool {
    let read_only = matches!(method,
        "computer.screenshot" | "computer.get_accessibility_tree" |
        "computer.get_active_window" | "computer.wait_for_change");
    match scope {
        GrantScope::RemoteComputerRead => read_only,
        GrantScope::RemoteComputerWrite => true,
        _ => false,
    }
}
```

### `ComputerAccessPolicy`（新增）

```toml
# crates/bifrost-admin/src/remote_invoke/computer_access.toml（planned）

[[computer_access_policy]]
id = "desktop-readonly"
name = "Desktop Read-only"
allowed_displays = []       # 空 = 全部
allowed_apps = []           # 空 = 全部（按 bundle id 白名单）
denied_apps = [             # 优先级高于 allowed_apps
    "com.1password.1password",
    "com.apple.keychainaccess",
    "com.apple.SecurityAgent",
]
no_control_apps = [
    "com.apple.systempreferences",
]
capture_other_users = false
ops = ["screenshot", "get_accessibility_tree", "get_active_window", "wait_for_change"]
max_screenshot_width = 1920
max_screenshot_height = 1080
screenshot_format = "png"
screenshot_quality = 80
max_keypress_per_minute = 120
max_clicks_per_minute = 60
require_confirmation = true
trust_duration_secs = 0     # 0 = 每次都确认
```

### 输入引擎（macOS，CGEvent）

- `mouse_move(x, y)`：`CGEventSource::new(CombinedSessionState)` + `CGEvent::new_mouse_event(MouseMoved)` + `post(HID)`。
- `click(x, y, button, count)`：先 `mouse_move`；按 count 循环发 `*MouseDown` + `*MouseUp`，间隔 50ms。
- `type_text(text, clear)`：可选 Cmd+A 全选；逐字符发 UniChar 事件。
- `key_press(keycode, modifiers)`：设置 `CGEventFlags`（Command / Alt / Control / Shift）；发 down + up。
- `scroll(x, y, dir, amount)`：`CGEvent::new_scroll_wheel_event(Pixel, ±amount, 0)`。
- `drag(from, to, duration_ms)`：MouseDown → 循环 MouseDragged 插值移动 → MouseUp。

### Accessibility Tree

比纯截图更可靠的 UI 感知：`AXUIElementCreateApplication(pid)` / `AXUIElementCreateSystemWide()` → 递归 `kAXChildrenAttribute`，返回结构化 `{role, title, value, position, size, enabled, focused, children}`。Agent 可以按 `role="AXButton" AND title="Save"` 精确定位，不必依赖视觉模型。

### 错误码（前缀 `computer.`）

| code | 触发场景 |
|------|----------|
| `computer.permission_denied` | grant scope 不允许 / policy 拒绝 |
| `computer.scope_required` | grant_scope 未包含 `remote_computer_read/write` |
| `computer.screen_recording_disabled` | macOS Screen Recording 权限未开 |
| `computer.accessibility_disabled` | macOS Accessibility 权限未开 |
| `computer.app_denied` | 目标应用在 `denied_apps` |
| `computer.app_no_control` | 目标应用在 `no_control_apps`（截图允许，操作拒绝） |
| `computer.rate_limited` | 超过 `max_keypress_per_minute` / `max_clicks_per_minute` |
| `computer.confirmation_required` | `require_confirmation=true` 且用户未确认 |
| `computer.confirmation_timeout` | 用户确认超时 |
| `computer.screenshot_failed` | 截图失败（如无显示器） |
| `computer.input_failed` | CGEvent 被 SIP 阻止等 |
| `computer.no_active_display` | 无活跃显示器 |
| `computer.invalid_coordinates` | 坐标超出屏幕范围 |
| `computer.io_error` | 底层系统错误 |

## CLI / Web / Admin API 表面（planned）

### CLI

```
bifrost remote computer screenshot   [--display N] [--region x,y,w,h] [--format png|jpeg] [--scale 0.5] [--save PATH]
bifrost remote computer click        <x> <y> [--button left|right|middle] [--count 1|2]
bifrost remote computer type         <text> [--clear]
bifrost remote computer key-press    <key> [--modifiers cmd,shift] [--keycode N]
bifrost remote computer scroll       <x> <y> --direction up|down --amount N
bifrost remote computer drag         <fx,fy> <tx,ty> [--duration-ms 500]
bifrost remote computer a11y-tree    [--pid N] [--depth 3] [--filter-role AXButton]
bifrost remote computer active-window
bifrost remote computer wait         [--region x,y,w,h] [--timeout-ms 5000]
```

所有命令支持 `--output human|json`。

### Web UI

Settings → Remote Invoke → Computer Access tab：
- policy 列表 CRUD；
- 授权确认弹窗（首次 write 操作时）；
- 信任窗口倒计时；
- 敏感应用列表可视化。

### Admin API

- `POST /_bifrost/api/computer-access-policy` / `GET` / `PUT` / `DELETE`：policy 管理。
- `POST /_bifrost/api/computer/confirmations/{token}`：用户确认写操作。
- `GET /_bifrost/api/computer/status`：报告 `bifrost-screen-agent` 存活、Screen Recording / Accessibility 权限状态。

### Relay 侧协议

复用 `RemoteInvokeRequest` 包络：

```jsonc
{
  "kind": "computer",
  "command": "computer.screenshot",
  "args_json": "{…}",
  "grant_scope": "remote_computer_read"
}
```

## Sync 边界

- `ComputerAccessPolicy` 不参与 Bifrost Sync（多设备物理硬件差异大，同步策略易造成误操作）。
- 屏幕截图内容永不落盘，只在响应中通过 relay 加密传输。
- 审计日志只记录"点击/输入了哪个 app 的哪个坐标"，不含截图内容。

## Phase 1-4 实施路径

### Phase 1：只读（截图 + UI 信息）

- 新增 `bifrost-screen-agent` 二进制（macOS: ScreenCaptureKit / CGWindowListCreateImage）。
- 新增 `computer.screenshot` / `get_active_window` / `get_accessibility_tree` / `wait_for_change`。
- 新增 `ComputerAccessPolicy` + `computer_access.toml`。
- 新增 `GrantScope::RemoteComputerRead` + `CommandKind::Computer`。
- 新增 CLI 只读子命令。
- 启动时权限检测 + 引导。
- 预计新增 ~800 行 Rust。

### Phase 2：输入操作

- Input Engine（macOS: CGEvent；Linux: xdotool）。
- 新增 `computer.click` / `type` / `key_press` / `scroll` / `drag`。
- 审批流 + 频率限制 + 信任窗口。
- 新增 `GrantScope::RemoteComputerWrite`。
- 预计新增 ~1200 行 Rust。

### Phase 3：高级能力

- Accessibility Tree 深度集成（按元素定位而非坐标）。
- 窗口管理（`computer.list_windows` / `focus_window` / `resize_window`）。
- 多显示器完整支持。
- 录屏回放（debug 用，可选）。
- 预计新增 ~600 行 Rust。

### Phase 4：文档 + E2E + Human tests

- `human_tests/remote-invoke-computer.md`（新建）。
- `e2e-tests/tests/test_remote_computer_e2e.sh`（新建）。
- 更新 `design/remote-invoke-file-api.md` scope 矩阵，加入 `remote_computer_read/write`。

## 测试方案

### 单元测试

- `computer_scope_allows_method` 只读 scope 拒绝写方法。
- `ComputerAccessPolicy::check` denied_apps 优先于 allowed_apps。
- `ComputerAccessPolicy::check` no_control_apps 允许截图、拒绝操作。
- 频率限制窗口按分钟重置。
- `bifrost-screen-agent` 权限缺失 → 明确错误码。
- Retina 坐标转换（逻辑坐标 → 物理像素 → 逻辑坐标）。

### E2E 测试

- `e2e-tests/tests/test_remote_computer_e2e.sh`（新建）：
  - 临时 data dir、动态 admin 端口、mock relay；
  - 启动 `bifrost-screen-agent`；
  - `bifrost remote computer screenshot --save /tmp/shot.png` → 断言 PNG magic bytes；
  - `bifrost remote computer active-window` → 断言返回 JSON 含 `pid` / `bundle_id`；
  - `bifrost remote computer click 100 100`（模拟环境）→ 断言 audit log 记录 `action=computer.click`；
  - 触发 `denied_apps` → 断言返回 `computer.app_denied`。

### Human tests（新建 `human_tests/remote-invoke-computer.md`）

- `TC-CU-01`：Screen Recording 权限缺失时启动 → 明确报错引导。
- `TC-CU-02`：截图排除 1Password 窗口。
- `TC-CU-03`：Retina 显示器截图 `scale_factor=2.0` + 点击坐标对齐。
- `TC-CU-04`：`require_confirmation=true` 时首个 click 弹出系统通知。
- `TC-CU-05`：信任窗口 30 分钟内跳过确认，过期后重新弹窗。
- `TC-CU-06`：频率限制超限返回 `computer.rate_limited`。
- `TC-CU-07`：`no_control_apps` 允许截图、拒绝点击。
- `TC-CU-08`：Accessibility Tree 返回结构化 UI 树，按 `role="AXButton"` 过滤有效。
- `TC-CU-09`：`bifrost-screen-agent` 崩溃后 computer 请求 fail-fast，不挂起。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：新枚举变体是否覆盖所有 method；policy 是否覆盖敏感应用、频率、确认三层；`bifrost-screen-agent` 是否真的在 GUI session 上下文运行。
- 复核 diff：`scope_allows_command` 是否被所有 handler 调用；relay 是否仍然只是路由不感知内容。
- 重点 review：
  - Retina 坐标转换是否在 CLI 层集中处理（避免每个 caller 自己写）；
  - `denied_apps` 排除是否用 CGWindowListCreateImage 硬排除而非事后遮挡；
  - 审计日志是否只记 metadata、不含截图内容。
- 复测：所有单元测试 + `test_remote_computer_e2e.sh` + `TC-CU-01 ~ 05`。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 检查 `git status --short` / `git diff`。
- 重点 review：
  - 频率限制在多个 grant 并行时是否隔离；
  - 信任窗口的时间基准（wall clock vs monotonic）；
  - `computer.wait_for_change` 的 CPU 占用是否合理。
- 复测：Human tests `TC-CU-01 ~ 09` 全部复跑。

## 风险与决策

- **macOS Screen Recording 权限**：截图完全不可用；启动时检测并引导用户开启。
- **macOS Accessibility 权限**：输入操作不可用；同上。
- **GPU 加速窗口黑屏**：Chrome / 游戏等窗口可能截不到；降级 ScreenCaptureKit 或提示用户关闭硬件加速。
- **CGEvent 被 SIP 阻止**：某些系统弹窗无法点击；文档说明限制，引导用户使用辅助功能权限。
- **Retina 坐标差异**：截图返回 `scale_factor`，CLI 层集中转换，Agent 无需关心。
- **relay 带宽**：截图较大；支持 JPEG 压缩 + `scale` 缩放 + `region` 裁剪。
- **Agent 无限循环**：频率限制 + 每步计数上限 + 用户可中断。
- **隐私泄露**：`denied_apps` 排除敏感窗口 + 审计日志。
- **常驻 `bifrost-screen-agent` 崩溃恢复**：admin executor 检测存活；崩溃时 fail-fast 而非挂起；`launchctl` 自动重启。

## 与 Claude Computer Use 的对比

| 维度 | Claude Computer Use | Bifrost Computer Use（本方案） |
|------|--------------------|------------------------------|
| 截图 | 支持 | 支持（ScreenCaptureKit） |
| 鼠标点击 | 支持 | 支持（CGEvent） |
| 键盘输入 | 支持 | 支持（CGEvent） |
| 拖拽 | 有限 | 支持（可调 duration） |
| Accessibility Tree | 不支持 | **支持**（结构化 UI 感知） |
| 等待屏幕变化 | 不支持（需轮询截图） | **支持**（`wait_for_change`） |
| 敏感应用保护 | 全局开关 | **细粒度**（per-app deny/allow） |
| 操作审批 | 全局开关 | **per-action 确认 + 信任窗口** |
| 频率限制 | 无 | **支持** |
| 跨平台 | Docker 容器 | **原生**（macOS + Linux） |
| 加密传输 | HTTPS | **E2E 加密**（relay） |
| 审计 | 无 | **支持** |

## 参考

- [Claude Computer Use Tool](https://docs.anthropic.com/en/docs/build-with-claude/computer-use)
- [macOS ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit)
- [macOS Accessibility Programming Guide](https://developer.apple.com/library/archive/documentation/Accessibility/Conceptual/AccessibilityMacOSX/)
- [Fazm MCP Server](https://fazm.ai/blog/mcp-server-macos-accessibility-screen-capture)
- `design/remote-invoke-file-api.md` — File API 设计（架构模式参考）
- `design/remote-invoke-shell-e2e-regressions.md` — Shell Access 安全模型参考
