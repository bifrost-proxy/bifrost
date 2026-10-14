import type { TrafficSummary, TrafficStatistics } from "../types";
import type { TrafficStoreAccess } from "./trafficStoreTypes";
export const POLL_INTERVAL = 1000;
export const POLL_MIN_INTERVAL = 200;
export const HAS_MORE_BURST_LIMIT = 3;
export const HAS_MORE_BACKOFF_INTERVAL = 500;
export const INITIAL_WINDOW_LIMIT = 500;
export const HISTORY_BATCH_LIMIT = 500;
export const UPDATE_BATCH_LIMIT = 1000;
export const UPDATE_THROTTLE_MS = 100;
export const MAX_PENDING_IDS = 500;
export const MEMBERSHIP_QUERY_LIMIT = 500;
export const RECONCILIATION_MAX_READS = 3;
export const RECONCILIATION_TIMEOUT_MS = 5_000;

export interface BatchedUpdate {
  newRecords: TrafficSummary[];
  updatedRecords: TrafficSummary[];
  serverTotal: number;
  serverSequence: number;
  oldestSequence: number | null;
  sourceNewRecordCount: number;
  sourceUpdatedRecordCount: number;
  hasMore: boolean;
}
export interface SelectedClearEvidence {
  requestedIds: Set<string>;
  ids: Set<string>;
  liveRecords: Map<string, TrafficSummary>;
}
export interface LiveReconciliationRead {
  historyGeneration: number;
  resumeGeneration: number;
  invalidated: boolean;
  deletedLiveIds: Set<string>;
  unconfirmedClearIds: Set<string>;
  liveRecords: Map<string, TrafficSummary>;
  liveTotals: Map<string, number>;
  snapshotTotal: number | null;
  emptyObserved: boolean;
  positiveWrites: boolean;
  implicitRemoval: boolean;
  latestSequence: number;
  oldestSequence: number | null;
  metadata: { total: number; sequence: number } | null;
}
export const captureCoordination = {
  pendingBatch: null as BatchedUpdate | null,
  rafId: null as number | null,
  updateTimerId: null as number | null,
  lastUpdateTime: 0,
  hasMoreBurst: 0,
  historyBackfillGeneration: 0,
  statisticsRequest: null as {
    historyGeneration: number;
    resumeGeneration: number;
    databaseEpoch: string | undefined;
    promise: Promise<void>;
  } | null,
  trafficEpochRequest: null as {
    historyGeneration: number;
    resumeGeneration: number;
    statisticsSequence: number;
    latestRecordSequence: number;
    databaseEpoch: string | undefined;
    followup: TrafficStatistics | null;
    promise: Promise<void>;
  } | null,
  membershipRequest: null as Promise<void> | null,
  membershipReconciliationRequired: false,
  membershipFollowupRequired: false,
  latestTrafficSequenceSinceResume: 0,
  trafficResumeGeneration: 0,
  trafficResumeStatisticsPending: false,
  trafficEpochResetSuspected: false,
  liveReconciliationReads: new Set<LiveReconciliationRead>(),
  selectedClearDeletions: null as
    | (SelectedClearEvidence & { generation: number })
    | null,
};
export function createTrafficCaptureCoordinator({
  get,
  set,
}: TrafficStoreAccess) {
  function observeReconciliationMetadata(
    total: number,
    sequence?: number,
    oldestSequence?: number | null,
  ): void {
    for (const read of captureCoordination.liveReconciliationReads) {
      read.latestSequence = Math.max(read.latestSequence, sequence ?? 0);
      // A delayed positive statistics packet must not replace a newer push's
      // totals. Zero still invalidates membership below, even when its cursor is
      // older; the clear signal and the totals watermark are separate facts.
      if (!read.metadata || (sequence ?? 0) >= read.metadata.sequence)
        read.metadata = { total, sequence: sequence ?? 0 };
      if (
        oldestSequence != null &&
        oldestSequence > (read.oldestSequence ?? 0)
      ) {
        read.oldestSequence = oldestSequence;
        read.invalidated = true;
      }
      // Clear keeps the allocation cursor. Re-read membership even if a later
      // insertion made this zero-total statistics packet stale for the totals.
      if (total === 0) {
        read.implicitRemoval = true;
        read.emptyObserved = true;
        if (
          read.snapshotTotal !== null &&
          (read.snapshotTotal > 0 || read.positiveWrites)
        )
          read.invalidated = true;
      }
    }
  }

  function observeTrafficRecordSequence(
    epoch: string | undefined,
    sequence: number,
  ) {
    if (
      captureCoordination.trafficEpochRequest &&
      (!epoch ||
        epoch === captureCoordination.trafficEpochRequest.databaseEpoch)
    ) {
      captureCoordination.trafficEpochRequest.latestRecordSequence = Math.max(
        captureCoordination.trafficEpochRequest.latestRecordSequence,
        sequence,
      );
    }
  }

  function getConsumedTrafficSequence(): number {
    const state = get();
    return Math.max(
      state.lastSequence ?? 0,
      state.records.at(-1)?.sequence ?? 0,
      captureCoordination.pendingBatch?.newRecords.at(-1)?.sequence ?? 0,
      captureCoordination.pendingBatch?.updatedRecords.at(-1)?.sequence ?? 0,
    );
  }

  function acceptTrafficDatabaseEpoch(
    epoch?: string,
    serverSequence?: number,
  ): boolean {
    if (!epoch) return true; // Older backends retain sequence rollback fallback.
    const state = get();
    if (!state.trafficDatabaseEpoch) {
      const previousSequence = Math.max(
        state.serverSequence,
        state.lastSequence ?? 0,
        captureCoordination.pendingBatch?.serverSequence ?? 0,
      );
      if (serverSequence !== undefined && serverSequence < previousSequence) {
        // An older backend may have populated this window without identity. Keep
        // legacy rollback evidence until a fresh read decides its provenance.
        captureCoordination.trafficResumeStatisticsPending = true;
        void state.fetchTrafficStatistics(epoch);
        return false;
      }
      set({ trafficDatabaseEpoch: epoch });
      return true;
    }
    if (state.trafficDatabaseEpoch === epoch) {
      if (
        serverSequence === undefined ||
        serverSequence > getConsumedTrafficSequence()
      )
        return true;
      // Restoring a backup preserves its UUID but can roll back persisted rows.
      // An allocation-only high-water regression is safe while real rows remain.
      captureCoordination.trafficResumeStatisticsPending = true;
      void state.fetchTrafficStatistics(epoch);
      return false;
    }
    // Do not mix rows or retention floors across databases while confirmation is
    // pending. Cursor-free replay retrieves these arrivals after the reset.
    void state.fetchTrafficStatistics(epoch);
    return false;
  }

  function capPendingIds(ids: Set<string>) {
    while (ids.size > MAX_PENDING_IDS) {
      const first = ids.values().next().value as string | undefined;
      if (!first) break;
      ids.delete(first);
    }
  }

  function clearPendingTrafficBatch() {
    captureCoordination.pendingBatch = null;
    if (captureCoordination.rafId !== null) {
      window.cancelAnimationFrame(captureCoordination.rafId);
      captureCoordination.rafId = null;
    }
    if (captureCoordination.updateTimerId !== null) {
      window.clearTimeout(captureCoordination.updateTimerId);
      captureCoordination.updateTimerId = null;
    }
  }

  return {
    observeReconciliationMetadata,
    observeTrafficRecordSequence,
    getConsumedTrafficSequence,
    acceptTrafficDatabaseEpoch,
    capPendingIds,
    clearPendingTrafficBatch,
  };
}
export type TrafficCaptureCoordinator = ReturnType<
  typeof createTrafficCaptureCoordinator
>;
