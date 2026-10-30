import { act, StrictMode, useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter, useNavigate } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { apiFetch } from "../../api/apiFetch";
import { useBreakpointStore } from "../../stores/useBreakpointStore";
import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import { usePerformanceModeStore } from "../../stores/usePerformanceModeStore";
import { useSearchStore } from "../../stores/useSearchStore";
import { useTrafficStore } from "../../stores/useTrafficStore";
import { encodeJsonForQueryParam } from "../../utils/urlState";
import Traffic from "./index";

vi.mock("../../api/apiFetch", () => ({ apiFetch: vi.fn() }));
vi.mock("../../api/traffic", () => ({
  getTrafficPage: vi.fn().mockResolvedValue({ records: [], has_more: false }),
  queryTraffic: vi.fn().mockResolvedValue({ records: [], has_more: false }),
}));
vi.mock("../../services/pushService", () => ({
  default: {
    connect: vi.fn(),
    getSubscription: () => ({}),
    onSettingsUpdate: () => () => {},
    disconnectIfIdle: vi.fn(),
  },
}));
vi.mock("antd", () => ({
  theme: { useToken: () => ({ token: {} }) },
  message: { error: vi.fn(), info: vi.fn(), success: vi.fn() },
  Button: () => null,
  Spin: () => null,
}));
vi.mock("@ant-design/icons", () => ({ ThunderboltOutlined: () => null }));
vi.mock("../../components/TrafficDetail", () => ({ default: () => null }));
vi.mock("../../components/Toolbar", () => ({ default: () => null }));
vi.mock("../../components/FilterBar", () => ({ default: () => null }));
vi.mock("../../components/FilterPanel", () => ({ default: () => null }));
vi.mock("../../components/SearchMode", () => ({ default: () => null }));
vi.mock("../../components/TrafficTable/VirtualTrafficTable", () => ({
  default: () => null,
}));
vi.mock("../../components/ThreeSplitPane", () => ({
  default: ({ center }: { center: ReactNode }) => center,
}));

let root: Root;
let container: HTMLDivElement;
let navigate: ReturnType<typeof useNavigate>;

function Navigation() {
  const routerNavigate = useNavigate();
  useEffect(() => {
    navigate = routerNavigate;
  }, [routerNavigate]);
  return <Traffic />;
}

async function render(entry: string, strict = false) {
  await act(async () => {
    const content = (
      <MemoryRouter initialEntries={[entry]}>
        <Navigation />
      </MemoryRouter>
    );
    root.render(strict ? <StrictMode>{content}</StrictMode> : content);
  });
}

function searchUrl(keyword = "tab_v2") {
  return `/?search=${encodeJsonForQueryParam({
    mode: "search",
    keyword,
    scope: { url: true, all: false },
  })}`;
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.mocked(apiFetch).mockReset();
  vi.mocked(apiFetch).mockImplementation(() => new Promise<Response>(() => {}));
  useSearchStore.setState(useSearchStore.getInitialState());
  useTrafficStore.setState(useTrafficStore.getInitialState());
  useFilterPanelStore.setState({
    ...useFilterPanelStore.getInitialState(),
    initialized: true,
  });
  useBreakpointStore.setState({ connectPush: vi.fn() });
  usePerformanceModeStore.setState({ superPerformanceMode: false });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
    useSearchStore.getState().cancelSearch();
  });
  container.remove();
  vi.unstubAllGlobals();
});

it.each([false, true])(
  "starts one uncancelled search with restored filters (StrictMode=%s)",
  async (strict) => {
    const params = new URLSearchParams({
      search: encodeJsonForQueryParam({
        mode: "search",
        keyword: "tab_v2",
        scope: { url: true, all: false },
      }),
      panel: encodeJsonForQueryParam({ domains: ["example.test"] }),
      toolbar: encodeJsonForQueryParam({
        protocol: ["https"],
        status: ["2xx"],
      }),
    });
    await render(`/?${params}`, strict);
    expect(apiFetch).toHaveBeenCalledTimes(1);
    const [url, request] = vi.mocked(apiFetch).mock.calls[0];
    expect(url).toBe("/_bifrost/api/search/stream");
    expect(request?.signal?.aborted).toBe(false);
    expect(useSearchStore.getState().isSearching).toBe(true);
    expect(JSON.parse(String(request?.body))).toMatchObject({
      keyword: "tab_v2",
      scope: { url: true, all: false },
      filters: {
        domains: ["example.test"],
        protocols: ["https"],
        status_ranges: ["2xx"],
      },
    });
    await act(async () => useSearchStore.getState().setKeyword("edited"));
    expect(apiFetch).toHaveBeenCalledTimes(1);
  },
);

it.each([
  "/",
  "/?search=invalid",
  `/?search=${encodeJsonForQueryParam({ mode: "normal", keyword: "tab_v2" })}`,
  searchUrl(""),
])("does not search for an inactive, invalid or empty URL: %s", async (url) => {
  await render(url);
  expect(apiFetch).not.toHaveBeenCalled();
});

it("searches again when navigating between search URLs", async () => {
  await render(searchUrl());
  await act(async () => navigate(searchUrl("another")));
  expect(apiFetch).toHaveBeenCalledTimes(2);
  const request = vi.mocked(apiFetch).mock.calls[1][1];
  expect(JSON.parse(String(request?.body)).keyword).toBe("another");
  expect(request?.signal?.aborted).toBe(false);
  await act(async () => navigate(-1));
  expect(apiFetch).toHaveBeenCalledTimes(3);
  expect(useSearchStore.getState().keyword).toBe("tab_v2");
});

it("restores the original URL after local edits without searching on each edit", async () => {
  await render(searchUrl());
  await act(async () => useSearchStore.getState().setKeyword("edited"));
  expect(apiFetch).toHaveBeenCalledTimes(1);
  await act(async () => navigate(searchUrl()));
  expect(apiFetch).toHaveBeenCalledTimes(2);
  expect(useSearchStore.getState().keyword).toBe("tab_v2");
  expect(vi.mocked(apiFetch).mock.calls[1][1]?.signal?.aborted).toBe(false);
});
