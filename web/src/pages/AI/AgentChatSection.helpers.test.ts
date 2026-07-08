import { describe, expect, it } from "vitest";
import {
  buildProcessLogItems,
  formatCommandGroupSummary,
  type ProcessStep,
} from "./AgentChatSection.helpers";

describe("process log grouping", () => {
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
    ).toBe("正在运行 2 条命令 · 1 条执行中 · 4s");

    expect(
      formatCommandGroupSummary(
        [{ type: "tool", summary: "pnpm test", status: "failed", durationMs: 300 }],
        13,
      ),
    ).toBe("失败 1 条命令 · 1s");
  });
});
