# 桌面端 Monaco 编辑命令与文档脏状态统一修复方案

## 功能模块详细描述

当前 Bifrost 桌面端（Tauri + macOS WebView）中，所有基于 Monaco 的编辑器都存在同一类运行时问题，并伴随一个保存后原生窗口状态不同步的问题：

- `Rules` 页面右侧规则编辑器
- `Values` 页面右侧值编辑器
- `Scripts` 页面脚本编辑器

第一类问题是标准编辑命令整体失效，表现为：

- `Cmd+A` / `Ctrl+A` 无法全选文本
- `Cmd+C / Cmd+V / Cmd+X` 无法按预期执行
- `Cmd+Z / Shift+Cmd+Z` 等撤销重做链路不可用
- macOS 原生 `Edit` 菜单中的 `Undo / Redo / Cut / Copy / Paste / Select All` 处于灰态

第二类问题是 macOS 窗口左上角关闭按钮黄点（`documentEdited`）在保存后不消失，典型复现为：

- 当前内容为 `A`
- 编辑成 `AB` 后黄点出现
- 执行 Undo 后内容回到 `A`，黄点仍保留
- 此时执行保存，黄点不消失

同样的前端代码在 Web 管理端正常，因此问题边界已经收敛到桌面端运行时，而不是单个页面的业务逻辑。

## 现状与根因

### 已确认现象

1. Web 管理端中的 Monaco 编辑器工作正常。
2. 桌面客户端中 `Rules / Values / Scripts` 的编辑器都复现同样问题。
3. 原生 `Edit` 菜单整组编辑命令灰掉，说明问题不是单独某个快捷键没注册，而是桌面端编辑命令链路未接通。
4. 保存成功后，Web 侧保存状态已恢复，但 macOS 原生黄点没有同步消失。

### 现状代码路径

- `web/src/components/BifrostEditor/index.ts`
  - 封装了 Rules 页面使用的 Monaco 初始化
  - 当前只显式添加了少量自定义行为，没有统一注册桌面端编辑命令兜底
- `web/src/pages/Rules/RuleEditor/index.tsx`
  - 显式注册了 `Cmd/Ctrl+S`
- `web/src/pages/Values/ValueEditor/index.tsx`
  - 显式注册了 `Cmd/Ctrl+S`
- `web/src/pages/Scripts/index.tsx`
  - 显式注册了 `Cmd/Ctrl+S`
- `desktop/src-tauri/src/main.rs`
  - 创建桌面 WebView，但没有为 Monaco 编辑命令提供额外桥接或桌面端兜底

### 根因判断

#### 问题一：编辑命令链路

基于当前证据，根因优先级如下：

1. 桌面端 WebView 中，Monaco 没有正确接入 macOS 原生 responder / Edit 菜单链路。
2. 项目当前只对 `Cmd/Ctrl+S` 这类业务命令做了显式注册，而把标准编辑命令完全交给 Monaco 默认行为。
3. Monaco 默认行为在 Web 端可用，在 Tauri 桌面壳中不可依赖，因此需要在编辑器初始化时统一补一层桌面端命令兜底。

本次修复先不追求让 macOS 原生 `Edit` 菜单即时恢复高亮，而是优先恢复用户实际可用的编辑快捷键能力。

#### 问题二：保存后原生文档脏状态未清理

1. Web 侧的未保存状态由页面 store 中的 `editingContent` / `savedContent` 或编辑内容本身驱动。
2. 保存成功后，Web 侧状态会被清理，因此列表未保存圆点、保存按钮状态本身可以恢复。
3. 但桌面端并没有在保存成功后通知原生窗口将 `documentEdited` 置回 `false`。
4. 因此会出现“业务状态已保存，但原生窗口黄点仍保留”的状态分裂。

## 方案边界

- 修复范围：桌面端所有 Monaco 编辑器
- 不修改普通 Web 管理端行为
- 不在页面层分别打补丁，而是抽到共享注册逻辑中统一接入
- 第一阶段目标：让快捷键实际可用
- 第二阶段目标：让保存成功后原生窗口黄点与业务保存状态保持一致

## 实施状态

已完成实施：
- Root cause: desktop Monaco editors do not receive standard edit commands reliably via native menu chain
- Scope: all desktop Monaco editors
- Fix: shared helper registers desktop edit command bindings explicitly
- Files created: `web/src/components/MonacoDesktopCommands.ts`, `web/src/components/MonacoDesktopCommands.test.ts`
- Wired into: Rules, Values, Scripts editors
- Vitest + jsdom configured for web unit tests

待完成实施：
- Root cause: macOS window `documentEdited` state is not cleared after successful save
- Scope: desktop Rules / Values / Scripts save flows
- Fix direction: add a desktop bridge command to clear native `documentEdited` after save succeeds

## 实现逻辑

### 统一修复策略

新增一层 Monaco 命令注册 helper，在桌面端编辑器初始化时显式注册下列命令：

