#!/usr/bin/env bash
set -euo pipefail
if [ "${BIFROST_DEVTOOLS_E2E_DEBUG:-false}" = "true" ]; then
  PS4='+ [${BASH_SOURCE[0]##*/}:${LINENO}] '
  set -x
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

ensure_playwright_browser() {
  # Check if chrome-headless-shell is installed; if not, try to install it.
  # Idempotent: fast path exits quickly when already installed.
  local bin_path
  bin_path=$(node --input-type=module -e "import('./web/node_modules/playwright-core/lib/server/registry/index.js').then(m => process.stdout.write(m.registry.findExecutable('chromium-headless-shell').executablePath() || ''))" 2>/dev/null || true)
  if [ -n "$bin_path" ] && [ -x "$bin_path" ]; then
    return 0
  fi
  echo "[INFO] Playwright chrome-headless-shell missing; installing via pnpm exec playwright install chromium-headless-shell" >&2
  (cd web && pnpm exec playwright install chromium-headless-shell) >&2 || {
    echo "[ERROR] Failed to install Playwright browser; test cannot proceed" >&2
    return 1
  }
}

pick_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

is_port_available() {
  local port="$1"
  python3 - "$port" <<'PY'
import socket
import sys

port = int(sys.argv[1])
with socket.socket() as sock:
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind(("127.0.0.1", port))
    except OSError:
        sys.exit(1)
PY
}

choose_port() {
  local name="$1"
  local requested="${2:-}"

  if [ -n "$requested" ]; then
    if ! [[ "$requested" =~ ^[0-9]+$ ]]; then
      echo "$name must be numeric, got: $requested" >&2
      exit 1
    fi
    if [ "$requested" = "9900" ]; then
      echo "Refusing to use reserved port 9900 for $name" >&2
      exit 1
    fi
    if ! is_port_available "$requested"; then
      echo "$name=$requested is already in use" >&2
      exit 1
    fi
    printf '%s\n' "$requested"
    return
  fi

  local candidate
  for _ in $(seq 1 50); do
    candidate="$(pick_port)"
    if [ "$candidate" != "9900" ] && is_port_available "$candidate"; then
      printf '%s\n' "$candidate"
      return
    fi
  done

  echo "Failed to allocate a free non-9900 port for $name" >&2
  exit 1
}

wait_for_site() {
  for _ in $(seq 1 50); do
    if ! kill -0 "$SITE_PID" >/dev/null 2>&1; then
      echo "Fixture HTTP server exited before becoming ready" >&2
      sed -n '1,120p' "$TEST_ROOT/site.log" >&2 || true
      return 1
    fi
    if curl --noproxy "*" -fsS "http://127.0.0.1:$SITE_PORT/basic.html" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done

  echo "Fixture HTTP server did not become ready on port $SITE_PORT" >&2
  sed -n '1,120p' "$TEST_ROOT/site.log" >&2 || true
  return 1
}

pid_command_contains() {
  local pid="$1"
  shift
  local command
  command="$(ps -p "$pid" -o command= 2>/dev/null || true)"
  [ -n "$command" ] || return 1
  local needle
  for needle in "$@"; do
    case "$command" in
      *"$needle"*) ;;
      *) return 1 ;;
    esac
  done
}

stop_owned_pid() {
  local name="$1"
  local pid="$2"
  shift 2

  [ -n "$pid" ] || return 0
  if ! kill -0 "$pid" >/dev/null 2>&1; then
    return 0
  fi
  if ! pid_command_contains "$pid" "$@"; then
    echo "Refusing to stop $name pid $pid because it no longer matches the expected command" >&2
    ps -p "$pid" -o pid,ppid,pgid,stat,command >&2 || true
    return 0
  fi

  kill -TERM "$pid" >/dev/null 2>&1 || true
  for _ in $(seq 1 30); do
    if ! kill -0 "$pid" >/dev/null 2>&1; then
      wait "$pid" 2>/dev/null || true
      return 0
    fi
    sleep 0.1
  done

  kill -KILL "$pid" >/dev/null 2>&1 || true
  wait "$pid" 2>/dev/null || true
}

TEST_ROOT="${TEST_ROOT:-}"
if [ -z "$TEST_ROOT" ]; then
  TEST_ROOT="$(mktemp -d -t bifrost-devtools-e2e.XXXXXX)"
fi
SITE_DIR="$TEST_ROOT/site"
mkdir -p "$SITE_DIR"

PROXY_PORT="$(choose_port PROXY_PORT "${PROXY_PORT:-}")"
SITE_PORT="$(choose_port SITE_PORT "${SITE_PORT:-}")"
HTTPS_SITE_PORT="$(choose_port HTTPS_SITE_PORT "${HTTPS_SITE_PORT:-}")"
HTTPS_HOST="${HTTPS_HOST:-$(python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    s.connect(("8.8.8.8", 80))
    print(s.getsockname()[0])
except Exception:
    print("127.0.0.1")
finally:
    s.close()
PY
)}"
if [ "$PROXY_PORT" = "$SITE_PORT" ] || [ "$PROXY_PORT" = "$HTTPS_SITE_PORT" ] || [ "$SITE_PORT" = "$HTTPS_SITE_PORT" ]; then
  echo "PROXY_PORT, SITE_PORT, and HTTPS_SITE_PORT must be different, got $PROXY_PORT/$SITE_PORT/$HTTPS_SITE_PORT" >&2
  exit 1
fi

cleanup() {
  local rc=$?
  if [ "${BIFROST_DEVTOOLS_E2E_DEBUG:-false}" = "true" ]; then
    echo "[devtools-page-bridge-e2e] cleanup rc=$rc line=${BASH_LINENO[0]:-unknown} bifrost_pid=${BIFROST_PID:-} site_pid=${SITE_PID:-}" >&2
    if [ -n "${SITE_PID:-}" ]; then
      ps -p "$SITE_PID" -o pid,ppid,pgid,stat,command >&2 || true
    fi
  fi
  if [ $rc -ne 0 ]; then
    echo "--- bifrost.log ---" >&2
    sed -n '1,260p' "$TEST_ROOT/bifrost.log" >&2 || true
    echo "--- site.log ---" >&2
    sed -n '1,120p' "$TEST_ROOT/site.log" >&2 || true
  fi
  stop_owned_pid "bifrost" "${BIFROST_PID:-}" "${BIFROST_BIN:-bifrost}" "start" "-p $PROXY_PORT"
  stop_owned_pid "https fixture" "${HTTPS_SITE_PID:-}" "node" "https_fixture.mjs"
  stop_owned_pid "http fixture" "${SITE_PID:-}" "node" "http_fixture.mjs"
  if [ $rc -ne 0 ]; then
    echo "Preserving failed test root: $TEST_ROOT" >&2
    return
  fi
  rm -rf "$TEST_ROOT" 2>/dev/null || {
    sleep 1
    rm -rf "$TEST_ROOT" 2>/dev/null || true
  }
}
handle_signal() {
  local signal="$1"
  local rc="$2"
  echo "[devtools-page-bridge-e2e] received $signal, exiting with $rc" >&2
  exit "$rc"
}
trap cleanup EXIT
trap 'handle_signal TERM 143' TERM
trap 'handle_signal INT 130' INT
if [ "${BIFROST_DEVTOOLS_E2E_DEBUG:-false}" = "true" ]; then
  trap 'echo "[devtools-page-bridge-e2e] ERR line=$LINENO status=$?" >&2' ERR
fi

cat > "$SITE_DIR/basic.html" <<'HTML'
<!doctype html>
<html>
<head>
  <title>Bifrost DevTools Basic</title>
  <script>
    window.__bifrostLoadCount = Number(sessionStorage.getItem("bifrost-load-count") || "0") + 1;
    sessionStorage.setItem("bifrost-load-count", String(window.__bifrostLoadCount));
    window.__bifrostBusinessFetchCount = 0;
    window.__bifrostBusinessFetchUrls = [];
    (function() {
      const originalFetch = window.fetch;
      if (!originalFetch) return;
      window.fetch = function(input, init) {
        const url = typeof input === "string" ? input : (input && input.url) || "";
        if (url && !String(url).includes("/_bifrost/api/devtools/bridge/")) {
          window.__bifrostBusinessFetchCount += 1;
          window.__bifrostBusinessFetchUrls.push(String(url));
        }
        return originalFetch.apply(this, arguments);
      };
    })();
    document.cookie = "bifrost-cookie-key=cookie-ready; path=/";
    localStorage.setItem("bifrost-storage-key", "storage-ready");
    sessionStorage.setItem("bifrost-session-key", "session-ready");
    console.log("bifrost-devtools-basic-ready");
    console.warn("bifrost-devtools-warning-ready");
    console.log("bifrost-console-object-ready", { pageId: "basic", nested: { answer: 42 }, items: ["alpha", "beta"] });
  </script>
</head>
<body>
  <div id="debug-fixture" data-case="basic" style="color: rgb(11, 22, 33); display: block;">ready</div>
  <img id="debug-fixture-static-resource" alt="" src="/devtools/api/static-resource?case=static-img&foo=tag">
  <script>fetch("/devtools/api/ping?case=basic").catch(function(){})</script>
</body>
</html>
HTML
printf '%s\n' '<!doctype html><html><head><title>Bifrost DevTools Secondary</title><script>console.log("bifrost-devtools-secondary-ready")</script></head><body><main id="debug-fixture-secondary" data-case="secondary">secondary</main></body></html>' > "$SITE_DIR/secondary.html"
cat > "$SITE_DIR/sw.html" <<'HTML'
<!doctype html>
<html>
<head>
  <title>Bifrost DevTools Service Worker</title>
  <script>
    (async function() {
      if (!("serviceWorker" in navigator)) {
        window.__bifrostSwUnsupported = true;
        return;
      }
      await navigator.serviceWorker.register("/sw.js");
      await navigator.serviceWorker.ready;
      if (!navigator.serviceWorker.controller) {
        location.reload();
        return;
      }
      window.__bifrostSwControlled = true;
      const script = document.createElement("script");
      script.src = "/sw-dynamic.js?case=service-worker-safe";
      script.onload = function() { window.__bifrostSwDynamicLoaded = true; };
      script.onerror = function() { window.__bifrostSwDynamicError = true; };
      document.head.appendChild(script);
    })().catch(function(error) {
      window.__bifrostSwError = String(error && error.message || error);
    });
  </script>
</head>
<body><main id="sw-fixture">service worker fixture</main></body>
</html>
HTML
cat > "$SITE_DIR/sw.js" <<'JS'
self.addEventListener("install", (event) => {
  self.skipWaiting();
});
self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});
self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (url.pathname.endsWith("/sw-dynamic.js") && url.searchParams.has("__bifrost_client_req_id")) {
    event.respondWith(new Response("window.__bifrostSwDynamicQueryLeaked = true;", {
      status: 200,
      headers: { "content-type": "application/javascript" },
    }));
  }
});
JS
cat > "$SITE_DIR/sw-dynamic.js" <<'JS'
window.__bifrostSwDynamicUrl = document.currentScript && document.currentScript.src;
JS

cat > "$TEST_ROOT/http_fixture.mjs" <<'NODEHTTP'
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const port = Number(process.env.HTTP_SITE_PORT);
const siteDir = process.env.SITE_DIR;
const requestLog = path.join(process.env.TEST_ROOT, 'http_requests.ndjson');

function contentType(filePath) {
  if (filePath.endsWith('.html')) return 'text/html; charset=utf-8';
  if (filePath.endsWith('.js')) return 'application/javascript; charset=utf-8';
  if (filePath.endsWith('.json')) return 'application/json; charset=utf-8';
  if (filePath.endsWith('.css')) return 'text/css; charset=utf-8';
  return 'application/octet-stream';
}

function resolveStaticPath(url) {
  const parsed = new URL(url || '/', `http://127.0.0.1:${port}`);
  const pathname = decodeURIComponent(parsed.pathname === '/' ? '/basic.html' : parsed.pathname);
  const relative = pathname.replace(/^\/+/, '');
  const resolved = path.resolve(siteDir, relative);
  const siteRoot = path.resolve(siteDir);
  if (resolved !== siteRoot && !resolved.startsWith(`${siteRoot}${path.sep}`)) {
    return null;
  }
  return resolved;
}

const server = http.createServer((req, res) => {
  fs.appendFileSync(requestLog, JSON.stringify({
    method: req.method,
    url: req.url,
    headers: req.headers,
  }) + '\n');

  if (req.url?.startsWith('/devtools/api/')) {
    res.writeHead(200, {
      'content-type': 'application/json',
      'x-bifrost-fixture-response': 'http-ok',
    });
    res.end(JSON.stringify({ ok: true, url: req.url }));
    return;
  }

  const filePath = resolveStaticPath(req.url);
  if (!filePath || !fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
    res.writeHead(404, {
      'content-type': 'text/plain; charset=utf-8',
      'x-bifrost-fixture-response': 'http-not-found',
    });
    res.end('not found');
    return;
  }

  res.writeHead(200, {
    'content-type': contentType(filePath),
    'x-bifrost-fixture-response': 'http-static',
  });
  res.end(fs.readFileSync(filePath));
});

server.listen(port, '127.0.0.1', () => {
  console.log(`http fixture listening on ${port}`);
});
NODEHTTP
HTTP_SITE_PORT="$SITE_PORT" SITE_DIR="$SITE_DIR" TEST_ROOT="$TEST_ROOT" \
  node "$TEST_ROOT/http_fixture.mjs" >"$TEST_ROOT/site.log" 2>&1 &
SITE_PID=$!
wait_for_site

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TEST_ROOT/https.key" \
  -out "$TEST_ROOT/https.crt" \
  -subj "/CN=devtools-tls-fixture.test" \
  -addext "subjectAltName=DNS:devtools-tls-fixture.test,DNS:localhost,IP:127.0.0.1,IP:$HTTPS_HOST" \
  -days 1 >/dev/null 2>&1
cat > "$TEST_ROOT/https_fixture.mjs" <<'NODEHTTPS'
import https from 'node:https';
import fs from 'node:fs';
import path from 'node:path';

const port = Number(process.env.HTTPS_SITE_PORT);
const siteDir = process.env.SITE_DIR;
const requestLog = path.join(process.env.TEST_ROOT, 'https_requests.ndjson');
const options = {
  key: fs.readFileSync(process.env.HTTPS_KEY),
  cert: fs.readFileSync(process.env.HTTPS_CERT),
};
const server = https.createServer(options, (req, res) => {
  fs.appendFileSync(requestLog, JSON.stringify({
    method: req.method,
    url: req.url,
    headers: req.headers,
  }) + '\n');
  if (req.url?.startsWith('/devtools/api/')) {
    res.writeHead(200, {
      'content-type': 'application/json',
      'x-bifrost-fixture-response': 'tls-ok',
    });
    res.end(JSON.stringify({ ok: true, url: req.url }));
    return;
  }
  res.writeHead(200, {
    'content-type': 'text/html; charset=utf-8',
    'x-bifrost-fixture-response': 'tls-html',
  });
  res.end(fs.readFileSync(path.join(siteDir, 'basic.html')));
});
server.listen(port, '0.0.0.0', () => {
  console.log(`https fixture listening on ${port}`);
});
NODEHTTPS
HTTPS_SITE_PORT="$HTTPS_SITE_PORT" SITE_DIR="$SITE_DIR" TEST_ROOT="$TEST_ROOT" HTTPS_KEY="$TEST_ROOT/https.key" HTTPS_CERT="$TEST_ROOT/https.crt" \
  node "$TEST_ROOT/https_fixture.mjs" >"$TEST_ROOT/https-site.log" 2>&1 &
HTTPS_SITE_PID=$!
for _ in $(seq 1 50); do
  if curl --noproxy "*" -k -fsS "https://127.0.0.1:$HTTPS_SITE_PORT/basic.html" >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
