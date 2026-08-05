import type { TrafficSummary } from "../types";

export const MAX_TRAFFIC_WINDOW_RECORDS = 2_000;

export type TrafficWindowMergeDirection = "older" | "newer";

export interface BoundedTrafficWindowResult {
  records: TrafficSummary[];
  trimmed: number;
  trimmedSide: "older" | "newer" | null;
}

export const mergeBoundedTrafficWindow = (
  current: TrafficSummary[],
  incoming: TrafficSummary[],
  direction: TrafficWindowMergeDirection,
  limit = MAX_TRAFFIC_WINDOW_RECORDS,
): BoundedTrafficWindowResult => {
  if (limit <= 0) {
    return {
      records: [],
      trimmed: new Set([...current, ...incoming].map((record) => record.id)).size,
      trimmedSide: direction === "older" ? "newer" : "older",
    };
  }

  const byId = new Map<string, TrafficSummary>();
  for (const record of current) {
    byId.set(record.id, record);
  }
  for (const record of incoming) {
    byId.set(record.id, record);
  }

  const merged = Array.from(byId.values()).sort((left, right) => {
    if (left.sequence !== right.sequence) {
      return left.sequence - right.sequence;
    }
    if (left.timestamp !== right.timestamp) {
      return left.timestamp - right.timestamp;
    }
    return left.id.localeCompare(right.id);
  });

  if (merged.length <= limit) {
    return { records: merged, trimmed: 0, trimmedSide: null };
  }

  const trimmed = merged.length - limit;
  if (direction === "older") {
    return {
      records: merged.slice(0, limit),
      trimmed,
      trimmedSide: "newer",
    };
  }

  return {
    records: merged.slice(trimmed),
    trimmed,
    trimmedSide: "older",
  };
};

export const getTrafficWindowBoundaries = (records: TrafficSummary[]) => ({
  oldestSequence: records[0]?.sequence ?? null,
  newestSequence: records.at(-1)?.sequence ?? null,
  newestId: records.at(-1)?.id ?? null,
});

export const buildTrafficRecordsMap = (
  records: TrafficSummary[],
): Map<string, TrafficSummary> =>
  new Map(records.map((record) => [record.id, record]));
