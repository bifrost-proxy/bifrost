import type { AsrTranscriptionMode } from "../../api/asr";

export interface DirectoryTaskModeFields {
  showMossPrompt: boolean;
  showStandardPipeline: boolean;
}

export function directoryTaskModeFields(
  mode: AsrTranscriptionMode | undefined,
): DirectoryTaskModeFields {
  const mossJoint = mode === "moss_joint";
  return {
    showMossPrompt: mossJoint,
    showStandardPipeline: !mossJoint,
  };
}
