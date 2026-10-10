import { describe, expect, it, vi } from 'vitest';
import { apiFetch } from '../api/apiFetch';
import type { SearchResultItem, TrafficSummaryCompact } from '../types';
import {
  coalesceLiveSearchMutation,
  createLiveSearchMutationAccumulator,
  MAX_LIVE_SEARCH_RECORD_IDS,
  mergeLiveSearchResults,
  useSearchStore,
} from './useSearchStore';

vi.mock('../api/apiFetch', () => ({ apiFetch: vi.fn() }));

const compact = (id: string, seq: number): TrafficSummaryCompact => ({
  id,
  seq,
  ts: seq,
  m: 'GET',
  h: 'example.com',
  p: `/${id}`,
  s: 200,
  ct: 'application/json',
  req_sz: 0,
  res_sz: 0,
  up: 0,
  down: 0,
  dur: 1,
  proto: 'http',
  cip: '127.0.0.1',
  flags: 0,
  fc: 0,
  rc: 0,
  rp: [],
  st: new Date(seq).toISOString(),
});

const result = (id: string, seq: number): SearchResultItem => ({
  record: compact(id, seq),
  matches: [{ field: 'url', preview: id, offset: 0 }],
});

it('does not let an aborted page overwrite a newer query or its pending loader', async () => {
  const filters = {
    protocols: [],
    status_ranges: [],
    content_types: [],
    conditions: [],
    client_ips: [],
    client_apps: [],
    account_names: [],
    domains: [],
  };
  const pending: Array<(response: Response) => void> = [];
  vi.mocked(apiFetch).mockImplementation(
    () => new Promise<Response>((resolve) => pending.push(resolve)),
  );
  const page = (id: string) =>
    new Response(
      `event: result\ndata: ${JSON.stringify(result(id, 1))}\n\nevent: done\ndata: ${JSON.stringify({ total_searched: 1, total_matched: 1, has_more: false, next_cursor: null, search_id: id })}\n\n`,
      { headers: { 'content-type': 'text/event-stream' } },
    );
  const store = useSearchStore;
  try {
    store.getState().reset();
    store.getState().setKeyword('old');
    store.setState({
      results: [result('old', 2)],
      nextCursor: 50,
      hasMore: true,
      isSearching: false,
    });
    const oldPage = store.getState().loadMore(filters);
    store.getState().setKeyword('new');
    store.setState({
      results: [result('new', 3)],
      nextCursor: 40,
      hasMore: true,
      isSearching: false,
    });
    const newPage = store.getState().loadMore(filters);
    pending[0](page('stale'));
    await oldPage;
    expect(store.getState().results.map((item) => item.record.id)).toEqual([
      'new',
    ]);
    expect(store.getState().isLoadingMore).toBe(true);
    pending[1](page('current'));
    await newPage;
    expect(store.getState().results.map((item) => item.record.id)).toEqual([
      'new',
      'current',
    ]);
    expect(store.getState().isLoadingMore).toBe(false);
  } finally {
    store.getState().reset();
    vi.mocked(apiFetch).mockReset();
  }
});

describe('mergeLiveSearchResults', () => {
  it('promotes new matches, demotes changed non-matches, deletes, and preserves descending order', () => {
    const merged = mergeLiveSearchResults(
      [result('old-match', 10), result('demoted', 9), result('deleted', 8)],
      ['demoted', 'new-match'],
      [result('new-match', 11)],
      ['deleted'],
    );

    expect(merged.results.map((item) => item.record.id)).toEqual([
      'new-match',
      'old-match',
    ]);
    expect(merged.knownMatchDelta).toBe(-1);
  });

  it('replaces pending records without duplicates and applies the retention floor', () => {
    const merged = mergeLiveSearchResults(
      [result('pending', 20), result('expired', 4), result('kept', 15)],
      ['pending'],
      [result('pending', 20), result('pending', 20)],
      [],
      10,
    );

    expect(merged.results.map((item) => item.record.id)).toEqual([
      'pending',
      'kept',
    ]);
    expect(new Set(merged.results.map((item) => item.record.id)).size).toBe(2);
  });

  it('keeps the live result window bounded to the newest 1000 records', () => {
    const replacements = Array.from({ length: 1200 }, (_, index) =>
      result(`record-${index + 1}`, index + 1),
    );
    const merged = mergeLiveSearchResults([], [], replacements, []);

    expect(merged.results).toHaveLength(1000);
    expect(merged.results[0].record.seq).toBe(1200);
    expect(merged.results.at(-1)?.record.seq).toBe(201);
  });
});

describe('coalesceLiveSearchMutation', () => {
  it('bounds a busy search mutation backlog and marks overflow for a full refresh', () => {
    const accumulator = coalesceLiveSearchMutation(
      createLiveSearchMutationAccumulator(),
      {
        reset: false,
        insertedIds: Array.from(
          { length: 1_200 },
          (_, index) => `insert-${index}`,
        ),
        updatedIds: [],
        deletedIds: [],
      },
    );

    expect(accumulator.changedIds.size).toBe(MAX_LIVE_SEARCH_RECORD_IDS);
    expect(accumulator.deletedIds.size).toBe(0);
    expect(accumulator.incomplete).toBe(true);
  });

  it('deduplicates changed and deleted IDs while keeping the latest retention floor', () => {
    const accumulator = createLiveSearchMutationAccumulator();
    coalesceLiveSearchMutation(accumulator, {
      reset: false,
      insertedIds: ['same', 'inserted'],
      updatedIds: ['same'],
      deletedIds: ['deleted'],
      oldestSequenceFloor: 10,
    });
    coalesceLiveSearchMutation(accumulator, {
      reset: false,
      insertedIds: [],
      updatedIds: ['inserted'],
      deletedIds: ['deleted'],
      oldestSequenceFloor: 20,
    });

    expect([...accumulator.changedIds]).toEqual(['same', 'inserted']);
    expect([...accumulator.deletedIds]).toEqual(['deleted']);
    expect(accumulator.oldestSequenceFloor).toBe(20);
    expect(accumulator.incomplete).toBe(false);
  });
});
