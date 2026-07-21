import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  buildAsrQueryForTest,
  buildVoiceRealtimeUrl,
  createAsrAssistedVoiceprintSession,
  createAsrTask,
  deleteAsrAssistedVoiceprintSession,
  deleteAsrSpeakerProfileSample,
  defaultAsrParams,
  defaultModelManagementParams,
  defaultVoiceRealtimeParams,
  loadVoiceRealtimeParams,
  saveAsrParams,
  finishAsrAssistedVoiceprintSession,
  streamAsrTranscription,
  updateAsrAssistedVoiceprintLabels,
  updateDailyAgentConfig,
} from "./asr";
import { clearAdminCsrfToken } from "./csrf";

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
    clearAdminCsrfToken();
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

  it("refreshes a stale admin CSRF token and retries unsafe fetch requests once", async () => {
    let csrfRequestCount = 0;
    let updateRequestCount = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/security/csrf")) {
        csrfRequestCount += 1;
        return new Response(
          JSON.stringify({
            csrf_token: csrfRequestCount === 1 ? "stale-token" : "fresh-token",
            header_name: "X-Bifrost-CSRF",
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      if (url.includes("/asr/tasks/task-1/daily-agent")) {
        updateRequestCount += 1;
        const headers = new Headers(init?.headers);
        if (headers.get("X-Bifrost-CSRF") !== "fresh-token") {
          return new Response(
            JSON.stringify({ error: "Missing or invalid admin CSRF token" }),
            { status: 403, headers: { "Content-Type": "application/json" } },
          );
        }
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });

    vi.stubGlobal("fetch", fetchMock);

    await updateDailyAgentConfig("task-1", { enabled: true });

    expect(csrfRequestCount).toBe(2);
    expect(updateRequestCount).toBe(2);
  });

  it("sends assisted voiceprint mutations to encoded endpoints with server-owned labels", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/security/csrf")) {
        return new Response(
          JSON.stringify({
            csrf_token: "voiceprint-csrf",
            header_name: "X-Bifrost-CSRF",
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      return new Response(JSON.stringify({ ok: true, session: {}, profile: {} }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });
    vi.stubGlobal("fetch", fetchMock);

    await createAsrAssistedVoiceprintSession({
      name: "Eden",
      task_id: "task/one",
      file_key: "meeting.wav",
    });
    await updateAsrAssistedVoiceprintLabels("session/one", [
      { candidate_id: "candidate-1", label: "mine" },
    ]);
    await finishAsrAssistedVoiceprintSession("session/one");
    await deleteAsrAssistedVoiceprintSession("session/one");
    await deleteAsrSpeakerProfileSample("profile/one", "sample/one");

    const calls = fetchMock.mock.calls.filter(
      ([input]) => !String(input).includes("/security/csrf"),
    );
    expect(calls).toHaveLength(5);
    expect(String(calls[0][0])).toContain("/asr/speaker-profiles/assisted-sessions");
    expect(JSON.parse(String(calls[0][1]?.body))).toEqual({
      name: "Eden",
      task_id: "task/one",
      file_key: "meeting.wav",
    });
    expect(String(calls[1][0])).toContain("session%2Fone/labels");
    expect(JSON.parse(String(calls[1][1]?.body))).toEqual({
      labels: [{ candidate_id: "candidate-1", label: "mine" }],
    });
    expect(String(calls[2][0])).toContain("session%2Fone/finish");
    expect(String(calls[3][0])).toContain("assisted-sessions/session%2Fone");
    expect(String(calls[4][0])).toContain("profile%2Fone/samples/sample%2Fone");
    calls.forEach(([, init]) => {
      expect(new Headers(init?.headers).get("X-Bifrost-CSRF")).toBe("voiceprint-csrf");
    });
  });
});
