import { createTrafficLiveCapture } from "./trafficLiveCapture";
// Traffic state wiring; domain logic lives in capture, reconciliation, record and filter modules.
import { create } from "zustand";
import { persist } from "zustand/middleware";
import type {
  TrafficSummary,
  ToolbarFilters,
  FilterCondition,
  TrafficUpdatesFilter,
  TrafficStatistics,
} from "../types";
import * as api from "../api";
import pushService from "../services/pushService";
import type { TrafficState } from "./trafficStoreTypes";
import {
  captureCoordination,
  createTrafficCaptureCoordinator,
  POLL_INTERVAL,
  POLL_MIN_INTERVAL,
  HAS_MORE_BURST_LIMIT,
  HAS_MORE_BACKOFF_INTERVAL,
  INITIAL_WINDOW_LIMIT,
  HISTORY_BATCH_LIMIT,
  UPDATE_BATCH_LIMIT,
  MEMBERSHIP_QUERY_LIMIT,
} from "./trafficCaptureCoordinator";
import { createTrafficReconciliation } from "./trafficReconciliation";
import {
  mergeSseBody,
  SSE_RESPONSE_BODY_CHAR_LIMIT,
  isPendingRecord,
  preprocessTrafficRecords,
  mergeDetailWithSummary,
  getBoundaryState,
  createRecordsMutation,
  createEmptyRecordsMutation,
  snapshotServerStatistics,
  compactTrafficSummaryToTrafficSummary,
} from "./trafficRecordModel";

import {
  buildTrafficRecordsMap,
  getTrafficWindowBoundaries,
  mergeBoundedTrafficWindow,
} from "./trafficWindow";
import { t } from "../i18n";
export type { TrafficRecordsMutation } from "./trafficStoreTypes";
export type { PanelFilters } from "./trafficFilters";
export {
  preprocessTrafficRecord,
  preprocessTrafficRecords,
  compareTrafficRecordsBySequence,
  mergeSortedTrafficRecords,
  replaceUpdatedTrafficRecordsInList,
  compactTrafficSummaryToTrafficSummary,
} from "./trafficRecordModel";
export {
  isFilterConditionApplicable,
  hasAnyTrafficFilters,
  matchesTrafficFilters,
  filterRecords,
  applyTrafficRecordsMutationToFilteredRecords,
} from "./trafficFilters";
const TRAFFIC_SELECTION_SYNC_CHANNEL = "bifrost-traffic-selection-sync";
const trafficSelectionSyncChannel =
  typeof BroadcastChannel !== "undefined"
    ? new BroadcastChannel(TRAFFIC_SELECTION_SYNC_CHANNEL)
    : null;

