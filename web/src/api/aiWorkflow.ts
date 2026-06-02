import { get, post } from './client';

export type WorkflowSummary = {
  id: string;
  name: string;
  revision: number;
  nodeCount: number;
  edgeCount: number;
  updatedAt: string;
};

export type WorkflowDiagnostic = {
  severity: 'error' | 'warning';
  code: string;
  path: string;
  message: string;
  suggestedFix?: string;
};

export type WorkflowValidationReport = {
  schemaVersion: string;
  valid: boolean;
  errors: WorkflowDiagnostic[];
  warnings: WorkflowDiagnostic[];
  autoFixes: WorkflowDiagnostic[];
  requiresConfirmation: WorkflowDiagnostic[];
};

export type WorkflowPreview = {
  draftHash: string;
  blockingErrors: WorkflowDiagnostic[];
  warnings: WorkflowDiagnostic[];
  markdown: string;
  reactFlow: {
    nodes: Array<Record<string, unknown>>;
    edges: Array<Record<string, unknown>>;
    viewport: Record<string, unknown>;
  };
  effectiveInputs: unknown[];
  permissionRisks: unknown[];
  dryRunRunbook: string[];
};

export type WorkflowDocument = Record<string, unknown> & {
  metadata?: {
    id?: string;
    name?: string;
    revision?: number;
  };
  spec?: {
    nodes?: unknown[];
    edges?: unknown[];
  };
};

export type WorkflowRun = {
  id: string;
  workflowId: string;
  workflowRevision: number;
  status: string;
  createdAt: string;
  finishedAt?: string | null;
  nodeStates: unknown[];
  events: unknown[];
  artifactsDir: string;
};

export type WorkflowTemplate = {
  id: string;
  name: string;
  description: string;
  tags: string[];
  workflow: WorkflowDocument;
  draft: string;
};

export function listAiWorkflows(): Promise<{ workflows: WorkflowSummary[] }> {
  return get('/ai/workflows');
}

export function getAiWorkflow(id: string): Promise<{ workflow: WorkflowDocument }> {
  return get(`/ai/workflows/${encodeURIComponent(id)}`);
}

export function listAiWorkflowTemplates(): Promise<{ templates: WorkflowTemplate[] }> {
  return get('/ai/workflows/templates');
}

export function getAiWorkflowTemplate(id: string): Promise<{ template: WorkflowTemplate }> {
  return get(`/ai/workflows/templates/${encodeURIComponent(id)}`);
}

export function validateAiWorkflow(draft: string): Promise<WorkflowValidationReport> {
  return post('/ai/workflows/validate', { draft });
}

export function previewAiWorkflow(draft: string): Promise<WorkflowPreview> {
  return post('/ai/workflows/preview', { draft });
}

export function applyAiWorkflow(
  workflow: WorkflowDocument,
  options: { dryRun?: boolean; baseRevision?: number } = {},
): Promise<{ workflow?: WorkflowDocument; validation?: WorkflowValidationReport; preview?: WorkflowPreview }> {
  return post('/ai/workflows', {
    workflow,
    dryRun: Boolean(options.dryRun),
    baseRevision: options.baseRevision,
  });
}

export function applyAiWorkflowDraft(
  draft: string,
  options: { dryRun?: boolean; baseRevision?: number } = {},
): Promise<{ workflow?: WorkflowDocument; validation?: WorkflowValidationReport; preview?: WorkflowPreview }> {
  return post('/ai/workflows', {
    draft,
    dryRun: Boolean(options.dryRun),
    baseRevision: options.baseRevision,
  });
}

export function runAiWorkflow(
  workflowId: string,
  inputs: Record<string, unknown> = {},
): Promise<{ run: WorkflowRun }> {
  return post(`/ai/workflows/${encodeURIComponent(workflowId)}/run`, { inputs });
}

export function getAiWorkflowRun(workflowId: string, runId: string): Promise<{ run: WorkflowRun }> {
  return get(`/ai/workflows/${encodeURIComponent(workflowId)}/runs/${encodeURIComponent(runId)}`);
}

export function listAiWorkflowRuns(workflowId: string): Promise<{ runs: WorkflowRun[] }> {
  return get(`/ai/workflows/${encodeURIComponent(workflowId)}/runs`);
}

export function workflowToDraft(workflow: WorkflowDocument): string {
  return JSON.stringify(workflow, null, 2);
}
