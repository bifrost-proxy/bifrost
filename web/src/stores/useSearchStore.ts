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
        search_id: string;
      };
    };

let currentSearchAbort: AbortController | null = null;
let currentLoadMoreAbort: AbortController | null = null;

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

  setMode: (mode: 'normal' | 'search') => void;
  setKeyword: (keyword: string) => void;
  setScope: (scope: Partial<SearchScope>) => void;
  search: (filters: SearchFilters) => Promise<void>;
  loadMore: (filters: SearchFilters) => Promise<void>;
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

  setMode: (mode) => {
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
        isSearching: false,
        isLoadingMore: false,
      });
    } else {
      set({ mode });
    }
  },

  setKeyword: (keyword) => set({ keyword }),

  setScope: (scopeUpdate) => {
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
      });
    } else {
      const newScope = { ...scope, ...scopeUpdate, all: false };
      const hasAny = newScope.request_body || newScope.response_body ||
        newScope.request_headers || newScope.response_headers || newScope.url ||
        newScope.websocket_messages || newScope.sse_events;
      if (!hasAny) {
        newScope.all = true;
      }
      set({ scope: newScope });
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

    currentSearchAbort = new AbortController();

    set({
      isSearching: true,
      isLoadingMore: false,
      results: [],
      totalSearched: 0,
      totalMatched: 0,
      hasMore: false,
      nextCursor: null,
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
        set({ isSearching: false });
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
        set({ isSearching: false });
        return;
      }
      console.error('[SearchStore] Search failed:', error);
      set({ isSearching: false });
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

      cancelSearch: () => {
        abortSearch();
        abortLoadMore();
        set({ isSearching: false, isLoadingMore: false });
      },

      reset: () => {
        abortSearch();
        abortLoadMore();
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
