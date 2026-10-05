// @vitest-environment node
import fs from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => {
  vi.unstubAllEnvs();
  vi.resetModules();
});

describe("full UI audit reporting", () => {
  it("keeps local runs unlimited with the interactive list reporter", async () => {
    vi.stubEnv("CI", "");
    vi.stubEnv("BIFROST_UI_TEST_GLOBAL_TIMEOUT_MS", "");
    const { default: config } = await import("../../playwright.config");
    expect(config.globalTimeout).toBe(0);
    expect(config.reporter).toBe("list");
  });

  it("uses a graceful Playwright deadline and durable CI reports", async () => {
    vi.stubEnv("CI", "true");
    vi.stubEnv("BIFROST_UI_TEST_GLOBAL_TIMEOUT_MS", "3000000");
    const { default: config } = await import("../../playwright.config");
    expect(config.globalTimeout).toBe(3000000);
    expect(config.reporter).toEqual([
      ["line"],
      ["json", { outputFile: "test-results/results.json" }],
      ["html", { open: "never" }],
    ]);
  });

  it("leaves reporting and upload time before both outer audit deadlines", () => {
    const workflow = fs.readFileSync(
      new URL("../../../.github/workflows/ui-e2e-full.yml", import.meta.url),
      "utf8",
    );
    const globalMs = Number(
      workflow.match(/BIFROST_UI_TEST_GLOBAL_TIMEOUT_MS: "(\d+)"/)?.[1],
    );
    const watchdogSeconds = Number(
      workflow.match(/BIFROST_E2E_SUITE_TIMEOUT: "(\d+)"/)?.[1],
    );
    const jobMinutes = Number(workflow.match(/timeout-minutes: (\d+)/)?.[1]);
    expect(globalMs).toBeGreaterThan(900000);
    expect(watchdogSeconds * 1000 - globalMs).toBeGreaterThanOrEqual(60000);
    expect(jobMinutes * 60 - watchdogSeconds).toBeGreaterThanOrEqual(600);
    expect(workflow).toContain("web/test-results/");
    expect(workflow).toContain("web/playwright-report/");
    expect(workflow).toContain(".bifrost-ui-test-runs/*/backend.log");
  });
});
