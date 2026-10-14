import type {
  TrafficSummary,
  TrafficRecord,
  TrafficSummaryCompact,
  TrafficStatistics,
} from "../types";

import type { TrafficRecordsMutation } from "./trafficStoreTypes";
let recordsMutationVersion = 0;
export const contentTypeMap: Record<string, string[]> = {
  JSON: ["json", "application/json"],
  Form: ["form", "x-www-form-urlencoded", "multipart/form-data"],
  XML: ["xml", "application/xml", "text/xml"],
  JS: ["javascript", "text/javascript", "application/javascript"],
  CSS: ["css", "text/css"],
  Font: ["font", "woff", "woff2", "ttf", "otf", "eot"],
  Doc: ["html", "text/html"],
  Media: [
    "image",
    "video",
    "audio",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "svg",
    "mp4",
    "webm",
    "mp3",
    "wav",
  ],
  SSE: ["event-stream", "text/event-stream"],
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

export const getStatusDotColor = (status: number): string => {
  if (status === 0) return STATUS_DOT_COLORS.pending;
  if (status >= 100 && status < 200) return STATUS_DOT_COLORS.info;
  if (status >= 200 && status < 300) return STATUS_DOT_COLORS.success;
  if (status >= 300 && status < 400) return STATUS_DOT_COLORS.redirect;
  if (status >= 400 && status < 500) return STATUS_DOT_COLORS.clientError;
  if (status >= 500) return STATUS_DOT_COLORS.serverError;
  return STATUS_DOT_COLORS.pending;
};

export const getStatusColor = (status: number): string => {
  if (status >= 500) return "error";
  if (status >= 400) return "warning";
  if (status >= 300) return "processing";
  if (status >= 200) return "success";
  return "default";
};

export const formatSize = (bytes: number): string => {
  if (bytes === 0) return "-";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
};

export const mergeSseBody = (prev: string | null, payload: string): string => {
  const trimmed = payload.replace(/\n+$/, "");
  if (!trimmed) return prev || "";
  if (!prev || prev.length === 0) return trimmed;
  if (prev.endsWith("\n\n")) return `${prev}${trimmed}`;
  if (prev.endsWith("\n")) return `${prev}\n${trimmed}`;
  return `${prev}\n\n${trimmed}`;
};

export const SSE_RESPONSE_BODY_CHAR_LIMIT = 2_000_000;

export const getDisplaySizeBytes = (
  record: TrafficSummary | undefined,
): number => {
  if (!record) return 0;
  if (
    record.socket_status &&
    (record.response_size === 0 || record.socket_status.is_open)
  ) {
    return record.socket_status.send_bytes + record.socket_status.receive_bytes;
  }
  return record.response_size;
};

export const isPendingRecord = (record: TrafficSummary): boolean => {
  return record.status === 0 || record.socket_status?.is_open === true;
};

export const shouldReplaceRecord = (
  existing: TrafficSummary | undefined,
  next: TrafficSummary,
): boolean => {
  if (!existing) return true;

  return (
    existing.client_app !== next.client_app ||
    existing.client_pid !== next.client_pid ||
    existing.account_name !== next.account_name ||
    existing.has_rule_hit !== next.has_rule_hit ||
    existing.matched_rule_count !== next.matched_rule_count ||
    existing.matched_protocols.join("|") !== next.matched_protocols.join("|") ||
    existing.status !== next.status ||
    existing.request_size !== next.request_size ||
    existing.response_size !== next.response_size ||
    existing.duration_ms !== next.duration_ms ||
    existing.frame_count !== next.frame_count ||
    existing.content_type !== next.content_type ||
    existing.socket_status?.is_open !== next.socket_status?.is_open ||
    existing.socket_status?.send_bytes !== next.socket_status?.send_bytes ||
    existing.socket_status?.receive_bytes !==
      next.socket_status?.receive_bytes ||
    existing.socket_status?.frame_count !== next.socket_status?.frame_count ||
    getDisplaySizeBytes(existing) !== getDisplaySizeBytes(next)
  );
};

export const mergeIncrementalRecord = (
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
    account_name: next.account_name ?? existing.account_name,
  };

  return preprocessTrafficRecord(merged);
};

