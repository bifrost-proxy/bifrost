import { act, useCallback } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const router = vi.hoisted(() => ({
  desktop: false,
  searchParams: new URLSearchParams(),
  setSearchParams: vi.fn(),
}));

vi.mock("react-router-dom", async (importOriginal) => {
  const { parsePath } = await importOriginal<typeof import("react-router-dom")>();
  return {
    parsePath,
    useSearchParams: () => {
      const params = router.searchParams;
      const setSearchParams = useCallback(
        (update: (previous: URLSearchParams) => URLSearchParams) =>
          router.setSearchParams(update, new URLSearchParams(params)),
        [params],
      );
      return [params, setSearchParams];
    },
  };
});
vi.mock("antd", () => ({
  theme: { useToken: () => ({ token: { colorBgContainer: "white" } }) },
  message: { warning: vi.fn() },
}));
vi.mock("../../components/SplitPane", () => ({ default: () => null }));
vi.mock("./RuleList", () => ({ default: () => null }));
vi.mock("./RuleEditor", () => ({ default: () => null }));
vi.mock("../../runtime", () => ({
  isMacDesktopShell: () => false,
  isDesktopShell: () => router.desktop,
}));
vi.mock("../../api", () => ({ getRule: vi.fn() }));
vi.mock("../../api/client", () => ({
  isConnectionIssueError: () => false,
  isNotFoundError: () => false,
  normalizeApiErrorMessage: vi.fn(),
  notifyApiBusinessError: vi.fn(),
}));
vi.mock("../../api/group", () => ({
  fetchGroupRules: vi.fn(),
  getGroupRule: vi.fn(),
  createGroupRule: vi.fn(),
  updateGroupRule: vi.fn(),
  deleteGroupRule: vi.fn(),
  enableGroupRule: vi.fn(),
  disableGroupRule: vi.fn(),
}));
vi.mock("../../desktop/tauri", () => ({
  clearDesktopDocumentEdited: vi.fn(),
}));
vi.mock("../../services/pushService", () => ({
  default: {
    connect: vi.fn(),
    onValuesUpdate: () => () => {},
    updateSubscription: vi.fn(),
    disconnectIfIdle: vi.fn(),
  },
}));

import * as api from "../../api";
import { fetchGroupRules, getGroupRule } from "../../api/group";
import { useRulesStore } from "../../stores/useRulesStore";
import Rules from "./index";

