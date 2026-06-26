import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  buildAsrQueryForTest,
  buildVoiceRealtimeUrl,
  createAsrTask,
  defaultAsrParams,
  defaultModelManagementParams,
  defaultVoiceRealtimeParams,
  loadVoiceRealtimeParams,
  saveAsrParams,
  streamAsrTranscription,
  updateDailyAgentConfig,
} from "./asr";

describe("Voice realtime ASR params", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("defaults every ASR entry to the lightweight 0.6B model", () => {
    expect(defaultAsrParams()).toMatchObject({
      model: "Qwen3-ASR-0.6B",
    });
  });

  it("defaults Web realtime voice input to the shared workbench model", () => {
    expect(defaultVoiceRealtimeParams()).toMatchObject({
      model: defaultAsrParams().model,
      chunkMs: 1000,
    });
    expect(loadVoiceRealtimeParams()).toMatchObject({
      model: defaultAsrParams().model,
      chunkMs: 1000,
    });

    const url = new URL(buildVoiceRealtimeUrl(loadVoiceRealtimeParams()));
    expect(url.pathname).toBe("/_bifrost/api/voice/listen-ws");
    expect(url.searchParams.get("provider")).toBe("qwen3_stateful_streaming");
    expect(url.searchParams.get("source")).toBe("web_mic");
    expect(url.searchParams.get("model")).toBe("Qwen3-ASR-0.6B");
    expect(url.searchParams.get("owner_module")).toBe("speech_workbench");
    expect(url.searchParams.get("chunk_ms")).toBe("1000");
    expect(url.searchParams.get("allow_stateful_17b")).toBeNull();
  });

  it("inherits the workbench model for realtime voice input", () => {
    saveAsrParams({
      host: "127.0.0.1",
      language: "english",
      model: "Qwen3-ASR-0.6B",
    });

    const url = new URL(buildVoiceRealtimeUrl(loadVoiceRealtimeParams()));
    expect(url.searchParams.get("provider")).toBe("qwen3_stateful_streaming");
    expect(url.searchParams.get("model")).toBe("Qwen3-ASR-0.6B");
    expect(url.searchParams.get("allow_stateful_17b")).toBeNull();
    expect(url.searchParams.get("language")).toBe("english");
  });

  it("enables the large-model guard only when workbench selects 1.7B", () => {
    const url = new URL(
      buildVoiceRealtimeUrl({
        ...loadVoiceRealtimeParams(),
        model: "Qwen3-ASR-1.7B",
      }),
    );
    expect(url.searchParams.get("model")).toBe("Qwen3-ASR-1.7B");
    expect(url.searchParams.get("allow_stateful_17b")).toBe("1");
  });

  it("keeps model management and workbench owners isolated in ASR queries", () => {
    const workbenchQuery = new URLSearchParams(buildAsrQueryForTest(defaultAsrParams()));
    const managementQuery = new URLSearchParams(
      buildAsrQueryForTest(defaultModelManagementParams()),
    );

    expect(workbenchQuery.get("owner_module")).toBe("speech_workbench");
    expect(managementQuery.get("owner_module")).toBe("model_management");
    expect(workbenchQuery.get("model")).toBe("Qwen3-ASR-0.6B");
    expect(managementQuery.get("model")).toBe("Qwen3-ASR-0.6B");
  });
});

describe("ASR admin API CSRF headers", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("adds the admin CSRF token to unsafe ASR module requests", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/security/csrf")) {
        return new Response(
          JSON.stringify({
            csrf_token: "csrf-token-for-test",
            header_name: "X-Bifrost-CSRF",
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      if (url.includes("/transcribe-stream")) {
        return new Response('event: done\ndata: {"ok":true}\n\n', { status: 200 });
      }
      return new Response(JSON.stringify({ ok: true, id: "task-1", config: {} }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });

    vi.stubGlobal("fetch", fetchMock);

    await createAsrTask({ audio_dir: "/tmp/audio" });
    await streamAsrTranscription(
      new Blob(["sample"], { type: "audio/wav" }),
      "sample.wav",
      defaultAsrParams(),
      () => {},
    );
    await updateDailyAgentConfig("task-1", { enabled: true });

    const unsafeAsrCalls = fetchMock.mock.calls.filter(([input]) => {
      const url = String(input);
      return (
        !url.includes("/security/csrf") &&
        (url.includes("/asr/tasks") || url.includes("/asr/transcribe-stream"))
      );
    });

    expect(unsafeAsrCalls).toHaveLength(3);
    unsafeAsrCalls.forEach(([, init]) => {
      const headers = new Headers(init?.headers);
      expect(headers.get("X-Bifrost-CSRF")).toBe("csrf-token-for-test");
    });
  });
});
