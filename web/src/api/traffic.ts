import { get, del, post } from './client';
import type { TrafficListResponse, TrafficRecord, TrafficFilter, TrafficUpdatesFilter, TrafficUpdatesResponseCompact, ApiResponse, TrafficQueryRequest, TrafficQueryResponse } from '../types';
import { buildApiUrl } from '../runtime';

export async function queryTraffic(request: TrafficQueryRequest): Promise<TrafficQueryResponse> {
  return post<TrafficQueryResponse>('/traffic/query', request);
}

export async function getTrafficPage(filter?: TrafficQueryRequest): Promise<TrafficQueryResponse> {
  const params = new URLSearchParams();
  if (filter) {
    Object.entries(filter).forEach(([key, value]) => {
      if (value !== undefined && value !== null) {
        params.append(key, String(value));
      }
    });
  }
  const query = params.toString();
  return get<TrafficQueryResponse>(`/traffic${query ? `?${query}` : ''}`);
}

export async function getTrafficList(filter?: TrafficFilter): Promise<TrafficListResponse> {
  const params = new URLSearchParams();
  if (filter) {
    Object.entries(filter).forEach(([key, value]) => {
      if (value !== undefined && value !== null) {
        params.append(key, String(value));
      }
    });
  }
  const query = params.toString();
  return get<TrafficListResponse>(`/traffic${query ? `?${query}` : ''}`);
}

export async function getTrafficUpdates(filter?: TrafficUpdatesFilter): Promise<TrafficUpdatesResponseCompact> {
  const params = new URLSearchParams();
  if (filter) {
    Object.entries(filter).forEach(([key, value]) => {
      if (value !== undefined && value !== null) {
        params.append(key, String(value));
      }
    });
  }
  const query = params.toString();
  return get<TrafficUpdatesResponseCompact>(`/traffic/updates${query ? `?${query}` : ''}`);
}

export async function getTrafficDetail(id: string): Promise<TrafficRecord> {
  return get<TrafficRecord>(`/traffic/${encodeURIComponent(id)}`);
}

export async function clearTraffic(ids?: string[]): Promise<ApiResponse> {
  if (ids && ids.length > 0) {
    return del<ApiResponse>('/traffic', { ids });
  }
  return del<ApiResponse>('/traffic');
}

export async function getRequestBody(id: string): Promise<string | null> {
  const response = await get<ApiResponse<string>>(`/traffic/${encodeURIComponent(id)}/request-body`);
  return response.data || null;
}

export async function getResponseBody(id: string): Promise<string | null> {
  const response = await get<ApiResponse<string>>(`/traffic/${encodeURIComponent(id)}/response-body`);
  return response.data || null;
}

export interface TrafficBodyContent {
  data: string | null;
  data_base64?: string | null;
  encoding?: 'text' | 'base64';
  size?: number;
}

interface BodyContentOptions {
  raw?: boolean;
  encoding?: 'text' | 'base64';
}

const buildBodyQuery = (options?: BodyContentOptions): string => {
  const params = new URLSearchParams();
  if (options?.raw) {
    params.set('raw', '1');
  }
  if (options?.encoding) {
    params.set('encoding', options.encoding);
  }
  const query = params.toString();
  return query ? `?${query}` : '';
};

export async function getRequestBodyContent(
  id: string,
  options?: BodyContentOptions,
): Promise<TrafficBodyContent | null> {
  const response = await get<ApiResponse<string> & TrafficBodyContent>(
    `/traffic/${encodeURIComponent(id)}/request-body${buildBodyQuery(options)}`
  );
  return {
    data: response.data ?? null,
    data_base64: response.data_base64 ?? null,
    encoding: response.encoding,
    size: response.size,
  };
}

export async function getResponseBodyContent(
  id: string,
  options?: BodyContentOptions,
): Promise<TrafficBodyContent | null> {
  const response = await get<ApiResponse<string> & TrafficBodyContent>(
    `/traffic/${encodeURIComponent(id)}/response-body${buildBodyQuery(options)}`
  );
  return {
    data: response.data ?? null,
    data_base64: response.data_base64 ?? null,
    encoding: response.encoding,
    size: response.size,
  };
}

export function getResponseBodyContentUrl(id: string, raw = true): string {
  const params = new URLSearchParams();
  if (raw) {
    params.set('raw', '1');
  }
  const suffix = params.toString();
  return buildApiUrl(
    `/traffic/${encodeURIComponent(id)}/response-body/content${suffix ? `?${suffix}` : ''}`
  );
}
