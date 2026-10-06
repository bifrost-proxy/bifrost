import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { BrowserRouter, HashRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const runtime = vi.hoisted(() => ({ desktop: false }));

vi.mock("antd", () => ({
  theme: { useToken: () => ({ token: { colorBgContainer: "white" } }) },
  message: { warning: vi.fn() },
}));
vi.mock("../../components/SplitPane", () => ({ default: () => null }));
vi.mock("./RuleList", () => ({ default: () => null }));
vi.mock("./RuleEditor", () => ({ default: () => null }));
vi.mock("../../runtime", () => ({
  isMacDesktopShell: () => false,
  isDesktopShell: () => runtime.desktop,
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
import { useRulesStore } from "../../stores/useRulesStore";
import Rules from "./index";

describe.each(["browser", "hash"])("Rules with the actual %s router", (mode) => {
  let root: Root;
  let container: HTMLDivElement;
  const prefix = mode === "hash" ? "/#/rules" : "/rules";

  function render() {
    const Router = mode === "hash" ? HashRouter : BrowserRouter;
    root.render(<Router><Rules /></Router>);
  }

  async function navigate(params: string) {
    await act(async () => {
      window.history.pushState(null, "", `${prefix}?${params}`);
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
  }

  beforeEach(() => {
    vi.clearAllMocks();
    runtime.desktop = mode === "hash";
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    window.history.replaceState(null, "", `${prefix}?rule=Default&source=badge`);
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
      rules: ["Default", "first", "second", "a b&c#d/规则"].map((name, sort_order) => ({
        name,
        enabled: true,
        sort_order,
        rule_count: 1,
        created_at: "2026-10-05T00:00:00Z",
        updated_at: "2026-10-05T00:00:00Z",
      })),
      selectedRuleName: "Default",
      currentRule: null,
      loading: false,
      error: null,
      activeGroupId: null,
      isGroupMode: false,
      groupWritable: false,
      editingContent: {},
      savedContent: {},
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

  it("preserves selection and responds to router navigation", async () => {
    await act(async () => render());
    await act(async () => useRulesStore.getState().selectRule("first"));
    await act(async () => useRulesStore.getState().selectRule("second"));
    expect(useRulesStore.getState().selectedRuleName).toBe("second");
    expect(useRulesStore.getState().currentRule?.name).toBe("second");

    for (const name of ["first", "second", "Default"]) {
      await navigate(`rule=${name}&source=badge`);
      expect(useRulesStore.getState().selectedRuleName).toBe(name);
      expect(useRulesStore.getState().currentRule?.name).toBe(name);
    }
  });

  it("restores a cold deep link after list loading", async () => {
    window.history.replaceState(null, "", `${prefix}?rule=second&source=badge`);
    useRulesStore.setState({ selectedRuleName: null, loading: true });
    await act(async () => render());
    await act(async () => useRulesStore.setState({ loading: false }));
    expect(useRulesStore.getState().selectedRuleName).toBe("second");
    expect(useRulesStore.getState().currentRule?.name).toBe("second");
  });

  it("restores history back and forward", async () => {
    await act(async () => render());
    await navigate("rule=first&source=history");
    await navigate("rule=second&source=history");
    await act(async () => {
      const popped = new Promise<void>((resolve) =>
        window.addEventListener("popstate", () => resolve(), { once: true }),
      );
      window.history.back();
      await popped;
    });
    expect(useRulesStore.getState().selectedRuleName).toBe("first");

    await act(async () => {
      const popped = new Promise<void>((resolve) =>
        window.addEventListener("popstate", () => resolve(), { once: true }),
      );
      window.history.forward();
      await popped;
    });
    expect(useRulesStore.getState().selectedRuleName).toBe("second");
  });

  it("accepts navigation with route fragments", async () => {
    await act(async () => render());
    await navigate(
      mode === "hash"
        ? "source=badge&rule=second#section"
        : "rule=second&source=badge#section?fragment=query",
    );
    expect(useRulesStore.getState().selectedRuleName).toBe("second");
    expect(useRulesStore.getState().currentRule?.name).toBe("second");
    expect(window.location.href).toContain("source=badge");
  });

  it("accepts encoded names and preserves other parameters", async () => {
    await act(async () => render());
    await navigate(
      "source=changed&rule=a%20b%26c%23d%2F%E8%A7%84%E5%88%99&extra=a%2Bb",
    );
    expect(useRulesStore.getState().selectedRuleName).toBe("a b&c#d/规则");
    expect(useRulesStore.getState().currentRule?.name).toBe("a b&c#d/规则");
    const params = new URLSearchParams(
      mode === "hash"
        ? window.location.hash.split("?")[1]
        : window.location.search,
    );
    expect(params.get("source")).toBe("changed");
    expect(params.get("extra")).toBe("a+b");
  });
});
