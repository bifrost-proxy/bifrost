import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  checkVersion: vi.fn(),
  getUpgradeProgress: vi.fn(),
  startUpgrade: vi.fn(),
}));

vi.mock("../api/version", () => ({
  checkVersion: apiMocks.checkVersion,
  getUpgradeProgress: apiMocks.getUpgradeProgress,
  startUpgrade: apiMocks.startUpgrade,
}));

vi.mock("../api/client", () => ({
  isConnectionIssueError: (error: unknown) =>
    error instanceof Error && error.message === "connection",
}));

import { useVersionStore } from "./useVersionStore";

describe("useVersionStore upgrade polling", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    apiMocks.checkVersion.mockResolvedValue({
      has_update: true,
      current_version: "0.0.104",
      latest_version: "0.0.105",
      release_highlights: [],
      release_url: null,
      checked_at: new Date().toISOString(),
    });
    apiMocks.getUpgradeProgress.mockReset();
    apiMocks.startUpgrade.mockReset();
    window.localStorage.clear();
    window.sessionStorage.clear();
    useVersionStore.getState().stopPollUpgradeProgress();
    useVersionStore.setState({
      hasUpdate: true,
      currentVersion: "0.0.104",
      latestVersion: "0.0.105",
      releaseHighlights: [],
      releaseUrl: null,
      loading: false,
      lastChecked: null,
      seenVersions: [],
      modalVisible: true,
      upgradePhase: "idle",
      upgradePercent: null,
      upgradeMessage: "",
      upgradeError: null,
      upgrading: false,
    });
  });

  afterEach(() => {
    useVersionStore.getState().stopPollUpgradeProgress();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("exits an active upgrade when terminal progress was already consumed", async () => {
    apiMocks.getUpgradeProgress.mockResolvedValue({
      phase: "idle",
      percent: null,
      message: "",
      target_version: null,
      source: null,
      error: null,
      updated_at: new Date().toISOString(),
    });

    useVersionStore.setState({
      upgrading: true,
      upgradePhase: "installing",
      upgradePercent: null,
      upgradeMessage: "Installing new version...",
      upgradeError: null,
    });

    useVersionStore.getState().pollUpgradeProgress();
    await vi.advanceTimersByTimeAsync(1000);

    expect(useVersionStore.getState().upgrading).toBe(false);
    expect(useVersionStore.getState().upgradePhase).toBe("idle");
    expect(apiMocks.checkVersion).toHaveBeenCalledWith({
      forceRefresh: true,
      skipCache: true,
    });
  });
});
