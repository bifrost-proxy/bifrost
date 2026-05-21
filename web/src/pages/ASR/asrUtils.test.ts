import { describe, expect, it } from "vitest";

import {
  encodePcm16Chunk,
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
