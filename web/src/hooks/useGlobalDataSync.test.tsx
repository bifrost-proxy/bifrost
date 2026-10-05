import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  proxy: { fetchSystemProxy: vi.fn(), fetchCliProxy: vi.fn() },
  filters: { loadFromServer: vi.fn() },
  metrics: {
    fetchOverview: vi.fn(),
    enablePush: vi.fn(),
    disablePush: vi.fn(),
  },
  traffic: {
    fetchInitialData: vi.fn(),
    startPolling: vi.fn(),
    stopPolling: vi.fn(),
    enablePush: vi.fn(),
    disablePush: vi.fn(),
    paused: false,
    polling: true,
    usePush: true,
  },
  version: {
    resumeUpgradeProgress: vi.fn(),
    checkVersion: vi.fn(),
    setModalVisible: vi.fn(),
    hasUpdate: false,
  },
  performance: { fetchPerformanceMode: vi.fn() },
  forceRefresh: { show: vi.fn() },
  pendingAuth: { stopSSE: vi.fn() },
  pendingTls: { stopSSE: vi.fn() },
  push: {
    connect: vi.fn(),
    getSubscription: vi.fn(),
    updateSubscription: vi.fn(),
    disconnectIfIdle: vi.fn(),
    onOverviewUpdate: vi.fn(),
    onMetricsUpdate: vi.fn(),
    onHistoryUpdate: vi.fn(),
    disconnect: vi.fn(),
    disableReconnectUntilRefresh: vi.fn(),
    onForceRefresh: vi.fn(),
    onNotification: vi.fn(),
  },
  syncDynamicData: vi.fn(),
}));

vi.mock("antd", () => ({ message: { success: vi.fn() } }));
vi.mock("../api", () => ({}));
vi.mock("../api/client", () => ({ isConnectionIssueError: vi.fn() }));
vi.mock("../stores/useProxyStore", () => ({
  useProxyStore: { getState: () => mocks.proxy },
}));
vi.mock("../stores/useFilterPanelStore", () => ({
  useFilterPanelStore: { getState: () => mocks.filters },
}));
vi.mock("../stores/useMetricsStore", () => ({
  useMetricsStore: { getState: () => mocks.metrics },
}));
vi.mock("../stores/useTrafficStore", () => ({
  useTrafficStore: { getState: () => mocks.traffic },
}));
vi.mock("../stores/useVersionStore", () => ({
  useVersionStore: { getState: () => mocks.version },
}));
vi.mock("../stores/usePerformanceModeStore", () => ({
  usePerformanceModeStore: { getState: () => mocks.performance },
}));
vi.mock("../stores/useForceRefreshStore", () => ({
  useForceRefreshStore: { getState: () => mocks.forceRefresh },
}));
vi.mock("../stores/usePendingAuthStore", () => ({
  usePendingAuthStore: { getState: () => mocks.pendingAuth },
}));
vi.mock("../stores/usePendingIpTlsStore", () => ({
  usePendingIpTlsStore: { getState: () => mocks.pendingTls },
}));
vi.mock("../services/pushService", () => ({
  default: mocks.push,
  METRICS_INTERVAL_DEFAULT_MS: 1000,
}));
vi.mock("./useEditorCompletion", () => ({
  syncDynamicData: mocks.syncDynamicData,
}));

import {
  isGlobalDataInitialized,
  useGlobalDataSync,
} from "./useGlobalDataSync";

