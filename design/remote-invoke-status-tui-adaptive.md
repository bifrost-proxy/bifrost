# Remote Invoke 状态 TUI 自适应布局

> 实施状态（2026-06-22 首版，2026-07-03 复核）：`bifrost status --tui` 的 Remote Invoke 面板已完整落地行数、列宽双维度自适应，纯函数 `visible_table_rows` / `adaptive_column_widths` 与 `caller_label_with_budget` 已合入 `crates/bifrost-cli/src/commands/status_tui.rs`，`MAX_REMOTE_TABLE_ROWS` / `REMOTE_MIN_CLIENT_BUDGET` / `REMOTE_MIN_CMD_BUDGET` 三个常量与相关单元测试均已上线。

## 背景

`bifrost status --tui` 的 Remote Invoke 标签页（`crates/bifrost-cli/src/commands/status_tui.rs`）包含两张关键表格：`Connected Clients` 与 `Recent Commands`，以及顶部 `Remote Invoke Status` 卡片中的 `Latest:` 摘要行。旧实现存在两类硬编码：

1. **行数固定**：两张表格均通过 `.take(12)` 截断数据行，无论终端窗口高度多大都只渲染 12 行，下方留出大片空白。
2. **列宽固定**：`caller_label` 固定 12 字符、命令预览 `truncate_text(..., 36/44/56)` 固定字符数、`Call ID` 固定 12 字符。即使窗口宽到 200+ 字符，文本仍在进入表格组件前就被截断为 `...`，右侧却留大片留白。

用户反馈：窗口很大时省略太多、宽屏下 caller ID / 命令 / Latest Command / Call ID 都读不完整。

2026-06-17 追加反馈：即便文本截断预算改为自适应，若表格列约束仍保留多个 `Constraint::Min`，`Client` 列会继续在宽屏上无限扩张，挤占后续高信息密度列。

## 用户目标验证清单

### 必须实现

- 可见行数随表格区域高度自适应，行数上限为 `MAX_REMOTE_TABLE_ROWS = 200`，避免一次性渲染过多历史。
- `Client` / 命令 / `Result` / `Call ID` 等列真实列宽随表格可用宽度自适应；`Client` 列有上限，不再挤压后续列。
- `Remote Invoke Status` 顶部 `Latest:` 摘要行按卡片宽度自适应命令预览预算。
- 极窄 / 极矮窗口下不 panic，回退到 `REMOTE_MIN_CLIENT_BUDGET` / `REMOTE_MIN_CMD_BUDGET` 及 0 行。
- 表头、列结构、标题总数计数保持不变（标题里的总数仍是全量计数）。

### 必须不破坏

- 表格数据来源、排序、过滤逻辑完全保持；仅调整可见行与列宽。
- 已有 `status_tui` 快照或 PTY 断言中未被截断的核心字段仍展示。
- 非 Remote Invoke 面板的布局与交互（Traffic / Rules / Groups 等）不受影响。

### 必须真实验证

- 单元测试覆盖 `visible_table_rows` / `adaptive_column_widths` 在正常 / 宽屏 / 窄屏 / 退化输入下的行为。
- E2E 通过 PTY 截屏比对宽屏与窄屏渲染差异。
- human 回归：真实终端拉伸窗口能看到行数与列宽随之增长，且 `Client` 列不再无限扩张。

## 产品语义

Remote Invoke 面板是运维排查远端调用的第一屏。`Client` 是 caller 的标识、`Command` / `Latest Command` 是执行内容、`Result` 是执行摘要、`Call ID` 是唯一定位符。四者信息密度由高到低分别是命令 > Result > Call ID > Client，因此列宽增长策略必须：

- 优先把富余空间灌给 `Command` / `Latest Command`；
- 其次分配给 `Result` 与 `Call ID`，各自设上限；
- `Client` 保留可读下限但严格封顶，避免宽屏下继续扩张。

## 技术细节

### 新增纯函数（`status_tui.rs`）