curl --noproxy "*" -k -fsS "https://127.0.0.1:$HTTPS_SITE_PORT/basic.html" >/dev/null

BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
if [ ! -x "$BIFROST_BIN" ] && [ -f "${BIFROST_BIN}.exe" ]; then
  BIFROST_BIN="${BIFROST_BIN}.exe"
fi
if [ "${SKIP_BUILD:-false}" = "true" ] && [ -x "$BIFROST_BIN" ]; then
  echo "[devtools-page-bridge-e2e] Skipping build (SKIP_BUILD=true), using $BIFROST_BIN"
else
  CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/.bifrost-devtools-target}" cargo build --bin bifrost
  BIFROST_BIN="${CARGO_TARGET_DIR:-$ROOT_DIR/.bifrost-devtools-target}/debug/bifrost"
fi

export BIFROST_DATA_DIR="$TEST_ROOT/data"
mkdir -p "$BIFROST_DATA_DIR"
BIFROST_DEVTOOLS_EVALUATE_AUDIT_CAPACITY=5 "$BIFROST_BIN" start -p "$PROXY_PORT" --unsafe-ssl --no-system-proxy >"$TEST_ROOT/bifrost.log" 2>&1 &
BIFROST_PID=$!

for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$PROXY_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done
curl -fsS "http://127.0.0.1:$PROXY_PORT/_bifrost/api/proxy/address" >/dev/null

RULE_CONTENT="$(sed "s/__SITE_PORT__/$SITE_PORT/g" e2e-tests/rules/devtools/page_bridge_basic.txt | grep -v '^#' | sed '/^$/d')"
CONTROL_RULE_CONTENT="$(sed "s/__SITE_PORT__/$SITE_PORT/g" e2e-tests/rules/devtools/page_bridge_control.txt | grep -v '^#' | sed '/^$/d')"
TLS_RULE_CONTENT="$(cat <<TLSEOF
devtools-tls-fixture.test tunnel://127.0.0.1:$HTTPS_SITE_PORT tlsIntercept://
devtools-tls-fixture.test devtools:// https://127.0.0.1:$HTTPS_SITE_PORT
TLSEOF
)"
ensure_playwright_browser
PROXY_PORT="$PROXY_PORT" SITE_PORT="$SITE_PORT" HTTPS_SITE_PORT="$HTTPS_SITE_PORT" HTTPS_HOST="$HTTPS_HOST" TEST_ROOT="$TEST_ROOT" RULE_CONTENT="$RULE_CONTENT" CONTROL_RULE_CONTENT="$CONTROL_RULE_CONTENT" TLS_RULE_CONTENT="$TLS_RULE_CONTENT" node --input-type=module <<'NODE'
import { chromium } from './web/node_modules/playwright/index.mjs';
import NodeWebSocket from './web/node_modules/ws/index.js';
import net from 'node:net';
import { createHash, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';

const proxyPort = process.env.PROXY_PORT;
const sitePort = process.env.SITE_PORT;
const httpsSitePort = process.env.HTTPS_SITE_PORT;
const httpsHost = process.env.HTTPS_HOST;
const testRoot = process.env.TEST_ROOT;
const ruleContent = process.env.RULE_CONTENT;
const controlRuleContent = process.env.CONTROL_RULE_CONTENT;
const tlsRuleContent = process.env.TLS_RULE_CONTENT;
const admin = `http://127.0.0.1:${proxyPort}/_bifrost/api`;
const webui = `http://127.0.0.1:${proxyPort}/_bifrost/`;

async function api(path, options = {}) {
  const response = await fetch(admin + path, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...(options.headers || {}),
    },
  });
  if (!response.ok) {
    throw new Error(`${options.method || 'GET'} ${path} failed: ${response.status} ${await response.text()}`);
  }
  return response.json();
}

async function waitForDevToolsPage(predicate, description, timeoutMs = 10000) {
  const startedAt = Date.now();
  let lastPages = [];
  while (Date.now() - startedAt < timeoutMs) {
    lastPages = (await api('/devtools/pages?online=true')).pages;
    const page = lastPages.find(predicate);
    if (page) return page;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`${description}: ${JSON.stringify(lastPages)}`);
}

async function roundtripCdp(webSocketUrl, messages) {
  const socket = createCdpSocket(webSocketUrl);
  const replies = [];
  socket.onmessage = (event) => replies.push(JSON.parse(event.data));
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  for (const message of messages) {
    socket.send(JSON.stringify(message));
  }
  await new Promise((resolve) => setTimeout(resolve, 700));
  socket.close();
  return replies;
}

function createCdpSocket(webSocketUrl) {
  const parsed = new URL(webSocketUrl);
  const options = parsed.port === String(proxyPort)
    ? { headers: { Origin: `http://127.0.0.1:${proxyPort}` } }
    : {};
  const socket = new NodeWebSocket(webSocketUrl, options);
  Object.defineProperty(socket, 'onmessage', {
    set(listener) {
      socket.on('message', (data) => listener({ data: data.toString('utf8') }));
    },
  });
  Object.defineProperty(socket, 'onopen', {
    set(listener) {
      socket.on('open', listener);
    },
  });
  Object.defineProperty(socket, 'onerror', {
    set(listener) {
      socket.on('error', listener);
    },
  });
  return socket;
}

function createSessionSocket(sessionId) {
  const socket = new NodeWebSocket(`ws://127.0.0.1:${proxyPort}/_bifrost/api/devtools/sessions/${sessionId}/ws`);
  Object.defineProperty(socket, 'onmessage', {
    set(listener) {
      socket.on('message', (data) => listener({ data: data.toString('utf8') }));
    },
  });
  Object.defineProperty(socket, 'onopen', {
    set(listener) {
      socket.on('open', listener);
    },
  });
  Object.defineProperty(socket, 'onerror', {
    set(listener) {
      socket.on('error', listener);
    },
  });
  return socket;
}

async function targetRuntimeCounters(page) {
  return page.evaluate(() => ({
    href: location.href,
    loadCount: Number(sessionStorage.getItem('bifrost-load-count') || '0'),
    businessFetchCount: Number(window.__bifrostBusinessFetchCount || 0),
    businessFetchUrls: Array.from(window.__bifrostBusinessFetchUrls || []),
    businessResources: performance
      .getEntriesByType('resource')
      .map((entry) => String(entry.name))
      .filter((name) => name.includes('/devtools/api/') && !name.includes('/_bifrost/api/devtools/bridge/')),
  }));
}

async function assertTargetRuntimeUnchanged(page, before, label, options = {}) {
  const after = await targetRuntimeCounters(page);
  const allowedBusinessDelta = options.allowedBusinessDelta || 0;
  if (after.href !== before.href) {
    throw new Error(`${label}: target URL changed ${JSON.stringify({ before, after })}`);
  }
  if (after.loadCount !== before.loadCount) {
    throw new Error(`${label}: WebUI action reloaded the target page ${JSON.stringify({ before, after })}`);
  }
  const businessDelta = after.businessFetchCount - before.businessFetchCount;
  if (businessDelta > allowedBusinessDelta) {
    throw new Error(`${label}: WebUI action triggered unexpected target fetches ${JSON.stringify({ before, after, businessDelta })}`);
  }
  return after;
}

async function clickDevtoolsWorkspaceTab(adminPage, name) {
  const workspace = adminPage.getByTestId('devtools-custom-workspace');
  await workspace.evaluate((node, tabName) => {
    const tabs = Array.from(node.querySelectorAll('.ant-tabs-tab'));
    const tab = tabs.find((candidate) => {
      const rect = candidate.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (candidate.textContent || '').trim() === tabName;
    });
    const target = tab?.querySelector('.ant-tabs-tab-btn') || tab;
    if (!target) throw new Error(`DevTools tab not found: ${tabName}`);
    target.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, view: window }));
    target.dispatchEvent(new MouseEvent('mouseup', { bubbles: true, cancelable: true, view: window }));
    target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, view: window }));
  }, name);
  const panelByName = {
    Elements: 'devtools-elements-panel',
    Network: 'devtools-network-panel',
    Cookies: 'devtools-cookies-panel',
    LocalStorage: 'devtools-local-storage-panel',
    SessionStorage: 'devtools-session-storage-panel',
    Console: 'devtools-console-panel',
  };
  const panelTestId = panelByName[name];
  if (panelTestId) {
    try {
      await adminPage.getByTestId(panelTestId).waitFor({ state: 'visible', timeout: 8000 });
    } catch (error) {
      const debugState = await adminPage.evaluate(() => ({
        url: location.href,
        detail: Boolean(document.querySelector('[data-testid="devtools-detail"]')),
        workspace: Boolean(document.querySelector('[data-testid="devtools-custom-workspace"]')),
        activeTab: document.querySelector('.devtools-workspace-tabs .ant-tabs-tab-active')?.textContent || '',
        tabTexts: Array.from(document.querySelectorAll('.devtools-workspace-tabs [role="tab"]')).map((node) => node.textContent || ''),
        bodyText: document.body.textContent?.slice(0, 500) || '',
      }));
      throw new Error(`Failed to switch DevTools tab to ${name}: ${JSON.stringify(debugState)} ${error.message}`);
    }
  }
}

function mergeSnapshot(base, incoming) {
  const next = base ? { ...base } : {};
  if (incoming.page) next.page = incoming.page;
  for (const key of ['console', 'network', 'storage', 'dom_snapshot', 'dom_tree']) {
    if (Object.prototype.hasOwnProperty.call(incoming, key)) next[key] = incoming[key];
  }
  return next;
}

async function collectSessionSnapshot(sessionId, scope, predicate, timeoutMs = 8000) {
  const socket = createSessionSocket(sessionId);
  let snapshot = null;
  let opened = false;
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.type === 'snapshot') {
      snapshot = mergeSnapshot(snapshot, message.snapshot);
    }
  };
  await new Promise((resolve, reject) => {
    socket.onopen = () => {
      opened = true;
      resolve();
    };
    socket.onerror = reject;
  });
  if (!opened) throw new Error('session WebSocket did not open');
  await api(`/devtools/sessions/${sessionId}/refresh`, {
    method: 'POST',
    body: JSON.stringify({ scope }),
  });
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (snapshot && predicate(snapshot)) {
      socket.close();
      return snapshot;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  socket.close();
  throw new Error(`DevTools session snapshot did not satisfy ${scope}: ${JSON.stringify(snapshot)}`);
}

async function rawWsHandshake(path, origin, token) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: '127.0.0.1', port: Number(proxyPort) });
    const key = randomBytes(16).toString('base64');
    let response = '';
    socket.setTimeout(5000, () => {
      socket.destroy();
      reject(new Error('WebSocket handshake timed out'));
    });
    socket.on('connect', () => {
      const lines = [
        `GET ${path} HTTP/1.1`,
        `Host: 127.0.0.1:${proxyPort}`,
        'Upgrade: websocket',
        'Connection: Upgrade',
        `Sec-WebSocket-Key: ${key}`,
        'Sec-WebSocket-Version: 13',
        `Origin: ${origin}`,
      ];
      if (token) lines.push(`Authorization: Bearer ${token}`);
      lines.push('', '');
      socket.write(lines.join('\r\n'));
    });
    socket.on('data', (chunk) => {
      response += chunk.toString('utf8');
      if (response.includes('\r\n\r\n')) {
        socket.end();
        const status = Number(response.match(/^HTTP\/1\.1\s+(\d+)/)?.[1] || 0);
        resolve({ status, response });
      }
    });
    socket.on('error', reject);
  });
}

function wsPathFromUrl(webSocketUrl) {
  const parsed = new URL(webSocketUrl);
  return `${parsed.pathname}${parsed.search}`;
}

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}

async function assertFlattenedCdpSession(webSocketUrl, targetPage) {
  const socket = createCdpSocket(webSocketUrl);
  const replies = [];
  socket.onmessage = (event) => replies.push(JSON.parse(event.data));
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  socket.send(JSON.stringify({ id: 201, method: 'Target.attachToTarget', params: { targetId: 'self', flatten: true } }));
  await new Promise((resolve) => setTimeout(resolve, 400));
  const attachReply = replies.find((reply) => reply.id === 201);
  const sessionId = attachReply?.result?.sessionId;
  if (!sessionId) {
    throw new Error(`AV-CDP-10 failed: attachToTarget did not return sessionId ${JSON.stringify(replies)}`);
  }
  socket.send(JSON.stringify({ id: 202, sessionId, method: 'Runtime.enable' }));
  socket.send(JSON.stringify({ id: 203, sessionId, method: 'DOM.getDocument' }));
  socket.send(JSON.stringify({ id: 204, sessionId, method: 'Network.enable' }));
  socket.send(JSON.stringify({ id: 205, sessionId, method: 'DOMStorage.getDOMStorageItems', params: { storageId: { securityOrigin: 'http://devtools-fixture.test', isLocalStorage: true } } }));
  socket.send(JSON.stringify({ id: 206, sessionId, method: 'Page.startScreencast', params: { format: 'png' } }));
  await new Promise((resolve) => setTimeout(resolve, 800));
  for (const id of [202, 203, 204, 205]) {
    const reply = replies.find((candidate) => candidate.id === id);
    if (!reply || reply.sessionId !== sessionId || reply.error) {
      throw new Error(`AV-CDP-10 failed: flattened session reply ${id} was not routed back to ${sessionId}: ${JSON.stringify(replies)}`);
    }
  }
  const screencastReply = replies.find((candidate) => candidate.id === 206);
  if (!screencastReply || screencastReply.sessionId !== sessionId || screencastReply.error?.message !== 'screencast_disabled') {
    throw new Error(`AV-CDP-15 failed: Page.startScreencast should be explicitly disabled ${JSON.stringify(replies)}`);
  }
  const sessionEvents = replies.filter((entry) => entry.sessionId === sessionId && entry.method);
  if (!sessionEvents.some((entry) => entry.method === 'Runtime.executionContextCreated')) {
    throw new Error(`AV-CDP-10 failed: Runtime event did not include flattened sessionId ${JSON.stringify(replies)}`);
  }
  const documentReply = replies.find((reply) => reply.id === 203);
  if (!JSON.stringify(documentReply?.result?.root || {}).includes('debug-fixture')) {
    throw new Error(`AV-CDP-11 failed: DOM.getDocument did not expose real page DOM ${JSON.stringify(replies)}`);
  }
  const debugNode = findDomNode(documentReply.result.root, (node) => {
    const attrs = node.attributes || [];
    for (let i = 0; i < attrs.length; i += 2) {
      if (attrs[i] === 'id' && attrs[i + 1] === 'debug-fixture') return true;
    }
    return false;
  });
  if (!debugNode?.nodeId) {
    throw new Error(`AV-CDP-11 failed: could not locate debug fixture nodeId ${JSON.stringify(documentReply)}`);
  }
  socket.send(JSON.stringify({ id: 207, sessionId, method: 'CSS.getInlineStylesForNode', params: { nodeId: debugNode.nodeId } }));
  socket.send(JSON.stringify({ id: 208, sessionId, method: 'CSS.getMatchedStylesForNode', params: { nodeId: debugNode.nodeId } }));
  socket.send(JSON.stringify({ id: 209, sessionId, method: 'CSS.getComputedStyleForNode', params: { nodeId: debugNode.nodeId } }));
  socket.send(JSON.stringify({ id: 210, sessionId, method: 'Overlay.highlightNode', params: { nodeId: debugNode.nodeId } }));
  await new Promise((resolve) => setTimeout(resolve, 500));
  await targetPage.waitForFunction(() => {
    const overlay = document.querySelector('#__bifrost_devtools_highlight__');
    return overlay && getComputedStyle(overlay).display !== 'none' && overlay.getBoundingClientRect().width > 0;
  }, null, { timeout: 4000 });
  socket.send(JSON.stringify({ id: 211, sessionId, method: 'Overlay.hideHighlight' }));
  await targetPage.waitForFunction(() => {
    const overlay = document.querySelector('#__bifrost_devtools_highlight__');
    return !overlay || getComputedStyle(overlay).display === 'none';
  }, null, { timeout: 4000 });
  socket.close();
  for (const id of [207, 208, 209, 210, 211]) {
    const reply = replies.find((candidate) => candidate.id === id);
    if (!reply || reply.sessionId !== sessionId || reply.error) {
      throw new Error(`AV-CDP-17 failed: inspect/overlay reply ${id} failed ${JSON.stringify(replies)}`);
    }
  }
  if (!JSON.stringify(replies.find((reply) => reply.id === 207)?.result || {}).includes('rgb(11, 22, 33)')) {
    throw new Error(`AV-CDP-11 failed: inline style did not expose real element style ${JSON.stringify(replies.find((reply) => reply.id === 207))}`);
  }
  if (!JSON.stringify(replies.find((reply) => reply.id === 209)?.result || {}).includes('display')) {
    throw new Error(`AV-CDP-11 failed: computed style did not expose inspectable properties ${JSON.stringify(replies.find((reply) => reply.id === 209))}`);
  }
  const storageReply = replies.find((reply) => reply.id === 205);
  if (!JSON.stringify(storageReply?.result?.entries || []).includes('bifrost-storage-key')) {
    throw new Error(`AV-CDP-11 failed: DOMStorage did not expose real localStorage ${JSON.stringify(replies)}`);
  }
  if (!sessionEvents.some((entry) => entry.method === 'Network.requestWillBeSent' && JSON.stringify(entry).includes('/devtools/api/ping'))) {
    throw new Error(`AV-CDP-11 failed: Network events did not expose page traffic ${JSON.stringify(replies)}`);
  }
  const screencastFrame = sessionEvents.find((entry) => entry.method === 'Page.screencastFrame');
  if (screencastFrame) {
    throw new Error(`AV-CDP-15 failed: screencast frame should not be emitted ${JSON.stringify(replies)}`);
  }
}

