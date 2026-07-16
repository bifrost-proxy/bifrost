import { describe, expect, it } from "vitest";
import {
  buildProcessLogItems,
  eventToProcessStep,
  formatCommandGroupSummary,
  type ProcessStep,
} from "./AgentChatSection.helpers";

describe("process log grouping", () => {
  it("suppresses internal usage refresh events from the live process log", () => {
    expect(
      eventToProcessStep({
        eventType: "assistant_delta",
        content: "token_usage: token usage updated",
      }),
    ).toBeNull();
    expect(
      eventToProcessStep({
        eventType: "status",
        title: "rate_limits",
        content: "usage updated",
      }),
    ).toBeNull();
  });

  it("groups adjacent tool steps while preserving text step order", () => {
    const steps: ProcessStep[] = [
      { type: "thinking", summary: "Inspecting", status: "success" },
      { type: "tool", summary: "rg", status: "success" },
      { type: "tool", summary: "sed", status: "running" },
      { type: "status", summary: "Reading output", status: "success" },
      { type: "tool", summary: "pnpm test", status: "failed" },
    ];

    expect(buildProcessLogItems(steps)).toEqual([
      { type: "text", step: steps[0], index: 0 },
      { type: "commands", steps: [steps[1], steps[2]], startIndex: 1 },
      { type: "text", step: steps[3], index: 3 },
      { type: "commands", steps: [steps[4]], startIndex: 4 },
    ]);
  });

  it("summarizes command groups by running and failed state", () => {
    expect(
      formatCommandGroupSummary(
        [
          { type: "tool", summary: "rg", status: "success", durationMs: 1200 },
          { type: "tool", summary: "sed", status: "running", startedAt: 10 },
        ],
        13,
      ),
    ).toBe("已运行 2 条命令");

    expect(
      formatCommandGroupSummary(
        [{ type: "tool", summary: "pnpm test", status: "failed", durationMs: 300 }],
        13,
      ),
    ).toBe("失败 1 条命令 · 1s");

    expect(
      formatCommandGroupSummary(
        [{ type: "tool", summary: "cargo test", status: "success" }],
        13,
      ),
    ).toBe("已运行 1 条命令");
  });
});
