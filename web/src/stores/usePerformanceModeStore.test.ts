// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getPerformanceConfig: vi.fn(),
}));

vi.mock("../api/config", () => ({
  getPerformanceConfig: apiMocks.getPerformanceConfig,
}));

type PerformanceModeStore =
  typeof import("./usePerformanceModeStore").usePerformanceModeStore;

const configWithMode = (enabled: boolean) => ({
  traffic: { super_performance_mode: enabled },
  breakpoint: {},
});

describe("usePerformanceModeStore", () => {
  let usePerformanceModeStore: PerformanceModeStore;

  beforeEach(async () => {
    vi.resetModules();
    apiMocks.getPerformanceConfig.mockReset();
    usePerformanceModeStore = (await import("./usePerformanceModeStore"))
      .usePerformanceModeStore;
  });

  it("loads once, reuses the cached mode, and refreshes when forced", async () => {
    apiMocks.getPerformanceConfig
      .mockResolvedValueOnce(configWithMode(true))
      .mockResolvedValueOnce(configWithMode(false));

    await expect(
      usePerformanceModeStore.getState().fetchPerformanceMode(),
    ).resolves.toBe(true);
    await expect(
      usePerformanceModeStore.getState().fetchPerformanceMode(),
    ).resolves.toBe(true);
    expect(apiMocks.getPerformanceConfig).toHaveBeenCalledTimes(1);

    await expect(
      usePerformanceModeStore.getState().fetchPerformanceMode(true),
    ).resolves.toBe(false);
    expect(apiMocks.getPerformanceConfig).toHaveBeenCalledTimes(2);
  });

  it("deduplicates concurrent requests", async () => {
    let resolveConfig!: (value: ReturnType<typeof configWithMode>) => void;
    apiMocks.getPerformanceConfig.mockReturnValue(
      new Promise((resolve) => {
        resolveConfig = resolve;
      }),
    );

    const first = usePerformanceModeStore.getState().fetchPerformanceMode();
    const second = usePerformanceModeStore.getState().fetchPerformanceMode();
    resolveConfig(configWithMode(true));

    await expect(Promise.all([first, second])).resolves.toEqual([true, true]);
    expect(apiMocks.getPerformanceConfig).toHaveBeenCalledTimes(1);
  });

  it("falls back to normal mode when the initial request fails", async () => {
    apiMocks.getPerformanceConfig.mockRejectedValue(new Error("offline"));

    await expect(
      usePerformanceModeStore.getState().fetchPerformanceMode(),
    ).resolves.toBe(false);
    expect(usePerformanceModeStore.getState().superPerformanceMode).toBe(false);
  });
});
