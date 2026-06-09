import { describe, expect, it } from "vitest";
import {
  NOTIFICATION_BUBBLE_DEFAULT_TOP,
  NOTIFICATION_BUBBLE_MARGIN,
  NOTIFICATION_BUBBLE_SIZE,
  NOTIFICATION_PANEL_WIDTH,
  clampNotificationBubblePosition,
  clampNotificationPanelPosition,
  defaultNotificationBubblePosition,
} from "./position";

describe("availability check notification position helpers", () => {
  it("places the default bubble below the top toolbar area", () => {
    const position = defaultNotificationBubblePosition({ width: 1280, height: 720 });

    expect(position.top).toBe(NOTIFICATION_BUBBLE_DEFAULT_TOP);
    expect(position.left).toBe(
      1280 - NOTIFICATION_BUBBLE_SIZE - NOTIFICATION_BUBBLE_MARGIN,
    );
  });

  it("keeps dragged bubble coordinates inside the viewport", () => {
    expect(
      clampNotificationBubblePosition({ left: -120, top: -90 }, { width: 320, height: 240 }),
    ).toEqual({
      left: NOTIFICATION_BUBBLE_MARGIN,
      top: NOTIFICATION_BUBBLE_MARGIN,
    });

    expect(
      clampNotificationBubblePosition({ left: 480, top: 420 }, { width: 320, height: 240 }),
    ).toEqual({
      left: 320 - NOTIFICATION_BUBBLE_SIZE - NOTIFICATION_BUBBLE_MARGIN,
      top: 240 - NOTIFICATION_BUBBLE_SIZE - NOTIFICATION_BUBBLE_MARGIN,
    });
  });

  it("keeps the expanded panel aligned within the viewport", () => {
    const position = clampNotificationPanelPosition(
      { left: 1180, top: 72 },
      { width: 1280, height: 720 },
    );

    expect(position).toEqual({
      left: 1280 - NOTIFICATION_PANEL_WIDTH - NOTIFICATION_BUBBLE_MARGIN,
      top: 72,
    });
  });
});
