import { get, post } from './client';
import type { UpgradeProgress, VersionCheckResponse } from '../types';

export async function checkVersion(forceRefresh = false): Promise<VersionCheckResponse> {
  const query = forceRefresh ? '?refresh=true' : '';
  return get<VersionCheckResponse>(`/system/version-check${query}`);
}

export async function startUpgrade(): Promise<UpgradeProgress> {
  return post<UpgradeProgress>('/system/upgrade');
}

export async function getUpgradeProgress(): Promise<UpgradeProgress> {
  return get<UpgradeProgress>('/system/upgrade/progress');
}
