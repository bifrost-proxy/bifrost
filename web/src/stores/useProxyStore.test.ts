import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getSystemProxyStatus,
  setSystemProxy,
  type SystemProxyStatus,
} from "../api/proxy";
import {
  doesSystemProxyMatchRequest,
  isSystemProxyConfiguredEnabled,
  isSystemProxyLiveEnabledByBifrost,
  useProxyStore,
} from "./useProxyStore";

vi.mock("../api/proxy", () => ({
  getCliProxyStatus: vi.fn(),
  getSystemProxyLaunchdStatus: vi.fn(),
  getSystemProxyStatus: vi.fn(),
  setSystemProxy: vi.fn(),
  setSystemProxyLaunchd: vi.fn(),
}));

const status = (overrides: Partial<SystemProxyStatus>): SystemProxyStatus => ({
  supported: true,
  enabled: false,
  host: "",
  port: 0,
  bypass: "",
  ...overrides,
});

describe("doesSystemProxyMatchRequest", () => {
  it("treats an external proxy left enabled as successful disable", () => {
    expect(
      doesSystemProxyMatchRequest(
        status({
          enabled: true,
          host: "127.0.0.1",
          port: 6152,
          managed_by_bifrost: false,
        }),
        false,
      ),
    ).toBe(true);
  });

  it("treats a Bifrost-managed proxy left enabled as failed disable", () => {
    expect(
      doesSystemProxyMatchRequest(
        status({
          enabled: true,
          host: "127.0.0.1",
          port: 8800,
          managed_by_bifrost: true,
        }),
        false,
      ),
    ).toBe(false);
  });

  it("does not treat an external proxy as successful enable", () => {
    expect(
      doesSystemProxyMatchRequest(
        status({
          enabled: true,
          host: "127.0.0.1",
          port: 9900,
          managed_by_bifrost: false,
        }),
        true,
      ),
    ).toBe(false);
  });
});

describe("system proxy status helpers", () => {
  it("separates the stored preference from live Bifrost ownership", () => {
    const cleanedUp = status({
      enabled: false,
      managed_by_bifrost: false,
      configured_enabled: true,
    });

    expect(isSystemProxyConfiguredEnabled(cleanedUp)).toBe(true);
    expect(isSystemProxyLiveEnabledByBifrost(cleanedUp)).toBe(false);
  });

  it("falls back to live Bifrost ownership for older status payloads", () => {
    expect(
      isSystemProxyConfiguredEnabled(
        status({
          enabled: true,
          host: "127.0.0.1",
          port: 8800,
          managed_by_bifrost: true,
        }),
      ),
    ).toBe(true);
  });
});

describe("system proxy intent updates", () => {
  const suspended = status({
    configured_enabled: true,
    managed_by_bifrost: false,
  });

  beforeEach(() => {
    vi.resetAllMocks();
    useProxyStore.setState({
      systemProxy: suspended,
      loading: false,
      error: null,
    });
  });

  it("disables retained intent even when the OS proxy is already inactive", async () => {
    const disabled = { ...suspended, configured_enabled: false };
    vi.mocked(setSystemProxy).mockResolvedValue(disabled);

    expect(await useProxyStore.getState().toggleSystemProxy(false)).toBe(true);

    expect(setSystemProxy).toHaveBeenCalledWith({ enabled: false });
    expect(useProxyStore.getState().systemProxy).toEqual(disabled);
    expect(useProxyStore.getState().loading).toBe(false);
    expect(useProxyStore.getState().error).toBeNull();
  });

  it("keeps retained intent when disabling fails without a newer snapshot", async () => {
    vi.mocked(setSystemProxy).mockRejectedValue(new Error("Disable failed"));

    expect(await useProxyStore.getState().toggleSystemProxy(false)).toBe(false);

    expect(useProxyStore.getState().systemProxy).toBe(suspended);
    expect(useProxyStore.getState().loading).toBe(false);
    expect(useProxyStore.getState().error).toBe("Disable failed");
  });

  it.each(["push", "refresh"])(
    "does not roll back newer %s intent when an earlier toggle fails",
    async (source) => {
      let rejectToggle!: (reason: Error) => void;
      vi.mocked(setSystemProxy).mockReturnValue(
        new Promise((_, reject) => {
          rejectToggle = reject;
        }),
      );
      const pendingToggle = useProxyStore.getState().toggleSystemProxy(false);
      const disabled = { ...suspended, configured_enabled: false };

      if (source === "push") {
        useProxyStore.getState().applySystemProxySnapshot(disabled);
      } else {
        vi.mocked(getSystemProxyStatus).mockResolvedValue(disabled);
        await useProxyStore.getState().fetchSystemProxy();
      }
      rejectToggle(new Error("Response lost"));
      expect(await pendingToggle).toBe(false);

      expect(useProxyStore.getState().systemProxy).toBe(disabled);
      expect(useProxyStore.getState().loading).toBe(false);
      expect(useProxyStore.getState().error).toBe("Response lost");
    },
  );

  it("preserves newer intent when a status refresh fails", async () => {
    let rejectRefresh!: (reason: Error) => void;
    vi.mocked(getSystemProxyStatus).mockReturnValue(
      new Promise((_, reject) => {
        rejectRefresh = reject;
      }),
    );
    const pendingRefresh = useProxyStore.getState().fetchSystemProxy();
    const disabled = { ...suspended, configured_enabled: false };
    useProxyStore.getState().applySystemProxySnapshot(disabled);

    rejectRefresh(new Error("Refresh failed"));
    await pendingRefresh;

    expect(useProxyStore.getState().systemProxy).toBe(disabled);
    expect(useProxyStore.getState().error).toBe("Refresh failed");
  });
});
