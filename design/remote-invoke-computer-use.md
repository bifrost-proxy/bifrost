# Computer Use API 设计方案

> 状态：RFC（征求意见稿）（planned, not yet shipped as of 2026-06-16）
> 作者：Mira Agent
> 日期：2026-04-25
> 关联：`design/remote-invoke-file-api.md`、`design/remote-invoke-shell-e2e-regressions.md`
> 范围：Remote Invoke 第四类能力——GUI 屏幕感知与输入控制
>
> **实现状态对照（2026-06-16 review）**：本文档描述的 Computer Use 能力**整体尚未落地**。代码侧核查结果：
> - `crates/bifrost-admin/src/remote_invoke/types.rs` 中 `GrantScope` 当前枚举值为 `RemoteQuery` / `RemoteShellExec` / `RemoteShellInteractive` / `RemotePowerMgmt` / `RemoteImGateway`，**未包含** `RemoteComputerRead` / `RemoteComputerWrite`。
> - 同文件中 `CommandKind` 当前枚举值为 `QueryReadonly` / `ShellExec` / `File` / `PowerMgmt` / `ImGateway`，**未包含** `Computer` 变体。
> - `crates/bifrost-cli` 不存在 `bifrost remote computer ...` 子命令，无 `computer.*` 方法分发。
> - 不存在 `bifrost-screen-agent` 二进制、`computer_ops` 模块、`ComputerAccessPolicy` 类型或 `computer_access.toml` 配置。
> 因此下文所有 method、scope、policy、CLI、Phase 路径仍属设计预案，落地时请按本节列出的当前 enum 变体做增量改动，并同步更新 `design/remote-invoke-file-api.md` 中关于 scope 矩阵的描述。

## 背景与目标

Bifrost Remote Invoke 已覆盖三类能力：

1. **只读查询**（`query.readonly`）：status、search、traffic
2. **Shell 控制**（`shell.exec`）：在 Shell Access policy 下执行命令
3. **文件操作**（`file.*`）：读写、编辑、搜索、patch

但 Agent 在远端设备上遇到 GUI 场景时完全无能为力：无法截图、无法看到屏幕内容、无法点击按钮或输入文本。典型场景：

- **验证 UI 渲染**：Agent 修改了前端代码，想看渲染结果
- **操作无 CLI 的应用**：某些 GUI 工具只有界面没有命令行
- **填写网页表单**：浏览器自动化（不依赖 Selenium/Puppeteer）
- **调试桌面应用**：查看崩溃弹窗、确认对话框

Claude Computer Use 已证明此能力对 Agent 的价值：模型通过「截图→推理→操作→再截图验证」的循环，可以像人一样使用计算机。

**目标**：在 Bifrost Remote Invoke 体系中新增 **Computer Use API**，作为第四类 remote 能力，复用现有 relay + encrypted channel + grant 授权体系。

---

## 能力矩阵（planned, not yet shipped as of 2026-06-16）

| method | 语义 | 必需 scope |
|--------|------|------------|
| `computer.screenshot` | 截取屏幕/窗口图像 | `remote_computer_read` |
| `computer.mouse_move` | 移动鼠标到指定坐标 | `remote_computer_write` |
| `computer.click` | 点击鼠标（左/右/中键，单击/双击） | `remote_computer_write` |
| `computer.type` | 输入文本 | `remote_computer_write` |
| `computer.key_press` | 按键/组合键 | `remote_computer_write` |
| `computer.scroll` | 滚动（指定方向和量） | `remote_computer_write` |
| `computer.drag` | 拖拽操作 | `remote_computer_write` |
| `computer.get_accessibility_tree` | 获取 UI 无障碍树（结构化） | `remote_computer_read` |
| `computer.get_active_window` | 获取当前活跃窗口信息 | `remote_computer_read` |
| `computer.wait_for_change` | 等待屏幕变化（减少轮询） | `remote_computer_read` |

---

## 架构设计（planned, not yet shipped as of 2026-06-16）

