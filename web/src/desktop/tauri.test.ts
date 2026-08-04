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

  it("issues upgrade origin credentials through the Tauri command bridge", async () => {
    const invoke = vi.fn().mockResolvedValue("desktop-token");
    window.__TAURI__ = { core: { invoke } };

    const { issueDesktopUpgradeOriginToken } = await import("./tauri");

    await expect(issueDesktopUpgradeOriginToken()).resolves.toBe("desktop-token");
    expect(invoke).toHaveBeenCalledWith("issue_desktop_upgrade_origin_token", undefined);
  });

  it("opens and closes the native traffic detail window through Tauri commands", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    window.__TAURI__ = { core: { invoke } };

    const { closeDesktopTrafficDetailWindow, openDesktopTrafficDetailWindow } =
      await import("./tauri");

    await openDesktopTrafficDetailWindow("REQ-special/1", "popup-1");
    await closeDesktopTrafficDetailWindow();

    expect(invoke).toHaveBeenNthCalledWith(1, "open_traffic_detail_window", {
      recordId: "REQ-special/1",
      popupId: "popup-1",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "close_traffic_detail_window", undefined);
  });
});
