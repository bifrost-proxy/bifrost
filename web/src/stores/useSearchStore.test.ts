import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { apiFetch } from "../api/apiFetch";
import type {
  SearchFilters,
  SearchResultItem,
  TrafficSummaryCompact,
} from "../types";
import { mergeSearchResults, useSearchStore as store } from "./useSearchStore";
vi.mock("../api/apiFetch", () => ({ apiFetch: vi.fn() }));
const filters: SearchFilters = {
  protocols: [],
  status_ranges: [],
  content_types: [],
  conditions: [],
  client_ips: [],
  client_apps: [],
  account_names: [],
  domains: [],
};
const result = (id: string, seq = 1): SearchResultItem => ({
  record: { id, seq, p: `/${id}` } as TrafficSummaryCompact,
  matches: [],
});
const done = {
  total_searched: 10,
  total_matched: 2,
  has_more: true,
  next_cursor: 5,
  search_id: "session",
};
function stream(events: Array<[string, unknown]>) {
  return new Response(
    events
      .map(
        ([event, data]) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`,
      )
      .join(""),
    { headers: { "content-type": "text/event-stream" } },
  );
}
const page = (...items: SearchResultItem[]) =>
  stream([
    ...items.map((item) => ["result", item] as [string, unknown]),
    ["done", done],
  ]);
beforeEach(() => {
  store.getState().reset();
  store.getState().setMode("search");
  store.getState().setKeyword("match");
  vi.mocked(apiFetch).mockReset();
});
afterEach(() => store.getState().reset());
it("merges by stable ID without reordering or recreating untouched rows", () => {
  const a = result("a"),
    b = result("b"),
    updated = result("a", 3);
  const merged = mergeSearchResults([a, b], [updated, result("c"), updated]);
  expect(merged.map((x) => x.record.id)).toEqual(["a", "b", "c"]);
  expect(merged[0]).toBe(updated);
  expect(merged[1]).toBe(b);
});
it("streams and pages incrementally, deduplicates overlap and stays complete", async () => {
  vi.mocked(apiFetch)
    .mockResolvedValueOnce(page(result("a"), result("a", 2), result("b")))
    .mockResolvedValueOnce(
      stream([
        ["result", result("b", 3)],
        ["result", result("c")],
        ["done", { ...done, has_more: false, next_cursor: null }],
      ]),
    );
  await store.getState().search(filters);
  const first = store.getState().results[0];
  expect(store.getState().results.map((x) => x.record.id)).toEqual(["a", "b"]);
  expect(store.getState().isSearching).toBe(false);
  await store.getState().loadMore(filters);
  expect(store.getState().results.map((x) => x.record.id)).toEqual([
    "a",
    "b",
    "c",
  ]);
  expect(store.getState().results[0]).toBe(first);
  expect(store.getState().isLoadingMore).toBe(false);
  await store.getState().loadMore(filters);
  expect(apiFetch).toHaveBeenCalledTimes(2);
  vi.mocked(apiFetch).mockResolvedValueOnce(page(result("fresh")));
  await store.getState().search(filters);
  expect(apiFetch).toHaveBeenCalledTimes(3);
  expect(store.getState().results.map((x) => x.record.id)).toEqual(["fresh"]);
});
it("ignores a superseded response before fallback and cannot overwrite the new loader", async () => {
  const pending: Array<(r: Response) => void> = [];
  vi.mocked(apiFetch).mockImplementation(
    () => new Promise((r) => pending.push(r)),
  );
  const old = store.getState().search(filters);
  store.getState().setKeyword("new");
  const current = store.getState().search(filters);
  pending[0](new Response("{}"));
  await old;
  expect(apiFetch).toHaveBeenCalledTimes(2);
  expect(store.getState().isSearching).toBe(true);
  pending[1](page(result("new")));
  await current;
  expect(store.getState().results.map((x) => x.record.id)).toEqual(["new"]);
});
it.each(["cancelSearch", "reset", "setKeyword", "setScope"] as const)(
  "invalidates late streaming results on %s",
  async (action) => {
    let send!: (r: Response) => void;
    vi.mocked(apiFetch).mockImplementation(
      () =>
        new Promise((r) => {
          send = r;
        }),
    );
    const pending = store.getState().search(filters);
    if (action === "setKeyword") store.getState().setKeyword("draft");
    else if (action === "setScope") store.getState().setScope({ url: true });
    else store.getState()[action]();
    send(page(result("late")));
    await pending;
    expect(store.getState().results).toEqual([]);
    expect(store.getState().isSearching).toBe(false);
  },
);
it("does not paginate edited queries or changed filters until explicit submission", async () => {
  vi.mocked(apiFetch).mockResolvedValue(page(result("a")));
  await store.getState().search(filters);
  await store.getState().loadMore({ ...filters, protocols: ["https"] });
  expect(apiFetch).toHaveBeenCalledTimes(1);
  store.getState().setKeyword("draft");
  await store.getState().loadMore(filters);
  expect(apiFetch).toHaveBeenCalledTimes(1);
  expect(store.getState().submittedKeyword).toBe("match");
  expect(store.getState().results.map((x) => x.record.id)).toEqual(["a"]);
});
it("ignores an old page arriving after a new explicit search", async () => {
  vi.mocked(apiFetch).mockResolvedValueOnce(page(result("old")));
  await store.getState().search(filters);
  let send!: (r: Response) => void;
  vi.mocked(apiFetch).mockImplementationOnce(
    () =>
      new Promise((r) => {
        send = r;
      }),
  );
  const more = store.getState().loadMore(filters);
  store.getState().setKeyword("new");
  vi.mocked(apiFetch).mockResolvedValueOnce(page(result("new")));
  await store.getState().search(filters);
  send(page(result("stale")));
  await more;
  expect(store.getState().results.map((x) => x.record.id)).toEqual(["new"]);
});

it("keeps visible rows and the loading state while later stream batches arrive", async () => {
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({
    start(value) {
      controller = value;
    },
  });
  vi.mocked(apiFetch).mockResolvedValue(
    new Response(body, { headers: { "content-type": "text/event-stream" } }),
  );
  const pending = store.getState().search(filters);
  const send = async (event: string, data: unknown) => {
    controller.enqueue(
      new TextEncoder().encode(
        `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`,
      ),
    );
    await vi.waitFor(() =>
      expect(store.getState().results.length).toBeGreaterThan(0),
    );
  };
  await send("result", result("a"));
  const a = store.getState().results[0];
  expect(store.getState().isSearching).toBe(true);
  await send("result", result("b"));
  await vi.waitFor(() => expect(store.getState().results).toHaveLength(2));
  expect(store.getState().results[0]).toBe(a);
  expect(store.getState().isSearching).toBe(true);
  await send("done", { ...done, has_more: false, next_cursor: null });
  controller.close();
  await pending;
  expect(store.getState().isSearching).toBe(false);
  expect(store.getState().results[0]).toBe(a);
});

it("keeps already streamed rows when stopped and ignores the remaining old stream", async () => {
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({
    start(value) {
      controller = value;
    },
  });
  vi.mocked(apiFetch).mockResolvedValue(
    new Response(body, { headers: { "content-type": "text/event-stream" } }),
  );
  const pending = store.getState().search(filters);
  controller.enqueue(
    new TextEncoder().encode(
      `event: result\ndata: ${JSON.stringify(result("kept"))}\n\n`,
    ),
  );
  await vi.waitFor(() => expect(store.getState().results).toHaveLength(1));
  const rows = store.getState().results;
  store.getState().cancelSearch();
  controller.enqueue(
    new TextEncoder().encode(
      `event: result\ndata: ${JSON.stringify(result("stale"))}\n\n`,
    ),
  );
  controller.close();
  await pending;
  expect(store.getState().results).toBe(rows);
  expect(store.getState().isSearching).toBe(false);
  await store.getState().loadMore(filters);
  expect(apiFetch).toHaveBeenCalledTimes(1);
});
