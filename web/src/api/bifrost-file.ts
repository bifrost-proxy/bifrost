import axios from 'axios';
import { getClientId } from '../services/clientId';
import { buildApiUrl } from '../runtime';

export type BifrostFileType = 'rules' | 'network' | 'script' | 'values' | 'template';

export interface DetectResponse {
  file_type: BifrostFileType;
  meta: Record<string, unknown>;
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
  if (axios.isAxiosError(error)) {
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
  const response = await axios.post<DetectResponse>(`${buildApiUrl('/bifrost-file')}/detect`, content, {
    headers: { 'Content-Type': 'text/plain', 'X-Client-Id': getClientId() },
  });
  return response.data;
}

export async function importFile(content: string): Promise<ImportResponse> {
  const response = await axios.post<ImportResponse>(`${buildApiUrl('/bifrost-file')}/import`, content, {
    headers: { 'Content-Type': 'text/plain', 'X-Client-Id': getClientId() },
  });
  return response.data;
}

export async function exportRules(request: ExportRulesRequest): Promise<string> {
  const response = await axios.post<string>(`${buildApiUrl('/bifrost-file')}/export/rules`, request, {
    responseType: 'text',
    headers: { 'X-Client-Id': getClientId() },
  });
  return response.data;
}

export async function exportNetwork(request: ExportNetworkRequest): Promise<string> {
  const response = await axios.post<string>(`${buildApiUrl('/bifrost-file')}/export/network`, request, {
    responseType: 'text',
    headers: { 'X-Client-Id': getClientId() },
  });
  return response.data;
}

export async function exportScripts(request: ExportScriptRequest): Promise<string> {
  const response = await axios.post<string>(`${buildApiUrl('/bifrost-file')}/export/scripts`, request, {
    responseType: 'text',
    headers: { 'X-Client-Id': getClientId() },
  });
  return response.data;
}

export async function exportValues(request: ExportValuesRequest): Promise<string> {
  const response = await axios.post<string>(`${buildApiUrl('/bifrost-file')}/export/values`, request, {
    responseType: 'text',
    headers: { 'X-Client-Id': getClientId() },
  });
  return response.data;
}

export async function exportTemplates(request: ExportTemplateRequest): Promise<string> {
  const response = await axios.post<string>(`${buildApiUrl('/bifrost-file')}/export/templates`, request, {
    responseType: 'text',
    headers: { 'X-Client-Id': getClientId() },
  });
  return response.data;
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