```
┌──────────────┐  computer.screenshot/click/type  ┌────────────┐
│ Caller (CLI) │ ─────────────────────────────▶  │   Relay    │
└──────────────┘                                  └─────┬──────┘
                                                          ▼
                                                ┌──────────────────┐
                                                │ Target Client    │
                                                │ (bifrost-admin)  │
                                                └────────┬─────────┘
                                                         ▼
                                                ┌──────────────────┐
                                                │ Computer Use     │
                                                │ Policy Guard     │
                                                └────────┬─────────┘
                                                         ▼
                                          ┌──────────────┴──────────────┐
                                          ▼                             ▼
                                  ┌──────────────┐           ┌──────────────┐
                                  │ Screenshot   │           │ Input        │
                                  │ Engine       │           │ Engine       │
                                  │(ScreenCapture│           │(CGEvent/     │
                                  │ Kit/CGWindow)│           │ Accessibility│
                                  └──────────────┘           └──────────────┘
```

与 File API 设计保持一致的分层原则：

- Caller 端新增 `bifrost remote computer <subcmd>` 系列子命令，打包请求走 relay
- Relay 仅路由转发，不理解 computer API 内容（保持端到端加密）
- Target 端 `bifrost-admin` 新增 `remote_invoke::computer_ops` 模块，负责：
  1. 依据 `grant_scope` 拒绝无权请求
  2. 依据 `ComputerAccessPolicy` 做应用/窗口/操作类型的细粒度控制
  3. 通过 Screenshot Engine 和 Input Engine 执行具体操作，返回结构化结果

---

## 授权模型（planned, not yet shipped as of 2026-06-16；下文新增 GrantScope/CommandKind 变体均未实现）

### 新增 GrantScope

在 `GrantScope` 枚举上新增两个变体：

```rust
#[serde(rename = "remote_computer_read")]
RemoteComputerRead,

#[serde(rename = "remote_computer_write")]
RemoteComputerWrite,
```

`CommandKind` 枚举新增：

```rust
#[serde(rename = "computer")]
Computer,
```

`GrantScope::allows_command` 规则：

- `RemoteComputerRead` → 允许 `computer.screenshot`、`computer.get_accessibility_tree`、`computer.get_active_window`、`computer.wait_for_change`
- `RemoteComputerWrite` → 允许所有 `computer.*` 命令（含只读）
- `RemoteQuery` / `RemoteShellExec` / `RemoteFileRead` / `RemoteFileWrite` 均**不**授予 computer API 能力

### Computer Access Policy

类比 FileAccessPolicy，新增独立的 Computer Access Policy 存储。首版数据结构：

```toml
[[computer_access_policy]]
id = "desktop-readonly"
name = "Desktop Read-only"

# 允许截屏的显示器（空 = 全部）
allowed_displays = []

# 允许截屏/操作的应用（bundle id 白名单，空 = 全部）
allowed_apps = []

# 禁止截屏/操作的应用（优先级高于 allowed_apps）
denied_apps = [
    "com.1password.1password",
    "com.apple.keychainaccess",
    "com.apple.SecurityAgent",
]

# 禁止输入操作的应用（可截图但不可点击/输入）
no_control_apps = [
    "com.apple.systempreferences",
]

# 是否允许截图中包含其他用户窗口
capture_other_users = false

# 允许的操作列表
ops = ["screenshot", "get_accessibility_tree", "get_active_window", "wait_for_change"]

# 截图参数
max_screenshot_width = 1920
max_screenshot_height = 1080
screenshot_format = "png"
screenshot_quality = 80

# 输入频率限制
max_keypress_per_minute = 120
max_clicks_per_minute = 60

# 是否需要用户逐次确认输入操作
require_confirmation = true

# 信任时长（秒），0 = 每次都需确认
trust_duration_secs = 0
```

内置默认 policy：

