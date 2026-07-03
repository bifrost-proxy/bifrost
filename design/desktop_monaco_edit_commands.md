# 桌面端 Monaco 编辑命令与文档脏状态统一修复方案

## 背景

Bifrost 桌面端（Tauri + macOS WebView）中所有基于 Monaco 的编辑器都存在同一类运行时问题，并伴随一个保存后原生窗口状态不同步的问题。同样的前端代码在 Web 管理端正常，因此问题边界收敛到桌面壳层运行时。

受影响入口：

- `Rules` 页面右侧规则编辑器
- `Values` 页面右侧值编辑器
- `Scripts` 页面脚本编辑器

第一类问题——**标准编辑命令整体失效**：

- `Cmd+A` / `Ctrl+A` 无法全选。
- `Cmd+C / V / X` 剪贴板行为异常。
- `Cmd+Z / Shift+Cmd+Z` 撤销重做链路不可用。
- macOS 原生 `Edit` 菜单里的 `Undo / Redo / Cut / Copy / Paste / Select All` 项灰态。

第二类问题——**macOS 关闭按钮黄点（`documentEdited`）保存后不消失**：编辑成 `AB` 后黄点亮起，Undo 回到 `A`，再执行保存，Web 侧保存状态清理，但原生黄点仍然停留。

本方案在**共享 helper** 中统一接入桌面端命令兜底与原生 dirty 状态清理，避免在每个编辑器页面重复补丁。

## 用户目标验证清单

### 必须实现

- 桌面端所有 Monaco 编辑器都能通过 macOS 原生 `Edit` 菜单触发 Undo / Redo / Select All。
- 桌面端所有 Monaco 编辑器保存成功后，macOS 关闭按钮黄点被清理。
- 修复代码统一在共享 helper `web/src/components/MonacoDesktopCommands.ts`，各编辑器只需要一行接入。
- Web 管理端行为完全不变，非桌面壳环境（`__TAURI__` 未定义）下所有桥接调用 no-op。
- Cut / Copy / Paste 由 Tauri 原生 `Edit` 菜单的 `PredefinedMenuItem` 承担，走系统 responder，不需要 helper 转发。

### 必须不破坏

- Web 端 Monaco 保存链路不受影响。
- 每个编辑器页面已有的 `Cmd/Ctrl+S` 显式注册保持工作。
- `BifrostEditor` 封装不在 `editor.create` 重写中强绑桌面态，桌面接入由页面显式调用。
- `Rules` / `Values` / `Scripts` 页面已有的业务保存 store 语义不变。
- 非桌面环境不引入 Tauri 依赖运行时错误。

### 必须真实验证

- macOS 真实桌面：菜单 Undo / Redo / Select All 能作用于当前聚焦 Monaco。
- macOS 真实桌面：编辑 → 保存后关闭按钮黄点消失；Undo 回到原文 → 保存后黄点消失。
- Web 管理端：所有编辑操作与保存链路无回归。
- 非桌面环境静默：无 `invoke is not defined` 或 `__TAURI__` 报错。

## 产品语义

### 桌面菜单是命令源头，Web 端保持 Monaco 默认

本方案的核心决策是：**不在每个 Monaco 实例里逐键 `addCommand`**，而是让 macOS 原生 `Edit` 菜单成为命令源头，通过 Rust `on_menu_event` → `webview.eval` 派发 DOM CustomEvent → 前端 helper 路由到当前/最近聚焦的 Monaco 实例。

好处：

- 命令注册收敛在 helper，编辑器数量增长时无重复维护成本。
- 命令语义与原生菜单一致，避免快捷键 vs. 菜单双通道竞态。
- Cut / Copy / Paste 交由 `PredefinedMenuItem` 承担系统标准 responder 行为，与 macOS 原生输入体验一致。

### helper 只桥接 Undo / Redo / Select All

`MonacoDesktopCommands` 只处理这三类命令，映射如下：

| CustomEvent detail | Monaco action                | DOM 兜底                       |
| ------------------ | ---------------------------- | ------------------------------ |
| `edit-undo`        | `undo`                       | `document.execCommand('undo')` |
| `edit-redo`        | `redo`                       | `document.execCommand('redo')` |
| `edit-select-all`  | `editor.action.selectAll`    | `document.execCommand('selectAll')` |

DOM 兜底用于在焦点被短暂抢走或路由失败时至少保持 web 层可用。

### 焦点追踪：跟随“最近聚焦编辑器”

