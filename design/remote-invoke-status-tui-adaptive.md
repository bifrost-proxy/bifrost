# Remote Invoke 状态 TUI 自适应布局

> 状态：已实现 | 更新时间：2026-06-16

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

## 目标

1. 表格可见行数随表格区域高度自适应，窗口越高展示越多行（设上限避免一次渲染过多历史）。
2. `Client` 与命令列截断阈值随表格可用宽度自适应，窗口越宽展示越完整。
3. `Remote Invoke Status` 卡片中的 `Latest:` 摘要行也按卡片宽度自适应截断命令预览。
4. 不改变列结构、表头、统计计数语义（标题里的总数仍是全量计数，不是可见行数）。
5. 极窄/极矮窗口下不 panic，回退到可读的最小宽度与 0 行。

## 实现方案

### 新增纯函数（便于单元测试）

- `visible_table_rows(area_height, header_lines, max_rows)`：
  - 扣除上下边框（2 行）与表头行（含 `bottom_margin`，此处为 2 行）后得到可用数据行数，
    再用 `max_rows` 上限钳制。
- `flex_column_budget(area_width, fixed_total, flex_mins, flex_index)`：
  - 在内容区宽度（总宽减去左右边框）中扣除定长列总宽与弹性列间隔后，得到弹性列可分配的
    富余宽度；超出各弹性列最小值之和的部分按弹性列数量平均分配，得到目标弹性列的真实字符预算。
  - 空间不足时回退到该列声明的最小宽度，保证不低于可读下限。

### render_remote_invoke 调整

- `Connected Clients` 表（`layout[1]`）：
  - `client_capacity = visible_table_rows(layout[1].height, 2, MAX_REMOTE_TABLE_ROWS)`
  - `client_label_budget = flex_column_budget(layout[1].width, 定长列之和, [12,18], 0)`
    再取 `max(REMOTE_MIN_CLIENT_BUDGET)`
  - `client_cmd_budget = flex_column_budget(layout[1].width, 定长列之和, [12,18], 1)`
    再取 `max(REMOTE_MIN_CMD_BUDGET)`
  - 用 `client_capacity` 替换 `.take(12)`，用 `client_label_budget` / `client_cmd_budget`
    替换固定的 `caller_label(..., 12)` 与命令截断宽度。
- `Recent Commands` 表（`layout[2]`）：
  - `call_capacity` / `call_client_budget` / `call_cmd_budget` 同理（弹性列 `[12,24]`），
    替换 `.take(12)`、固定 client label 12 字符与固定命令宽度 `44`。
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
- `flex_column_budget_expands_with_width`：窄窗口回退最小值、宽窗口预算变大、越宽越大单调不减。
- `flex_column_budget_expands_client_column_with_width`：验证 `Client` 列和命令列一样会在宽屏下获得更大预算。
- `flex_column_budget_handles_degenerate_inputs`：无弹性列、0 宽度等退化输入不 panic。
- `remote_invoke_labels_prefer_human_names_and_normalize_auth`：补充验证 caller fingerprint 默认 12 字符截断，
  但使用宽屏预算时可完整展示。

### E2E 测试

- 执行 `e2e-tests/tests/test_status_tui_remote_invoke_panel.sh`：
  - 使用隔离 `BIFROST_DATA_DIR` 和随机端口启动临时 Bifrost。
  - 启动参数包含 `--no-system-proxy`，避免修改系统代理。
  - 通过 PTY 打开 `bifrost status --tui`，切到 `Remote Invoke` 标签页，断言面板关键区块可见。
- 使用当前真实 9900 服务补充宽屏/窄屏 PTY 捕获：
  - 宽屏验证上下两张表的 `Client` 列在空间足够时不再提前省略 caller ID。
  - 窄屏验证 TUI 不 panic，内容按可用空间省略。

### 真实场景测试（human_tests）

- 更新 `human_tests/remote-invoke.md`，新增 TUI 自适应布局回归用例：
  - 小窗口与大窗口分别打开 `Remote Invoke` 标签页
  - 大窗口下表格行数明显多于 12 行（数据足够时）
  - 大窗口下 `Client` / 命令 / `Latest Command` / `Call ID` 列展示更完整，不再大面积省略
  - 极窄窗口不崩溃、仍可读

## Review/Fix/Test 闭环方案

- 第 1 轮：复核 `take`/`Client`/命令固定截断是否全部替换为自适应；`git diff` 检查；运行 `cargo test -p
  bifrost-cli` 相关用例。
- 第 2 轮：复核宽屏/窄屏边界、标题计数语义未变、无 panic 路径；复跑单元测试。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-cli`

## 文档更新要求

- 本设计文档。
- `human_tests/remote-invoke.md` 与 `human_tests/readme.md` 索引行。