```toml
[[computer_access_policy]]
id = "desktop-readonly"
name = "Desktop Read-only"
denied_apps = ["com.1password.1password", "com.apple.keychainaccess"]
ops = ["screenshot", "get_accessibility_tree", "get_active_window", "wait_for_change"]
max_screenshot_width = 1920
max_screenshot_height = 1080
screenshot_format = "png"
require_confirmation = false
```

---

## Target 端核心实现（planned, not yet shipped as of 2026-06-16）

### Screenshot Engine

**核心问题**：`screencapture` 从 remote shell 无法访问 GUI session。

**方案：常驻 Screenshot Agent 进程**

Bifrost 在 `bifrost start` 时以用户 GUI session 上下文启动一个轻量常驻进程 `bifrost-screen-agent`：

- **macOS**：
  - 优先使用 `ScreenCaptureKit`（Apple 推荐的现代 API，macOS 12.3+）
  - 降级使用 `CGWindowListCreateImage`（兼容旧系统）
  - 最后降级调用系统 `screencapture` 命令
  - 需要 **Screen Recording** 权限（macOS 系统设置 → 隐私与安全 → 屏幕录制）

- **Linux**：
  - 使用 `xdotool` + `import`（ImageMagick）或 PipeWire
  - 需要 X11/Wayland 访问权限

该进程监听 admin HTTP 端口上的请求，admin executor 转发 screenshot 请求到此进程。

```rust
// bifrost-screen-agent 核心逻辑（macOS）
pub async fn capture_screenshot(
    display: u32,
    region: Option<Rect>,
    format: ImageFormat,
    quality: u8,
    scale: f32,
    exclude_pids: &[u32],
) -> Result<ScreenshotResult> {
    // 1. 检查 Screen Recording 权限
    // 2. 调用 ScreenCaptureKit / CGWindowListCreateImage
    // 3. 如有 denied_apps，排除对应窗口
    // 4. 如有 region，裁剪
    // 5. 如有 scale，缩放
    // 6. 编码为 PNG/JPEG
    // 7. 返回 bytes + 元信息
}

pub struct ScreenshotResult {
    pub image_b64: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub scale_factor: f32,
    pub active_window: Option<WindowInfo>,
    pub cursor_position: Option<Position>,
}
```

**备选方案：降级到 screencapture CLI**

如果用户不想运行常驻 agent，可以在 Bifrost daemon 启动时用 `launchctl` 注册一个用户 agent（LaunchAgent），确保在 GUI session 中执行 `screencapture`。

### Input Engine

输入操作需要 **Accessibility** 权限（macOS 系统设置 → 隐私与安全 → 辅助功能）。

**macOS 实现（CGEvent）**：

