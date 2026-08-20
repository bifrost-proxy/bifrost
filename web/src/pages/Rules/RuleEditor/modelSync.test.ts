import { describe, expect, it } from "vitest";
import {
  normalizeRuleEditorContent,
  shouldReplaceRuleEditorModel,
} from "./modelSync";

describe("normalizeRuleEditorContent", () => {
  it("normalizes line endings and trailing blank lines", () => {
    expect(normalizeRuleEditorContent("first\r\nsecond\r\n\r\n")).toBe(
      "first\nsecond",
    );
  });
});

describe("shouldReplaceRuleEditorModel", () => {
  it("preserves the model for equivalent same-rule content", () => {
    expect(
      shouldReplaceRuleEditorModel({
        currentContent: "first\n",
        nextContent: "first",
        currentRuleName: "demo",
        nextRuleName: "demo",
      }),
    ).toBe(false);
  });

  it("replaces the model when same-rule content changes meaningfully", () => {
    expect(
      shouldReplaceRuleEditorModel({
        currentContent: "first\nsecond",
        nextContent: "first\nthird",
        currentRuleName: "demo",
        nextRuleName: "demo",
      }),
    ).toBe(true);
  });

  it("replaces the model after switching rules even when content is equivalent", () => {
    expect(
      shouldReplaceRuleEditorModel({
        currentContent: "first\n",
        nextContent: "first",
        currentRuleName: "previous",
        nextRuleName: "next",
      }),
    ).toBe(true);
  });
});
