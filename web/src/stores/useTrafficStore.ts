import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { TrafficSummary, TrafficRecord, ToolbarFilters, FilterCondition, TrafficUpdatesFilter, TrafficSummaryCompact, TrafficDeltaData } from '../types';
import * as api from '../api';
import type { TrafficBodyContent } from '../api';
import pushService, { type TrafficUpdatesData } from '../services/pushService';

export interface TrafficRecordsMutation {
  version: number;
  reset: boolean;
  inserted: TrafficSummary[];
  updated: TrafficSummary[];
  deletedIds: string[];
}

interface TrafficState {
  records: TrafficSummary[];
  recordsMap: Map<string, TrafficSummary>;
  currentRecord: TrafficRecord | null;
  requestBody: string | null;
  responseBody: string | null;
  requestRawBody: TrafficBodyContent | null;
  responseRawBody: TrafficBodyContent | null;
  serverTotal: number;
  serverSequence: number;
  hasMore: boolean;
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
  filterVersion: number;
  initialized: boolean;
  selectedId: string | undefined;
  useDbMode: boolean;
  historyLoading: boolean;
  catchingUp: boolean;
  availableClientApps: string[];
  availableClientIps: string[];
  availableDomains: string[];
  clientAppCounts: Map<string, number>;
  clientIpCounts: Map<string, number>;
  domainCounts: Map<string, number>;
  recordsMutation: TrafficRecordsMutation;

  startPolling: () => void;
  stopPolling: () => void;
  fetchUpdates: () => Promise<void>;
  fetchInitialData: () => Promise<void>;
  backfillHistory: () => Promise<void>;
  catchUpUpdates: () => Promise<void>;
  reloadRecords: () => Promise<void>;
  fetchTrafficDetail: (id: string) => Promise<void>;
  appendSseResponseBody: (recordId: string, payload: string) => void;
  setResponseBody: (recordId: string, body: string | null) => void;
  clearTraffic: (ids?: string[]) => Promise<boolean>;
  setToolbarFilters: (filters: ToolbarFilters) => void;
  setFilterConditions: (conditions: FilterCondition[]) => void;
  setPaused: (paused: boolean) => void;
  setAutoScroll: (autoScroll: boolean) => void;
  clearNewRecordsCount: () => void;
  clearError: () => void;
  clearCurrentRecord: () => void;
  initFromUrl: (filters: FilterCondition[], toolbar: ToolbarFilters | null) => void;
  setScrollTop: (scrollTop: number) => void;
  setSelectedId: (id: string | undefined) => void;
  handleTrafficPush: (data: TrafficUpdatesData) => void;
  handleTrafficDelta: (data: TrafficDeltaData) => void;
  handleTrafficDeleted: (ids: string[]) => void;
  enablePush: () => void;
  disablePush: () => void;
}

const POLL_INTERVAL = 1000;
const POLL_MIN_INTERVAL = 200;
const HAS_MORE_BURST_LIMIT = 3;
const HAS_MORE_BACKOFF_INTERVAL = 500;
const INITIAL_WINDOW_LIMIT = 500;
const HISTORY_BATCH_LIMIT = 500;
const UPDATE_BATCH_LIMIT = 1000;
const UPDATE_THROTTLE_MS = 100;
const MAX_PENDING_IDS = 500;

interface BatchedUpdate {
  newRecords: TrafficSummary[];
  updatedRecords: TrafficSummary[];
  serverTotal: number;
  hasMore: boolean;
}

let pendingBatch: BatchedUpdate | null = null;
let rafId: number | null = null;
let lastUpdateTime = 0;
let hasMoreBurst = 0;
let historyBackfillGeneration = 0;
let historyRetryTimerId: number | null = null;
let historyRetryDelayMs = 1000;
let recordsMutationVersion = 0;
const TRAFFIC_SELECTION_SYNC_CHANNEL = 'bifrost-traffic-selection-sync';
const trafficSelectionSyncChannel =
  typeof BroadcastChannel !== 'undefined'
    ? new BroadcastChannel(TRAFFIC_SELECTION_SYNC_CHANNEL)
    : null;

function capPendingIds(ids: Set<string>) {
  while (ids.size > MAX_PENDING_IDS) {
    const first = ids.values().next().value as string | undefined;
    if (!first) break;
    ids.delete(first);
  }
}