菜单激活时焦点会被原生菜单短暂抢走，无法用 `document.activeElement` 直接判断当前 Monaco。helper 通过 `onDidFocusEditorText` 记录 last-focused editor，`onDidDispose` 时清理。

### 原生 dirty 状态清理只在保存路径触发

保存链路是明确的清理时机；Undo 回到原文是否消除黄点，交给原生窗口决定，不由前端主动清理，避免误覆盖用户预期。

## 技术细节

### 前端 helper

`web/src/components/MonacoDesktopCommands.ts` 导出两个 API：

```ts
export function initDesktopEditEventListener(): void;
export function registerDesktopMonacoCommands(
  editor: monaco.editor.IStandaloneCodeEditor,
  isDesktop: boolean,
): void;
```

- `initDesktopEditEventListener()`：应用启动一次性安装 `window.addEventListener("bifrost-edit-command", ...)`，把 detail 中的命令名派发给当前/最近聚焦编辑器；Web 模式下也可以安装（listener 本身不会触发），或由调用方跳过。
- `registerDesktopMonacoCommands(editor, isDesktop)`：注册编辑器 focus/dispose hook，把该 editor 加入 last-focused 追踪集合。若 `isDesktop === false` 直接 no-op。

### Rust 侧菜单与事件派发

`desktop/src-tauri/src/main.rs`：

- 注册原生 `Edit` 菜单项：`edit-undo`、`edit-redo`、`edit-select-all`（自定义 MenuItem）+ Cut/Copy/Paste（`PredefinedMenuItem`）。
- `on_menu_event` 内匹配 `edit-undo/edit-redo/edit-select-all`，通过 `webview.eval()` 派发 `bifrost-edit-command` CustomEvent：

  ```js
  window.dispatchEvent(new CustomEvent('bifrost-edit-command', { detail: 'edit-undo' }));
  ```

- Cut/Copy/Paste 无需 `on_menu_event` 处理。

### 原生 `documentEdited` 桥接

- Tauri 命令 `set_document_edited(edited: bool)`：macOS 主线程上调用 `NSWindow::setDocumentEdited(edited)`（通过 `objc2_app_kit::NSWindow`），非 macOS 平台 no-op。
- 前端桥接 `web/src/desktop/tauri.ts`：
  - `setDesktopDocumentEdited(edited: boolean)`：invoke。
  - `clearDesktopDocumentEdited()`：等价 `setDesktopDocumentEdited(false)`。
- 所有调用以 `.catch(() => undefined)` 兜底，非桌面环境静默 no-op。

### 接入点

- 应用启动：`web/src/App.tsx` 中 `useEffect` 调用 `initDesktopEditEventListener()`。
- 编辑器创建后调用 `registerDesktopMonacoCommands(editor, isDesktopShell())`：
  - `web/src/pages/Rules/RuleEditor/index.tsx`
  - `web/src/pages/Values/ValueEditor/index.tsx`
  - `web/src/pages/Scripts/index.tsx`
- 保存成功路径调用 `clearDesktopDocumentEdited()`：
  - `web/src/stores/useRulesStore.ts`（规则保存、批量保存、规则删除成功分支）
  - `web/src/stores/useValuesStore.ts`（值保存成功分支）
  - `web/src/pages/Scripts/index.tsx`（脚本保存、新建成功分支）
- `web/src/components/BifrostEditor/index.ts` **不**在 `editor.create` 重写中调用 helper，由各页面显式接入。

## CLI / Admin API / Sync 边界

- 无 CLI 变更。
- 无 Admin API 变更。
- 无 Sync 边界变更。所有能力都在桌面壳层，不进入 rules/values/scripts 数据同步链路。

## 实现切分

### Phase 1：共享 helper 与命令桥接

- 新增 `web/src/components/MonacoDesktopCommands.ts` 与单元测试。
- 在 `Rules / Values / Scripts` 编辑器创建后接入 `registerDesktopMonacoCommands`。
- `App.tsx` 启动调用 `initDesktopEditEventListener`。
- Rust 侧 `on_menu_event` 派发 `bifrost-edit-command` CustomEvent。

### Phase 2：原生黄点清理

- Tauri 命令 `set_document_edited` 实现（macOS 生效，其它 no-op）。
- 前端桥接 `setDesktopDocumentEdited` / `clearDesktopDocumentEdited`。
- Store 保存成功分支接入 `clearDesktopDocumentEdited()` + `.catch(() => undefined)` 兜底。

### Phase 3：测试与文档

