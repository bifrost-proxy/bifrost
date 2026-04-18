# Desktop Monaco Edit Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore standard edit shortcuts in all desktop Monaco editors so Rules, Values, and Scripts editors work inside the Tauri desktop shell.

**Architecture:** Fix the issue once at the Monaco integration layer instead of patching each page separately. Add a small shared registration helper that explicitly binds desktop edit commands to Monaco actions, then wire every editor initialization path through it and verify with unit tests plus desktop human-tests.

**Tech Stack:** React 19, TypeScript, Monaco Editor, Tauri desktop shell, Playwright, Vitest

---

### Task 1: Create a Shared Desktop Monaco Command Helper

**Files:**
- Create: `web/src/components/MonacoDesktopCommands.ts`
- Create: `web/src/components/MonacoDesktopCommands.test.ts`
- Modify: `web/package.json`

- [ ] **Step 1: Add a web unit-test script and Vitest dependency**

```json
{
  "scripts": {
    "test:unit": "vitest run"
  },
  "devDependencies": {
    "vitest": "^4.1.2"
  }
}
```

- [ ] **Step 2: Write the failing unit tests for command registration**

```ts
import { describe, expect, it, vi } from "vitest";
import { KeyCode, KeyMod } from "monaco-editor";
import {
  getDesktopMonacoCommandBindings,
  registerDesktopMonacoCommands,
} from "./MonacoDesktopCommands";

describe("getDesktopMonacoCommandBindings", () => {
  it("includes select all, clipboard, undo, and redo bindings", () => {
    const bindings = getDesktopMonacoCommandBindings();

    expect(bindings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          keybinding: KeyMod.CtrlCmd | KeyCode.KeyA,
          actionId: "editor.action.selectAll",
        }),
        expect.objectContaining({
          keybinding: KeyMod.CtrlCmd | KeyCode.KeyC,
          actionId: "editor.action.clipboardCopyAction",
        }),
        expect.objectContaining({
          keybinding: KeyMod.CtrlCmd | KeyCode.KeyV,
          actionId: "editor.action.clipboardPasteAction",
        }),
        expect.objectContaining({
          keybinding: KeyMod.CtrlCmd | KeyCode.KeyX,
          actionId: "editor.action.clipboardCutAction",
        }),
        expect.objectContaining({
          keybinding: KeyMod.CtrlCmd | KeyCode.KeyZ,
          actionId: "undo",
        }),
        expect.objectContaining({
          keybinding: KeyMod.CtrlCmd | KeyMod.Shift | KeyCode.KeyZ,
          actionId: "redo",
        }),
      ]),
    );
  });
});

describe("registerDesktopMonacoCommands", () => {
  it("registers every desktop binding by calling addCommand", () => {
    const addCommand = vi.fn();
    const trigger = vi.fn();
    const editor = { addCommand, trigger } as any;

    registerDesktopMonacoCommands(editor, true);

    expect(addCommand).toHaveBeenCalledTimes(6);
  });

  it("skips registration outside desktop mode", () => {
    const addCommand = vi.fn();
    const trigger = vi.fn();
    const editor = { addCommand, trigger } as any;

    registerDesktopMonacoCommands(editor, false);

    expect(addCommand).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `pnpm --dir web exec vitest run web/src/components/MonacoDesktopCommands.test.ts`

Expected: FAIL because the helper file does not exist yet.

- [ ] **Step 4: Implement the shared helper**

```ts
import { KeyCode, KeyMod, editor as MonacoEditor } from "monaco-editor";

export interface MonacoDesktopCommandBinding {
  keybinding: number;
  actionId: string;
}

export function getDesktopMonacoCommandBindings(): MonacoDesktopCommandBinding[] {
  return [
    { keybinding: KeyMod.CtrlCmd | KeyCode.KeyA, actionId: "editor.action.selectAll" },
    { keybinding: KeyMod.CtrlCmd | KeyCode.KeyC, actionId: "editor.action.clipboardCopyAction" },
    { keybinding: KeyMod.CtrlCmd | KeyCode.KeyV, actionId: "editor.action.clipboardPasteAction" },
    { keybinding: KeyMod.CtrlCmd | KeyCode.KeyX, actionId: "editor.action.clipboardCutAction" },
    { keybinding: KeyMod.CtrlCmd | KeyCode.KeyZ, actionId: "undo" },
    { keybinding: KeyMod.CtrlCmd | KeyMod.Shift | KeyCode.KeyZ, actionId: "redo" },
  ];
}

