import type { TrafficSummary, TrafficUpdatesFilter } from "../types";
import {
  buildTrafficRecordsMap,
  mergeBoundedTrafficWindow,
} from "./trafficWindow";
import * as api from "../api";
import { getApiErrorPayload } from "../api/client";
import pushService from "../services/pushService";
import {
  captureCoordination,
  MEMBERSHIP_QUERY_LIMIT,
  RECONCILIATION_MAX_READS,
  RECONCILIATION_TIMEOUT_MS,
  INITIAL_WINDOW_LIMIT,
} from "./trafficCaptureCoordinator";
import type {
  ClearReconciliationEvidence,
  LiveReconciliationRead,
  TrafficCaptureCoordinator,
} from "./trafficCaptureCoordinator";
import type { TrafficStoreAccess } from "./trafficStoreTypes";
import {
  isPendingRecord,
  mergeIncrementalRecord,
  preprocessTrafficRecord,
  preprocessTrafficRecords,
  mergeDetailWithSummary,
  findLastRecordById,
  getBoundaryState,
  createRecordsMutation,
  compactTrafficSummaryToTrafficSummary,
} from "./trafficRecordModel";
import { t } from "../i18n";

function withinReconciliationDeadline<T>(
  request: Promise<T>,
  deadline: number,
): Promise<T> {
  let timer: number | undefined;
  const expired = new Promise<never>((_, reject) => {
    timer = window.setTimeout(
      () => reject(new Error("Traffic reconciliation deadline exceeded")),
      Math.max(0, deadline - Date.now()),
    );
  });
  return Promise.race([request, expired]).finally(() =>
    window.clearTimeout(timer),
  );
}

