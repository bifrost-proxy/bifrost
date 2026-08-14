import { describe, expect, it } from "vitest";
import {
  formatRunDuration,
  liveRunDuration,
  runSourceLabel,
} from "./runSummary";

const runningItem = {
  session_key: "run-1",
  status: "running" as const,
  title: "Run",
  runner_id: "codex",
  duration_secs: 65,
  user_message_count: 2,
  source: "web",
  start_time: 100,
};

describe("run summary formatting", () => {
  it("formats seconds, minutes and hours", () => {
    expect(formatRunDuration(9)).toBe("9s");
    expect(formatRunDuration(65)).toBe("1m 5s");
    expect(formatRunDuration(7320)).toBe("2h 2m");
  });

  it("advances only running durations from the snapshot timestamp", () => {
    expect(liveRunDuration(runningItem, 200, 215)).toBe(80);
    expect(
      liveRunDuration({ ...runningItem, status: "completed" }, 200, 215),
    ).toBe(65);
  });

  it("uses friendly source labels and a stable fallback", () => {
    expect(runSourceLabel("feishu")).toBe("Feishu");
    expect(runSourceLabel("custom")).toBe("CUSTOM");
  });
});
