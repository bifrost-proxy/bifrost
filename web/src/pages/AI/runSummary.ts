import type {
  AgentRunSummaryItem,
  AgentRunSummaryStatus,
} from "../../api/agentSummaries";

export const RUN_STATUS_LABELS: Record<AgentRunSummaryStatus, string> = {
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  stopped: "Stopped",
};

export const RUN_SOURCE_LABELS: Record<string, string> = {
  web: "Web",
  feishu: "Feishu",
  weixin: "Weixin",
  api: "API",
  schedule: "Schedule",
  asr: "ASR",
};

export function formatRunDuration(seconds: number): string {
  const safeSeconds = Math.max(0, Math.floor(seconds));
  if (safeSeconds < 60) return `${safeSeconds}s`;
  const minutes = Math.floor(safeSeconds / 60);
  if (minutes < 60) return `${minutes}m ${safeSeconds % 60}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

export function liveRunDuration(
  item: AgentRunSummaryItem,
  snapshotUpdatedAt: number,
  nowSeconds: number,
): number {
  if (item.status !== "running") return item.duration_secs;
  return item.duration_secs + Math.max(0, nowSeconds - snapshotUpdatedAt);
}

export function runSourceLabel(source: string): string {
  return RUN_SOURCE_LABELS[source] || source.toUpperCase();
}