- 新增 `web/src/components/MonacoDesktopCommands.test.ts` 与 `web/src/stores/useRulesStore.test.ts`。
- 更新 `human_tests/webui-rules.md`、`webui-values.md`、`webui-scripts.md`、`readme.md`。
- 更新本设计文档。

### Phase 4：观察与回归护栏

- Vitest + jsdom 覆盖桌面/非桌面两种模式。
- Web UI Playwright 保留 Rules/Values/Scripts 保存链路回归。
- 桌面真实菜单验证由 human_tests 承担。

## 测试方案

### 单元测试

Vitest + jsdom，覆盖：

- 桌面模式下 `registerDesktopMonacoCommands` 注册全部预期 focus/dispose hook。
- Web 模式下 `registerDesktopMonacoCommands(editor, false)` 完全 no-op。
- `bifrost-edit-command` 事件按 detail 派发到 last-focused editor 的正确 Monaco action。
- 无 last-focused editor 时 DOM 兜底 `document.execCommand` 被调用。
- `useRulesStore` 保存成功分支调用 `clearDesktopDocumentEdited()`；非桌面环境不影响保存链路。

关键测试文件：

- `web/src/components/MonacoDesktopCommands.test.ts`
- `web/src/stores/useRulesStore.test.ts`

### E2E 测试

现有 Playwright Web 套件保留：

- `web/tests/ui/admin-rules-values.spec.ts`：验证 Rules / Values 保存链路未回归。
- `web/tests/ui/admin-scripts.spec.ts`：验证 Scripts 保存与新建路径未回归。

桌面原生菜单和 macOS 黄点无 headless E2E，回归依赖 human_tests。

### 真实场景测试 human_tests

必须新增或更新用例：

- `human_tests/webui-rules.md`：TC-WR-MonacoDesktop-01/02，验证 Rules 编辑器桌面端 Cmd+A、Cmd+Z/Shift+Cmd+Z、菜单 Undo/Redo/Select All、保存后黄点消失、Undo 回到原文再保存后黄点消失。
- `human_tests/webui-values.md`：TC-WV-MonacoDesktop-01/02，对 Values 编辑器覆盖相同回归。
- `human_tests/webui-scripts.md`：TC-WS-MonacoDesktop-01/02，对 Scripts 编辑器覆盖相同回归。
- `human_tests/readme.md`：同步用例总数与索引。

所有真实场景测试必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

### 覆盖率与项目校验