```rust
pub fn mouse_move(x: i32, y: i32) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)?;
    let event = CGEvent::new_mouse_event(
        source, CGEventType::MouseMoved,
        x, y, CGMouseButton::Left,
    )?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn click(x: i32, y: i32, button: MouseButton, count: u8) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)?;
    mouse_move(x, y)?;

    let (down_type, up_type) = match button {
        MouseButton::Left => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp),
        MouseButton::Right => (CGEventType::RightMouseDown, CGEventType::RightMouseUp),
        MouseButton::Middle => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp),
    };

    for _ in 0..count {
        let down = CGEvent::new_mouse_event(source.clone(), down_type, x, y, button.into())?;
        down.post(CGEventTapLocation::HID);
        let up = CGEvent::new_mouse_event(source.clone(), up_type, x, y, button.into())?;
        up.post(CGEventTapLocation::HID);
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub fn type_text(text: &str, clear: bool) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)?;
    if clear {
        // Cmd+A 全选
        key_press(&source, 0x00, &[ModifierKey::Command])?;
        std::thread::sleep(Duration::from_millis(50));
    }
    for ch in text.chars() {
        // 使用 CGEvent::new_keyboard_event 逐字符输入
        // 处理 Unicode 字符需要 UniChar 事件
    }
    Ok(())
}

pub fn key_press(source: &CGEventSource, keycode: u16, modifiers: &[ModifierKey]) -> Result<()> {
    // 设置 modifier flags
    let mut flags = CGEventFlags::empty();
    for m in modifiers {
        flags |= match m {
            ModifierKey::Command => CGEventFlags::CGEventFlagCommand,
            ModifierKey::Option => CGEventFlags::CGEventFlagAlternate,
            ModifierKey::Control => CGEventFlags::CGEventFlagControl,
            ModifierKey::Shift => CGEventFlags::CGEventFlagShift,
        };
    }
    let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)?;
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);

    let up = CGEvent::new_keyboard_event(source.clone(), keycode, false)?;
    up.set_flags(flags);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn scroll(x: i32, y: i32, direction: ScrollDirection, amount: u32) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)?;
    mouse_move(x, y)?;
    let (units, count) = match direction {
        ScrollDirection::Up => (ScrollUnit::Pixel, -(amount as i32)),
        ScrollDirection::Down => (ScrollUnit::Pixel, amount as i32),
    };
    let event = CGEvent::new_scroll_wheel_event(source, units, count.abs() as u32, 0)?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn drag(from: Position, to: Position, duration_ms: u64) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)?;
    mouse_move(from.x, from.y)?;
    let down = CGEvent::new_mouse_event(source.clone(), CGEventType::LeftMouseDown, from.x, from.y, CGMouseButton::Left)?;
    down.post(CGEventTapLocation::HID);

    // 在 duration 内均匀插值移动
    let steps = (duration_ms / 16).max(1);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let cx = from.x + ((to.x - from.x) as f32 * t) as i32;
        let cy = from.y + ((to.y - from.y) as f32 * t) as i32;
        let moved = CGEvent::new_mouse_event(source.clone(), CGEventType::LeftMouseDragged, cx, cy, CGMouseButton::Left)?;
        moved.post(CGEventTapLocation::HID);
        std::thread::sleep(Duration::from_millis(16));
    }

    let up = CGEvent::new_mouse_event(source.clone(), CGEventType::LeftMouseUp, to.x, to.y, CGMouseButton::Left)?;
    up.post(CGEventTapLocation::HID);
    Ok(())
}
```

**Linux 实现（xdotool）**：

```bash
xdotool mousemove $x $y
xdotool click 1                    # left click
xdotool click --repeat 2 1        # double click
xdotool type --clearmodifiers "$text"
xdotool key ctrl+c
xdotool click 4                    # scroll up
xdotool click 5                    # scroll down
```

### Accessibility Tree Engine

这是比 Claude Computer Use 更进阶的能力——不只靠视觉，还能获取结构化 UI 信息：

```rust
pub fn get_accessibility_tree(pid: Option<u32>, depth: u32, filter_role: Option<&str>) -> Result<AccessibilityTree> {
    let element = match pid {
        Some(p) => AXUIElementCreateApplication(p),
        None => AXUIElementCreateSystemWide(),
    };
    traverse_tree(&element, depth, filter_role)
}

fn traverse_tree(element: &AXUIElement, depth: u32, filter_role: Option<&str>) -> Result<AccessibilityTree> {
    let role = get_string_attr(element, kAXRoleAttribute)?;
    let title = get_string_attr(element, kAXTitleAttribute)?;
    let value = get_string_attr(element, kAXValueAttribute)?;
    let position = get_value_attr::<AXValue>(element, kAXPositionAttribute)?;
    let size = get_value_attr::<AXValue>(element, kAXSizeAttribute)?;
    let enabled = get_bool_attr(element, kAXEnabledAttribute)?;
    let focused = get_bool_attr(element, kAXFocusedAttribute)?;

    let mut children = Vec::new();
    if depth > 0 {
        if let Ok(child_array) = get_attr(element, kAXChildrenAttribute) {
            for child in child_array {
                children.push(traverse_tree(&child, depth - 1, filter_role)?);
            }
        }
    }

    Ok(AccessibilityTree {
        role,
        title,
        value,
        position,
        size,
        enabled,
        focused,
        children,
    })
}
```

