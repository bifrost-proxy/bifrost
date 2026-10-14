import type { TrafficSummary, TrafficDeltaData } from "../types";
import type { TrafficUpdatesData } from "../services/pushService";
import pushService from "../services/pushService";
import type { TrafficStoreAccess } from "./trafficStoreTypes";
import type { TrafficCaptureCoordinator } from "./trafficCaptureCoordinator";
import {
  captureCoordination,
  UPDATE_THROTTLE_MS,
} from "./trafficCaptureCoordinator";
import {
  buildTrafficRecordsMap,
  MAX_TRAFFIC_WINDOW_RECORDS,
  mergeBoundedTrafficWindow,
} from "./trafficWindow";
import {
  isPendingRecord,
  shouldReplaceRecord,
  mergeIncrementalRecord,
  preprocessTrafficRecords,
  mergeDetailWithSummary,
  mergeNewRecordsIntoList,
  replaceUpdatedTrafficRecordsInList,
  findLastRecordById,
  hasRecordInList,
  getBoundaryState,
  createRecordsMutation,
  compactTrafficSummaryToTrafficSummary,
} from "./trafficRecordModel";
export function createTrafficLiveCapture(
  { get, set }: TrafficStoreAccess,
  coordination: TrafficCaptureCoordinator,
) {
  const {
    observeReconciliationMetadata,
    observeTrafficRecordSequence,
    acceptTrafficDatabaseEpoch,
    capPendingIds,
  } = coordination;
  return {
    handleTrafficPush: (data: TrafficUpdatesData) => {
      const state = get();
      if (state.paused) return;
      const accepted = acceptTrafficDatabaseEpoch(
        data.database_epoch,
        data.server_sequence,
      );
      // Restored rows withheld by an old cursor still prove that an earlier
      // confirmation is stale. Keep their freshness separate from merging.
      observeTrafficRecordSequence(
        data.database_epoch,
        Math.max(
          data.new_records.reduce(
            (sequence, record) => Math.max(sequence, record.sequence),
            0,
          ),
          data.updated_records.reduce(
            (sequence, record) => Math.max(sequence, record.sequence),
            0,
          ),
        ),
      );
      if (!accepted) return;
      captureCoordination.latestTrafficSequenceSinceResume = Math.max(
        captureCoordination.latestTrafficSequenceSinceResume,
        data.server_sequence ?? 0,
      );
      const hasRecordChanges =
        data.new_records.length > 0 || data.updated_records.length > 0;
      const hasMetadataChanges =
        captureCoordination.membershipReconciliationRequired ||
        state.records.length > data.server_total ||
        data.server_total !== state.serverTotal ||
        (data.server_sequence !== undefined &&
          data.server_sequence !== state.serverSequence) ||
        (data.oldest_sequence !== undefined &&
          data.oldest_sequence !== null &&
          data.oldest_sequence > (state.serverOldestSequence ?? 0));
      if (!hasRecordChanges && !hasMetadataChanges) return;

      observeReconciliationMetadata(
        data.server_total,
        data.server_sequence,
        data.oldest_sequence,
      );
      // Deletion and traffic events use different server queues. Ignore a
      // delayed row already deleted during any active reconciliation read.
      const deletedIds = new Set<string>(
        captureCoordination.selectedClearDeletions?.generation ===
          captureCoordination.historyBackfillGeneration
          ? captureCoordination.selectedClearDeletions.ids
          : [],
      );
      for (const read of captureCoordination.liveReconciliationReads) {
        if (
          read.historyGeneration !==
            captureCoordination.historyBackfillGeneration ||
          read.resumeGeneration !== captureCoordination.trafficResumeGeneration
        )
          continue;
        for (const id of read.deletedLiveIds) deletedIds.add(id);
      }
      const survivesDeletion = (record: TrafficSummary) =>
        !deletedIds.has(record.id);
      const preprocessedNew = preprocessTrafficRecords(data.new_records).filter(
        survivesDeletion,
      );
      const preprocessedUpdated = preprocessTrafficRecords(
        data.updated_records,
      ).filter(survivesDeletion);
      if (
        captureCoordination.selectedClearDeletions?.generation ===
        captureCoordination.historyBackfillGeneration
      ) {
        for (const record of [...preprocessedNew, ...preprocessedUpdated]) {
          if (
            captureCoordination.selectedClearDeletions.requestedIds.has(
              record.id,
            )
          )
            captureCoordination.selectedClearDeletions.liveRecords.set(
              record.id,
              mergeIncrementalRecord(
                captureCoordination.selectedClearDeletions.liveRecords.get(
                  record.id,
                ),
                record,
              ),
            );
        }
      }
      for (const read of captureCoordination.liveReconciliationReads) {
        if (
          data.server_total > 0 &&
          (preprocessedNew.length > 0 || preprocessedUpdated.length > 0)
        )
          read.positiveWrites = true;
        // Retain accepted writes even if their UI batch has not flushed yet.
        for (const record of preprocessedUpdated) {
          read.liveTotals.set(record.id, data.server_total);
          const existing = read.liveRecords.get(record.id);
          const merged = mergeIncrementalRecord(existing, record);
          if (
            shouldReplaceRecord(existing, merged) ||
            existing?.sequence !== merged.sequence
          )
            read.liveRecords.set(record.id, merged);
        }
        for (const record of preprocessedNew) {
          read.liveTotals.set(record.id, data.server_total);
          if (!read.liveRecords.has(record.id))
            read.liveRecords.set(record.id, record);
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
      }

      // Await membership evidence for requested IDs, but retain their live
      // fields in the journal so a surviving active row can complete.
      const canPublish = (record: TrafficSummary) =>
        !Array.from(captureCoordination.liveReconciliationReads).some(
          (read) =>
            read.historyGeneration ===
              captureCoordination.historyBackfillGeneration &&
            read.resumeGeneration ===
              captureCoordination.trafficResumeGeneration &&
            read.unconfirmedClearIds.has(record.id),
        );
      const publishNew = preprocessedNew.filter(canPublish);
      const publishUpdated = preprocessedUpdated.filter(canPublish);
      if (captureCoordination.pendingBatch) {
        captureCoordination.pendingBatch.newRecords = mergeBoundedTrafficWindow(
          captureCoordination.pendingBatch.newRecords,
          publishNew,
          "newer",
        ).records;
        captureCoordination.pendingBatch.updatedRecords =
          mergeBoundedTrafficWindow(
            captureCoordination.pendingBatch.updatedRecords,
            publishUpdated,
            "newer",
          ).records;
        if (
          (data.server_sequence ??
            captureCoordination.pendingBatch.serverSequence) >=
          captureCoordination.pendingBatch.serverSequence
        )
          captureCoordination.pendingBatch.serverTotal = data.server_total;
        captureCoordination.pendingBatch.serverSequence = Math.max(
          captureCoordination.pendingBatch.serverSequence,
          data.server_sequence ?? 0,
        );
        if (
          data.oldest_sequence !== undefined &&
          data.oldest_sequence !== null
        ) {
          captureCoordination.pendingBatch.oldestSequence = Math.max(
            captureCoordination.pendingBatch.oldestSequence ?? 0,
            data.oldest_sequence,
          );
        }
        captureCoordination.pendingBatch.sourceNewRecordCount +=
          preprocessedNew.length;
        captureCoordination.pendingBatch.sourceUpdatedRecordCount +=
          preprocessedUpdated.length;
        captureCoordination.pendingBatch.hasMore = data.has_more;
      } else {
        captureCoordination.pendingBatch = {
          newRecords: mergeBoundedTrafficWindow([], publishNew, "newer")
            .records,
          updatedRecords: mergeBoundedTrafficWindow([], publishUpdated, "newer")
            .records,
          serverTotal: data.server_total,
          serverSequence: data.server_sequence ?? state.serverSequence,
          oldestSequence: data.oldest_sequence ?? state.serverOldestSequence,
          sourceNewRecordCount: preprocessedNew.length,
          sourceUpdatedRecordCount: preprocessedUpdated.length,
          hasMore: data.has_more,
        };
      }

      const now = performance.now();
      const timeSinceLastUpdate = now - captureCoordination.lastUpdateTime;

      if (
        captureCoordination.rafId !== null ||
        captureCoordination.updateTimerId !== null
      ) {
        return;
      }

      const scheduleUpdate = () => {
        captureCoordination.updateTimerId = null;
        captureCoordination.rafId = requestAnimationFrame(() => {
          captureCoordination.rafId = null;
          const batch = captureCoordination.pendingBatch;
          if (!batch) return;
          captureCoordination.pendingBatch = null;
          captureCoordination.lastUpdateTime = performance.now();
          let needsMembershipReconciliation = false;

          set((prevState) => {
            const recordsMap = new Map(prevState.recordsMap);
            let hasChanges = false;
            const uniqueNewRecords: TrafficSummary[] = [];
            const replacedRecords: TrafficSummary[] = [];
            let promotedUpdateCount = 0;

            for (const r of batch.updatedRecords) {
              const existing = recordsMap.get(r.id);
              const mergedRecord = mergeIncrementalRecord(existing, r);
              if (shouldReplaceRecord(existing, mergedRecord)) {
                recordsMap.set(r.id, mergedRecord);
                hasChanges = true;
                if (hasRecordInList(prevState.records, r.id)) {
                  replacedRecords.push(mergedRecord);
                } else {
                  uniqueNewRecords.push(mergedRecord);
                  promotedUpdateCount += 1;
                }
              }
            }

            let actualNewCount = 0;
            for (const r of batch.newRecords) {
              if (!recordsMap.has(r.id)) {
                recordsMap.set(r.id, r);
                hasChanges = true;
                actualNewCount++;
                uniqueNewRecords.push(r);
              }
            }

            const newPendingIds = new Set(prevState.pendingIds);

            for (const r of batch.updatedRecords) {
              const isPending = isPendingRecord(r);
              if (!isPending) {
                newPendingIds.delete(r.id);
              }
            }

            for (const r of batch.newRecords) {
              const isPending = isPendingRecord(r);
              if (isPending) {
                newPendingIds.add(r.id);
              }
            }
            capPendingIds(newPendingIds);

            let allRecords: TrafficSummary[];
            if (hasChanges) {
              allRecords = replaceUpdatedTrafficRecordsInList(
                prevState.records,
                replacedRecords,
              );
              if (!prevState.hasNewer) {
                allRecords = mergeNewRecordsIntoList(
                  allRecords,
                  uniqueNewRecords,
                );
              }
            } else {
              allRecords = prevState.records;
            }
            const serverOldestSequence =
              batch.oldestSequence === null
                ? prevState.serverOldestSequence
                : Math.max(
                    prevState.serverOldestSequence ?? 0,
                    batch.oldestSequence,
                  );
            const countBeforeRetention = allRecords.length;
            if (serverOldestSequence !== null) {
              allRecords = allRecords.filter(
                (record) => record.sequence >= serverOldestSequence,
              );
            }
            const retainedRemovals = countBeforeRetention - allRecords.length;
            needsMembershipReconciliation =
              allRecords.length > batch.serverTotal ||
              (serverOldestSequence === prevState.serverOldestSequence &&
                prevState.serverTotal - batch.serverTotal > retainedRemovals);
            // A smaller total does not identify deleted rows. Keep the bounded
            // window intact until an authoritative ID query reconciles misses.
            const bounded = mergeBoundedTrafficWindow(
              allRecords,
              [],
              "newer",
              MAX_TRAFFIC_WINDOW_RECORDS,
            );
            allRecords = bounded.records;
            const boundaries = getBoundaryState(allRecords);
            const visibleRecordsMap = buildTrafficRecordsMap(allRecords);
            for (const pendingId of newPendingIds) {
              if (!visibleRecordsMap.has(pendingId)) {
                newPendingIds.delete(pendingId);
              }
            }
            const sourceLatest = [
              ...batch.updatedRecords,
              ...batch.newRecords,
            ].reduce<TrafficSummary | null>(
              (latest, record) =>
                !latest || record.sequence > latest.sequence ? record : latest,
              null,
            );
            const sourceLastSequence =
              Math.max(
                prevState.lastSequence ?? 0,
                sourceLatest?.sequence ?? 0,
              ) || null;
            const sourceLastId =
              sourceLatest &&
              sourceLatest.sequence >= (prevState.lastSequence ?? 0)
                ? sourceLatest.id
                : prevState.lastId;

            const updatedNewRecordsCount =
              prevState.autoScroll && !prevState.hasNewer
                ? 0
                : prevState.newRecordsCount +
                  Math.max(actualNewCount, batch.sourceNewRecordCount) +
                  promotedUpdateCount;

            pushService.updateSubscription({
              last_traffic_id: sourceLastId || undefined,
              last_sequence: sourceLastSequence || undefined,
              pending_ids: Array.from(newPendingIds),
            });

            let updatedCurrentRecord = prevState.currentRecord;
            if (updatedCurrentRecord) {
              const updatedSummary = findLastRecordById(
                batch.updatedRecords,
                updatedCurrentRecord.id,
              );
              if (updatedSummary) {
                updatedCurrentRecord = mergeDetailWithSummary(
                  updatedCurrentRecord,
                  updatedSummary,
                );
              }
            }

            return {
              records: allRecords,
              recordsMap: visibleRecordsMap,
              serverTotal: batch.serverTotal,
              serverSequence: Math.max(
                prevState.serverSequence,
                batch.serverSequence,
              ),
              serverOldestSequence,
              hasMore: prevState.hasMore || bounded.trimmedSide === "older",
              oldestSequence: boundaries.oldestSequence,
              lastId: sourceLastId,
              lastSequence: sourceLastSequence,
              pendingIds: newPendingIds,
              newRecordsCount: updatedNewRecordsCount,
              currentRecord: updatedCurrentRecord,
              recordsMutation:
                hasChanges ||
                serverOldestSequence !== prevState.serverOldestSequence
                  ? createRecordsMutation({
                      reset: false,
                      inserted: uniqueNewRecords,
                      updated: replacedRecords,
                      deletedIds: [],
                      oldestSequenceFloor: serverOldestSequence,
                      incomplete:
                        batch.hasMore ||
                        batch.sourceNewRecordCount > batch.newRecords.length ||
                        batch.sourceUpdatedRecordCount >
                          batch.updatedRecords.length,
                    })
                  : prevState.recordsMutation,
            };
          });
          captureCoordination.membershipReconciliationRequired ||=
            needsMembershipReconciliation;
          if (
            captureCoordination.membershipReconciliationRequired &&
            !batch.hasMore
          ) {
            if (captureCoordination.membershipRequest) {
              captureCoordination.membershipFollowupRequired = true;
            } else {
              void get().reconcileTrafficWindow();
            }
          }
        });
      };

      if (timeSinceLastUpdate >= UPDATE_THROTTLE_MS) {
        scheduleUpdate();
      } else {
        captureCoordination.updateTimerId = window.setTimeout(
          scheduleUpdate,
          UPDATE_THROTTLE_MS - timeSinceLastUpdate,
        );
      }
    },

    handleTrafficDelta: (data: TrafficDeltaData) => {
      get().handleTrafficPush({
        database_epoch: data.database_epoch,
        new_records: data.inserts.map(compactTrafficSummaryToTrafficSummary),
        updated_records: data.updates.map(
          compactTrafficSummaryToTrafficSummary,
        ),
        has_more: data.has_more,
        server_total: data.server_total,
        server_sequence: data.server_sequence,
        oldest_sequence: data.oldest_sequence,
      });
    },
  };
}
