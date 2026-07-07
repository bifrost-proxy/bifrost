import { afterEach, describe, expect, it, vi } from "vitest";

function windowHandle(label: string) {
  return {
    label,
    startDragging: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
    toggleMaximize: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
    minimize: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
    close: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
    isMaximized: vi.fn<() => Promise<boolean>>().mockResolvedValue(false),
  };
}

describe("desktop tauri window bridge", () => {
  afterEach(() => {
    vi.resetModules();
    delete window.__TAURI__;
  });

  it("uses the official window namespace for current-window controls", async () => {
    const officialWindow = windowHandle("window");
    const webviewWindow = windowHandle("webviewWindow");
    window.__TAURI__ = {
      window: {
        getCurrentWindow: () => officialWindow,
      },
      webviewWindow: {
        getCurrentWebviewWindow: () => webviewWindow,
      },
    };

    const { getCurrentDesktopWindow } = await import("./tauri");

    expect(getCurrentDesktopWindow()).toBe(officialWindow);
  });

  it("falls back to the webview window namespace for older runtime bridges", async () => {
    const webviewWindow = windowHandle("webviewWindow");
    window.__TAURI__ = {
      webviewWindow: {
        getCurrentWebviewWindow: () => webviewWindow,
      },
    };

    const { getCurrentDesktopWindow } = await import("./tauri");

    expect(getCurrentDesktopWindow()).toBe(webviewWindow);
  });
});
