# Remote Invoke 状态 TUI 自适应布局

> 状态：已实现 | 更新时间：2026-06-17

## 背景

`bifrost status` 的终端 TUI（`crates/bifrost-cli/src/commands/status_tui.rs`）中
`Remote Invoke` 标签页存在两个表格：`Connected Clients` 与 `Recent Commands`。

当前实现存在固定阈值，无法跟随终端窗口尺寸自适应：

1. **行数固定**：两张表格都用 `.take(12)` 截断数据行，即使窗口很高、可以展示 30+ 行，
   也只渲染 12 行，下方留下大片空白。
2. **截断宽度固定**：命令预览列用 `truncate_text(..., 36/44/56)` 固定字符数截断，`Client`
   通过 `caller_label` 固定在 12 字符，`Call ID` 固定 12 字符。即使窗口很宽、`Client` /
   命令列被 `Constraint::Min(..)` 拉伸到很宽，文本仍在进入表格组件前就被提前截断成 `...`，
   导致宽屏下 caller ID、命令、Latest Command、Call ID 等列大量省略号，右侧却有大片空白。

用户反馈：窗口很大时省略太多、看不到完整内容，应当自适应。

2026-06-17 追加反馈：前一版已经把文本截断阈值改为自适应，但表格列约束仍保留多个
`Constraint::Min`。在宽屏下 `Client` 列仍会继续扩张，占用大量横向空间，导致后侧
`Latest Command`、`Result`、`Call ID` 等高信息密度列仍然偏窄。

## 目标

1. 表格可见行数随表格区域高度自适应，窗口越高展示越多行（设上限避免一次渲染过多历史）。
2. `Client`、命令、`Result`、`Call ID` 等列的真实列宽随表格可用宽度自适应，窗口越宽展示越完整。
3. `Remote Invoke Status` 卡片中的 `Latest:` 摘要行也按卡片宽度自适应截断命令预览。
4. 不改变列结构、表头、统计计数语义（标题里的总数仍是全量计数，不是可见行数）。
5. 极窄/极矮窗口下不 panic，回退到可读的最小宽度与 0 行。
6. 宽屏下 `Client` 列有上限，不能继续挤压后侧命令、结果和调用 ID 列。

## 实现方案

### 新增纯函数（便于单元测试）

- `visible_table_rows(area_height, header_lines, max_rows)`：
  - 扣除上下边框（2 行）与表头行（含 `bottom_margin`，此处为 2 行）后得到可用数据行数，
    再用 `max_rows` 上限钳制。
- `adaptive_column_widths(area_width, fixed_total, total_columns, specs)`：
  - 每个弹性列声明 `min`、可选 `max` 和 `weight`。
  - 在内容区宽度（总宽减去左右边框）中扣除定长列总宽与列间隔后，按权重分配富余宽度。
  - `Client` 等低信息密度列设置上限，达到上限后停止扩张；命令列不设上限，吸收剩余空间。
  - 空间不足时回退到各列最小宽度，保证不低于可读下限。

### render_remote_invoke 调整

- `Connected Clients` 表（`layout[1]`）：
  - `client_capacity = visible_table_rows(layout[1].height, 2, MAX_REMOTE_TABLE_ROWS)`
  - 定长列保留 `Auth`、`Scope`、`Grant`、`Connected`、`Last Cmd`、`Status`。
  - 弹性列为 `Client`（最小 12、最大 36）、`Latest Command`（最小 24、无最大值）、
    `Result`（最小 18、最大 30）。
  - 用计算出的真实列宽同时驱动 `Constraint::Length(...)` 和 `truncate_text(...)`，避免表格真实布局与文本预算不一致。
- `Recent Commands` 表（`layout[2]`）：
  - 定长列保留 `Auth`、`Status`、`Started`。
  - 弹性列为 `Client`（最小 12、最大 36）、`Command`（最小 32、无最大值）、
    `Result`（最小 20、最大 30）、`Call ID`（最小 12、最大 24）。
  - `Client` 达到上限后，宽屏新增空间继续进入 `Command`，`Result` / `Call ID` 在上限内补足展示。
- `Latest:` 摘要行：按 `layout[0].width` 扣除标签、status、result 与分隔符后得到命令预算，
  替换固定 `56`。

常量：

- `MAX_REMOTE_TABLE_ROWS = 200`：单表数据行上限。
- `REMOTE_MIN_CLIENT_BUDGET = 12`：`Client` 列极窄窗口兜底宽度。
- `REMOTE_MIN_CMD_BUDGET = 18`：命令列极窄窗口兜底宽度。

## 依赖项

- 仅依赖 `ratatui` 现有的 `Rect`（提供 `width`/`height`）与现有 `truncate_text`，无新增依赖。

## 测试方案

### 单元测试（`status_tui.rs` 内 `mod tests`）

- `visible_table_rows_scales_with_height`：矮窗口 0 行、普通窗口按高度递增、超大窗口被
  上限钳制、0 高度不 panic。
- `adaptive_column_widths_caps_client_and_expands_command`：验证宽屏下 `Client` 列封顶，
  命令列吸收主要富余空间，`Result` 比旧固定宽度更完整。
- `adaptive_column_widths_keep_command_monotonic_after_client_cap`：验证窗口继续变宽时
  `Client` 不再扩张，命令列继续单调增大，`Call ID` 不缩小。
- `adaptive_column_widths_handles_narrow_and_degenerate_inputs`：无弹性列、0 宽度等退化输入不 panic。
- `remote_invoke_labels_prefer_human_names_and_normalize_auth`：补充验证 caller fingerprint 默认 12 字符截断，
  但使用宽屏预算时可完整展示。

### E2E 测试

- 执行 `e2e-tests/tests/test_status_tui_remote_invoke_panel.sh`：
  - 使用隔离 `BIFROST_DATA_DIR` 和随机端口启动临时 Bifrost。
  - 启动参数包含 `--no-system-proxy`，避免修改系统代理。
  - 通过 PTY 打开 `bifrost status --tui`，切到 `Remote Invoke` 标签页，断言面板关键区块可见。
- 使用当前真实 9900 服务补充宽屏/窄屏 PTY 捕获：
  - 宽屏验证上下两张表的 `Client` 列在空间足够时不再提前省略 caller ID。
  - 宽屏验证 `Client` 列不再占用过多宽度，`Latest Command` / `Command` / `Result` / `Call ID`
    相比旧布局展示更多内容。
  - 窄屏验证 TUI 不 panic，内容按可用空间省略。

### 真实场景测试（human_tests）

- 更新 `human_tests/remote-invoke.md`，新增 TUI 自适应布局回归用例：
  - 小窗口与大窗口分别打开 `Remote Invoke` 标签页
  - 大窗口下表格行数明显多于 12 行（数据足够时）
  - 大窗口下 `Client` / 命令 / `Latest Command` / `Result` / `Call ID` 列展示更完整，不再大面积省略
  - 大窗口下 `Client` 列不会继续挤占后侧列空间
  - 极窄窗口不崩溃、仍可读

## Review/Fix/Test 闭环方案

- 第 1 轮：复核 `take`/`Client`/命令固定截断和表格 `Constraint::Min` 是否全部替换为自适应；`git diff` 检查；运行 `cargo test -p
  bifrost-cli` 相关用例。
- 第 2 轮：复核宽屏/窄屏边界、`Client` 列封顶、标题计数语义未变、无 panic 路径；复跑单元测试。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-cli`

## 文档更新要求

- 本设计文档。
- `human_tests/remote-invoke.md` 与 `human_tests/readme.md` 索引行。
