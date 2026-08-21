import { expect, type Locator, type Page } from "@playwright/test";
import { getDefaultRemoteBaseUrl } from "../../../src/api/sync";
import { backendPort } from "./admin-helpers";

type JsonPrimitive = string | number | boolean | null;
type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

export const MAX_BODY_PROBE_MARK_LABELS = ["Off", "16KB", "64KB", "256KB", "1MB"] as const;

const DEFAULT_REMOTE_BASE_URL = getDefaultRemoteBaseUrl();

const sliderLayoutMetrics = {
  timestamp: 0,
  memory_used: 128 * 1024 * 1024,
  memory_total: 16 * 1024 * 1024 * 1024,
  cpu_usage: 0,
  total_requests: 0,
  active_connections: 0,
  bytes_sent: 0,
  bytes_received: 0,
  bytes_sent_rate: 0,
  bytes_received_rate: 0,
  qps: 0,
  max_qps: 0,
  max_bytes_sent_rate: 0,
  max_bytes_received_rate: 0,
  http: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  https: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  tunnel: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  ws: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  wss: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  h3: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  h3s: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  socks5: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
};

const sliderLayoutTlsConfig = {
  enable_tls_interception: false,
  intercept_exclude: [],
  intercept_include: [],
  app_intercept_exclude: [],
  app_intercept_include: [],
  ip_intercept_exclude: [],
  ip_intercept_include: [],
  unsafe_ssl: false,
  disconnect_on_config_change: true,
};

const sliderLayoutPerformanceConfig = {
  traffic: {
    max_records: 10000,
    max_db_size_bytes: 1024 * 1024 * 1024,
    max_body_memory_size: 64 * 1024,
    max_body_buffer_size: 10 * 1024 * 1024,
    max_body_probe_size: 64 * 1024,
    super_performance_mode: false,
    binary_traffic_performance_mode: true,
    inject_bifrost_badge: true,
    file_retention_days: 7,
  },
  breakpoint: {
    timeout_ms: 30000,
    timeout_min_ms: 5000,
    timeout_max_ms: 300000,
  },
  body_store_stats: null,
  frame_store_stats: null,
  ws_payload_store_stats: null,
  resource_alerts: {
    overall_level: "ok",
    body_stream_writers: null,
    ws_payload_writers: null,
  },
};