返回的 accessibility tree 让 Agent 可以：
- **不依赖视觉模型**定位 UI 元素（按 role/title/value 精确匹配）
- **获取坐标**用于后续 click/type 操作
- **读取 UI 状态**（按钮是否 enabled、文本框内容等）

这形成了「视觉 + 结构化」的双通道感知，比纯截图更可靠。

---

## 协议细节（planned, not yet shipped as of 2026-06-16）

Remote invoke 请求复用 `RemoteInvokeRequest` 包络，`CommandKind` 新增 `Computer` 变体。

### 通用请求字段

```jsonc
{
  "kind": "computer",
  "command": "computer.screenshot",  // 具体 method
  "args_json": "{…}",                // 每个 method 独立 schema
  "grant_scope": "remote_computer_read"
}
```

### `computer.screenshot`

```jsonc
// request.args_json
{
  "display": 0,              // 显示器编号，默认 0
  "region": null,            // 可选 {x, y, w, h} 裁剪区域
  "format": "png",           // png | jpeg | webp
  "quality": 80,             // JPEG quality (1-100)
  "scale": 1.0               // 缩放比（0.5 = 半分辨率，节省带宽）
}

// response
{
  "image_b64": "...",        // base64 编码的图片
  "width": 1920,
  "height": 1080,
  "format": "png",
  "scale_factor": 2.0,      // Retina 显示器的实际缩放比
  "active_window": {
    "pid": 9054,
    "title": "VS Code",
    "bundle_id": "com.microsoft.VSCode",
    "bounds": { "x": 0, "y": 0, "w": 1920, "h": 1080 }
  },
  "cursor_position": { "x": 500, "y": 300 }
}
```

### `computer.click`

```jsonc
// request.args_json
{
  "x": 500,
  "y": 300,
  "button": "left",         // left | right | middle
  "count": 1                 // 1 = 单击, 2 = 双击
}

// response
{
  "ok": true,
  "cursor_position": { "x": 500, "y": 300 },
  "target_window": { "pid": 9054, "bundle_id": "com.microsoft.VSCode" }
}
```

### `computer.type`

```jsonc
// request.args_json
{
  "text": "Hello World",
  "clear": false             // true = 先全选再输入（替换）
}

// response
{
  "ok": true,
  "characters_typed": 11
}
```

### `computer.key_press`

```jsonc
// request.args_json
{
  "key": "c",                // 字符名（与 keycode 二选一）
  "keycode": null,           // 虚拟键码（macOS: CGKeyCode, Linux: xkeysym）
  "modifiers": ["command"],  // command | option | control | shift
  "down": true,              // true = 只按下, false = 只释放, null = 按下+释放
  "up": true
}

// 常用组合键示例
// Cmd+C:   { "key": "c", "modifiers": ["command"] }
// Cmd+V:   { "key": "v", "modifiers": ["command"] }
// Ctrl+Z:  { "key": "z", "modifiers": ["control"] }
// Return:  { "keycode": 36 }
// Tab:     { "keycode": 48 }
// Escape:  { "keycode": 53 }

// response
{
  "ok": true
}
```

### `computer.scroll`

```jsonc
// request.args_json
{
  "x": 500,
  "y": 300,
  "direction": "down",       // up | down | left | right
  "amount": 5                // 滚动单位数
}

// response
{
  "ok": true
}
```

### `computer.drag`

```jsonc
// request.args_json
{
  "from": { "x": 100, "y": 200 },
  "to": { "x": 300, "y": 400 },
  "duration_ms": 500         // 拖拽持续时间
}

// response
{
  "ok": true
}
```

### `computer.get_accessibility_tree`

