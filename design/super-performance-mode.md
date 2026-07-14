# Super Performance Mode

## Goal

Super Performance Mode is an opt-in runtime mode for high-throughput proxy use
cases where Bifrost should keep applying proxy rules but stop recording traffic.
It is disabled by default.

When enabled:

- Request and response rule handling still runs.
- No traffic record is created or updated.
- No request body, response body, SSE body, WebSocket frame, or WebSocket payload
  is persisted.
- Traffic query APIs return an empty result set.
- The global navigation stays visible, while the entire Network workbench to its
  right becomes a full-area status view explaining that recording is disabled.
  This covers the Network toolbar, filter panel, traffic list, and request detail
  pane, and includes a button to open Settings > Performance and highlight the
  switch.

## Configuration Surface

The canonical setting is `traffic.super_performance_mode` in `UnifiedConfig`.
The default is `false`.

Entrypoints:

- CLI startup flag: `bifrost start --super-performance-mode`
- Admin API:
  - `GET /_bifrost/api/config/performance`
  - `PUT /_bifrost/api/config/performance` with `super_performance_mode`
- Web UI: Settings > Performance, first control in the tab

The CLI flag persists `super_performance_mode=true` so daemon restarts keep the
same behavior until the user disables it from Settings or Admin API.

## Runtime Contract

The runtime bit lives in `AdminState::super_performance_mode` so request hot
paths can read it without waiting on config locks.

The following paths must short-circuit when the bit is true:

- `AdminState::record_traffic`
- `AdminState::update_traffic_by_id`
- body storage helpers in `bifrost-proxy::utils::tee`
- direct request/response body stores in HTTP and TLS-intercepted handlers
- SSE raw body streaming
- WebSocket frame and payload stores
- traffic list/query/update APIs

Metrics collection can continue because it is in-memory operational telemetry,
not a traffic history record. Rule matching, header/body modification, scripts,
mocking, forwarding, breakpoint rules, and routing behavior must not be skipped.

## UI Behavior

Settings > Performance shows "Super Performance Mode" at the top of the tab.
The description states that Bifrost will process rules but will not store any
traffic entries, bodies, frames, or DB updates.

Network stays mounted. When the mode is active, a status layer covers the entire
Network workbench to the right of the global navigation, including the top
toolbar, left filter panel, traffic list, and request detail pane. The global
navigation and global status bar remain outside the layer. The status view uses
theme tokens for its surface, text, warning icon, and action so light and dark
themes remain equivalent without a large yellow warning card. Its "Open
Performance Settings" button navigates to:

```text
/settings?tab=performance&highlight=super-performance-mode
```

The app-level data sync preloads and caches the mode before the user opens
Network. On a direct Network load where the request is still pending, Network
keeps its explicit loading state and does not claim that Super Performance Mode
is enabled. After the request resolves, Network transitions directly from
loading to either the full-area status view or the normal workbench. The filters
and traffic list must never flash between those states.

The Settings page reads the `highlight` query parameter and visually highlights
the Super Performance Mode switch.

## Verification Plan

Unit tests:

- `TrafficConfig` defaults to `super_performance_mode=false`.
- `ConfigManager::update_traffic_config` persists `super_performance_mode`.
- `AdminState::record_traffic` and `update_traffic_by_id` are no-ops in the
  mode.
- request and response body storage helpers return `None` and write no files.
- OpenAPI and CLI/API serialization expose the field.

E2E:

- Start Bifrost with `--super-performance-mode`.
- Send real proxy traffic through a local upstream.
- Assert response rule processing still modifies the response.
- Assert `GET /api/config/performance` reports the mode as enabled.
- Assert traffic list/query endpoints return zero records after requests.
- Assert body cache directories do not receive request/response payload files.

UI:

- Force the mode on through Admin API.
- Open Network and assert the status view covers the full Network workbench while
  leaving the global navigation outside it, in both light and dark themes.
- Delay the performance-config response and assert Network keeps its explicit
  loading state before the enabled status content appears.
- Click the overlay action and assert Settings > Performance opens with the
  Super Performance Mode switch highlighted and checked.

Performance:

- Run `scripts/loadtest-super-performance-mode.mjs`.
- The script runs normal mode and super mode against the same local upstream and
  request count, then writes a JSON report under `.artifacts/loadtest/`.
- Required output includes request count, concurrency, RPS, p50/p95/p99, error
  count, and final traffic record count for each mode.
- Super mode must report zero retained traffic records.

## Coverage Gate

Local full coverage is intentionally not run by default because the workspace
coverage suite is expensive. The PR CI coverage gate
`bash scripts/ci/coverage-all.sh --json --gate` remains the 90% coverage
authority for this change.
