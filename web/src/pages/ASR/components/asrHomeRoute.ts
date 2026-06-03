export const ASR_HOME_TAB_KEYS = ["scheduled", "management", "voice"] as const;

export type AsrHomeTabKey = (typeof ASR_HOME_TAB_KEYS)[number];

export function isAsrHomeTabKey(value: string | null): value is AsrHomeTabKey {
  return ASR_HOME_TAB_KEYS.includes(value as AsrHomeTabKey);
}
