import type {
  TrafficSummary,
  TrafficSummaryCompact,
  TrafficDeltaData,
  SystemOverview,
  MetricsSnapshot,
  PendingAuth,
  ReplayGroup,
  ReplayRequestSummary,
  WhitelistStatus,
  TrafficStatistics,
} from '../types';
import type { ScriptInfo } from '../api/scripts';
import type { ValueItem } from '../api/values';
import type { TlsConfig, PerformanceConfig, PendingIpTls } from '../api/config';
import type { CertInfo, MobileDevicesResponse, TrustProbeSession } from '../api/cert';
import type {
  CliProxyStatus,
  ProxyAddressInfo,
  SystemProxyStatus,
} from '../api/proxy';
import { getClientId } from './clientId';
import { buildWsUrl } from '../runtime';
import {
  isDesktopCoreTransitionActive,
  useDesktopCoreStore,
} from '../stores/useDesktopCoreStore';

export interface TrafficUpdatesData {
  new_records: TrafficSummary[];
  updated_records: TrafficSummary[];
  has_more: boolean;
  server_total: number;
  server_sequence?: number;
  oldest_sequence?: number | null;
}

export interface TrafficUpdatesDataCompact {
  new_records: TrafficSummaryCompact[];
  updated_records: TrafficSummaryCompact[];
  has_more: boolean;
  server_total: number;
  server_sequence: number;
}

export type { TrafficDeltaData };

export interface TrafficDeletedData {
  ids: string[];
}

export interface OverviewData {
  system: SystemOverview['system'];
  metrics: MetricsSnapshot;
  rules: { total: number; enabled: number };
  traffic: { recorded: number };
  server: { port: number; admin_url: string };
  pending_authorizations: number;
  pending_ip_tls: number;
  untrusted_clients: number;
  unread_notifications: number;
}

export interface MetricsData {
  metrics: MetricsSnapshot;
}

export interface HistoryData {
  history: MetricsSnapshot[];
}

export interface ValuesData {
  values: ValueItem[];
  total: number;
}

export interface ScriptsData {
  request: ScriptInfo[];
  response: ScriptInfo[];
  decode: ScriptInfo[];
  parser: ScriptInfo[];
}

export type SettingsScope =
  | 'proxy_settings'
  | 'tls_config'
  | 'performance_config'
  | 'cert_info'
  | 'proxy_address'
  | 'system_proxy'
  | 'cli_proxy'
  | 'whitelist_status'
  | 'pending_authorizations'
  | 'pending_ip_tls'
  | 'client_trust'
  | 'notifications'
  | 'trust_probe'
  | 'mobile_devices';

export interface SettingsUpdateData {
  scope: SettingsScope;
  data:
    | unknown
    | TlsConfig
    | PerformanceConfig
    | CertInfo
    | ProxyAddressInfo
    | SystemProxyStatus
    | CliProxyStatus
    | WhitelistStatus
    | PendingAuth[]
    | PendingIpTls[]
    | TrustProbeSession[]
    | MobileDevicesResponse;
}

export interface ReplaySavedRequestsData {
  requests: ReplayRequestSummary[];
  total: number;
  max_requests: number;
}

export interface ReplayGroupsData {
  groups: ReplayGroup[];
}

export interface ConnectedData {
  client_id: number;
  message: string;
}

export interface ErrorData {
  message: string;
}

export interface DisconnectData {
  reason: string;
}

export interface NotificationPushData {
  notification_type: string;
  title: string;
  message: string;
  metadata?: unknown;
  unread_count: number;
}

export interface ReplayRequestUpdatedData {
  action: string;
  request_id?: string;
  group_id?: string;
}

export interface ReplayHistoryUpdatedData {
  action: string;
  request_id?: string;
  history_id?: string;
}

export interface BreakpointPausedPushData {
  phase: string;
  request_id: string;
  method?: string;
  url?: string;
  status?: number;
  headers: [string, string][];
  body?: string;
  body_omitted?: boolean;
  body_size?: number;
  max_body_bytes?: number;
}

export interface BreakpointSettingsPushData {
  enabled: boolean;
  max_body_bytes: number;
}

export interface BreakpointResumedPushData {
  request_id: string;
}

export type PushMessageType =
  | 'traffic_updates'
  | 'traffic_delta'
  | 'traffic_deleted'
  | 'traffic_statistics'
  | 'overview_update'
  | 'metrics_update'
  | 'history_update'
  | 'values_update'
  | 'scripts_update'
  | 'settings_update'
  | 'replay_saved_requests_update'
  | 'replay_groups_update'
  | 'connected'
  | 'error'
  | 'disconnect'
  | 'notification'
  | 'replay_request_updated'
  | 'replay_history_updated'
  | 'breakpoint_paused'
  | 'breakpoint_settings_updated'
  | 'breakpoint_resumed';

