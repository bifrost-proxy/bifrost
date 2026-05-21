import { beforeEach, describe, expect, it } from "vitest";
import {
  buildVoiceRealtimeUrl,
  defaultAsrParams,
  defaultVoiceRealtimeParams,
  loadVoiceRealtimeParams,
  saveAsrParams,
} from "./asr";

describe("Voice realtime ASR params", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("keeps offline ASR default on the high-accuracy 1.7B model", () => {
    expect(defaultAsrParams()).toMatchObject({
      model: "Qwen3-ASR-1.7B",
    });
  });

  it("defaults Web realtime voice input to the 0.6B stateful model", () => {
    expect(defaultVoiceRealtimeParams()).toMatchObject({
      model: "Qwen3-ASR-0.6B",
      chunkMs: 1000,
    });
    expect(loadVoiceRealtimeParams()).toMatchObject({
      model: "Qwen3-ASR-0.6B",
      chunkMs: 1000,
    });

    const url = new URL(buildVoiceRealtimeUrl(loadVoiceRealtimeParams()));
    expect(url.pathname).toBe("/_bifrost/api/voice/listen-ws");
    expect(url.searchParams.get("provider")).toBe("qwen3_stateful_streaming");
    expect(url.searchParams.get("source")).toBe("web_mic");
    expect(url.searchParams.get("model")).toBe("Qwen3-ASR-0.6B");
    expect(url.searchParams.get("chunk_ms")).toBe("1000");
    expect(url.searchParams.get("allow_stateful_17b")).toBeNull();
  });

  it("does not inherit the offline 1.7B model for realtime voice input", () => {
    saveAsrParams({
      host: "127.0.0.1",
      language: "english",
      model: "Qwen3-ASR-1.7B",
    });

    const url = new URL(buildVoiceRealtimeUrl(loadVoiceRealtimeParams()));
    expect(url.searchParams.get("provider")).toBe("qwen3_stateful_streaming");
    expect(url.searchParams.get("model")).toBe("Qwen3-ASR-0.6B");
    expect(url.searchParams.get("allow_stateful_17b")).toBeNull();
    expect(url.searchParams.get("language")).toBe("english");
  });

  it("uses 1.7B for realtime only when explicitly requested", () => {
    const url = new URL(
      buildVoiceRealtimeUrl({
        ...loadVoiceRealtimeParams(),
        model: "Qwen3-ASR-1.7B",
      }),
    );
    expect(url.searchParams.get("model")).toBe("Qwen3-ASR-1.7B");
    expect(url.searchParams.get("allow_stateful_17b")).toBe("1");
  });
});
