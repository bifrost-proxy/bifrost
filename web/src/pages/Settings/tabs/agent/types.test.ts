import { describe, expect, it } from "vitest";
import { DEFAULTS, getEffectiveCompactLimit, type AgentConfig } from "./types";

describe("Agent config derived limits", () => {
  it("uses explicit auto compact limit when present", () => {
    expect(
      getEffectiveCompactLimit({
        model_context_window: 200_000,
        model_auto_compact_token_limit: 123_456,
      }),
    ).toBe(123_456);
  });

  it("derives the compact limit from context window when unset", () => {
    expect(
      getEffectiveCompactLimit({
        model_context_window: 128_001,
      }),
    ).toBe(Math.floor(128_001 * 0.9));
  });

  it("falls back to the default context window for partial API payloads", () => {
    expect(getEffectiveCompactLimit({} as Partial<AgentConfig>)).toBe(
      Math.floor(DEFAULTS.model_context_window * 0.9),
    );
  });
});