```jsonc
// request.args_json
{
  "pid": null,               // 应用 PID，null = 系统全局
  "depth": 3,                // 遍历深度
  "filter_role": null        // 可选，只返回指定 role（如 "AXButton"）
}

// response
{
  "tree": {
    "role": "AXApplication",
    "title": "VS Code",
    "value": null,
    "position": { "x": 0, "y": 0 },
    "size": { "w": 1920, "h": 1080 },
    "enabled": true,
    "focused": true,
    "children": [
      {
        "role": "AXWindow",
        "title": "main.rs - bifrost",
        "children": [
          {
            "role": "AXTextArea",
            "title": null,
            "value": "fn main() { ... }",
            "position": { "x": 200, "y": 50 },
            "size": { "w": 1500, "h": 900 }
          }
        ]
      }
    ]
  },
  "pid": 9054,
  "bundle_id": "com.microsoft.VSCode"
}
```

### `computer.get_active_window`

```jsonc
// request.args_json
{}

// response
{
  "pid": 9054,
  "title": "main.rs - bifrost",
  "bundle_id": "com.microsoft.VSCode",
  "bounds": { "x": 0, "y": 0, "w": 1920, "h": 1080 },
  "is_frontmost": true
}
```

### `computer.wait_for_change`

```jsonc
// request.args_json
{
  "region": null,             // 等待变化的区域，null = 全屏
  "timeout_ms": 5000,         // 超时时间
  "threshold": 0.05,          // 像素变化比例阈值（5%）
  "poll_interval_ms": 200     // 轮询间隔
}

// response
{
  "changed": true,
  "elapsed_ms": 1200,
  "changed_region": { "x": 500, "y": 300, "w": 200, "h": 50 }
}
```

### 错误码

统一前缀 `computer.`：

| code | 触发场景 |
|------|----------|
| `computer.permission_denied` | grant scope 不允许 / policy 拒绝 |
| `computer.scope_required` | grant_scope 未包含 `remote_computer_read/write` |
| `computer.screen_recording_disabled` | macOS Screen Recording 权限未开启 |
| `computer.accessibility_disabled` | macOS Accessibility 权限未开启（输入操作） |
| `computer.app_denied` | 目标应用在 denied_apps 中 |
| `computer.app_no_control` | 目标应用在 no_control_apps 中，只允许截图不允许操作 |
| `computer.rate_limited` | 超过 max_keypress_per_minute / max_clicks_per_minute |
| `computer.confirmation_required` | require_confirmation=true 且用户未确认 |
| `computer.confirmation_timeout` | 用户确认超时 |
| `computer.screenshot_failed` | 截图失败（如无显示器） |
| `computer.input_failed` | 输入操作失败（如 CGEvent 被阻止） |
| `computer.no_active_display` | 无活跃显示器 |
| `computer.invalid_coordinates` | 坐标超出屏幕范围 |
| `computer.io_error` | 底层系统错误 |

---

## 安全设计（planned, not yet shipped as of 2026-06-16）

### 权限分层（三级）

| 级别 | 能力 | 授权方式 |
|------|------|----------|
| 只读 | screenshot / a11y-tree / active-window / wait_for_change | grant 授权 + Screen Recording 权限 |
| 操作 | click / type / key_press / scroll / drag | grant 授权 + Accessibility 权限 + **二次确认** |
| 危险 | 操作系统偏好设置 / 终端 / 密码管理器 | 默认拒绝，需显式 allowed_apps 放行 |

### 审批流（关键安全特性）

Computer Use 的写操作应**默认需用户确认**：

- 首次 `computer.click` 时，target 端弹出系统通知：「Agent 请求点击 (500, 300)，是否允许？」
- 用户可设置「信任此 Agent × 30 分钟」跳过逐次确认
- 用户可随时在 Web UI 撤销信任
- `trust_duration_secs` 控制信任窗口长度

这比 Claude Computer Use 的安全模型更细粒度——Claude 是全局开关，Bifrost 是 per-grant + per-action-type。

### 敏感应用保护

截图时遇到 `denied_apps` 的窗口：

- 方案 A：用 `CGWindowListCreateImage` 的 `exclude_windows` 参数排除（推荐）
- 方案 B：在截图后用矩形遮挡（黑色/模糊）覆盖
- 方案 C：返回「该区域因安全策略已遮挡」标记

