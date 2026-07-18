// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  checkVersion: vi.fn(),
  getUpgradeProgress: vi.fn(),
  startUpgrade: vi.fn(),
}));
const runtimeMocks = vi.hoisted(() => ({
  desktopShell: false,
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

vi.mock("../runtime", () => ({
  isDesktopShell: () => runtimeMocks.desktopShell,
}));

type VersionStore = typeof import("./useVersionStore").useVersionStore;

function createMemoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key: string) => values.get(key) ?? null,
    key: (index: number) => Array.from(values.keys())[index] ?? null,
    removeItem: (key: string) => {
      values.delete(key);
    },
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}

describe("useVersionStore upgrade polling", () => {
  let useVersionStore: VersionStore;

  beforeEach(async () => {
    vi.useFakeTimers();
    vi.resetModules();
    runtimeMocks.desktopShell = false;

    const localStorage = createMemoryStorage();
    const sessionStorage = createMemoryStorage();

    Object.defineProperty(globalThis, "localStorage", {
      value: localStorage,
      configurable: true,
    });
    Object.defineProperty(globalThis, "sessionStorage", {
      value: sessionStorage,
      configurable: true,
    });
    Object.defineProperty(globalThis, "window", {
      value: {
        localStorage,
        sessionStorage,
        location: { reload: vi.fn() },
      },
      configurable: true,
    });

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

    useVersionStore = (await import("./useVersionStore")).useVersionStore;
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
    useVersionStore?.getState().stopPollUpgradeProgress();
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
    expect(apiMocks.checkVersion).toHaveBeenCalledWith(true, "cli");
  });

  it("uses the desktop channel for version checks and upgrades in desktop shell", async () => {
    runtimeMocks.desktopShell = true;
    useVersionStore.getState().stopPollUpgradeProgress();
    useVersionStore = (await import("./useVersionStore")).useVersionStore;
    useVersionStore.setState({
      lastChecked: null,
      upgrading: false,
      upgradePhase: "idle",
    });
    apiMocks.startUpgrade.mockResolvedValue({
      phase: "checking",
      percent: null,
      message: "Checking for updates...",
      target_version: "0.0.105",
      source: "desktop",
      error: null,
      updated_at: new Date().toISOString(),
    });

    await useVersionStore.getState().checkVersion({ forceRefresh: true });
    await useVersionStore.getState().startUpgrade();

    expect(apiMocks.checkVersion).toHaveBeenCalledWith(true, "desktop");
    expect(apiMocks.startUpgrade).toHaveBeenCalledWith("desktop");
  });

  it("does not report success when the desktop restart handoff fails", async () => {
    runtimeMocks.desktopShell = true;
    const invoke = vi.fn().mockRejectedValueOnce(new Error("helper spawn failed"));
    Object.assign(window, {
      __TAURI__: {
        core: {
          invoke,
        },
      },
    });
    useVersionStore.getState().stopPollUpgradeProgress();
    useVersionStore = (await import("./useVersionStore")).useVersionStore;
    useVersionStore.setState({ upgrading: true, upgradePhase: "restarting" });
    apiMocks.getUpgradeProgress.mockResolvedValue({
      phase: "restarting",
      percent: null,
      message: "Waiting for desktop shell to stop before installing…",
      target_version: "0.0.105",
      source: "desktop",
      error: null,
      updated_at: new Date().toISOString(),
    });

    useVersionStore.getState().pollUpgradeProgress();
    await vi.advanceTimersByTimeAsync(1000);
    await vi.waitFor(() => {
      expect(useVersionStore.getState().upgradePhase).toBe("failed");
    });

    expect(useVersionStore.getState().upgradeError).toBe("helper spawn failed");
    expect(window.location.reload).not.toHaveBeenCalled();
    expect(sessionStorage.getItem("bifrost-upgrade-reload-pending")).toBeNull();

    invoke.mockResolvedValueOnce(undefined);
    await useVersionStore.getState().startUpgrade();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
    expect(apiMocks.startUpgrade).not.toHaveBeenCalled();
    expect(useVersionStore.getState().upgradePhase).toBe("restarting");
  });

  it("does not hand off the desktop shell for a CLI-owned backend restart", async () => {
    runtimeMocks.desktopShell = true;
    const invoke = vi.fn();
    Object.assign(window, {
      __TAURI__: {
        core: {
          invoke,
        },
      },
    });
    useVersionStore.getState().stopPollUpgradeProgress();
    useVersionStore = (await import("./useVersionStore")).useVersionStore;
    useVersionStore.setState({ upgrading: true, upgradePhase: "installing" });
    apiMocks.getUpgradeProgress.mockResolvedValue({
      phase: "restarting",
      percent: null,
      message: "Restarting CLI-owned proxy…",
      target_version: "0.0.105",
      source: "admin",
      error: null,
      updated_at: new Date().toISOString(),
    });

    useVersionStore.getState().pollUpgradeProgress();
    await vi.advanceTimersByTimeAsync(1000);

    expect(invoke).not.toHaveBeenCalled();
    expect(useVersionStore.getState().upgradePhase).toBe("restarting");
    expect(useVersionStore.getState().upgrading).toBe(true);
  });
});