function findDomNode(node, predicate) {
  if (!node) return null;
  if (predicate(node)) return node;
  for (const child of node.children || []) {
    const found = findDomNode(child, predicate);
    if (found) return found;
  }
  return null;
}

async function assertRealtimeCdpUpdates(webSocketUrl, targetPage) {
  const socket = createCdpSocket(webSocketUrl);
  const replies = [];
  socket.onmessage = (event) => replies.push(JSON.parse(event.data));
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  socket.send(JSON.stringify({ id: 301, method: 'Target.attachToTarget', params: { targetId: 'self', flatten: true } }));
  await new Promise((resolve) => setTimeout(resolve, 300));
  const attachReply = replies.find((reply) => reply.id === 301);
  const sessionId = attachReply?.result?.sessionId;
  if (!sessionId) {
    throw new Error(`AV-CDP-12 failed: attachToTarget did not return sessionId ${JSON.stringify(replies)}`);
  }
  socket.send(JSON.stringify({ id: 302, sessionId, method: 'Runtime.enable' }));
  socket.send(JSON.stringify({ id: 303, sessionId, method: 'DOM.enable' }));
  socket.send(JSON.stringify({ id: 304, sessionId, method: 'Network.enable' }));
  socket.send(JSON.stringify({ id: 305, sessionId, method: 'Page.startScreencast', params: { format: 'png' } }));
  await new Promise((resolve) => setTimeout(resolve, 700));
  await targetPage.evaluate(() => {
    console.error('bifrost-devtools-live-console');
    localStorage.setItem('bifrost-live-key', 'live-ready');
    const marker = document.createElement('div');
    marker.id = 'debug-fixture-live';
    marker.textContent = 'live update ready';
    document.body.appendChild(marker);
    fetch('/devtools/api/ping?case=live').catch(function(){});
  });
  await new Promise((resolve) => setTimeout(resolve, 2200));
  socket.send(JSON.stringify({ id: 306, sessionId, method: 'DOM.getDocument' }));
  socket.send(JSON.stringify({ id: 307, sessionId, method: 'DOMStorage.getDOMStorageItems', params: { storageId: { securityOrigin: 'http://devtools-fixture.test', isLocalStorage: true } } }));
  await new Promise((resolve) => setTimeout(resolve, 600));
  socket.close();

  const sessionEvents = replies.filter((entry) => entry.sessionId === sessionId && entry.method);
  if (!sessionEvents.some((entry) => entry.method === 'Runtime.consoleAPICalled' && JSON.stringify(entry).includes('bifrost-devtools-live-console'))) {
    throw new Error(`AV-CDP-12 failed: live console event was not pushed ${JSON.stringify(replies)}`);
  }
  if (!sessionEvents.some((entry) => entry.method === 'Network.requestWillBeSent' && JSON.stringify(entry).includes('case=live'))) {
    throw new Error(`AV-CDP-12 failed: live network event was not pushed ${JSON.stringify(replies)}`);
  }
  if (!sessionEvents.some((entry) => entry.method === 'DOM.documentUpdated')) {
    throw new Error(`AV-CDP-12 failed: live DOM update event was not pushed ${JSON.stringify(replies)}`);
  }
  const screencastReply = replies.find((reply) => reply.id === 305);
  if (!screencastReply || screencastReply.sessionId !== sessionId || screencastReply.error?.message !== 'screencast_disabled') {
    throw new Error(`AV-CDP-15 failed: live Page.startScreencast should be explicitly disabled ${JSON.stringify(replies)}`);
  }
  if (sessionEvents.some((entry) => entry.method === 'Page.screencastFrame')) {
    throw new Error(`AV-CDP-15 failed: live screencast frames should not be emitted ${JSON.stringify(replies)}`);
  }
  const documentReply = replies.find((reply) => reply.id === 306);
  if (!JSON.stringify(documentReply?.result?.root || {}).includes('debug-fixture-live')) {
    throw new Error(`AV-CDP-12 failed: live DOM.getDocument did not include mutation ${JSON.stringify(replies)}`);
  }
  const storageReply = replies.find((reply) => reply.id === 307);
  if (!JSON.stringify(storageReply?.result?.entries || []).includes('bifrost-live-key')) {
    throw new Error(`AV-CDP-12 failed: live storage snapshot did not include mutation ${JSON.stringify(replies)}`);
  }
}

async function assertDomSyncIsChangeDriven(webSocketUrl) {
  const socket = createCdpSocket(webSocketUrl);
  const replies = [];
  socket.onmessage = (event) => replies.push(JSON.parse(event.data));
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  socket.send(JSON.stringify({ id: 351, method: 'Target.attachToTarget', params: { targetId: 'self', flatten: true } }));
  await new Promise((resolve) => setTimeout(resolve, 300));
  const attachReply = replies.find((reply) => reply.id === 351);
  const sessionId = attachReply?.result?.sessionId;
  if (!sessionId) {
    throw new Error(`AV-CDP-16 failed: attachToTarget did not return sessionId ${JSON.stringify(replies)}`);
  }
  socket.send(JSON.stringify({ id: 352, sessionId, method: 'DOM.enable' }));
  await new Promise((resolve) => setTimeout(resolve, 700));
  const initialDomEvents = replies.filter((entry) => entry.sessionId === sessionId && entry.method === 'DOM.documentUpdated').length;
  await new Promise((resolve) => setTimeout(resolve, 2200));
  socket.close();
  const finalDomEvents = replies.filter((entry) => entry.sessionId === sessionId && entry.method === 'DOM.documentUpdated').length;
  if (finalDomEvents !== initialDomEvents) {
    throw new Error(`AV-CDP-16 failed: DOM sync emitted updates without page mutations, initial=${initialDomEvents}, final=${finalDomEvents}, replies=${JSON.stringify(replies)}`);
  }
}

async function assertInspectorSelectionSurvivesDomNoise(webSocketUrl, targetPage) {
  const socket = createCdpSocket(webSocketUrl);
  const replies = [];
  socket.onmessage = (event) => replies.push(JSON.parse(event.data));
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  socket.send(JSON.stringify({ id: 371, method: 'Target.attachToTarget', params: { targetId: 'self', flatten: true } }));
  await new Promise((resolve) => setTimeout(resolve, 300));
  const attachReply = replies.find((reply) => reply.id === 371);
  const sessionId = attachReply?.result?.sessionId;
  if (!sessionId) {
    throw new Error(`AV-CDP-19 failed: attachToTarget did not return sessionId ${JSON.stringify(replies)}`);
  }
  socket.send(JSON.stringify({ id: 372, sessionId, method: 'DOM.enable' }));
  socket.send(JSON.stringify({ id: 373, sessionId, method: 'DOM.getDocument' }));
  await new Promise((resolve) => setTimeout(resolve, 700));
  const documentReply = replies.find((reply) => reply.id === 373);
  const debugNode = findDomNode(documentReply?.result?.root, (node) => {
    const attrs = node.attributes || [];
    for (let i = 0; i < attrs.length; i += 2) {
      if (attrs[i] === 'id' && attrs[i + 1] === 'debug-fixture') return true;
    }
    return false;
  });
  if (!debugNode?.nodeId) {
    throw new Error(`AV-CDP-19 failed: could not locate stable debug node ${JSON.stringify(documentReply)}`);
  }

  const domEventsBeforeOverlay = replies.filter((entry) => entry.sessionId === sessionId && entry.method === 'DOM.documentUpdated').length;
  socket.send(JSON.stringify({ id: 374, sessionId, method: 'Overlay.highlightNode', params: { nodeId: debugNode.nodeId } }));
  await targetPage.waitForFunction(() => {
    const overlay = document.querySelector('#__bifrost_devtools_highlight__');
    return overlay && getComputedStyle(overlay).display !== 'none' && overlay.getBoundingClientRect().width > 0;
  }, null, { timeout: 4000 });
  await targetPage.evaluate(() => {
    const fixture = document.querySelector('#debug-fixture');
    for (let i = 0; i < 12; i += 1) {
      fixture.setAttribute('data-noise', String(i));
      fixture.style.setProperty('--bifrost-noise', String(i));
      document.body.classList.toggle('bifrost-noise-class', i % 2 === 0);
    }
  });
  await new Promise((resolve) => setTimeout(resolve, 1800));
  const domEventsAfterNoise = replies.filter((entry) => entry.sessionId === sessionId && entry.method === 'DOM.documentUpdated').length;
  if (domEventsAfterNoise !== domEventsBeforeOverlay) {
    throw new Error(`AV-CDP-19 failed: internal overlay or attribute noise triggered documentUpdated, before=${domEventsBeforeOverlay}, after=${domEventsAfterNoise}, replies=${JSON.stringify(replies)}`);
  }
  socket.send(JSON.stringify({ id: 375, sessionId, method: 'CSS.getInlineStylesForNode', params: { nodeId: debugNode.nodeId } }));
  await new Promise((resolve) => setTimeout(resolve, 500));
  const cssAfterNoise = replies.find((reply) => reply.id === 375);
  if (!cssAfterNoise || cssAfterNoise.error) {
    throw new Error(`AV-CDP-19 failed: selected nodeId became unusable after DOM noise ${JSON.stringify(replies)}`);
  }

  await targetPage.evaluate(() => {
    const marker = document.createElement('span');
    marker.id = 'debug-fixture-structural';
    marker.textContent = 'structural update';
    document.querySelector('#debug-fixture').appendChild(marker);
  });
  await new Promise((resolve) => setTimeout(resolve, 1200));
  const domEventsAfterStructuralChange = replies.filter((entry) => entry.sessionId === sessionId && entry.method === 'DOM.documentUpdated').length;
  if (domEventsAfterStructuralChange <= domEventsAfterNoise) {
    throw new Error(`AV-CDP-19 failed: external structural DOM mutation did not trigger documentUpdated ${JSON.stringify(replies)}`);
  }
  socket.send(JSON.stringify({ id: 376, sessionId, method: 'DOM.getDocument' }));
  await new Promise((resolve) => setTimeout(resolve, 500));
  const refreshedDocument = replies.find((reply) => reply.id === 376);
  if (!JSON.stringify(refreshedDocument?.result?.root || {}).includes('debug-fixture-structural')) {
    throw new Error(`AV-CDP-19 failed: structural DOM update was not visible after refresh ${JSON.stringify(replies)}`);
  }
  socket.send(JSON.stringify({ id: 377, sessionId, method: 'Overlay.hideHighlight' }));
  await new Promise((resolve) => setTimeout(resolve, 300));
  socket.close();
}

