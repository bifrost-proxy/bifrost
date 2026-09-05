import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { message } from 'antd';
import {
  DEFAULT_TIMEOUT_MS,
  type ReplayGroup,
  type ReplayRequest,
  type ReplayRequestSummary,
  type ReplayHistory,
  type ReplayExecuteResponse,
  type RuleConfig,
  type ReplayKeyValueItem,
  type ReplayBody,
  type TrafficRecord,
  type SSEEvent,
  type WebSocketMessage,
  type StreamingConnection,
  type SessionTargetSearchState,
  type DisplayFormat,
} from '../types';
import * as replayApi from '../api/replay';
import * as trafficApi from '../api/traffic';
import { pushService } from '../services/pushService';
import { apiFetch } from '../api/apiFetch';
import { getClientId } from '../services/clientId';
import { buildApiUrl, buildWsUrl } from '../runtime';
import { isConnectionIssueError, notifyApiBusinessError } from '../api/client';

import type { RequestType } from '../types';

export type RequestPanelTab = 'params' | 'headers' | 'cookies' | 'body' | 'history';
export type ResponsePanelTab = 'Body' | 'Header' | 'Set-Cookie' | 'Matched Rules' | 'Messages';
export type ResponseViewMode = 'pretty' | 'raw' | 'preview';
export type ResponseContentType = 'json' | 'xml' | 'html' | 'javascript' | 'css' | 'text';
export type ReplayMode = 'composer' | 'history';
export type ReplayHistoryFilter =
  | { type: 'all' }
  | { type: 'unbound' }
  | { type: 'request'; requestId: string };

interface ReplayUIState {
  mode: ReplayMode;
  requestType: RequestType;
  requestPanelActiveTab: RequestPanelTab;
  responsePanelActiveTab: ResponsePanelTab;
  responseViewMode: ResponseViewMode;
  responseContentType: ResponseContentType | null;
  saveModalVisible: boolean;
  saveName: string;
  ruleSelectVisible: boolean;
  collectionPanelSection: 'collections' | 'history';
  collectionSearchText: string;
  collectionExpandedKeys: string[];
  historySearchText: string;
  historyPage: number;
  historyPageSize: number;
  selectedHistoryId: string | null;
  wsMessageInput: string;
  selectedRequestId: string | null;
  responsePanelSearch: SessionTargetSearchState;
  responsePanelDisplayFormat: DisplayFormat;
  messagesPanelSearch: string;
  messagesPanelSearchMode: 'highlight' | 'filter';
  messagesPanelFullscreen: boolean;
  messagesPanelFollowTail: boolean;
}

interface ReplayState {
  currentRequest: ReplayRequest | null;
  savedRequests: ReplayRequestSummary[];
  recentHistory: ReplayHistory[];
  allHistory: ReplayHistory[];
  groups: ReplayGroup[];
  historyFilter: ReplayHistoryFilter;
  deletingRequestIds: string[];

  currentResponse: ReplayExecuteResponse | null;
  currentTrafficRecord: TrafficRecord | null;

  selectedHistoryRecord: TrafficRecord | null;
  historyRequestBody: string | null;
  historyResponseBody: string | null;
  historyDetailLoading: boolean;

  ruleConfig: RuleConfig;
  timeoutMs: number;

  loading: boolean;
  executing: boolean;
  responsePanelCollapsed: boolean;

  requestsTotal: number;
  historyTotal: number;
  allHistoryTotal: number;

  streamingConnection: StreamingConnection | null;
  sseEvents: SSEEvent[];
  wsMessages: WebSocketMessage[];
  eventSourceRef: EventSource | null;
  webSocketRef: WebSocket | null;
  abortControllerRef: AbortController | null;

  uiState: ReplayUIState;

  createNewRequest: () => void;
  applySavedRequestsSnapshot: (requests: ReplayRequestSummary[], total: number) => void;
  applyGroupsSnapshot: (groups: ReplayGroup[]) => void;
  updateCurrentRequest: (updates: Partial<ReplayRequest>) => void;
  saveRequest: (name?: string, groupId?: string) => Promise<boolean>;
  executeRequest: () => Promise<void>;
  loadSavedRequests: () => Promise<void>;
  loadRecentHistory: (filter?: ReplayHistoryFilter) => Promise<void>;
  loadAllHistory: (page?: number, pageSize?: number) => Promise<void>;
  loadGroups: () => Promise<void>;
  createGroup: (name: string) => Promise<boolean>;
  deleteGroup: (id: string) => Promise<boolean>;
  updateGroup: (id: string, name: string) => Promise<boolean>;
  moveRequest: (requestId: string, groupId: string | null) => Promise<boolean>;
  deleteRequest: (id: string) => Promise<boolean>;
  deleteHistory: (id: string) => Promise<boolean>;
  clearHistory: (requestId?: string) => Promise<boolean>;
  setRuleConfig: (config: RuleConfig) => void;
  setTimeoutMs: (timeout: number) => void;
  selectRequest: (request: ReplayRequestSummary | ReplayRequest) => Promise<void>;
  selectHistory: (history: ReplayHistory) => Promise<void>;
  selectHistoryForDetail: (history: ReplayHistory) => Promise<void>;
  importFromTraffic: (trafficId: string) => Promise<void>;
  reuseSelectedHistory: () => void;
  setResponsePanelCollapsed: (collapsed: boolean) => void;
  setHistoryFilter: (filter: ReplayHistoryFilter) => void;
  updateUIState: (updates: Partial<ReplayUIState>) => void;
  connectSSE: () => void;
  disconnectSSE: () => void;
  connectWebSocket: () => void;
  disconnectWebSocket: () => void;
  sendWebSocketMessage: (data: string, type?: 'text' | 'binary') => void;
  clearStreamingMessages: () => void;
  cancelRequest: () => void;
  reset: () => void;
}

