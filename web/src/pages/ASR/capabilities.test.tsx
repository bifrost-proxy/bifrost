import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  getAsrCapabilities,
  getAsrStatus,
  getAsrTask,
  listAsrTasks,
  type AsrCapabilities,
} from "../../api/asr";
import ASR from "./index";

vi.mock("../../api/asr", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../api/asr")>()),
  getAsrCapabilities: vi.fn(),
  getAsrStatus: vi.fn().mockResolvedValue({ ready: true }),
  getSpeechPipelinesStatus: vi.fn().mockResolvedValue({}),
  getAsrTask: vi.fn().mockResolvedValue({ id: "task-1" }),
  listAsrTasks: vi.fn().mockResolvedValue([]),
}));

vi.mock("./components/ASRHomeTabs", () => ({
  default: () => <div data-testid="asr-home" />,
}));
vi.mock("./components/DirectoryTaskDetailPage", () => ({
  default: () => <div data-testid="asr-detail" />,
}));

function capabilities(supported: boolean): AsrCapabilities {
  const flag = {
    enabled: supported,
    hidden: !supported,
    platform_supported: supported,
  };
  return {
    platform: supported ? "macos" : "linux",
    arch: supported ? "aarch64" : "x86_64",
    supported_target: "macos-aarch64",
    qwen3_asr: flag,
    local_transcription: flag,
    speech_workbench: flag,
    directory_tasks: flag,
    speaker_diarization: flag,
    voiceprint: flag,
    voice_wake_asr: flag,
  };
}

describe("ASR capability gate", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.useFakeTimers();
    vi.clearAllMocks();
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it.each(["/ai/asr", "/ai/asr?asrTask=task-1"])(
    "keeps unsupported content hidden after capabilities resolve at %s",
    async (url) => {
      let resolveCapabilities!: (value: AsrCapabilities) => void;
      vi.mocked(getAsrCapabilities).mockReturnValue(
        new Promise((resolve) => {
          resolveCapabilities = resolve;
        }),
      );
      await act(async () => {
        root.render(
          <MemoryRouter initialEntries={[url]}>
            <ASR />
          </MemoryRouter>,
        );
      });
      expect(getAsrCapabilities).toHaveBeenCalledOnce();
      expect(container.querySelector("[data-testid]")).toBeNull();

      // Flush the response and React effects before testing absence, so a
      // loading screen cannot incorrectly satisfy unsupported-platform checks.
      await act(async () => resolveCapabilities(capabilities(false)));
      await act(async () => vi.advanceTimersByTimeAsync(10_000));
      expect(container.querySelector("[data-testid]")).toBeNull();
      expect(getAsrStatus).not.toHaveBeenCalled();
      expect(listAsrTasks).not.toHaveBeenCalled();
      expect(getAsrTask).not.toHaveBeenCalled();
    },
  );

  it("renders supported task details and loads their data", async () => {
    vi.mocked(getAsrCapabilities).mockResolvedValue(capabilities(true));
    await act(async () => {
      root.render(
        <MemoryRouter initialEntries={["/ai/asr?asrTask=task-1"]}>
          <ASR />
        </MemoryRouter>,
      );
    });
    await act(async () => vi.advanceTimersByTimeAsync(0));
    expect(
      container.querySelector('[data-testid="asr-detail"]'),
    ).not.toBeNull();
    expect(getAsrTask).toHaveBeenCalledWith("task-1");
    expect(getAsrStatus).toHaveBeenCalledOnce();
    expect(listAsrTasks).toHaveBeenCalledOnce();
  });
});
