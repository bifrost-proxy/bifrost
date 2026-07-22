import { describe, expect, it } from "vitest";
import {
  DIRECTORY_TASK_MODE_HISTORY_NOTE,
  directoryTaskModeFields,
  directoryTaskModeOptions,
} from "./directoryTaskMode";

describe("directoryTaskModeFields", () => {
  it("shows only the standard pipeline fields for standard and initial forms", () => {
    expect(directoryTaskModeFields(undefined)).toEqual({
      showMossPrompt: false,
      showStandardPipeline: true,
    });
    expect(directoryTaskModeFields("standard")).toEqual({
      showMossPrompt: false,
      showStandardPipeline: true,
    });
  });

  it("shows only the MOSS prompt for MOSS joint transcription", () => {
    expect(directoryTaskModeFields("moss_joint")).toEqual({
      showMossPrompt: true,
      showStandardPipeline: false,
    });
  });

  it("enables MOSS only after the backend confirms Apple Silicon support", () => {
    expect(directoryTaskModeOptions(true)[1]).toMatchObject({
      value: "moss_joint",
      disabled: false,
    });
    expect(directoryTaskModeOptions(false)[1]).toMatchObject({
      value: "moss_joint",
      disabled: true,
      label: expect.stringContaining("requires Apple Silicon macOS"),
    });
    expect(directoryTaskModeOptions(null)[1]).toMatchObject({
      value: "moss_joint",
      disabled: true,
      label: expect.stringContaining("checking platform support"),
    });
  });

  it("explains that mode changes preserve historical transcripts", () => {
    expect(DIRECTORY_TASK_MODE_HISTORY_NOTE).toContain("existing transcript files are preserved");
  });
});
