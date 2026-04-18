import { describe, expect, it, vi } from "vitest";
import { registerDesktopMonacoCommands } from "./MonacoDesktopCommands";

function createMockEditor() {
  return {
    trigger: vi.fn(),
    onDidDispose: vi.fn(),
    onDidFocusEditorText: vi.fn(),
    hasTextFocus: vi.fn(() => false),
    focus: vi.fn(),
  } as unknown as Parameters<typeof registerDesktopMonacoCommands>[0];
}

describe("registerDesktopMonacoCommands", () => {
  it("tracks editor focus and disposal", () => {
    const editor = createMockEditor();
    registerDesktopMonacoCommands(editor, true);

    const onDispose = editor.onDidDispose as ReturnType<typeof vi.fn>;
    const onFocus = editor.onDidFocusEditorText as ReturnType<typeof vi.fn>;
    expect(onDispose).toHaveBeenCalledTimes(1);
    expect(onFocus).toHaveBeenCalledTimes(1);
  });

  it("skips registration outside desktop mode", () => {
    const editor = createMockEditor();
    registerDesktopMonacoCommands(editor, false);

    const onDispose = editor.onDidDispose as ReturnType<typeof vi.fn>;
    const onFocus = editor.onDidFocusEditorText as ReturnType<typeof vi.fn>;
    expect(onDispose).not.toHaveBeenCalled();
    expect(onFocus).not.toHaveBeenCalled();
  });
});