function generateId(): string {
  return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
}

const defaultUIState: ReplayUIState = {
  mode: 'composer',
  requestType: 'http',
  requestPanelActiveTab: 'params',
  responsePanelActiveTab: 'Body',
  responseViewMode: 'pretty',
  responseContentType: null,
  saveModalVisible: false,
  saveName: '',
  ruleSelectVisible: false,
  collectionPanelSection: 'collections',
  collectionSearchText: '',
  collectionExpandedKeys: ['saved', 'ungrouped'],
  historySearchText: '',
  historyPage: 1,
  historyPageSize: 50,
  selectedHistoryId: null,
  wsMessageInput: '',
  selectedRequestId: null,
  responsePanelSearch: { value: '', total: 0, show: false },
  responsePanelDisplayFormat: 'HighLight',
  messagesPanelSearch: '',
  messagesPanelSearchMode: 'highlight',
  messagesPanelFullscreen: false,
  messagesPanelFollowTail: true,
};

function createEmptyRequest(requestType: RequestType = 'http'): ReplayRequest {
  const now = Date.now();
  return {
    id: generateId(),
    request_type: requestType,
    method: 'GET',
    url: '',
    headers: [
      { id: generateId(), key: 'Content-Type', value: 'application/json', enabled: true },
      { id: generateId(), key: 'Accept', value: '*/*', enabled: true },
    ],
    body: { type: 'none' },
    is_saved: false,
    sort_order: 0,
    created_at: now,
    updated_at: now,
  };
}

function handleReplayLoadError(error: unknown, fallback: string) {
  if (isConnectionIssueError(error)) {
    return;
  }

  notifyApiBusinessError(error, fallback);
}

function toHistoryApiParams(
  filter: ReplayHistoryFilter,
  limit: number,
  offset: number,
): { request_id?: string; binding?: 'unbound'; limit: number; offset: number } {
  if (filter.type === 'request') {
    return { request_id: filter.requestId, limit, offset };
  }
  if (filter.type === 'unbound') {
    return { binding: 'unbound', limit, offset };
  }
  return { limit, offset };
}

export function buildReplayRequestFromTrafficRecord(
  record: TrafficRecord,
  requestBody: string | null,
): { request: ReplayRequest; requestType: RequestType } {
  const headers: ReplayKeyValueItem[] = (record.request_headers || [])
    .filter(([key]) => {
      const normalized = key.trim().toLowerCase();
      return normalized !== 'content-encoding' && normalized !== 'content-length';
    })
    .map(([key, value]) => ({
      id: generateId(),
      key,
      value,
      enabled: true,
    }));

  let body: ReplayBody = { type: 'none' };
  if (requestBody) {
    const contentType = record.request_content_type || '';
    if (contentType.includes('json')) {
      body = { type: 'raw', raw_type: 'json', content: requestBody };
    } else if (contentType.includes('xml')) {
      body = { type: 'raw', raw_type: 'xml', content: requestBody };
    } else if (contentType.includes('form-urlencoded')) {
      const params = new URLSearchParams(requestBody);
      const formData: ReplayKeyValueItem[] = [];
      params.forEach((value, key) => {
        formData.push({ id: generateId(), key, value, enabled: true });
      });
      body = { type: 'x-www-form-urlencoded', form_data: formData };
    } else {
      body = { type: 'raw', raw_type: 'text', content: requestBody };
    }
  }

  const now = Date.now();
  const requestType =
    record.url?.toLowerCase().startsWith('ws://') || record.url?.toLowerCase().startsWith('wss://')
      ? 'websocket'
      : 'http';

  return {
    request: {
      id: generateId(),
      request_type: requestType,
      method: record.method,
      url: record.url,
      headers,
      body,
      is_saved: false,
      sort_order: 0,
      created_at: now,
      updated_at: now,
    },
    requestType,
  };
}