- `pnpm --dir web exec vitest run web/src/components/MonacoDesktopCommands.test.ts`
- `pnpm --dir web exec vitest run web/src/stores/useRulesStore.test.ts`
- `pnpm --dir web run test:ui -- admin-rules-values admin-scripts`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`

本机 no-local-coverage 约定下不跑 `make coverage`；在交付备注中说明依赖远端 CI 与 human_tests 覆盖。

## 常见问题排查

- **菜单 Undo 触发后没有反应**：检查 `initDesktopEditEventListener()` 是否已在 `App.tsx` 启动阶段调用；`webview.eval()` 是否成功派发 CustomEvent；`registerDesktopMonacoCommands(editor, isDesktopShell())` 是否在编辑器创建后被调用。
- **保存后黄点仍在**：确认 `useRulesStore` / `useValuesStore` / `Scripts` 保存成功分支是否调用 `clearDesktopDocumentEdited()`；确认 `set_document_edited` Tauri 命令是否被注册；确认非桌面环境（`__TAURI__` 未定义）不会误 throw。
- **Web 环境报错 `invoke is not defined`**：`clearDesktopDocumentEdited().catch(() => undefined)` 兜底应吞掉；如报错说明未加 `.catch`，需要修复接入点。
- **Cut/Copy/Paste 不生效**：确认菜单项是 `PredefinedMenuItem`，走系统 responder；helper 不应也不需要为其转发。
- **多编辑器 tab 快速切换后菜单命令路由错**：last-focused 追踪存在偶发误路由，可接受；如高频复现需要引入 focus timestamp 比较。

## 已知问题与后续演进

- Undo 后是否清理黄点：当前方案交给原生窗口决定，未来若产品要求“内容等于最初 loaded 内容时黄点自动消失”，需要在 store 层引入 baseline 比较，并扩展 `set_document_edited` 语义或新增 `document_content_equals_baseline` API。
- `document.execCommand` 已在 Web 标准中过时，仅作为兜底；未来 Chromium/Webkit 若彻底移除，需要改为 Monaco action 内部实现或 IPC 直接指令。
- 若增加更多桌面命令（例如 Find/Replace 菜单接入），可复用 `bifrost-edit-command` CustomEvent 通道，扩展 detail 值。
- 若未来 Windows / Linux 也提供 macOS 类似的原生 documentEdited 指示，需要扩展 `set_document_edited` 到相应平台。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：三个编辑器菜单 Undo/Redo/Select All 可用、保存后黄点清理。
- 复核 diff：helper、页面接入、store 接入、Rust 菜单事件、Tauri 命令。
- 重点 review：last-focused 追踪是否有 leak（`onDidDispose` 是否解绑）；非桌面环境是否有静默 no-op；BifrostEditor 是否被误改成自动接入。
- 复测：Vitest 单测 + 桌面手工 Cmd+A/Z/Shift+Z + 保存黄点观察。

### 第 2 轮

- 复核第 1 轮发现问题的修复。
- 再次核对 `git status --short`、新增测试文件、human_tests 索引数量。
- 重点 review：批量保存路径是否漏接入；快捷键与菜单双通道是否重复执行 Undo；Windows / Linux 侧 no-op 是否有异常。
- 复测：Web 端 Playwright + 桌面 macOS 真实回归 human_tests 全部用例。

## 已实施状态记录（截至 2026-06-16）

已完成实施：

- 桌面端 Monaco 编辑命令链路
  - 共享 helper `web/src/components/MonacoDesktopCommands.ts`（含 `initDesktopEditEventListener` + `registerDesktopMonacoCommands`）。
  - 接入 `Rules / Values / Scripts` 三个编辑器；`App.tsx` 启动时安装事件监听。
  - Rust 侧 `on_menu_event` 经 `webview.eval()` 派发 `bifrost-edit-command` CustomEvent，前端转发到聚焦/最近聚焦的 Monaco 实例。
  - 单测 `web/src/components/MonacoDesktopCommands.test.ts`；Vitest + jsdom 已配置。
- 保存后原生 `documentEdited` 清理
  - Tauri 命令 `set_document_edited`（macOS 调用 `NSWindow::setDocumentEdited`）。
  - 前端桥接 `clearDesktopDocumentEdited()`；接入点：
    - `web/src/stores/useRulesStore.ts`（规则保存 / 批量保存路径）。
    - `web/src/stores/useValuesStore.ts`（值保存路径）。
    - `web/src/pages/Scripts/index.tsx`（脚本保存 / 新建路径）。
  - 单测 `web/src/stores/useRulesStore.test.ts`。

## 依赖项

- `web/src/components/BifrostEditor/index.ts`
- `web/src/pages/Rules/RuleEditor/index.tsx`
- `web/src/pages/Values/ValueEditor/index.tsx`
- `web/src/pages/Scripts/index.tsx`
- `web/src/components/MonacoDesktopCommands.ts`（新增）
- `web/src/components/MonacoDesktopCommands.test.ts`（新增）
- `web/src/stores/useRulesStore.test.ts`（新增）
- `web/src/desktop/tauri.ts`
- `desktop/src-tauri/src/main.rs`
- `web/tests/ui/admin-rules-values.spec.ts`
- `web/tests/ui/admin-scripts.spec.ts`
- `human_tests/webui-rules.md`
- `human_tests/webui-values.md`
- `human_tests/webui-scripts.md`
- `human_tests/readme.md`

## 文档更新要求

- 无需更新 `README.md`。
- 必须更新：
  - `human_tests/webui-rules.md`
  - `human_tests/webui-values.md`
  - `human_tests/webui-scripts.md`
  - `human_tests/readme.md`
  - 本设计文档

## 风险与决策点

- Undo 后黄点是否清理：本方案不主动清理 Undo 场景，交由原生窗口决定；如果产品要求“内容等于最初 loaded 内容时黄点应消失”，需要额外引入 baseline 比较，并明确 API 变更。
- Cut/Copy/Paste 未走 CustomEvent：依赖 `PredefinedMenuItem` 与系统 responder，跨版本 Tauri 升级时需要 verify 是否仍然生效。
- last-focused 追踪并发：多编辑器 tab 快速切换时可能出现 focus race，helper 采用最近 focus 优先，可接受偶发误路由。
- `document.execCommand` 已经在标准中被标记为过时；作为兜底可接受，但不能作为主链路，Monaco action 应优先。
- 桌面壳层升级 Tauri 版本时需要 verify `webview.eval()` 与 CustomEvent 派发行为不变。
