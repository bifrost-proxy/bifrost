import type { AsrTranscriptionMode } from "../../api/asr";
import { t } from "../../i18n";

/**
 * English copy of the history note. Prefer `directoryTaskModeHistoryNote()`,
 * which follows the current UI language.
 */
export const DIRECTORY_TASK_MODE_HISTORY_NOTE =
  "Changing this setting applies to untranscribed and new files; existing transcript files are preserved.";

export function directoryTaskModeHistoryNote(): string {
  return t("asr.directoryMode.historyNote");
}

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
      ? t("asr.directoryMode.mossChecking")
      : t("asr.directoryMode.mossUnsupported");
  return [
    {
      value: "standard",
      label: t("asr.directoryMode.standard"),
    },
    {
      value: "moss_joint",
      label:
        mossPlatformSupported === true
          ? t("asr.directoryMode.mossSupported")
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
