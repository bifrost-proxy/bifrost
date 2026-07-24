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
  normalizeApiErrorMessage: (error: unknown, fallback: string) =>
    error instanceof Error ? error.message : fallback,
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

  it("resumes persisted active progress after a WebView reload", async () => {
    sessionStorage.setItem("bifrost-upgrade-reload-pending", "done");
    apiMocks.getUpgradeProgress.mockResolvedValue({
      phase: "downloading",
      percent: 42,
      message: "Downloading from fallback source…",
      target_version: "0.0.105",
      source: "admin",
      error: null,
      updated_at: new Date().toISOString(),
    });
    useVersionStore.setState({
      hasUpdate: false,
      latestVersion: null,
      modalVisible: false,
      upgrading: false,
      upgradePhase: "idle",
    });

    const resumed = await useVersionStore.getState().resumeUpgradeProgress();

    expect(resumed).toBe(true);
    expect(useVersionStore.getState()).toMatchObject({
      hasUpdate: true,
      latestVersion: "0.0.105",
      modalVisible: true,
      upgrading: true,
      upgradePhase: "downloading",
      upgradePercent: 42,
      upgradeMessage: "Downloading from fallback source…",
      upgradeError: null,
    });
    expect(sessionStorage.getItem("bifrost-upgrade-reload-pending")).toBeNull();
  });

  it("adopts an existing active upgrade instead of surfacing a 409 as failure", async () => {
    apiMocks.startUpgrade.mockRejectedValue(
      new Error("Request failed with status code 409"),
    );
    apiMocks.getUpgradeProgress.mockResolvedValue({
      phase: "downloading",
      percent: 73,
      message: "Downloading existing transaction…",
      target_version: "0.0.105",
      source: "admin",
      error: null,
      updated_at: new Date().toISOString(),
    });

    await useVersionStore.getState().startUpgrade();

    expect(apiMocks.startUpgrade).toHaveBeenCalledTimes(1);
    expect(useVersionStore.getState()).toMatchObject({
      modalVisible: true,
      upgrading: true,
      upgradePhase: "downloading",
      upgradePercent: 73,
      upgradeMessage: "Downloading existing transaction…",
      upgradeError: null,
    });
  });

  it("keeps the real start error when no active upgrade can be resumed", async () => {
    apiMocks.startUpgrade.mockRejectedValue(
      new Error("Desktop upgrade origin is invalid"),
    );
    apiMocks.getUpgradeProgress.mockResolvedValue({
      phase: "idle",
      percent: null,
      message: "",
      target_version: null,
      source: null,
      error: null,
      updated_at: new Date().toISOString(),
    });

    await useVersionStore.getState().startUpgrade();

    expect(useVersionStore.getState()).toMatchObject({
      upgrading: false,
      upgradePhase: "failed",
      upgradeError: "Desktop upgrade origin is invalid",
    });
  });

  it("does not hide active progress when a version check observes the CLI at target", async () => {
    useVersionStore.setState({
      hasUpdate: true,
      latestVersion: "0.0.105",
      upgrading: true,
      upgradePhase: "downloading",
    });
    apiMocks.checkVersion.mockResolvedValue({
      has_update: false,
      current_version: "0.0.105",
      latest_version: "0.0.105",
      release_highlights: [],
      release_url: null,
      checked_at: new Date().toISOString(),
    });

    await useVersionStore.getState().checkVersion({ forceRefresh: true });

    expect(useVersionStore.getState()).toMatchObject({
      hasUpdate: true,
      latestVersion: "0.0.105",
      upgrading: true,
      upgradePhase: "downloading",
    });
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

  it("reloads the WebView without an App handoff for completed CLI-owned progress", async () => {
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
    useVersionStore.setState({ upgrading: true, upgradePhase: "restarting" });
    apiMocks.getUpgradeProgress.mockResolvedValue({
      phase: "completed",
      percent: null,
      message: "CLI-owned upgrade complete",
      target_version: "0.0.105",
      source: "admin",
      error: null,
      updated_at: new Date().toISOString(),
    });

    useVersionStore.getState().pollUpgradeProgress();
    await vi.advanceTimersByTimeAsync(1000);

    expect(invoke).not.toHaveBeenCalled();
    expect(window.location.reload).toHaveBeenCalledTimes(1);
    expect(useVersionStore.getState().upgrading).toBe(false);
  });
});