- `Cmd/Ctrl+A` -> `editor.action.selectAll`
- `Cmd/Ctrl+C` -> `editor.action.clipboardCopyAction`
- `Cmd/Ctrl+V` -> `editor.action.clipboardPasteAction`
- `Cmd/Ctrl+X` -> `editor.action.clipboardCutAction`
- `Cmd/Ctrl+Z` -> `undo`
- `Shift+Cmd/Ctrl+Z` -> `redo`

### 接入方式

1. 新增共享 helper，例如 `web/src/components/MonacoDesktopCommands.ts`
2. 在所有 Monaco 编辑器初始化处统一调用：
   - `web/src/components/BifrostEditor/index.ts`
   - `web/src/pages/Values/ValueEditor/index.tsx`
   - `web/src/pages/Scripts/index.tsx`
3. helper 内部用 `isDesktopShell()` 保护，避免影响 Web 端默认行为

### 为什么要抽共享 helper

因为当前问题不是页面业务问题，而是“桌面端 Monaco 共性运行时问题”。如果在 `Rules / Values / Scripts` 各自修，会造成：

- 命令注册重复
- 后续遗漏新的编辑器入口
- 无法保证桌面端行为统一

统一 helper 才符合当前问题的真实边界。

### 文档脏状态修复策略

新增一层 desktop runtime helper，在桌面端保存成功后显式调用 Tauri command：

- Web -> `invokeDesktop("set_document_edited", { edited: false })`
- Tauri macOS -> `WindowExtMacOS::set_is_document_edited(false)`

接入点遵循“保存成功后立即清理原生状态”的原则：

1. `Rules` 保存成功后清理
2. `Values` 保存成功后清理
3. `Scripts` 保存/新建成功后清理

这样可以保证：

- Web 行为完全不变
- 桌面端黄点和“保存成功”状态保持一致
- Undo 后是否仍保留黄点继续交给原生窗口/编辑器行为决定，保存是唯一明确的清理时机

## 依赖项

- `web/src/components/BifrostEditor/index.ts`
- `web/src/pages/Rules/RuleEditor/index.tsx`
- `web/src/pages/Values/ValueEditor/index.tsx`
- `web/src/pages/Scripts/index.tsx`
- 新增：`web/src/components/MonacoDesktopCommands.ts`
- 新增：`web/src/components/MonacoDesktopCommands.test.ts`
- 新增：`web/src/stores/useRulesStore.test.ts`
- `web/src/desktop/tauri.ts`
- `desktop/src-tauri/src/main.rs`
- `web/tests/ui/admin-rules-values.spec.ts`
- `web/tests/ui/admin-scripts.spec.ts`
- `human_tests/webui-rules.md`
- `human_tests/webui-values.md`
- `human_tests/webui-scripts.md`
- `human_tests/readme.md`

## 测试方案

### 单元测试

为共享 helper 与 desktop dirty-state 清理链路编写测试，覆盖：

- 桌面端模式下会注册全部预期命令
- Web 模式下不会额外注册桌面端兜底命令
- `Cmd/Ctrl+A / C / V / X / Z / Shift+Cmd/Ctrl+Z` 与目标 action 映射正确
- Rules 保存成功后会调用 desktop runtime 清理原生 `documentEdited`
- 非桌面环境下清理 helper 不应影响现有保存逻辑

### E2E 测试

由于当前 UI 自动化主要跑 Web 管理端，而问题仅出现在桌面壳层，因此本次 E2E 目标分两部分：

1. 对共享 helper 做最小单测，保证映射行为不回归
2. 保持现有 Web UI E2E 不受影响，并补一个最小回归验证：
   - `Rules` 编辑器快捷键注册逻辑仍不影响保存快捷键
   - `Scripts` / `Values` 页编辑器初始化路径不报错

说明：桌面端原生命令链路目前不适合直接用现有 Playwright Web 套件验证，因此核心回归依赖 human_tests。

本次新增回归重点：

1. Web UI 自动化继续验证 Rules / Values / Scripts 保存链路未回归
2. 原生窗口黄点清理依赖 human_tests 在真实桌面壳中验证

### 真实场景测试（human_tests）

必须新增或更新以下文档：

- `human_tests/webui-rules.md`
- `human_tests/webui-values.md`
- `human_tests/webui-scripts.md`

新增桌面端专用回归用例，覆盖：

- `Cmd+A` 文本全选
- `Cmd+C / Cmd+V / Cmd+X`
- `Cmd+Z / Shift+Cmd+Z`
- 编辑后保存，macOS 窗口黄点消失
- Undo 回到原文后保存，macOS 窗口黄点消失
- Web 端不受影响

同步更新 `human_tests/readme.md` 的测试用例数量与说明。

## 校验要求

实现完成后按以下顺序验证：

1. `pnpm --dir web exec vitest run web/src/components/MonacoDesktopCommands.test.ts`
2. 定向运行受影响的 Web UI 测试，确保无回归
3. 使用桌面端开发链路手工执行 human_tests 新增用例
4. `cargo fmt --all -- --check`
5. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
6. `cargo test --workspace --all-features`
7. `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 无需更新 `README.md`
- 必须更新：
  - `human_tests/webui-rules.md`
  - `human_tests/webui-values.md`
  - `human_tests/webui-scripts.md`
  - `human_tests/readme.md`
  - 本设计文档
