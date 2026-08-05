import { get } from './client';
import type {
  MetricsSnapshot,
  SystemOverview,
  AppMetricsResponse,
  HostMetricsResponse,
} from '../types';

export async function getMetrics(): Promise<MetricsSnapshot> {
  return get<MetricsSnapshot>('/metrics');
}

export async function getMetricsHistory(limit?: number): Promise<MetricsSnapshot[]> {
  const query = limit ? `?limit=${limit}` : '';
  return get<MetricsSnapshot[]>(`/metrics/history${query}`);
}

export async function getSystemOverview(): Promise<SystemOverview> {
  return get<SystemOverview>('/system/overview');
}

export async function getAppMetrics(): Promise<AppMetricsResponse> {
  return get<AppMetricsResponse>('/metrics/apps?include_summary=true');
}

export async function getHostMetrics(): Promise<HostMetricsResponse> {
  return get<HostMetricsResponse>('/metrics/hosts?include_summary=true');
}