export function createTrafficReconciliation(
  { get, set }: TrafficStoreAccess,
  coordination: TrafficCaptureCoordinator,
) {
  const {
    acceptTrafficDatabaseEpoch,
    clearPendingTrafficBatch,
    capPendingIds,
    observeTrafficRecordSequence,
  } = coordination;
  return {
    reloadRecords: async (
      preserveLiveRecords = false,
      clearEvidence?: ClearReconciliationEvidence,
    ) => {
      const generation = captureCoordination.historyBackfillGeneration;
      const resumeGeneration = captureCoordination.trafficResumeGeneration;
      const stateAtReadStart = get();
      const read: LiveReconciliationRead | null = preserveLiveRecords
        ? {
            historyGeneration: generation,
            resumeGeneration,
            invalidated: false,
            deletedLiveIds: new Set(clearEvidence?.ids),
            unconfirmedClearIds: new Set(
              Array.from(clearEvidence?.requestedIds ?? []).filter(
                (id) => !clearEvidence?.ids.has(id),
              ),
            ),
            liveRecords: new Map([
              ...get().recordsMap,
              ...(clearEvidence?.liveRecords ?? []),
            ]),
            liveTotals: new Map(
              [
                ...get().records,
                ...(clearEvidence?.liveRecords.values() ?? []),
              ].map((record) => [record.id, get().serverTotal]),
            ),
            snapshotTotal: null,
            emptyObserved: false,
            positiveWrites: false,
            implicitRemoval: clearEvidence?.reconcileMembership ?? false,
            latestSequence: Math.max(
              get().serverSequence,
              captureCoordination.pendingBatch?.serverSequence ?? 0,
            ),
            oldestSequence:
              captureCoordination.pendingBatch?.oldestSequence == null
                ? get().serverOldestSequence
                : Math.max(
                    get().serverOldestSequence ?? 0,
                    captureCoordination.pendingBatch.oldestSequence,
                  ),
            metadata: null,
          }
        : null;
      if (read) {
        // Include writes queued before this read starts. Every accepted batch
        // is then represented in the journal before the final GET consumes it.
        if (captureCoordination.pendingBatch) {
          for (const record of captureCoordination.pendingBatch
            .updatedRecords) {
            read.liveRecords.set(
              record.id,
              mergeIncrementalRecord(read.liveRecords.get(record.id), record),
            );
            read.liveTotals.set(
              record.id,
              captureCoordination.pendingBatch.serverTotal,
            );
          }
          for (const record of captureCoordination.pendingBatch.newRecords) {
            if (!read.liveRecords.has(record.id))
              read.liveRecords.set(record.id, record);
            read.liveTotals.set(
              record.id,
              captureCoordination.pendingBatch.serverTotal,
            );
          }
        }
        read.liveRecords = buildTrafficRecordsMap(
          mergeBoundedTrafficWindow(
            Array.from(read.liveRecords.values()),
            [],
            "newer",
          ).records,
        );
        for (const id of read.liveTotals.keys())
          if (!read.liveRecords.has(id)) read.liveTotals.delete(id);
        captureCoordination.liveReconciliationReads.add(read);
      }
      let committed = false;
      let queriedLiveIds = new Set<string>();
      let confirmedLiveIds = new Set<string>();
      let completedMembershipIds = new Set<string>();
      let membershipLiveAtStart = new Map<string, TrafficSummary>();
      const commit = (
        snapshot?: Awaited<ReturnType<typeof api.getTrafficUpdates>>,
      ) => {
        const state = get();
        const response = snapshot ?? {
          new_records: [],
          updated_records: [],
          has_more: state.hasMore,
          server_total:
            read?.metadata && read.metadata.sequence >= state.serverSequence
              ? read.metadata.total
              : state.serverTotal,
          server_sequence: Math.max(
            state.serverSequence,
            read?.latestSequence ?? 0,
          ),
        };
        if (snapshot)
          observeTrafficRecordSequence(
            response.database_epoch,
            Math.max(
              response.new_records.reduce(
                (sequence, record) => Math.max(sequence, record.seq),
                0,
              ),
              response.updated_records.reduce(
                (sequence, record) => Math.max(sequence, record.seq),
                0,
              ),
            ),
          );
        if (
          snapshot &&
          !acceptTrafficDatabaseEpoch(
            response.database_epoch,
            response.server_sequence,
          )
        )
          return false;
        const convertedRecords = response.new_records.map(
          compactTrafficSummaryToTrafficSummary,
        );
        const snapshotRecords = preprocessTrafficRecords(
          convertedRecords,
        ).filter((record) => !read?.deletedLiveIds.has(record.id));
        const snapshotIds = new Set(snapshotRecords.map((record) => record.id));
        const canKeepLive = (record: TrafficSummary) =>
          !read ||
          (!read.deletedLiveIds.has(record.id) &&
            (!snapshot ||
              !read.unconfirmedClearIds.has(record.id) ||
              snapshotIds.has(record.id) ||
              confirmedLiveIds.has(record.id)) &&
            !(
              read.implicitRemoval &&
              !snapshotIds.has(record.id) &&
              read.liveTotals.get(record.id) === 0
            ) &&
            (!completedMembershipIds.has(record.id) ||
              confirmedLiveIds.has(record.id) ||
              (read.liveRecords.has(record.id) &&
                read.liveRecords.get(record.id) !==
                  membershipLiveAtStart.get(record.id))));
        // Keep surviving live fields, including writes received after the ID
        // query's snapshot. Explicit deletions/floors still invalidate reads.
        let preprocessedRecords = preserveLiveRecords
          ? mergeBoundedTrafficWindow(
              snapshotRecords,
              get().records.filter(
                (record) =>
                  resumeGeneration !==
                    captureCoordination.trafficResumeGeneration ||
                  canKeepLive(record),
              ),
              "newer",
            ).records
          : snapshotRecords;
        if (read)
          preprocessedRecords = mergeBoundedTrafficWindow(
            preprocessedRecords,
            Array.from(read.liveRecords.values()).filter(canKeepLive),
            "newer",
          ).records;
        const oldestSequenceFloor = read
          ? read.oldestSequence === null && state.serverOldestSequence === null
            ? null
            : Math.max(
                read.oldestSequence ?? 0,
                state.serverOldestSequence ?? 0,
              )
          : undefined;
        const retainedRecords =
          oldestSequenceFloor != null
            ? preprocessedRecords.filter(
                (record) => record.sequence >= oldestSequenceFloor,
              )
            : preprocessedRecords;

        const newPendingIds = new Set<string>();
        const newRecordsMap = new Map<string, TrafficSummary>();
        for (const r of retainedRecords) {
          newRecordsMap.set(r.id, r);
          if (isPendingRecord(r)) {
            newPendingIds.add(r.id);
          }
        }
        capPendingIds(newPendingIds);

        const currentRecord = get().currentRecord;
        const pendingDetailUpdate =
          read && currentRecord && captureCoordination.pendingBatch
            ? findLastRecordById(
                captureCoordination.pendingBatch.updatedRecords,
                currentRecord.id,
              )
            : undefined;
        const detailSummary =
          read && currentRecord
            ? (newRecordsMap.get(currentRecord.id) ?? pendingDetailUpdate)
            : undefined;
        const removedByReconciliation = (id: string, sequence?: number) => {
          if (read)
            return (
              read.deletedLiveIds.has(id) ||
              (oldestSequenceFloor != null &&
                sequence !== undefined &&
                sequence < oldestSequenceFloor) ||
              (read.liveRecords.has(id) &&
                !canKeepLive(read.liveRecords.get(id)!))
            );
          // A complete replacement snapshot proves absence only for the same
          // selected state we read. Bounded windows and later accepted writes
          // cannot establish deletion of an independently loaded detail.
          const pending = captureCoordination.pendingBatch;
          const queuedWrite =
            pending &&
            (findLastRecordById(pending.newRecords, id) ||
              findLastRecordById(pending.updatedRecords, id));
          return (
            !!snapshot &&
            !response.has_more &&
            response.server_total === snapshotRecords.length &&
            response.updated_records.every((record) =>
              snapshotIds.has(record.id),
            ) &&
            !snapshotIds.has(id) &&
            !queuedWrite &&
            (stateAtReadStart.selectedId === id ||
              stateAtReadStart.recordsMap.has(id) ||
              stateAtReadStart.currentRecord?.id === id) &&
            stateAtReadStart.selectedId === state.selectedId &&
            stateAtReadStart.recordsMap.get(id) === state.recordsMap.get(id)
          );
        };
        const selectionRemoved =
          !!state.selectedId &&
          removedByReconciliation(
            state.selectedId,
            state.recordsMap.get(state.selectedId)?.sequence ??
              (currentRecord?.id === state.selectedId
                ? currentRecord.sequence
                : undefined),
          );
        const detailRemoved =
          selectionRemoved ||
          (!!currentRecord &&
            removedByReconciliation(currentRecord.id, currentRecord.sequence));
        const reconciledDetail = detailRemoved
          ? null
          : currentRecord &&
              detailSummary &&
              canKeepLive(detailSummary) &&
              (oldestSequenceFloor == null ||
                detailSummary.sequence >= oldestSequenceFloor)
            ? mergeDetailWithSummary(currentRecord, detailSummary)
            : currentRecord;

        const detailState =
          read || detailRemoved
            ? {
                currentRecord: reconciledDetail,
                ...(detailRemoved
                  ? {
                      selectedId: selectionRemoved
                        ? undefined
                        : state.selectedId,
                      requestBody: null,
                      responseBody: null,
                      requestRawBody: null,
                      responseRawBody: null,
                      detailLoading:
                        selectionRemoved || !state.selectedId
                          ? false
                          : state.detailLoading,
                      detailError:
                        selectionRemoved || !state.selectedId
                          ? t("shared.traffic.requestDeleted")
                          : state.detailError,
                    }
                  : {}),
              }
            : {};

        // All surviving writes have been merged from the journal. Consume
        // their batch too: a later animation-frame flush must not restore
        // totals from before this GET (including an equal allocation cursor).
        if (
          read &&
          resumeGeneration === captureCoordination.trafficResumeGeneration
        )
          clearPendingTrafficBatch();
        const boundaries = getBoundaryState(retainedRecords);
        const serverTotal =
          preserveLiveRecords &&
          read?.metadata &&
          read.metadata.sequence > response.server_sequence
            ? read.metadata.total
            : response.server_total;
        const serverSequence = preserveLiveRecords
          ? Math.max(
              response.server_sequence,
              state.serverSequence,
              read?.latestSequence ?? 0,
            )
          : response.server_sequence;
        const inserted = retainedRecords.filter(
          (record) => !state.recordsMap.has(record.id),
        );
        const updated = retainedRecords.filter(
          (record) =>
            state.recordsMap.has(record.id) &&
            state.recordsMap.get(record.id) !== record,
        );
        const deletedIds = state.records
          .filter((record) => !newRecordsMap.has(record.id))
          .map((record) => record.id);
        const rowsChanged =
          inserted.length > 0 || updated.length > 0 || deletedIds.length > 0;
        const pendingChanged =
          newPendingIds.size !== state.pendingIds.size ||
          Array.from(newPendingIds).some((id) => !state.pendingIds.has(id));
        if (
          !snapshot &&
          !rowsChanged &&
          !pendingChanged &&
          serverTotal === state.serverTotal &&
          serverSequence === state.serverSequence &&
          oldestSequenceFloor === state.serverOldestSequence
        ) {
          if (detailRemoved || reconciledDetail !== state.currentRecord)
            set(detailState);
          return false;
        }
        set({
          records: retainedRecords,
          recordsMap: newRecordsMap,
          ...detailState,
          serverTotal,
          ...(read ? { serverOldestSequence: oldestSequenceFloor } : {}),
          serverSequence,
          hasMore: response.has_more,
          hasNewer: snapshot ? false : state.hasNewer,
          oldestSequence: boundaries.oldestSequence,
          lastId: boundaries.lastId,
          lastSequence: boundaries.lastSequence,
          pendingIds: newPendingIds,
          newRecordsCount: snapshot ? 0 : state.newRecordsCount,
          filterVersion: snapshot
            ? state.filterVersion + 1
            : state.filterVersion,
          recordsMutation: createRecordsMutation({
            reset: !!snapshot,
            inserted: snapshot ? retainedRecords : inserted,
            updated: snapshot ? [] : updated,
            deletedIds: snapshot ? [] : deletedIds,
            ...(snapshot ? {} : { oldestSequenceFloor }),
          }),
        });
        pushService.updateSubscription({
          last_traffic_id: boundaries.lastId || undefined,
          last_sequence: boundaries.lastSequence || undefined,
          pending_ids: Array.from(newPendingIds),
        });
        return true;
      };
      try {
        const filter: TrafficUpdatesFilter = {
          limit: INITIAL_WINDOW_LIMIT,
        };
        let response: Awaited<ReturnType<typeof api.getTrafficUpdates>>;
        const deadline = Date.now() + RECONCILIATION_TIMEOUT_MS;
        let attempts = 0;
        do {
          if (attempts++ >= RECONCILIATION_MAX_READS)
            throw new Error("Traffic reconciliation retry limit exceeded");
          if (read) {
            // Cancel metadata/rows queued before a membership change. Their
            // surviving writes remain in the bounded per-read journal.
            if (read.invalidated) clearPendingTrafficBatch();
            read.invalidated = false;
            if (
              captureCoordination.pendingBatch &&
              (!read.metadata ||
                captureCoordination.pendingBatch.serverSequence >=
                  read.metadata.sequence)
            )
              read.metadata = {
                total: captureCoordination.pendingBatch.serverTotal,
                sequence: captureCoordination.pendingBatch.serverSequence,
              };
            read.snapshotTotal = null;
            read.emptyObserved = false;
            read.positiveWrites = false;
          }
          const metadataAtReadStart = read?.metadata;
          response = await withinReconciliationDeadline(
            api.getTrafficUpdates(filter),
            deadline,
          );
          if (
            generation !== captureCoordination.historyBackfillGeneration ||
            resumeGeneration !== captureCoordination.trafficResumeGeneration
          )
            return;
          if (read) {
            read.snapshotTotal = Math.max(
              response.server_total,
              response.new_records.length,
              response.updated_records.length,
            );
            if (
              read.emptyObserved &&
              (read.snapshotTotal > 0 || read.positiveWrites)
            )
              read.invalidated = true;
          }
          // Deletions do not advance server_sequence. Delivery after the
          // query snapshot invalidates it even when no visible ID matched.
          if (
            read?.metadata &&
            read.metadata !== metadataAtReadStart &&
            read.metadata.sequence >= response.server_sequence &&
            read.metadata.total < response.server_total
          ) {
            read.implicitRemoval = true;
            read.invalidated = true;
          }
          if (
            read?.metadata &&
            read.metadata.sequence === response.server_sequence &&
            read.metadata.total !== response.server_total
          )
            read.implicitRemoval = true;
          queriedLiveIds = new Set();
          confirmedLiveIds = new Set();
          completedMembershipIds = new Set();
          if (read && !read.invalidated) {
            for (const record of response.new_records)
              if (!read.deletedLiveIds.has(record.id))
                read.unconfirmedClearIds.delete(record.id);
          }
          if (
            read &&
            (read.implicitRemoval || read.unconfirmedClearIds.size > 0) &&
            !read.invalidated
          ) {
            const snapshotIds = new Set(
              response.new_records.map((record) => record.id),
            );
            const candidates = [...get().records, ...read.liveRecords.values()];
            queriedLiveIds = new Set(
              candidates
                .filter(
                  (record) =>
                    (read.implicitRemoval ||
                      read.unconfirmedClearIds.has(record.id)) &&
                    !snapshotIds.has(record.id) &&
                    !read.deletedLiveIds.has(record.id) &&
                    (read.oldestSequence === null ||
                      record.sequence >= read.oldestSequence),
                )
                .map((record) => record.id)
                .concat(
                  [
                    ...read.unconfirmedClearIds,
                    ...(read.implicitRemoval
                      ? [get().selectedId, get().currentRecord?.id].filter(
                          (id): id is string => Boolean(id),
                        )
                      : []),
                  ].filter(
                    (id) =>
                      !snapshotIds.has(id) && !read.deletedLiveIds.has(id),
                  ),
                ),
            );
            membershipLiveAtStart = new Map(read.liveRecords);
            const ids = Array.from(queriedLiveIds);
            for (
              let offset = 0;
              offset < ids.length;
              offset += MEMBERSHIP_QUERY_LIMIT
            ) {
              const chunk = ids.slice(offset, offset + MEMBERSHIP_QUERY_LIMIT);
              const membership = await withinReconciliationDeadline(
                api.queryTraffic({
                  record_ids: chunk,
                  limit: chunk.length,
                }),
                deadline,
              );
              if (
                generation !== captureCoordination.historyBackfillGeneration ||
                resumeGeneration !== captureCoordination.trafficResumeGeneration
              )
                return;
              // Absence from a bounded latest window does not prove deletion.
              // Only a complete ID query can remove earlier live arrivals.
              if (
                membership.has_more ||
                membership.total !== membership.records.length
              ) {
                throw new Error("Incomplete traffic membership snapshot");
              }
              const found = new Set(
                membership.records.map((record) => record.id),
              );
              const missing = new Set<string>();
              for (const id of chunk) {
                completedMembershipIds.add(id);
                // Keep negative membership evidence across later retries.
                // Writes received after the query snapshot retain their new
                // reference and must not be discarded by its older miss.
                if (
                  !found.has(id) &&
                  read.liveRecords.get(id) === membershipLiveAtStart.get(id)
                ) {
                  read.deletedLiveIds.add(id);
                  read.liveRecords.delete(id);
                  read.liveTotals.delete(id);
                  missing.add(id);
                }
              }
              if (captureCoordination.pendingBatch && missing.size > 0) {
                captureCoordination.pendingBatch.newRecords =
                  captureCoordination.pendingBatch.newRecords.filter(
                    (record) => !missing.has(record.id),
                  );
                captureCoordination.pendingBatch.updatedRecords =
                  captureCoordination.pendingBatch.updatedRecords.filter(
                    (record) => !missing.has(record.id),
                  );
              }
              for (const record of membership.records) {
                confirmedLiveIds.add(record.id);
                if (
                  read.unconfirmedClearIds.has(record.id) &&
                  !read.deletedLiveIds.has(record.id)
                ) {
                  const authority =
                    compactTrafficSummaryToTrafficSummary(record);
                  const live = read.liveRecords.get(record.id);
                  read.liveRecords.set(
                    record.id,
                    live
                      ? mergeIncrementalRecord(authority, live)
                      : preprocessTrafficRecord(authority),
                  );
                  read.liveTotals.set(record.id, membership.total);
                  read.unconfirmedClearIds.delete(record.id);
                }
              }
            }
          }
        } while (read?.invalidated);
        if (
          generation !== captureCoordination.historyBackfillGeneration ||
          resumeGeneration !== captureCoordination.trafficResumeGeneration
        )
          return;
        committed = commit(response);
        // Allocation cursors can stay equal while preallocated writes commit
        // or deletions occur. Settle conflicting counts with a read issued
        // after both observations, rather than infer order from that cursor.
        if (
          read?.metadata &&
          read.metadata.sequence === response.server_sequence &&
          read.metadata.total !== response.server_total
        )
          void get().fetchTrafficStatistics(undefined, true);
      } catch {
        // Retry membership on a later push/reconnect; never apply a stale or
        // incomplete snapshot when the replacement query fails.
        if (read?.implicitRemoval && get().records.length > 0)
          captureCoordination.membershipReconciliationRequired = true;
      } finally {
        // A failed/deadline/cancelled read cannot discard accepted writes or
        // commit its obsolete GET. A new clear/database generation supersedes
        // this journal; a pause/resume cancellation still retains its writes.
        if (
          read &&
          !committed &&
          generation === captureCoordination.historyBackfillGeneration
        ) {
          const restored = commit();
          captureCoordination.membershipReconciliationRequired ||=
            read.implicitRemoval || read.unconfirmedClearIds.size > 0;
          // Idle connections need not emit another delta. Refresh the count
          // independently once, rather than wait for an event to repair it.
          if (
            resumeGeneration === captureCoordination.trafficResumeGeneration &&
            (restored || read.implicitRemoval || read.deletedLiveIds.size > 0)
          )
            void get().fetchTrafficStatistics(undefined, true);
        }
        if (read) captureCoordination.liveReconciliationReads.delete(read);
      }
    },

    handleTrafficDeleted: (
      ids: string[],
      authoritativeTotal?: number,
      confirmed = true,
    ) => {
      if (ids.length === 0) return;
      const idsSet = new Set(ids);
      if (
        confirmed &&
        captureCoordination.selectedClearDeletions?.generation ===
          captureCoordination.historyBackfillGeneration
      )
        for (const id of idsSet) {
          captureCoordination.selectedClearDeletions.ids.add(id);
          captureCoordination.selectedClearDeletions.liveRecords.delete(id);
        }
      for (const read of captureCoordination.liveReconciliationReads) {
        read.invalidated = true;
        for (const id of idsSet) {
          if (confirmed) {
            read.deletedLiveIds.add(id);
            read.unconfirmedClearIds.delete(id);
            read.liveRecords.delete(id);
            read.liveTotals.delete(id);
          } else read.unconfirmedClearIds.add(id);
        }
      }
      if (captureCoordination.pendingBatch) {
        captureCoordination.pendingBatch.newRecords =
          captureCoordination.pendingBatch.newRecords.filter(
            (record) => !idsSet.has(record.id),
          );
        captureCoordination.pendingBatch.updatedRecords =
          captureCoordination.pendingBatch.updatedRecords.filter(
            (record) => !idsSet.has(record.id),
          );
      }

      set((prevState) => {
        const recordsMap = new Map(prevState.recordsMap);
        const pendingIds = new Set(prevState.pendingIds);
        let removedCount = 0;
        let pendingChanged = false;

        for (const id of idsSet) {
          if (recordsMap.delete(id)) {
            removedCount += 1;
          }
          if (
            !confirmed &&
            !captureCoordination.selectedClearDeletions?.ids.has(id)
          ) {
            if (!pendingIds.has(id)) pendingChanged = true;
            pendingIds.add(id);
          } else if (pendingIds.delete(id)) pendingChanged = true;
        }

        capPendingIds(pendingIds);
        const currentDeleted =
          prevState.currentRecord && idsSet.has(prevState.currentRecord.id);
        const selectedDeleted =
          prevState.selectedId && idsSet.has(prevState.selectedId);

        if (
          !currentDeleted &&
          !selectedDeleted &&
          removedCount === 0 &&
          !pendingChanged
        ) {
          return {};
        }

        const records =
          removedCount > 0
            ? prevState.records.filter((record) => !idsSet.has(record.id))
            : prevState.records;
        const boundaries = getBoundaryState(records);

        const detailRemoved = currentDeleted || !!selectedDeleted;
        return {
          records,
          recordsMap,
          pendingIds,
          // A total snapshot may already include this deletion. Row removal
          // proves membership, not an extra subtraction from snapshot totals.
          serverTotal: authoritativeTotal ?? prevState.serverTotal,
          oldestSequence: boundaries.oldestSequence,
          lastId: prevState.lastId,
          lastSequence: prevState.lastSequence,
          currentRecord: detailRemoved ? null : prevState.currentRecord,
          requestBody: detailRemoved ? null : prevState.requestBody,
          responseBody: detailRemoved ? null : prevState.responseBody,
          requestRawBody: detailRemoved ? null : prevState.requestRawBody,
          responseRawBody: detailRemoved ? null : prevState.responseRawBody,
          detailLoading: detailRemoved ? false : prevState.detailLoading,
          detailError: detailRemoved
            ? t("shared.traffic.requestDeleted")
            : prevState.detailError,
          selectedId: selectedDeleted ? undefined : prevState.selectedId,
          filterVersion:
            removedCount > 0
              ? prevState.filterVersion + 1
              : prevState.filterVersion,
          recordsMutation:
            removedCount > 0
              ? createRecordsMutation({
                  reset: false,
                  inserted: [],
                  updated: [],
                  deletedIds: Array.from(idsSet),
                })
              : prevState.recordsMutation,
        };
      });
      if (authoritativeTotal === undefined && get().serverTotal > 0)
        void get().fetchTrafficStatistics(undefined, true);
    },

    clearTraffic: async (ids?: string[]) => {
      if (get().clearing) return false;
      set({ error: null, clearing: true });
      const clearGeneration = captureCoordination.historyBackfillGeneration;
      const deletions = ids?.length
        ? {
            generation: captureCoordination.historyBackfillGeneration,
            ids: new Set<string>(),
            requestedIds: new Set(ids),
            liveRecords: new Map(
              get()
                .records.filter((record) => ids.includes(record.id))
                .map((record) => [record.id, record]),
            ),
          }
        : null;
      captureCoordination.selectedClearDeletions = deletions;
      try {
        const result = await api.clearTraffic(
          ids && ids.length > 0 ? ids : undefined,
        );
        if (result?.success === false) {
          throw new Error(result.message || "Failed to clear traffic");
        }
      } catch (e) {
        // A failed response cannot undo deletion frames already confirmed by
        // the server. Hand their tombstones and accepted writes to recovery
        // before releasing the DELETE journal.
        if (clearGeneration === captureCoordination.historyBackfillGeneration) {
          void get().reloadRecords(
            true,
            deletions ?? {
              ids: new Set(),
              requestedIds: new Set(),
              liveRecords: new Map(),
              reconcileMembership: true,
            },
          );
          if (!deletions) pushService.resetTrafficCursor();
        }
        if (captureCoordination.selectedClearDeletions === deletions)
          captureCoordination.selectedClearDeletions = null;
        set({ error: getApiErrorPayload(e).message, clearing: false });
        return false;
      }
      try {
        // The DELETE belongs to the database visible when it was issued.
        // Never hand its buffered fields or subscription edits into a new one.
        if (clearGeneration !== captureCoordination.historyBackfillGeneration)
          return true;
        if (ids && ids.length > 0) {
          get().handleTrafficDeleted(ids, get().serverTotal, false);
          pushService.updateSubscription({
            pending_ids: Array.from(get().pendingIds),
          });
          void get().fetchTrafficStatistics(undefined, true);
          // A successful ID clear may preserve active connections. Let the
          // authoritative read recover those rows and the post-delete count.
          // Confirmed deletion frames are tombstones. Merely requested IDs
          // await membership evidence while their completion fields survive.
          void get().reloadRecords(true, deletions ?? undefined);
          if (captureCoordination.selectedClearDeletions === deletions)
            captureCoordination.selectedClearDeletions = null;

          return true;
        }

        captureCoordination.historyBackfillGeneration += 1;
        captureCoordination.membershipReconciliationRequired = false;
        captureCoordination.membershipFollowupRequired = false;
        captureCoordination.trafficResumeStatisticsPending = false;
        captureCoordination.trafficEpochResetSuspected = false;
        clearPendingTrafficBatch();
        set({
          records: [],
          recordsMap: new Map(),
          serverTotal: 0,
          serverSequence: 0,
          serverOldestSequence: null,
          hasMore: false,
          hasNewer: false,
          oldestSequence: null,
          lastId: null,
          lastSequence: null,
          pendingIds: new Set(),
          currentRecord: null,
          requestBody: null,
          responseBody: null,
          requestRawBody: null,
          responseRawBody: null,
          detailError: null,
          loading: false,
          filterVersion: 0,
          initialized: false,
          selectedId: undefined,
          historyLoading: false,
          catchingUp: false,
          recordsMutation: createRecordsMutation({
            reset: true,
            inserted: [],
            updated: [],
            deletedIds: [],
          }),
        });

        pushService.updateSubscription({ pending_ids: [] });

        // Recover active connections and records written after the server's
        // clear point, including pushes received while the API was pending.
        pushService.resetTrafficCursor();
        // DELETE succeeded. Background synchronization has a separate bounded
        // lifetime; it must not hold the button or turn success into failure.
        void get().reloadRecords(true);
        return true;
      } finally {
        if (captureCoordination.selectedClearDeletions === deletions)
          captureCoordination.selectedClearDeletions = null;
        set({ clearing: false });
      }
    },
  };
}