- `visible_table_rows(area_height, header_lines, max_rows)`（`status_tui.rs:975`）：
  - 扣除上下边框（2 行）与表头 `bottom_margin`（2 行）后得到数据行容量；
  - 用 `max_rows` 上限钳制；0 高度或不足以放下表头时返回 0。
- `adaptive_column_widths(area_width, fixed_total, total_columns, specs)`（`status_tui.rs:995`）：
  - 每个弹性列声明 `min` / 可选 `max` / `weight`；
  - 内容区宽度 = 总宽 - 左右边框 - 列间隔 - 定长列总宽；
  - 按权重分配富余空间；达到 `max` 的列不再吃增量；命令列不设 `max`，吸收剩余空间；
  - 空间不足时回退到各列 `min`，保证不低于可读下限。
- `caller_label_with_budget(caller, fingerprint, max_chars)`（`status_tui.rs:1062`）：
  - 优先使用 caller display name，缺失则使用 fingerprint；
  - 使用 `truncate_text` 应用 budget，宽屏下可完整展示 caller ID。

### `render_remote_invoke` 调整

常量（`status_tui.rs:1830-1834`）：

```rust
const MAX_REMOTE_TABLE_ROWS: usize = 200;
const REMOTE_MIN_CMD_BUDGET: usize = 18;
const REMOTE_MIN_CLIENT_BUDGET: usize = 12;
```

`Latest:` 摘要（顶部卡片）：

- 按 `layout[0].width` 扣除标签、status、result 与分隔符后得到命令预算，兜底 `REMOTE_MIN_CMD_BUDGET`，用于 `truncate_text(command_preview, cmd_budget)`（`status_tui.rs:1871-1874`）。

`Connected Clients` 表（`layout[1]`）：

- `client_capacity = visible_table_rows(layout[1].height, 2, MAX_REMOTE_TABLE_ROWS)`（`status_tui.rs:1927`）；
- `client_widths = adaptive_column_widths(...)`（`status_tui.rs:1928`）；
- 定长列：`Auth` / `Scope` / `Grant` / `Connected` / `Last Cmd` / `Status`；
- 弹性列：`Client` (min 12, max 36)、`Latest Command`（min 24，无 max）、`Result`（min 18, max 30）；
- 由计算宽度同时驱动 `Constraint::Length(..)` 和 `truncate_text(..)`，避免表格与文本预算不一致（`status_tui.rs:1937-1965`）。

`Recent Commands` 表（`layout[2]`）：

- `call_capacity = visible_table_rows(layout[2].height, 2, MAX_REMOTE_TABLE_ROWS)`（`status_tui.rs:2042`）；
- `call_widths = adaptive_column_widths(...)`（`status_tui.rs:2043`）；
- 定长列：`Auth` / `Status` / `Started`；
- 弹性列：`Client`（min 12, max 36）、`Command`（min 32，无 max）、`Result`（min 20, max 30）、`Call ID`（min 12, max 24）；
- `Client` 达到上限后新增宽度进入 `Command`，`Result` / `Call ID` 在各自 max 内补齐（`status_tui.rs:2071-2082`）。

## 依赖项

- 仅依赖 `ratatui` 现有 `Rect`（`width` / `height`）与既有 `truncate_text`；无新增外部依赖。

## CLI

- 面板行为透明改进，不新增 CLI 子命令。
- `bifrost status --tui` 打开后切到 `Remote Invoke` 标签页即生效。

## Web UI

- 本改动仅作用于 TUI，不涉及 Web。

## Admin API

- 无 API 变更；面板数据仍通过既有 `RemoteInvokeSnapshot` 接口获取。

## Sync 边界

- 仅本地终端渲染层改造，不涉及 sync / 持久化。

## Phase 拆分