export interface PushMessage {
  type: PushMessageType;
  data:
  | TrafficUpdatesData
  | TrafficDeltaData
  | TrafficDeletedData
  | TrafficStatistics
  | OverviewData
  | MetricsData
  | HistoryData
  | ValuesData
  | ScriptsData
  | SettingsUpdateData
  | ReplaySavedRequestsData
  | ReplayGroupsData
  | ConnectedData
  | ErrorData
  | DisconnectData
  | NotificationPushData
  | ReplayRequestUpdatedData
  | ReplayHistoryUpdatedData
  | BreakpointPausedPushData
  | BreakpointSettingsPushData
  | BreakpointResumedPushData;
}

export interface ClientSubscription {
  last_traffic_id?: string;
  last_sequence?: number;
  pending_ids?: string[];
  need_traffic?: boolean;
  need_overview?: boolean;
  need_metrics?: boolean;
  need_history?: boolean;
  need_values?: boolean;
  need_scripts?: boolean;
  need_replay_saved_requests?: boolean;
  need_replay_groups?: boolean;
  settings_scopes?: SettingsScope[];
  history_limit?: number;
  metrics_interval_ms?: number;
}

export const METRICS_INTERVAL_MIN_MS = 200;
export const METRICS_INTERVAL_MAX_MS = 5000;
export const METRICS_INTERVAL_DEFAULT_MS = 2000;
export const METRICS_INTERVAL_FAST_MS = 250;

type MessageHandler<T> = (data: T) => void;

interface PushServiceConfig {
  reconnectInterval?: number;
  maxReconnectAttempts?: number;
}

function normalizeStringArray(values?: string[]): string[] | undefined {
  if (!values || values.length === 0) {
    return undefined;
  }

  return [...new Set(values)].sort();
}

function normalizeSubscription(
  subscription: ClientSubscription,
): ClientSubscription {
  return {
    ...subscription,
    pending_ids: normalizeStringArray(subscription.pending_ids),
    settings_scopes: normalizeStringArray(subscription.settings_scopes) as
      | SettingsScope[]
      | undefined,
  };
}

function subscriptionsEqual(
  left: ClientSubscription,
  right: ClientSubscription,
): boolean {
  return JSON.stringify(normalizeSubscription(left)) === JSON.stringify(normalizeSubscription(right));
}

class PushService {
  private ws: WebSocket | null = null;
  private subscription: ClientSubscription = {};
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempts = 0;
  private config: Required<PushServiceConfig>;
  private isManualClose = false;
  private forceRefresh = false;

  private trafficHandlers: Set<MessageHandler<TrafficUpdatesData>> = new Set();
  private trafficDeltaHandlers: Set<MessageHandler<TrafficDeltaData>> = new Set();
  private trafficDeletedHandlers: Set<MessageHandler<TrafficDeletedData>> = new Set();
  private trafficStatisticsHandlers: Set<MessageHandler<TrafficStatistics>> = new Set();
  private overviewHandlers: Set<MessageHandler<OverviewData>> = new Set();
  private metricsHandlers: Set<MessageHandler<MetricsData>> = new Set();
  private historyHandlers: Set<MessageHandler<HistoryData>> = new Set();
  private valuesHandlers: Set<MessageHandler<ValuesData>> = new Set();
  private scriptsHandlers: Set<MessageHandler<ScriptsData>> = new Set();
  private settingsHandlers: Set<MessageHandler<SettingsUpdateData>> = new Set();
  private replaySavedRequestsHandlers: Set<MessageHandler<ReplaySavedRequestsData>> = new Set();
  private replayGroupsHandlers: Set<MessageHandler<ReplayGroupsData>> = new Set();
  private connectionHandlers: Set<MessageHandler<{ connected: boolean; clientId?: number }>> = new Set();
  private forceRefreshHandlers: Set<MessageHandler<DisconnectData>> = new Set();
  private notificationHandlers: Set<MessageHandler<NotificationPushData>> = new Set();
  private replayRequestHandlers: Set<MessageHandler<ReplayRequestUpdatedData>> = new Set();
  private replayHistoryHandlers: Set<MessageHandler<ReplayHistoryUpdatedData>> = new Set();
  private breakpointPausedHandlers: Set<MessageHandler<BreakpointPausedPushData>> = new Set();
  private breakpointSettingsHandlers: Set<MessageHandler<BreakpointSettingsPushData>> = new Set();
  private breakpointResumedHandlers: Set<MessageHandler<BreakpointResumedPushData>> = new Set();