function Harness({ trafficEnabled = true }: { trafficEnabled?: boolean }) {
  useGlobalDataSync({ trafficEnabled });
  return null;
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("global realtime lifecycle", () => {
  let root: Root | null;
  let container: HTMLDivElement;
  const forceRefreshHandlers = new Set<(data: { reason: string }) => void>();
  const notificationHandlers = new Set<unknown>();

  async function render(strict = false, trafficEnabled = true) {
    await act(async () => {
      const element = <Harness trafficEnabled={trafficEnabled} />;
      root!.render(strict ? <StrictMode>{element}</StrictMode> : element);
    });
  }

  async function unmount() {
    await act(async () => root?.unmount());
    root = null;
  }

  async function useRealMetricsActions() {
    const { useMetricsStore } = await vi.importActual<
      typeof import("../stores/useMetricsStore")
    >("../stores/useMetricsStore");
    useMetricsStore.setState({
      pushRefCount: 0,
      overviewUnsubscribe: null,
      metricsUnsubscribe: null,
      historyUnsubscribe: null,
    });
    mocks.metrics.enablePush.mockImplementation(
      useMetricsStore.getState().enablePush,
    );
    mocks.metrics.disablePush.mockImplementation(
      useMetricsStore.getState().disablePush,
    );
    return useMetricsStore;
  }

  function visibility(state: "hidden" | "visible") {
    vi.spyOn(document, "visibilityState", "get").mockReturnValue(state);
    document.dispatchEvent(new Event("visibilitychange"));
  }

  beforeEach(() => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.useFakeTimers();
    vi.resetAllMocks();
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    mocks.version.resumeUpgradeProgress.mockResolvedValue(undefined);
    mocks.traffic.fetchInitialData.mockResolvedValue(undefined);
    mocks.traffic.paused = false;
    mocks.traffic.polling = true;
    mocks.traffic.usePush = true;
    mocks.push.getSubscription.mockReturnValue({
      need_overview: true,
      need_metrics: true,
      settings_scopes: ["tls"],
    });
    mocks.push.onOverviewUpdate.mockReturnValue(() => {});
    mocks.push.onMetricsUpdate.mockReturnValue(() => {});
    mocks.push.onHistoryUpdate.mockReturnValue(() => {});
    mocks.push.onForceRefresh.mockImplementation((handler) => {
      forceRefreshHandlers.add(handler);
      return () => forceRefreshHandlers.delete(handler);
    });
    mocks.push.onNotification.mockImplementation((handler) => {
      notificationHandlers.add(handler);
      return () => notificationHandlers.delete(handler);
    });
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await unmount();
    container.remove();
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    forceRefreshHandlers.clear();
    notificationHandlers.clear();
  });

  it("restores visibility and push handlers after StrictMode effect replay", async () => {
    await render(true);
    expect(isGlobalDataInitialized()).toBe(true);
    expect(forceRefreshHandlers.size).toBe(1);
    expect(notificationHandlers.size).toBe(1);
    expect(mocks.metrics.enablePush).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(1);
    mocks.metrics.enablePush.mockClear();
    mocks.traffic.disablePush.mockClear();

    visibility("hidden");
    window.dispatchEvent(new Event("pagehide"));
    expect(mocks.traffic.disablePush).toHaveBeenCalledOnce();
    expect(mocks.push.disconnect).toHaveBeenCalledOnce();
    visibility("visible");
    window.dispatchEvent(new Event("pageshow"));
    expect(mocks.traffic.enablePush).toHaveBeenCalledOnce();
    expect(mocks.metrics.enablePush).toHaveBeenCalledOnce();

    // A second visibility cycle must use the same live handlers exactly once.
    visibility("hidden");
    visibility("visible");
    expect(mocks.push.disconnect).toHaveBeenCalledTimes(2);
    expect(mocks.traffic.enablePush).toHaveBeenCalledTimes(2);

    await unmount();
    expect(forceRefreshHandlers.size).toBe(0);
    expect(notificationHandlers.size).toBe(0);
    expect(isGlobalDataInitialized()).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    visibility("hidden");
    visibility("visible");
    expect(mocks.push.disconnect).toHaveBeenCalledTimes(2);
    expect(mocks.traffic.enablePush).toHaveBeenCalledTimes(2);

    root = createRoot(container);
    await render(true);
    visibility("hidden");
    expect(mocks.push.disconnect).toHaveBeenCalledTimes(3);
    expect(forceRefreshHandlers.size).toBe(1);
    expect(vi.getTimerCount()).toBe(1);
  });

  it("ignores an obsolete StrictMode initialization when its promise resolves later", async () => {
    const stale = deferred();
    mocks.version.resumeUpgradeProgress.mockReturnValueOnce(stale.promise);
    await render(true);
    expect(mocks.metrics.enablePush).toHaveBeenCalledOnce();
    expect(mocks.proxy.fetchSystemProxy).toHaveBeenCalledOnce();
    await act(async () => stale.resolve());
    expect(mocks.metrics.enablePush).toHaveBeenCalledOnce();
    expect(mocks.proxy.fetchSystemProxy).toHaveBeenCalledOnce();
    expect(mocks.syncDynamicData).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(1);
  });

  it.each(["upgrade", "global data"])(
    "does not reconnect after unmount during %s loading",
    async (phase) => {
      const pending = deferred();
      if (phase === "upgrade") {
        mocks.version.resumeUpgradeProgress.mockReturnValue(pending.promise);
      } else {
        mocks.metrics.fetchOverview.mockReturnValue(pending.promise);
      }
      await render();
      await unmount();
      await act(async () => pending.resolve());
      expect(mocks.metrics.enablePush).not.toHaveBeenCalled();
      expect(mocks.syncDynamicData).not.toHaveBeenCalled();
      expect(vi.getTimerCount()).toBe(0);
      if (phase === "upgrade")
        expect(mocks.proxy.fetchSystemProxy).not.toHaveBeenCalled();
    },
  );

  it("keeps delayed metrics initialization paused while hidden", async () => {
    const pending = deferred();
    mocks.metrics.fetchOverview.mockReturnValue(pending.promise);
    await render();
    visibility("hidden");
    await act(async () => pending.resolve());
    expect(mocks.metrics.enablePush).not.toHaveBeenCalled();
    visibility("visible");
    expect(mocks.metrics.enablePush).toHaveBeenCalledOnce();
  });

  it("keeps traffic disabled across route changes and force refresh", async () => {
    await render(true);
    await render(true, false);
    visibility("hidden");
    visibility("visible");
    expect(mocks.traffic.enablePush).not.toHaveBeenCalled();
    await render(true, true);
    visibility("hidden");
    visibility("visible");
    expect(mocks.traffic.enablePush).toHaveBeenCalledOnce();
    for (const handler of forceRefreshHandlers) handler({ reason: "upgrade" });
    expect(mocks.push.disableReconnectUntilRefresh).toHaveBeenCalledOnce();
    expect(mocks.forceRefresh.show).toHaveBeenCalledWith("upgrade");
    visibility("visible");
    expect(mocks.traffic.enablePush).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("reconnects non-traffic routes without stealing another metrics consumer's reference", async () => {
    const store = await useRealMetricsActions();
    // StatusBar independently owns a metrics subscription on every route.
    store.getState().enablePush();
    await render(true, false);
    expect(store.getState().pushRefCount).toBe(2);
    visibility("hidden");
    expect(store.getState().pushRefCount).toBe(1);
    mocks.push.connect.mockClear();
    visibility("visible");
    expect(store.getState().pushRefCount).toBe(2);
    expect(mocks.push.connect).toHaveBeenCalledExactlyOnceWith({
      need_overview: true,
      need_metrics: true,
      settings_scopes: ["tls"],
    });
    expect(mocks.traffic.enablePush).not.toHaveBeenCalled();
    visibility("hidden");
    await unmount();
    expect(store.getState().pushRefCount).toBe(1);
    store.getState().disablePush();
    expect(store.getState().pushRefCount).toBe(0);
  });

  it("owns only one metrics reference when visibility resumes before initialization finishes", async () => {
    const store = await useRealMetricsActions();
    const pending = deferred();
    mocks.metrics.fetchOverview.mockReturnValue(pending.promise);
    store.getState().enablePush();
    await render(true, false);
    expect(store.getState().pushRefCount).toBe(1);
    visibility("hidden");
    expect(store.getState().pushRefCount).toBe(1);
    visibility("visible");
    expect(store.getState().pushRefCount).toBe(2);
    await act(async () => pending.resolve());
    expect(store.getState().pushRefCount).toBe(2);
    expect(mocks.metrics.enablePush).toHaveBeenCalledOnce();
    await unmount();
    expect(store.getState().pushRefCount).toBe(1);
    store.getState().disablePush();
  });
});
