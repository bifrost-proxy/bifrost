import type {
  AgentRunSummaryItem,
  AgentRunSummaryStatus,
} from "../../api/agentSummaries";
import { t, type TFunction } from "../../i18n";

export function getRunStatusLabels(
  tr: TFunction = t,
): Record<AgentRunSummaryStatus, string> {
  return {
    running: tr("ai.runs.status.running"),
    completed: tr("ai.runs.status.completed"),
    failed: tr("ai.runs.status.failed"),
    stopped: tr("ai.runs.status.stopped"),
  };
}

export function getRunSourceLabels(tr: TFunction = t): Record<string, string> {
  return {
    web: tr("ai.runs.source.web"),
    feishu: tr("ai.runs.source.feishu"),
    weixin: tr("ai.runs.source.weixin"),
    api: tr("ai.runs.source.api"),
    schedule: tr("ai.runs.source.schedule"),
    asr: tr("ai.runs.source.asr"),
  };
}

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
  nowSeconds: number,
): number {
  if (item.status !== "running") return item.duration_secs;
  if (!item.start_time) return item.duration_secs;
  return Math.max(0, nowSeconds - item.start_time);
}

export function runSourceLabel(source: string, tr: TFunction = t): string {
  return getRunSourceLabels(tr)[source] || source.toUpperCase();
}