async function assertCdpProtocolMatrix(webSocketUrl, targetPage) {
  const socket = createCdpSocket(webSocketUrl);
  const replies = [];
  socket.onmessage = (event) => replies.push(JSON.parse(event.data));
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });

  const waitForIds = async (ids, timeoutMs = 7000) => {
    const startedAt = Date.now();
    while (Date.now() - startedAt < timeoutMs) {
      const seen = new Set(replies.filter((reply) => ids.includes(reply.id)).map((reply) => reply.id));
      if (ids.every((id) => seen.has(id))) return;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    const missing = ids.filter((id) => !replies.some((reply) => reply.id === id));
    throw new Error(`AV-CDP-20 failed: missing CDP replies for ids ${missing.join(',')}: ${JSON.stringify(replies)}`);
  };

  socket.send(JSON.stringify({ id: 501, method: 'Target.attachToTarget', params: { targetId: 'self', flatten: true } }));
  await waitForIds([501]);
  const sessionId = replies.find((reply) => reply.id === 501)?.result?.sessionId;
  if (!sessionId) {
    throw new Error(`AV-CDP-20 failed: Target.attachToTarget did not return sessionId ${JSON.stringify(replies)}`);
  }

  const send = (id, method, params = {}, withSession = true) => {
    socket.send(JSON.stringify({ id, ...(withSession ? { sessionId } : {}), method, params }));
    return id;
  };
  send(502, 'DOM.getDocument');
  await waitForIds([502]);
  const documentReply = replies.find((reply) => reply.id === 502);
  const debugNode = findDomNode(documentReply?.result?.root, (node) => {
    const attrs = node.attributes || [];
    for (let i = 0; i < attrs.length; i += 2) {
      if (attrs[i] === 'id' && attrs[i + 1] === 'debug-fixture') return true;
    }
    return false;
  });
  if (!debugNode?.nodeId) {
    throw new Error(`AV-CDP-20 failed: could not locate debug fixture node ${JSON.stringify(documentReply)}`);
  }

  const expectedSuccess = [];
  const success = (id, method, params = {}, validator = null, withSession = true) => {
    expectedSuccess.push({ id, method, validator, withSession });
    send(id, method, params, withSession);
  };
  const expectObject = (value) => value && typeof value === 'object' && !Array.isArray(value);
  const expectArray = (value) => Array.isArray(value);

  success(503, 'Browser.getVersion', {}, (result) => result.product === 'Bifrost DevTools Bridge', false);
  success(504, 'Target.getTargetInfo', {}, (result) => result.targetInfo?.targetId);
  success(505, 'Target.getTargets', {}, (result) => result.targetInfos?.some((target) => target.targetId));
  success(506, 'Target.setDiscoverTargets');
  success(507, 'Target.setAutoAttach', { flatten: true, autoAttach: true, waitForDebuggerOnStart: false });
  success(508, 'Target.setRemoteLocations', { locations: [] });

  success(509, 'Runtime.enable');
  success(510, 'Runtime.getIsolateId', {}, (result) => result.id === 'bifrost-page-bridge');
  success(511, 'Runtime.getHeapUsage', {}, (result) => Number.isFinite(result.usedSize) && Number.isFinite(result.totalSize));
  success(512, 'Runtime.runIfWaitingForDebugger');
  success(513, 'Runtime.releaseObjectGroup', { objectGroup: 'console' });
  success(514, 'Runtime.addBinding', { name: '__bifrostMatrixBinding' });

  success(515, 'Page.enable');
  success(516, 'Page.getFrameTree', {}, (result) => result.frameTree?.frame?.url?.includes('case=av-cdp-01'));
  success(517, 'Page.getResourceTree', {}, (result) => expectArray(result.frameTree?.resources));
  success(518, 'Page.getNavigationHistory', {}, (result) => result.entries?.some((entry) => entry.url?.includes('case=av-cdp-01')));
  success(519, 'Page.setAdBlockingEnabled', { enabled: false });

  success(520, 'DOM.enable');
  success(521, 'DOM.getFlattenedDocument', {}, (result) => result.nodes?.some((node) => JSON.stringify(node).includes('debug-fixture')));
  success(522, 'DOM.pushNodesByBackendIdsToFrontend', { backendNodeIds: [debugNode.backendNodeId || debugNode.nodeId] }, (result) => result.nodeIds?.[0] === (debugNode.backendNodeId || debugNode.nodeId));
  success(523, 'DOM.resolveNode', { nodeId: debugNode.nodeId }, (result) => result.object?.objectId === `bifrost-node-${debugNode.nodeId}`);
  success(524, 'DOM.setInspectedNode', { nodeId: debugNode.nodeId });

  success(525, 'CSS.enable');
  success(526, 'CSS.getMatchedStylesForNode', { nodeId: debugNode.nodeId }, (result) => expectArray(result.matchedCSSRules));
  success(527, 'CSS.getComputedStyleForNode', { nodeId: debugNode.nodeId }, (result) => result.computedStyle?.some((prop) => prop.name === 'color' && prop.value === 'rgb(11, 22, 33)'));
  success(528, 'CSS.getInlineStylesForNode', { nodeId: debugNode.nodeId }, (result) => JSON.stringify(result.inlineStyle || {}).includes('rgb(11, 22, 33)'));
  success(529, 'CSS.getPlatformFontsForNode', { nodeId: debugNode.nodeId }, (result) => expectArray(result.fonts));
  success(530, 'CSS.getAnimatedStylesForNode', { nodeId: debugNode.nodeId }, (result) => expectArray(result.animationStyles));
  success(531, 'CSS.getEnvironmentVariables', {}, (result) => expectArray(result.variables));
  success(532, 'CSS.trackComputedStyleUpdates', { propertiesToTrack: [] });
  success(533, 'CSS.takeComputedStyleUpdates', {}, expectObject);
  success(534, 'CSS.trackComputedStyleUpdatesForNode', { nodeId: debugNode.nodeId, propertiesToTrack: [] });

  success(535, 'Network.enable');
  success(536, 'Network.setCacheDisabled', { cacheDisabled: false });
  success(537, 'Network.setBypassServiceWorker', { bypass: false });
  success(538, 'Network.setAttachDebugStack', { enabled: false });
  success(539, 'Network.setBlockedURLs', { urls: [] });
  success(540, 'Network.emulateNetworkConditionsByRule', { offline: false, matchedNetworkConditions: [] });
  success(541, 'Network.overrideNetworkState', { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1 });
  success(542, 'Network.clearAcceptedEncodingsOverride');

  success(543, 'DOMStorage.enable');
  success(544, 'DOMStorage.getDOMStorageItems', { storageId: { securityOrigin: 'http://devtools-fixture.test', isLocalStorage: true } }, (result) => JSON.stringify(result.entries || []).includes('bifrost-storage-key'));
  success(545, 'DOMStorage.getDOMStorageItems', { storageId: { securityOrigin: 'http://devtools-fixture.test', isLocalStorage: false } }, (result) => JSON.stringify(result.entries || []).includes('bifrost-session-key'));
  success(546, 'IndexedDB.enable');
  success(547, 'IndexedDB.requestDatabaseNames', { securityOrigin: 'http://devtools-fixture.test' }, (result) => expectArray(result.databaseNames));
  success(548, 'CacheStorage.requestCacheNames', { securityOrigin: 'http://devtools-fixture.test' }, (result) => expectArray(result.caches));
  success(549, 'Storage.getStorageKey', { frameId: 'self' }, (result) => result.storageKey?.startsWith('http://devtools-fixture.test'));
  success(550, 'Storage.getStorageKeyForFrame', { frameId: 'self' }, (result) => result.storageKey?.startsWith('http://devtools-fixture.test'));
  success(551, 'Storage.getUsageAndQuota', { origin: 'http://devtools-fixture.test' }, (result) => Number.isFinite(result.usage) && expectArray(result.usageBreakdown));
  success(552, 'Storage.setStorageBucketTracking', { storageKey: 'http://devtools-fixture.test', enable: false });

  success(553, 'Log.enable');
  success(554, 'Log.startViolationsReport', { config: [] });
  success(555, 'Debugger.enable');
  success(556, 'Debugger.setPauseOnExceptions', { state: 'none' });
  success(557, 'Debugger.setAsyncCallStackDepth', { maxDepth: 0 });
  success(558, 'Debugger.setBlackboxPatterns', { patterns: [] });
  success(559, 'Overlay.enable');
  success(560, 'Overlay.setShowViewportSizeOnResize', { show: false });
  success(561, 'Overlay.setShowHinge', { hingeConfig: null });
  success(562, 'Overlay.setShowGridOverlays', { gridNodeHighlightConfigs: [] });
  success(563, 'Overlay.setShowFlexOverlays', { flexNodeHighlightConfigs: [] });
  success(564, 'Overlay.setShowScrollSnapOverlays', { scrollSnapHighlightConfigs: [] });
  success(565, 'Overlay.setShowContainerQueryOverlays', { containerQueryHighlightConfigs: [] });
  success(566, 'Overlay.setShowIsolatedElements', { isolatedElementHighlightConfigs: [] });

  success(567, 'Accessibility.enable');
  success(568, 'Performance.enable');
  success(569, 'Profiler.enable');
  success(570, 'Security.enable');
  success(571, 'Inspector.enable');
  success(572, 'ServiceWorker.enable');
  success(573, 'Audits.enable');
  success(574, 'Animation.enable');
  success(575, 'Autofill.enable');
  success(576, 'Autofill.setAddresses', { addresses: [] });
  success(577, 'Emulation.setTouchEmulationEnabled', { enabled: false });
  success(578, 'Emulation.setEmitTouchEventsForMouse', { enabled: false });
  success(579, 'Emulation.setFocusEmulationEnabled', { enabled: false });
  success(580, 'Emulation.setEmulatedMedia', { media: '' });
  success(581, 'Emulation.setEmulatedVisionDeficiency', { type: 'none' });
  success(582, 'DOMDebugger.setBreakOnCSPViolation', { violationTypes: [] });

  success(583, 'Overlay.highlightNode', { nodeId: debugNode.nodeId });
  await waitForIds(expectedSuccess.map((entry) => entry.id));
  await targetPage.waitForFunction(() => {
    const overlay = document.querySelector('#__bifrost_devtools_highlight__');
    return overlay && getComputedStyle(overlay).display !== 'none' && overlay.getBoundingClientRect().width > 0;
  }, null, { timeout: 4000 });
  success(584, 'Overlay.hideHighlight');
  success(585, 'Runtime.evaluate', { expression: 'document.title' }, (result) => result.result?.value === 'Bifrost DevTools Basic');

  const expectedErrors = [
    { id: 586, method: 'Page.startScreencast', message: 'screencast_disabled', params: { format: 'png' } },
    { id: 587, method: 'Page.stopScreencast', message: 'screencast_disabled' },
    { id: 588, method: 'Page.screencastFrameAck', message: 'screencast_disabled', params: { sessionId: 1 } },
    { id: 589, method: 'Page.captureScreenshot', message: 'unsupported CDP method' },
    { id: 590, method: 'Input.dispatchMouseEvent', message: 'unsupported CDP method', params: { type: 'mouseMoved', x: 1, y: 1 } },
    { id: 591, method: 'Debugger.setBreakpointByUrl', message: 'unsupported CDP method', params: { lineNumber: 0, url: targetPage.url() } },
    { id: 592, method: 'Network.getResponseBody', message: 'unsupported CDP method', params: { requestId: 'bifrost-network-0' } },
    { id: 593, method: 'Network.getCookies', message: 'unsupported CDP method' },
    { id: 594, method: 'Storage.getCookies', message: 'unsupported CDP method' },
    { id: 595, method: 'Security.getSecurityState', message: 'unsupported CDP method' },
    { id: 596, method: 'Profiler.start', message: 'unsupported CDP method' },
    { id: 597, method: 'HeapProfiler.enable', message: 'unsupported CDP method' },
  ];
  for (const item of expectedErrors) {
    send(item.id, item.method, item.params || {});
  }

  await waitForIds([584, 585, ...expectedErrors.map((entry) => entry.id)]);
  socket.close();

  for (const entry of expectedSuccess) {
    const reply = replies.find((candidate) => candidate.id === entry.id);
    if (!reply || reply.error || (entry.withSession && reply.sessionId !== sessionId)) {
      throw new Error(`AV-CDP-20 failed: ${entry.method} did not succeed with routed response ${JSON.stringify(reply)} all=${JSON.stringify(replies)}`);
    }
    if (entry.validator && !entry.validator(reply.result || {})) {
      throw new Error(`AV-CDP-20 failed: ${entry.method} returned incomplete result ${JSON.stringify(reply)}`);
    }
  }
  const hideReply = replies.find((candidate) => candidate.id === 584);
  if (!hideReply || hideReply.error || hideReply.sessionId !== sessionId) {
    throw new Error(`AV-CDP-20 failed: Overlay.hideHighlight did not succeed ${JSON.stringify(replies)}`);
  }
  for (const entry of expectedErrors) {
    const reply = replies.find((candidate) => candidate.id === entry.id);
    if (!reply || reply.sessionId !== sessionId || !String(reply.error?.message || '').includes(entry.message)) {
      throw new Error(`AV-CDP-20 failed: ${entry.method} should return ${entry.message} ${JSON.stringify(reply)} all=${JSON.stringify(replies)}`);
    }
  }
  if (replies.some((entry) => entry.method === 'Page.screencastFrame')) {
    throw new Error(`AV-CDP-20 failed: screencast frames must never be emitted ${JSON.stringify(replies)}`);
  }
}

const browser = await chromium.launch({ headless: true, args: ['--proxy-bypass-list=<-loopback>'] });
const context = await browser.newContext({ proxy: { server: `http://127.0.0.1:${proxyPort}` } });

const noRulePage = await context.newPage();
await noRulePage.goto(`http://127.0.0.1:${sitePort}/basic.html?case=no-rule`, { waitUntil: 'load' });
const noRuleInjected = await noRulePage.evaluate(() => Boolean(document.querySelector('#__bifrost_devtools_bridge__') || window.__BIFROST_DEVTOOLS_BRIDGE__));
if (noRuleInjected) {
  throw new Error('AV-CDP-02 failed: page without devtools:// rule was injected');
}
let pages = (await api('/devtools/pages?online=true')).pages;
if (pages.some((page) => page.url.includes('case=no-rule'))) {
  throw new Error('AV-CDP-02 failed: no-rule page appeared in DevTools page list');
}

await api('/rules', {
  method: 'POST',
  body: JSON.stringify({
    name: 'devtools-page-bridge-api',
    content: ruleContent,
    enabled: true,
  }),
});
const syntaxInfo = await api('/syntax');
const devtoolsProtocol = syntaxInfo.protocols?.find((protocol) => protocol.name === 'devtools');
if (!devtoolsProtocol || devtoolsProtocol.value_type !== 'empty') {
  throw new Error(`AV-CDP-26 failed: devtools:// should be advertised as a no-argument protocol ${JSON.stringify(devtoolsProtocol)}`);
}
if (devtoolsProtocol.example !== 'devtools://') {
  throw new Error(`AV-CDP-26 failed: syntax example should suggest bare devtools:// ${JSON.stringify(devtoolsProtocol)}`);
}
const page = await context.newPage();
await page.goto(`http://devtools-fixture.test:${sitePort}/basic.html?case=av-cdp-01`, { waitUntil: 'load' });
await page.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
const bridgeState = await page.evaluate(() => ({
  injected: Boolean(document.querySelector('#__bifrost_devtools_bridge__')),
  state: window.__BIFROST_DEVTOOLS_BRIDGE__?.state,
  pageId: window.__BIFROST_DEVTOOLS_BRIDGE__?.page_id,
}));
if (!bridgeState.injected || bridgeState.state !== 'connected' || !bridgeState.pageId) {
  throw new Error(`AV-CDP-01 failed: invalid bridge state ${JSON.stringify(bridgeState)}`);
}
await page.evaluate(() => performance.clearResourceTimings());
await page.waitForTimeout(2600);
const idleBridgeHttpEntries = await page.evaluate(() =>
  performance.getEntriesByType('resource')
    .map((entry) => String(entry.name))
    .filter((name) => name.includes('/_bifrost/api/devtools/bridge/') && !name.endsWith('/ws')),
);
if (idleBridgeHttpEntries.length !== 0) {
  throw new Error(`AV-CDP-30 failed: bridge page traffic must use WebSocket only, got HTTP entries ${JSON.stringify(idleBridgeHttpEntries)}`);
}
const bridgeLeak = await page.evaluate(() => ({
  names: Object.getOwnPropertyNames(window).filter((key) => /BIFROST/i.test(key)),
  descriptor: Object.getOwnPropertyDescriptor(window, '__BIFROST_DEVTOOLS_BRIDGE__'),
  json: JSON.stringify(window.__BIFROST_DEVTOOLS_BRIDGE__),
  keys: Object.keys(window.__BIFROST_DEVTOOLS_BRIDGE__ || {}),
}));
if (!bridgeLeak.names.includes('__BIFROST_DEVTOOLS_BRIDGE__')) {
  throw new Error(`F1 failed: bridge shim missing from page names ${JSON.stringify(bridgeLeak)}`);
}
if (bridgeLeak.descriptor?.enumerable || bridgeLeak.descriptor?.configurable || bridgeLeak.descriptor?.writable) {
  throw new Error(`F1 failed: bridge shim descriptor is not hardened ${JSON.stringify(bridgeLeak.descriptor)}`);
}
if (/token|bdt_|fetch|eval-next|eval-result/i.test(`${bridgeLeak.json} ${bridgeLeak.keys.join(',')}`)) {
  throw new Error(`F1 failed: bridge token or transport leaked to page ${JSON.stringify(bridgeLeak)}`);
}
await api('/rules', {
  method: 'POST',
  body: JSON.stringify({
    name: 'devtools-page-bridge-sw-api',
    content: `http://sw-devtools-fixture.test:${sitePort}/* devtools:// host://127.0.0.1:${sitePort}`,
    enabled: true,
  }),
});
const swPage = await context.newPage();
await swPage.goto(`http://sw-devtools-fixture.test:${sitePort}/sw.html?case=service-worker-safe`, { waitUntil: 'load' });
await swPage.waitForFunction(() => window.__bifrostSwUnsupported || window.__bifrostSwControlled, null, { timeout: 12000 });
const swUnsupported = await swPage.evaluate(() => Boolean(window.__bifrostSwUnsupported));
if (!swUnsupported) {
  await swPage.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
  await swPage.waitForFunction(() => window.__bifrostSwDynamicLoaded || window.__bifrostSwDynamicError || window.__bifrostSwDynamicQueryLeaked, null, { timeout: 8000 });
  const swState = await swPage.evaluate(() => ({
    controlled: Boolean(window.__bifrostSwControlled),
    loaded: Boolean(window.__bifrostSwDynamicLoaded),
    error: Boolean(window.__bifrostSwDynamicError),
    queryLeaked: Boolean(window.__bifrostSwDynamicQueryLeaked),
    dynamicUrl: window.__bifrostSwDynamicUrl || '',
    resources: performance.getEntriesByType('resource').map((entry) => String(entry.name)).filter((name) => name.includes('sw-dynamic.js')),
    swError: window.__bifrostSwError || '',
  }));
  if (!swState.controlled || !swState.loaded || swState.error || swState.queryLeaked || swState.dynamicUrl.includes('__bifrost_client_req_id') || swState.resources.some((name) => name.includes('__bifrost_client_req_id'))) {
    throw new Error(`AV-CDP-46 failed: Service Worker controlled dynamic tag resource should not be polluted by internal query ${JSON.stringify(swState)}`);
  }
}

