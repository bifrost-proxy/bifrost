import { beforeEach, expect, it, vi } from "vitest";
import type { ValueItem } from "../api/values";
import { useValuesStore } from "./useValuesStore";
import * as api from "../api";

vi.mock("../api", () => ({ createValue: vi.fn() }));
vi.mock("../api/client", () => ({ isConnectionIssueError: () => false }));
vi.mock("../desktop/tauri", () => ({ clearDesktopDocumentEdited: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
  useValuesStore.setState({
    values: [],
    currentValue: null,
    selectedValueName: null,
  });
});

it("keeps one authoritative value when push arrives before create completes", async () => {
  let complete!: () => void;
  vi.mocked(api.createValue).mockReturnValue(
    new Promise((resolve) => {
      complete = () => resolve({ success: true });
    }),
  );
  const creating = useValuesStore.getState().createValue("new", "requested");
  const pushed: ValueItem = {
    name: "new",
    value: "requested",
    created_at: "2026-10-11T01:00:00Z",
    updated_at: "2026-10-11T01:00:00Z",
  };
  const other = { ...pushed, name: "concurrent" };
  useValuesStore.getState().applyValuesSnapshot([pushed, other]);
  complete();
  expect(await creating).toBe(true);
  expect(useValuesStore.getState().values).toEqual([other, pushed]);
  expect(useValuesStore.getState().currentValue).toEqual(pushed);
});
