import { post } from './client';

export type DiagnosticSeverity = 'Info' | 'Warning' | 'Error';
export type SupportLevel =
  | 'FullySupported'
  | 'TranslatedWithBehaviorNote'
  | 'NeedsManualReview'
  | 'NotSupportedYet';

export interface SourceLine {
  line: number;
  column: number;
  raw: string;
  content: string;
  comment?: string | null;
}

export interface ProfileDiagnostic {
  severity: DiagnosticSeverity;
  line: number;
  column: number;
  code: string;
  message: string;
  suggestion?: string | null;
}

export interface ProfileEntry {
  [key: string]: unknown;
}

export interface ProfileSection {
  name: string;
  kind: string;
  line: number;
  entries: ProfileEntry[];
}

export interface CompatibilitySummary {
  fully_supported: number;
  translated_with_behavior_note: number;
  needs_manual_review: number;
  not_supported_yet: number;
}

export interface CompatibilityItem {
  level: SupportLevel;
  section: string;
  line: number;
  capability: string;
  message: string;
  suggestion?: string | null;
}

export interface CompatibilityReport {
  summary: CompatibilitySummary;
  items: CompatibilityItem[];
  diagnostics: ProfileDiagnostic[];
}

export interface ProfileResource {
  kind: string;
  reference: string;
  source_line: number;
  status: string;
  cache_key?: string | null;
  etag?: string | null;
  last_modified?: string | null;
  loaded_from_cache: boolean;
  item_count: number;
  diagnostics: ProfileDiagnostic[];
}

export interface RuntimeRule {
  source: SourceLine;
  rule_type: string;
  value?: string | null;
  policy: string;
  parameters: string[];
  origin: string;
}

export interface RuntimePolicyGroup {
  source: SourceLine;
  name: string;
  group_type: string;
  policies: string[];
  parameters: Record<string, string>;
  missing_members: string[];
}

export interface RuntimeKeyValue {
  section: string;
  key: string;
  value: string;
  source: SourceLine;
}

export interface ProfileRuntimePlan {
  mode: string;
  proxies: Array<{
    source: SourceLine;
    name: string;
    protocol: string;
    fields: string[];
  }>;
  rules: RuntimeRule[];
  policy_groups: RuntimePolicyGroup[];
  dns: RuntimeKeyValue[];
  mitm: RuntimeKeyValue[];
  http_pipeline: RuntimeKeyValue[];
  diagnostics: ProfileDiagnostic[];
}

export interface ExplainStep {
  stage: string;
  line?: number | null;
  message: string;
}

export interface ExplainReport {
  matched_rule?: {
    source: SourceLine;
    rule_type: string;
    value?: string | null;
    policy: string;
    parameters: string[];
  } | null;
  target_policy?: string | null;
  dns_decision: {
    matched_host_mapping?: string | null;
    notes: string[];
  };
  policy_decision?: {
    requested_policy: string;
    terminal_policy: string;
    chain: string[];
    reason: string;
  } | null;
  mitm_decision: {
    included: boolean;
    excluded: boolean;
    matched_patterns: string[];
    reason: string;
  };
  http_pipeline: Array<{
    section: string;
    line: number;
    matched: boolean;
    action: string;
    reason: string;
  }>;
  timeline: ExplainStep[];
  diagnostics: ProfileDiagnostic[];
}

export interface ConversionPreview {
  format: string;
  content: string;
  report: CompatibilityReport;
}

export interface SurgeImportResponse {
  source_label: string;
  sections: ProfileSection[];
  diagnostics: ProfileDiagnostic[];
  compatibility: CompatibilityReport;
  resources: ProfileResource[];
  runtime_plan: ProfileRuntimePlan;
  conversion_preview: ConversionPreview;
  explain?: ExplainReport | null;
}

export interface SurgeExplainResponse {
  source_label: string;
  report: ExplainReport;
  resources: ProfileResource[];
  diagnostics: ProfileDiagnostic[];
}

export function importSurgeProfile(request: {
  content: string;
  source_label?: string;
  explain_url?: string;
}): Promise<SurgeImportResponse> {
  return post<SurgeImportResponse>('/profile/surge/import', request);
}

export function explainSurgeProfile(request: {
  content: string;
  url: string;
  source_label?: string;
}): Promise<SurgeExplainResponse> {
  return post<SurgeExplainResponse>('/profile/surge/explain', request);
}
