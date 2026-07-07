import { describe, expect, it } from "vitest";
import {
  getDesktopDragRegionAttributes,
  getDesktopTopDragRightInset,
  WINDOWS_WINDOW_CONTROLS_HIT_TEST_INSET,
} from "./desktopChrome";

describe("desktop chrome drag regions", () => {
  it("keeps the Windows caption button area out of the top drag region", () => {
    expect(getDesktopTopDragRightInset("windows")).toBe(
      WINDOWS_WINDOW_CONTROLS_HIT_TEST_INSET,
    );
    expect(getDesktopTopDragRightInset("macos")).toBe(0);
    expect(getDesktopTopDragRightInset("linux")).toBe(0);
  });

  it("does not mark interactive controls as Tauri drag regions", () => {
    expect(getDesktopDragRegionAttributes(true)).toEqual({
      "data-desktop-window-drag-region": "true",
      "data-tauri-drag-region": "",
    });
    expect(
      getDesktopDragRegionAttributes(true, { interactive: true }),
    ).toEqual({});
    expect(getDesktopDragRegionAttributes(false)).toEqual({});
  });
});
