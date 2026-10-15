import type {
  TrafficSummary,
  TrafficRecord,
  ToolbarFilters,
  FilterCondition,
  TrafficDeltaData,
  TrafficStatistics,
} from "../types";
import type { TrafficBodyContent } from "../api";
import type { TrafficUpdatesData } from "../services/pushService";
import type { ClearReconciliationEvidence } from "./trafficCaptureCoordinator";
export interface TrafficRecordsMutation {
  version: number;
  reset: boolean;
  inserted: TrafficSummary[];
  updated: TrafficSummary[];
  deletedIds: string[];
  oldestSequenceFloor?: number | null;
  incomplete?: boolean;
}

export interface TrafficState {
  records: TrafficSummary[];
  recordsMap: Map<string, TrafficSummary>;
  currentRecord: TrafficRecord | null;
  requestBody: string | null;
  responseBody: string | null;
  requestRawBody: TrafficBodyContent | null;
  responseRawBody: TrafficBodyContent | null;
  serverTotal: number;
  serverSequence: number;
  serverOldestSequence: number | null;
  trafficEpochVersion: number;
  trafficDatabaseEpoch: string | null;
  hasMore: boolean;
  hasNewer: boolean;
  oldestSequence: number | null;
  lastId: string | null;
  lastSequence: number | null;
  pendingIds: Set<string>;
  toolbarFilters: ToolbarFilters;
  filterConditions: FilterCondition[];
  paused: boolean;
  loading: boolean;
  detailLoading: boolean;
  polling: boolean;
  error: string | null;
  detailError: string | null;
  pollTimeoutId: number | null;
  autoScroll: boolean;
  newRecordsCount: number;
  scrollTop: number;
  usePush: boolean;
  pushUnsubscribe: (() => void) | null;
  pushDeltaUnsubscribe: (() => void) | null;
  pushDeletedUnsubscribe: (() => void) | null;
  pushStatisticsUnsubscribe: (() => void) | null;
  pushConnectionUnsubscribe: (() => void) | null;
  filterVersion: number;
  initialized: boolean;
  selectedId: string | undefined;
  useDbMode: boolean;
  historyLoading: boolean;
  catchingUp: boolean;
  availableClientApps: string[];
  availableAccountNames: string[];
  availableClientIps: string[];
  availableProxyPorts: string[];
  availableDomains: string[];
  clientAppCounts: Map<string, number>;
  accountNameCounts: Map<string, number>;
  clientIpCounts: Map<string, number>;
  proxyPortCounts: Map<string, number>;
  domainCounts: Map<string, number>;
  trafficStatisticsTotal: number;
  trafficStatisticsSequence: number;
  trafficStatisticsLoaded: boolean;
  recordsMutation: TrafficRecordsMutation;

  startPolling: () => void;
  stopPolling: () => void;
  fetchUpdates: () => Promise<void>;
  fetchInitialData: () => Promise<void>;
  fetchTrafficStatistics: (
    expectedEpoch?: string,
    reconcileTotal?: boolean,
  ) => Promise<void>;
  backfillHistory: () => Promise<void>;
  loadNewer: () => Promise<void>;
  catchUpUpdates: () => Promise<void>;
  reloadRecords: (
    preserveLiveRecords?: boolean,
    clearEvidence?: ClearReconciliationEvidence,
  ) => Promise<void>;
  fetchTrafficDetail: (id: string) => Promise<void>;
  appendSseResponseBody: (recordId: string, payload: string) => void;
  setResponseBody: (recordId: string, body: string | null) => void;
  clearing: boolean;
  clearTraffic: (ids?: string[]) => Promise<boolean>;
  setToolbarFilters: (filters: ToolbarFilters) => void;
  setFilterConditions: (conditions: FilterCondition[]) => void;
  setPaused: (paused: boolean) => void;
  setAutoScroll: (autoScroll: boolean) => void;
  clearNewRecordsCount: () => void;
  clearError: () => void;
  clearCurrentRecord: () => void;
  initFromUrl: (
    filters: FilterCondition[],
    toolbar: ToolbarFilters | null,
  ) => void;
  setScrollTop: (scrollTop: number) => void;
  setSelectedId: (id: string | undefined) => void;
  handleTrafficPush: (data: TrafficUpdatesData) => void;
  handleTrafficDelta: (data: TrafficDeltaData) => void;
  handleTrafficDeleted: (
    ids: string[],
    authoritativeTotal?: number,
    confirmed?: boolean,
  ) => void;
  reconcileTrafficWindow: () => Promise<void>;
  handleTrafficStatistics: (
    statistics: TrafficStatistics,
    reconcileTotal?: boolean,
  ) => void;
  enablePush: () => void;
  disablePush: () => void;
}

export interface TrafficStoreAccess {
  get: () => TrafficState;
  set: (
    partial:
      | Partial<TrafficState>
      | ((state: TrafficState) => Partial<TrafficState>),
  ) => void;
}
