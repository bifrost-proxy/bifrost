import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type {
  SearchScope,
  SearchFilters,
  SearchResultItem,
  SearchResponse,
  SearchRequest,
  TrafficSummary,
  TrafficSummaryCompact,
} from '../types';
import { TrafficFlags } from '../types';
import { apiFetch } from '../api/apiFetch';

type SearchStreamEvent =
  | { event: 'result'; data: SearchResultItem }
  | {
      event: 'progress';
      data: {
        total_searched: number;
        total_matched: number;
        next_cursor: number | null;
        has_more_hint: boolean;
        iterations: number;
      };
    }
  | {
      event: 'done';
      data: {
        total_searched: number;
        total_matched: number;
        next_cursor: number | null;
        has_more: boolean;
        partial_reason?: string;
        search_id: string;
      };
    };

let currentSearchAbort: AbortController | null = null;
let currentLoadMoreAbort: AbortController | null = null;
let liveSearchGeneration = 0;

export const MAX_LIVE_SEARCH_RECORD_IDS = 500;
const MAX_SEARCH_RESULTS = 1000;

export interface LiveSearchMutation {
  reset: boolean;
  insertedIds: string[];
  updatedIds: string[];
  deletedIds: string[];
  oldestSequenceFloor?: number | null;
  incomplete?: boolean;
}

export interface LiveSearchMutationAccumulator {
  reset: boolean;
  changedIds: Set<string>;
  deletedIds: Set<string>;
  oldestSequenceFloor?: number | null;
  incomplete: boolean;
}

export function createLiveSearchMutationAccumulator(): LiveSearchMutationAccumulator {
  return {
    reset: false,
    changedIds: new Set<string>(),
    deletedIds: new Set<string>(),
    oldestSequenceFloor: undefined,
    incomplete: false,
  };
}

export function coalesceLiveSearchMutation(
  accumulator: LiveSearchMutationAccumulator,
  mutation: LiveSearchMutation,
): LiveSearchMutationAccumulator {
  accumulator.reset ||= mutation.reset;
  accumulator.incomplete ||= mutation.incomplete === true;

  const addBounded = (id: string, target: Set<string>) => {
    if (accumulator.changedIds.has(id) || accumulator.deletedIds.has(id)) return;
    if (
      accumulator.changedIds.size + accumulator.deletedIds.size >=
      MAX_LIVE_SEARCH_RECORD_IDS
    ) {
      accumulator.incomplete = true;
      return;
    }
    target.add(id);
  };

  for (const id of mutation.insertedIds) addBounded(id, accumulator.changedIds);
  for (const id of mutation.updatedIds) addBounded(id, accumulator.changedIds);
  for (const id of mutation.deletedIds) addBounded(id, accumulator.deletedIds);

  if (mutation.oldestSequenceFloor !== undefined) {
    accumulator.oldestSequenceFloor =
      Math.max(
        accumulator.oldestSequenceFloor ?? 0,
        mutation.oldestSequenceFloor ?? 0,
      ) || null;
  }
  return accumulator;
}

export function mergeLiveSearchResults(
  current: SearchResultItem[],
  changedIds: Iterable<string>,
  replacements: SearchResultItem[],
  deletedIds: Iterable<string>,
  oldestSequenceFloor?: number | null,
): { results: SearchResultItem[]; knownMatchDelta: number } {
  const changed = new Set(changedIds);
  const deleted = new Set(deletedIds);
  const removedKnownIds = new Set<string>();
  const replacementIds = new Set<string>();
  const merged = new Map<string, SearchResultItem>();

  for (const item of current) {
    const belowFloor = oldestSequenceFloor !== undefined &&
      oldestSequenceFloor !== null &&
      item.record.seq < oldestSequenceFloor;
    if (changed.has(item.record.id) || deleted.has(item.record.id) || belowFloor) {
      removedKnownIds.add(item.record.id);
      continue;
    }
    merged.set(item.record.id, item);
  }

  for (const item of replacements) {
    if (deleted.has(item.record.id)) continue;
    if (
      oldestSequenceFloor !== undefined &&
      oldestSequenceFloor !== null &&
      item.record.seq < oldestSequenceFloor
    ) {
      continue;
    }
    replacementIds.add(item.record.id);
    merged.set(item.record.id, item);
  }

  const results = Array.from(merged.values())
    .sort((a, b) => b.record.seq - a.record.seq)
    .slice(0, MAX_SEARCH_RESULTS);

  return {
    results,
    knownMatchDelta: replacementIds.size - removedKnownIds.size,
  };
}

