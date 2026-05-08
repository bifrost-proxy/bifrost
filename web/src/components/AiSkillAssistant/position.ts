export interface SkillAssistantPosition {
  x: number;
  y: number;
}

export interface SkillAssistantViewport {
  width: number;
  height: number;
}

export const SKILL_ASSISTANT_SIZE = 56;
export const SKILL_ASSISTANT_MARGIN = 12;
export const SKILL_ASSISTANT_DRAG_THRESHOLD = 4;

export function clampSkillAssistantPosition(
  position: SkillAssistantPosition,
  viewport: SkillAssistantViewport,
): SkillAssistantPosition {
  const maxX = Math.max(
    SKILL_ASSISTANT_MARGIN,
    viewport.width - SKILL_ASSISTANT_SIZE - SKILL_ASSISTANT_MARGIN,
  );
  const maxY = Math.max(
    SKILL_ASSISTANT_MARGIN,
    viewport.height - SKILL_ASSISTANT_SIZE - SKILL_ASSISTANT_MARGIN,
  );

  return {
    x: Math.min(Math.max(position.x, SKILL_ASSISTANT_MARGIN), maxX),
    y: Math.min(Math.max(position.y, SKILL_ASSISTANT_MARGIN), maxY),
  };
}

export function isSkillAssistantDrag(deltaX: number, deltaY: number): boolean {
  return (
    Math.hypot(deltaX, deltaY) >= SKILL_ASSISTANT_DRAG_THRESHOLD
  );
}