export const useTrafficStore = create<TrafficState>()(
  persist(
    (set, get) => {
      const coordination = createTrafficCaptureCoordinator({ get, set });
      const {
        observeReconciliationMetadata,
        observeTrafficRecordSequence,
        getConsumedTrafficSequence,
        acceptTrafficDatabaseEpoch,
        capPendingIds,
        clearPendingTrafficBatch,
      } = coordination;
      return {
        records: [],
        recordsMap: new Map(),
        currentRecord: null,
        requestBody: null,
        responseBody: null,
        requestRawBody: null,
        responseRawBody: null,
        serverTotal: 0,
        serverSequence: 0,
        serverOldestSequence: null,
        trafficEpochVersion: 0,
        trafficDatabaseEpoch: null,
        hasMore: false,
        hasNewer: false,
        oldestSequence: null,
        lastId: null,
        lastSequence: null,
        pendingIds: new Set(),
        toolbarFilters: {
          rule: [],
          protocol: [],
          type: [],
          status: [],
          imported: [],
        },
        filterConditions: [],
        paused: false,
        loading: false,
        detailLoading: false,
        polling: false,
        error: null,
        detailError: null,
        pollTimeoutId: null,
        autoScroll: true,
        newRecordsCount: 0,
        scrollTop: 0,
        usePush: true,
        pushUnsubscribe: null,
        pushDeltaUnsubscribe: null,
        pushDeletedUnsubscribe: null,
        pushStatisticsUnsubscribe: null,
        pushConnectionUnsubscribe: null,
        filterVersion: 0,
        initialized: false,
        selectedId: undefined,
        useDbMode: true,
        historyLoading: false,
        catchingUp: false,
        availableClientApps: [],
        availableAccountNames: [],
        availableClientIps: [],
        availableProxyPorts: [],
        availableDomains: [],
        clientAppCounts: new Map(),
        accountNameCounts: new Map(),
        clientIpCounts: new Map(),
        proxyPortCounts: new Map(),
        domainCounts: new Map(),
        trafficStatisticsTotal: 0,
        trafficStatisticsSequence: 0,
        trafficStatisticsLoaded: false,
        clearing: false,
        recordsMutation: createEmptyRecordsMutation(),

        fetchTrafficStatistics: async (
          expectedEpoch?: string,
          reconcileTotal = false,
        ) => {
          const generation = captureCoordination.historyBackfillGeneration;
          const resumeGeneration = captureCoordination.trafficResumeGeneration;
          // A count read issued after a deletion must supersede an older in-flight
          // read; coalescing that older request would not prove post-delete totals.
          if (
            reconcileTotal ||
            !captureCoordination.statisticsRequest ||
            captureCoordination.statisticsRequest.historyGeneration !==
              generation ||
            captureCoordination.statisticsRequest.resumeGeneration !==
              resumeGeneration ||
            captureCoordination.statisticsRequest.databaseEpoch !==
              expectedEpoch
          ) {
            const request = api
              .getTrafficStatistics()
              .then((statistics) => {
                if (
                  captureCoordination.statisticsRequest?.promise !== request ||
                  generation !==
                    captureCoordination.historyBackfillGeneration ||
                  resumeGeneration !==
                    captureCoordination.trafficResumeGeneration
                )
                  return;
                get().handleTrafficStatistics(statistics, reconcileTotal);
              })
              .catch(() => {
                // Statistics are refreshed independently from the traffic window.
                // Keep the last authoritative snapshot if a transient request fails.
              })
              .finally(() => {
                if (captureCoordination.statisticsRequest?.promise === request)
                  captureCoordination.statisticsRequest = null;
              });
            captureCoordination.statisticsRequest = {
              historyGeneration: generation,
              resumeGeneration,
              databaseEpoch: expectedEpoch,
              promise: request,
            };
          }
          await captureCoordination.statisticsRequest.promise;
        },

        startPolling: () => {
          const state = get();
          if (state.polling) return;

          set({ polling: true });

          if (state.usePush) {
            get().enablePush();
          } else {
            get().fetchUpdates();
          }
        },

        stopPolling: () => {
          const state = get();
          if (state.pollTimeoutId) {
            clearTimeout(state.pollTimeoutId);
          }
          captureCoordination.hasMoreBurst = 0;
          if (state.usePush) {
            get().disablePush();
          }
          set({ polling: false, pollTimeoutId: null });
        },

        enablePush: () => {
          const state = get();
          if (
            state.pushUnsubscribe ||
            state.pushDeltaUnsubscribe ||
            state.pushDeletedUnsubscribe ||
            state.pushStatisticsUnsubscribe ||
            state.pushConnectionUnsubscribe
          )
            return;

          const unsubscribe = pushService.onTrafficUpdates((data) => {
            get().handleTrafficPush(data);
          });

          const unsubscribeDelta = pushService.onTrafficDelta((data) => {
            get().handleTrafficDelta(data);
          });

          const unsubscribeDeleted = pushService.onTrafficDeleted((data) => {
            get().handleTrafficDeleted(data.ids);
          });

          const unsubscribeStatistics = pushService.onTrafficStatistics(
            (statistics) => {
              get().handleTrafficStatistics(statistics);
            },
          );

          // Deletion notifications are not replayed. Equal-total reconnects can
          // still replace cached rows; statistics also finish bounded backlogs.
          const unsubscribeConnection = pushService.onConnectionChange(
            ({ connected }) => {
              if (!connected) {
                captureCoordination.latestTrafficSequenceSinceResume = 0;
                captureCoordination.trafficResumeGeneration += 1;
                captureCoordination.trafficEpochResetSuspected = false;
                if (
                  get().records.length > 0 ||
                  get().lastSequence !== null ||
                  captureCoordination.pendingBatch !== null
                ) {
                  captureCoordination.membershipReconciliationRequired = true;
                  captureCoordination.trafficResumeStatisticsPending = true;
                }
              }
            },
          );

          set({
            pushUnsubscribe: unsubscribe,
            pushDeltaUnsubscribe: unsubscribeDelta,
            pushDeletedUnsubscribe: unsubscribeDeleted,
            pushStatisticsUnsubscribe: unsubscribeStatistics,
            pushConnectionUnsubscribe: unsubscribeConnection,
          });

          const subscription = {
            last_traffic_id: state.lastId || undefined,
            last_sequence: state.lastSequence || undefined,
            pending_ids: Array.from(state.pendingIds),
            need_traffic: true,
          };

          pushService.connect(subscription);
        },

        disablePush: () => {
          const state = get();
          captureCoordination.latestTrafficSequenceSinceResume = 0;
          captureCoordination.trafficResumeGeneration += 1;
          captureCoordination.trafficEpochResetSuspected = false;
          if (
            state.pushConnectionUnsubscribe &&
            (state.records.length > 0 ||
              state.lastSequence !== null ||
              captureCoordination.pendingBatch !== null)
          ) {
            captureCoordination.membershipReconciliationRequired = true;
            captureCoordination.trafficResumeStatisticsPending = true;
          }
          state.pushConnectionUnsubscribe?.();
          if (state.pushUnsubscribe) {
            state.pushUnsubscribe();
          }
          if (state.pushDeltaUnsubscribe) {
            state.pushDeltaUnsubscribe();
          }
          if (state.pushDeletedUnsubscribe) {
            state.pushDeletedUnsubscribe();
          }
          if (state.pushStatisticsUnsubscribe) {
            state.pushStatisticsUnsubscribe();
          }
          set({
            pushUnsubscribe: null,
            pushDeltaUnsubscribe: null,
            pushDeletedUnsubscribe: null,
            pushStatisticsUnsubscribe: null,
            pushConnectionUnsubscribe: null,
          });
          pushService.updateSubscription({
            need_traffic: false,
            last_traffic_id: undefined,
            last_sequence: undefined,
            pending_ids: [],
          });
          pushService.disconnectIfIdle();
        },

        ...createTrafficLiveCapture({ get, set }, coordination),

        reconcileTrafficWindow: async () => {
          if (!captureCoordination.membershipRequest) {
            captureCoordination.membershipReconciliationRequired = false;
            const generation = captureCoordination.historyBackfillGeneration;
            const recordIds = get().records.map((record) => record.id);
            if (recordIds.length === 0) return;
            captureCoordination.membershipRequest = (async () => {
              const existingIds = new Set<string>();
              for (
                let offset = 0;
                offset < recordIds.length;
                offset += MEMBERSHIP_QUERY_LIMIT
              ) {
                const ids = recordIds.slice(
                  offset,
                  offset + MEMBERSHIP_QUERY_LIMIT,
                );
                const response = await api.queryTraffic({
                  record_ids: ids,
                  limit: ids.length,
                });
                // Never infer deletions from an incomplete/failed query snapshot.
                if (
                  generation !== captureCoordination.historyBackfillGeneration
                )
                  return;
                if (
                  response.has_more ||
                  response.total !== response.records.length
                ) {
                  captureCoordination.membershipReconciliationRequired = true;
                  return;
                }
                for (const record of response.records)
                  existingIds.add(record.id);
              }
              if (generation !== captureCoordination.historyBackfillGeneration)
                return;
              const deletedIds = recordIds.filter((id) => !existingIds.has(id));
              // Delta metadata already included these deletions in the global total.
              // Remove only queried IDs, preserving concurrent arrivals and paging.
              get().handleTrafficDeleted(deletedIds, get().serverTotal);
            })()
              .catch(() => {
                // Keep the visible window on transient errors. A later delta can retry.
                if (
                  generation === captureCoordination.historyBackfillGeneration
                ) {
                  captureCoordination.membershipReconciliationRequired = true;
                }
              })
              .finally(() => {
                captureCoordination.membershipRequest = null;
                if (captureCoordination.membershipFollowupRequired) {
                  captureCoordination.membershipFollowupRequired = false;
                  void get().reconcileTrafficWindow();
                }
              });
          }
          await captureCoordination.membershipRequest;
        },

        handleTrafficStatistics: (
          statistics: TrafficStatistics,
          reconcileTotal = false,
        ) => {
          const state = get();
          const identityChanged =
            !!statistics.database_epoch &&
            !!state.trafficDatabaseEpoch &&
            statistics.database_epoch !== state.trafficDatabaseEpoch;
          const identityMatches =
            !!statistics.database_epoch &&
            statistics.database_epoch === state.trafficDatabaseEpoch;
          const previousSequence = Math.max(
            state.serverSequence,
            state.lastSequence ?? 0,
            captureCoordination.pendingBatch?.serverSequence ?? 0,
          );
          const consumedRecordSequence = getConsumedTrafficSequence();
          // server_sequence is the next allocation, so it must exceed every row
          // already consumed. UUIDs survive backup restore; unused allocations
          // alone (serverSequence) cannot establish that persisted rows rolled back.
          const sameIdentityRecordRollback =
            identityMatches &&
            statistics.server_sequence <= consumedRecordSequence;
          const unprovenLegacyIdentity =
            !state.trafficDatabaseEpoch &&
            !!statistics.database_epoch &&
            statistics.server_sequence < previousSequence;
          // Sequence watermarks only have meaning within one database. A queued
          // old-epoch delta must not suppress a lower new-epoch statistics push.
          if (
            !identityChanged &&
            !unprovenLegacyIdentity &&
            !sameIdentityRecordRollback
          ) {
            observeReconciliationMetadata(
              statistics.total_requests,
              statistics.server_sequence,
            );
          }
          if (
            !identityChanged &&
            !unprovenLegacyIdentity &&
            !sameIdentityRecordRollback &&
            statistics.server_sequence <
              captureCoordination.latestTrafficSequenceSinceResume
          )
            return;
          if (!identityChanged) {
            set({
              ...snapshotServerStatistics(statistics),
              trafficDatabaseEpoch: unprovenLegacyIdentity
                ? state.trafficDatabaseEpoch
                : (statistics.database_epoch ?? state.trafficDatabaseEpoch),
            });
          }
          // Initial statistics follow all reconnect chunks. has_more can remain
          // true on the final chunk because older history lies outside the bounded
          // tail, so use this snapshot to finish membership reconciliation too.
          // Compare only this connection's traffic: empty storage can restart at 1.
          if (
            !state.paused &&
            (reconcileTotal ||
              statistics.total_requests === 0 ||
              identityChanged ||
              sameIdentityRecordRollback ||
              unprovenLegacyIdentity ||
              captureCoordination.membershipReconciliationRequired ||
              captureCoordination.trafficResumeStatisticsPending)
          ) {
            if (
              identityChanged ||
              sameIdentityRecordRollback ||
              (!identityMatches &&
                statistics.server_sequence < previousSequence)
            ) {
              captureCoordination.trafficEpochResetSuspected = true;
              const generation = captureCoordination.historyBackfillGeneration;
              const resumeGeneration =
                captureCoordination.trafficResumeGeneration;
              if (
                captureCoordination.trafficEpochRequest?.historyGeneration !==
                  generation ||
                captureCoordination.trafficEpochRequest.resumeGeneration !==
                  resumeGeneration ||
                captureCoordination.trafficEpochRequest.databaseEpoch !==
                  statistics.database_epoch
              ) {
                // A periodic push may have been captured before this reconnect.
                // Confirm a new identity (or legacy counter rollback) with a fresh
                // read before discarding the current database's window.
                const request = api
                  .getTrafficStatistics()
                  .then((current) => {
                    if (
                      captureCoordination.trafficEpochRequest?.promise !==
                        request ||
                      state.trafficDatabaseEpoch !==
                        get().trafficDatabaseEpoch ||
                      generation !==
                        captureCoordination.historyBackfillGeneration ||
                      resumeGeneration !==
                        captureCoordination.trafficResumeGeneration ||
                      get().paused
                    )
                      return;
                    if (
                      (identityChanged ||
                        sameIdentityRecordRollback ||
                        unprovenLegacyIdentity) &&
                      current.database_epoch !== statistics.database_epoch
                    ) {
                      // A response naming another database cannot confirm this
                      // candidate. A third identity needs its own fresh read.
                      if (current.database_epoch)
                        get().handleTrafficStatistics(current);
                      return;
                    }
                    if (sameIdentityRecordRollback) {
                      // A confirmation captured before newer delivered rows is
                      // stale, even if those rows have not reached the render batch.
                      if (
                        current.server_sequence <=
                        captureCoordination.trafficEpochRequest
                          .latestRecordSequence
                      )
                        return;
                    } else if (
                      !identityChanged &&
                      !unprovenLegacyIdentity &&
                      current.server_sequence <
                        captureCoordination.latestTrafficSequenceSinceResume
                    )
                      return;
                    const confirmedSameIdentity =
                      !!state.trafficDatabaseEpoch &&
                      current.database_epoch === state.trafficDatabaseEpoch;
                    const rollbackDisproved = confirmedSameIdentity
                      ? current.server_sequence > consumedRecordSequence
                      : current.server_sequence >= previousSequence;
                    if (!identityChanged && rollbackDisproved) {
                      get().handleTrafficStatistics(current);
                      return;
                    }

                    // The new epoch may already contain arrivals. Reopen with no
                    // cursor so initial replay recovers them, even if the old server
                    // subscription filtered them out. Clear the old window first:
                    // its high sequences must not evict the new epoch's low rows.
                    captureCoordination.historyBackfillGeneration += 1;
                    captureCoordination.latestTrafficSequenceSinceResume = 0;
                    clearPendingTrafficBatch();
                    captureCoordination.membershipReconciliationRequired = false;
                    captureCoordination.membershipFollowupRequired = false;
                    captureCoordination.trafficResumeStatisticsPending = false;
                    captureCoordination.trafficEpochResetSuspected = false;
                    set((latest) => ({
                      ...snapshotServerStatistics(current),
                      trafficDatabaseEpoch: current.database_epoch ?? null,
                      records: [],
                      recordsMap: new Map(),
                      currentRecord: null,
                      selectedId: undefined,
                      requestBody: null,
                      responseBody: null,
                      requestRawBody: null,
                      responseRawBody: null,
                      detailLoading: false,
                      detailError: null,
                      serverTotal: current.total_requests,
                      lastId: null,
                      lastSequence: null,
                      oldestSequence: null,
                      serverSequence: current.server_sequence,
                      serverOldestSequence: null,
                      trafficEpochVersion: latest.trafficEpochVersion + 1,
                      pendingIds: new Set(),
                      hasMore: current.total_requests > INITIAL_WINDOW_LIMIT,
                      hasNewer: false,
                      newRecordsCount: 0,
                      loading: false,
                      historyLoading: false,
                      catchingUp: false,
                      filterVersion: latest.filterVersion + 1,
                      recordsMutation: createRecordsMutation({
                        reset: true,
                        inserted: [],
                        updated: [],
                        deletedIds: [],
                      }),
                    }));
                    pushService.resetTrafficCursor();
                    if (!get().usePush && get().polling) {
                      const timeoutId = get().pollTimeoutId;
                      if (timeoutId !== null) window.clearTimeout(timeoutId);
                      set({ pollTimeoutId: null });
                      void get().fetchUpdates();
                    }
                  })
                  .catch(() => {
                    // Preserve the window and retry on the next statistics push.
                  })
                  .finally(() => {
                    if (
                      captureCoordination.trafficEpochRequest?.promise !==
                      request
                    )
                      return;
                    const followup =
                      captureCoordination.trafficEpochRequest.followup;
                    captureCoordination.trafficEpochRequest = null;
                    if (
                      followup &&
                      captureCoordination.trafficEpochResetSuspected &&
                      generation ===
                        captureCoordination.historyBackfillGeneration &&
                      resumeGeneration ===
                        captureCoordination.trafficResumeGeneration
                    ) {
                      get().handleTrafficStatistics(followup);
                    }
                  });
                captureCoordination.trafficEpochRequest = {
                  historyGeneration: generation,
                  resumeGeneration,
                  statisticsSequence: statistics.server_sequence,
                  latestRecordSequence: 0,
                  databaseEpoch: statistics.database_epoch,
                  followup: null,
                  promise: request,
                };
              } else if (
                statistics.server_sequence >
                captureCoordination.trafficEpochRequest.statisticsSequence
              ) {
                // Do not lose a newer snapshot while confirmation is pending: an
                // idle server may never push it again after this read returns.
                captureCoordination.trafficEpochRequest.followup = statistics;
              }
              return;
            }
            captureCoordination.trafficResumeStatisticsPending = false;
            captureCoordination.trafficEpochResetSuspected = false;
            get().handleTrafficPush({
              new_records: [],
              updated_records: [],
              has_more: false,
              server_total: statistics.total_requests,
              server_sequence: statistics.server_sequence,
            });
          }
        },

        fetchInitialData: async () => {
          const state = get();
          if (state.loading || state.initialized) {
            return;
          }

          const generation = ++captureCoordination.historyBackfillGeneration;
          const resumeGeneration = captureCoordination.trafficResumeGeneration;
          captureCoordination.membershipReconciliationRequired = false;
          captureCoordination.membershipFollowupRequired = false;
          captureCoordination.trafficResumeStatisticsPending = false;
          captureCoordination.trafficEpochResetSuspected = false;
          set({ loading: true, error: null });
          try {
            const filter: TrafficUpdatesFilter = {
              limit: INITIAL_WINDOW_LIMIT,
            };
            const response = await api.getTrafficUpdates(filter);
            if (generation !== captureCoordination.historyBackfillGeneration)
              return;
            if (
              resumeGeneration !== captureCoordination.trafficResumeGeneration
            ) {
              set({ loading: false });
              return;
            }
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
              !acceptTrafficDatabaseEpoch(
                response.database_epoch,
                response.server_sequence,
              )
            ) {
              set({ loading: false });
              return;
            }

            const convertedRecords = response.new_records.map(
              compactTrafficSummaryToTrafficSummary,
            );
            const preprocessedRecords =
              preprocessTrafficRecords(convertedRecords);

            const newPendingIds = new Set<string>();
            const newRecordsMap = new Map<string, TrafficSummary>();
            for (const r of preprocessedRecords) {
              newRecordsMap.set(r.id, r);
              if (isPendingRecord(r)) {
                newPendingIds.add(r.id);
              }
            }
            capPendingIds(newPendingIds);

            const boundaries = getBoundaryState(preprocessedRecords);
            set({
              records: preprocessedRecords,
              recordsMap: newRecordsMap,
              serverTotal: response.server_total,
              serverSequence: response.server_sequence,
              hasMore: response.has_more,
              hasNewer: false,
              oldestSequence: boundaries.oldestSequence,
              lastId: boundaries.lastId,
              lastSequence: boundaries.lastSequence,
              pendingIds: newPendingIds,
              loading: false,
              filterVersion: 0,
              initialized: true,
              catchingUp: false,
              recordsMutation: createRecordsMutation({
                reset: true,
                inserted: preprocessedRecords,
                updated: [],
                deletedIds: [],
              }),
            });
            await get().fetchTrafficStatistics();
          } catch (e) {
            if (generation === captureCoordination.historyBackfillGeneration) {
              set({
                loading: false,
                ...(resumeGeneration ===
                captureCoordination.trafficResumeGeneration
                  ? { error: (e as Error).message }
                  : {}),
              });
            }
          }
        },

        backfillHistory: async () => {
          const state = get();
          if (
            state.historyLoading ||
            !state.hasMore ||
            state.oldestSequence === null
          ) {
            return;
          }

          const generation = captureCoordination.historyBackfillGeneration;
          set({ historyLoading: true });

          try {
            const response = await api.getTrafficPage({
              cursor: state.oldestSequence,
              limit: HISTORY_BATCH_LIMIT,
              direction: "backward",
            });

            if (generation !== captureCoordination.historyBackfillGeneration) {
              return;
            }

            const olderRecords = preprocessTrafficRecords(
              response.records
                .map(compactTrafficSummaryToTrafficSummary)
                .reverse(),
            );

            set((prevState) => {
              if (
                generation !== captureCoordination.historyBackfillGeneration
              ) {
                return {};
              }

              const merged = mergeBoundedTrafficWindow(
                prevState.records,
                olderRecords,
                "older",
              );
              const records = merged.records;
              const boundaries = getTrafficWindowBoundaries(records);
              const pendingIds = new Set(
                records.filter(isPendingRecord).map((record) => record.id),
              );
              capPendingIds(pendingIds);

              return {
                records,
                recordsMap: buildTrafficRecordsMap(records),
                serverTotal: response.total,
                serverSequence: Math.max(
                  prevState.serverSequence,
                  response.server_sequence,
                ),
                hasMore: response.has_more && olderRecords.length > 0,
                hasNewer: prevState.hasNewer || merged.trimmedSide === "newer",
                oldestSequence: boundaries.oldestSequence,
                pendingIds,
                recordsMutation: createRecordsMutation({
                  reset: true,
                  inserted: records,
                  updated: [],
                  deletedIds: [],
                }),
              };
            });
          } catch (e) {
            if (generation === captureCoordination.historyBackfillGeneration) {
              set({ error: (e as Error).message });
            }
          } finally {
            if (generation === captureCoordination.historyBackfillGeneration) {
              set({ historyLoading: false });
            }
          }
        },

        loadNewer: async () => {
          const state = get();
          const newestSequence = state.records.at(-1)?.sequence ?? null;
          if (
            state.historyLoading ||
            !state.hasNewer ||
            newestSequence === null
          ) {
            return;
          }

          const generation = captureCoordination.historyBackfillGeneration;
          set({ historyLoading: true });

          try {
            const response = await api.getTrafficPage({
              cursor: newestSequence,
              limit: HISTORY_BATCH_LIMIT,
              direction: "forward",
            });

            if (generation !== captureCoordination.historyBackfillGeneration) {
              return;
            }

            const newerRecords = preprocessTrafficRecords(
              response.records.map(compactTrafficSummaryToTrafficSummary),
            );

            set((prevState) => {
              if (
                generation !== captureCoordination.historyBackfillGeneration
              ) {
                return {};
              }

              const merged = mergeBoundedTrafficWindow(
                prevState.records,
                newerRecords,
                "newer",
              );
              const records = merged.records;
              const boundaries = getTrafficWindowBoundaries(records);
              const pendingIds = new Set(
                records.filter(isPendingRecord).map((record) => record.id),
              );
              capPendingIds(pendingIds);

              return {
                records,
                recordsMap: buildTrafficRecordsMap(records),
                serverTotal: response.total,
                serverSequence: Math.max(
                  prevState.serverSequence,
                  response.server_sequence,
                ),
                hasMore: prevState.hasMore || merged.trimmedSide === "older",
                hasNewer: response.has_more && newerRecords.length > 0,
                oldestSequence: boundaries.oldestSequence,
                pendingIds,
                recordsMutation: createRecordsMutation({
                  reset: true,
                  inserted: records,
                  updated: [],
                  deletedIds: [],
                }),
              };
            });
          } catch (e) {
            if (generation === captureCoordination.historyBackfillGeneration) {
              set({ error: (e as Error).message });
            }
          } finally {
            if (generation === captureCoordination.historyBackfillGeneration) {
              set({ historyLoading: false });
            }
          }
        },

        fetchUpdates: async () => {
          const state = get();
          if (state.paused || !state.polling || state.catchingUp) return;
          const generation = captureCoordination.historyBackfillGeneration;
          const resumeGeneration = captureCoordination.trafficResumeGeneration;
          let nextDelay = POLL_INTERVAL;
          try {
            const pendingIdsArray = Array.from(state.pendingIds);
            const response = await api.getTrafficUpdates({
              after_id: state.lastId || undefined,
              after_seq: state.lastSequence || undefined,
              pending_ids:
                pendingIdsArray.length > 0
                  ? pendingIdsArray.join(",")
                  : undefined,
              limit: UPDATE_BATCH_LIMIT,
            });
            if (
              generation !== captureCoordination.historyBackfillGeneration ||
              resumeGeneration !== captureCoordination.trafficResumeGeneration
            )
              return;
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
              !acceptTrafficDatabaseEpoch(
                response.database_epoch,
                response.server_sequence,
              )
            )
              return;

            get().handleTrafficDelta({
              database_epoch: response.database_epoch,
              inserts: response.new_records,
              updates: response.updated_records,
              has_more: response.has_more,
              server_total: response.server_total,
              server_sequence: response.server_sequence,
            });
            captureCoordination.hasMoreBurst = response.has_more
              ? captureCoordination.hasMoreBurst + 1
              : 0;
            nextDelay = response.has_more
              ? captureCoordination.hasMoreBurst > HAS_MORE_BURST_LIMIT
                ? HAS_MORE_BACKOFF_INTERVAL
                : POLL_MIN_INTERVAL
              : POLL_INTERVAL;
          } catch (e) {
            if (generation === captureCoordination.historyBackfillGeneration)
              set({ error: (e as Error).message });
          } finally {
            // A failed identity confirmation must not terminate polling. A reset
            // starts its own cursor-free poll; the prior generation cannot reschedule.
            if (
              generation === captureCoordination.historyBackfillGeneration &&
              get().polling
            ) {
              const timeoutId = window.setTimeout(() => {
                get().fetchUpdates();
              }, nextDelay);
              set({ pollTimeoutId: timeoutId });
            }
          }
        },

        catchUpUpdates: async () => {
          const state = get();
          if (state.paused || !state.polling || state.catchingUp) return;
          const generation = captureCoordination.historyBackfillGeneration;
          const resumeGeneration = captureCoordination.trafficResumeGeneration;

          set({ catchingUp: true });
          try {
            const pendingIdsArray = Array.from(get().pendingIds);
            const filter: TrafficUpdatesFilter = {
              after_id: get().lastId || undefined,
              after_seq: get().lastSequence || undefined,
              pending_ids:
                pendingIdsArray.length > 0
                  ? pendingIdsArray.join(",")
                  : undefined,
              limit: UPDATE_BATCH_LIMIT,
            };
            const response = await api.getTrafficUpdates(filter);
            if (
              generation !== captureCoordination.historyBackfillGeneration ||
              resumeGeneration !== captureCoordination.trafficResumeGeneration
            )
              return;
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
              !acceptTrafficDatabaseEpoch(
                response.database_epoch,
                response.server_sequence,
              )
            )
              return;

            set((prev) => ({
              serverTotal: response.server_total,
              serverSequence: response.server_sequence ?? prev.serverSequence,
            }));

            if (
              response.new_records.length > 0 ||
              response.updated_records.length > 0
            ) {
              get().handleTrafficDelta({
                database_epoch: response.database_epoch,
                inserts: response.new_records,
                updates: response.updated_records,
                has_more: response.has_more,
                server_total: response.server_total,
                server_sequence: response.server_sequence,
              });
            }
          } catch (e) {
            if (generation !== captureCoordination.historyBackfillGeneration)
              return;
            set({ error: (e as Error).message });
          } finally {
            if (generation === captureCoordination.historyBackfillGeneration)
              set({ catchingUp: false });
          }

          const afterState = get();
          if (
            afterState.records.length > 0 &&
            afterState.serverTotal < afterState.records.length
          ) {
            await get().reloadRecords();
          }
        },

        ...createTrafficReconciliation({ get, set }, coordination),

        fetchTrafficDetail: async (id: string) => {
          const generation = captureCoordination.historyBackfillGeneration;
          const prevState = get();
          if (prevState.selectedId !== id) return;
          const preserveBodies = prevState.currentRecord?.id === id;
          set({
            detailLoading: true,
            detailError: null,
            requestBody: preserveBodies ? prevState.requestBody : null,
            responseBody: preserveBodies ? prevState.responseBody : null,
            requestRawBody: preserveBodies ? prevState.requestRawBody : null,
            responseRawBody: preserveBodies ? prevState.responseRawBody : null,
          });
          try {
            const record = await api.getTrafficDetail(id);
            if (
              generation !== captureCoordination.historyBackfillGeneration ||
              get().selectedId !== id
            )
              return;
            const summary = get().recordsMap.get(id);
            const mergedRecord = mergeDetailWithSummary(record, summary);
            set({
              currentRecord: mergedRecord,
              detailLoading: false,
              detailError: null,
            });

            api
              .getRequestBody(id)
              .then((body) => {
                if (
                  generation ===
                    captureCoordination.historyBackfillGeneration &&
                  get().selectedId === id &&
                  get().currentRecord?.id === id
                )
                  set({ requestBody: body });
              })
              .catch(() => {});

            if (mergedRecord.raw_request_body_ref) {
              api
                .getRequestBodyContent(id, { raw: true, encoding: "base64" })
                .then((body) => {
                  if (
                    generation ===
                      captureCoordination.historyBackfillGeneration &&
                    get().selectedId === id &&
                    get().currentRecord?.id === id
                  )
                    set({ requestRawBody: body });
                })
                .catch(() => {});
            }

            const isOpenSse =
              !!mergedRecord.is_sse && !!mergedRecord.socket_status?.is_open;
            if (!isOpenSse) {
              api
                .getResponseBody(id)
                .then((body) => {
                  if (
                    generation ===
                      captureCoordination.historyBackfillGeneration &&
                    get().selectedId === id &&
                    get().currentRecord?.id === id
                  )
                    set({ responseBody: body });
                })
                .catch(() => {});
              if (mergedRecord.raw_response_body_ref) {
                api
                  .getResponseBodyContent(id, { raw: true, encoding: "base64" })
                  .then((body) => {
                    if (
                      generation ===
                        captureCoordination.historyBackfillGeneration &&
                      get().selectedId === id &&
                      get().currentRecord?.id === id
                    )
                      set({ responseRawBody: body });
                  })
                  .catch(() => {});
              }
            }
          } catch (e) {
            if (
              generation !== captureCoordination.historyBackfillGeneration ||
              get().selectedId !== id
            )
              return;
            const error = e as {
              response?: { data?: { error?: string } };
              message?: string;
            };
            const message =
              error.response?.data?.error ||
              error.message ||
              t("shared.traffic.detailNotFound");
            set({
              currentRecord: null,
              requestBody: null,
              responseBody: null,
              requestRawBody: null,
              responseRawBody: null,
              detailError: message,
              detailLoading: false,
            });
          }
        },

        appendSseResponseBody: (recordId: string, payload: string) => {
          set((state) => {
            if (!payload) return {};
            if (state.currentRecord?.id !== recordId) return {};
            const prev = state.responseBody || "";
            if (prev.length >= SSE_RESPONSE_BODY_CHAR_LIMIT) return {};
            let next = mergeSseBody(prev, payload);
            if (next.length > SSE_RESPONSE_BODY_CHAR_LIMIT) {
              next = `${next.slice(0, SSE_RESPONSE_BODY_CHAR_LIMIT)}\n\n... (truncated)`;
            }
            return { responseBody: next };
          });
        },

        setResponseBody: (recordId: string, body: string | null) => {
          set((state) => {
            if (state.currentRecord?.id !== recordId) return {};
            return { responseBody: body };
          });
        },

        setToolbarFilters: (filters: ToolbarFilters) => {
          set((state) => ({
            toolbarFilters: filters,
            filterVersion: state.filterVersion + 1,
          }));
        },

        setFilterConditions: (conditions: FilterCondition[]) => {
          set((state) => ({
            filterConditions: conditions,
            filterVersion: state.filterVersion + 1,
          }));
        },

        setPaused: (paused: boolean) => {
          set({ paused });
          if (paused) {
            get().stopPolling();
          } else {
            get().startPolling();
          }
        },

        setAutoScroll: (autoScroll: boolean) => {
          set({ autoScroll });
          if (autoScroll) {
            set({ newRecordsCount: 0 });
          }
        },

        clearNewRecordsCount: () => set({ newRecordsCount: 0 }),

        clearError: () => set({ error: null }),

        clearCurrentRecord: () =>
          set({
            currentRecord: null,
            requestBody: null,
            responseBody: null,
            requestRawBody: null,
            responseRawBody: null,
            detailError: null,
          }),

        initFromUrl: (
          filters: FilterCondition[],
          toolbar: ToolbarFilters | null,
        ) => {
          set({
            filterConditions: filters,
            toolbarFilters: toolbar || {
              rule: [],
              protocol: [],
              type: [],
              status: [],
              imported: [],
            },
          });
        },

        setScrollTop: (scrollTop: number) => set({ scrollTop }),

        setSelectedId: (id: string | undefined) => set({ selectedId: id }),
      };
    },
    {
      name: "bifrost-traffic-ui",
      partialize: (state) => ({
        toolbarFilters: state.toolbarFilters,
        filterConditions: state.filterConditions,
        autoScroll: state.autoScroll,
        useDbMode: state.useDbMode,
      }),
      version: 2,
    },
  ),
);

let isApplyingExternalSelectedId = false;
let trafficSelectionSyncInitialized = false;

function applyExternalSelectedId(id: string | undefined) {
  isApplyingExternalSelectedId = true;
  useTrafficStore.setState({ selectedId: id });
  isApplyingExternalSelectedId = false;
}

function initializeTrafficSelectionSync() {
  if (trafficSelectionSyncInitialized || typeof window === "undefined") {
    return;
  }
  trafficSelectionSyncInitialized = true;

  useTrafficStore.subscribe((state, prevState) => {
    if (
      isApplyingExternalSelectedId ||
      state.selectedId === prevState.selectedId
    ) {
      return;
    }

    trafficSelectionSyncChannel?.postMessage({
      type: "selected-id",
      id: state.selectedId,
    });
  });

  trafficSelectionSyncChannel?.addEventListener(
    "message",
    (event: MessageEvent) => {
      const data = event.data as { type?: string; id?: string } | undefined;
      if (data?.type !== "selected-id") {
        return;
      }
      applyExternalSelectedId(data.id);
    },
  );
}

initializeTrafficSelectionSync();