function buildSearchKey(
  keyword: string,
  scope: SearchScope,
  filters: SearchFilters,
): string {
  return JSON.stringify({ keyword: keyword.trim(), scope, filters });
}

async function* parseSseStream(
  body: ReadableStream<Uint8Array>,
): AsyncGenerator<SearchStreamEvent> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  while (true) {
    const { value, done } = await reader.read();
    if (done) break;

    // Normalize CRLF to LF to simplify parsing.
    buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, '\n');

    // SSE event delimiter is a blank line
    let idx;
    while ((idx = buffer.indexOf('\n\n')) !== -1) {
      const raw = buffer.slice(0, idx);
      buffer = buffer.slice(idx + 2);

      const lines = raw.split('\n');
      let eventName = '';
      const dataLines: string[] = [];
      for (const line of lines) {
        if (line.startsWith('event:')) {
          eventName = line.slice('event:'.length).trim();
        } else if (line.startsWith('data:')) {
          dataLines.push(line.slice('data:'.length).trim());
        }
      }

      if (!eventName || dataLines.length === 0) continue;
      const dataText = dataLines.join('\n');

      try {
        const data = JSON.parse(dataText);
        if (eventName === 'result') {
          yield { event: 'result', data } as SearchStreamEvent;
        } else if (eventName === 'progress') {
          yield { event: 'progress', data } as SearchStreamEvent;
        } else if (eventName === 'done') {
          yield { event: 'done', data } as SearchStreamEvent;
        }
      } catch {
        // ignore malformed chunk
      }
    }
  }
}

function abortSearch() {
  try {
    currentSearchAbort?.abort();
  } finally {
    currentSearchAbort = null;
  }
}

function abortLoadMore() {
  try {
    currentLoadMoreAbort?.abort();
  } finally {
    currentLoadMoreAbort = null;
  }
}

function isAbortError(err: unknown): boolean {
  if (err instanceof DOMException) {
    return err.name === 'AbortError';
  }
  return (
    typeof err === 'object' &&
    err !== null &&
    'name' in err &&
    err.name === 'AbortError'
  );
}

function hasSearchFilters(filters: SearchFilters): boolean {
  return (
    filters.protocols.length > 0 ||
    filters.status_ranges.length > 0 ||
    filters.content_types.length > 0 ||
    filters.conditions.length > 0 ||
    filters.client_ips.length > 0 ||
    filters.client_apps.length > 0 ||
    filters.account_names.length > 0 ||
    filters.domains.length > 0 ||
    filters.has_rule_hit !== undefined
  );
}

interface SearchState {
  mode: 'normal' | 'search';
  keyword: string;
  scope: SearchScope;

  results: SearchResultItem[];
  totalSearched: number;
  totalMatched: number;
  hasMore: boolean;
  nextCursor: number | null;

  isSearching: boolean;
  isLoadingMore: boolean;
  searchId: string | null;
  activeSearchKey: string | null;

  setMode: (mode: 'normal' | 'search') => void;
  setKeyword: (keyword: string) => void;
  setScope: (scope: Partial<SearchScope>) => void;
  search: (filters: SearchFilters) => Promise<void>;
  loadMore: (filters: SearchFilters) => Promise<void>;
  refreshChangedRecords: (
    filters: SearchFilters,
    mutation: LiveSearchMutation,
  ) => Promise<'updated' | 'full_refresh' | 'busy' | 'skipped'>;
  cancelSearch: () => void;
  reset: () => void;
}

const defaultScope: SearchScope = {
  request_body: false,
  response_body: false,
  request_headers: false,
  response_headers: false,
  url: false,
  websocket_messages: false,
  sse_events: false,
  all: true,
};

