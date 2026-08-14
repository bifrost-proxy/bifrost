import { apiFetch } from "./apiFetch";

export type AgentRunSummaryStatus =
  | "running"
  | "completed"
  | "failed"
  | "stopped";

export interface AgentRunSummaryItem {
  session_key: string;
  status: AgentRunSummaryStatus;
  title: string;
  runner_id: string;
  duration_secs: number;
  user_message_count: number;
  source: string;
  start_time: number;
}

export interface AgentRunSummaryResponse {
  items: AgentRunSummaryItem[];
  summary: {
    running_count: number;
    total_count: number;
    active_runners: Array<{ runner_id: string; count: number }>;
  };
  next_cursor: string | null;
  updated_at: number;
}

export interface AgentRunSummaryQuery {
  q?: string;
  status?: string;
  runner?: string;
  source?: string;
  cursor?: string;
  limit?: number;
}

export async function getAgentRunSummaries(
  query: AgentRunSummaryQuery = {},
): Promise<AgentRunSummaryResponse> {
  const params = new URLSearchParams();
  Object.entries(query).forEach(([key, value]) => {
    if (value !== undefined && value !== "") {
      params.set(key, String(value));
    }
  });
  const suffix = params.size ? `?${params.toString()}` : "";
  const response = await apiFetch(
    `/api/im-gateway/agent/session-summaries${suffix}`,
  );
  if (!response.ok) {
    throw new Error(`Failed to load agent run summaries (${response.status})`);
  }
  return response.json() as Promise<AgentRunSummaryResponse>;
}
