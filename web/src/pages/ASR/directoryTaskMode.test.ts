import { describe, expect, it } from "vitest";
import { directoryTaskModeFields } from "./directoryTaskMode";

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
});
