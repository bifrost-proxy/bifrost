import type { AsrTranscriptionMode } from "../../api/asr";

export const DIRECTORY_TASK_MODE_HISTORY_NOTE =
  "Changing this setting applies to untranscribed and new files; existing transcript files are preserved.";

export interface DirectoryTaskModeFields {
  showMossPrompt: boolean;
  showStandardPipeline: boolean;
}

export interface DirectoryTaskModeOption {
  value: AsrTranscriptionMode;
  label: string;
  disabled?: boolean;
}

export function directoryTaskModeOptions(
  mossPlatformSupported: boolean | null,
): DirectoryTaskModeOption[] {
  const unavailableLabel =
    mossPlatformSupported === null
      ? "MOSS joint transcription (checking platform support)"
      : "MOSS joint transcription (requires Apple Silicon macOS)";
  return [
    {
      value: "standard",
      label: "Standard ASR + speaker diarization",
    },
    {
      value: "moss_joint",
      label:
        mossPlatformSupported === true
          ? "MOSS joint transcription (speaker-aware)"
          : unavailableLabel,
      disabled: mossPlatformSupported !== true,
    },
  ];
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
