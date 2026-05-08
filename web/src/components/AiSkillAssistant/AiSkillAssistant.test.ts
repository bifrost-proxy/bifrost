import { describe, expect, it } from "vitest";
import {
  SKILL_ASSISTANT_MARGIN,
  clampSkillAssistantPosition,
  isSkillAssistantDrag,
} from "./position";

describe("AiSkillAssistant position helpers", () => {
  it("keeps the launcher inside the visible viewport", () => {
    expect(
      clampSkillAssistantPosition({ x: -40, y: -20 }, { width: 400, height: 300 }),
    ).toEqual({ x: SKILL_ASSISTANT_MARGIN, y: SKILL_ASSISTANT_MARGIN });
    expect(
      clampSkillAssistantPosition({ x: 800, y: 600 }, { width: 400, height: 300 }),
    ).toEqual({ x: 332, y: 232 });
    expect(
      clampSkillAssistantPosition({ x: 120, y: 96 }, { width: 400, height: 300 }),
    ).toEqual({ x: 120, y: 96 });
  });

  it("distinguishes a click from a drag", () => {
    expect(isSkillAssistantDrag(1, 2)).toBe(false);
    expect(isSkillAssistantDrag(4, 0)).toBe(true);
    expect(isSkillAssistantDrag(3, 4)).toBe(true);
  });
});