  constructor(config: PushServiceConfig = {}) {
    this.config = {
      reconnectInterval: config.reconnectInterval ?? 3000,
      maxReconnectAttempts: config.maxReconnectAttempts ?? 10,
    };
  }

  connect(subscription: ClientSubscription = {}): void {
    if (this.forceRefresh) {
      return;
    }
    if (this.ws?.readyState === WebSocket.OPEN || this.ws?.readyState === WebSocket.CONNECTING) {
      this.updateSubscription(subscription);
      return;
    }

    this.subscription = subscription;
    this.isManualClose = false;
    this.createConnection();
  }

  private handleConnectionIssue(detail = 'Bifrost core is starting. Reconnecting the interface...'): void {
    useDesktopCoreStore.getState().showBooting(detail);
  }

  private shouldSuppressConnectionLogs(): boolean {
    const state = useDesktopCoreStore.getState();
    return isDesktopCoreTransitionActive() || !state.readyOnce;
  }

  private createConnection(): void {
    const params = this.buildQueryParams();
    const url = buildWsUrl('/api/push', params ? new URLSearchParams(params) : undefined);

    try {
      this.ws = new WebSocket(url);
      this.setupEventHandlers();
    } catch (error) {
      this.handleConnectionIssue();
      if (!this.shouldSuppressConnectionLogs()) {
        console.error('[PushService] Failed to create WebSocket:', error);
      }
      this.scheduleReconnect();
    }
  }

  private buildQueryParams(): string {
    const params = new URLSearchParams();

    params.append('x_client_id', getClientId());

    if (this.subscription.last_traffic_id) {
      params.append('last_traffic_id', this.subscription.last_traffic_id);
    }

    if (this.subscription.last_sequence !== undefined) {
      params.append('last_sequence', String(this.subscription.last_sequence));
    }

    if (this.subscription.pending_ids && this.subscription.pending_ids.length > 0) {
      params.append('pending_ids', this.subscription.pending_ids.join(','));
    }

    if (this.subscription.need_traffic) {
      params.append('need_traffic', 'true');
    }

    if (this.subscription.need_overview) {
      params.append('need_overview', 'true');
    }

    if (this.subscription.need_metrics) {
      params.append('need_metrics', 'true');
    }

    if (this.subscription.need_history) {
      params.append('need_history', 'true');
    }

    if (this.subscription.need_values) {
      params.append('need_values', 'true');
    }

    if (this.subscription.need_scripts) {
      params.append('need_scripts', 'true');
    }

    if (this.subscription.need_replay_saved_requests) {
      params.append('need_replay_saved_requests', 'true');
    }

    if (this.subscription.need_replay_groups) {
      params.append('need_replay_groups', 'true');
    }

    if (this.subscription.settings_scopes && this.subscription.settings_scopes.length > 0) {
      params.append('settings_scopes', this.subscription.settings_scopes.join(','));
    }

    if (this.subscription.history_limit) {
      params.append('history_limit', String(this.subscription.history_limit));
    }

    if (this.subscription.metrics_interval_ms) {
      params.append('metrics_interval_ms', String(this.subscription.metrics_interval_ms));
    }

    return params.toString();
  }

  private setupEventHandlers(): void {
    if (!this.ws) return;

    this.ws.onopen = () => {
      console.log('[PushService] Connected');
      this.reconnectAttempts = 0;
      // The connection URL only captures the subscription snapshot at create time.
      // If another store updates the subscription while the socket is CONNECTING,
      // send the latest merged subscription once the socket opens.
      this.ws?.send(JSON.stringify(this.subscription));
      useDesktopCoreStore.getState().markReady();
    };

    this.ws.onclose = (event) => {
      if (!this.isManualClose) {
        this.handleConnectionIssue();
      }
      if (!this.shouldSuppressConnectionLogs()) {
        console.log('[PushService] Disconnected:', event.code, event.reason);
      }
      this.notifyConnectionHandlers(false);
      if (!this.isManualClose) {
        this.scheduleReconnect();
      }
    };

    this.ws.onerror = (error) => {
      this.handleConnectionIssue();
      if (!this.shouldSuppressConnectionLogs()) {
        console.error('[PushService] Error:', error);
      }
    };

    this.ws.onmessage = (event) => {
      try {
        const message: PushMessage = JSON.parse(event.data);
        this.handleMessage(message);
      } catch (error) {
        console.error('[PushService] Failed to parse message:', error);
      }
    };
  }