describe("Rules URL selection", () => {
  let root: Root;
  let container: HTMLDivElement;

  beforeEach(() => {
    vi.clearAllMocks();
    router.desktop = false;
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    window.history.replaceState(null, "", "/rules?rule=Default&source=badge");
    router.searchParams = new URLSearchParams("rule=Default&source=badge");
    // BrowserRouter writes history immediately, but publishes the new location
    // in a React transition. Keep that render pending until the test advances it.
    router.setSearchParams.mockImplementation((update, previous) => {
      const next = update(previous) as URLSearchParams;
      const prefix = window.location.hash ? "/#/rules" : "/rules";
      window.history.replaceState(null, "", `${prefix}?${next}`);
    });
    vi.mocked(api.getRule).mockImplementation(async (name) => ({
      name,
      content: name,
      enabled: true,
      sort_order: 0,
      created_at: "2026-10-05T00:00:00Z",
      updated_at: "2026-10-05T00:00:00Z",
      sync: { status: "local_only" },
    }));
    useRulesStore.setState({
      rules: ["Default", "first", "second"].map((name, sort_order) => ({
        name,
        enabled: true,
        sort_order,
        rule_count: 1,
        created_at: "2026-10-05T00:00:00Z",
        updated_at: "2026-10-05T00:00:00Z",
      })),
      selectedRuleName: "Default",
      currentRule: null,
      editingContent: {},
      savedContent: {},
      loading: false,
      error: null,
      activeGroupId: null,
      isGroupMode: false,
      groupWritable: false,
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  it.each(["browser", "hash"])(
    "keeps successive list selections while the %s router render is pending",
    async (mode) => {
      if (mode === "hash") {
        router.desktop = true;
        window.history.replaceState(null, "", "/#/rules?rule=Default&source=badge");
      }
      await act(async () => root.render(<Rules />));
      await act(async () => useRulesStore.getState().selectRule("first"));
      expect(useRulesStore.getState().selectedRuleName).toBe("first");

      await act(async () => useRulesStore.getState().selectRule("second"));

      expect(useRulesStore.getState().selectedRuleName).toBe("second");
      expect(vi.mocked(api.getRule).mock.calls.map(([name]) => name)).toEqual([
        "first",
        "second",
      ]);
      expect(window.location.href).toContain("rule=second");
      expect(window.location.href).toContain("source=badge");

      // Even an intermediate router commit must not replace the latest selection.
      router.searchParams = new URLSearchParams("rule=first&source=badge");
      await act(async () => root.render(<Rules />));
      expect(useRulesStore.getState().selectedRuleName).toBe("second");

      router.searchParams = new URLSearchParams("rule=second&source=badge");
      await act(async () => root.render(<Rules />));
      expect(useRulesStore.getState().selectedRuleName).toBe("second");
      expect(useRulesStore.getState().currentRule?.name).toBe("second");
      expect(api.getRule).toHaveBeenCalledTimes(2);
    },
  );

  it("restores the initial deep link after the list loads", async () => {
    window.history.replaceState(null, "", "/rules?rule=second&source=badge");
    router.searchParams = new URLSearchParams("rule=second&source=badge");
    useRulesStore.setState({ selectedRuleName: null, loading: true });
    await act(async () => root.render(<Rules />));

    await act(async () => useRulesStore.setState({ loading: false }));

    expect(useRulesStore.getState().selectedRuleName).toBe("second");
    expect(api.getRule).toHaveBeenCalledWith("second");
    expect(window.location.search).toContain("source=badge");
  });

  it("keeps an ordinary list selection when its URL render arrives", async () => {
    await act(async () => root.render(<Rules />));
    await act(async () => useRulesStore.getState().selectRule("first"));
    router.searchParams = new URLSearchParams(window.location.search);

    await act(async () => root.render(<Rules />));

    expect(useRulesStore.getState().selectedRuleName).toBe("first");
    expect(api.getRule).toHaveBeenCalledExactlyOnceWith("first");
    expect(window.location.search).toContain("source=badge");
  });

  it.each(["browser", "hash"])(
    "restores back/forward URL selections with the %s router",
    async (mode) => {
      router.desktop = mode === "hash";
      const prefix = mode === "hash" ? "/#/rules" : "/rules";
      window.history.replaceState(null, "", `${prefix}?rule=Default&source=badge`);
      await act(async () => root.render(<Rules />));

      for (const name of ["second", "first", "second", "Default"]) {
        window.history.replaceState(null, "", `${prefix}?rule=${name}&source=badge`);
        router.searchParams = new URLSearchParams(`rule=${name}&source=badge`);
        await act(async () => root.render(<Rules />));
        expect(useRulesStore.getState().selectedRuleName).toBe(name);
        expect(window.location.href).toContain("source=badge");
      }

      expect(vi.mocked(api.getRule).mock.calls.map(([name]) => name)).toEqual([
        "second",
        "first",
        "second",
        "Default",
      ]);
    },
  );

  it("loads a new group and its requested rule from a deep link", async () => {
    vi.mocked(fetchGroupRules).mockResolvedValue({
      group_id: "team",
      group_name: "Team",
      writable: true,
      rules: useRulesStore.getState().rules.slice(1),
    });
    vi.mocked(getGroupRule).mockResolvedValue({
      name: "second",
      content: "group rule",
      enabled: true,
      sort_order: 0,
      created_at: "2026-10-05T00:00:00Z",
      updated_at: "2026-10-05T00:00:00Z",
      sync: { status: "synced" },
    });
    await act(async () => root.render(<Rules />));
    window.history.replaceState(null, "", "/rules?group=team&rule=second&source=badge");
    router.searchParams = new URLSearchParams(window.location.search);

    await act(async () => root.render(<Rules />));

    expect(useRulesStore.getState().activeGroupId).toBe("team");
    expect(useRulesStore.getState().selectedRuleName).toBe("second");
    expect(getGroupRule).toHaveBeenCalledWith("team", "second");
    expect(window.location.search).toContain("source=badge");
  });
});
