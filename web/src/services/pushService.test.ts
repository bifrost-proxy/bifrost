import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./clientId", () => ({ getClientId: () => "test-client" }));
vi.mock("../runtime", () => ({
  buildWsUrl: (path: string, params?: URLSearchParams) =>
    `ws://localhost${path}?${params ?? ""}`,
}));

class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 3;
  static instances: FakeWebSocket[] = [];
  readyState = FakeWebSocket.CONNECTING;
  onopen: (() => void) | null = null;
  onclose: ((event: { code: number; reason: string }) => void) | null = null;
  onerror: ((error: unknown) => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  send = vi.fn();
  close = vi.fn(() => { this.readyState = FakeWebSocket.CLOSED; });
  readonly url: string;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }

  open() {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.();
  }

  receive(type: string, data: unknown) {
    this.onmessage?.({ data: JSON.stringify({ type, data }) });
  }
}

describe("Push service traffic cursor reset", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
    FakeWebSocket.instances = [];
    vi.stubGlobal("WebSocket", FakeWebSocket);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("reopens with all shared subscriptions and retires the previous socket", async () => {
    const { default: pushService } = await import("./pushService");
    const connectionChanged = vi.fn();
    const trafficReceived = vi.fn();
    const overviewReceived = vi.fn();
    pushService.onConnectionChange(connectionChanged);
    pushService.onTrafficDelta(trafficReceived);
    pushService.onOverviewUpdate(overviewReceived);
    pushService.connect({
      need_traffic: true,
      need_overview: true,
      need_metrics: true,
      need_values: true,
      need_history: true,
      history_limit: 120,
      last_sequence: 104,
      last_traffic_id: "old-record",
      pending_ids: ["old-pending"],
    });
    const oldSocket = FakeWebSocket.instances[0];
    oldSocket.open();
    oldSocket.receive("connected", { client_id: 1 });
    const lateOpen = oldSocket.onopen!;
    const lateClose = oldSocket.onclose!;
    const lateError = oldSocket.onerror!;
    const lateMessage = oldSocket.onmessage!;
    // Another subscription owner can update the socket after creation.
    pushService.updateSubscription({ settings_scopes: ["proxy_settings"] });
    pushService.resetTrafficCursor();

    expect(oldSocket.close).toHaveBeenCalledTimes(1);
    expect(oldSocket.onopen).toBeNull();
    expect(oldSocket.onclose).toBeNull();
    expect(oldSocket.onerror).toBeNull();
    expect(oldSocket.onmessage).toBeNull();
    const newSocket = FakeWebSocket.instances[1];
    const params = new URL(newSocket.url).searchParams;
    for (const name of ["last_sequence", "last_traffic_id", "pending_ids"]) {
      expect(params.has(name)).toBe(false);
    }
    for (const name of ["need_traffic", "need_overview", "need_metrics", "need_values", "need_history"]) {
      expect(params.get(name)).toBe("true");
    }
    newSocket.open();
    expect(JSON.parse(newSocket.send.mock.calls[0][0])).toEqual({
      need_traffic: true,
      need_overview: true,
      need_metrics: true,
      need_values: true,
      need_history: true,
      history_limit: 120,
      settings_scopes: ["proxy_settings"],
      pending_ids: [],
    });
    newSocket.receive("connected", { client_id: 2 });
    newSocket.receive("traffic_delta", { inserts: [{ id: "first", seq: 1 }] });
    newSocket.receive("overview_update", { traffic: { recorded: 1 } });
    expect(trafficReceived).toHaveBeenCalledOnce();
    expect(overviewReceived).toHaveBeenCalledOnce();
    expect(connectionChanged.mock.calls).toEqual([
      [{ connected: true, clientId: 1 }],
      [{ connected: false, clientId: undefined }],
      [{ connected: true, clientId: 2 }],
    ]);
    // Late events from the retired socket cannot send on or close its replacement.
    oldSocket.receive("disconnect", { reason: "old connection" });
    oldSocket.onclose?.({ code: 1000, reason: "retired" });
    lateOpen();
    lateClose({ code: 1000, reason: "retired" });
    lateError(new Error("old socket"));
    lateMessage({ data: JSON.stringify({ type: "disconnect", data: { reason: "old" } }) });
    await vi.runAllTimersAsync();
    expect(FakeWebSocket.instances).toHaveLength(2);
    expect(pushService.isConnected()).toBe(true);
    expect(newSocket.send).toHaveBeenCalledOnce();
    pushService.updateSubscription({ last_sequence: 1, last_traffic_id: "first" });
    newSocket.onclose?.({ code: 1006, reason: "retry" });
    await vi.advanceTimersByTimeAsync(3000);
    expect(new URL(FakeWebSocket.instances[2].url).searchParams.get("last_sequence")).toBe("1");
    pushService.disconnect();
  });

  it("does not reconnect after a force-refresh disconnect", async () => {
    const { default: pushService } = await import("./pushService");
    pushService.connect({ need_traffic: true, last_sequence: 100 });
    const socket = FakeWebSocket.instances[0];
    socket.open();
    socket.receive("disconnect", { reason: "refresh required" });
    pushService.resetTrafficCursor();
    await vi.runAllTimersAsync();
    expect(FakeWebSocket.instances).toHaveLength(1);
    expect(pushService.isConnected()).toBe(false);
  });
});
