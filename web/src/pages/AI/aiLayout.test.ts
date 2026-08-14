import { describe, expect, it } from "vitest";
import {
  buildRunnerOptions,
  normalizeAiModulePath,
  resolveLegacyAiDestination,
  selectDefaultRunner,
} from "./aiLayout";

function params(query = "") {
  return new URLSearchParams(query);
}

describe("AI module routing", () => {
  it("keeps only supported module paths", () => {
    expect(normalizeAiModulePath("/ai/runs/")).toBe("/ai/runs");
    expect(normalizeAiModulePath("/ai/removed-detail")).toBe("/ai");
  });

  it("maps legacy feature links to module detail pages", () => {
    expect(resolveLegacyAiDestination(params("view=asr"))).toBe("/ai/asr");
    expect(
      resolveLegacyAiDestination(params("aiSection=im-gateway-routes")),
    ).toBe("/ai/channels");
    expect(resolveLegacyAiDestination(params("settings=agent"))).toBe(
      "/ai/agents",
    );
  });

  it("maps removed chat and session details to summary-only runs", () => {
    expect(resolveLegacyAiDestination(params("view=chat&mode=new"))).toBe(
      "/ai/runs",
    );
    expect(resolveLegacyAiDestination(params("session=admin-chat-1"))).toBe(
      "/ai/runs",
    );
    expect(
      resolveLegacyAiDestination(params("historyPath=%2Ftmp%2Fsecret.jsonl")),
    ).toBe("/ai/runs");
  });

  it("leaves a clean AI home URL on the hub", () => {
    expect(resolveLegacyAiDestination(params())).toBeNull();
  });
});

describe("runner options", () => {
  it("sorts product runners before custom runners", () => {
    const options = buildRunnerOptions({
      runners: {
        custom_runner: { enabled: true, adapter: "shell" },
        trae_runner: { enabled: true, adapter: "traex" },
        claude_runner: { enabled: true, adapter: "claude_code" },
        codex_runner: { enabled: true, adapter: "codex" },
      },
    });

    expect(options.map((option) => option.label)).toEqual([
      "Codex Runner",
      "Claude Code",
      "Trae X",
      "custom_runner (shell)",
    ]);
  });

  it("uses Codex as the default runner when available", () => {
    const options = buildRunnerOptions({
      runners: {
        claude_runner: { enabled: true, adapter: "claude_code" },
        codex_runner: { enabled: true, adapter: "codex" },
      },
    });

    expect(selectDefaultRunner(options)).toMatchObject({
      label: "Codex Runner",
      value: "codex_runner",
    });
  });

  it("falls back to the first available runner when Codex is unavailable", () => {
    const options = buildRunnerOptions({
      runners: {
        claude_runner: { enabled: true, adapter: "claude_code" },
      },
    });

    expect(selectDefaultRunner(options)).toMatchObject({
      label: "Claude Code",
      value: "claude_runner",
    });
  });

  it("filters disabled runners", () => {
    const options = buildRunnerOptions({
      runners: {
        codex_runner: { enabled: false, adapter: "codex" },
        traex_runner: { enabled: true, adapter: "traex" },
      },
    });

    expect(options.some((option) => option.value === "codex_runner")).toBe(
      false,
    );
    expect(options.map((option) => option.label)).toContain("Trae X");
  });
});