await api('/rules', {
  method: 'POST',
  body: JSON.stringify({
    name: 'devtools-page-bridge-tls-api',
    content: tlsRuleContent,
    enabled: true,
  }),
});
const tlsContext = await browser.newContext({
  proxy: { server: `http://127.0.0.1:${proxyPort}` },
  ignoreHTTPSErrors: true,
});
const tlsPage = await tlsContext.newPage();
await tlsPage.goto(`https://devtools-tls-fixture.test:${httpsSitePort}/basic.html?case=tls-av-cdp-45`, { waitUntil: 'load' });
await tlsPage.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 10000 });
await tlsPage.evaluate(() => fetch('/devtools/api/tls?case=network-traffic-match&foo=tls', {
  headers: { 'x-bifrost-fixture-header': 'tls-request' },
}).catch(() => {}));
await tlsPage.waitForFunction(() => (window.__bifrostBusinessFetchUrls || []).some((url) => String(url).includes('network-traffic-match')), null, { timeout: 8000 });
await tlsPage.evaluate(() => new Promise((resolve) => {
  const img = document.createElement('img');
  img.id = 'tls-resource-image';
  img.onload = img.onerror = resolve;
  img.src = '/devtools/api/pixel?case=tls-img-resource&foo=img';
  document.body.appendChild(img);
}));
const tlsDebugPage = await waitForDevToolsPage(
  (candidate) => candidate.url.includes('case=tls-av-cdp-45') && candidate.url.startsWith('https://devtools-tls-fixture.test'),
  'AV-CDP-45 failed: TLS-intercepted DevTools page not listed',
);
const tlsSession = await api('/devtools/sessions', {
  method: 'POST',
  body: JSON.stringify({ page_id: tlsDebugPage.page_id }),
});
const tlsSnapshot = await collectSessionSnapshot(
  tlsSession.session_id,
  'network',
  (candidate) => candidate.network?.some((entry) => entry.url.includes('network-traffic-match')),
  10000,
);
const tlsNetworkEvent = tlsSnapshot.network.find((entry) => entry.url.includes('network-traffic-match'));
if (!tlsNetworkEvent) {
  throw new Error(`AV-CDP-45 failed: TLS Network event missing ${JSON.stringify(tlsSnapshot.network)}`);
}
if (tlsNetworkEvent.status !== 200 || tlsNetworkEvent.method !== 'GET' || !tlsNetworkEvent.client_req_id) {
  throw new Error(`AV-CDP-45 failed: TLS Network event missing status/method/client id ${JSON.stringify(tlsNetworkEvent)}`);
}
if (!tlsNetworkEvent.query_params?.some(([key, value]) => key === 'foo' && value === 'tls')) {
  throw new Error(`AV-CDP-45 failed: TLS Network query params missing ${JSON.stringify(tlsNetworkEvent)}`);
}
if (!tlsNetworkEvent.request_headers?.some(([key, value]) => key.toLowerCase() === 'x-bifrost-fixture-header' && value === 'tls-request')) {
  throw new Error(`AV-CDP-45 failed: TLS Network request headers missing ${JSON.stringify(tlsNetworkEvent)}`);
}
if (!tlsNetworkEvent.response_headers?.some(([key, value]) => key.toLowerCase() === 'x-bifrost-fixture-response' && value === 'tls-ok')) {
  throw new Error(`AV-CDP-45 failed: TLS Network response headers missing ${JSON.stringify(tlsNetworkEvent)}`);
}
const tlsTrafficMapping = await api(`/devtools/network/traffic/${encodeURIComponent(tlsNetworkEvent.client_req_id)}`);
const tlsTrafficRecord = await api(`/traffic/${encodeURIComponent(tlsTrafficMapping.traffic_id)}`);
if (
  tlsTrafficRecord.method !== tlsNetworkEvent.method ||
  tlsTrafficRecord.status !== tlsNetworkEvent.status ||
  !String(tlsTrafficRecord.url || '').includes('/devtools/api/tls?case=network-traffic-match&foo=tls')
) {
  throw new Error(`AV-CDP-45 failed: TLS Network event does not match mapped Traffic record ${JSON.stringify({ network: tlsNetworkEvent, traffic: tlsTrafficRecord })}`);
}
if ((tlsTrafficRecord.request_headers || []).some(([key]) => key.toLowerCase() === 'x-bifrost-client-request-id')) {
  throw new Error(`AV-CDP-45 failed: internal client request id leaked into Traffic headers ${JSON.stringify(tlsTrafficRecord.request_headers)}`);
}
const tlsServerRequests = readFileSync(`${testRoot}/https_requests.ndjson`, 'utf8')
  .trim()
  .split(/\n+/)
  .filter(Boolean)
  .map((line) => JSON.parse(line));
const tlsUpstreamRequest = tlsServerRequests.find((entry) => String(entry.url || '').includes('network-traffic-match'));
if (!tlsUpstreamRequest) {
  throw new Error(`AV-CDP-45 failed: TLS upstream fixture did not receive expected request ${JSON.stringify(tlsServerRequests)}`);
}
if (Object.keys(tlsUpstreamRequest.headers || {}).some((key) => key.toLowerCase() === 'x-bifrost-client-request-id')) {
  throw new Error(`AV-CDP-45 failed: internal client request id leaked to real TLS upstream ${JSON.stringify(tlsUpstreamRequest)}`);
}
const tlsImageSnapshot = await collectSessionSnapshot(
  tlsSession.session_id,
  'network',
  (candidate) => candidate.network?.some((entry) => entry.url.includes('tls-img-resource')),
  10000,
);
const tlsImageEvent = tlsImageSnapshot.network.find((entry) => entry.url.includes('tls-img-resource'));
if (!tlsImageEvent?.client_req_id) {
  throw new Error(`AV-CDP-45 failed: dynamic parser resource Network event should carry an internal query client id ${JSON.stringify(tlsImageEvent)}`);
}
if (String(tlsImageEvent.url || '').includes('__bifrost_client_req_id')) {
  throw new Error(`AV-CDP-45 failed: internal query id should be stripped from browser-side Network display ${JSON.stringify(tlsImageEvent)}`);
}
const tlsImageMappedTraffic = await api(`/devtools/network/traffic/${encodeURIComponent(tlsImageEvent.client_req_id)}`);
const tlsImageExpectedTrafficId = tlsImageMappedTraffic.traffic_id || tlsImageEvent.traffic_id;
const tlsImageTraffic = await api('/traffic/query', {
  method: 'POST',
  body: JSON.stringify({
    url_contains: '/devtools/api/pixel?case=tls-img-resource&foo=img',
    method: 'GET',
    limit: 20,
  }),
});
const tlsImageTrafficRecord = tlsImageTraffic.records?.find((record) =>
  record.id === tlsImageExpectedTrafficId &&
  record.p === '/devtools/api/pixel' &&
  record.s === 200 &&
  record.proto === 'https'
);
if (!tlsImageTrafficRecord) {
  throw new Error(`AV-CDP-45 failed: TLS parser resource should have matching full Traffic data ${JSON.stringify({ event: tlsImageEvent, mapped: tlsImageMappedTraffic, traffic: tlsImageTraffic.records })}`);
}
if (tlsImageMappedTraffic.traffic_id && tlsImageMappedTraffic.traffic_id !== tlsImageTrafficRecord.id) {
  throw new Error(`AV-CDP-45 failed: parser resource client id should map to exact Traffic record ${JSON.stringify({ event: tlsImageEvent, mapped: tlsImageMappedTraffic, traffic: tlsImageTrafficRecord })}`);
}
const tlsImageTrafficDetail = await api(`/traffic/${encodeURIComponent(tlsImageTrafficRecord.id)}`);
if ((tlsImageTrafficDetail.request_headers || []).some(([key]) => key.toLowerCase() === 'x-bifrost-client-request-id')) {
  throw new Error(`AV-CDP-45 failed: dynamic tag request leaked header id ${JSON.stringify(tlsImageTrafficDetail.request_headers)}`);
}
if (String(tlsImageTrafficDetail.url || '').includes('__bifrost_client_req_id')) {
  throw new Error(`AV-CDP-45 failed: dynamic tag request leaked query id to Traffic URL ${JSON.stringify(tlsImageTrafficDetail.url)}`);
}
await tlsContext.close();
await api('/rules/devtools-page-bridge-tls-api', { method: 'DELETE' });

await page.waitForTimeout(800);
pages = (await api('/devtools/pages?online=true')).pages;
const debugPage = pages.find((candidate) => candidate.url.includes('case=av-cdp-01'));
if (!debugPage) {
  throw new Error('AV-CDP-01 failed: page not listed by DevTools API');
}
if (debugPage.adapter !== 'page_bridge' || debugPage.fidelity !== 'fallback' || debugPage.state !== 'discoverable') {
  throw new Error(`AV-CDP-01 failed: wrong page state ${JSON.stringify(debugPage)}`);
}
if (debugPage.title !== 'Bifrost DevTools Basic') {
  throw new Error(`AV-CDP-01 failed: title not reported (${debugPage.title})`);
}
await page.evaluate(() => {
  window.postMessage({ type: 'hello', token: 'guess' }, '*');
  window.postMessage({ __bifrost_devtools_bridge__: true, type: 'hello', token: 'guess' }, '*');
});
await page.waitForTimeout(500);
const pagesAfterForgedPostMessage = (await api('/devtools/pages?online=true')).pages.filter((candidate) => candidate.url.includes('case=av-cdp-01'));
if (pagesAfterForgedPostMessage.length !== 1 || pagesAfterForgedPostMessage[0].page_id !== debugPage.page_id) {
  throw new Error(`F1 failed: forged postMessage changed admin-side page state ${JSON.stringify(pagesAfterForgedPostMessage)}`);
}
await page.reload({ waitUntil: 'load' });
await page.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
await page.waitForTimeout(800);
pages = (await api('/devtools/pages?online=true')).pages;
const duplicatePrimaryPages = pages.filter((candidate) => candidate.url.includes('case=av-cdp-01'));
if (duplicatePrimaryPages.length !== 1) {
  throw new Error(`AV-CDP-13 failed: reloaded page should have one online DevTools target, got ${duplicatePrimaryPages.length}: ${JSON.stringify(duplicatePrimaryPages)}`);
}
const activeDebugPage = duplicatePrimaryPages[0];
await page.evaluate(() => fetch('/secondary.html?case=ghost-fetch').then((response) => response.text()).catch(() => ''));
await page.waitForTimeout(800);
pages = (await api('/devtools/pages?online=true')).pages;
if (pages.some((candidate) => candidate.url.includes('case=ghost-fetch'))) {
  throw new Error(`AV-CDP-21 failed: fetched HTML candidate should not be listed as an online debug page ${JSON.stringify(pages)}`);
}
const targetsAfterGhostFetch = await api('/devtools/cdp/json/list');
if (targetsAfterGhostFetch.some((target) => target.url.includes('case=ghost-fetch'))) {
  throw new Error(`AV-CDP-21 failed: fetched HTML candidate should not be exposed as a CDP target ${JSON.stringify(targetsAfterGhostFetch)}`);
}
const sameUrlPage = await context.newPage();
await sameUrlPage.goto(`http://devtools-fixture.test:${sitePort}/basic.html?case=av-cdp-01`, { waitUntil: 'load' });
await sameUrlPage.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
await sameUrlPage.waitForTimeout(800);
pages = (await api('/devtools/pages?online=true')).pages;
const independentSameUrlPages = pages.filter((candidate) => candidate.url.includes('case=av-cdp-01'));
if (independentSameUrlPages.length !== 2) {
  throw new Error(`AV-CDP-13 failed: independent tabs with the same URL should stay distinct, got ${independentSameUrlPages.length}: ${JSON.stringify(independentSameUrlPages)}`);
}
await sameUrlPage.close();

const secondaryPage = await context.newPage();
await secondaryPage.goto(`http://devtools-fixture.test:${sitePort}/secondary.html?case=av-cdp-secondary`, { waitUntil: 'load' });
await secondaryPage.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
await secondaryPage.waitForTimeout(800);
pages = (await api('/devtools/pages?online=true')).pages;
const secondaryDebugPage = pages.find((candidate) => candidate.url.includes('case=av-cdp-secondary'));
if (!secondaryDebugPage) {
  throw new Error('AV-CDP-09 failed: secondary page not listed by DevTools API');
}
if (secondaryDebugPage.title !== 'Bifrost DevTools Secondary') {
  throw new Error(`AV-CDP-09 failed: wrong secondary title ${secondaryDebugPage.title}`);
}
if (secondaryDebugPage.page_id === debugPage.page_id) {
  throw new Error('AV-CDP-09 failed: secondary page reused primary page id');
}

const missingInspectorResponse = await fetch(`http://127.0.0.1:${proxyPort}/_bifrost/api/devtools/frontend/inspector.html?ws=127.0.0.1:${proxyPort}/_bifrost/api/devtools/cdp/${debugPage.page_id}`);
if (missingInspectorResponse.status !== 404) {
  throw new Error(`AV-CDP-06 failed: removed Chrome DevTools frontend endpoint should return 404 (${missingInspectorResponse.status})`);
}

