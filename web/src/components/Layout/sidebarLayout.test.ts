import { describe, expect, it } from "vitest";
import {
  APP_SIDEBAR_ITEM_HEIGHT,
  APP_SIDEBAR_WIDTH,
  SIDEBAR_MENU_ITEM_STYLE,
  SIDEBAR_MENU_SCROLL_STYLE,
} from "./sidebarLayout";

describe("WebKit 侧栏布局契约", () => {
  it("不会为滚动条预留稳定横向 gutter", () => {
    expect(SIDEBAR_MENU_SCROLL_STYLE).toMatchObject({
      width: "100%",
      overflowX: "hidden",
      overflowY: "auto",
      scrollbarGutter: "auto",
      scrollbarWidth: "none",
    });
  });

  it("保持 50px 侧栏项和 64px 最小点击高度", () => {
    expect(APP_SIDEBAR_WIDTH).toBe(50);
    expect(APP_SIDEBAR_ITEM_HEIGHT).toBe(64);
    expect(SIDEBAR_MENU_ITEM_STYLE).toMatchObject({
      width: APP_SIDEBAR_WIDTH,
      height: APP_SIDEBAR_ITEM_HEIGHT,
      minHeight: APP_SIDEBAR_ITEM_HEIGHT,
      flexShrink: 0,
    });
  });
});
