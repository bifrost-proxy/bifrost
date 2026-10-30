import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SearchResultItem } from "../../types";
import SearchResultsList from "./SearchResultsList";

const virtual = vi.hoisted(() => {
  const state = { count: 0 };
  return {
    state,
    instance: {
      getTotalSize: () => state.count * 64,
      getVirtualItems: () => [],
    },
  };
});
vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({ count }: { count: number }) => {
    virtual.state.count = count;
    return virtual.instance;
  },
}));
vi.mock("antd", () => {
  const token = {};
  return { theme: { useToken: () => ({ token }) } };
});
vi.mock("../AppIcon", () => ({ default: () => null }));

let container: HTMLDivElement;
let root: Root;
let resize: () => void;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resize = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(256);
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(
    function (this: HTMLElement) {
      return Math.max(
        256,
        parseFloat(
          (this.firstElementChild as HTMLElement)?.style.height || "0",
        ),
      );
    },
  );
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(() => root.unmount());
  container.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("short filtered search pagination", () => {
  it("continues across empty and short pages, attempts each cursor once, and stops at exhaustion", async () => {
    const load = vi.fn();
    const render = async (
      key: string,
      count = 0,
      hasMore = true,
      loading = false,
    ) => {
      await act(() =>
        root.render(
          <SearchResultsList
            results={Array(count).fill({}) as SearchResultItem[]}
            keyword="match"
            paginationKey={key}
            hasMore={hasMore}
            isLoadingMore={loading}
            onLoadMore={load}
            onSelect={() => {}}
            onDoubleClick={() => {}}
          />,
        ),
      );
    };
    await render("cursor-100");
    expect(load).toHaveBeenCalledTimes(1);
    await act(() => resize());
    await render("cursor-100", 0, true, true);
    await render("cursor-100");
    expect(load).toHaveBeenCalledTimes(1);
    await render("cursor-50");
    expect(load).toHaveBeenCalledTimes(2);
    await render("cursor-25", 1);
    expect(load).toHaveBeenCalledTimes(3);
    await render("cursor-0", 2, false);
    expect(load).toHaveBeenCalledTimes(3);
  });

  it("keeps long lists on scroll pagination and waits for active search/loading", async () => {
    const load = vi.fn();
    await act(() =>
      root.render(
        <SearchResultsList
          results={Array(10).fill({}) as SearchResultItem[]}
          keyword="match"
          paginationKey="long"
          hasMore
          isLoadingMore={false}
          onLoadMore={load}
          onSelect={() => {}}
          onDoubleClick={() => {}}
        />,
      ),
    );
    expect(load).not.toHaveBeenCalled();
    await act(() =>
      root.render(
        <SearchResultsList
          results={[]}
          keyword="match"
          paginationKey="loading"
          hasMore
          isLoadingMore
          onLoadMore={load}
          onSelect={() => {}}
          onDoubleClick={() => {}}
        />,
      ),
    );
    expect(load).not.toHaveBeenCalled();
  });
});

it("updates the virtual height when batches grow with a stable virtualizer instance", async () => {
  const render = async (count: number) =>
    act(() =>
      root.render(
        <SearchResultsList
          results={Array(count).fill({}) as SearchResultItem[]}
          keyword="match"
          hasMore={false}
          isLoadingMore={false}
          onLoadMore={() => {}}
          onSelect={() => {}}
          onDoubleClick={() => {}}
        />,
      ),
    );
  await render(10);
  const list = container.firstElementChild!.firstElementChild as HTMLElement;
  expect(list.style.height).toBe("640px");
  await render(30);
  expect(container.firstElementChild!.firstElementChild).toBe(list);
  expect(list.style.height).toBe("1920px");
});