const session = await api('/devtools/sessions', {
  method: 'POST',
  body: JSON.stringify({ page_id: activeDebugPage.page_id }),
});
await page.evaluate(() => fetch('/devtools/api/meta?case=network-meta&foo=bar', {
  headers: { 'x-bifrost-fixture-header': 'fixture-request' },
}).catch(() => {}));
const snapshot = await collectSessionSnapshot(
  session.session_id,
  'full',
  (candidate) =>
    candidate.dom_snapshot?.includes('id="debug-fixture"') &&
    JSON.stringify(candidate.dom_tree || {}).includes('debug-fixture') &&
    candidate.console?.some((entry) => entry.text.includes('bifrost-devtools-basic-ready')) &&
    candidate.network?.some((entry) => entry.url.includes('/devtools/api/ping')) &&
    candidate.network?.some((entry) => entry.url.includes('/devtools/api/meta?case=network-meta')) &&
    JSON.stringify(candidate.storage || {}).includes('bifrost-storage-key'),
);
if (!snapshot.dom_snapshot?.includes('id="debug-fixture"')) {
  throw new Error('AV-CDP-01 failed: DOM snapshot missing fixture');
}
if (!JSON.stringify(snapshot.dom_tree || {}).includes('debug-fixture')) {
  throw new Error('AV-CDP-11 failed: structured DOM tree missing fixture');
}
if (!snapshot.console.some((entry) => entry.text.includes('bifrost-devtools-basic-ready'))) {
  throw new Error('AV-CDP-01 failed: console message missing');
}
if (!snapshot.console.some((entry) => entry.level === 'warn' && entry.text.includes('bifrost-devtools-warning-ready'))) {
  throw new Error('AV-CDP-11 failed: warn console message missing');
}
const objectConsole = snapshot.console.find((entry) => entry.text.includes('bifrost-console-object-ready'));
if (!objectConsole?.args?.some((arg) => arg.subtype === 'array' || JSON.stringify(arg).includes('"nested"'))) {
  throw new Error(`AV-CDP-32 failed: structured console object args missing ${JSON.stringify(objectConsole)}`);
}
if (!snapshot.network.some((entry) => entry.url.includes('/devtools/api/ping'))) {
  throw new Error('AV-CDP-11 failed: network event missing');
}
const pingNetworkEvents = snapshot.network.filter((entry) => entry.url.includes('/devtools/api/ping?case=basic'));
if (pingNetworkEvents.length !== 1) {
  throw new Error(`AV-CDP-39 failed: Network list must use frontend bridge events as source of truth and not duplicate performance/Traffic events (${pingNetworkEvents.length}) ${JSON.stringify(pingNetworkEvents)}`);
}
if (!pingNetworkEvents[0].client_req_id) {
  throw new Error(`AV-CDP-39 failed: frontend-captured Network event must include x-bifrost-client-request-id mapping id ${JSON.stringify(pingNetworkEvents[0])}`);
}
const mappedPingTraffic = await api(`/devtools/network/traffic/${encodeURIComponent(pingNetworkEvents[0].client_req_id)}`);
if (!mappedPingTraffic.traffic_id) {
  throw new Error(`AV-CDP-39 failed: client request id did not map to a Traffic record ${JSON.stringify(mappedPingTraffic)}`);
}
const mappedPingTrafficRecord = await api(`/traffic/${encodeURIComponent(mappedPingTraffic.traffic_id)}`);
if (
  mappedPingTrafficRecord.method !== pingNetworkEvents[0].method ||
  mappedPingTrafficRecord.status !== pingNetworkEvents[0].status ||
  !String(mappedPingTrafficRecord.url || '').includes('/devtools/api/ping?case=basic')
) {
  throw new Error(`AV-CDP-39 failed: Network event does not fully match mapped Traffic record ${JSON.stringify({ network: pingNetworkEvents[0], traffic: mappedPingTrafficRecord })}`);
}
const leakedDevtoolsHeader = (mappedPingTrafficRecord.request_headers || []).find(([key]) =>
  key.toLowerCase() === 'x-bifrost-client-request-id'
);
if (leakedDevtoolsHeader) {
  throw new Error(`AV-CDP-39 failed: internal DevTools client request header must be stripped before upstream capture ${JSON.stringify(leakedDevtoolsHeader)}`);
}
const metaNetworkEvent = snapshot.network.find((entry) => entry.url.includes('/devtools/api/meta?case=network-meta'));
if (!metaNetworkEvent) {
  throw new Error('AV-CDP-42 failed: browser-side Network metadata event missing');
}
if (metaNetworkEvent.status !== 200) {
  throw new Error(`AV-CDP-42 failed: browser-side Network event should preserve status, got ${JSON.stringify(metaNetworkEvent)}`);
}
if (!metaNetworkEvent.query_params?.some(([key, value]) => key === 'foo' && value === 'bar')) {
  throw new Error(`AV-CDP-42 failed: browser-side Network event should preserve query params ${JSON.stringify(metaNetworkEvent)}`);
}
if (!metaNetworkEvent.request_headers?.some(([key, value]) => key.toLowerCase() === 'x-bifrost-fixture-header' && value === 'fixture-request')) {
  throw new Error(`AV-CDP-42 failed: browser-side Network event should preserve request headers ${JSON.stringify(metaNetworkEvent)}`);
}
if (!metaNetworkEvent.response_headers?.length) {
  throw new Error(`AV-CDP-42 failed: browser-side Network event should preserve response headers ${JSON.stringify(metaNetworkEvent)}`);
}
if (!JSON.stringify(snapshot.storage || {}).includes('bifrost-storage-key')) {
  throw new Error('AV-CDP-11 failed: storage snapshot missing');
}

const evalResponse = await api(`/devtools/sessions/${session.session_id}/commands`, {
  method: 'POST',
  body: JSON.stringify({ command: 'runtime.evaluate', params: { expression: 'document.title' } }),
});
if (evalResponse.result?.value !== 'Bifrost DevTools Basic') {
  throw new Error(`AV-CDP-03 failed: bare devtools:// should enable runtime.evaluate ${JSON.stringify(evalResponse)}`);
}
const defaultStorageResponse = await api(`/devtools/sessions/${session.session_id}/commands`, {
  method: 'POST',
  body: JSON.stringify({
    command: 'storage.set',
    params: { area: 'local_storage', key: 'bifrost-default-storage-edit', value: 'default-ok' },
  }),
});
if (!defaultStorageResponse.ok || defaultStorageResponse.result?.key !== 'bifrost-default-storage-edit') {
  throw new Error(`AV-CDP-25 failed: bare devtools:// storage.set should be allowed ${JSON.stringify(defaultStorageResponse)}`);
}
const defaultStorageStartedAt = Date.now();
while (Date.now() - defaultStorageStartedAt < 8000) {
  if (await page.evaluate(() => localStorage.getItem('bifrost-default-storage-edit') === 'default-ok')) {
    break;
  }
  await new Promise((resolve) => setTimeout(resolve, 100));
}
const defaultStorageActual = await page.evaluate(() => ({
  href: location.href,
  bridge: {
    state: window.__BIFROST_DEVTOOLS_BRIDGE__?.state,
    pageId: window.__BIFROST_DEVTOOLS_BRIDGE__?.page_id,
    tabId: window.__BIFROST_DEVTOOLS_BRIDGE__?.tab_id,
  },
  value: localStorage.getItem('bifrost-default-storage-edit'),
}));
if (defaultStorageActual.value !== 'default-ok') {
  throw new Error(`AV-CDP-25 failed: storage.set did not update the active page ${JSON.stringify({ defaultStorageResponse, defaultStorageActual, activeDebugPage })}`);
}

await api('/rules/devtools-page-bridge-api', {
  method: 'PUT',
  body: JSON.stringify({
    content: controlRuleContent,
    enabled: true,
  }),
});
await new Promise((resolve) => setTimeout(resolve, 1500));
await page.goto(`http://devtools-fixture.test:${sitePort}/basic.html?case=av-cdp-control`, { waitUntil: 'load' });
const controlBridgeStartedAt = Date.now();
while (Date.now() - controlBridgeStartedAt < 8000) {
  const currentState = await page.evaluate(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state || 'missing');
  if (currentState === 'connected') {
    break;
  }
  await new Promise((resolve) => setTimeout(resolve, 100));
}
const controlBridgeState = await page.evaluate(() => ({
  href: location.href,
  injected: Boolean(document.querySelector('#__bifrost_devtools_bridge__')),
  bridge: {
    state: window.__BIFROST_DEVTOOLS_BRIDGE__?.state,
    pageId: window.__BIFROST_DEVTOOLS_BRIDGE__?.page_id,
    tabId: window.__BIFROST_DEVTOOLS_BRIDGE__?.tab_id,
  },
}));
if (controlBridgeState.bridge.state !== 'connected') {
  throw new Error(`AV-CDP-14 failed: control page bridge did not connect ${JSON.stringify(controlBridgeState)}`);
}
await page.waitForTimeout(800);
const controlDebugPage = await waitForDevToolsPage(
  (candidate) => candidate.url.includes('case=av-cdp-control') && candidate.mode === 'control' && candidate.state === 'discoverable',
  'AV-CDP-14 failed: control mode page not listed',
);
const controlSession = await api('/devtools/sessions', {
  method: 'POST',
  body: JSON.stringify({ page_id: controlDebugPage.page_id }),
});
const datasetExpression = 'document.querySelector("#debug-fixture").dataset.case';
const controlEvalReply = await api(`/devtools/sessions/${controlSession.session_id}/commands`, {
  method: 'POST',
  body: JSON.stringify({ command: 'runtime.evaluate', params: { expression: datasetExpression } }),
});
if (controlEvalReply?.result?.value !== 'basic') {
  throw new Error(`AV-CDP-14 failed: Runtime.evaluate did not execute in page bridge control mode ${JSON.stringify(controlEvalReply)}`);
}
let auditRecords = await api('/devtools/audit/evaluate?limit=5');
if (!auditRecords.some((entry) => entry.expression_sha256 === sha256(datasetExpression) && entry.expression_preview === datasetExpression && entry.target_page_id === controlDebugPage.page_id)) {
  throw new Error(`F3 failed: Runtime.evaluate audit record missing ${JSON.stringify(auditRecords)}`);
}
const titleEvalReply = await api(`/devtools/sessions/${controlSession.session_id}/commands`, {
  method: 'POST',
  body: JSON.stringify({ command: 'runtime.evaluate', params: { expression: 'document.title' } }),
});
const cookieEvalReply = await api(`/devtools/sessions/${controlSession.session_id}/commands`, {
  method: 'POST',
  body: JSON.stringify({ command: 'runtime.evaluate', params: { expression: 'document.cookie' } }),
});
if (titleEvalReply?.result?.value !== 'Bifrost DevTools Basic') {
  throw new Error(`F3 failed: default Runtime.evaluate should succeed ${JSON.stringify(titleEvalReply)}`);
}
if (!String(cookieEvalReply?.result?.value || '').includes('bifrost-cookie-key=')) {
  throw new Error(`F3 failed: bare devtools:// should not restrict document.cookie evaluation ${JSON.stringify(cookieEvalReply)}`);
}
for (let i = 0; i < 7; i += 1) {
  const reply = await api(`/devtools/sessions/${controlSession.session_id}/commands`, {
    method: 'POST',
    body: JSON.stringify({ command: 'runtime.evaluate', params: { expression: 'document.title' } }),
  });
  if (reply?.result?.value !== 'Bifrost DevTools Basic') {
    throw new Error(`F3 failed: ring-buffer seed evaluate ${i} failed ${JSON.stringify(reply)}`);
  }
}
auditRecords = await api('/devtools/audit/evaluate?limit=50');
if (auditRecords.length !== 5) {
  throw new Error(`F3 failed: audit ring buffer should be bounded to capacity 5, got ${auditRecords.length}: ${JSON.stringify(auditRecords)}`);
}
const finalDebugPage = controlDebugPage;

const mobileContext = await browser.newContext({
  proxy: { server: `http://127.0.0.1:${proxyPort}` },
  viewport: { width: 390, height: 844 },
  isMobile: true,
  hasTouch: true,
  userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1',
});
const mobilePage = await mobileContext.newPage();
await mobilePage.goto(`http://devtools-fixture.test:${sitePort}/basic.html?case=av-cdp-mobile`, { waitUntil: 'load' });
await mobilePage.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
await mobilePage.waitForTimeout(800);
pages = (await api('/devtools/pages?online=true')).pages;
const mobileDebugPage = pages.find((candidate) => candidate.url.includes('case=av-cdp-mobile'));
if (!mobileDebugPage) {
  throw new Error('AV-CDP-04 failed: mobile fallback page not listed by DevTools API');
}
if (mobileDebugPage.adapter !== 'page_bridge' || mobileDebugPage.fidelity !== 'fallback') {
  throw new Error(`AV-CDP-04 failed: wrong mobile fallback adapter ${JSON.stringify(mobileDebugPage)}`);
}
if (!mobileDebugPage.user_agent.includes('Mobile') || !mobileDebugPage.user_agent.includes('Safari')) {
  throw new Error(`AV-CDP-04 failed: mobile Safari UA not preserved ${mobileDebugPage.user_agent}`);
}

const adminContext = await browser.newContext();
await adminContext.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: `http://127.0.0.1:${proxyPort}` });
const adminPage = await adminContext.newPage();
await adminPage.goto(webui, { waitUntil: 'load' });
const devtoolsNavItem = adminPage.locator('[data-testid="app-sidebar-nav-item"][data-nav-label="DevTools"]').first();
try {
  await devtoolsNavItem.waitFor({ state: 'visible', timeout: 10000 });
} catch (error) {
  const navDebugState = await adminPage.evaluate(() => ({
    url: window.location.href,
    title: document.title,
    navItems: Array.from(document.querySelectorAll('[data-testid="app-sidebar-nav-item"]')).map((item) => ({
      label: item.getAttribute('data-nav-label'),
      key: item.getAttribute('data-nav-key'),
      text: item.textContent,
    })),
    bodyText: document.body?.textContent?.slice(0, 500) || '',
  }));
  throw new Error(`AV-CDP-10 failed: DevTools sidebar entry did not become visible ${JSON.stringify(navDebugState)} ${error.message}`);
}
let navLabels = await adminPage
  .locator('[data-testid="app-sidebar-nav-item"]')
  .evaluateAll((items) => items.map((item) => item.getAttribute('data-nav-label')));
