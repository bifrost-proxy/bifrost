export const TASK_DETAIL_TAB_KEYS = [
  "overview",
  "files",
  "daily",
  "daily-agent",
  "daily-agent-records",
] as const;

export type DirectoryTaskDetailTabKey = (typeof TASK_DETAIL_TAB_KEYS)[number];

export function isDirectoryTaskDetailTabKey(
  value: string | null,
): value is DirectoryTaskDetailTabKey {
  return TASK_DETAIL_TAB_KEYS.includes(value as DirectoryTaskDetailTabKey);
}