export const preprocessTrafficRecord = (
  record: TrafficSummary,
): TrafficSummary => {
  const isH3 =
    record.is_h3 || record.protocol === "h3" || record.protocol === "h3s";
  const displayProtocol = isH3
    ? "H3"
    : record.protocol?.replace("HTTP/", "").toUpperCase() || "-";

  const methodColor = METHOD_COLORS[record.method?.toUpperCase()] || "default";
  const statusColor = getStatusColor(record.status);
  const statusDotColor = getStatusDotColor(record.status);

  const size = getDisplaySizeBytes(record);
  const displaySize = formatSize(size);

  const contentTypeShort =
    record.content_type?.split(";")[0]?.split("/").pop() || "-";

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

export const preprocessTrafficRecords = (
  records: TrafficSummary[],
): TrafficSummary[] => {
  for (let i = 0; i < records.length; i++) {
    preprocessTrafficRecord(records[i]);
  }
  return records;
};

export const mergeDetailWithSummary = (
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

export const compareTrafficRecordsBySequence = (
  left: TrafficSummary,
  right: TrafficSummary,
): number => {
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

export const mergeNewRecordsIntoList = (
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

  const updatedById = new Map(
    updatedRecords.map((record) => [record.id, record]),
  );
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

export const findLastRecordById = (
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

export const hasRecordInList = (
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

export const getBoundaryState = (records: TrafficSummary[]) => {
  const oldestRecord = records[0];
  const latestRecord = records[records.length - 1];
  return {
    oldestSequence: oldestRecord?.sequence ?? null,
    lastSequence: latestRecord?.sequence ?? null,
    lastId: latestRecord?.id ?? null,
  };
};

export const createRecordsMutation = (
  mutation: Omit<TrafficRecordsMutation, "version">,
): TrafficRecordsMutation => ({
  version: ++recordsMutationVersion,
  ...mutation,
});

export const createEmptyRecordsMutation = (): TrafficRecordsMutation => ({
  version: recordsMutationVersion,
  reset: false,
  inserted: [],
  updated: [],
  deletedIds: [],
});

export const buildSortedKeys = (counts: Map<string, number>): string[] =>
  Array.from(counts.keys()).sort();

export const snapshotServerStatistics = (statistics: TrafficStatistics) => {
  const clientIpCounts = new Map(Object.entries(statistics.client_ips));
  const proxyPortCounts = new Map(Object.entries(statistics.proxy_ports));
  const clientAppCounts = new Map(Object.entries(statistics.applications));
  const accountNameCounts = new Map(Object.entries(statistics.account_names));
  const domainCounts = new Map(Object.entries(statistics.domains));

  return {
    clientIpCounts,
    proxyPortCounts,
    clientAppCounts,
    accountNameCounts,
    domainCounts,
    availableClientIps: buildSortedKeys(clientIpCounts),
    availableProxyPorts: buildSortedKeys(proxyPortCounts),
    availableClientApps: buildSortedKeys(clientAppCounts),
    availableAccountNames: buildSortedKeys(accountNameCounts),
    availableDomains: buildSortedKeys(domainCounts),
    trafficStatisticsTotal: statistics.total_requests,
    trafficStatisticsSequence: statistics.server_sequence,
    trafficStatisticsLoaded: true,
  };
};

export const compactTrafficSummaryToTrafficSummary = (
  c: TrafficSummaryCompact,
): TrafficSummary => {
  const FLAGS = {
    IS_TUNNEL: 1,
    IS_WEBSOCKET: 2,
    IS_SSE: 4,
    IS_H3: 8,
    HAS_RULE_HIT: 16,
  };
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
    upload_bytes: c.up ?? c.req_sz,
    download_bytes: c.down ?? c.res_sz,
    duration_ms: c.dur,
    listener_port: c.lp || undefined,
    protocol: c.proto,
    client_ip: c.cip,
    client_app: c.capp || undefined,
    client_pid: c.cpid || undefined,
    account_name: c.acct || undefined,
    is_tunnel: (c.flags & FLAGS.IS_TUNNEL) !== 0,
    is_websocket: (c.flags & FLAGS.IS_WEBSOCKET) !== 0,
    is_sse: (c.flags & FLAGS.IS_SSE) !== 0,
    is_h3: (c.flags & FLAGS.IS_H3) !== 0,
    has_rule_hit: (c.flags & FLAGS.HAS_RULE_HIT) !== 0,
    matched_rule_count: c.rc || 0,
    matched_protocols: c.rp || [],
    frame_count: c.fc,
    socket_status: c.ss || undefined,
    url: `${c.proto === "https" ? "https" : "http"}://${c.h}${c.p}`,
    start_time: c.st,
    end_time: c.et || undefined,
  };
};