export function registerDesktopMonacoCommands(
  editor: MonacoEditor.IStandaloneCodeEditor,
  isDesktop: boolean,
): void {
  if (!isDesktop) return;

  for (const binding of getDesktopMonacoCommandBindings()) {
    editor.addCommand(binding.keybinding, () => {
      editor.trigger("keyboard", binding.actionId, null);
    });
  }
}
```

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `pnpm --dir web exec vitest run web/src/components/MonacoDesktopCommands.test.ts`

Expected: PASS with 3 passing tests.

- [ ] **Step 6: Commit the helper slice**

```bash
git add web/package.json web/src/components/MonacoDesktopCommands.ts web/src/components/MonacoDesktopCommands.test.ts
git commit -m "test: cover desktop monaco command helper"
```

### Task 2: Wire the Shared Helper into Every Monaco Initialization Path

**Files:**
- Modify: `web/src/components/BifrostEditor/index.ts`
- Modify: `web/src/pages/Rules/RuleEditor/index.tsx`
- Modify: `web/src/pages/Values/ValueEditor/index.tsx`
- Modify: `web/src/pages/Scripts/index.tsx`
- Use: `web/src/components/MonacoDesktopCommands.ts`
- Use: `web/src/runtime.ts`

- [ ] **Step 1: Import the shared helper and desktop runtime flag in Rules editor**

```ts
import { isDesktopShell } from "../../../runtime";
import { registerDesktopMonacoCommands } from "../../../components/MonacoDesktopCommands";
```

- [ ] **Step 2: Register desktop commands immediately after editor creation in Rules**

```ts
const ed = BifrostEditor.create(containerElement, {
  theme: editorTheme,
  readOnly: !canEdit,
});

registerDesktopMonacoCommands(ed, isDesktopShell());

ed.setModel(model);
ed.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => {
  handleSave();
});
```

- [ ] **Step 3: Register the same helper in Values editor**

```ts
const ed = MonacoEditor.create(containerElement, {
  value: currentValueRef.current?.currentValue?.value || "",
  language: detectLanguage(currentValueRef.current?.currentValue?.value || ""),
  theme: resolvedTheme === "dark" ? "vs-dark" : "vs",
  minimap: { enabled: false },
  automaticLayout: true,
  scrollBeyondLastLine: false,
  fontSize: 12,
  lineHeight: 20,
  tabSize: 2,
  wordWrap: "on",
  lineNumbers: "on",
  folding: true,
  renderLineHighlight: "all",
});

registerDesktopMonacoCommands(ed, isDesktopShell());
```

- [ ] **Step 4: Register the helper in Scripts editor initialization**

```ts
editorDidMount={(editor, monaco) => {
  registerDesktopMonacoCommands(editor, isDesktopShell());

  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
    void handleSaveScript();
  });
}}
```

- [ ] **Step 5: Run a desktop-targeted build to catch TypeScript regressions**

Run: `pnpm --dir web run build:desktop`

Expected: PASS with no TypeScript or Vite errors.

- [ ] **Step 6: Commit the unified editor wiring**

```bash
git add web/src/components/BifrostEditor/index.ts web/src/pages/Rules/RuleEditor/index.tsx web/src/pages/Values/ValueEditor/index.tsx web/src/pages/Scripts/index.tsx
git commit -m "feat: restore desktop monaco edit shortcuts"
```

### Task 3: Add Focused Regression Coverage Around Editor Initialization

**Files:**
- Modify: `web/tests/ui/admin-rules-values.spec.ts`
- Modify: `web/tests/ui/admin-scripts.spec.ts`

- [ ] **Step 1: Add a focused Rules/Values regression test that keeps save shortcut working**

```ts
test("Rules 编辑器在快捷键兜底接入后仍支持保存", async ({ page, request }) => {
  const ruleName = uniqueName("desktop-editor-save-rule");

  const createRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: "127.0.0.1 reqHeaders://X-Init=before",
    },
  });
  if (!createRuleRes.ok()) throw new Error(await createRuleRes.text());

  await openPage(page, "rules");
  await page.getByTestId("rule-item").filter({ hasText: ruleName }).first().click();

  await setMonacoEditor(page, page.getByTestId("rule-editor-container"), "127.0.0.1 reqHeaders://X-Init=after");
  await page.keyboard.press(process.platform === "darwin" ? "Meta+S" : "Control+S");

  await expect(page.getByText("Saved")).toBeVisible();
});
```

- [ ] **Step 2: Add a focused Scripts regression test that editor mount still works**

```ts
test("Scripts 编辑器接入桌面快捷键兜底后仍可保存脚本", async ({ page }) => {
  const scriptName = uniqueName("desktop-editor-script");

  await openPage(page, "scripts");
  await page.getByRole("button", { name: "New Request" }).click();
  await page.getByPlaceholder("Script name").fill(scriptName);
  await page.getByRole("button", { name: "Create" }).click();

  await setMonacoEditor(
    page,
    page.locator(".monaco-editor").first(),
    'request.headers["x-desktop-editor"] = "ok";',
  );
  await page.keyboard.press(process.platform === "darwin" ? "Meta+S" : "Control+S");

  await expect(page.getByText("Saved")).toBeVisible();
});
```

- [ ] **Step 3: Run the focused tests to verify no web regression**

Run:

```bash
pnpm --dir web exec playwright test web/tests/ui/admin-rules-values.spec.ts -g "Rules 编辑器在快捷键兜底接入后仍支持保存"
pnpm --dir web exec playwright test web/tests/ui/admin-scripts.spec.ts -g "Scripts 编辑器接入桌面快捷键兜底后仍可保存脚本"
```

Expected: PASS.

- [ ] **Step 4: Commit the focused regression coverage**

```bash
git add web/tests/ui/admin-rules-values.spec.ts web/tests/ui/admin-scripts.spec.ts
git commit -m "test: keep editor initialization stable after desktop shortcut fix"
```

### Task 4: Update Human Tests and Execute Them Immediately

**Files:**
- Modify: `human_tests/webui-rules.md`
- Modify: `human_tests/webui-values.md`
- Modify: `human_tests/webui-scripts.md`
- Modify: `human_tests/readme.md`

- [ ] **Step 1: Add a Rules desktop editor regression case**

```md
### TC-WRU-35：桌面端 Rules 编辑器支持基础编辑快捷键

