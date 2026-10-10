import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ValueItem } from "../../api/values";
import { useValuesStore } from "../../stores/useValuesStore";
import Values from "./index";

const push = vi.hoisted(() => ({
  handlers: new Set<(data: { values: ValueItem[] }) => void>(),
  connect: vi.fn(),
  updateSubscription: vi.fn(),
  disconnectIfIdle: vi.fn(),
}));
vi.mock("../../services/pushService", () => ({
  default: {
    ...push,
    onValuesUpdate: (handler: (data: { values: ValueItem[] }) => void) => {
      push.handlers.add(handler);
      return () => push.handlers.delete(handler);
    },
  },
}));
vi.mock("../../api", () => ({
  getValues: vi.fn(async () => ({ values: [] })),
}));
vi.mock("../../api/client", () => ({
  notifyApiBusinessError: vi.fn(),
  isConnectionIssueError: () => false,
}));
vi.mock("../../desktop/tauri", () => ({ clearDesktopDocumentEdited: vi.fn() }));
vi.mock("../../runtime", () => ({ isMacDesktopShell: () => false }));
vi.mock("antd", () => ({ theme: { useToken: () => ({ token: {} }) } }));
vi.mock("../../components/SplitPane", () => ({ default: () => null }));
vi.mock("./ValueList", () => ({ default: () => null }));
vi.mock("./ValueEditor", () => ({ default: () => null }));

let root: Root;
let container: HTMLDivElement;
const value = (name: string): ValueItem => ({
  name,
  value: "actual snapshot",
  created_at: "2026-10-10T00:00:00Z",
  updated_at: "2026-10-10T00:00:00Z",
});
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.clearAllMocks();
  push.handlers.clear();
  useValuesStore.setState({ values: [], selectedValueName: null, error: null });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(() => root.unmount());
  container.remove();
  expect(push.handlers.size).toBe(0);
  vi.unstubAllGlobals();
});

it("keeps the live subscription when the value count changes", async () => {
  await act(() =>
    root.render(
      <MemoryRouter>
        <Values />
      </MemoryRouter>,
    ),
  );
  await act(() =>
    useValuesStore.getState().applyValuesSnapshot([value("first")]),
  );
  expect(push.handlers.size).toBe(1);
  await act(() => {
    for (const handler of push.handlers)
      handler({ values: [value("first"), value("later")] });
  });
  expect(useValuesStore.getState().values.map((item) => item.name)).toEqual([
    "first",
    "later",
  ]);
  expect(push.handlers.size).toBe(1);
});

it("restores the subscription after StrictMode effect cleanup", async () => {
  await act(() =>
    root.render(
      <StrictMode>
        <MemoryRouter>
          <Values />
        </MemoryRouter>
      </StrictMode>,
    ),
  );
  expect(push.handlers.size).toBe(1);
});
