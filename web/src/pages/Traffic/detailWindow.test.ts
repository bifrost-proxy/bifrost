import { describe, expect, it, vi } from "vitest";
import { openTrafficDetailWindow } from "./detailWindow";

function popupWindow(closed = false): Window {
  return {
    closed,
    location: { href: "about:blank" },
    focus: vi.fn(),
  } as unknown as Window;
}

describe("openTrafficDetailWindow", () => {
  it("uses the native desktop command without calling window.open", async () => {
    const openDesktop = vi.fn().mockResolvedValue(undefined);
    const openBrowser = vi.fn();

    await expect(
      openTrafficDetailWindow({
        desktop: true,
        recordId: "REQ-1",
        popupId: "popup-1",
        url: "tauri://localhost/#/traffic/detail",
        existingPopup: null,
        openDesktop,
        openBrowser,
      }),
    ).resolves.toEqual({ kind: "desktop" });

    expect(openDesktop).toHaveBeenCalledWith("REQ-1", "popup-1");
    expect(openBrowser).not.toHaveBeenCalled();
  });

  it("reuses and focuses an existing browser popup", async () => {
    const existingPopup = popupWindow();
    const openBrowser = vi.fn();

    const result = await openTrafficDetailWindow({
      desktop: false,
      recordId: "REQ-2",
      popupId: "popup-2",
      url: "http://localhost/detail?id=REQ-2",
      existingPopup,
      openDesktop: vi.fn(),
      openBrowser,
    });

    expect(result).toEqual({ kind: "browser", popup: existingPopup });
    expect(existingPopup.location.href).toBe("http://localhost/detail?id=REQ-2");
    expect(existingPopup.focus).toHaveBeenCalledOnce();
    expect(openBrowser).not.toHaveBeenCalled();
  });

  it("returns null when the browser blocks a new popup", async () => {
    const openBrowser = vi.fn().mockReturnValue(null);

    await expect(
      openTrafficDetailWindow({
        desktop: false,
        recordId: "REQ-3",
        popupId: "popup-3",
        url: "http://localhost/detail?id=REQ-3",
        existingPopup: popupWindow(true),
        openDesktop: vi.fn(),
        openBrowser,
      }),
    ).resolves.toEqual({ kind: "browser", popup: null });
  });
});