if (navLabels.length === 0) {
  navLabels = await adminPage.locator('aside, nav, [class*="sidebar"]').first().locator('text=/Network|Replay|Rules|Scripts|DevTools|Values|Settings/').allTextContents();
}
const scriptsNavIndex = navLabels.indexOf('Scripts');
const devtoolsNavIndex = navLabels.indexOf('DevTools');
if (scriptsNavIndex === -1 || devtoolsNavIndex === -1 || devtoolsNavIndex <= scriptsNavIndex) {
  throw new Error(`AV-CDP-10 failed: DevTools sidebar entry must appear after Scripts (${navLabels.join(' > ')})`);
}
await devtoolsNavItem.click();
await adminPage.getByTestId('devtools-page-list').waitFor({ timeout: 8000 });
await adminPage.getByPlaceholder('Search online pages').fill('av-cdp-control');
const primaryCard = adminPage.getByTestId('devtools-page-card').filter({ hasText: 'Bifrost DevTools Basic' });
await primaryCard.waitFor({ timeout: 8000 });
const visiblePrimaryCards = await primaryCard.count();
if (visiblePrimaryCards !== 1) {
  throw new Error(`AV-CDP-13 failed: WebUI should list one target after same-tab reload, got ${visiblePrimaryCards}`);
}
let targetCounters = await targetRuntimeCounters(page);
await primaryCard.click();
await adminPage.getByTestId('devtools-detail').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-back').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-custom-workspace').waitFor({ timeout: 8000 });
targetCounters = await assertTargetRuntimeUnchanged(page, targetCounters, 'AV-CDP-44 failed: opening DevTools detail should not refresh or refetch the target page');
if (!/\/devtools\/pg_[A-Za-z0-9_%-]+/.test(adminPage.url())) {
  throw new Error(`AV-CDP-38 failed: DevTools detail should update the route with page id, got ${adminPage.url()}`);
}
await adminPage.reload({ waitUntil: 'load' });
await adminPage.getByTestId('devtools-detail').waitFor({ timeout: 12000 });
await adminPage.getByTestId('devtools-custom-workspace').waitFor({ timeout: 12000 });
targetCounters = await assertTargetRuntimeUnchanged(page, targetCounters, 'AV-CDP-44 failed: reloading WebUI detail route should not refresh or refetch the target page');
if (!/\/devtools\/pg_[A-Za-z0-9_%-]+/.test(adminPage.url())) {
  throw new Error(`AV-CDP-38 failed: DevTools detail route should survive WebUI reload, got ${adminPage.url()}`);
}
await adminPage.getByTestId('devtools-elements-tree').waitFor({ timeout: 8000 });
if ((await adminPage.locator('html').getAttribute('data-theme')) !== 'dark') {
  await adminPage.getByTestId('theme-toggle').click();
}
await adminPage.waitForFunction(() => document.documentElement.getAttribute('data-theme') === 'dark', null, { timeout: 8000 });
const darkSurfaceColors = await adminPage.evaluate(() => {
  const detail = document.querySelector('[data-testid="devtools-detail"]');
  const workspace = document.querySelector('[data-testid="devtools-custom-workspace"]');
  const elements = document.querySelector('[data-testid="devtools-elements-tree"]');
  return {
    detail: detail ? getComputedStyle(detail).backgroundColor : '',
    workspace: workspace ? getComputedStyle(workspace).backgroundColor : '',
    elements: elements ? getComputedStyle(elements).backgroundColor : '',
  };
});
for (const [name, color] of Object.entries(darkSurfaceColors)) {
  if (!color || color === 'rgb(255, 255, 255)' || color === 'rgb(247, 249, 252)') {
    throw new Error(`AV-CDP-37 failed: DevTools ${name} surface should follow dark theme, got ${color}`);
  }
}
const detailBoxInitial = await adminPage.getByTestId('devtools-detail').boundingBox();
const workspaceBoxInitial = await adminPage.getByTestId('devtools-custom-workspace').boundingBox();
if (!detailBoxInitial || !workspaceBoxInitial || workspaceBoxInitial.height < detailBoxInitial.height - 100) {
  throw new Error(`AV-CDP-29 failed: DevTools content area should fill remaining height detail=${JSON.stringify(detailBoxInitial)} workspace=${JSON.stringify(workspaceBoxInitial)}`);
}
const panelSearch = adminPage.getByTestId('devtools-panel-search');
const searchBoxInitial = await panelSearch.boundingBox();
if (!searchBoxInitial || searchBoxInitial.x + searchBoxInitial.width > workspaceBoxInitial.x + workspaceBoxInitial.width - 10) {
  throw new Error(`AV-CDP-32 failed: Elements tab search should remain visible inside the workspace header search=${JSON.stringify(searchBoxInitial)} workspace=${JSON.stringify(workspaceBoxInitial)}`);
}
if (searchBoxInitial.x < workspaceBoxInitial.x + 8) {
  throw new Error(`AV-CDP-32 failed: workspace right search layout lost horizontal padding search=${JSON.stringify(searchBoxInitial)} workspace=${JSON.stringify(workspaceBoxInitial)}`);
}
await adminPage.getByTestId('devtools-traffic-link').waitFor({ timeout: 8000 });
if (await adminPage.getByText('Adapter', { exact: true }).count()) {
  throw new Error('AV-CDP-29 failed: detail summary info strip should be removed');
}
await adminPage.getByTestId('devtools-target-url').hover();
await adminPage.getByTestId('devtools-copy-url').click();
const copiedTargetUrl = await adminPage.evaluate(() => navigator.clipboard.readText());
if (!copiedTargetUrl.includes('/basic.html?case=av-cdp-control')) {
  throw new Error(`AV-CDP-29 failed: copied target URL mismatch (${copiedTargetUrl})`);
}
if (await adminPage.getByText('Install Chrome DevTools').count()) {
  throw new Error('AV-CDP-06 failed: WebUI should not expose Chrome DevTools frontend installer');
}
if (await adminPage.getByText('Open in Chrome DevTools').count()) {
  throw new Error('AV-CDP-07 failed: WebUI should not expose system Chrome DevTools opener');
}
if (await adminPage.getByText('Debug URL', { exact: true }).count()) {
  throw new Error('AV-CDP-07 failed: WebUI should not expose devtools:// debug URL');
}
const debugFixtureNode = adminPage.getByTestId('devtools-dom-node').filter({ hasText: 'debug-fixture' }).first();
await debugFixtureNode.waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-elements-tree').waitFor({ timeout: 8000 });
if (await adminPage.getByTestId('devtools-elements-sidebar').count()) {
  throw new Error('AV-CDP-24 failed: Elements selected-node sidebar should not be rendered');
}
await adminPage.getByTestId('devtools-elements-tree').getByText('<').first().waitFor({ timeout: 8000 });
const firstDomNodeText = await adminPage.getByTestId('devtools-elements-tree').getByTestId('devtools-dom-node').first().textContent();
if (!firstDomNodeText || !firstDomNodeText.includes('<html')) {
  throw new Error(`AV-CDP-23 failed: Elements tree should start at <html> instead of a blank #document root (${firstDomNodeText})`);
}
const blankDomNodeCount = await adminPage.getByTestId('devtools-elements-tree').getByTestId('devtools-dom-node').evaluateAll((nodes) =>
  nodes.filter((node) => (node.textContent || '').trim() === '').length,
);
if (blankDomNodeCount !== 0) {
  throw new Error(`AV-CDP-23 failed: Elements tree rendered ${blankDomNodeCount} blank DOM rows`);
}
await adminPage.getByTestId('devtools-elements-tree').getByText('data-case').waitFor({ timeout: 8000 });
const detailButtons = adminPage.getByTestId('devtools-dom-value-detail');
if ((await detailButtons.count()) === 0) {
  throw new Error('AV-CDP-33 failed: long Elements values should render a compact preview with a detail trigger');
}
const elementsTreeText = await adminPage.getByTestId('devtools-elements-tree').textContent();
if ((elementsTreeText || '').includes('copyMerged(btn)')) {
  throw new Error('AV-CDP-33 failed: long inline script should not be fully expanded in the Elements tree');
}
await detailButtons.first().click();
const detailText = await adminPage.getByTestId('devtools-elements-detail-value').textContent();
if (!detailText || detailText.length <= 120) {
  throw new Error(`AV-CDP-33 failed: Elements detail modal should show full long value (${detailText})`);
}
await adminPage.getByTestId('devtools-elements-detail-copy').click();
const copiedElementDetail = await adminPage.evaluate(() => navigator.clipboard.readText());
if (copiedElementDetail !== detailText) {
  throw new Error('AV-CDP-33 failed: Elements detail copy should write the full detail value to clipboard');
}
await adminPage.keyboard.press('Escape');
await debugFixtureNode.click();
if (await adminPage.getByTestId('devtools-elements-sidebar').count()) {
  throw new Error('AV-CDP-24 failed: Elements selected-node sidebar should stay removed after selecting a node');
}
await page.waitForFunction(() => {
  const overlay = document.querySelector('#__bifrost_devtools_highlight__');
  return overlay && getComputedStyle(overlay).display !== 'none' && overlay.getBoundingClientRect().width > 0;
}, null, { timeout: 8000 });
await page.evaluate(() => {
  const item = document.createElement('section');
  item.id = 'debug-fixture-manual-refresh';
  item.textContent = 'manual refresh ready';
  document.body.appendChild(item);
});
targetCounters = await targetRuntimeCounters(page);
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-elements-panel').getByText(/debug-fixture-manual-refresh/).waitFor({ timeout: 8000 });
targetCounters = await assertTargetRuntimeUnchanged(page, targetCounters, 'AV-CDP-44 failed: Elements refresh should use WS snapshot without target reload or business refetch');
await panelSearch.fill('manual-refresh');
await adminPage.locator('[data-testid="devtools-dom-node"][data-selected="true"]').filter({ hasText: 'debug-fixture-manual-refresh' }).waitFor({ timeout: 8000 });
await panelSearch.fill('');
await adminPage.getByTestId('devtools-elements-inspect').click();
await page.waitForFunction(() => document.documentElement.getAttribute('data-bifrost-devtools-inspecting') === 'true', null, { timeout: 8000 });
await page.locator('#debug-fixture').hover();
await page.waitForFunction(() => {
  const overlay = document.querySelector('#__bifrost_devtools_highlight__');
  const info = document.querySelector('#__bifrost_devtools_highlight_info__');
  return overlay &&
    info &&
    getComputedStyle(overlay).display !== 'none' &&
    /debug-fixture/.test(info.textContent || '') &&
    /Color/.test(info.textContent || '') &&
    /Font/.test(info.textContent || '') &&
    /Padding/.test(info.textContent || '') &&
    /Margin/.test(info.textContent || '');
}, null, { timeout: 8000 });
await page.locator('#debug-fixture').click();
await page.waitForFunction(() => document.documentElement.getAttribute('data-bifrost-devtools-inspecting') !== 'true', null, { timeout: 8000 });
await adminPage.locator('[data-testid="devtools-dom-node"][data-selected="true"]').filter({ hasText: 'debug-fixture' }).waitFor({ timeout: 8000 });
const inspectOverlayInfo = await page.evaluate(() => document.querySelector('#__bifrost_devtools_highlight_info__')?.textContent || '');
if (!/div#debug-fixture/.test(inspectOverlayInfo) || !/\d+\s*x\s*\d+/.test(inspectOverlayInfo)) {
  throw new Error(`AV-CDP-43 failed: target inspect overlay should show node label and size (${inspectOverlayInfo})`);
}
if (!/Color/.test(inspectOverlayInfo) || !/Font/.test(inspectOverlayInfo) || !/Padding/.test(inspectOverlayInfo) || !/Margin/.test(inspectOverlayInfo)) {
  throw new Error(`AV-CDP-43 failed: target inspect overlay should show style summary (${inspectOverlayInfo})`);
}
const inspectOverlayBox = await page.evaluate(() => {
  const info = document.querySelector('#__bifrost_devtools_highlight_info__');
  if (!info) return null;
  const rect = info.getBoundingClientRect();
  return { width: rect.width, left: rect.left, right: rect.right, viewport: window.innerWidth };
});
if (!inspectOverlayBox || inspectOverlayBox.width < 200 || inspectOverlayBox.left < -1 || inspectOverlayBox.right > inspectOverlayBox.viewport + 1) {
  throw new Error(`AV-CDP-43 failed: target inspect overlay info card width/position is invalid ${JSON.stringify(inspectOverlayBox)} text=${inspectOverlayInfo}`);
}
await clickDevtoolsWorkspaceTab(adminPage, 'Network');
await page.evaluate(() => fetch('/devtools/api/extra?case=webui-network-complete').catch(() => {}));
await page.waitForFunction(() => (window.__bifrostBusinessFetchUrls || []).some((url) => String(url).includes('webui-network-complete')), null, { timeout: 8000 });
targetCounters = await targetRuntimeCounters(page);
await adminPage.getByTestId('devtools-refresh').click();
try {
  await adminPage.getByTestId('devtools-network-traffic-table').getByTestId('traffic-table').waitFor({ timeout: 8000 });
} catch (error) {
  const debugState = await adminPage.evaluate(() => {
    const panel = document.querySelector('[data-testid="devtools-network-panel"]');
    const table = document.querySelector('[data-testid="devtools-network-traffic-table"]');
    const trafficTable = document.querySelector('[data-testid="traffic-table"]');
    const activeTab = document.querySelector('.devtools-workspace-tabs .ant-tabs-tab-active')?.textContent || '';
    const rect = (node) => node ? node.getBoundingClientRect().toJSON?.() || {
      x: node.getBoundingClientRect().x,
      y: node.getBoundingClientRect().y,
      width: node.getBoundingClientRect().width,
      height: node.getBoundingClientRect().height,
    } : null;
    return {
      activeTab,
      panelText: panel?.textContent?.slice(0, 500) || null,
      tableRect: rect(table),
      trafficTableRect: rect(trafficTable),
      tableCount: document.querySelectorAll('[data-testid="devtools-network-traffic-table"]').length,
      trafficTableCount: document.querySelectorAll('[data-testid="traffic-table"]').length,
    };
  });
  throw new Error(`AV-CDP-44 failed: Network table did not become visible ${JSON.stringify(debugState)} ${error.message}`);
}
await adminPage.getByTestId('devtools-network-panel').getByText('Protocol').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByText('Host').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByText('Path').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByText(/webui-network-complete/).waitFor({ timeout: 8000 });
targetCounters = await assertTargetRuntimeUnchanged(page, targetCounters, 'AV-CDP-44 failed: Network refresh should use WS snapshot without target reload or business refetch');
await panelSearch.fill('webui-network-complete');
await adminPage.getByTestId('devtools-network-panel').getByText(/webui-network-complete/).waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByText(/devtools\/api\/ping/).waitFor({ state: 'detached', timeout: 8000 });
const webuiNetworkRows = await adminPage.getByTestId('devtools-network-panel').getByTestId('traffic-row').evaluateAll((rows) =>
  rows.map((row) => row.textContent || '').filter((text) => text.includes('webui-network-complete'))
);
if (webuiNetworkRows.length !== 1) {
  throw new Error(`AV-CDP-44 failed: Network should dedupe fetch hook and PerformanceResourceTiming rows ${JSON.stringify(webuiNetworkRows)}`);
}
const devtoolsUrlBeforeNetworkDetail = adminPage.url();
await adminPage.getByTestId('devtools-network-panel').getByTestId('traffic-row').first().click({ force: true });
await adminPage.getByTestId('devtools-network-detail').getByTestId('traffic-detail').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-detail').getByText(/webui-network-complete/).first().waitFor({ timeout: 8000 });
const networkPanelBox = await adminPage.getByTestId('devtools-network-panel').boundingBox();
const networkTableBox = await adminPage.getByTestId('devtools-network-traffic-table').boundingBox();
const networkDetailBox = await adminPage.getByTestId('devtools-network-detail').boundingBox();
if (!networkPanelBox || !networkTableBox || !networkDetailBox || networkDetailBox.x <= networkTableBox.x + networkTableBox.width - 20) {
  throw new Error(`AV-CDP-35 failed: Network detail should render as a right-side panel, panel=${JSON.stringify(networkPanelBox)}, table=${JSON.stringify(networkTableBox)}, detail=${JSON.stringify(networkDetailBox)}`);
}
if (!adminPage.url().includes('/devtools') || adminPage.url().includes('/traffic')) {
  throw new Error(`AV-CDP-35 failed: Network detail should stay inside DevTools, before=${devtoolsUrlBeforeNetworkDetail}, after=${adminPage.url()}`);
}
await adminPage.route('**/_bifrost/api/devtools/network/traffic/**', async (route) => {
  await route.fulfill({
    status: 404,
    contentType: 'application/json',
    body: JSON.stringify({ error: 'missing test traffic' }),
  });
});
await adminPage.route('**/_bifrost/api/traffic/**', async (route) => {
  if (route.request().method() === 'GET') {
    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'missing test traffic detail' }),
    });
    return;
  }
  await route.fallback();
});
await page.evaluate(() => fetch('/devtools/api/missing?case=bridge-only-detail&foo=fallback', {
  headers: { 'x-bifrost-fallback-header': 'fallback-request' },
}).catch(() => {}));
await adminPage.getByTestId('devtools-refresh').click();
await panelSearch.fill('bridge-only-detail');
await adminPage.getByTestId('devtools-network-panel').getByText(/bridge-only-detail/).waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByTestId('traffic-row').first().click({ force: true });
await adminPage.getByTestId('devtools-network-fallback-detail').getByText(/bridge-only-detail/).first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-fallback-detail').getByText('404').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-query').getByText('foo').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-request-headers').getByText(/x-bifrost-fallback-header/i).waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-detail').getByText(/Traffic Match|No matching Traffic record|missing test traffic|Request failed/).first().waitFor({ timeout: 8000 });
if (!adminPage.url().includes('/devtools') || adminPage.url().includes('/traffic')) {
  throw new Error(`AV-CDP-35 failed: fallback Network detail should stay inside DevTools, after=${adminPage.url()}`);
}
await page.evaluate(() => new Promise((resolve) => {
  const img = document.createElement('img');
  img.id = 'debug-fixture-tag-fallback';
  img.onload = img.onerror = resolve;
  img.src = '/devtools/api/tag-missing?case=bridge-only-tag-fallback&foo=tag';
  document.body.appendChild(img);
}));
await adminPage.getByTestId('devtools-refresh').click();
await panelSearch.fill('bridge-only-tag-fallback');
await adminPage.getByTestId('devtools-network-panel').getByText(/bridge-only-tag-fallback/).waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByText('404').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByTestId('traffic-row').first().click({ force: true });
await adminPage.getByTestId('devtools-network-fallback-detail').getByText(/bridge-only-tag-fallback/).first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-fallback-detail').getByText('404').first().waitFor({ timeout: 8000 });
await adminPage.unroute('**/_bifrost/api/devtools/network/traffic/**');
await adminPage.unroute('**/_bifrost/api/traffic/**');
await panelSearch.fill('');
await page.evaluate(() => new Promise((resolve) => {
  const img = document.createElement('img');
  img.id = 'debug-fixture-parser-resource';
  img.onload = img.onerror = resolve;
  img.src = '/devtools/api/parser-resource?case=ui-traffic-enrich&foo=img';
  document.body.appendChild(img);
}));
await adminPage.getByTestId('devtools-refresh').click();
await panelSearch.fill('ui-traffic-enrich');
await adminPage.getByTestId('devtools-network-panel').getByText(/ui-traffic-enrich/).waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByText('404').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByTestId('traffic-row').first().click({ force: true });
await adminPage.getByTestId('devtools-network-detail').getByTestId('traffic-detail').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-detail').getByText(/ui-traffic-enrich/).first().waitFor({ timeout: 8000 });
await panelSearch.fill('');
await clickDevtoolsWorkspaceTab(adminPage, 'LocalStorage');
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-key').waitFor({ timeout: 8000 });
await clickDevtoolsWorkspaceTab(adminPage, 'Cookies');
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-key').waitFor({ timeout: 8000 });
await clickDevtoolsWorkspaceTab(adminPage, 'SessionStorage');
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-key').waitFor({ timeout: 8000 });
await clickDevtoolsWorkspaceTab(adminPage, 'LocalStorage');
await page.evaluate(() => {
  document.cookie = 'bifrost-cookie-live=cookie-live; path=/';
  localStorage.setItem('bifrost-storage-live', 'storage-live');
  sessionStorage.setItem('bifrost-session-live', 'session-live');
});
targetCounters = await targetRuntimeCounters(page);
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-live').waitFor({ timeout: 8000 });
targetCounters = await assertTargetRuntimeUnchanged(page, targetCounters, 'AV-CDP-44 failed: Storage refresh should use WS snapshot without target reload or business refetch');
await panelSearch.fill('bifrost-storage-live');
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-key').waitFor({ state: 'detached', timeout: 8000 });
await panelSearch.fill('');
await clickDevtoolsWorkspaceTab(adminPage, 'Cookies');
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-live').waitFor({ timeout: 8000 });
await clickDevtoolsWorkspaceTab(adminPage, 'SessionStorage');
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-live').waitFor({ timeout: 8000 });
await page.evaluate(() => {
  for (let i = 0; i < 420; i += 1) {
    localStorage.setItem(`bifrost-virtual-local-${i}`, `local-${i}`);
    sessionStorage.setItem(`bifrost-virtual-session-${i}`, `session-${i}`);
  }
});
await adminPage.getByTestId('devtools-refresh').click();
await clickDevtoolsWorkspaceTab(adminPage, 'LocalStorage');
await adminPage.getByText(/42\d \/ 42\d items/).waitFor({ timeout: 8000 });
const localStorageRenderedRows = await adminPage.getByTestId('devtools-local-storage-panel').getByTestId('devtools-storage-row').count();
if (localStorageRenderedRows > 80) {
  throw new Error(`AV-CDP-40 failed: LocalStorage should virtualize large storage lists, rendered rows=${localStorageRenderedRows}`);
}
const storageSwitchStartedAt = Date.now();
await clickDevtoolsWorkspaceTab(adminPage, 'SessionStorage');
await adminPage.getByText(/42\d \/ 42\d items/).waitFor({ timeout: 2500 });
if (Date.now() - storageSwitchStartedAt > 2500) {
  throw new Error(`AV-CDP-40 failed: switching to SessionStorage should not block for seconds, elapsed=${Date.now() - storageSwitchStartedAt}ms`);
}
const sessionStorageRenderedRows = await adminPage.getByTestId('devtools-session-storage-panel').getByTestId('devtools-storage-row').count();
if (sessionStorageRenderedRows > 80) {
  throw new Error(`AV-CDP-40 failed: SessionStorage should virtualize large storage lists, rendered rows=${sessionStorageRenderedRows}`);
}
await clickDevtoolsWorkspaceTab(adminPage, 'LocalStorage');
await panelSearch.fill('bifrost-storage-live');
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-live').waitFor({ timeout: 8000 });
await adminPage.getByLabel('Edit bifrost-storage-live').click();
if (await adminPage.getByTestId('devtools-storage-key').inputValue() !== 'bifrost-storage-live') {
  throw new Error('AV-CDP-15 failed: storage row edit did not copy key into editor');
}
if (await adminPage.getByTestId('devtools-storage-value').inputValue() !== 'storage-live') {
  throw new Error('AV-CDP-15 failed: storage row edit did not copy value into editor');
}
await adminPage.getByLabel('Copy bifrost-storage-live').click();
const copiedStorageValue = await adminPage.evaluate(() => navigator.clipboard.readText());
if (copiedStorageValue !== 'storage-live') {
  throw new Error(`AV-CDP-15 failed: storage copy did not write clipboard (${copiedStorageValue})`);
}
await panelSearch.fill('');
await clickDevtoolsWorkspaceTab(adminPage, 'Cookies');
await adminPage.getByTestId('devtools-storage-add').click();
await adminPage.getByTestId('devtools-storage-key').fill('bifrost-cookie-edit');
await adminPage.getByTestId('devtools-storage-value').fill('cookie-edited');
await adminPage.getByTestId('devtools-storage-save').click();
await page.waitForFunction(() => document.cookie.includes('bifrost-cookie-edit=cookie-edited'), null, { timeout: 8000 });
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-edit').waitFor({ timeout: 8000 });
await adminPage.getByLabel('Delete bifrost-cookie-edit').click();
await page.waitForFunction(() => !document.cookie.includes('bifrost-cookie-edit='), null, { timeout: 8000 });
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-edit').waitFor({ state: 'detached', timeout: 8000 });
await clickDevtoolsWorkspaceTab(adminPage, 'LocalStorage');
await adminPage.getByTestId('devtools-storage-add').click();
await adminPage.getByTestId('devtools-storage-key').fill('bifrost-storage-edit');
await adminPage.getByTestId('devtools-storage-value').fill('storage-edited');
await adminPage.getByTestId('devtools-storage-save').click();
await page.waitForFunction(() => localStorage.getItem('bifrost-storage-edit') === 'storage-edited', null, { timeout: 8000 });
await panelSearch.fill('bifrost-storage-edit');
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-edit').waitFor({ timeout: 8000 });
await adminPage.getByLabel('Delete bifrost-storage-edit').click();
await page.waitForFunction(() => localStorage.getItem('bifrost-storage-edit') === null, null, { timeout: 8000 });
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-edit').waitFor({ state: 'detached', timeout: 8000 });
await panelSearch.fill('');
await clickDevtoolsWorkspaceTab(adminPage, 'SessionStorage');
await adminPage.getByTestId('devtools-storage-add').click();
await adminPage.getByTestId('devtools-storage-key').fill('bifrost-session-edit');
await adminPage.getByTestId('devtools-storage-value').fill('session-edited');
await adminPage.getByTestId('devtools-storage-save').click();
await page.waitForFunction(() => sessionStorage.getItem('bifrost-session-edit') === 'session-edited', null, { timeout: 8000 });
await panelSearch.fill('bifrost-session-edit');
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-edit').waitFor({ timeout: 8000 });
await adminPage.getByLabel('Delete bifrost-session-edit').click();
await page.waitForFunction(() => sessionStorage.getItem('bifrost-session-edit') === null, null, { timeout: 8000 });
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-edit').waitFor({ state: 'detached', timeout: 8000 });
await panelSearch.fill('');
await clickDevtoolsWorkspaceTab(adminPage, 'Console');
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-devtools-basic-ready').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-devtools-warning-ready').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-object-ready').first().waitFor({ timeout: 8000 });
const plainLogValue = adminPage.getByTestId('devtools-console-row-log').getByTestId('devtools-console-value-string').filter({ hasText: 'bifrost-devtools-basic-ready' }).first();
await plainLogValue.waitFor({ timeout: 8000 });
const plainLogColor = await plainLogValue.evaluate((node) => getComputedStyle(node).color);
if (plainLogColor === 'rgb(196, 29, 29)' || plainLogColor === 'rgb(255, 0, 0)') {
  throw new Error(`AV-CDP-36 failed: plain console.log string should not render as red object string value (${plainLogColor})`);
}
await adminPage.getByTestId('devtools-console-panel').getByText(/Object \{.*pageId/).first().waitFor({ timeout: 8000 });
const objectRow = adminPage.getByTestId('devtools-console-row-log').filter({ hasText: 'bifrost-console-object-ready' }).first();
await objectRow.getByTestId('devtools-console-expand-value').last().click();
await objectRow.getByText('nested:', { exact: true }).waitFor({ timeout: 8000 });
await objectRow.getByText('items:', { exact: true }).waitFor({ timeout: 8000 });
const objectPreviewBox = await objectRow.getByText(/Object \{.*pageId/).boundingBox();
const nestedBox = await objectRow.getByText('nested:', { exact: true }).boundingBox();
if (!objectPreviewBox || !nestedBox || nestedBox.x < objectPreviewBox.x + 10) {
  throw new Error(`AV-CDP-36 failed: expanded console object tree should align beneath the object preview, object=${JSON.stringify(objectPreviewBox)}, nested=${JSON.stringify(nestedBox)}`);
}
await objectRow.getByLabel('Copy raw console content').click();
const copiedConsoleRaw = await adminPage.evaluate(() => navigator.clipboard.readText());
if (!copiedConsoleRaw.includes('bifrost-console-object-ready') || !copiedConsoleRaw.includes('"nested"')) {
  throw new Error(`AV-CDP-32 failed: copy raw console object content mismatch ${JSON.stringify(copiedConsoleRaw)}`);
}
await page.evaluate(() => {
  console.info('bifrost-console-info-live');
  console.debug('bifrost-console-debug-live');
  console.error('bifrost-console-error-live');
  console.log('%cbifrost-console-css-live', 'color: rgb(255, 0, 0); font-weight: 700');
});
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-info-live').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-debug-live').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-error-live').first().waitFor({ timeout: 8000 });
const cssConsoleText = adminPage.getByTestId('devtools-console-row-log').getByTestId('devtools-console-formatted-text').filter({ hasText: 'bifrost-console-css-live' }).first();
await cssConsoleText.waitFor({ timeout: 8000 });
const cssConsoleStyle = await cssConsoleText.evaluate((node) => {
  const style = getComputedStyle(node);
  return { color: style.color, fontWeight: style.fontWeight, text: node.textContent || '' };
});
if (cssConsoleStyle.text.includes('%c') || cssConsoleStyle.text.includes('color:')) {
  throw new Error(`AV-CDP-41 failed: formatted console text should hide %c/style args ${JSON.stringify(cssConsoleStyle)}`);
}
if (cssConsoleStyle.color !== 'rgb(255, 0, 0)' || Number(cssConsoleStyle.fontWeight) < 700) {
  throw new Error(`AV-CDP-41 failed: formatted console text should apply CSS style ${JSON.stringify(cssConsoleStyle)}`);
}
await adminPage.getByTestId('devtools-console-row-log').getByText('bifrost-devtools-basic-ready').first().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-warn').getByText('bifrost-devtools-warning-ready').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-info').getByText('bifrost-console-info-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-debug').getByText('bifrost-console-debug-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-error').getByText('bifrost-console-error-live').waitFor({ timeout: 8000 });
const consoleTimestamp = (await adminPage.getByTestId('devtools-console-row-time').first().textContent()) || '';
if (!/^\d{2}:\d{2}:\d{2}\.\d{3}$/.test(consoleTimestamp.trim())) {
  throw new Error(`AV-CDP-31 failed: console rows should show millisecond timestamps, got ${JSON.stringify(consoleTimestamp)}`);
}
await panelSearch.fill('bifrost-console-error-live');
await adminPage.getByTestId('devtools-console-row-error').getByText('bifrost-console-error-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-info-live').waitFor({ state: 'detached', timeout: 8000 });
await panelSearch.fill('');
await adminPage.getByTestId('devtools-console-input').fill('document.title');
await adminPage.getByTestId('devtools-console-run').click();
await adminPage.getByTestId('devtools-console-row-input').getByText('document.title').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-result').filter({ hasText: 'Bifrost DevTools Basic' }).last().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-input').fill('window.reload()');
await adminPage.getByTestId('devtools-console-run').click();
await adminPage.getByTestId('devtools-console-row-input').getByText('window.reload()').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-error').filter({ hasText: /reload.*not a function|window\.reload/ }).last().waitFor({ timeout: 8000 });
if (await adminPage.getByTestId('devtools-console-row-error').filter({ hasText: 'Request failed with status code 400' }).count()) {
  throw new Error('AV-CDP-28 failed: console JavaScript exceptions should show the remote JS error instead of HTTP 400');
}
await adminPage.getByTestId('devtools-console-input').fill('(() => {\n  return document.title + " fullscreen";\n})()');
await adminPage.getByTestId('devtools-console-expand-editor').click();
await adminPage.getByTestId('devtools-console-fullscreen-editor').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-fullscreen-run').click();
await adminPage.getByTestId('devtools-console-row-input').getByText('fullscreen').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-result').filter({ hasText: 'Bifrost DevTools Basic fullscreen' }).last().waitFor({ timeout: 8000 });
const inputBox = await adminPage.getByTestId('devtools-console-input').boundingBox();
const panelBox = await adminPage.getByTestId('devtools-console-panel').boundingBox();
if (!inputBox || !panelBox || inputBox.y + inputBox.height > panelBox.y + panelBox.height + 4) {
  throw new Error(`AV-CDP-27 failed: console input should remain pinned inside panel bottom input=${JSON.stringify(inputBox)} panel=${JSON.stringify(panelBox)}`);
}
await page.reload({ waitUntil: 'load' });
await page.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
await page.waitForTimeout(800);
await clickDevtoolsWorkspaceTab(adminPage, 'Elements');
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-elements-tree').getByText(/debug-fixture/).first().waitFor({ timeout: 8000 });
await clickDevtoolsWorkspaceTab(adminPage, 'Console');
await adminPage.getByTestId('devtools-console-input').fill('document.title');
await adminPage.getByTestId('devtools-console-run').click();
await adminPage.getByTestId('devtools-console-row-result').filter({ hasText: 'Bifrost DevTools Basic' }).last().waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-back').click();
await adminPage.getByTestId('devtools-page-list').waitFor({ timeout: 8000 });
await adminPage.getByPlaceholder('Search online pages').fill('av-cdp-control');
const reloadedPrimaryCard = adminPage.getByTestId('devtools-page-card').filter({ hasText: 'Bifrost DevTools Basic' });
await reloadedPrimaryCard.waitFor({ timeout: 8000 });
if (await reloadedPrimaryCard.count() !== 1) {
  throw new Error(`AV-CDP-22 failed: target reload should leave exactly one WebUI card, got ${await reloadedPrimaryCard.count()}`);
}

await adminPage.getByTestId('devtools-page-list').waitFor({ timeout: 8000 });
await adminPage.getByPlaceholder('Search online pages').waitFor({ state: 'visible', timeout: 8000 });
await adminPage.waitForTimeout(500);
await adminPage.getByPlaceholder('Search online pages').fill('secondary');
await adminPage.getByRole('button', { name: /Refresh Pages/ }).click();
const secondaryCard = adminPage.getByTestId('devtools-page-card').filter({ hasText: 'Bifrost DevTools Secondary' });
await secondaryCard.waitFor({ timeout: 8000 });
await secondaryCard.click();
await adminPage.getByTestId('devtools-detail').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-custom-workspace').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-elements-panel').getByText(/debug-fixture-secondary/).waitFor({ timeout: 8000 });
if (await adminPage.locator('iframe[title="Chrome DevTools Frontend"]').count()) {
  throw new Error('AV-CDP-09 failed: WebUI should not embed Chrome DevTools frontend when switching pages');
}
await adminPage.getByTestId('devtools-traffic-link').click();
await adminPage.waitForURL(/\/traffic/, { timeout: 8000 });

await browser.close();
console.log('DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed');
NODE
