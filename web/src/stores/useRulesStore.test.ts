import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../api", () => ({
  getRules: vi.fn(),
  getRule: vi.fn(),
  updateRule: vi.fn(),
}));

vi.mock("../api/group", () => ({
  fetchGroupRules: vi.fn(),
  getGroupRule: vi.fn(),
  createGroupRule: vi.fn(),
  updateGroupRule: vi.fn(),
  deleteGroupRule: vi.fn(),
  enableGroupRule: vi.fn(),
  disableGroupRule: vi.fn(),
}));

vi.mock("../desktop/tauri", () => ({
  clearDesktopDocumentEdited: vi.fn(),
}));

import { clearDesktopDocumentEdited } from "../desktop/tauri";
import { useRulesStore } from "./useRulesStore";
import * as api from "../api";

describe("useRulesStore.saveCurrentRule", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(clearDesktopDocumentEdited).mockResolvedValue(undefined);
    useRulesStore.setState({
      rules: [],
      currentRule: null,
      selectedRuleName: null,
      editingContent: {},
      savedContent: {},
      searchKeyword: "",
      loading: false,
      saving: false,
      error: null,
      activeGroupId: null,
      isGroupMode: false,
      groupWritable: false,
    });
  });

  it("clears desktop document edited state when save is a no-op after undo restored original content", async () => {
    useRulesStore.setState({
      selectedRuleName: "demo.rule",
      currentRule: {
        name: "demo.rule",
        content: "A",
        enabled: true,
        sort_order: 0,
        created_at: "2026-04-18T00:00:00Z",
        updated_at: "2026-04-18T00:00:00Z",
        sync: { status: "local_only" },
      },
      editingContent: {
        "demo.rule": "A",
      },
      savedContent: {
        "demo.rule": "A",
      },
    });

    const success = await useRulesStore.getState().saveCurrentRule();

    expect(success).toBe(true);
    expect(clearDesktopDocumentEdited).toHaveBeenCalledTimes(1);
    expect(useRulesStore.getState().editingContent["demo.rule"]).toBeUndefined();
  });

  it("clears desktop document edited state after persisting changed content", async () => {
    vi.mocked(api.updateRule).mockResolvedValue({ success: true });
    vi.mocked(api.getRules).mockResolvedValue([]);
    vi.mocked(api.getRule).mockResolvedValue({
      name: "demo.rule",
      content: "AB",
      enabled: true,
      sort_order: 0,
      created_at: "2026-04-18T00:00:00Z",
      updated_at: "2026-04-18T00:00:01Z",
      sync: { status: "local_only" },
    });

    useRulesStore.setState({
      selectedRuleName: "demo.rule",
      currentRule: {
        name: "demo.rule",
        content: "A",
        enabled: true,
        sort_order: 0,
        created_at: "2026-04-18T00:00:00Z",
        updated_at: "2026-04-18T00:00:00Z",
        sync: { status: "local_only" },
      },
      editingContent: {
        "demo.rule": "AB",
      },
      savedContent: {
        "demo.rule": "A",
      },
    });

    const success = await useRulesStore.getState().saveCurrentRule();

    expect(success).toBe(true);
    expect(api.updateRule).toHaveBeenCalledWith("demo.rule", "AB");
    expect(clearDesktopDocumentEdited).toHaveBeenCalledTimes(1);
  });
});