- Phase 1：新增 `visible_table_rows` / `adaptive_column_widths` / `caller_label_with_budget` 纯函数与常量。
- Phase 2：在 `render_remote_invoke` 中替换 `.take(12)` 与固定截断阈值，接入自适应结果。
- Phase 3：`Latest:` 摘要与两张表接入同一套预算；补齐 PTY 断言。
- Phase 4：human_tests 回归 + 文档更新。

## 测试方案

### 单元测试（`status_tui.rs::mod tests`）

- `visible_table_rows_scales_with_height`（`status_tui.rs:2243`）：
  - 高度不足 → 0；
  - 高度 20 / header 2 / max 200 → 16 行；
  - 高度 1000 → 被 `MAX_REMOTE_TABLE_ROWS` 钳制为 200；
  - 高度 0 → 0 不 panic。
- `adaptive_column_widths_caps_client_and_expands_command`（`status_tui.rs:2258`）：
  - 240 宽度输入下 `Client` 列封顶，命令列拿到主要富余，`Result` 比旧固定宽度更完整。
- `adaptive_column_widths_keep_command_monotonic_after_client_cap`（`status_tui.rs:2295`）：
  - 220 → 320 宽度递增时，`Client` 不再扩张，命令列单调增长，`Call ID` 不缩小。
- `adaptive_column_widths_handles_narrow_and_degenerate_inputs`（`status_tui.rs:2339`）：
  - 0 宽度、100 宽度但 fixed=90 → 回退到 min 宽度，且不 panic。
- `remote_invoke_labels_prefer_human_names_and_normalize_auth`：验证 caller fingerprint 默认 12 字符截断，宽屏预算下可完整展示。

### E2E 测试

`e2e-tests/tests/test_status_tui_remote_invoke_panel.sh`：

- 使用隔离 `BIFROST_DATA_DIR`、随机端口、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 启动临时 Bifrost；
- PTY 打开 `bifrost status --tui`，切到 `Remote Invoke` 标签页；
- 宽屏 PTY（列宽 200+）：断言 `Client` 列 caller ID 完整、`Command` / `Latest Command` / `Result` / `Call ID` 相比旧布局展示更多字符、`Client` 不占据过宽；
- 窄屏 PTY：断言 UI 不 panic，内容按可用空间省略且能读；
- 高窗口：断言实际渲染行数 > 12（数据足够时）。

### human_tests

`human_tests/remote-invoke.md`：新增 TUI 自适应布局回归用例：

- 小窗口与大窗口分别打开 `Remote Invoke` 标签页；
- 大窗口下表格行数明显多于 12 行（数据足够时）；
- 大窗口下 `Client` / 命令 / `Latest Command` / `Result` / `Call ID` 列展示更完整，不再大面积省略；
- 大窗口下 `Client` 列不会继续挤占后侧列空间；
- 极窄窗口不崩溃、仍可读。

同步刷新 `human_tests/readme.md`。

### 校验命令

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-cli`
- `bash e2e-tests/tests/test_status_tui_remote_invoke_panel.sh`

本机 no-local-coverage，coverage 交给 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核所有 `.take(12)` / 固定截断阈值 / `Constraint::Min(..)` 是否全部替换为自适应；
- 复核 `git diff`；确认 `Client` 列封顶生效；
- 运行 `cargo test -p bifrost-cli` 相关用例。

### 第 2 轮

- 复核宽 / 窄边界、标题计数语义未变、无 panic 路径；
- 复跑单元测试与 PTY E2E；
- 回归 human_tests 中新用例。

## 风险与决策

- `MAX_REMOTE_TABLE_ROWS = 200`：兼顾大窗口可视化与渲染成本，未来若需分页可以下沉到虚拟滚动。
- `Client` 列封顶为 36 字符：兼容常见 caller display name / fingerprint 前缀；如后续 caller ID 变长需要重新评估。
- `Latest Command` 与 `Command` 未设 `max`：优先信息密度，可能导致极超宽窗口下命令列超长，可接受，因为它是主要观察目标。
- 若未来引入更多弹性列，需要保证 `weight` 平衡，避免命令列被稀释。