**前置条件**：通过桌面客户端打开 Rules 页面，并已选中一条规则

**操作步骤**：
1. 在右侧编辑器中输入三行文本
2. 按 `Cmd+A`
3. 按 `Cmd+C`
4. 按 `Cmd+X`
5. 按 `Shift+Cmd+Z` 与 `Cmd+Z`

**预期结果**：
- `Cmd+A` 可全选编辑器文本
- `Cmd+C` 可复制选中文本
- `Cmd+X` 可剪切选中文本
- `Cmd+Z` / `Shift+Cmd+Z` 可撤销与重做
```

- [ ] **Step 2: Add parallel Values and Scripts desktop regression cases**

```md
### TC-WVA-19：桌面端 Values 编辑器支持基础编辑快捷键
### TC-WSC-20：桌面端 Scripts 编辑器支持基础编辑快捷键
```

Expected coverage:
- `Cmd+A`
- `Cmd+C / Cmd+V / Cmd+X`
- `Cmd+Z / Shift+Cmd+Z`

- [ ] **Step 3: Update the human-test index counts**

```md
| [webui-rules.md](./webui-rules.md) | Web UI Rules 页面 | 37 | ... 桌面端编辑器快捷键回归 |
| [webui-scripts.md](./webui-scripts.md) | Web UI Scripts 页面 | 20 | ... 桌面端编辑器快捷键回归 |
| [webui-values.md](./webui-values.md) | Web UI Values 页面 | 19 | ... 桌面端编辑器快捷键回归 |
```

- [ ] **Step 4: Execute the human tests immediately after editing**

Run:

```bash
BIFROST_DATA_DIR=$PWD/.bifrost-desktop-editor-select-all pnpm run desktop:dev
```

Manual execution checklist:
- Execute the new Rules desktop shortcut case
- Execute the new Values desktop shortcut case
- Execute the new Scripts desktop shortcut case
- Record actual results for each shortcut family before closing the task

Expected:
- All newly added desktop editor shortcut cases PASS

- [ ] **Step 5: Commit the human-test docs**

```bash
git add human_tests/webui-rules.md human_tests/webui-values.md human_tests/webui-scripts.md human_tests/readme.md
git commit -m "docs: add desktop monaco editor shortcut regressions"
```

### Task 5: Run Validation and Close Out

**Files:**
- Modify: `design/desktop_monaco_edit_commands.md`
- Verify: existing workspace files only

- [ ] **Step 1: Keep the design doc aligned with the shipped implementation**

```md
- Root cause: desktop Monaco editors do not receive standard edit commands reliably via native menu chain
- Scope: all desktop Monaco editors
- Fix: shared helper registers desktop edit command bindings explicitly
```

- [ ] **Step 2: Run required validation commands**

Run:

```bash
pnpm --dir web exec vitest run web/src/components/MonacoDesktopCommands.test.ts
pnpm --dir web exec playwright test web/tests/ui/admin-rules-values.spec.ts -g "Rules 编辑器在快捷键兜底接入后仍支持保存"
pnpm --dir web exec playwright test web/tests/ui/admin-scripts.spec.ts -g "Scripts 编辑器接入桌面快捷键兜底后仍可保存脚本"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
bash scripts/ci/local-ci.sh --skip-e2e
```

Expected:
- All commands PASS

- [ ] **Step 3: Confirm only intentional changes remain**

Run: `git status --short`

Expected:
- Only the planned files are modified

- [ ] **Step 4: Commit the final integrated change**

```bash
git add design/desktop_monaco_edit_commands.md
git commit -m "fix: restore desktop monaco edit commands"
```
