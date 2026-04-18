export interface DesktopRuntimeInfo {
  expectedProxyPort: number;
  proxyPort: number;
  platform: string;
  startupReady: boolean;
  startupError: string | null;
}

export const DESKTOP_HANDOFF_COMPLETE_EVENT = "desktop://handoff-complete";

type TauriInvoke = <T>(
  cmd: string,
  args?: Record<string, unknown>,
) => Promise<T>;

type TauriWindowHandle = {
  startDragging(): Promise<void>;
  toggleMaximize(): Promise<void>;
  minimize(): Promise<void>;
  close(): Promise<void>;
  isMaximized(): Promise<boolean>;
};

type TauriEvent = {
  event: string;
  id: number;
  payload: unknown;
};

type TauriEventUnlisten = () => void | Promise<void>;

type TauriEventApi = {
  listen(
    event: string,
    handler: (event: TauriEvent) => void,
  ): Promise<TauriEventUnlisten>;
};

declare global {
  interface Window {
    __TAURI__?: {
      core?: {
        invoke: TauriInvoke;
      };
      event?: TauriEventApi;
      webviewWindow?: {
        getCurrentWebviewWindow(): TauriWindowHandle;
      };
    };
  }
}

let cachedInvoke: TauriInvoke | null = null;
let cachedWindowHandle: TauriWindowHandle | null = null;

function getCurrentInvoke(): TauriInvoke | null {
  const invoke = window.__TAURI__?.core?.invoke;
  if (invoke) {
    cachedInvoke = invoke;
    return invoke;
  }

  return cachedInvoke;
}

export async function invokeDesktop<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const invoke = getCurrentInvoke();
  if (!invoke) {
    console.error(
      "[desktop-runtime] Tauri invoke bridge is unavailable. Check tauri.conf.json app.withGlobalTauri and desktop runtime injection.",
    );
    throw new Error("Tauri API is not available");
  }
  return invoke<T>(command, args);
}

export async function getDesktopRuntime(): Promise<DesktopRuntimeInfo> {
  return invokeDesktop<DesktopRuntimeInfo>("get_desktop_runtime");
}

export async function updateDesktopProxyPort(
  port: number,
): Promise<DesktopRuntimeInfo> {
  return invokeDesktop<DesktopRuntimeInfo>("update_desktop_proxy_port", {
    port,
  });
}

export async function notifyMainWindowReady(): Promise<void> {
  await invokeDesktop<void>("notify_main_window_ready");
}

export async function setDesktopDocumentEdited(edited: boolean): Promise<void> {
  const invoke = getCurrentInvoke();
  if (!invoke) {
    return;
  }

  await invoke<void>("set_document_edited", { edited });
}

export async function clearDesktopDocumentEdited(): Promise<void> {
  await setDesktopDocumentEdited(false);
}

export async function listenDesktopEvent(
  event: string,
  handler: (event: TauriEvent) => void,
): Promise<TauriEventUnlisten> {
  const eventApi = window.__TAURI__?.event;
  if (!eventApi) {
    throw new Error("Tauri event API is not available");
  }

  return eventApi.listen(event, handler);
}

export function getCurrentDesktopWindow() {
  const currentWindow = window.__TAURI__?.webviewWindow?.getCurrentWebviewWindow();
  if (currentWindow) {
    cachedWindowHandle = currentWindow;
    return currentWindow;
  }

  return cachedWindowHandle;
}
