export const DESKTOP_TOP_DRAG_HEIGHT = 35;
export const WINDOWS_WINDOW_CONTROLS_HIT_TEST_INSET = 124;

type DesktopDragRegionAttributes = {
  "data-desktop-window-drag-region"?: "true";
  "data-tauri-drag-region"?: "";
};

export function getDesktopTopDragRightInset(platform: string): number {
  return platform === "windows" ? WINDOWS_WINDOW_CONTROLS_HIT_TEST_INSET : 0;
}

export function getDesktopDragRegionAttributes(
  enabled: boolean,
  options: { interactive?: boolean } = {},
): DesktopDragRegionAttributes {
  if (!enabled || options.interactive) {
    return {};
  }

  return {
    "data-desktop-window-drag-region": "true",
    "data-tauri-drag-region": "",
  };
}
