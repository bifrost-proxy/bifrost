import { get } from "./client";

export type TemporaryPortRuleSetRef =
  | { type: "local_rule"; name: string }
  | { type: "group_rule"; group_id: string; name: string }
  | { type: "rule_file"; path: string }
  | { type: "inline_rule"; content: string };

export interface TemporaryPortBinding {
  port: number;
  host: string;
  name?: string;
  status: "running" | "degraded";
  rule_refs: TemporaryPortRuleSetRef[];
  missing_refs: TemporaryPortRuleSetRef[];
  created_at: number;
  updated_at: number;
}

export interface TemporaryPortRuleItem {
  name: string;
  rule_count: number;
  group_id?: string | null;
  group_name?: string | null;
}

export interface TemporaryPortActiveSummary {
  port: number;
  total: number;
  rules: TemporaryPortRuleItem[];
  merged_content: string;
}

export async function getTemporaryPorts(): Promise<TemporaryPortBinding[]> {
  return get<TemporaryPortBinding[]>("/ports");
}

export async function getTemporaryPortActiveSummary(
  port: number,
): Promise<TemporaryPortActiveSummary> {
  return get<TemporaryPortActiveSummary>(`/ports/${port}/active-summary`);
}