export const useReplayStore = create<ReplayState>()(
  persist(
    (set, get) => ({
      currentRequest: createEmptyRequest(),
      savedRequests: [],
      recentHistory: [],
      allHistory: [],
      groups: [],
      historyFilter: { type: 'all' },
      deletingRequestIds: [],
      currentResponse: null,
      currentTrafficRecord: null,
      selectedHistoryRecord: null,
      historyRequestBody: null,
      historyResponseBody: null,
      historyDetailLoading: false,
      ruleConfig: { mode: 'enabled' },
      timeoutMs: DEFAULT_TIMEOUT_MS,
      loading: false,
      executing: false,
      responsePanelCollapsed: true,
      requestsTotal: 0,
      historyTotal: 0,
      allHistoryTotal: 0,
      streamingConnection: null,
      sseEvents: [],
      wsMessages: [],
      eventSourceRef: null,
      webSocketRef: null,
      abortControllerRef: null,
      uiState: { ...defaultUIState },

      createNewRequest: () => {
        const { uiState, disconnectSSE, disconnectWebSocket } = get();
        disconnectSSE();
        disconnectWebSocket();
        set({
          currentRequest: createEmptyRequest(),
          currentResponse: null,
          currentTrafficRecord: null,
          recentHistory: [],
          allHistory: [],
          allHistoryTotal: 0,
          streamingConnection: null,
          sseEvents: [],
          wsMessages: [],
          historyFilter: { type: 'unbound' },
          uiState: {
            ...uiState,
            selectedRequestId: null,
            selectedHistoryId: null,
            mode: 'composer',
            requestType: 'http',
            historyPage: 1,
          },
        });

        void get().loadRecentHistory({ type: 'unbound' });
        void get().loadAllHistory(1, get().uiState.historyPageSize);
      },

      applySavedRequestsSnapshot: (savedRequests, requestsTotal) => {
        const { currentRequest, uiState } = get();
        const selectedRequestId = uiState.selectedRequestId;
        const selectedRequestStillExists =
          !!selectedRequestId && savedRequests.some((item) => item.id === selectedRequestId);

        const nextState: Partial<ReplayState> = {
          savedRequests,
          requestsTotal,
          loading: false,
        };

        if (selectedRequestId && !selectedRequestStillExists) {
          nextState.uiState = {
            ...uiState,
            selectedRequestId: null,
            selectedHistoryId: null,
          };

          if (currentRequest?.id === selectedRequestId) {
            nextState.currentRequest = createEmptyRequest();
            nextState.currentResponse = null;
            nextState.currentTrafficRecord = null;
            nextState.recentHistory = [];
          }
        }

        set(nextState);
      },

      applyGroupsSnapshot: (groups) => {
        set({ groups });
      },

      updateCurrentRequest: (updates) => {
        const { currentRequest } = get();
        if (!currentRequest) return;

        set({
          currentRequest: {
            ...currentRequest,
            ...updates,
            updated_at: Date.now(),
          },
        });
      },

      saveRequest: async (name?: string, groupId?: string) => {
        const { currentRequest } = get();
        if (!currentRequest) return false;

        try {
          set({ loading: true });

          let savedRequest = currentRequest;
          if (currentRequest.is_saved) {
            savedRequest = await replayApi.updateRequest(currentRequest.id, {
              name: name || currentRequest.name,
              request_type: currentRequest.request_type,
              method: currentRequest.method,
              url: currentRequest.url,
              headers: currentRequest.headers,
              body: currentRequest.body,
              group_id: groupId !== undefined ? groupId : currentRequest.group_id,
            });
          } else {
            const saved = await replayApi.createRequest({
              name: name || `Request ${Date.now()}`,
              request_type: currentRequest.request_type,
              method: currentRequest.method,
              url: currentRequest.url,
              headers: currentRequest.headers,
              body: currentRequest.body,
              is_saved: true,
              group_id: groupId,
            });
            savedRequest = saved;
          }

          const nextHistoryFilter: ReplayHistoryFilter = {
            type: 'request',
            requestId: savedRequest.id,
          };
          const nextUiState = {
            ...get().uiState,
            selectedRequestId: savedRequest.id,
            selectedHistoryId: null,
            historyPage: 1,
          };

          set({
            currentRequest: savedRequest,
            recentHistory: [],
            historyTotal: 0,
            allHistory: [],
            allHistoryTotal: 0,
            historyFilter: nextHistoryFilter,
            uiState: nextUiState,
          });

          await get().loadRecentHistory(nextHistoryFilter);
          if (nextUiState.mode === 'history') {
            await get().loadAllHistory(1, nextUiState.historyPageSize);
          }

          message.success('Request saved');
          return true;
        } catch (e) {
          message.error(`Failed to save: ${e}`);
          return false;
        } finally {
          set({ loading: false });
        }
      },

      executeRequest: async () => {
        const { currentRequest, ruleConfig, timeoutMs, loadRecentHistory, disconnectSSE, disconnectWebSocket, uiState } = get();
        if (!currentRequest || !currentRequest.url) {
          set({
            currentResponse: {
              traffic_id: '',
              status: 0,
              headers: [],
              duration_ms: 0,
              applied_rules: [],
              error: 'Please enter a URL',
            },
            responsePanelCollapsed: false,
          });
          return;
        }

        disconnectSSE();
        disconnectWebSocket();

        let bodyContent: string | undefined;
        if (currentRequest.body) {
          if (currentRequest.body.type === 'raw' && currentRequest.body.content) {
            bodyContent = currentRequest.body.content;
          } else if (currentRequest.body.type === 'x-www-form-urlencoded' && currentRequest.body.form_data) {
            const params = new URLSearchParams();
            currentRequest.body.form_data
              .filter(item => item.enabled && item.key)
              .forEach(item => params.append(item.key, item.value));
            bodyContent = params.toString();
          } else if (currentRequest.body.type === 'form-data' && currentRequest.body.form_data) {
            const formData: Record<string, string> = {};
            currentRequest.body.form_data
              .filter(item => item.enabled && item.key)
              .forEach(item => { formData[item.key] = item.value; });
            bodyContent = JSON.stringify(formData);
          }
        }

        const headers = currentRequest.headers
          .filter(h => h.enabled)
          .map(h => [h.key, h.value] as [string, string]);

        try {
          const abortController = new AbortController();
          set({
            executing: true,
            responsePanelCollapsed: false,
            streamingConnection: null,
            sseEvents: [],
            wsMessages: [],
            currentResponse: null,
            currentTrafficRecord: null,
            abortControllerRef: abortController,
          });

          const timeoutId = setTimeout(() => abortController.abort(), timeoutMs + 5000);

          const response = await apiFetch('/_bifrost/api/replay/execute/unified', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              url: currentRequest.url,
              method: currentRequest.method || 'GET',
              headers,
              body: bodyContent,
              request_id: currentRequest.is_saved ? currentRequest.id : undefined,
              rule_config: ruleConfig,
              timeout_ms: timeoutMs,
            }),
            signal: abortController.signal,
          });

          clearTimeout(timeoutId);

          if (!response.ok) {
            const errorText = await response.text();
            throw new Error(`Request failed: ${response.status} ${errorText}`);
          }

          const contentType = response.headers.get('content-type') || '';
          const isSSE = contentType.includes('text/event-stream');

          if (isSSE) {
            set({ uiState: { ...uiState, responsePanelActiveTab: 'Messages' } });

            const connectionId = `sse-${Date.now()}`;
            const connection: StreamingConnection = {
              id: connectionId,
              type: 'sse',
              status: 'connected',
              url: currentRequest.url,
              startedAt: Date.now(),
            };

            const reader = response.body?.getReader();
            if (!reader) {
              throw new Error('No response body');
            }

            const sseController = {
              close: () => {
                abortController.abort();
                reader.cancel().catch(() => { });
              },
            };

            set({
              eventSourceRef: sseController as unknown as EventSource,
              streamingConnection: connection,
            });

            let buffer = '';
            let isRunning = true;

            const appendSseEvent = (sseEvent: SSEEvent) => {
              set((state) => ({ sseEvents: [...state.sseEvents, sseEvent] }));
            };

            const processSseLine = (rawLine: string) => {
              const line = rawLine.trimEnd();
              if (!line || !line.startsWith('data: ')) {
                return;
              }

              const data = line.substring(6);
              try {
                const eventData = JSON.parse(data);
                if (eventData.type_ === 'connection') {
                  const { streamingConnection } = get();
                  if (streamingConnection) {
                    set({
                      streamingConnection: {
                        ...streamingConnection,
                        trafficId: eventData.traffic_id,
                        appliedUrl: eventData.applied_url,
                        appliedRules: eventData.applied_rules,
                      },
                    });
                  }
                  return;
                }

                appendSseEvent({
                  id: eventData.id,
                  event: eventData.event || eventData.type_,
                  data:
                    typeof eventData.data === 'string'
                      ? eventData.data
                      : JSON.stringify(eventData.data),
                  timestamp: Date.now(),
                });
              } catch {
                appendSseEvent({
                  data,
                  timestamp: Date.now(),
                });
              }
            };

            const processStream = async () => {
              try {
                while (isRunning) {
                  const { done, value } = await reader.read();
                  if (done) {
                    if (buffer.trim().length > 0) {
                      processSseLine(buffer);
                      buffer = '';
                    }
                    break;
                  }

                  buffer += new TextDecoder().decode(value);
                  const lines = buffer.split('\n');
                  buffer = lines.pop() || '';

                  for (const line of lines) {
                    processSseLine(line);
                  }
                }
              } catch (e) {
                if ((e as Error).name !== 'AbortError') {
                  console.error('SSE stream error:', e);
                }
              } finally {
                isRunning = false;
                const { streamingConnection } = get();
                if (streamingConnection?.status === 'connected') {
                  set({
                    streamingConnection: { ...streamingConnection, status: 'disconnected', endedAt: Date.now() },
                    eventSourceRef: null,
                    executing: false,
                  });
                } else {
                  set({ executing: false });
                }
              }
            };

            processStream();
          } else {
            const result = await response.json();
            if (result.success && result.data) {
              set({ currentResponse: result.data });

              if (result.data.traffic_id) {
                try {
                  const trafficRecord = await trafficApi.getTrafficDetail(result.data.traffic_id);
                  set({ currentTrafficRecord: trafficRecord });
                } catch {
                  // ignore
                }
              }

              if (currentRequest.is_saved) {
                await loadRecentHistory({ type: 'request', requestId: currentRequest.id });
              } else {
                await loadRecentHistory({ type: 'unbound' });
              }

              const { historyFilter, uiState: nextUiState, loadAllHistory } = get();
              const shouldRefreshAllHistory =
                historyFilter.type === 'all' ||
                (historyFilter.type === 'request' &&
                  currentRequest.is_saved &&
                  historyFilter.requestId === currentRequest.id) ||
                (historyFilter.type === 'unbound' && !currentRequest.is_saved);

              if (shouldRefreshAllHistory) {
                await loadAllHistory(1, nextUiState.historyPageSize);
              }
            } else {
              throw new Error(result.error || 'Unknown error');
            }
            set({ executing: false, abortControllerRef: null });
          }
        } catch (e) {
          const errorMessage = e instanceof Error ? e.message : String(e);
          const isCancelled = e instanceof Error && e.name === 'AbortError';
          set({
            executing: false,
            abortControllerRef: null,
            currentResponse: isCancelled ? null : {
              traffic_id: '',
              status: 0,
              headers: [],
              duration_ms: 0,
              applied_rules: [],
              error: errorMessage,
            },
          });
        }
      },

      loadSavedRequests: async () => {
        try {
          set({ loading: true });
          const response = await replayApi.listRequests({ saved: true, limit: 100 });
          set({
            savedRequests: response.requests,
            requestsTotal: response.total,
          });
        } catch (e) {
          handleReplayLoadError(e, 'Failed to load requests');
        } finally {
          set({ loading: false });
        }
      },

      loadRecentHistory: async (filter = get().historyFilter) => {
        try {
          const response = await replayApi.listHistory(toHistoryApiParams(filter, 50, 0));
          set({
            recentHistory: response.history,
            historyTotal: response.total,
          });
        } catch (e) {
          handleReplayLoadError(e, 'Failed to load history');
        }
      },

      loadAllHistory: async (page, pageSize) => {
        try {
          set({ loading: true });
          const { historyFilter, uiState } = get();
          const nextPage = page ?? uiState.historyPage;
          const nextPageSize = pageSize ?? uiState.historyPageSize;
          const response = await replayApi.listHistory(
            toHistoryApiParams(
              historyFilter,
              nextPageSize,
              (nextPage - 1) * nextPageSize,
            ),
          );
          set({
            allHistory: response.history,
            allHistoryTotal: response.total,
            uiState: {
              ...get().uiState,
              historyPage: nextPage,
              historyPageSize: nextPageSize,
            },
          });
        } catch (e) {
          handleReplayLoadError(e, 'Failed to load history');
        } finally {
          set({ loading: false });
        }
      },

      loadGroups: async () => {
        try {
          const groups = await replayApi.listGroups();
          set({ groups });
        } catch (e) {
          handleReplayLoadError(e, 'Failed to load groups');
        }
      },

      createGroup: async (name: string) => {
        try {
          const group = await replayApi.createGroup(name);
          set((state) => ({
            groups: state.groups.some((item) => item.id === group.id)
              ? state.groups.map((item) => (item.id === group.id ? group : item))
              : [...state.groups, group],
          }));
          message.success('Folder created');
          return true;
        } catch (e) {
          message.error(`Failed to create folder: ${e}`);
          return false;
        }
      },

      deleteGroup: async (id: string) => {
        try {
          await replayApi.deleteGroup(id);
          const { groups } = get();
          set({ groups: groups.filter(g => g.id !== id) });
          message.success('Folder deleted');
          return true;
        } catch (e) {
          message.error(`Failed to delete folder: ${e}`);
          return false;
        }
      },

      updateGroup: async (id: string, name: string) => {
        try {
          await replayApi.updateGroup(id, { name });
          const { groups } = get();
          set({
            groups: groups.map(g => g.id === id ? { ...g, name } : g),
          });
          message.success('Folder renamed');
          return true;
        } catch (e) {
          message.error(`Failed to rename folder: ${e}`);
          return false;
        }
      },

      moveRequest: async (requestId: string, groupId: string | null) => {
        try {
          await replayApi.moveRequest(requestId, groupId ?? undefined);
          const { currentRequest, savedRequests } = get();
          set({
            savedRequests: savedRequests.map((item) =>
              item.id === requestId ? { ...item, group_id: groupId ?? undefined } : item,
            ),
          });
          if (currentRequest?.id === requestId) {
            set({
              currentRequest: {
                ...currentRequest,
                group_id: groupId ?? undefined,
              },
            });
          }
          message.success('Request moved');
          return true;
        } catch (e) {
          message.error(`Failed to move request: ${e}`);
          return false;
        }
      },

      deleteRequest: async (id: string) => {
        try {
          set({ deletingRequestIds: [...get().deletingRequestIds, id] });
          await replayApi.deleteRequest(id);
          const { savedRequests, currentRequest, historyFilter, uiState, loadAllHistory } = get();
          const shouldResetHistoryFilter =
            historyFilter.type === 'request' && historyFilter.requestId === id;
          const shouldResetSelection =
            uiState.selectedRequestId === id || currentRequest?.id === id;

          if (currentRequest?.id === id) {
            set({
              currentRequest: createEmptyRequest(),
              currentResponse: null,
              currentTrafficRecord: null,
              recentHistory: [],
              historyFilter: shouldResetHistoryFilter ? { type: 'all' } : historyFilter,
              uiState: {
                ...get().uiState,
                selectedRequestId: shouldResetSelection ? null : get().uiState.selectedRequestId,
                selectedHistoryId: shouldResetSelection ? null : get().uiState.selectedHistoryId,
              },
            });
          }

          set({
            savedRequests: savedRequests.filter(r => r.id !== id),
            allHistory: shouldResetHistoryFilter ? [] : get().allHistory,
            allHistoryTotal: shouldResetHistoryFilter ? 0 : get().allHistoryTotal,
            uiState: shouldResetHistoryFilter || shouldResetSelection
              ? { ...get().uiState, selectedRequestId: null, selectedHistoryId: null, historyPage: 1 }
              : get().uiState,
          });

          if (uiState.mode === 'history') {
            await loadAllHistory(1);
          }
          message.success('Request deleted');
          return true;
        } catch (e) {
          message.error(`Failed to delete: ${e}`);
          return false;
        } finally {
          set({
            deletingRequestIds: get().deletingRequestIds.filter((item) => item !== id),
          });
        }
      },

      deleteHistory: async (id: string) => {
        try {
          await replayApi.deleteHistory(id);
          const { recentHistory } = get();
          set({
            recentHistory: recentHistory.filter(h => h.id !== id),
          });
          message.success('History deleted');
          return true;
        } catch (e) {
          message.error(`Failed to delete: ${e}`);
          return false;
        }
      },

      clearHistory: async (requestId?: string) => {
        try {
          const result = await replayApi.clearHistory(requestId);
          if (result.success) {
            set({ recentHistory: [] });
            message.success(`Deleted ${result.deleted} history records`);
            return true;
          }
          return false;
        } catch (e) {
          message.error(`Failed to clear: ${e}`);
          return false;
        }
      },

      setRuleConfig: (config) => {
        set({ ruleConfig: config });
      },

      setTimeoutMs: (timeout) => {
        set({ timeoutMs: timeout });
      },

      selectRequest: async (request) => {
        try {
          if (get().deletingRequestIds.includes(request.id)) {
            return;
          }
          const { disconnectSSE, disconnectWebSocket } = get();
          disconnectSSE();
          disconnectWebSocket();
          set({ loading: true });
          const fullRequest = await replayApi.getRequest(request.id);
          const { uiState } = get();
          const url = fullRequest.url?.toLowerCase() || '';
          const requestType = (url.startsWith('ws://') || url.startsWith('wss://')) ? 'websocket' : 'http';
          set({
            currentRequest: fullRequest,
            currentResponse: null,
            currentTrafficRecord: null,
            streamingConnection: null,
            sseEvents: [],
            wsMessages: [],
            historyFilter: fullRequest.is_saved ? { type: 'request', requestId: fullRequest.id } : { type: 'all' },
            uiState: {
              ...uiState,
              selectedRequestId: fullRequest.is_saved ? fullRequest.id : null,
              selectedHistoryId: null,
              historyPage: 1,
              requestType,
            },
          });

          if (fullRequest.is_saved) {
            const { loadRecentHistory, loadAllHistory, uiState: nextUiState } = get();
            await loadRecentHistory({ type: 'request', requestId: fullRequest.id });
            if (nextUiState.mode === 'history') {
              await loadAllHistory(1);
            }
          }
        } catch (e) {
          handleReplayLoadError(e, 'Failed to load request');
        } finally {
          set({ loading: false });
        }
      },

      selectHistory: async (history) => {
        try {
          if (history.traffic_id) {
            const trafficRecord = await trafficApi.getTrafficDetail(history.traffic_id);
            set({
              currentTrafficRecord: trafficRecord,
              currentResponse: {
                traffic_id: history.traffic_id,
                status: history.status,
                headers: trafficRecord.response_headers || [],
                body: trafficRecord.response_body || undefined,
                duration_ms: history.duration_ms,
                applied_rules: trafficRecord.matched_rules || [],
              },
              responsePanelCollapsed: false,
            });
          }
        } catch (e) {
          handleReplayLoadError(e, 'Failed to load history detail');
        }
      },

      selectHistoryForDetail: async (history) => {
        const { uiState } = get();
        set({
          uiState: { ...uiState, selectedHistoryId: history.id },
          historyDetailLoading: true,
          selectedHistoryRecord: null,
          historyRequestBody: null,
          historyResponseBody: null,
        });

        try {
          if (history.traffic_id) {
            const trafficRecord = await trafficApi.getTrafficDetail(history.traffic_id);
            let requestBody: string | null = null;
            let responseBody: string | null = null;

            try {
              requestBody = await trafficApi.getRequestBody(history.traffic_id);
            } catch {
              // ignore request body fetch error
            }

            try {
              responseBody = await trafficApi.getResponseBody(history.traffic_id);
            } catch {
              // ignore response body fetch error
            }

            set({
              selectedHistoryRecord: trafficRecord,
              historyRequestBody: requestBody,
              historyResponseBody: responseBody,
            });
          }
        } catch (e) {
          handleReplayLoadError(e, 'Failed to load history detail');
        } finally {
          set({ historyDetailLoading: false });
        }
      },

      importFromTraffic: async (trafficId: string) => {
        try {
          set({ loading: true });
          const record = await trafficApi.getTrafficDetail(trafficId);
          let requestBody: string | null = null;

          try {
            requestBody = await trafficApi.getRequestBody(trafficId);
          } catch {
            // ignore request body fetch error
          }
          const { request: newRequest, requestType } = buildReplayRequestFromTrafficRecord(
            record,
            requestBody,
          );

          const { uiState } = get();
          set({
            currentRequest: newRequest,
            currentResponse: null,
            currentTrafficRecord: null,
            responsePanelCollapsed: true,
            recentHistory: [],
            allHistory: [],
            allHistoryTotal: 0,
            historyFilter: { type: 'unbound' },
            uiState: {
              ...uiState,
              selectedRequestId: null,
              selectedHistoryId: null,
              mode: 'composer',
              historyPage: 1,
              requestType,
            },
          });

          message.success('Request imported from traffic');
        } catch (e) {
          message.error(`Failed to import: ${e}`);
        } finally {
          set({ loading: false });
        }
      },

      reuseSelectedHistory: () => {
        const {
          selectedHistoryRecord,
          historyRequestBody,
          uiState,
        } = get();

        if (!selectedHistoryRecord) {
          return;
        }

        const { request: newRequest, requestType } = buildReplayRequestFromTrafficRecord(
          selectedHistoryRecord,
          historyRequestBody,
        );

        set({
          currentRequest: newRequest,
          currentResponse: null,
          currentTrafficRecord: null,
          responsePanelCollapsed: true,
          recentHistory: [],
          allHistory: [],
          allHistoryTotal: 0,
          historyFilter: { type: 'unbound' },
          uiState: {
            ...uiState,
            selectedRequestId: null,
            selectedHistoryId: null,
            mode: 'composer',
            historyPage: 1,
            requestType,
          },
        });

        message.success('Replay parameters restored from history');
      },

      setResponsePanelCollapsed: (collapsed) => {
        set({ responsePanelCollapsed: collapsed });
      },

      setHistoryFilter: (filter) => {
        const { uiState } = get();
        set({
          historyFilter: filter,
          selectedHistoryRecord: null,
          historyRequestBody: null,
          historyResponseBody: null,
          uiState: { ...uiState, selectedHistoryId: null, historyPage: 1 },
        });
      },

      updateUIState: (updates) => {
        const { uiState } = get();
        set({ uiState: { ...uiState, ...updates } });
      },

      connectSSE: async () => {
        get().executeRequest();
      },

      disconnectSSE: () => {
        const { eventSourceRef, streamingConnection } = get();
        if (eventSourceRef) {
          eventSourceRef.close();
        }
        set({
          eventSourceRef: null,
          streamingConnection: streamingConnection
            ? { ...streamingConnection, status: 'disconnected', endedAt: Date.now() }
            : null,
        });
      },

      connectWebSocket: () => {
        const { currentRequest, ruleConfig, webSocketRef, disconnectWebSocket, disconnectSSE } = get();
        if (!currentRequest?.url) {
          set({
            currentResponse: {
              traffic_id: '',
              status: 0,
              headers: [],
              duration_ms: 0,
              applied_rules: [],
              error: 'Please enter a URL',
            },
            responsePanelCollapsed: false,
          });
          return;
        }

        disconnectSSE();
        if (webSocketRef) {
          disconnectWebSocket();
        }

        set({
          streamingConnection: null,
          sseEvents: [],
          wsMessages: [],
          currentResponse: null,
          currentTrafficRecord: null,
        });

        let wsUrl = currentRequest.url;
        if (wsUrl.startsWith('http://')) {
          wsUrl = wsUrl.replace('http://', 'ws://');
        } else if (wsUrl.startsWith('https://')) {
          wsUrl = wsUrl.replace('https://', 'wss://');
        } else if (!wsUrl.startsWith('ws://') && !wsUrl.startsWith('wss://')) {
          wsUrl = `ws://${wsUrl}`;
        }

        const proxyUrl = new URL(buildApiUrl('/replay/execute/ws'));
        proxyUrl.searchParams.set('url', wsUrl);
        proxyUrl.searchParams.set('x_client_id', getClientId());
        if (currentRequest.is_saved) {
          proxyUrl.searchParams.set('request_id', currentRequest.id);
        }
        proxyUrl.searchParams.set('rule_config', JSON.stringify(ruleConfig));

        // Replace with WebSocket protocol
        const wsProxyUrl = buildWsUrl(
          '/api/replay/execute/ws',
          proxyUrl.searchParams,
        );

        const connectionId = `ws-${Date.now()}`;
        const connection: StreamingConnection = {
          id: connectionId,
          type: 'websocket',
          status: 'connecting',
          url: wsUrl,
          startedAt: Date.now(),
        };

        set({
          streamingConnection: connection,
          wsMessages: [],
          responsePanelCollapsed: false,
          uiState: { ...get().uiState, responsePanelActiveTab: 'Messages' },
        });

        try {
          const ws = new WebSocket(wsProxyUrl);

          ws.onopen = () => {
            set({
              streamingConnection: { ...connection, status: 'connected' },
            });
          };

          ws.onmessage = (event) => {
            const wsMessage: WebSocketMessage = {
              id: `msg-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
              direction: 'receive',
              type: typeof event.data === 'string' ? 'text' : 'binary',
              data: typeof event.data === 'string' ? event.data : '[Binary Data]',
              timestamp: Date.now(),
            };
            set((state) => ({ wsMessages: [...state.wsMessages, wsMessage] }));
          };

          ws.onclose = (event) => {
            const { streamingConnection } = get();
            set({
              streamingConnection: streamingConnection
                ? {
                  ...streamingConnection,
                  status: 'disconnected',
                  endedAt: Date.now(),
                  error: event.code !== 1000 ? `Closed: ${event.code} ${event.reason}` : undefined,
                }
                : null,
              webSocketRef: null,
            });
          };

          ws.onerror = () => {
            const { streamingConnection } = get();
            set({
              streamingConnection: streamingConnection
                ? {
                  ...streamingConnection,
                  status: 'error',
                  error: 'Connection error',
                  endedAt: Date.now(),
                }
                : null,
              webSocketRef: null,
            });
          };

          set({ webSocketRef: ws });
        } catch (e) {
          set({
            streamingConnection: {
              ...connection,
              status: 'error',
              error: String(e),
              endedAt: Date.now(),
            },
          });
        }
      },

      disconnectWebSocket: () => {
        const { webSocketRef, streamingConnection } = get();
        if (webSocketRef) {
          webSocketRef.close(1000, 'User disconnected');
          set({
            webSocketRef: null,
            streamingConnection: streamingConnection
              ? { ...streamingConnection, status: 'disconnected', endedAt: Date.now() }
              : null,
          });
        }
      },

      sendWebSocketMessage: (data: string, type: 'text' | 'binary' = 'text') => {
        const { webSocketRef, streamingConnection } = get();
        if (!webSocketRef || streamingConnection?.status !== 'connected') {
          return;
        }

        try {
          webSocketRef.send(data);
          const wsMessage: WebSocketMessage = {
            id: `msg-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
            direction: 'send',
            type,
            data,
            timestamp: Date.now(),
          };
          set((state) => ({ wsMessages: [...state.wsMessages, wsMessage] }));
        } catch (e) {
          if (streamingConnection) {
            set({
              streamingConnection: {
                ...streamingConnection,
                error: `Failed to send: ${e}`,
              },
            });
          }
        }
      },

      clearStreamingMessages: () => {
        set({ sseEvents: [], wsMessages: [] });
      },

      cancelRequest: () => {
        const { abortControllerRef, disconnectSSE, disconnectWebSocket } = get();
        if (abortControllerRef) {
          abortControllerRef.abort();
        }
        disconnectSSE();
        disconnectWebSocket();
        set({
          executing: false,
          abortControllerRef: null,
          streamingConnection: null,
        });
      },

      reset: () => {
        const { disconnectSSE, disconnectWebSocket } = get();
        disconnectSSE();
        disconnectWebSocket();
        set({
          currentRequest: createEmptyRequest(),
          savedRequests: [],
          recentHistory: [],
          allHistory: [],
          groups: [],
          historyFilter: { type: 'all' },
          currentResponse: null,
          currentTrafficRecord: null,
          ruleConfig: { mode: 'enabled' },
          loading: false,
          executing: false,
          streamingConnection: null,
          sseEvents: [],
          wsMessages: [],
        });
      },
    }),
    {
      name: 'bifrost-replay',
      partialize: (state) => ({
        uiState: {
          collectionExpandedKeys: state.uiState.collectionExpandedKeys,
          selectedRequestId: state.uiState.selectedRequestId,
          requestPanelActiveTab: state.uiState.requestPanelActiveTab,
          responsePanelActiveTab: state.uiState.responsePanelActiveTab,
          responseViewMode: state.uiState.responseViewMode,
        },
      }),
      merge: (persisted, current) => {
        const persistedState = persisted as Partial<ReplayState>;
        const mergedExpandedKeys = persistedState?.uiState?.collectionExpandedKeys || [];
        if (!mergedExpandedKeys.includes('ungrouped')) {
          mergedExpandedKeys.push('ungrouped');
        }
        return {
          ...current,
          uiState: {
            ...current.uiState,
            ...(persistedState?.uiState || {}),
            collectionExpandedKeys: mergedExpandedKeys,
          },
        };
      },
    },
  ),
);

