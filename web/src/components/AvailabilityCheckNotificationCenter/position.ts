export interface FloatingPosition {
  left: number;
  top: number;
}

export interface FloatingViewport {
  width: number;
  height: number;
}

export const NOTIFICATION_BUBBLE_SIZE = 42;
export const NOTIFICATION_BUBBLE_MARGIN = 18;
export const NOTIFICATION_BUBBLE_DEFAULT_TOP = 72;
export const NOTIFICATION_PANEL_WIDTH = 360;

export function defaultNotificationBubblePosition(
  viewport: FloatingViewport,
): FloatingPosition {
  return clampNotificationBubblePosition(
    {
      left: viewport.width - NOTIFICATION_BUBBLE_SIZE - NOTIFICATION_BUBBLE_MARGIN,
      top: NOTIFICATION_BUBBLE_DEFAULT_TOP,
    },
    viewport,
  );
}

export function clampNotificationBubblePosition(
  position: FloatingPosition,
  viewport: FloatingViewport,
  bubbleSize = NOTIFICATION_BUBBLE_SIZE,
  margin = NOTIFICATION_BUBBLE_MARGIN,
): FloatingPosition {
  const maxLeft = Math.max(margin, viewport.width - bubbleSize - margin);
  const maxTop = Math.max(margin, viewport.height - bubbleSize - margin);

  return {
    left: Math.min(Math.max(position.left, margin), maxLeft),
    top: Math.min(Math.max(position.top, margin), maxTop),
  };
}

export function clampNotificationPanelPosition(
  bubblePosition: FloatingPosition,
  viewport: FloatingViewport,
  panelWidth = NOTIFICATION_PANEL_WIDTH,
  margin = NOTIFICATION_BUBBLE_MARGIN,
): FloatingPosition {
  const maxLeft = Math.max(margin, viewport.width - panelWidth - margin);

  return {
    left: Math.min(Math.max(bubblePosition.left, margin), maxLeft),
    top: bubblePosition.top,
  };
}