  private handleMessage(message: PushMessage): void {
    switch (message.type) {
      case 'connected': {
        const data = message.data as ConnectedData;
        console.log('[PushService] Connected with client ID:', data.client_id);
        this.notifyConnectionHandlers(true, data.client_id);
        break;
      }
      case 'disconnect': {
        const data = message.data as DisconnectData;
        this.forceRefresh = true;
        this.isManualClose = true;
        this.forceRefreshHandlers.forEach((handler) => handler(data));
        this.disconnect();
        break;
      }
      case 'traffic_updates': {
        const data = message.data as TrafficUpdatesData;
        this.trafficHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'traffic_delta': {
        const data = message.data as TrafficDeltaData;
        this.trafficDeltaHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'traffic_deleted': {
        const data = message.data as TrafficDeletedData;
        this.trafficDeletedHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'traffic_statistics': {
        const data = message.data as TrafficStatistics;
        this.trafficStatisticsHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'overview_update': {
        const data = message.data as OverviewData;
        this.overviewHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'metrics_update': {
        const data = message.data as MetricsData;
        this.metricsHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'history_update': {
        const data = message.data as HistoryData;
        this.historyHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'values_update': {
        const data = message.data as ValuesData;
        this.valuesHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'scripts_update': {
        const data = message.data as ScriptsData;
        this.scriptsHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'settings_update': {
        const data = message.data as SettingsUpdateData;
        this.settingsHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'replay_saved_requests_update': {
        const data = message.data as ReplaySavedRequestsData;
        this.replaySavedRequestsHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'replay_groups_update': {
        const data = message.data as ReplayGroupsData;
        this.replayGroupsHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'error': {
        const data = message.data as ErrorData;
        console.error('[PushService] Server error:', data.message);
        break;
      }
      case 'notification': {
        const data = message.data as NotificationPushData;
        this.notificationHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'replay_request_updated': {
        const data = message.data as ReplayRequestUpdatedData;
        this.replayRequestHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'replay_history_updated': {
        const data = message.data as ReplayHistoryUpdatedData;
        this.replayHistoryHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'breakpoint_paused': {
        const data = message.data as BreakpointPausedPushData;
        this.breakpointPausedHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'breakpoint_settings_updated': {
        const data = message.data as BreakpointSettingsPushData;
        this.breakpointSettingsHandlers.forEach((handler) => handler(data));
        break;
      }
      case 'breakpoint_resumed': {
        const data = message.data as BreakpointResumedPushData;
        this.breakpointResumedHandlers.forEach((handler) => handler(data));
        break;
      }
    }
  }

  private notifyConnectionHandlers(connected: boolean, clientId?: number): void {
    this.connectionHandlers.forEach((handler) => handler({ connected, clientId }));
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
    }

    if (this.forceRefresh) {
      return;
    }

    if (this.reconnectAttempts >= this.config.maxReconnectAttempts) {
      console.error('[PushService] Max reconnect attempts reached');
      return;
    }

    const delay = this.config.reconnectInterval * Math.pow(1.5, this.reconnectAttempts);
    console.log(`[PushService] Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts + 1})`);

    this.reconnectTimer = setTimeout(() => {
      this.reconnectAttempts++;
      this.createConnection();
    }, delay);
  }

  disconnect(): void {
    this.isManualClose = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  disableReconnectUntilRefresh(): void {
    this.forceRefresh = true;
    this.isManualClose = true;
    this.disconnect();
  }

  disconnectIfIdle(): void {
    const hasHandlers =
      this.trafficHandlers.size > 0 ||
      this.trafficDeltaHandlers.size > 0 ||
      this.trafficStatisticsHandlers.size > 0 ||
      this.overviewHandlers.size > 0 ||
      this.metricsHandlers.size > 0 ||
      this.historyHandlers.size > 0 ||
      this.valuesHandlers.size > 0 ||
      this.scriptsHandlers.size > 0 ||
      this.settingsHandlers.size > 0 ||
      this.replaySavedRequestsHandlers.size > 0 ||
      this.replayGroupsHandlers.size > 0 ||
      this.notificationHandlers.size > 0 ||
      this.replayRequestHandlers.size > 0 ||
      this.replayHistoryHandlers.size > 0 ||
      this.breakpointPausedHandlers.size > 0 ||
      this.breakpointSettingsHandlers.size > 0 ||
      this.breakpointResumedHandlers.size > 0;
    if (!hasHandlers) {
      this.disconnect();
    }
  }

  updateSubscription(subscription: Partial<ClientSubscription>): void {
    if (this.forceRefresh) {
      return;
    }

    const nextSubscription = {
      ...this.subscription,
      ...subscription,
    };

    if (subscriptionsEqual(this.subscription, nextSubscription)) {
      return;
    }

    this.subscription = nextSubscription;

    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(this.subscription));
    }
  }

  getSubscription(): ClientSubscription {
    return { ...this.subscription };
  }

  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  onTrafficUpdates(handler: MessageHandler<TrafficUpdatesData>): () => void {
    this.trafficHandlers.add(handler);
    return () => this.trafficHandlers.delete(handler);
  }

  onTrafficDelta(handler: MessageHandler<TrafficDeltaData>): () => void {
    this.trafficDeltaHandlers.add(handler);
    return () => this.trafficDeltaHandlers.delete(handler);
  }

  onTrafficDeleted(handler: MessageHandler<TrafficDeletedData>): () => void {
    this.trafficDeletedHandlers.add(handler);
    return () => this.trafficDeletedHandlers.delete(handler);
  }

  onTrafficStatistics(handler: MessageHandler<TrafficStatistics>): () => void {
    this.trafficStatisticsHandlers.add(handler);
    return () => this.trafficStatisticsHandlers.delete(handler);
  }

  onOverviewUpdate(handler: MessageHandler<OverviewData>): () => void {
    this.overviewHandlers.add(handler);
    return () => this.overviewHandlers.delete(handler);
  }

  onMetricsUpdate(handler: MessageHandler<MetricsData>): () => void {
    this.metricsHandlers.add(handler);
    return () => this.metricsHandlers.delete(handler);
  }

  onHistoryUpdate(handler: MessageHandler<HistoryData>): () => void {
    this.historyHandlers.add(handler);
    return () => this.historyHandlers.delete(handler);
  }

  onValuesUpdate(handler: MessageHandler<ValuesData>): () => void {
    this.valuesHandlers.add(handler);
    return () => this.valuesHandlers.delete(handler);
  }

  onScriptsUpdate(handler: MessageHandler<ScriptsData>): () => void {
    this.scriptsHandlers.add(handler);
    return () => this.scriptsHandlers.delete(handler);
  }

  onSettingsUpdate(handler: MessageHandler<SettingsUpdateData>): () => void {
    this.settingsHandlers.add(handler);
    return () => this.settingsHandlers.delete(handler);
  }

  onReplaySavedRequestsUpdate(handler: MessageHandler<ReplaySavedRequestsData>): () => void {
    this.replaySavedRequestsHandlers.add(handler);
    return () => this.replaySavedRequestsHandlers.delete(handler);
  }

  onReplayGroupsUpdate(handler: MessageHandler<ReplayGroupsData>): () => void {
    this.replayGroupsHandlers.add(handler);
    return () => this.replayGroupsHandlers.delete(handler);
  }

  onConnectionChange(handler: MessageHandler<{ connected: boolean; clientId?: number }>): () => void {
    this.connectionHandlers.add(handler);
    return () => this.connectionHandlers.delete(handler);
  }

  onForceRefresh(handler: MessageHandler<DisconnectData>): () => void {
    this.forceRefreshHandlers.add(handler);
    return () => this.forceRefreshHandlers.delete(handler);
  }

  onNotification(handler: MessageHandler<NotificationPushData>): () => void {
    this.notificationHandlers.add(handler);
    return () => this.notificationHandlers.delete(handler);
  }

  onReplayRequestUpdated(handler: MessageHandler<ReplayRequestUpdatedData>): () => void {
    this.replayRequestHandlers.add(handler);
    return () => this.replayRequestHandlers.delete(handler);
  }

  onReplayHistoryUpdated(handler: MessageHandler<ReplayHistoryUpdatedData>): () => void {
    this.replayHistoryHandlers.add(handler);
    return () => this.replayHistoryHandlers.delete(handler);
  }

  onBreakpointPaused(handler: MessageHandler<BreakpointPausedPushData>): () => void {
    this.breakpointPausedHandlers.add(handler);
    return () => this.breakpointPausedHandlers.delete(handler);
  }

  onBreakpointSettingsUpdated(handler: MessageHandler<BreakpointSettingsPushData>): () => void {
    this.breakpointSettingsHandlers.add(handler);
    return () => this.breakpointSettingsHandlers.delete(handler);
  }

  onBreakpointResumed(handler: MessageHandler<BreakpointResumedPushData>): () => void {
    this.breakpointResumedHandlers.add(handler);
    return () => this.breakpointResumedHandlers.delete(handler);
  }
}

export const pushService = new PushService();
export default pushService;
