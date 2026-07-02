import { describe, expect, it } from "vitest";

import {
  encodePcm16Chunk,
  hasRunningDailyAgent,
  resampleFloat32Linear,
  VOICE_REALTIME_SAMPLE_RATE,
} from "./asrUtils";

describe("ASR realtime audio helpers", () => {
  it("keeps already-normalized 16k samples unchanged before PCM16 encoding", () => {
    const samples = new Float32Array([0, 0.5, -0.5, 1]);
    const encoded = encodePcm16Chunk(samples, VOICE_REALTIME_SAMPLE_RATE);

    expect(encoded.byteLength).toBe(samples.length * 2);
    const view = new DataView(encoded);
    expect(view.getInt16(0, true)).toBe(0);
    expect(view.getInt16(2, true)).toBe(16383);
    expect(view.getInt16(4, true)).toBe(-16384);
    expect(view.getInt16(6, true)).toBe(32767);
  });

  it("resamples browser 48k microphone frames to 16k before PCM16 encoding", () => {
    const samples = new Float32Array(480);
    for (let index = 0; index < samples.length; index += 1) {
      samples[index] = Math.sin(index / 10);
    }

    const resampled = resampleFloat32Linear(samples, 48_000, VOICE_REALTIME_SAMPLE_RATE);
    expect(resampled.length).toBe(160);
    expect(encodePcm16Chunk(samples, 48_000).byteLength).toBe(160 * 2);
  });
});

describe("Daily Agent status helpers", () => {
  it("detects a running child agent even when the top-level run is stale", () => {
    expect(
      hasRunningDailyAgent(
        {
          last_status: "failed",
          agents: [
            {
              id: "daily_report",
              name: "daily_report",
              enabled: true,
              runner: "gpt",
              timeout_ms: 1000,
              trigger_policy: "manual_only",
              instructions_source: "custom",
              im_delivery: { enabled: false, mode: "full_report", send_policy: "always" },
              output_dir: "daily_report",
              last_status: "failed",
            },
            {
              id: "tomorrow_todo",
              name: "tomorrow_todo",
              enabled: true,
              runner: "gpt",
              timeout_ms: 1000,
              trigger_policy: "manual_only",
              instructions_source: "custom",
              im_delivery: { enabled: false, mode: "full_report", send_policy: "always" },
              output_dir: "tomorrow_todo",
              last_status: "running",
            },
          ],
        },
        "tomorrow_todo",
      ),
    ).toBe(true);
  });

  it("does not treat another running child agent as the selected agent running", () => {
    expect(
      hasRunningDailyAgent(
        {
          agents: [
            {
              id: "daily_report",
              name: "daily_report",
              enabled: true,
              runner: "gpt",
              timeout_ms: 1000,
              trigger_policy: "manual_only",
              instructions_source: "custom",
              im_delivery: { enabled: false, mode: "full_report", send_policy: "always" },
              output_dir: "daily_report",
              last_status: "running",
            },
            {
              id: "tomorrow_todo",
              name: "tomorrow_todo",
              enabled: true,
              runner: "gpt",
              timeout_ms: 1000,
              trigger_policy: "manual_only",
              instructions_source: "custom",
              im_delivery: { enabled: false, mode: "full_report", send_policy: "always" },
              output_dir: "tomorrow_todo",
              last_status: "interrupted",
            },
          ],
        },
        "tomorrow_todo",
      ),
    ).toBe(false);
  });
});
