import { message } from "antd";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  getDailyAgentConfig,
  getDailyAgentInstructions,
  updateDailyAgentConfig,
  type AsrDailyAgentConfigResponse,
  type AsrDailyAgentInstructionsResponse,
} from "../../../api/asr";
import DailyAgentTab from "./DailyAgentTab";

vi.mock("antd", async (importOriginal) => {
  const antd = await importOriginal<typeof import("antd")>();
  return {
    ...antd,
    message: { ...antd.message, success: vi.fn(), error: vi.fn() },
  };
});

vi.mock("../../../api/asr", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../api/asr")>()),
  getDailyAgentConfig: vi.fn(),
  getDailyAgentInstructions: vi.fn(),
  updateDailyAgentConfig: vi.fn(),
}));

vi.mock("../../../api/imGateway", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../api/imGateway")>()),
  getExternalCliConfig: vi.fn().mockResolvedValue({ runners: {} }),
  listProviders: vi.fn().mockResolvedValue([]),
  listTargets: vi.fn().mockResolvedValue([]),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function config(
  reportSyncDir = "",
  taskId = "task-a",
): AsrDailyAgentConfigResponse {
  const agent = {
    id: "daily_report",
    name: "Daily report",
    enabled: true,
    runner: "codex",
    timeout_ms: 7200000,
    trigger_policy: "manual_only" as const,
    instructions_source: "default" as const,
    im_delivery: {
      enabled: false,
      mode: "summary" as const,
      send_policy: "on_success" as const,
    },
    output_dir: "report",
  };
  return {
    task_id: taskId,
    config: {
      ...agent,
      agent_id: agent.id,
      agents: [agent],
      report_sync_dir: reportSyncDir,
    },
    last_run: {},
  };
}

function instructions(taskId = "task-a"): AsrDailyAgentInstructionsResponse {
  return { task_id: taskId, content: "Write a daily report", source: "default" };
}

describe("Daily Agent report sync directory drafts", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
    vi.stubGlobal("matchMedia", () => ({
      matches: false,
      addListener() {},
      removeListener() {},
    }));
    const getComputedStyle = window.getComputedStyle;
    vi.spyOn(window, "getComputedStyle").mockImplementation((element) =>
      getComputedStyle(element),
    );
    vi.mocked(getDailyAgentConfig).mockImplementation(async (taskId) =>
      config("", taskId),
    );
    vi.mocked(getDailyAgentInstructions).mockImplementation(async (taskId) =>
      instructions(taskId),
    );
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  async function render(taskId = "task-a", detail = false) {
    await act(async () => {
      root.render(
        <StrictMode>
          <MemoryRouter
            initialEntries={[
              detail ? "/?asrDailyAgentEdit=daily_report" : "/",
            ]}
          >
            <DailyAgentTab taskId={taskId} />
          </MemoryRouter>
        </StrictMode>,
      );
    });
  }

  function input() {
    return container.querySelector<HTMLInputElement>(
      '[data-testid="asr-daily-agent-report-sync-dir"]',
    )!;
  }

  function saveButton() {
    return container.querySelector<HTMLButtonElement>(
      '[data-testid="asr-daily-agent-report-sync-dir-save"]',
    )!;
  }

  async function clickButton(label: string) {
    const button = Array.from(container.querySelectorAll("button")).find(
      (node) => node.textContent?.trim() === label,
    );
    expect(button).toBeDefined();
    await act(async () => button!.click());
  }

  async function edit(value: string) {
    await act(async () => {
      const setValue = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!;
      setValue.call(input(), value);
      input().dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  it("preserves a draft typed while the return-to-list refresh waits for instructions", async () => {
    await render("task-a", true);
    const refresh = deferred<AsrDailyAgentInstructionsResponse>();
    vi.mocked(getDailyAgentInstructions).mockReturnValueOnce(refresh.promise);
    await clickButton("Daily Agents");
    expect(saveButton().disabled).toBe(true);
    await edit("/draft/reports");
    expect(input().value).toBe("/draft/reports");

    await act(async () => refresh.resolve(instructions()));
    expect(input().value).toBe("/draft/reports");
    expect(saveButton().disabled).toBe(false);
  });

  it("updates a clean directory from a completed refresh", async () => {
    await render();
    vi.mocked(getDailyAgentConfig).mockResolvedValueOnce(
      config("/external/reports"),
    );
    await clickButton("Refresh");
    expect(input().value).toBe("/external/reports");
    expect(saveButton().disabled).toBe(true);
  });

  it("keeps an intentionally cleared directory dirty during refresh", async () => {
    vi.mocked(getDailyAgentConfig).mockResolvedValue(config("/saved/reports"));
    await render();
    await edit("");
    await clickButton("Refresh");
    expect(input().value).toBe("");
    expect(saveButton().disabled).toBe(false);
  });

  it("saves the draft, marks it clean, and accepts later server changes", async () => {
    await render();
    await edit("/draft/reports");
    vi.mocked(updateDailyAgentConfig).mockResolvedValue({
      ok: true,
      config: config("/draft/reports").config,
    });
    vi.mocked(getDailyAgentConfig).mockResolvedValue(config("/draft/reports"));
    await act(async () => saveButton().click());
    expect(updateDailyAgentConfig).toHaveBeenCalledWith("task-a", {
      report_sync_dir: "/draft/reports",
    });
    expect(input().value).toBe("/draft/reports");
    expect(saveButton().disabled).toBe(true);

    vi.mocked(getDailyAgentConfig).mockResolvedValue(config("/new/server/path"));
    await clickButton("Refresh");
    expect(input().value).toBe("/new/server/path");
    expect(saveButton().disabled).toBe(true);
  });

  it("retains a failed save for retry, including across a refresh", async () => {
    await render();
    await edit("/draft/reports");
    vi.mocked(updateDailyAgentConfig).mockRejectedValue(
      new Error("permission denied"),
    );
    await act(async () => saveButton().click());
    expect(message.error).toHaveBeenCalledWith(
      "Failed to save sync directory: permission denied",
    );
    await clickButton("Refresh");
    expect(input().value).toBe("/draft/reports");
    expect(saveButton().disabled).toBe(false);
  });

  it("keeps the draft dirty when the server declines the save", async () => {
    await render();
    await edit("/draft/reports");
    vi.mocked(updateDailyAgentConfig).mockResolvedValue({
      ok: false,
      config: config().config,
    });
    await act(async () => saveButton().click());
    expect(message.success).not.toHaveBeenCalled();
    expect(message.error).toHaveBeenCalledWith(
      "Failed to save sync directory: Configuration was not saved",
    );
    expect(input().value).toBe("/draft/reports");
    expect(saveButton().disabled).toBe(false);
  });

  it("uses the acknowledged value even if the post-save refresh fails", async () => {
    await render();
    await edit(" /saved/reports ");
    vi.mocked(updateDailyAgentConfig).mockResolvedValue({
      ok: true,
      config: config("/saved/reports").config,
    });
    vi.mocked(getDailyAgentConfig).mockRejectedValueOnce(new Error("offline"));
    await act(async () => saveButton().click());
    expect(message.success).toHaveBeenCalledWith("Report sync directory saved");
    expect(input().value).toBe("/saved/reports");
    expect(saveButton().disabled).toBe(true);
  });

  it("does not let a pre-save refresh replace the successfully saved directory", async () => {
    await render();
    const oldRefresh = deferred<AsrDailyAgentInstructionsResponse>();
    vi.mocked(getDailyAgentInstructions).mockReturnValueOnce(oldRefresh.promise);
    await clickButton("Refresh");
    await edit("/saved/reports");
    vi.mocked(updateDailyAgentConfig).mockResolvedValue({
      ok: true,
      config: config("/saved/reports").config,
    });
    vi.mocked(getDailyAgentConfig).mockResolvedValue(config("/saved/reports"));
    await act(async () => saveButton().click());
    expect(input().value).toBe("/saved/reports");
    await act(async () => oldRefresh.resolve(instructions()));
    expect(input().value).toBe("/saved/reports");
    expect(saveButton().disabled).toBe(true);
  });

  it("resets the draft when changing tasks and ignores the previous task's late refresh", async () => {
    await render();
    await edit("/task-a/draft");
    const oldRefresh = deferred<AsrDailyAgentInstructionsResponse>();
    vi.mocked(getDailyAgentInstructions).mockReturnValueOnce(oldRefresh.promise);
    await clickButton("Refresh");
    vi.mocked(getDailyAgentConfig).mockResolvedValue(
      config("/task-b/saved", "task-b"),
    );
    await render("task-b");
    expect(input().value).toBe("/task-b/saved");
    expect(saveButton().disabled).toBe(true);
    await edit("/task-b/draft");
    await act(async () => oldRefresh.resolve(instructions()));
    expect(input().value).toBe("/task-b/draft");
    expect(saveButton().disabled).toBe(false);
  });

  it("does not mark another task's draft clean when the previous task's save completes", async () => {
    await render();
    await edit("/task-a/draft");
    const save = deferred<Awaited<ReturnType<typeof updateDailyAgentConfig>>>();
    vi.mocked(updateDailyAgentConfig).mockReturnValueOnce(save.promise);
    await act(async () => saveButton().click());
    expect(input().disabled).toBe(true);
    vi.mocked(getDailyAgentConfig).mockImplementation(async (taskId) =>
      config(`/${taskId}/saved`, taskId),
    );
    await render("task-b");
    expect(input().disabled).toBe(false);
    await edit("/task-b/draft");
    await act(async () =>
      save.resolve({
        ok: true,
        config: config("/task-a/draft").config,
      }),
    );
    expect(input().value).toBe("/task-b/draft");
    expect(saveButton().disabled).toBe(false);
    expect(updateDailyAgentConfig).toHaveBeenCalledTimes(1);
    expect(updateDailyAgentConfig).toHaveBeenCalledWith("task-a", {
      report_sync_dir: "/task-a/draft",
    });
  });
});
