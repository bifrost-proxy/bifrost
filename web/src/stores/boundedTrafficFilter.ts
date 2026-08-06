import type {
  FilterCondition,
  ToolbarFilters,
  TrafficQueryRequest,
  TrafficQueryResponse,
  TrafficSummary,
} from "../types";
import {
  compactTrafficSummaryToTrafficSummary,
  filterRecords,
  preprocessTrafficRecords,
  type PanelFilters,
} from "./useTrafficStore";
import {
  MAX_TRAFFIC_WINDOW_RECORDS,
  mergeBoundedTrafficWindow,
} from "./trafficWindow";

export const TRAFFIC_FILTER_SCAN_PAGE_SIZE = 500;
export const TRAFFIC_FILTER_INITIAL_MATCHES = 500;

interface ScanBoundedTrafficMatchesOptions {
  fetchPage: (request: TrafficQueryRequest) => Promise<TrafficQueryResponse>;
  toolbar: ToolbarFilters;
  conditions: FilterCondition[];
  panel: PanelFilters;
  initialRecords?: TrafficSummary[];
  cursor?: number | null;
  targetMatches?: number;
  maxResults?: number;
  isCurrent: () => boolean;
  yieldToBrowser?: () => Promise<void>;
}

export interface BoundedTrafficFilterScanResult {
  records: TrafficSummary[];
  cursor: number | null;
  hasMore: boolean;
  scannedCount: number;
  cancelled: boolean;
}

const defaultYieldToBrowser = (): Promise<void> =>
  new Promise((resolve) => window.setTimeout(resolve, 0));

export const scanBoundedTrafficMatches = async ({
  fetchPage,
  toolbar,
  conditions,
  panel,
  initialRecords = [],
  cursor = null,
  targetMatches = TRAFFIC_FILTER_INITIAL_MATCHES,
  maxResults = MAX_TRAFFIC_WINDOW_RECORDS,
  isCurrent,
  yieldToBrowser = defaultYieldToBrowser,
}: ScanBoundedTrafficMatchesOptions): Promise<BoundedTrafficFilterScanResult> => {
  let records = initialRecords;
  let nextCursor = cursor;
  let hasMore = true;
  let scannedCount = 0;
  let matchedCount = 0;

  while (hasMore && matchedCount < targetMatches) {
    const response = await fetchPage({
      cursor: nextCursor ?? undefined,
      limit: TRAFFIC_FILTER_SCAN_PAGE_SIZE,
      direction: "backward",
    });

    if (!isCurrent()) {
      return {
        records: initialRecords,
        cursor,
        hasMore: false,
        scannedCount,
        cancelled: true,
      };
    }

    const pageRecords = preprocessTrafficRecords(
      response.records
        .map(compactTrafficSummaryToTrafficSummary)
        .reverse(),
    );
    scannedCount += pageRecords.length;

    const matching = filterRecords(pageRecords, toolbar, conditions, panel);
    matchedCount += matching.length;
    records = mergeBoundedTrafficWindow(
      records,
      matching,
      "older",
      maxResults,
    ).records;

    nextCursor = pageRecords[0]?.sequence ?? nextCursor;
    hasMore = response.has_more && pageRecords.length > 0;

    if (hasMore && matchedCount < targetMatches) {
      await yieldToBrowser();
      if (!isCurrent()) {
        return {
          records: initialRecords,
          cursor,
          hasMore: false,
          scannedCount,
          cancelled: true,
        };
      }
    }
  }

  return {
    records,
    cursor: nextCursor,
    hasMore,
    scannedCount,
    cancelled: false,
  };
};