export const useSearchStore = create<SearchState>()(
  persist(
    (set, get) => ({
      mode: 'normal',
      keyword: '',
      scope: { ...defaultScope },

  results: [],
  totalSearched: 0,
  totalMatched: 0,
  hasMore: false,
  nextCursor: null,

  isSearching: false,
  isLoadingMore: false,
  searchId: null,
  activeSearchKey: null,

  setMode: (mode) => {
    liveSearchGeneration += 1;
    if (mode === 'normal') {
      abortSearch();
      abortLoadMore();
      set({
        mode,
        results: [],
        totalSearched: 0,
        totalMatched: 0,
        hasMore: false,
        nextCursor: null,
        searchId: null,
        activeSearchKey: null,
        isSearching: false,
        isLoadingMore: false,
      });
    } else {
      set({ mode });
    }
  },

  setKeyword: (keyword) => {
    liveSearchGeneration += 1;
    abortSearch();
    abortLoadMore();
    set({
      keyword,
      activeSearchKey: null,
      isSearching: false,
      isLoadingMore: false,
    });
  },

  setScope: (scopeUpdate) => {
    liveSearchGeneration += 1;
    abortSearch();
    abortLoadMore();
    const { scope } = get();
    if (scopeUpdate.all === true) {
      set({
        scope: {
          ...scope,
          request_body: false,
          response_body: false,
          request_headers: false,
          response_headers: false,
          url: false,
          websocket_messages: false,
          sse_events: false,
          all: true,
        },
        activeSearchKey: null,
        isSearching: false,
        isLoadingMore: false,
      });
    } else {
      const newScope = { ...scope, ...scopeUpdate, all: false };
      const hasAny = newScope.request_body || newScope.response_body ||
        newScope.request_headers || newScope.response_headers || newScope.url ||
        newScope.websocket_messages || newScope.sse_events;
      if (!hasAny) {
        newScope.all = true;
      }
      set({
        scope: newScope,
        activeSearchKey: null,
        isSearching: false,
        isLoadingMore: false,
      });
    }
  },

  search: async (filters) => {
    const { keyword, scope } = get();
    if (!keyword.trim() && !hasSearchFilters(filters)) {
      return;
    }

    // abort previous search or loadMore immediately to keep UI responsive
    abortSearch();
    abortLoadMore();
    const generation = ++liveSearchGeneration;
    const activeSearchKey = buildSearchKey(keyword, scope, filters);

    currentSearchAbort = new AbortController();

    set({
      isSearching: true,
      isLoadingMore: false,
      results: [],
      totalSearched: 0,
      totalMatched: 0,
      hasMore: false,
      nextCursor: null,
      activeSearchKey,
    });

    try {
      const request: SearchRequest = {
        keyword: keyword.trim(),
        scope,
        filters,
        limit: 50,
        max_results: 1000,
      };

      // Prefer streaming search so the UI can see the first batch of results/progress sooner.
      const streamResp = await apiFetch('/_bifrost/api/search/stream', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(request),
        signal: currentSearchAbort.signal,
      });

      const ct = streamResp.headers.get('content-type') || '';
      if (streamResp.ok && ct.includes('text/event-stream') && streamResp.body) {
        let accResults: SearchResultItem[] = [];

        for await (const ev of parseSseStream(streamResp.body)) {
          if (generation !== liveSearchGeneration) return;
          if (ev.event === 'result') {
            accResults = [...accResults, ev.data];
            set({ results: accResults });
          } else if (ev.event === 'progress') {
            set({
              totalSearched: ev.data.total_searched,
              totalMatched: ev.data.total_matched,
              nextCursor: ev.data.next_cursor,
              hasMore: ev.data.has_more_hint,
            });
          } else if (ev.event === 'done') {
            if (generation !== liveSearchGeneration) return;
            set({
              totalSearched: ev.data.total_searched,
              totalMatched: ev.data.total_matched,
              hasMore: ev.data.has_more,
              nextCursor: ev.data.next_cursor,
              searchId: ev.data.search_id,
              isSearching: false,
            });
            return;
          }
        }

        // stream ended unexpectedly
        if (generation === liveSearchGeneration) set({ isSearching: false });
        return;
      }

      // fallback: non-streaming
      const response = await apiFetch('/_bifrost/api/search', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(request),
        signal: currentSearchAbort.signal,
      });

      if (!response.ok) {
        throw new Error(`Search failed: ${response.statusText}`);
      }

      const data: SearchResponse = await response.json();

      if (generation !== liveSearchGeneration) return;

      set({
        results: data.results,
        totalSearched: data.total_searched,
        totalMatched: data.total_matched,
        hasMore: data.has_more,
        nextCursor: data.next_cursor,
        searchId: data.search_id,
        isSearching: false,
      });
    } catch (error) {
      if (isAbortError(error)) {
        // aborted by user or replaced by a new search
        if (generation === liveSearchGeneration) set({ isSearching: false });
        return;
      }
      console.error('[SearchStore] Search failed:', error);
      if (generation === liveSearchGeneration) set({ isSearching: false });
    }
  },

  loadMore: async (filters) => {
    const { keyword, scope, nextCursor, hasMore, isLoadingMore, results } = get();
    if (
      (!keyword.trim() && !hasSearchFilters(filters)) ||
      !hasMore ||
      isLoadingMore ||
      nextCursor === null
    ) {
      return;
    }

    abortLoadMore();
    currentLoadMoreAbort = new AbortController();

    set({ isLoadingMore: true });

    try {
      const request: SearchRequest = {
        keyword: keyword.trim(),
        scope,
        filters,
        cursor: nextCursor,
        limit: 50,
        max_results: 1000,
      };

      const streamResp = await apiFetch('/_bifrost/api/search/stream', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(request),
        signal: currentLoadMoreAbort.signal,
      });

      const ct = streamResp.headers.get('content-type') || '';
      if (streamResp.ok && ct.includes('text/event-stream') && streamResp.body) {
        let accResults: SearchResultItem[] = results;
        const baseSearched = get().totalSearched;
        const baseMatched = get().totalMatched;

        for await (const ev of parseSseStream(streamResp.body)) {
          if (ev.event === 'result') {
            accResults = [...accResults, ev.data];
            set({ results: accResults });
          } else if (ev.event === 'progress') {
            set({
              totalSearched: baseSearched + ev.data.total_searched,
              totalMatched: baseMatched + ev.data.total_matched,
              nextCursor: ev.data.next_cursor,
              hasMore: ev.data.has_more_hint,
            });
          } else if (ev.event === 'done') {
            set({
              totalSearched: baseSearched + ev.data.total_searched,
              totalMatched: baseMatched + ev.data.total_matched,
              hasMore: ev.data.has_more,
              nextCursor: ev.data.next_cursor,
              isLoadingMore: false,
            });
            return;
          }
        }

        set({ isLoadingMore: false });
        return;
      }

      const response = await apiFetch('/_bifrost/api/search', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(request),
        signal: currentLoadMoreAbort.signal,
      });

      if (!response.ok) {
        throw new Error(`Search failed: ${response.statusText}`);
      }

      const data: SearchResponse = await response.json();

      set({
        results: [...results, ...data.results],
        totalSearched: get().totalSearched + data.total_searched,
        totalMatched: get().totalMatched + data.total_matched,
        hasMore: data.has_more,
        nextCursor: data.next_cursor,
        isLoadingMore: false,
      });
    } catch (error) {
      if (isAbortError(error)) {
        set({ isLoadingMore: false });
        return;
      }
      console.error('[SearchStore] Load more failed:', error);
      set({ isLoadingMore: false });
    }
  },

  refreshChangedRecords: async (filters, mutation) => {
    const state = get();
    const expectedKey = buildSearchKey(state.keyword, state.scope, filters);
    if (
      state.mode !== 'search' ||
      state.activeSearchKey !== expectedKey
    ) {
      return 'skipped';
    }
    if (state.isSearching || state.isLoadingMore) return 'busy';

    const changedIds = Array.from(new Set([
      ...mutation.insertedIds,
      ...mutation.updatedIds,
    ]));

    if (
      mutation.reset ||
      mutation.incomplete ||
      changedIds.length > MAX_LIVE_SEARCH_RECORD_IDS
    ) {
      await get().search(filters);
      return 'full_refresh';
    }

    const generation = liveSearchGeneration;
    let replacements: SearchResultItem[] = [];
    if (changedIds.length > 0) {
      const request: SearchRequest = {
        keyword: state.keyword.trim(),
        scope: state.scope,
        filters,
        record_ids: changedIds,
        limit: MAX_LIVE_SEARCH_RECORD_IDS,
        max_scan: MAX_LIVE_SEARCH_RECORD_IDS,
        max_results: MAX_LIVE_SEARCH_RECORD_IDS,
      };
      try {
        const response = await apiFetch('/_bifrost/api/search', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(request),
        });
        if (!response.ok) {
          throw new Error(`Live search refresh failed: ${response.statusText}`);
        }
        const data: SearchResponse = await response.json();
        replacements = data.results;
      } catch (error) {
        console.error('[SearchStore] Live refresh failed, running full search:', error);
        if (generation === liveSearchGeneration) {
          await get().search(filters);
          return 'full_refresh';
        }
        return 'skipped';
      }
    }

    const latest = get();
    if (
      generation !== liveSearchGeneration ||
      latest.activeSearchKey !== expectedKey ||
      latest.mode !== 'search'
    ) {
      return 'skipped';
    }

    const merged = mergeLiveSearchResults(
      latest.results,
      changedIds,
      replacements,
      mutation.deletedIds,
      mutation.oldestSequenceFloor,
    );
    set({
      results: merged.results,
      totalMatched: Math.max(
        merged.results.length,
        latest.totalMatched + merged.knownMatchDelta,
      ),
    });
    return 'updated';
  },

      cancelSearch: () => {
        liveSearchGeneration += 1;
        abortSearch();
        abortLoadMore();
        set({
          isSearching: false,
          isLoadingMore: false,
          activeSearchKey: null,
        });
      },

      reset: () => {
        abortSearch();
        abortLoadMore();
        liveSearchGeneration += 1;
        set({
          mode: 'normal',
          keyword: '',
          scope: { ...defaultScope },
          results: [],
          totalSearched: 0,
          totalMatched: 0,
          hasMore: false,
          nextCursor: null,
          isSearching: false,
          isLoadingMore: false,
          searchId: null,
          activeSearchKey: null,
        });
      },
    }),
    {
      name: 'bifrost-search-ui',
      partialize: (state) => ({
        mode: state.mode,
        keyword: state.keyword,
        scope: state.scope,
      }),
      version: 1,
    },
  ),
);

