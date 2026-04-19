import type { IStorage } from '../dao/types';
import type { RemoteInvokeConfig } from '../types';

let cleanupTimer: ReturnType<typeof setInterval> | null = null;

export function startCleanupScheduler(storage: IStorage, config: RemoteInvokeConfig) {
  if (cleanupTimer) return;

  const intervalMs = 60 * 60 * 1000;
  cleanupTimer = setInterval(async () => {
    try {
      const now = new Date().toISOString();
      const deleted = await storage.remoteInvoke.cleanupExpiredData(
        now,
        config.retention_days,
        config.max_records,
      );
      if (deleted > 0) {
        console.log(`[remote-invoke] cleanup: removed ${deleted} expired records`);
      }
    } catch (e) {
      console.error('[remote-invoke] cleanup error:', e);
    }
  }, intervalMs);

  setTimeout(async () => {
    try {
      const now = new Date().toISOString();
      await storage.remoteInvoke.cleanupExpiredData(
        now,
        config.retention_days,
        config.max_records,
      );
    } catch (e) {
      console.error('[remote-invoke] initial cleanup error:', e);
    }
  }, 5000);
}

export function stopCleanupScheduler() {
  if (cleanupTimer) {
    clearInterval(cleanupTimer);
    cleanupTimer = null;
  }
}