pushService.onReplaySavedRequestsUpdate((data) => {
  useReplayStore.getState().applySavedRequestsSnapshot(data.requests, data.total);
});

pushService.onReplayGroupsUpdate((data) => {
  useReplayStore.getState().applyGroupsSnapshot(data.groups);
});

pushService.onReplayHistoryUpdated((data) => {
  const { currentRequest, historyFilter, uiState, loadRecentHistory, loadAllHistory } = useReplayStore.getState();

  if (currentRequest?.is_saved && currentRequest.id === data.request_id) {
    void loadRecentHistory({ type: 'request', requestId: currentRequest.id });
  } else if (!data.request_id && historyFilter.type === 'unbound') {
    void loadRecentHistory({ type: 'unbound' });
  }

  if (uiState.mode !== 'history') {
    return;
  }

  if (historyFilter.type === 'all') {
    void loadAllHistory(uiState.historyPage, uiState.historyPageSize);
    return;
  }

  if (historyFilter.type === 'unbound' && !data.request_id) {
    void loadAllHistory(uiState.historyPage, uiState.historyPageSize);
    return;
  }

  if (historyFilter.type === 'request' && historyFilter.requestId === data.request_id) {
    void loadAllHistory(uiState.historyPage, uiState.historyPageSize);
  }
});