const sliderLayoutApiResponses = new Map<string, JsonValue>(
  [
    [
      "GET /api/auth/status",
      {
        remote_access_enabled: false,
        auth_required: false,
        username: "",
        has_password: false,
        locked_out: false,
        failed_attempts: 0,
        max_attempts: 5,
        min_password_length: 8,
      },
    ],
    ["GET /api/config/performance", sliderLayoutPerformanceConfig],
    ["GET /api/config/tls", sliderLayoutTlsConfig],
    [
      "GET /api/config",
      {
        tls: sliderLayoutTlsConfig,
        tray: {
          enabled: false,
          supported: true,
          system_stats_supported: true,
          show_system_stats: false,
          system_stats_items: {
            cpu: true,
            memory: true,
            disk: true,
            upload: true,
            download: true,
          },
        },
        port: backendPort,
        host: "127.0.0.1",
      },
    ],
    [
      "GET /api/proxy/address",
      {
        port: backendPort,
        local_ips: ["127.0.0.1"],
        addresses: [
          {
            ip: "127.0.0.1",
            address: `127.0.0.1:${backendPort}`,
            qrcode_url: "",
            is_preferred: true,
          },
        ],
      },
    ],
    [
      "GET /api/system/overview",
      {
        system: {
          version: "0.0.0-test",
          device_name: "mock",
          os: "macos",
          arch: "arm64",
          cpu_logical_cores: 8,
          cpu_physical_cores: 4,
          memory_total_bytes: sliderLayoutMetrics.memory_total,
          memory_available_bytes:
            sliderLayoutMetrics.memory_total - sliderLayoutMetrics.memory_used,
          uptime_secs: 1,
          pid: 1,
        },
        metrics: sliderLayoutMetrics,
        rules: { total: 0, enabled: 0 },
        traffic: { recorded: 0 },
        server: {
          port: backendPort,
          admin_url: `http://127.0.0.1:${backendPort}/_bifrost/`,
        },
        pending_authorizations: 0,
      },
    ],
    [
      "GET /api/rules/active-summary",
      {
        rules: [],
        variable_conflicts: [],
        merged_content: "",
      },
    ],
    [
      "GET /api/sync/status",
      {
        enabled: false,
        auto_sync: false,
        remote_base_url: DEFAULT_REMOTE_BASE_URL,
        has_session: false,
        reachable: false,
        authorized: false,
        syncing: false,
        reason: "disabled",
        providers: [],
      },
    ],
    [
      "GET /api/system/version-check",
      {
        has_update: false,
        current_version: "0.0.0-test",
        latest_version: "0.0.0-test",
        release_highlights: [],
        release_url: null,
      },
    ],
    [
      "GET /api/system/upgrade/progress",
      {
        phase: "idle",
        percent: null,
        message: "",
        target_version: null,
        source: null,
        error: null,
        updated_at: "2026-08-21T00:00:00Z",
      },
    ],
    [
      "GET /api/proxy/system",
      {
        supported: true,
        enabled: false,
        host: "127.0.0.1",
        port: backendPort,
        bypass: "localhost,127.0.0.1",
        managed_by_bifrost: false,
        configured_enabled: false,
      },
    ],
    [
      "GET /api/proxy/cli",
      {
        enabled: false,
        shell: "zsh",
        config_files: [],
        proxy_url: `http://127.0.0.1:${backendPort}`,
      },
    ],
    [
      "GET /api/config/ui",
      {
        pinnedFilters: [],
        filterPanel: {
          collapsed: false,
          width: 220,
          collapsedSections: {
            pinned: false,
            clientIp: false,
            proxyPort: false,
            clientApp: false,
            accountName: false,
            domain: false,
          },
        },
        detailPanelCollapsed: false,
        rulesSortMode: "file_order",
      },
    ],
    ["GET /api/scripts", { request: [], response: [], decode: [], parser: [] }],
    [
      "GET /api/syntax",
      {
        protocols: [],
        protocol_aliases: {},
        template_variables: [],
        filter_specs: [],
        scripts: {
          request_scripts: [],
          response_scripts: [],
          decode_scripts: [],
          parser_scripts: [],
        },
      },
    ],
    [
      "GET /api/whitelist",
      {
        mode: "allow_all",
        allow_lan: false,
        whitelist: [],
        temporary_whitelist: [],
        pending: [],
        userpass_enabled: false,
        userpass: {
          enabled: false,
          accounts: [],
          loopback_requires_auth: false,
        },
        loopback_requires_auth: false,
        accounts: [],
        session_denied: [],
      },
    ],
    ["GET /api/whitelist/pending", []],
    ["GET /api/config/ip-tls/pending", []],
    ["GET /api/remote-invoke/pairings/pending", { pairings: [] }],
    ["GET /api/notifications/unread-count", { unread_count: 0 }],
  ] satisfies Array<[string, JsonValue]>,
);

function getApiPath(url: string): string | null {
  const path = new URL(url).pathname;
  if (path.startsWith("/_bifrost/api/")) {
    return path.slice("/_bifrost".length);
  }
  if (path.startsWith("/api/")) {
    return path;
  }
  return null;
}

export async function routeSliderLayoutApis(page: Page): Promise<void> {
  await page.route(/.*\/(?:_bifrost\/)?api\/.*/, async (route) => {
    const apiPath = getApiPath(route.request().url());
    if (!apiPath) {
      await route.fallback();
      return;
    }

    const method = route.request().method();
    await route.fulfill({
      json: sliderLayoutApiResponses.get(`${method} ${apiPath}`) ?? {},
    });
  });
}

export async function expectNoHorizontalOverlap(
  leftLocator: Locator,
  rightLocator: Locator,
): Promise<void> {
  const [leftBox, rightBox] = await Promise.all([
    leftLocator.boundingBox(),
    rightLocator.boundingBox(),
  ]);
  expect(leftBox).not.toBeNull();
  expect(rightBox).not.toBeNull();
  if (!leftBox || !rightBox) {
    return;
  }

  expect(leftBox.x + leftBox.width).toBeLessThanOrEqual(rightBox.x - 4);
}
