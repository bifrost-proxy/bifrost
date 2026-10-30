import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { apiFetch } from "../../api/apiFetch";
import { useSearchStore } from "../../stores/useSearchStore";
import { useTrafficStore } from "../../stores/useTrafficStore";
import SearchMode from "./index";
vi.mock("../../api/apiFetch", () => ({ apiFetch: vi.fn() }));
vi.mock("antd", () => ({
  theme: { useToken: () => ({ token: {} }) },
  Typography: {
    Text: ({ children }: { children: ReactNode }) => <span>{children}</span>,
  },
  Input: () => null,
  Checkbox: () => null,
  Empty: () => null,
  Spin: () => null,
  Button: ({
    children,
    onClick,
    ...props
  }: {
    children: ReactNode;
    onClick: () => void;
    "data-testid"?: string;
  }) => (
    <button data-testid={props["data-testid"]} onClick={onClick}>
      {children}
    </button>
  ),
  Space: ({ children }: { children: ReactNode }) => <div>{children}</div>,
}));
vi.mock("./SearchResultsList", () => ({
  default: () => <div data-testid="results" />,
}));
let root: Root, container: HTMLDivElement;
beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  useSearchStore.getState().reset();
  useSearchStore.getState().setMode("search");
  useSearchStore.getState().setKeyword("match");
  useTrafficStore.setState(useTrafficStore.getInitialState());
  vi.mocked(apiFetch).mockReset();
  vi.mocked(apiFetch).mockImplementation(
    async () =>
      new Response(
        'event: done\ndata: {"total_searched":10,"total_matched":0,"next_cursor":null,"has_more":false,"search_id":"a"}\n\n',
        { headers: { "content-type": "text/event-stream" } },
      ),
  );
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(() => root.unmount());
  container.remove();
  useSearchStore.getState().reset();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
it("keeps completed search stable through continuous traffic reset/overflow; only submit restarts it", async () => {
  await act(() =>
    root.render(<SearchMode onSelect={() => {}} onDoubleClick={() => {}} />),
  );
  await act(async () => {
    container
      .querySelector<HTMLButtonElement>('[data-testid="search-mode-submit"]')!
      .click();
  });
  expect(apiFetch).toHaveBeenCalledTimes(1);
  const completed = useSearchStore.getState();
  for (let i = 1; i <= 4; i++) {
    await act(async () => {
      useTrafficStore.setState({
        recordsMutation: {
          ...useTrafficStore.getState().recordsMutation,
          version: i,
          reset: true,
          incomplete: true,
        },
      });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1100);
    });
    expect(apiFetch).toHaveBeenCalledTimes(1);
    expect(useSearchStore.getState().results).toBe(completed.results);
    expect(useSearchStore.getState().isSearching).toBe(false);
    expect(useSearchStore.getState().searchId).toBe("a");
  }
  vi.mocked(apiFetch).mockImplementation(
    async () =>
      new Response(
        'event: done\ndata: {"total_searched":11,"total_matched":0,"next_cursor":null,"has_more":false,"search_id":"b"}\n\n',
        { headers: { "content-type": "text/event-stream" } },
      ),
  );
  await act(async () => {
    container
      .querySelector<HTMLButtonElement>('[data-testid="search-mode-submit"]')!
      .click();
  });
  expect(apiFetch).toHaveBeenCalledTimes(2);
  expect(useSearchStore.getState().searchId).toBe("b");
});
