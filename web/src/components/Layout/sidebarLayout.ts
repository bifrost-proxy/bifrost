import type { CSSProperties } from "react";

export const APP_SIDEBAR_WIDTH = 50;
export const APP_SIDEBAR_ITEM_HEIGHT = 64;

export const SIDEBAR_MENU_SCROLL_STYLE = {
  width: "100%",
  flex: 1,
  minHeight: 0,
  overflowY: "auto",
  overflowX: "hidden",
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  scrollbarGutter: "auto",
  scrollbarWidth: "none",
} satisfies CSSProperties;

export const SIDEBAR_MENU_ITEM_STYLE = {
  width: APP_SIDEBAR_WIDTH,
  height: APP_SIDEBAR_ITEM_HEIGHT,
  minHeight: APP_SIDEBAR_ITEM_HEIGHT,
  flexShrink: 0,
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  justifyContent: "center",
  position: "relative",
} satisfies CSSProperties;
