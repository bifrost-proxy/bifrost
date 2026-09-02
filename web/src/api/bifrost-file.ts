import { getClientId } from '../services/clientId';
import { buildApiUrl } from '../runtime';
import { apiFetch } from './apiFetch';
import type { TrafficRecord } from '../types';

interface AxiosLikeError {
  isAxiosError: true;
  message?: string;
  response?: { data?: unknown };
}

function isAxiosLikeError(error: unknown): error is AxiosLikeError {
  return (
    typeof error === 'object' &&
    error !== null &&
    (error as { isAxiosError?: unknown }).isAxiosError === true
  );
}

export type BifrostFileType = 'rules' | 'network' | 'script' | 'values' | 'template';

export interface DetectResponse {
  file_type: BifrostFileType;
  meta: Record<string, unknown>;
}

export interface PreviewResponse {
  file_type: BifrostFileType;
  meta: Record<string, unknown> | null;
  rules?: RulesPreview;
  network?: NetworkPreview;
  item_count?: number;
}

export interface RulesPreview {
  name: string;
  enabled: boolean;
  description?: string | null;
  line_count: number;
  content: string;
}

export interface NetworkPreview {
  record_count: number;
  hosts: string[];
  records: NetworkPreviewRecord[];
  single_record?: NetworkPreviewDetail | null;
  warnings?: string[];
}

export interface NetworkPreviewRecord {
  id: string;
  method: string;
  url: string;
  status: number;
  host: string;
  path: string;
  protocol: string;
  client_app?: string | null;
  duration_ms: number;
  request_size: number;
  response_size: number;
}

export interface NetworkPreviewDetail {
  record: TrafficRecord;
  request_body?: string | null;
  response_body?: string | null;
}

export interface ImportResponse {
  success: boolean;
  file_type: BifrostFileType;
  data: ImportedData;
  warnings?: string[];
}

export interface ImportedData {
  rule_names?: string[];
  rule_count?: number;
  record_count?: number;
  script_names?: string[];
  script_count?: number;
  value_names?: string[];
  value_count?: number;
  group_count?: number;
  request_count?: number;
}

export function getImportedItemCount(result: ImportResponse): number | undefined {
  switch (result.file_type) {
    case 'rules':
      return result.data.rule_count;
    case 'network':
      return result.data.record_count;
    case 'script':
      return result.data.script_count;
    case 'values':
      return result.data.value_count;
    case 'template':
      return (result.data.group_count ?? 0) + (result.data.request_count ?? 0);
    default:
      return undefined;
  }
}

export function formatImportSuccessMessage(result: ImportResponse, filename?: string): string {
  const count = getImportedItemCount(result);
  const target = filename ? `${filename} ` : '';

  if (count === undefined) {
    return `Imported ${target}successfully`;
  }

  if (count === 0) {
    return `${target || 'File '}contains no ${result.file_type} items to import`;
  }

  const item = count === 1 ? 'item' : 'items';
  return `Imported ${target}successfully (${count} ${item})`;
}

export function formatBifrostFileError(error: unknown): string {
  if (isAxiosLikeError(error)) {
    const data = error.response?.data;
    if (data && typeof data === 'object' && 'error' in data) {
      const message = (data as { error?: unknown }).error;
      if (typeof message === 'string' && message.trim()) {
        return message;
      }
    }
    if (error.message) {
      return error.message;
    }
  }

  if (error instanceof Error) {
    return error.message;
  }

  return String(error);
}

export interface ExportRulesRequest {
  rule_names: string[];
  description?: string;
}

export interface ExportNetworkRequest {
  record_ids: string[];
  include_body?: boolean;
  description?: string;
}

export interface ExportScriptRequest {
  script_names: string[];
  description?: string;
}

export interface ExportValuesRequest {
  value_names?: string[];
  description?: string;
}

export interface ExportTemplateRequest {
  group_ids?: string[];
  request_ids?: string[];
  description?: string;
}

export type ExportRequest =
  | ExportRulesRequest
  | ExportNetworkRequest
  | ExportScriptRequest
  | ExportValuesRequest
  | ExportTemplateRequest;

