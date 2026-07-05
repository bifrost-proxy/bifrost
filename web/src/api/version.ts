import { get, post } from './client';
import type { UpgradeProgress, VersionCheckResponse } from '../types';

export type UpgradeChannel = 'cli' | 'desktop';

function versionQuery(forceRefresh: boolean, channel: UpgradeChannel): string {
  const params = new URLSearchParams();
  if (forceRefresh) {
    params.set('refresh', 'true');
  }
  if (channel === 'desktop') {
    params.set('channel', 'desktop');
  }
  const query = params.toString();
  return query ? `?${query}` : '';
}

export async function checkVersion(
  forceRefresh = false,
  channel: UpgradeChannel = 'cli',
): Promise<VersionCheckResponse> {
  const query = versionQuery(forceRefresh, channel);
  return get<VersionCheckResponse>(`/system/version-check${query}`);
}

export async function startUpgrade(channel: UpgradeChannel = 'cli'): Promise<UpgradeProgress> {
  const query = channel === 'desktop' ? '?channel=desktop' : '';
  return post<UpgradeProgress>(`/system/upgrade${query}`);
}

export async function getUpgradeProgress(): Promise<UpgradeProgress> {
  return get<UpgradeProgress>('/system/upgrade/progress');
}