输入操作遇到 `no_control_apps` 的窗口：

- `computer.click` 返回 `computer.app_no_control`
- `computer.type` 同上
- `computer.screenshot` 仍然允许（只读不受限）

### 输入频率限制

防止 Agent 失控：

- `max_keypress_per_minute = 120`（每分钟最多 120 次按键）
- `max_clicks_per_minute = 60`（每分钟最多 60 次点击）
- 超限返回 `computer.rate_limited` 错误
- 计数器每分钟重置

### 操作审计

每次操作记录：

```
{
  grant_id: "g-abc",
  action: "computer.click",
  coordinates: { x: 500, y: 300 },
  target_app: "com.microsoft.VSCode",
  target_pid: 9054,
  timestamp: 1714000000,
  result: "ok"
}
```

截图不记录图像内容，只记录「截图了哪个显示器/窗口」。失败操作也记录（`result=rejected/failed`）。

### Retina 坐标系处理

macOS Retina 显示器存在逻辑坐标与物理像素的 2x 差异：

- 截图返回 `scale_factor: 2.0`，图像物理尺寸 = 逻辑尺寸 × scale_factor
- 所有输入操作（click/type/scroll）统一使用**逻辑坐标**
- CLI 自动处理坐标转换，Agent 无需关心

---

## CLI 映射（caller 侧）（planned, not yet shipped as of 2026-06-16；`bifrost remote computer` 子命令尚未在 bifrost-cli 中注册）

```
bifrost remote computer screenshot    [--display N] [--region x,y,w,h] [--format png|jpeg] [--scale 0.5] [--save path]
bifrost remote computer click          <x> <y> [--button left|right|middle] [--count 1|2]
bifrost remote computer type           <text> [--clear]
bifrost remote computer key-press      <key> [--modifiers cmd,shift] [--keycode N]
bifrost remote computer scroll         <x> <y> --direction up|down --amount N
bifrost remote computer drag           <from-x,from-y> <to-x,to-y> [--duration-ms 500]
bifrost remote computer a11y-tree      [--pid N] [--depth 3] [--filter-role AXButton]
bifrost remote computer active-window
bifrost remote computer wait           [--region x,y,w,h] [--timeout-ms 5000]
```

所有命令支持 `--output json|compact`，默认为人类可读格式。

---

## 实现路径（planned, not yet shipped as of 2026-06-16；Phase 1/2/3 均未启动）

### Phase 1：只读（截图 + UI 信息）（planned, not yet shipped as of 2026-06-16）

**目标**：Agent 能「看到」远端屏幕。

- `bifrost-screen-agent` 常驻进程（macOS: ScreenCaptureKit / CGWindowListCreateImage）
- `computer.screenshot`
- `computer.get_active_window`
- `computer.get_accessibility_tree`
- `computer.wait_for_change`
- `ComputerAccessPolicy` + `computer_access.toml`
- CLI 子命令
- 预计新增 ~800 行 Rust

**前置条件**：
- macOS Screen Recording 权限
- 启动时检测权限，缺权限时明确报错并引导用户开启

### Phase 2：输入操作（planned, not yet shipped as of 2026-06-16）

**目标**：Agent 能「操作」远端 GUI。

- Input Engine（macOS: CGEvent, Linux: xdotool）
- `computer.click` / `computer.type` / `computer.key_press` / `computer.scroll` / `computer.drag`
- 审批流 + 频率限制
- 预计新增 ~1200 行 Rust

**前置条件**：
- macOS Accessibility 权限
- Phase 1 已稳定运行

### Phase 3：高级能力（planned, not yet shipped as of 2026-06-16）

**目标**：更精准的 GUI 交互。

- Accessibility Tree 深度集成（按元素定位而非坐标）
- 窗口管理（`computer.list_windows` / `computer.focus_window` / `computer.resize_window`）
- 多显示器完整支持
- 录屏回放（debug 用，可选）
- 预计新增 ~600 行 Rust