export const compactToSummary = (c: TrafficSummaryCompact): TrafficSummary => {
  return {
    id: c.id,
    sequence: c.seq,
    timestamp: c.ts,
    method: c.m,
    host: c.h,
    path: c.p,
    status: c.s,
    content_type: c.ct || null,
    request_size: c.req_sz,
    response_size: c.res_sz,
    upload_bytes: c.up ?? c.req_sz,
    download_bytes: c.down ?? c.res_sz,
    duration_ms: c.dur,
    protocol: c.proto,
    client_ip: c.cip,
    client_app: c.capp || undefined,
    client_pid: c.cpid || undefined,
    account_name: c.acct || undefined,
    is_tunnel: (c.flags & TrafficFlags.IS_TUNNEL) !== 0,
    is_websocket: (c.flags & TrafficFlags.IS_WEBSOCKET) !== 0,
    is_sse: (c.flags & TrafficFlags.IS_SSE) !== 0,
    is_h3: (c.flags & TrafficFlags.IS_H3) !== 0,
    has_rule_hit: (c.flags & TrafficFlags.HAS_RULE_HIT) !== 0,
    matched_rule_count: c.rc || 0,
    matched_protocols: c.rp || [],
    frame_count: c.fc,
    socket_status: c.ss || undefined,
    url: `${c.proto === 'https' ? 'https' : 'http'}://${c.h}${c.p}`,
    start_time: c.st,
    end_time: c.et || undefined,
  };
};