export function getExportItemCount(
  fileType: BifrostFileType,
  request: ExportRequest,
): number | undefined {
  switch (fileType) {
    case 'rules':
      return (request as ExportRulesRequest).rule_names.length;
    case 'network':
      return (request as ExportNetworkRequest).record_ids.length;
    case 'script':
      return (request as ExportScriptRequest).script_names.length;
    case 'values':
      return (request as ExportValuesRequest).value_names?.length;
    case 'template': {
      const templateReq = request as ExportTemplateRequest;
      return (templateReq.request_ids?.length || 0) + (templateReq.group_ids?.length || 0);
    }
    default:
      return undefined;
  }
}

export function getEmptyExportMessage(
  fileType: BifrostFileType,
  request: ExportRequest,
): string | undefined {
  const count = getExportItemCount(fileType, request);
  if (count !== 0) {
    return undefined;
  }

  switch (fileType) {
    case 'network':
      return 'Select at least one Network record before exporting a .bifrost file';
    case 'rules':
      return 'Select at least one rule before exporting a .bifrost file';
    case 'script':
      return 'Select at least one script before exporting a .bifrost file';
    case 'template':
      return 'Select at least one replay item before exporting a .bifrost file';
    default:
      return undefined;
  }
}

export async function detectType(content: string): Promise<DetectResponse> {
  return postBifrostFileJson<DetectResponse>('/detect', content, 'text/plain');
}

export async function previewFile(content: string): Promise<PreviewResponse> {
  return postBifrostFileJson<PreviewResponse>('/preview', content, 'text/plain');
}

export async function importFile(content: string): Promise<ImportResponse> {
  return postBifrostFileJson<ImportResponse>('/import', content, 'text/plain');
}

export async function exportRules(request: ExportRulesRequest): Promise<string> {
  return postBifrostFileText('/export/rules', request);
}

export async function exportNetwork(request: ExportNetworkRequest): Promise<string> {
  return postBifrostFileText('/export/network', request);
}

export async function exportScripts(request: ExportScriptRequest): Promise<string> {
  return postBifrostFileText('/export/scripts', request);
}

export async function exportValues(request: ExportValuesRequest): Promise<string> {
  return postBifrostFileText('/export/values', request);
}

export async function exportTemplates(request: ExportTemplateRequest): Promise<string> {
  return postBifrostFileText('/export/templates', request);
}

async function postBifrostFileJson<T>(
  path: string,
  body: string | unknown,
  contentType = 'application/json',
): Promise<T> {
  const response = await postBifrostFile(path, body, contentType);
  return (await response.json()) as T;
}

async function postBifrostFileText(path: string, body: unknown): Promise<string> {
  const response = await postBifrostFile(path, body, 'application/json');
  return response.text();
}

async function postBifrostFile(
  path: string,
  body: string | unknown,
  contentType: string,
): Promise<Response> {
  const response = await apiFetch(`${buildApiUrl('/bifrost-file')}${path}`, {
    method: 'POST',
    headers: {
      'Content-Type': contentType,
      'X-Client-Id': getClientId(),
    },
    body: typeof body === 'string' ? body : JSON.stringify(body),
  });

  if (!response.ok) {
    throw new Error(await readBifrostFileError(response));
  }

  return response;
}

async function readBifrostFileError(response: Response): Promise<string> {
  const fallback = `Request failed with status ${response.status}`;
  const contentType = response.headers.get('Content-Type') || '';
  try {
    if (contentType.includes('application/json')) {
      const payload = (await response.json()) as { error?: unknown; message?: unknown };
      const message = payload.error || payload.message;
      if (typeof message === 'string' && message.trim()) {
        return message;
      }
      return fallback;
    }
    const text = await response.text();
    return text.trim() || fallback;
  } catch {
    return fallback;
  }
}

export function downloadFile(content: string, filename: string): void {
  const blob = new Blob([content], { type: 'text/plain;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

export function formatExportFilename(type: BifrostFileType, count?: number): string {
  const date = new Date().toISOString().slice(0, 19).replace(/[:-]/g, '');
  const suffix = count && count > 1 ? `-${count}` : '';
  return `bifrost-${type}${suffix}-${date}.bifrost`;
}