---

## 与现有能力的分工原则

| 需求 | 推荐路径 | 说明 |
|------|----------|------|
| 读写代码文件 | `file.*` | 结构化、安全、可审计 |
| 运行命令 | `shell.exec` | 进程级操作 |
| 操作 GUI 应用 | `computer.*` | 视觉 + 输入 |
| 验证 UI 渲染 | `computer.screenshot` | 截图验证 |
| 查询 UI 元素 | `computer.get_accessibility_tree` | 结构化定位 |
| 填写网页表单 | `computer.*` | 浏览器 GUI 操作 |
| 操控 IDE | `computer.*` | 点击菜单/按钮 |
| 运行 `git` 命令 | `shell.exec` | 命令行更可靠 |
| 编译代码 | `shell.exec` | `cargo build` 无需 GUI |

**关键原则**：能用 `file.*` 和 `shell.exec` 解决的，不用 `computer.*`。Computer Use 是**最后手段**，适用于只有 GUI 才能触达的操作。

---

## 关键技术风险

| 风险 | 影响 | 缓解 |
|------|------|------|
| macOS Screen Recording 权限 | 截图完全不可用 | 启动时检测权限，缺权限时明确报错引导用户开启 |
| macOS Accessibility 权限 | 输入操作不可用 | 同上 |
| GPU 加速窗口截图黑屏 | Chrome/游戏等窗口截不到 | 降级到 ScreenCaptureKit 或通知用户关闭硬件加速 |
| CGEvent 被 macOS SIP 阻止 | 某些系统弹窗无法点击 | 文档说明限制，引导用户使用辅助功能权限 |
| 坐标系差异（Retina） | 2x 屏幕点击偏移 | 截图返回 scale_factor，CLI 自动转换坐标 |
| relay 带宽 | 截图图片较大 | 支持 JPEG 压缩 + scale 缩放 + region 裁剪 |
| Agent 无限循环 | 持续点击/输入 | 频率限制 + 每步操作计数上限 + 用户可中断 |
| 隐私泄露 | 截图包含敏感信息 | denied_apps 排除敏感窗口 + 审计日志 |

---

## 与 Claude Computer Use 的对比

| 维度 | Claude Computer Use | Bifrost Computer Use |
|------|--------------------|--------------------|
| 截图 | 支持 | 支持（ScreenCaptureKit） |
| 鼠标点击 | 支持 | 支持（CGEvent） |
| 键盘输入 | 支持 | 支持（CGEvent） |
| 滚动 | 支持 | 支持 |
| 拖拽 | 有限 | 支持（可调 duration） |
| Accessibility Tree | 不支持 | **支持**（结构化 UI 感知） |
| 等待屏幕变化 | 不支持（需轮询截图） | **支持**（wait_for_change） |
| 敏感应用保护 | 全局开关 | **细粒度**（per-app deny/allow） |
| 操作审批 | 全局开关 | **per-action 确认** |
| 频率限制 | 无 | **支持** |
| 跨平台 | Docker 容器 | **原生**（macOS + Linux） |
| 加密传输 | HTTPS | **E2E 加密**（relay） |
| 审计 | 无 | **支持** |

---

## 参考

- [Claude Computer Use Tool](https://docs.anthropic.com/en/docs/build-with-claude/computer-use) — Anthropic 的 computer use API 设计
- [macOS ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit) — Apple 推荐的截屏 API
- [macOS Accessibility Programming](https://developer.apple.com/library/archive/documentation/Accessibility/Conceptual/AccessibilityMacOSX/) — 无障碍 API
- [Fazm MCP Server](https://fazm.ai/blog/mcp-server-macos-accessibility-screen-capture) — macOS Accessibility + Screen Capture 的 MCP 实现参考
- `design/remote-invoke-file-api.md` — File API 设计（架构模式参考）
- `design/remote-invoke-shell-e2e-regressions.md` — Shell Access 安全模型参考
