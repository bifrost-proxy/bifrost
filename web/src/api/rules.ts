import { get, post, put, del } from './client';
import type { RuleFile, RuleFileDetail, ApiResponse, RuleSyntaxReport } from '../types';

export interface ActiveRuleItem {
  name: string;
  rule_count: number;
  group_id: string | null;
  group_name: string | null;
}

export interface VariableDefinition {
  rule_name: string;
  group_id: string | null;
  value_preview: string;
}

export interface VariableConflict {
  variable_name: string;
  definitions: VariableDefinition[];
}

export interface ActiveSummaryResponse {
  total: number;
  rules: ActiveRuleItem[];
  variable_conflicts: VariableConflict[];
  merged_content: string;
}

export interface RuleReferenceCandidate {
  name: string;
  rule_name: string;
  group_name: string | null;
  group_id: string | null;
}

export interface RuleShareLinkResponse {
  url: string;
  query_param: string;
  payload_version: number;
  rule_name: string;
  content_hash: string;
}

export interface RuleMutationResponse extends ApiResponse {
  saved?: boolean;
  syntax?: RuleSyntaxReport;
}

export async function getActiveSummary(): Promise<ActiveSummaryResponse> {
  return get<ActiveSummaryResponse>('/rules/active-summary');
}

export async function getRuleReferenceCandidates(): Promise<RuleReferenceCandidate[]> {
  return get<RuleReferenceCandidate[]>('/rules/reference-candidates');
}

export async function getRules(): Promise<RuleFile[]> {
  return get<RuleFile[]>('/rules');
}

export async function getRule(name: string): Promise<RuleFileDetail> {
  return get<RuleFileDetail>(`/rules/${encodeURIComponent(name)}`);
}

export async function createRule(name: string, content: string, enabled = true): Promise<RuleMutationResponse> {
  return post<RuleMutationResponse>('/rules', { name, content, enabled });
}

export async function updateRule(name: string, content?: string, enabled?: boolean): Promise<RuleMutationResponse> {
  return put<RuleMutationResponse>(`/rules/${encodeURIComponent(name)}`, { content, enabled });
}

export async function reorderRules(order: string[]): Promise<ApiResponse> {
  return put<ApiResponse>('/rules/reorder', { order });
}

export async function deleteRule(name: string): Promise<ApiResponse> {
  return del<ApiResponse>(`/rules/${encodeURIComponent(name)}`);
}

export async function enableRule(name: string): Promise<ApiResponse> {
  return put<ApiResponse>(`/rules/${encodeURIComponent(name)}/enable`);
}

export async function disableRule(name: string): Promise<ApiResponse> {
  return put<ApiResponse>(`/rules/${encodeURIComponent(name)}/disable`);
}

export async function renameRule(oldName: string, newName: string): Promise<ApiResponse> {
  return put<ApiResponse>(`/rules/${encodeURIComponent(oldName)}/rename`, { new_name: newName });
}

export async function createRuleShareLink(
  name: string,
  targetUrl: string,
): Promise<RuleShareLinkResponse> {
  return post<RuleShareLinkResponse>('/rules/share-link', {
    name,
    target_url: targetUrl,
    exclusive_scope: 'my_rules',
  });
}
