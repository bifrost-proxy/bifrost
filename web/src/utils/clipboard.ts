import { isDesktopShell } from '../runtime';

async function nativeClipboardWrite(text: string): Promise<boolean> {
  const invoke = window.__TAURI__?.core?.invoke;
  if (!invoke) return false;
  try {
    await invoke('write_clipboard', { text });
    return true;
  } catch {
    return false;
  }
}

/**
 * Synchronous clipboard write via the deprecated execCommand API.
 *
 * When a modal / dialog is open its focus-trap prevents a textarea appended
 * to `document.body` from receiving focus.  We therefore detect the nearest
 * dialog container from `document.activeElement` and insert there instead.
 */
function execCommandCopy(text: string): boolean {
  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.left = '0';
  textarea.style.top = '0';
  textarea.style.width = '1px';
  textarea.style.height = '1px';
  textarea.style.padding = '0';
  textarea.style.border = 'none';
  textarea.style.outline = 'none';
  textarea.style.boxShadow = 'none';
  textarea.style.background = 'transparent';
  textarea.style.opacity = '0';

  // Prefer inserting inside the active modal to bypass its focus-trap.
  const container =
    document.activeElement?.closest('.ant-modal-content') ??
    document.activeElement?.closest('[role="dialog"]') ??
    document.body;

  container.appendChild(textarea);
  textarea.focus();
  textarea.select();
  try {
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    container.removeChild(textarea);
  }
}

export async function copyToClipboard(text: string): Promise<boolean> {
  if (isDesktopShell()) {
    return nativeClipboardWrite(text);
  }

  // 1. Async Clipboard API – works in secure contexts (HTTPS / localhost).
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Permission denied or insecure context – fall through.
    }
  }

  // 2. Synchronous execCommand fallback (modal-aware).
  return execCommandCopy(text);
}
