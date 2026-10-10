import { beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({
  settings: vi.fn(),
  updateSettings: vi.fn(),
  pending: vi.fn(),
  resume: vi.fn(),
  selected: vi.fn(),
  expand: vi.fn(),
  reload: vi.fn(),
  handlers: {} as Record<string, (data: unknown) => void>,
}));
vi.mock("../api/breakpoint", () => ({
  getPendingBreakpoints: mocks.pending,
  resumeBreakpoint: mocks.resume,
  getBreakpointSettings: mocks.settings,
  updateBreakpointSettings: mocks.updateSettings,
}));
vi.mock("./useTrafficStore", () => ({
  useTrafficStore: {
    setState: vi.fn(),
    getState: () => ({
      setSelectedId: mocks.selected,
      reloadRecords: mocks.reload,
    }),
  },
}));
vi.mock("./useFilterPanelStore", () => ({
  useFilterPanelStore: {
    getState: () => ({ setDetailPanelCollapsed: mocks.expand }),
  },
}));
vi.mock("../services/pushService", () => ({
  pushService: {
    onBreakpointPaused: (fn: (data: unknown) => void) => {
      mocks.handlers.paused = fn;
    },
    onBreakpointSettingsUpdated: (fn: (data: unknown) => void) => {
      mocks.handlers.settings = fn;
    },
    onBreakpointResumed: vi.fn(),
    onConnectionChange: (fn: (data: unknown) => void) => {
      mocks.handlers.connection = fn;
    },
  },
}));
import { useBreakpointStore } from "./useBreakpointStore";
const snapshot = (id: string, phase = "request", paused = 1) => ({
  request_id: id,
  phase,
  method: "POST",
  url: "http://localhost/x",
  headers: [
    ["x-test", "one"],
    ["x-test", "two"],
  ],
  body: "original",
  body_omitted: false,
  body_encoding: "utf8",
  body_representation: "decoded",
  paused_at_ms: paused,
  deadline_at_ms: 10000,
  server_now_ms: 0,
  max_body_bytes: 1024,
});
beforeEach(() => {
  vi.clearAllMocks();
  mocks.pending.mockResolvedValue([]);
  mocks.settings.mockResolvedValue({ enabled: true, max_body_bytes: 1024 });
  useBreakpointStore.setState({
    enabled: true,
    pendingOnly: false,
    autoSelected: false,
    pendingRevision: 0,
    settingsRevision: 0,
    pausedRequests: new Map(),
    pausedResponses: new Map(),
    pushInitialized: false,
  });
});
describe("breakpoint pending lifecycle", () => {
  it("clears the paused filter on gate off and keeps it clear after re-enabling", () => {
    const store = useBreakpointStore.getState();
    store.setPendingOnly(true);
    expect(useBreakpointStore.getState().pendingOnly).toBe(true);
    store.applySettings({ enabled: false, max_body_bytes: 1024 });
    expect(useBreakpointStore.getState().pendingOnly).toBe(false);
    store.setPendingOnly(true);
    expect(useBreakpointStore.getState().pendingOnly).toBe(false);
    store.applySettings({ enabled: true, max_body_bytes: 1024 });
    expect(useBreakpointStore.getState().pendingOnly).toBe(false);
  });
  it("ignores an initial stale disabled GET after a settings push and preserves the first-hit latch", async () => {
    let resolveSettings!: (value: {
      enabled: boolean;
      max_body_bytes: number;
    }) => void;
    mocks.settings.mockReturnValue(
      new Promise((resolve) => {
        resolveSettings = resolve;
      }),
    );
    useBreakpointStore.setState({ enabled: false });
    useBreakpointStore.getState().connectPush();
    mocks.handlers.settings({ enabled: true, max_body_bytes: 1024 });
    mocks.handlers.paused(snapshot("first"));
    useBreakpointStore.getState().updatePausedBody("first", "request", "draft");
    mocks.pending.mockResolvedValue([snapshot("first")]);
    resolveSettings({ enabled: false, max_body_bytes: 1024 });
    await vi.waitFor(() => expect(mocks.pending).toHaveBeenCalledTimes(1));
    await vi.waitFor(() =>
      expect(useBreakpointStore.getState().pendingLoading).toBe(false),
    );
    expect(useBreakpointStore.getState().enabled).toBe(true);
    expect(
      useBreakpointStore.getState().pausedRequests.get("first")?.body,
    ).toBe("draft");
    mocks.handlers.paused(snapshot("second"));
    expect(mocks.selected).toHaveBeenCalledTimes(1);
  });

  it("invalidates settings GET when a gate toggle starts and ignores stale enabled GET after disabling", async () => {
    let resolveGet!: (value: {
      enabled: boolean;
      max_body_bytes: number;
    }) => void;
    let resolveToggle!: (value: {
      enabled: boolean;
      max_body_bytes: number;
    }) => void;
    mocks.settings.mockReturnValue(
      new Promise((resolve) => {
        resolveGet = resolve;
      }),
    );
    mocks.updateSettings.mockReturnValue(
      new Promise((resolve) => {
        resolveToggle = resolve;
      }),
    );
    const read = useBreakpointStore.getState().fetchSettings();
    const toggle = useBreakpointStore.getState().toggleEnabled(false);
    await useBreakpointStore.getState().fetchSettings();
    expect(mocks.settings).toHaveBeenCalledTimes(1);
    resolveGet({ enabled: true, max_body_bytes: 777 });
    await read;
    expect(useBreakpointStore.getState().loading).toBe(true);
    expect(useBreakpointStore.getState().maxBodyBytes).not.toBe(777);
    resolveToggle({ enabled: false, max_body_bytes: 1024 });
    await toggle;
    expect(useBreakpointStore.getState().enabled).toBe(false);
    mocks.settings.mockReturnValue(
      new Promise((resolve) => {
        resolveGet = resolve;
      }),
    );
    const staleRead = useBreakpointStore.getState().fetchSettings();
    useBreakpointStore
      .getState()
      .applySettings({ enabled: false, max_body_bytes: 1024 });
    resolveGet({ enabled: true, max_body_bytes: 1024 });
    await staleRead;
    expect(useBreakpointStore.getState().enabled).toBe(false);
  });

  it("loads gate settings before pending on startup and never selects a second hit", async () => {
    let resolveSettings!: (value: {
      enabled: boolean;
      max_body_bytes: number;
    }) => void;
    mocks.settings.mockReturnValue(
      new Promise((resolve) => {
        resolveSettings = resolve;
      }),
    );
    mocks.pending.mockResolvedValue([snapshot("existing")]);
    useBreakpointStore.setState({ enabled: false });
    useBreakpointStore.getState().connectPush();
    expect(mocks.pending).not.toHaveBeenCalled();
    resolveSettings({ enabled: true, max_body_bytes: 1024 });
    await vi.waitFor(() =>
      expect(mocks.selected).toHaveBeenCalledWith("existing"),
    );
    mocks.handlers.paused(snapshot("new"));
    expect(mocks.selected).toHaveBeenCalledTimes(1);
    mocks.settings.mockResolvedValue({ enabled: false, max_body_bytes: 1024 });
    await useBreakpointStore.getState().fetchSettings();
    expect(useBreakpointStore.getState().pausedRequests.size).toBe(0);
    mocks.handlers.paused(snapshot("late"));
    expect(useBreakpointStore.getState().pausedRequests.size).toBe(0);
  });
  it("selects and expands only the first hit, resets only after gate changes", async () => {
    useBreakpointStore.getState().connectPush();
    await Promise.resolve();
    mocks.handlers.paused(snapshot("a"));
    mocks.handlers.paused(snapshot("b"));
    expect(mocks.selected.mock.calls).toEqual([["a"]]);
    expect(mocks.expand).toHaveBeenCalledWith(false);
    useBreakpointStore
      .getState()
      .applySettings({ enabled: true, max_body_bytes: 1024 });
    mocks.handlers.paused(snapshot("c"));
    expect(mocks.selected).toHaveBeenCalledTimes(1);
    useBreakpointStore
      .getState()
      .applySettings({ enabled: false, max_body_bytes: 1024 });
    useBreakpointStore
      .getState()
      .applySettings({ enabled: true, max_body_bytes: 1024 });
    mocks.handlers.paused(snapshot("d"));
    expect(mocks.selected).toHaveBeenLastCalledWith("d");
  });
  it("retains separate phase drafts across duplicate pushes and reconnect snapshots", async () => {
    useBreakpointStore.getState().connectPush();
    await Promise.resolve();
    mocks.handlers.paused(snapshot("a"));
    mocks.handlers.paused(snapshot("b"));
    mocks.handlers.paused(snapshot("a", "response"));
    const store = useBreakpointStore.getState();
    store.updatePausedBody("a", "request", "request draft");
    store.updatePausedBody("a", "response", "response draft");
    store.updatePausedBody("b", "request", "other draft");
    mocks.handlers.paused(snapshot("a"));
    mocks.pending.mockResolvedValue([
      snapshot("a"),
      snapshot("b"),
      snapshot("a", "response"),
    ]);
    await store.fetchPending();
    expect(useBreakpointStore.getState().pausedRequests.get("a")?.body).toBe(
      "request draft",
    );
    expect(useBreakpointStore.getState().pausedResponses.get("a")?.body).toBe(
      "response draft",
    );
    expect(useBreakpointStore.getState().pausedRequests.get("b")?.body).toBe(
      "other draft",
    );
    mocks.handlers.paused(snapshot("a", "request", 2));
    expect(useBreakpointStore.getState().pausedRequests.get("a")?.body).toBe(
      "original",
    );
  });
  it("removes resumed and closed gate items promptly", async () => {
    mocks.pending.mockResolvedValue([snapshot("a"), snapshot("b", "response")]);
    await useBreakpointStore.getState().fetchPending();
    useBreakpointStore.getState().removePaused("a", "request");
    expect(useBreakpointStore.getState().pausedRequests.size).toBe(0);
    useBreakpointStore
      .getState()
      .applySettings({ enabled: false, max_body_bytes: 1024 });
    expect(useBreakpointStore.getState().pausedResponses.size).toBe(0);
  });
});
