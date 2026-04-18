import { editor as MonacoEditor } from "monaco-editor";

// ---------- Global edit command dispatch for Tauri native menu ----------
// The Rust side calls webview.eval() to dispatch a DOM CustomEvent named
// "bifrost-edit-command" when the user triggers Undo/Redo/SelectAll from
// the macOS native Edit menu. We listen for that CustomEvent here and
// forward the action to the focused (or last-focused) Monaco editor.

const activeEditors = new Set<MonacoEditor.IStandaloneCodeEditor>();

// Track the most recently focused editor so we can still dispatch actions
// when the native menu activation briefly steals focus from the WebView.
let lastFocusedEditor: MonacoEditor.IStandaloneCodeEditor | null = null;

// Map Monaco action ids → document.execCommand names (fallback for non-Monaco inputs)
const EXEC_COMMAND_FALLBACK: Record<string, string> = {
  "editor.action.selectAll": "selectAll",
  undo: "undo",
  redo: "redo",
};

function dispatchEditAction(action: string) {
  // Try focused Monaco editor first
  for (const editor of activeEditors) {
    if (editor.hasTextFocus()) {
      editor.trigger("menu", action, null);
      return;
    }
  }
  // Fallback: the native menu activation may have stolen focus, so use
  // the last editor that had focus and re-focus it before triggering.
  if (lastFocusedEditor && activeEditors.has(lastFocusedEditor)) {
    lastFocusedEditor.focus();
    lastFocusedEditor.trigger("menu", action, null);
    return;
  }
  // DOM fallback for regular (non-Monaco) inputs
  const cmd = EXEC_COMMAND_FALLBACK[action];
  if (cmd) document.execCommand(cmd);
}

let editEventListenerInstalled = false;

/**
 * Call once at app startup (e.g. in App.tsx) to start listening for
 * the DOM CustomEvent dispatched by the Rust-side webview.eval().
 */
export function initDesktopEditEventListener() {
  if (editEventListenerInstalled) return;
  editEventListenerInstalled = true;

  // Listen for the DOM CustomEvent that the Rust on_menu_event handler
  // dispatches via webview.eval(). This is more reliable than the Tauri
  // event system because it doesn't depend on event target routing.
  window.addEventListener("bifrost-edit-command", (e) => {
    const action = (e as CustomEvent).detail as string;
    if (action) dispatchEditAction(action);
  });
}

// ---------- Per-editor registration ----------

export function registerDesktopMonacoCommands(
  editor: MonacoEditor.IStandaloneCodeEditor,
  isDesktop: boolean,
): void {
  if (!isDesktop) return;

  activeEditors.add(editor);
  editor.onDidDispose(() => {
    activeEditors.delete(editor);
    if (lastFocusedEditor === editor) lastFocusedEditor = null;
  });

  // Track editor focus so dispatchEditAction can fall back to it
  editor.onDidFocusEditorText(() => {
    lastFocusedEditor = editor;
  });
}