const contentTypeMap: Record<string, string[]> = {
  'JSON': ['json', 'application/json'],
  'Form': ['form', 'x-www-form-urlencoded', 'multipart/form-data'],
  'XML': ['xml', 'application/xml', 'text/xml'],
  'JS': ['javascript', 'text/javascript', 'application/javascript'],
  'CSS': ['css', 'text/css'],
  'Font': ['font', 'woff', 'woff2', 'ttf', 'otf', 'eot'],
  'Doc': ['html', 'text/html'],
  'Media': ['image', 'video', 'audio', 'png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'mp4', 'webm', 'mp3', 'wav'],
  'SSE': ['event-stream', 'text/event-stream'],
};

const METHOD_COLORS: Record<string, string> = {
  GET: "green",
  POST: "blue",
  PUT: "orange",
  DELETE: "red",
  PATCH: "purple",
  OPTIONS: "default",
  HEAD: "cyan",
  CONNECT: "magenta",
};

const STATUS_DOT_COLORS: Record<string, string> = {
  pending: "#d9d9d9",
  info: "#73d13d",
  success: "#52c41a",
  redirect: "#faad14",
  clientError: "#fa8c16",
  serverError: "#f5222d",
};

const getStatusDotColor = (status: number): string => {
  if (status === 0) return STATUS_DOT_COLORS.pending;
  if (status >= 100 && status < 200) return STATUS_DOT_COLORS.info;
  if (status >= 200 && status < 300) return STATUS_DOT_COLORS.success;
  if (status >= 300 && status < 400) return STATUS_DOT_COLORS.redirect;
  if (status >= 400 && status < 500) return STATUS_DOT_COLORS.clientError;
  if (status >= 500) return STATUS_DOT_COLORS.serverError;
  return STATUS_DOT_COLORS.pending;
};

const getStatusColor = (status: number): string => {
  if (status >= 500) return "error";
  if (status >= 400) return "warning";
  if (status >= 300) return "processing";
  if (status >= 200) return "success";
  return "default";
};

const formatSize = (bytes: number): string => {
  if (bytes === 0) return "-";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
};

const mergeSseBody = (prev: string | null, payload: string): string => {
  const trimmed = payload.replace(/\n+$/, '');
  if (!trimmed) return prev || '';
  if (!prev || prev.length === 0) return trimmed;
  if (prev.endsWith('\n\n')) return `${prev}${trimmed}`;
  if (prev.endsWith('\n')) return `${prev}\n${trimmed}`;
  return `${prev}\n\n${trimmed}`;
};

const SSE_RESPONSE_BODY_CHAR_LIMIT = 2_000_000;

const getDisplaySizeBytes = (record: TrafficSummary | undefined): number => {
  if (!record) return 0;
  if (record.socket_status && (record.response_size === 0 || record.socket_status.is_open)) {
    return record.socket_status.send_bytes + record.socket_status.receive_bytes;
  }
  return record.response_size;
};

const isPendingRecord = (record: TrafficSummary): boolean => {
  return record.status === 0 || record.socket_status?.is_open === true;
};

const shouldReplaceRecord = (
  existing: TrafficSummary | undefined,
  next: TrafficSummary,
): boolean => {
  if (!existing) return true;

  return (
    existing.client_app !== next.client_app ||
    existing.client_pid !== next.client_pid ||
    existing.has_rule_hit !== next.has_rule_hit ||
    existing.matched_rule_count !== next.matched_rule_count ||
    existing.matched_protocols.join('|') !== next.matched_protocols.join('|') ||
    existing.status !== next.status ||
    existing.request_size !== next.request_size ||
    existing.response_size !== next.response_size ||
    existing.duration_ms !== next.duration_ms ||
    existing.frame_count !== next.frame_count ||
    existing.content_type !== next.content_type ||
    existing.socket_status?.is_open !== next.socket_status?.is_open ||
    existing.socket_status?.send_bytes !== next.socket_status?.send_bytes ||
    existing.socket_status?.receive_bytes !== next.socket_status?.receive_bytes ||
    existing.socket_status?.frame_count !== next.socket_status?.frame_count ||
    getDisplaySizeBytes(existing) !== getDisplaySizeBytes(next)
  );
};

const mergeIncrementalRecord = (
  existing: TrafficSummary | undefined,
  next: TrafficSummary,
): TrafficSummary => {
  if (!existing) {
    return next;
  }

  // Keep sticky identity fields when incremental updates arrive without them.
  // This avoids filtered rows disappearing until a full refresh reconstructs them.
  const merged: TrafficSummary = {
    ...next,
    method: next.method || existing.method,
    host: next.host || existing.host,
    path: next.path || existing.path,
    protocol: next.protocol || existing.protocol,
    client_ip: next.client_ip || existing.client_ip,
    client_app: next.client_app ?? existing.client_app,
    client_pid: next.client_pid ?? existing.client_pid,
  };

  return preprocessRecord(merged);
};

const preprocessRecord = (record: TrafficSummary): TrafficSummary => {
  const isH3 = record.is_h3 || record.protocol === 'h3' || record.protocol === 'h3s';
  const displayProtocol = isH3
    ? 'H3'
    : record.protocol?.replace("HTTP/", "").toUpperCase() || "-";

  const methodColor = METHOD_COLORS[record.method?.toUpperCase()] || "default";
  const statusColor = getStatusColor(record.status);
  const statusDotColor = getStatusDotColor(record.status);

  const size = getDisplaySizeBytes(record);
  const displaySize = formatSize(size);

  const contentTypeShort = record.content_type?.split(";")[0]?.split("/").pop() || "-";

  const clientApp = record.client_app || "";
  const clientIp = record.client_ip || "";
  const hasApp = Boolean(clientApp);
  const clientDisplay = clientApp || clientIp || "-";
  const clientTooltip = hasApp
    ? `${clientApp} (PID: ${record.client_pid || "?"}, IP: ${clientIp || "?"})`
    : clientIp || "-";

  record._displayProtocol = displayProtocol;
  record._methodColor = methodColor;
  record._statusColor = statusColor;
  record._statusDotColor = statusDotColor;
  record._displaySize = displaySize;
  record._contentTypeShort = contentTypeShort;
  record._clientDisplay = clientDisplay;
  record._clientTooltip = clientTooltip;

  return record;
};

const preprocessRecords = (records: TrafficSummary[]): TrafficSummary[] => {
  for (let i = 0; i < records.length; i++) {
    preprocessRecord(records[i]);
  }
  return records;
};

const mergeDetailWithSummary = (
  detail: TrafficRecord,
  summary?: TrafficSummary,
): TrafficRecord => {
  if (!summary || summary.id !== detail.id) return detail;

  return {
    ...detail,
    request_size: summary.request_size,
    response_size: summary.response_size,
    duration_ms: summary.duration_ms,
    frame_count: summary.frame_count,
    socket_status: summary.socket_status ?? detail.socket_status,
    end_time: summary.end_time ?? detail.end_time,
  };
};

export const compareTrafficRecordsBySequence = (left: TrafficSummary, right: TrafficSummary): number => {
  if (left.sequence !== right.sequence) {
    return left.sequence - right.sequence;
  }
  if (left.timestamp !== right.timestamp) {
    return left.timestamp - right.timestamp;
  }
  return left.id.localeCompare(right.id);
};

export const mergeSortedTrafficRecords = (
  current: TrafficSummary[],
  incoming: TrafficSummary[],
): TrafficSummary[] => {
  if (incoming.length === 0) return current;
  if (current.length === 0) return incoming.slice();

  const incomingFirst = incoming[0]!;
  const incomingLast = incoming[incoming.length - 1]!;
  const currentFirst = current[0]!;
  const currentLast = current[current.length - 1]!;

  if (compareTrafficRecordsBySequence(incomingLast, currentFirst) < 0) {
    return [...incoming, ...current];
  }
  if (compareTrafficRecordsBySequence(incomingFirst, currentLast) > 0) {
    return [...current, ...incoming];
  }

  const merged: TrafficSummary[] = [];
  let leftIndex = 0;
  let rightIndex = 0;

  while (leftIndex < current.length && rightIndex < incoming.length) {
    const left = current[leftIndex]!;
    const right = incoming[rightIndex]!;
    if (compareTrafficRecordsBySequence(left, right) <= 0) {
      merged.push(left);
      leftIndex += 1;
    } else {
      merged.push(right);
      rightIndex += 1;
    }
  }

  if (leftIndex < current.length) {
    merged.push(...current.slice(leftIndex));
  }
  if (rightIndex < incoming.length) {
    merged.push(...incoming.slice(rightIndex));
  }

  return merged;
};

const mergeNewRecordsIntoList = (
  current: TrafficSummary[],
  incoming: TrafficSummary[],
): TrafficSummary[] => mergeSortedTrafficRecords(current, incoming);

export const replaceUpdatedTrafficRecordsInList = (
  current: TrafficSummary[],
  updatedRecords: TrafficSummary[],
): TrafficSummary[] => {
  if (updatedRecords.length === 0 || current.length === 0) {
    return current;
  }

  const updatedById = new Map(updatedRecords.map((record) => [record.id, record]));
  let changed = false;
  const next = current.map((record) => {
    const updated = updatedById.get(record.id);
    if (!updated) {
      return record;
    }
    changed = true;
    return updated;
  });

  return changed ? next : current;
};

const findLastRecordById = (
  records: TrafficSummary[],
  id: string,
): TrafficSummary | undefined => {
  for (let i = records.length - 1; i >= 0; i -= 1) {
    const record = records[i];
    if (record?.id === id) {
      return record;
    }
  }
  return undefined;
};

const hasRecordInList = (
  records: TrafficSummary[],
  id: string,
): boolean => {
  for (let i = 0; i < records.length; i += 1) {
    if (records[i]?.id === id) {
      return true;
    }
  }
  return false;
};

const getBoundaryState = (records: TrafficSummary[]) => {
  const oldestRecord = records[0];
  const latestRecord = records[records.length - 1];
  return {
    oldestSequence: oldestRecord?.sequence ?? null,
    lastSequence: latestRecord?.sequence ?? null,
    lastId: latestRecord?.id ?? null,
  };
};

const createRecordsMutation = (
  mutation: Omit<TrafficRecordsMutation, 'version'>,
): TrafficRecordsMutation => ({
  version: ++recordsMutationVersion,
  ...mutation,
});

const createEmptyRecordsMutation = (): TrafficRecordsMutation => ({
  version: recordsMutationVersion,
  reset: false,
  inserted: [],
  updated: [],
  deletedIds: [],
});

const clearHistoryRetryTimer = () => {
  if (historyRetryTimerId !== null) {
    window.clearTimeout(historyRetryTimerId);
    historyRetryTimerId = null;
  }
};

const resetHistoryRetryState = () => {
  clearHistoryRetryTimer();
  historyRetryDelayMs = 1000;
};

const incrementCount = (counts: Map<string, number>, value: string | null | undefined) => {
  if (!value) {
    return;
  }
  counts.set(value, (counts.get(value) || 0) + 1);
};

const decrementCount = (counts: Map<string, number>, value: string | null | undefined) => {
  if (!value) {
    return;
  }
  const next = (counts.get(value) || 0) - 1;
  if (next > 0) {
    counts.set(value, next);
  } else {
    counts.delete(value);
  }
};

const buildSortedKeys = (counts: Map<string, number>): string[] => (
  Array.from(counts.keys()).sort()
);

const buildClientCatalog = (records: TrafficSummary[]) => {
  const clientAppCounts = new Map<string, number>();
  const clientIpCounts = new Map<string, number>();
  const domainCounts = new Map<string, number>();

  for (const record of records) {
    incrementCount(clientAppCounts, record.client_app || null);
    incrementCount(clientIpCounts, record.client_ip || null);
    incrementCount(domainCounts, record.host || null);
  }

  return {
    clientAppCounts,
    clientIpCounts,
    domainCounts,
    availableClientApps: buildSortedKeys(clientAppCounts),
    availableClientIps: buildSortedKeys(clientIpCounts),
    availableDomains: buildSortedKeys(domainCounts),
  };
};

const cloneClientCatalog = (
  state: Pick<
    TrafficState,
    'clientAppCounts' | 'clientIpCounts' | 'domainCounts'
  >,
) => ({
  clientAppCounts: new Map(state.clientAppCounts),
  clientIpCounts: new Map(state.clientIpCounts),
  domainCounts: new Map(state.domainCounts),
});

const snapshotClientCatalog = (
  catalog: ReturnType<typeof cloneClientCatalog>,
) => ({
  clientAppCounts: catalog.clientAppCounts,
  clientIpCounts: catalog.clientIpCounts,
  domainCounts: catalog.domainCounts,
  availableClientApps: buildSortedKeys(catalog.clientAppCounts),
  availableClientIps: buildSortedKeys(catalog.clientIpCounts),
  availableDomains: buildSortedKeys(catalog.domainCounts),
});

const addRecordToClientCatalog = (
  catalog: ReturnType<typeof cloneClientCatalog>,
  record: TrafficSummary,
) => {
  incrementCount(catalog.clientAppCounts, record.client_app || null);
  incrementCount(catalog.clientIpCounts, record.client_ip || null);
  incrementCount(catalog.domainCounts, record.host || null);
};

const removeRecordFromClientCatalog = (
  catalog: ReturnType<typeof cloneClientCatalog>,
  record: TrafficSummary,
) => {
  decrementCount(catalog.clientAppCounts, record.client_app || null);
  decrementCount(catalog.clientIpCounts, record.client_ip || null);
  decrementCount(catalog.domainCounts, record.host || null);
};

const replaceRecordInClientCatalog = (
  catalog: ReturnType<typeof cloneClientCatalog>,
  previous: TrafficSummary | undefined,
  next: TrafficSummary,
) => {
  if (previous) {
    removeRecordFromClientCatalog(catalog, previous);
  }
  addRecordToClientCatalog(catalog, next);
};
export const isFilterConditionApplicable = (
  condition: FilterCondition,
): boolean => {
  return (
    condition.enabled !== false &&
    (condition.operator === 'is_empty' ||
      condition.operator === 'is_not_empty' ||
      condition.value.trim().length > 0)
  );
};

const hasActiveFilters = (toolbar: ToolbarFilters, conditions: FilterCondition[]): boolean => {
  return toolbar.rule.length > 0 ||
    toolbar.protocol.length > 0 ||
    toolbar.status.length > 0 ||
    toolbar.type.length > 0 ||
    toolbar.imported.length > 0 ||
    conditions.some(isFilterConditionApplicable);
};

interface CompiledCondition {
  field: string;
  operator: string;
  valueLower: string;
  regex: RegExp | null;
}

const compileConditions = (conditions: FilterCondition[]): CompiledCondition[] => {
  return conditions
    .filter(isFilterConditionApplicable)
    .map(c => {
      let regex: RegExp | null = null;
      if (c.operator === 'regex') {
        try {
          regex = new RegExp(c.value, 'i');
        } catch {
          regex = null;
        }
      }
      return {
        field: c.field,
        operator: c.operator,
        valueLower: c.value.toLowerCase(),
        regex,
      };
    });
};

const matchRecord = (
  record: TrafficSummary,
  toolbar: ToolbarFilters,
  compiledConditions: CompiledCondition[],
  protocolSet: Set<string>,
  statusSet: Set<string>,
  typeSet: Set<string>
): boolean => {
  if (toolbar.rule.length > 0 && !record.has_rule_hit) {
    return false;
  }

  if (toolbar.imported.length > 0) {
    const isImported = record.id.startsWith('OUT-') || record.client_app === 'Bifrost Import';
    if (!isImported) {
      return false;
    }
  }

  if (protocolSet.size > 0) {
    const protocol = record.protocol?.toUpperCase() || '';
    let matched = false;
    if (protocolSet.has('H2') && protocol.includes('HTTP/2')) matched = true;
    else if (protocolSet.has('HTTP') && (protocol === 'HTTP/1.0' || protocol === 'HTTP/1.1')) matched = true;
    else if (protocolSet.has('HTTPS') && protocol === 'HTTPS') matched = true;
    else if (protocolSet.has('WS') && record.is_websocket && protocol === 'WS') matched = true;
    else if (protocolSet.has('WSS') && record.is_websocket && protocol === 'WSS') matched = true;
    else if (protocolSet.has('H3') && (record.is_h3 || protocol === 'H3')) matched = true;
    if (!matched) return false;
  }

  if (statusSet.size > 0) {
    const status = record.status;
    let matched = false;
    if (statusSet.has('error') && (status === 0 || status >= 500)) matched = true;
    else if (statusSet.has('1xx') && status >= 100 && status < 200) matched = true;
    else if (statusSet.has('2xx') && status >= 200 && status < 300) matched = true;
    else if (statusSet.has('3xx') && status >= 300 && status < 400) matched = true;
    else if (statusSet.has('4xx') && status >= 400 && status < 500) matched = true;
    else if (statusSet.has('5xx') && status >= 500 && status < 600) matched = true;
    if (!matched) return false;
  }

  if (typeSet.size > 0) {
    const resContentType = (record.content_type || '').toLowerCase();
    const reqContentType = (record.request_content_type || '').toLowerCase();
    let matched = false;
    for (const t of typeSet) {
      const patterns = contentTypeMap[t] || [t.toLowerCase()];
      if (patterns.some(pattern => resContentType.includes(pattern) || reqContentType.includes(pattern))) {
        matched = true;
        break;
      }
    }
    if (!matched) return false;
  }

  for (const cond of compiledConditions) {
    let fieldValue = '';
    switch (cond.field) {
      case 'url':
        fieldValue = `${record.host || ''}${record.path || ''}`;
        break;
      case 'host':
        fieldValue = record.host || '';
        break;
      case 'path':
        fieldValue = record.path || '';
        break;
      case 'method':
        fieldValue = record.method || '';
        break;
      case 'content_type':
        fieldValue = record.content_type || '';
        break;
      case 'client_app':
        fieldValue = record.client_app || '';
        break;
      case 'client_ip':
        fieldValue = record.client_ip || '';
        break;
      case 'listener_port':
      case 'port':
        fieldValue = record.listener_port ? String(record.listener_port) : '';
        break;
      default:
        continue;
    }

    const fieldValueLower = fieldValue.toLowerCase();
    let matched = false;

    switch (cond.operator) {
      case 'contains':
        matched = fieldValueLower.includes(cond.valueLower);
        break;
      case 'equals':
        matched = fieldValueLower === cond.valueLower;
        break;
      case 'regex':
        matched = cond.regex ? cond.regex.test(fieldValue) : false;
        break;
      case 'not_contains':
        matched = !fieldValueLower.includes(cond.valueLower);
        break;
      case 'is_empty':
        matched = fieldValue.trim().length === 0;
        break;
      case 'is_not_empty':
        matched = fieldValue.trim().length > 0;
        break;
      default:
        matched = fieldValueLower.includes(cond.valueLower);
    }

    if (!matched) return false;
  }

  return true;
};

export interface PanelFilters {
  clientIps: string[];
  clientApps: string[];
  domains: string[];
}

const hasPanelFilters = (panel: PanelFilters): boolean => {
  return panel.clientIps.length > 0 || panel.clientApps.length > 0 || panel.domains.length > 0;
};

const matchPanelFilters = (record: TrafficSummary, panel: PanelFilters): boolean => {
  const clientIpMatch = panel.clientIps.length === 0
    || panel.clientIps.includes(record.client_ip || '');

  const clientAppMatch = panel.clientApps.length === 0
    || panel.clientApps.includes(record.client_app || '');

  const domainMatch = panel.domains.length === 0
    || panel.domains.some(domain => (record.host || '').includes(domain));

  return clientIpMatch && clientAppMatch && domainMatch;
};

export const matchesTrafficFilters = (
  record: TrafficSummary,
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
  panel: PanelFilters = { clientIps: [], clientApps: [], domains: [] },
): boolean => {
  const hasToolbarOrConditions = hasActiveFilters(toolbar, conditions);
  const hasPanelActive = hasPanelFilters(panel);

  if (!hasToolbarOrConditions && !hasPanelActive) {
    return true;
  }

  const compiledConditions = compileConditions(conditions);
  const protocolSet = new Set(toolbar.protocol.map((p) => p.toUpperCase()));
  const statusSet = new Set(toolbar.status);
  const typeSet = new Set(toolbar.type);

  const toolbarMatch = !hasToolbarOrConditions
    || matchRecord(record, toolbar, compiledConditions, protocolSet, statusSet, typeSet);
  const panelMatch = !hasPanelActive || matchPanelFilters(record, panel);
  return toolbarMatch && panelMatch;
};

export const filterRecords = (
  records: TrafficSummary[],
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
  panel: PanelFilters = { clientIps: [], clientApps: [], domains: [] }
): TrafficSummary[] => {
  const hasToolbarOrConditions = hasActiveFilters(toolbar, conditions);
  const hasPanelActive = hasPanelFilters(panel);

  if (!hasToolbarOrConditions && !hasPanelActive) {
    return records;
  }

  const compiledConditions = compileConditions(conditions);
  const protocolSet = new Set(toolbar.protocol.map(p => p.toUpperCase()));
  const statusSet = new Set(toolbar.status);
  const typeSet = new Set(toolbar.type);

  const result: TrafficSummary[] = [];
  for (const record of records) {
    const toolbarMatch = !hasToolbarOrConditions || matchRecord(record, toolbar, compiledConditions, protocolSet, statusSet, typeSet);
    const panelMatch = !hasPanelActive || matchPanelFilters(record, panel);

    if (toolbarMatch && panelMatch) {
      result.push(record);
    }
  }
  return result;
};

export const applyTrafficRecordsMutationToFilteredRecords = (
  current: TrafficSummary[],
  mutation: TrafficRecordsMutation,
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
  panel: PanelFilters = { clientIps: [], clientApps: [], domains: [] },
): TrafficSummary[] => {
  if (mutation.reset) {
    return current;
  }

  let next = mutation.deletedIds.length > 0
    ? current.filter((record) => !mutation.deletedIds.includes(record.id))
    : current;

  if (mutation.updated.length > 0) {
    const updatedById = new Map(mutation.updated.map((record) => [record.id, record]));
    const rebuilt: TrafficSummary[] = [];
    const promotedUpdates: TrafficSummary[] = [];

    for (const record of next) {
      const updated = updatedById.get(record.id);
      if (!updated) {
        rebuilt.push(record);
        continue;
      }

      if (matchesTrafficFilters(updated, toolbar, conditions, panel)) {
        rebuilt.push(updated);
      }
      updatedById.delete(record.id);
    }

    for (const updated of updatedById.values()) {
      if (matchesTrafficFilters(updated, toolbar, conditions, panel)) {
        promotedUpdates.push(updated);
      }
    }

    next = promotedUpdates.length > 0
      ? mergeSortedTrafficRecords(rebuilt, promotedUpdates)
      : rebuilt;
  }

  if (mutation.inserted.length === 0) {
    return next;
  }

  const matchingInserted = mutation.inserted.filter((record) =>
    matchesTrafficFilters(record, toolbar, conditions, panel),
  );

  if (matchingInserted.length === 0) {
    return next;
  }

  return mergeSortedTrafficRecords(next, matchingInserted);
};

const compactToSummary = (c: TrafficSummaryCompact): TrafficSummary => {
  const FLAGS = { IS_TUNNEL: 1, IS_WEBSOCKET: 2, IS_SSE: 4, IS_H3: 8, HAS_RULE_HIT: 16 };
  return {
    id: c.id,
    sequence: c.seq,
    timestamp: c.ts,
    method: c.m,
    host: c.h,
    path: c.p,
    status: c.s,
    content_type: c.ct || null,
    request_content_type: c.req_ct || null,
    request_size: c.req_sz,
    response_size: c.res_sz,
    duration_ms: c.dur,
    listener_port: c.lp || undefined,
    protocol: c.proto,
    client_ip: c.cip,
    client_app: c.capp || undefined,
    client_pid: c.cpid || undefined,
    is_tunnel: (c.flags & FLAGS.IS_TUNNEL) !== 0,
    is_websocket: (c.flags & FLAGS.IS_WEBSOCKET) !== 0,
    is_sse: (c.flags & FLAGS.IS_SSE) !== 0,
    is_h3: (c.flags & FLAGS.IS_H3) !== 0,
    has_rule_hit: (c.flags & FLAGS.HAS_RULE_HIT) !== 0,
    matched_rule_count: c.rc || 0,
    matched_protocols: c.rp || [],
    frame_count: c.fc,
    socket_status: c.ss || undefined,
    url: `${c.proto === 'https' ? 'https' : 'http'}://${c.h}${c.p}`,
    start_time: c.st,
    end_time: c.et || undefined,
  };
};

export const useTrafficStore = create<TrafficState>()(
  persist(
    (set, get) => ({
      records: [],
      recordsMap: new Map(),
      currentRecord: null,
      requestBody: null,
      responseBody: null,
      requestRawBody: null,
      responseRawBody: null,
      serverTotal: 0,
      serverSequence: 0,
      hasMore: false,
      oldestSequence: null,
      lastId: null,
      lastSequence: null,
      pendingIds: new Set(),
      toolbarFilters: { rule: [], protocol: [], type: [], status: [], imported: [] },
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
      filterVersion: 0,
      initialized: false,
      selectedId: undefined,
      useDbMode: true,
      historyLoading: false,
      catchingUp: false,
      availableClientApps: [],
      availableClientIps: [],
      availableDomains: [],
      clientAppCounts: new Map(),
      clientIpCounts: new Map(),
      domainCounts: new Map(),
      recordsMutation: createEmptyRecordsMutation(),

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
        hasMoreBurst = 0;
        if (state.usePush) {
          get().disablePush();
        }
        set({ polling: false, pollTimeoutId: null });
      },

      enablePush: () => {
        const state = get();
        if (state.pushUnsubscribe || state.pushDeltaUnsubscribe || state.pushDeletedUnsubscribe) return;

        const unsubscribe = pushService.onTrafficUpdates((data) => {
          get().handleTrafficPush(data);
        });

        const unsubscribeDelta = pushService.onTrafficDelta((data) => {
          get().handleTrafficDelta(data);
        });

        const unsubscribeDeleted = pushService.onTrafficDeleted((data) => {
          get().handleTrafficDeleted(data.ids);
        });

        set({
          pushUnsubscribe: unsubscribe,
          pushDeltaUnsubscribe: unsubscribeDelta,
          pushDeletedUnsubscribe: unsubscribeDeleted,
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
        if (state.pushUnsubscribe) {
          state.pushUnsubscribe();
        }
        if (state.pushDeltaUnsubscribe) {
          state.pushDeltaUnsubscribe();
        }
        if (state.pushDeletedUnsubscribe) {
          state.pushDeletedUnsubscribe();
        }
        set({ pushUnsubscribe: null, pushDeltaUnsubscribe: null, pushDeletedUnsubscribe: null });
        pushService.updateSubscription({
          need_traffic: false,
          last_traffic_id: undefined,
          last_sequence: undefined,
          pending_ids: [],
        });
        pushService.disconnectIfIdle();
      },

      handleTrafficPush: (data: TrafficUpdatesData) => {
        const state = get();
        if (state.paused) return;
        if (data.new_records.length === 0 && data.updated_records.length === 0) return;

        const preprocessedNew = preprocessRecords(data.new_records);
        const preprocessedUpdated = preprocessRecords(data.updated_records);

        if (pendingBatch) {
          pendingBatch.newRecords.push(...preprocessedNew);
          pendingBatch.updatedRecords.push(...preprocessedUpdated);
          pendingBatch.serverTotal = data.server_total;
          pendingBatch.hasMore = data.has_more;
        } else {
          pendingBatch = {
            newRecords: [...preprocessedNew],
            updatedRecords: [...preprocessedUpdated],
            serverTotal: data.server_total,
            hasMore: data.has_more,
          };
        }

        const now = performance.now();
        const timeSinceLastUpdate = now - lastUpdateTime;

        if (rafId !== null) {
          return;
        }

        const scheduleUpdate = () => {
          rafId = requestAnimationFrame(() => {
            rafId = null;
            const batch = pendingBatch;
            if (!batch) return;
            pendingBatch = null;
            lastUpdateTime = performance.now();

            set((prevState) => {
              const recordsMap = prevState.recordsMap;
              const clientCatalog = cloneClientCatalog(prevState);
              let hasChanges = false;
              const uniqueNewRecords: TrafficSummary[] = [];
              const replacedRecords: TrafficSummary[] = [];
              let promotedUpdateCount = 0;

              for (const r of batch.updatedRecords) {
                const existing = recordsMap.get(r.id);
                const mergedRecord = mergeIncrementalRecord(existing, r);
                if (shouldReplaceRecord(existing, mergedRecord)) {
                  recordsMap.set(r.id, mergedRecord);
                  replaceRecordInClientCatalog(clientCatalog, existing, mergedRecord);
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
                  addRecordToClientCatalog(clientCatalog, r);
                  hasChanges = true;
                  actualNewCount++;
                  uniqueNewRecords.push(r);
                }
              }

              const newPendingIds = prevState.pendingIds;

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
                allRecords = replaceUpdatedTrafficRecordsInList(prevState.records, replacedRecords);
                allRecords = mergeNewRecordsIntoList(allRecords, uniqueNewRecords);
              } else {
                allRecords = prevState.records;
              }
              const boundaries = getBoundaryState(allRecords);

              const updatedNewRecordsCount = prevState.autoScroll
                ? 0
                : prevState.newRecordsCount + actualNewCount + promotedUpdateCount;

              pushService.updateSubscription({
                last_traffic_id: boundaries.lastId || undefined,
                last_sequence: boundaries.lastSequence || undefined,
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
                recordsMap,
                serverTotal: batch.serverTotal,
                hasMore: prevState.hasMore,
                oldestSequence: boundaries.oldestSequence,
                lastId: boundaries.lastId,
                lastSequence: boundaries.lastSequence,
                pendingIds: newPendingIds,
                newRecordsCount: updatedNewRecordsCount,
                currentRecord: updatedCurrentRecord,
                recordsMutation: hasChanges
                  ? createRecordsMutation({
                    reset: false,
                    inserted: uniqueNewRecords,
                    updated: replacedRecords,
                    deletedIds: [],
                  })
                  : prevState.recordsMutation,
                ...snapshotClientCatalog(clientCatalog),
              };
            });
          });
        };

        if (timeSinceLastUpdate >= UPDATE_THROTTLE_MS) {
          scheduleUpdate();
        } else {
          setTimeout(scheduleUpdate, UPDATE_THROTTLE_MS - timeSinceLastUpdate);
        }
      },

      handleTrafficDelta: (data: TrafficDeltaData) => {
        const state = get();
        if (state.paused) return;
        if (data.inserts.length === 0 && data.updates.length === 0) return;

        const newRecords = data.inserts.map(c => preprocessRecord(compactToSummary(c)));
        const updatedRecords = data.updates.map(c => preprocessRecord(compactToSummary(c)));

        set((prevState) => {
          const recordsMap = prevState.recordsMap;
          const clientCatalog = cloneClientCatalog(prevState);
          let hasChanges = false;
          const uniqueNewRecords: TrafficSummary[] = [];
          const replacedRecords: TrafficSummary[] = [];
          let promotedUpdateCount = 0;

          for (const r of updatedRecords) {
            const existing = recordsMap.get(r.id);
            const mergedRecord = mergeIncrementalRecord(existing, r);
            if (shouldReplaceRecord(existing, mergedRecord)) {
              recordsMap.set(r.id, mergedRecord);
              replaceRecordInClientCatalog(clientCatalog, existing, mergedRecord);
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
          for (const r of newRecords) {
            if (!recordsMap.has(r.id)) {
              recordsMap.set(r.id, r);
              addRecordToClientCatalog(clientCatalog, r);
              hasChanges = true;
              actualNewCount++;
              uniqueNewRecords.push(r);
            }
          }

          const newPendingIds = prevState.pendingIds;

          for (const r of updatedRecords) {
            const isPending = isPendingRecord(r);
            if (!isPending) {
              newPendingIds.delete(r.id);
            }
          }

          for (const r of newRecords) {
            const isPending = isPendingRecord(r);
            if (isPending) {
              newPendingIds.add(r.id);
            }
          }
          capPendingIds(newPendingIds);

          let allRecords: TrafficSummary[];
          if (hasChanges) {
            allRecords = replaceUpdatedTrafficRecordsInList(prevState.records, replacedRecords);
            allRecords = mergeNewRecordsIntoList(allRecords, uniqueNewRecords);
          } else {
            allRecords = prevState.records;
          }
          const boundaries = getBoundaryState(allRecords);

          const updatedNewRecordsCount = prevState.autoScroll
            ? 0
            : prevState.newRecordsCount + actualNewCount + promotedUpdateCount;

          pushService.updateSubscription({
            last_sequence: boundaries.lastSequence || undefined,
            pending_ids: Array.from(newPendingIds),
          });

          let updatedCurrentRecord = prevState.currentRecord;
          if (updatedCurrentRecord) {
            const updatedSummary = findLastRecordById(
              updatedRecords,
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
            recordsMap,
            serverTotal: data.server_total,
            serverSequence: data.server_sequence,
            hasMore: prevState.hasMore,
            oldestSequence: boundaries.oldestSequence,
            lastId: boundaries.lastId,
            lastSequence: boundaries.lastSequence,
            pendingIds: newPendingIds,
            newRecordsCount: updatedNewRecordsCount,
            currentRecord: updatedCurrentRecord,
            recordsMutation: hasChanges
              ? createRecordsMutation({
                reset: false,
                inserted: uniqueNewRecords,
                updated: replacedRecords,
                deletedIds: [],
              })
              : prevState.recordsMutation,
            ...snapshotClientCatalog(clientCatalog),
          };
        });
      },

      handleTrafficDeleted: (ids: string[]) => {
        if (ids.length === 0) return;
        const idsSet = new Set(ids);
        set((prevState) => {
          const recordsMap = new Map(prevState.recordsMap);
          const pendingIds = new Set(prevState.pendingIds);
          const clientCatalog = cloneClientCatalog(prevState);
          let removedCount = 0;

          for (const id of idsSet) {
            const existing = recordsMap.get(id);
            if (recordsMap.delete(id)) {
              if (existing) {
                removeRecordFromClientCatalog(clientCatalog, existing);
              }
              removedCount += 1;
            }
            pendingIds.delete(id);
          }

          const currentDeleted = prevState.currentRecord && idsSet.has(prevState.currentRecord.id);
          const selectedDeleted = prevState.selectedId && idsSet.has(prevState.selectedId);

          if (!currentDeleted && !selectedDeleted && removedCount === 0) {
            return {};
          }

          const records = removedCount > 0
            ? prevState.records.filter((record) => !idsSet.has(record.id))
            : prevState.records;
          const boundaries = getBoundaryState(records);


          const detailRemoved = currentDeleted || !!selectedDeleted;
          return {
            records,
            recordsMap,
            pendingIds,
            serverTotal: Math.max(prevState.serverTotal - removedCount, 0),
            oldestSequence: boundaries.oldestSequence,
            lastId: boundaries.lastId,
            lastSequence: boundaries.lastSequence,
            currentRecord: detailRemoved ? null : prevState.currentRecord,
            requestBody: detailRemoved ? null : prevState.requestBody,
            responseBody: detailRemoved ? null : prevState.responseBody,
            detailLoading: detailRemoved ? false : prevState.detailLoading,
            detailError: detailRemoved ? 'Request was deleted' : prevState.detailError,
            selectedId: selectedDeleted ? undefined : prevState.selectedId,
            filterVersion: removedCount > 0 ? prevState.filterVersion + 1 : prevState.filterVersion,
            recordsMutation: removedCount > 0
              ? createRecordsMutation({
                reset: false,
                inserted: [],
                updated: [],
                deletedIds: Array.from(idsSet),
              })
              : prevState.recordsMutation,
            ...snapshotClientCatalog(clientCatalog),
          };
        });
      },

      fetchInitialData: async () => {
        const state = get();
        if (state.loading || state.initialized) {
          return;
        }

        const generation = ++historyBackfillGeneration;
        resetHistoryRetryState();
        set({ loading: true, error: null });
        try {
          const filter: TrafficUpdatesFilter = {
            limit: INITIAL_WINDOW_LIMIT,
          };
          const response = await api.getTrafficUpdates(filter);
          if (generation !== historyBackfillGeneration) {
            return;
          }

          const convertedRecords = response.new_records.map(compactToSummary);
          const preprocessedRecords = preprocessRecords(convertedRecords);


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
          const clientCatalog = buildClientCatalog(preprocessedRecords);

          set({
            records: preprocessedRecords,
            recordsMap: newRecordsMap,
            serverTotal: response.server_total,
            serverSequence: response.server_sequence,
            hasMore: response.has_more,
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
            ...clientCatalog,
          });
          if (response.has_more) {
            void get().backfillHistory();
          }
        } catch (e) {
          if (generation === historyBackfillGeneration) {
            set({ error: (e as Error).message, loading: false });
          }
        }
      },

      backfillHistory: async () => {
        const state = get();
        if (state.historyLoading || !state.hasMore || state.oldestSequence === null) {
          return;
        }

        const generation = historyBackfillGeneration;
        clearHistoryRetryTimer();
        set({ historyLoading: true });

        try {
          while (true) {
            const currentState = get();
            if (
              generation !== historyBackfillGeneration ||
              !currentState.hasMore ||
              currentState.oldestSequence === null
            ) {
              break;
            }

            const response = await api.getTrafficPage({
              cursor: currentState.oldestSequence,
              limit: HISTORY_BATCH_LIMIT,
              direction: 'backward',
            });
            historyRetryDelayMs = 1000;

            if (generation !== historyBackfillGeneration) {
              return;
            }

            const olderRecords = preprocessRecords(
              response.records.map(compactToSummary).reverse(),
            );

            set((prevState) => {
              if (generation !== historyBackfillGeneration) {
                return {};
              }

              const recordsMap = new Map(prevState.recordsMap);
              const clientCatalog = cloneClientCatalog(prevState);
              const uniqueOlderRecords: TrafficSummary[] = [];
              const replacedRecords: TrafficSummary[] = [];

              for (const record of olderRecords) {
                const existing = recordsMap.get(record.id);
                if (!existing) {
                  recordsMap.set(record.id, record);
                  addRecordToClientCatalog(clientCatalog, record);
                  uniqueOlderRecords.push(record);
                  continue;
                }

                const mergedRecord = mergeIncrementalRecord(existing, record);
                if (shouldReplaceRecord(existing, mergedRecord)) {
                  recordsMap.set(record.id, mergedRecord);
                  replaceRecordInClientCatalog(clientCatalog, existing, mergedRecord);
                  if (hasRecordInList(prevState.records, record.id)) {
                    replacedRecords.push(mergedRecord);
                  } else {
                    uniqueOlderRecords.push(mergedRecord);
                  }
                }
              }

              let records = replaceUpdatedTrafficRecordsInList(prevState.records, replacedRecords);
              records = uniqueOlderRecords.length > 0
                ? mergeNewRecordsIntoList(records, uniqueOlderRecords)
                : records;
              const boundaries = getBoundaryState(records);

              return {
                records,
                recordsMap,
                serverTotal: response.total,
                serverSequence: response.server_sequence,
                hasMore: response.has_more,
                oldestSequence: boundaries.oldestSequence,
                lastId: boundaries.lastId,
                lastSequence: boundaries.lastSequence,
                recordsMutation: (uniqueOlderRecords.length > 0 || replacedRecords.length > 0)
                  ? createRecordsMutation({
                    reset: false,
                    inserted: uniqueOlderRecords,
                    updated: replacedRecords,
                    deletedIds: [],
                  })
                  : prevState.recordsMutation,
                ...snapshotClientCatalog(clientCatalog),
              };
            });

            const afterMerge = get();
            if (!afterMerge.hasMore || olderRecords.length === 0) {
              break;
            }

            await new Promise<void>((resolve) => {
              window.setTimeout(resolve, 0);
            });
          }
        } catch (e) {
          if (generation === historyBackfillGeneration) {
            set({ error: (e as Error).message });
            const retryGeneration = generation;
            const retryDelay = historyRetryDelayMs;
            historyRetryDelayMs = Math.min(historyRetryDelayMs * 2, 10_000);
            clearHistoryRetryTimer();
            historyRetryTimerId = window.setTimeout(() => {
              historyRetryTimerId = null;
              if (retryGeneration !== historyBackfillGeneration) {
                return;
              }
              const nextState = get();
              if (nextState.hasMore && nextState.oldestSequence !== null) {
                void nextState.backfillHistory();
              }
            }, retryDelay);
          }
        } finally {
          if (generation === historyBackfillGeneration) {
            set({ historyLoading: false });
          }
        }
      },

      fetchUpdates: async () => {
        const state = get();
        if (state.paused || !state.polling || state.catchingUp) return;

        try {
          const pendingIdsArray = Array.from(state.pendingIds);

          const filter: TrafficUpdatesFilter = {
            after_id: state.lastId || undefined,
            after_seq: state.lastSequence || undefined,
            pending_ids: pendingIdsArray.length > 0 ? pendingIdsArray.join(',') : undefined,
            limit: UPDATE_BATCH_LIMIT,
          };

          const response = await api.getTrafficUpdates(filter);

          if (response.new_records.length > 0 || response.updated_records.length > 0) {
            const convertedNew = response.new_records.map(compactToSummary);
            const convertedUpdated = response.updated_records.map(compactToSummary);
            const preprocessedNew = preprocessRecords(convertedNew);
            const preprocessedUpdated = preprocessRecords(convertedUpdated);

            set((prevState) => {
              const recordsMap = prevState.recordsMap;
              const clientCatalog = cloneClientCatalog(prevState);
              let hasChanges = false;
              const uniqueNewRecords: TrafficSummary[] = [];
              const replacedRecords: TrafficSummary[] = [];
              let promotedUpdateCount = 0;

              for (const r of preprocessedUpdated) {
                const existing = recordsMap.get(r.id);
                const mergedRecord = mergeIncrementalRecord(existing, r);
                if (shouldReplaceRecord(existing, mergedRecord)) {
                  recordsMap.set(r.id, mergedRecord);
                  replaceRecordInClientCatalog(clientCatalog, existing, mergedRecord);
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
              for (const r of preprocessedNew) {
                if (!recordsMap.has(r.id)) {
                  recordsMap.set(r.id, r);
                  addRecordToClientCatalog(clientCatalog, r);
                  hasChanges = true;
                  actualNewCount++;
                  uniqueNewRecords.push(r);
                }
              }

              const newPendingIds = prevState.pendingIds;

              for (const r of preprocessedUpdated) {
                const isPending = isPendingRecord(r);
                if (!isPending) {
                  newPendingIds.delete(r.id);
                }
              }

              for (const r of preprocessedNew) {
                const isPending = isPendingRecord(r);
                if (isPending) {
                  newPendingIds.add(r.id);
                }
              }
              capPendingIds(newPendingIds);

              let allRecords: TrafficSummary[];
              if (hasChanges) {
                allRecords = replaceUpdatedTrafficRecordsInList(prevState.records, replacedRecords);
                allRecords = mergeNewRecordsIntoList(allRecords, uniqueNewRecords);
              } else {
                allRecords = prevState.records;
              }
              const boundaries = getBoundaryState(allRecords);

              const updatedNewRecordsCount = prevState.autoScroll
                ? 0
                : prevState.newRecordsCount + actualNewCount + promotedUpdateCount;

              return {
                records: allRecords,
                recordsMap,
                serverTotal: response.server_total,
                serverSequence: response.server_sequence,
                hasMore: prevState.hasMore,
                oldestSequence: boundaries.oldestSequence,
                lastId: boundaries.lastId,
                lastSequence: boundaries.lastSequence,
                pendingIds: newPendingIds,
                newRecordsCount: updatedNewRecordsCount,
                recordsMutation: hasChanges
                  ? createRecordsMutation({
                    reset: false,
                    inserted: uniqueNewRecords,
                    updated: replacedRecords,
                    deletedIds: [],
                  })
                  : prevState.recordsMutation,
                ...snapshotClientCatalog(clientCatalog),
              };
            });
          }

          const currentState = get();
          if (currentState.polling) {
            if (response.has_more) {
              hasMoreBurst += 1;
            } else {
              hasMoreBurst = 0;
            }
            const nextDelay = response.has_more
              ? (hasMoreBurst > HAS_MORE_BURST_LIMIT ? HAS_MORE_BACKOFF_INTERVAL : POLL_MIN_INTERVAL)
              : POLL_INTERVAL;
            const timeoutId = window.setTimeout(() => {
              get().fetchUpdates();
            }, nextDelay);
            set({ pollTimeoutId: timeoutId });
          }
        } catch (e) {
          set({ error: (e as Error).message });

          const currentState = get();
          if (currentState.polling) {
            const timeoutId = window.setTimeout(() => {
              get().fetchUpdates();
            }, POLL_INTERVAL);
            set({ pollTimeoutId: timeoutId });
          }
        }
      },

      catchUpUpdates: async () => {
        const state = get();
        if (state.paused || !state.polling || state.catchingUp) return;

        set({ catchingUp: true });
        try {
          const pendingIdsArray = Array.from(get().pendingIds);
          const filter: TrafficUpdatesFilter = {
            after_id: get().lastId || undefined,
            after_seq: get().lastSequence || undefined,
            pending_ids: pendingIdsArray.length > 0 ? pendingIdsArray.join(',') : undefined,
            limit: UPDATE_BATCH_LIMIT,
          };
          const response = await api.getTrafficUpdates(filter);

          set((prev) => ({
            serverTotal: response.server_total,
            serverSequence: response.server_sequence ?? prev.serverSequence,
          }));

          if (response.new_records.length > 0 || response.updated_records.length > 0) {
            const convertedNew = response.new_records.map(compactToSummary);
            const convertedUpdated = response.updated_records.map(compactToSummary);
            const preprocessedNew = preprocessRecords(convertedNew);
            const preprocessedUpdated = preprocessRecords(convertedUpdated);

            set((prevState) => {
              const recordsMap = prevState.recordsMap;
              const clientCatalog = cloneClientCatalog(prevState);
              let hasChanges = false;
              const uniqueNewRecords: TrafficSummary[] = [];
              const replacedRecords: TrafficSummary[] = [];

              for (const r of preprocessedUpdated) {
                const existing = recordsMap.get(r.id);
                const mergedRecord = mergeIncrementalRecord(existing, r);
                if (shouldReplaceRecord(existing, mergedRecord)) {
                  recordsMap.set(r.id, mergedRecord);
                  replaceRecordInClientCatalog(clientCatalog, existing, mergedRecord);
                  hasChanges = true;
                  if (hasRecordInList(prevState.records, r.id)) {
                    replacedRecords.push(mergedRecord);
                  } else {
                    uniqueNewRecords.push(mergedRecord);
                  }
                }
              }

              for (const r of preprocessedNew) {
                if (!recordsMap.has(r.id)) {
                  recordsMap.set(r.id, r);
                  addRecordToClientCatalog(clientCatalog, r);
                  hasChanges = true;
                  uniqueNewRecords.push(r);
                }
              }

              const newPendingIds = new Set(prevState.pendingIds);
              for (const r of preprocessedUpdated) {
                if (!isPendingRecord(r)) {
                  newPendingIds.delete(r.id);
                }
              }
              for (const r of preprocessedNew) {
                if (isPendingRecord(r)) {
                  newPendingIds.add(r.id);
                }
              }
              capPendingIds(newPendingIds);

              let allRecords = prevState.records;
              if (hasChanges) {
                allRecords = replaceUpdatedTrafficRecordsInList(allRecords, replacedRecords);
                allRecords = mergeNewRecordsIntoList(allRecords, uniqueNewRecords);
              }
              const boundaries = getBoundaryState(allRecords);

              pushService.updateSubscription({
                last_traffic_id: boundaries.lastId || undefined,
                last_sequence: boundaries.lastSequence || undefined,
                pending_ids: Array.from(newPendingIds),
              });

              return {
                records: allRecords,
                recordsMap,
                serverTotal: response.server_total,
                serverSequence: response.server_sequence,
                hasMore: prevState.hasMore,
                oldestSequence: boundaries.oldestSequence,
                lastId: boundaries.lastId,
                lastSequence: boundaries.lastSequence,
                pendingIds: newPendingIds,
                recordsMutation: hasChanges
                  ? createRecordsMutation({
                    reset: false,
                    inserted: uniqueNewRecords,
                    updated: replacedRecords,
                    deletedIds: [],
                  })
                  : prevState.recordsMutation,
                ...snapshotClientCatalog(clientCatalog),
              };
            });
          }
        } catch (e) {
          set({ error: (e as Error).message });
        } finally {
          set({ catchingUp: false });
        }

        const afterState = get();
        if (afterState.records.length > 0 && afterState.serverTotal < afterState.records.length) {
          await get().reloadRecords();
        }
      },

      reloadRecords: async () => {
        try {
          const filter: TrafficUpdatesFilter = {
            limit: INITIAL_WINDOW_LIMIT,
          };
          const response = await api.getTrafficUpdates(filter);
          const convertedRecords = response.new_records.map(compactToSummary);
          const preprocessedRecords = preprocessRecords(convertedRecords);

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
          const clientCatalog = buildClientCatalog(preprocessedRecords);

          set({
            records: preprocessedRecords,
            recordsMap: newRecordsMap,
            serverTotal: response.server_total,
            serverSequence: response.server_sequence,
            hasMore: response.has_more,
            oldestSequence: boundaries.oldestSequence,
            lastId: boundaries.lastId,
            lastSequence: boundaries.lastSequence,
            pendingIds: newPendingIds,
            filterVersion: get().filterVersion + 1,
            recordsMutation: createRecordsMutation({
              reset: true,
              inserted: preprocessedRecords,
              updated: [],
              deletedIds: [],
            }),
            ...clientCatalog,
          });

          pushService.updateSubscription({
            last_traffic_id: boundaries.lastId || undefined,
            last_sequence: boundaries.lastSequence || undefined,
            pending_ids: Array.from(newPendingIds),
          });

          if (response.has_more) {
            void get().backfillHistory();
          }
        } catch {
          // reload is best-effort; errors are non-fatal
        }
      },

      fetchTrafficDetail: async (id: string) => {
        const prevState = get();
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
          const summary = get().recordsMap.get(id);
          const mergedRecord = mergeDetailWithSummary(record, summary);
          set({ currentRecord: mergedRecord, detailLoading: false, detailError: null });

          api.getRequestBody(id).then(body => {
            set({ requestBody: body });
          }).catch(() => { });

          if (mergedRecord.raw_request_body_ref) {
            api.getRequestBodyContent(id, { raw: true, encoding: 'base64' }).then(body => {
              set({ requestRawBody: body });
            }).catch(() => { });
          }

          const isOpenSse = !!mergedRecord.is_sse && !!mergedRecord.socket_status?.is_open;
          if (!isOpenSse) {
            api.getResponseBody(id).then(body => {
              set({ responseBody: body });
            }).catch(() => { });
            if (mergedRecord.raw_response_body_ref) {
              api.getResponseBodyContent(id, { raw: true, encoding: 'base64' }).then(body => {
                set({ responseRawBody: body });
              }).catch(() => { });
            }
          }
        } catch (e) {
          const error = e as { response?: { data?: { error?: string } }; message?: string };
          const message = error.response?.data?.error || error.message || 'Request detail not found';
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
          const prev = state.responseBody || '';
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

      clearTraffic: async (ids?: string[]) => {
        set({ error: null });

        if (ids && ids.length > 0) {
          const idsToRemove = new Set(ids);
          let removedCount = 0;
          let nextPendingIds: string[] | null = null;

          set((state) => {
            const newRecordsMap = new Map(state.recordsMap);
            const newPendingIds = new Set(state.pendingIds);
            const clientCatalog = cloneClientCatalog(state);
            const currentDeleted = state.currentRecord && idsToRemove.has(state.currentRecord.id);
            const selectedDeleted = state.selectedId && idsToRemove.has(state.selectedId);

            for (const id of idsToRemove) {
              const existing = newRecordsMap.get(id);
              if (newRecordsMap.delete(id)) {
                if (existing) {
                  removeRecordFromClientCatalog(clientCatalog, existing);
                }
                removedCount += 1;
              }
              newPendingIds.delete(id);
            }

            nextPendingIds = Array.from(newPendingIds);

            const newRecords = removedCount > 0
              ? state.records.filter((record) => !idsToRemove.has(record.id))
              : state.records;
            const boundaries = getBoundaryState(newRecords);
            const detailRemoved = currentDeleted || !!selectedDeleted;

            return {
              records: newRecords,
              recordsMap: newRecordsMap,
              pendingIds: newPendingIds,
              serverTotal: Math.max(state.serverTotal - removedCount, 0),
              oldestSequence: boundaries.oldestSequence,
              lastId: boundaries.lastId,
              lastSequence: boundaries.lastSequence,
              currentRecord: detailRemoved ? null : state.currentRecord,
              requestBody: detailRemoved ? null : state.requestBody,
              responseBody: detailRemoved ? null : state.responseBody,
              detailLoading: detailRemoved ? false : state.detailLoading,
              detailError: detailRemoved ? 'Request was deleted' : state.detailError,
              selectedId: selectedDeleted ? undefined : state.selectedId,
              filterVersion: removedCount > 0 ? state.filterVersion + 1 : state.filterVersion,
              recordsMutation: removedCount > 0
                ? createRecordsMutation({
                  reset: false,
                  inserted: [],
                  updated: [],
                  deletedIds: Array.from(idsToRemove),
                })
                : state.recordsMutation,
              ...snapshotClientCatalog(clientCatalog),
            };
          });

          if (nextPendingIds) {
            pushService.updateSubscription({ pending_ids: nextPendingIds });
          }

          api.clearTraffic(ids).catch((e) => {
            const err = e as Error;
            set({ error: err.message });
          });

          return true;
        }

        historyBackfillGeneration += 1;
        resetHistoryRetryState();
        set({
          records: [],
          recordsMap: new Map(),
          serverTotal: 0,
          serverSequence: 0,
          hasMore: false,
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
          availableClientApps: [],
          availableClientIps: [],
          availableDomains: [],
          clientAppCounts: new Map(),
          clientIpCounts: new Map(),
          domainCounts: new Map(),
          recordsMutation: createRecordsMutation({
            reset: true,
            inserted: [],
            updated: [],
            deletedIds: [],
          }),
        });

        pushService.updateSubscription({ pending_ids: [] });

        api.clearTraffic().catch((e) => {
          const err = e as Error;
          set({ error: err.message });
        });

        return true;
      },

      setToolbarFilters: (filters: ToolbarFilters) => {
        set((state) => ({ toolbarFilters: filters, filterVersion: state.filterVersion + 1 }));
      },

      setFilterConditions: (conditions: FilterCondition[]) => {
        set((state) => ({ filterConditions: conditions, filterVersion: state.filterVersion + 1 }));
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

      clearCurrentRecord: () => set({
        currentRecord: null,
        requestBody: null,
        responseBody: null,
        requestRawBody: null,
        responseRawBody: null,
        detailError: null,
      }),

      initFromUrl: (filters: FilterCondition[], toolbar: ToolbarFilters | null) => {
        set({
          filterConditions: filters,
          toolbarFilters: toolbar || { rule: [], protocol: [], type: [], status: [], imported: [] },
        });
      },

      setScrollTop: (scrollTop: number) => set({ scrollTop }),

      setSelectedId: (id: string | undefined) => set({ selectedId: id }),
    }),
    {
      name: 'bifrost-traffic-ui',
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
  if (trafficSelectionSyncInitialized || typeof window === 'undefined') {
    return;
  }
  trafficSelectionSyncInitialized = true;

  useTrafficStore.subscribe((state, prevState) => {
    if (isApplyingExternalSelectedId || state.selectedId === prevState.selectedId) {
      return;
    }

    trafficSelectionSyncChannel?.postMessage({
      type: 'selected-id',
      id: state.selectedId,
    });
  });

  trafficSelectionSyncChannel?.addEventListener('message', (event: MessageEvent) => {
    const data = event.data as { type?: string; id?: string } | undefined;
    if (data?.type !== 'selected-id') {
      return;
    }
    applyExternalSelectedId(data.id);
  });
}

initializeTrafficSelectionSync();
