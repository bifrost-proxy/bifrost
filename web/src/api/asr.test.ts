import { beforeEach, describe, expect, it } from "vitest";
import {
  buildAsrQueryForTest,
  buildVoiceRealtimeUrl,
  defaultAsrParams,
  defaultModelManagementParams,
  defaultVoiceRealtimeParams,
  loadVoiceRealtimeParams,
  saveAsrParams,
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
