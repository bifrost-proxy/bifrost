import { beforeEach, describe, expect, it, vi } from "vitest";

async function loadStore() {
  vi.resetModules();
  return (await import("./useRuleTreeStore")).useRuleTreeStore;
}

describe("rule tree preferences", () => {
  beforeEach(() => {
    localStorage.removeItem("bifrost-rule-tree");
  });

  it("restores nested collapsed folders after reload and remembers reopening", async () => {
    const store = await loadStore();
    expect(store.getState().collapsedFoldersByGroup).toEqual({});
    store.getState().toggleFolder(null, "team/service");
    store.getState().toggleFolder(null, "team");

    const restored = await loadStore();
    expect(restored.getState().collapsedFoldersByGroup.null).toEqual([
      "team/service",
      "team",
    ]);
    restored.getState().toggleFolder(null, "team");
    const reopened = await loadStore();
    expect(reopened.getState().collapsedFoldersByGroup.null).toEqual([
      "team/service",
    ]);
    reopened.getState().toggleFolder(null, "team/service");
    expect((await loadStore()).getState().collapsedFoldersByGroup.null).toEqual(
      [],
    );
  });

  it("keeps personal and different groups independent for identical folder paths", async () => {
    const store = await loadStore();
    for (const group of [null, "null", "another-group"]) {
      store.getState().toggleFolder(group, "team");
    }
    store.getState().toggleFolder("null", "team");
    expect((await loadStore()).getState().collapsedFoldersByGroup).toEqual({
      null: ["team"],
      '"null"': [],
      '"another-group"': ["team"],
    });
  });

  it("uses default expansion when the stored JSON cannot be read", async () => {
    localStorage.setItem("bifrost-rule-tree", "invalid JSON");
    const store = await loadStore();
    expect(store.getState().collapsedFoldersByGroup).toEqual({});
    store.getState().toggleFolder(null, "team");
    expect((await loadStore()).getState().collapsedFoldersByGroup.null).toEqual(
      ["team"],
    );
  });
});
