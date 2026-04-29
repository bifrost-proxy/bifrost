#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

pick_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

TEST_ROOT="$(mktemp -d -t bifrost-devtools-e2e.XXXXXX)"
SITE_DIR="$TEST_ROOT/site"
mkdir -p "$SITE_DIR"

PROXY_PORT="$(pick_port)"
SITE_PORT="$(pick_port)"
CHROME_DEBUG_PORT="$(pick_port)"
if [ "$PROXY_PORT" = "9900" ] || [ "$SITE_PORT" = "9900" ] || [ "$CHROME_DEBUG_PORT" = "9900" ]; then
  echo "Refusing to use reserved port 9900" >&2
  exit 1
fi

cleanup() {
  local rc=$?
  if [ $rc -ne 0 ]; then
    echo "--- bifrost.log ---" >&2
    sed -n '1,260p' "$TEST_ROOT/bifrost.log" >&2 || true
    echo "--- site.log ---" >&2
    sed -n '1,120p' "$TEST_ROOT/site.log" >&2 || true
  fi
  if [ -n "${BIFROST_PID:-}" ]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [ -n "${SITE_PID:-}" ]; then
    kill "$SITE_PID" >/dev/null 2>&1 || true
  fi
  if [ -n "${SYSTEM_CHROME_PROFILE:-}" ]; then
    pkill -f "$SYSTEM_CHROME_PROFILE" >/dev/null 2>&1 || true
  fi
  sleep 0.5
  rm -rf "$TEST_ROOT" 2>/dev/null || {
    sleep 1
    rm -rf "$TEST_ROOT" 2>/dev/null || true
  }
}
trap cleanup EXIT

printf '%s\n' '<!doctype html><html><head><title>Bifrost DevTools Basic</title><script>localStorage.setItem("bifrost-storage-key","storage-ready"); sessionStorage.setItem("bifrost-session-key","session-ready"); console.log("bifrost-devtools-basic-ready"); console.warn("bifrost-devtools-warning-ready");</script></head><body><div id="debug-fixture" data-case="basic" style="color: rgb(11, 22, 33); display: block;">ready</div><script>fetch("/devtools/api/ping?case=basic").catch(function(){})</script></body></html>' > "$SITE_DIR/basic.html"
printf '%s\n' '<!doctype html><html><head><title>Bifrost DevTools Secondary</title><script>console.log("bifrost-devtools-secondary-ready")</script></head><body><main id="debug-fixture-secondary" data-case="secondary">secondary</main></body></html>' > "$SITE_DIR/secondary.html"

python3 -m http.server "$SITE_PORT" --bind 127.0.0.1 --directory "$SITE_DIR" >"$TEST_ROOT/site.log" 2>&1 &
SITE_PID=$!

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
if [ -x "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" ]; then
  SYSTEM_CHROME_BIN="/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"
elif [ -x "/Applications/Chromium.app/Contents/MacOS/Chromium" ]; then
  SYSTEM_CHROME_BIN="/Applications/Chromium.app/Contents/MacOS/Chromium"
else
  SYSTEM_CHROME_BIN="$(node - <<'NODE'
const { chromium } = require('./web/node_modules/playwright');
console.log(chromium.executablePath());
NODE
)"
fi
export BIFROST_DEVTOOLS_CHROME="${BIFROST_DEVTOOLS_CHROME:-$SYSTEM_CHROME_BIN}"
export BIFROST_DEVTOOLS_CHROME_DEBUG_PORT="$CHROME_DEBUG_PORT"
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
ALLOWLIST_RULE_CONTENT="$(sed "s/__SITE_PORT__/$SITE_PORT/g" e2e-tests/rules/devtools/page_bridge_control_allowlist.txt | grep -v '^#' | sed '/^$/d')"

PROXY_PORT="$PROXY_PORT" SITE_PORT="$SITE_PORT" CHROME_DEBUG_PORT="$CHROME_DEBUG_PORT" RULE_CONTENT="$RULE_CONTENT" CONTROL_RULE_CONTENT="$CONTROL_RULE_CONTENT" ALLOWLIST_RULE_CONTENT="$ALLOWLIST_RULE_CONTENT" node --input-type=module <<'NODE'
import { chromium } from './web/node_modules/playwright/index.mjs';
import NodeWebSocket from './web/node_modules/ws/index.js';
import net from 'node:net';
import { createHash, randomBytes } from 'node:crypto';

const proxyPort = process.env.PROXY_PORT;
const sitePort = process.env.SITE_PORT;
const chromeDebugPort = process.env.CHROME_DEBUG_PORT;
const ruleContent = process.env.RULE_CONTENT;
const controlRuleContent = process.env.CONTROL_RULE_CONTENT;
const allowlistRuleContent = process.env.ALLOWLIST_RULE_CONTENT;
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

async function waitForChromeTarget(predicate, timeoutMs = 40000) {
  const startedAt = Date.now();
  let lastError = '';
  while (Date.now() - startedAt < timeoutMs) {
    try {
      const response = await fetch(`http://127.0.0.1:${chromeDebugPort}/json/list`);
      if (response.ok) {
        const targets = await response.json();
        const target = targets.find(predicate);
        if (target) return target;
        lastError = JSON.stringify(targets.map((item) => item.url));
      } else {
        lastError = `HTTP ${response.status}`;
      }
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`Chrome target did not appear on remote debugging port ${chromeDebugPort}: ${lastError}`);
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

async function chromeTargetEvaluate(webSocketUrl, expression) {
  const replies = await roundtripCdp(webSocketUrl, [
    { id: 9001, method: 'Runtime.enable' },
    { id: 9002, method: 'Runtime.evaluate', params: { expression, returnByValue: true } },
  ]);
  const reply = replies.find((candidate) => candidate.id === 9002);
  if (!reply || reply.error) {
    throw new Error(`Chrome DevTools target evaluation failed: ${JSON.stringify(replies)}`);
  }
  return reply.result?.result?.value;
}

async function waitForChromeTargetExpression(webSocketUrl, expression, timeoutMs = 20000) {
  const startedAt = Date.now();
  let lastValue;
  while (Date.now() - startedAt < timeoutMs) {
    lastValue = await chromeTargetEvaluate(webSocketUrl, expression);
    if (lastValue === true) return;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`Chrome DevTools target expression did not become true: ${expression}, last=${lastValue}`);
}

async function chromeTargetScreenshot(webSocketUrl) {
  const replies = await roundtripCdp(webSocketUrl, [
    { id: 9011, method: 'Page.enable' },
    { id: 9012, method: 'Page.captureScreenshot', params: { format: 'png' } },
  ]);
  const reply = replies.find((candidate) => candidate.id === 9012);
  if (!reply?.result?.data || reply.result.data.length < 1000) {
    throw new Error(`Chrome DevTools target screenshot failed: ${JSON.stringify(replies)}`);
  }
  return reply.result.data;
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

  const expectedErrors = [
    { id: 585, method: 'Runtime.evaluate', message: 'requires_control', params: { expression: 'document.title' } },
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

  await waitForIds([584, ...expectedErrors.map((entry) => entry.id)]);
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

const browser = await chromium.launch({ headless: true });
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

const cdpTargets = await api('/devtools/cdp/json/list');
const cdpTarget = cdpTargets.find((target) => target.id === activeDebugPage.page_id);
const secondaryCdpTarget = cdpTargets.find((target) => target.id === secondaryDebugPage.page_id);
if (!cdpTarget?.webSocketDebuggerUrl || !cdpTarget.systemChromeFrontendUrl) {
  throw new Error(`AV-CDP-05 failed: CDP target missing frontend URLs ${JSON.stringify(cdpTarget)}`);
}
if (!secondaryCdpTarget?.webSocketDebuggerUrl || !secondaryCdpTarget.systemChromeFrontendUrl) {
  throw new Error(`AV-CDP-09 failed: secondary CDP target missing frontend URLs ${JSON.stringify(secondaryCdpTarget)}`);
}
if (!cdpTarget.webSocketDebuggerUrl.includes(`/_bifrost/api/devtools/cdp/${activeDebugPage.page_id}`)) {
  throw new Error(`AV-CDP-05 failed: wrong CDP websocket URL ${cdpTarget.webSocketDebuggerUrl}`);
}
const evilOriginHandshake = await rawWsHandshake(wsPathFromUrl(cdpTarget.webSocketDebuggerUrl), 'http://evil.com');
if (evilOriginHandshake.status !== 401 || !evilOriginHandshake.response.includes('origin_not_allowed')) {
  throw new Error(`F23 failed: foreign Origin should be rejected with origin_not_allowed ${JSON.stringify(evilOriginHandshake)}`);
}
const cdpVersion = await api('/devtools/cdp/json/version');
if (cdpVersion['Protocol-Version'] !== '1.3') {
  throw new Error(`AV-CDP-05 failed: wrong protocol version ${JSON.stringify(cdpVersion)}`);
}
const frontendStatusBeforeInstall = await api('/devtools/frontend/status');
if (frontendStatusBeforeInstall.installed) {
  throw new Error(`AV-CDP-06 failed: fresh data dir unexpectedly has Chrome frontend installed ${JSON.stringify(frontendStatusBeforeInstall)}`);
}
const missingInspectorResponse = await fetch(`http://127.0.0.1:${proxyPort}/_bifrost/api/devtools/frontend/inspector.html?ws=127.0.0.1:${proxyPort}/_bifrost/api/devtools/cdp/${debugPage.page_id}`);
if (missingInspectorResponse.status !== 404) {
  throw new Error(`AV-CDP-06 failed: embedded frontend unexpectedly served without explicit install (${missingInspectorResponse.status})`);
}
async function assertEmbeddedFrontendInstalled(pageId) {
  const frontendStatusAfterInstall = await api('/devtools/frontend/status');
  if (!frontendStatusAfterInstall.installed || frontendStatusAfterInstall.state !== 'installed') {
    throw new Error(`AV-CDP-08 failed: Chrome frontend was not installed ${JSON.stringify(frontendStatusAfterInstall)}`);
  }
  if (!frontendStatusAfterInstall.installPath.includes('/admin/devtools-frontend/chrome-devtools-frontend-')) {
    throw new Error(`AV-CDP-08 failed: frontend install path is not under BIFROST_DATA_DIR admin cache ${frontendStatusAfterInstall.installPath}`);
  }
  if ((frontendStatusAfterInstall.totalSizeBytes ?? 0) < 1_000_000) {
    throw new Error(`AV-CDP-08 failed: frontend cache size is too small ${JSON.stringify(frontendStatusAfterInstall)}`);
  }
  const inspectorResponse = await fetch(`http://127.0.0.1:${proxyPort}/_bifrost/api/devtools/frontend/inspector.html?ws=127.0.0.1:${proxyPort}/_bifrost/api/devtools/cdp/${pageId}`);
  if (!inspectorResponse.ok) {
    throw new Error(`AV-CDP-08 failed: inspector.html returned ${inspectorResponse.status}`);
  }
  const inspectorHtml = await inspectorResponse.text();
  if (!inspectorHtml.includes('Runtime.js') || !inspectorHtml.includes('inspector.js')) {
    throw new Error('AV-CDP-08 failed: inspector.html did not look like Chrome DevTools frontend');
  }
  const screencastAppResponse = await fetch(`http://127.0.0.1:${proxyPort}/_bifrost/api/devtools/frontend/screencast/ScreencastApp.js`);
  if (!screencastAppResponse.ok) {
    throw new Error(`AV-CDP-18 failed: ScreencastApp.js returned ${screencastAppResponse.status}`);
  }
  const screencastAppJs = await screencastAppResponse.text();
  if (!screencastAppJs.includes("createSetting('screencastEnabled', false)") || !screencastAppJs.includes('this._enabledSetting.set(false)')) {
    throw new Error('AV-CDP-18 failed: embedded frontend did not force screencastEnabled=false');
  }
  if (!screencastAppJs.includes('this._toggleButton.setEnabled(false)') || !screencastAppJs.includes('return;')) {
    throw new Error('AV-CDP-18 failed: embedded frontend did not disable screencast creation path');
  }
}
async function assertEmbeddedFrontendHasNoScreencastPane(frameLocator) {
  const frameHandle = await frameLocator.elementHandle();
  const frontendFrame = await frameHandle?.contentFrame();
  if (!frontendFrame) {
    throw new Error('AV-CDP-18 failed: embedded Chrome DevTools iframe content frame was not available');
  }
  await frontendFrame.waitForLoadState('domcontentloaded', { timeout: 20000 });
  await frontendFrame.waitForTimeout(1500);
  const visibleScreencastControls = await frontendFrame
    .locator('[title="Toggle screencast"], .largeicon-phone, .screencast')
    .evaluateAll((nodes) =>
      nodes.filter((node) => {
        const element = node;
        const style = window.getComputedStyle(element);
        return (
          style.display !== 'none' &&
          style.visibility !== 'hidden' &&
          style.opacity !== '0' &&
          (element.offsetWidth > 0 || element.offsetHeight > 0 || element.getClientRects().length > 0)
        );
      }).length,
    );
  if (visibleScreencastControls !== 0) {
    throw new Error(`AV-CDP-18 failed: embedded frontend still exposes screencast controls/pane (${visibleScreencastControls})`);
  }
}
const cdpReplies = await roundtripCdp(cdpTarget.webSocketDebuggerUrl, [
  { id: 1, method: 'Browser.getVersion' },
  { id: 2, method: 'DOM.getDocument' },
  { id: 3, method: 'Runtime.evaluate', params: { expression: 'document.title' } },
  { id: 4, method: 'Page.getResourceTree' },
  { id: 5, method: 'Page.getFrameTree' },
  { id: 6, method: 'CSS.getMatchedStylesForNode', params: { nodeId: 2 } },
  { id: 7, method: 'CSS.getComputedStyleForNode', params: { nodeId: 2 } },
  { id: 8, method: 'CSS.getInlineStylesForNode', params: { nodeId: 2 } },
  { id: 9, method: 'Runtime.getHeapUsage' },
  { id: 10, method: 'Network.enable' },
  { id: 11, method: 'Page.enable' },
  { id: 12, method: 'Debugger.enable' },
]);
if (!cdpReplies.some((reply) => reply.id === 1 && reply.result?.product === 'Bifrost DevTools Bridge')) {
  throw new Error(`AV-CDP-05 failed: Browser.getVersion did not return Bifrost product ${JSON.stringify(cdpReplies)}`);
}
if (!cdpReplies.some((reply) => reply.id === 2 && reply.result?.root?.nodeName === '#document')) {
  throw new Error(`AV-CDP-05 failed: DOM.getDocument did not return document root ${JSON.stringify(cdpReplies)}`);
}
if (!cdpReplies.some((reply) => reply.id === 3 && reply.error?.message === 'requires_control')) {
  throw new Error(`AV-CDP-05 failed: Runtime.evaluate did not preserve control policy ${JSON.stringify(cdpReplies)}`);
}
for (const id of [4, 5, 6, 7, 8, 9, 10, 11, 12]) {
  const reply = cdpReplies.find((candidate) => candidate.id === id);
  if (!reply || reply.error) {
    throw new Error(`AV-CDP-05 failed: CDP capability id ${id} did not succeed ${JSON.stringify(cdpReplies)}`);
  }
}
await assertFlattenedCdpSession(cdpTarget.webSocketDebuggerUrl, page);
await assertRealtimeCdpUpdates(cdpTarget.webSocketDebuggerUrl, page);
await assertDomSyncIsChangeDriven(cdpTarget.webSocketDebuggerUrl);
await assertInspectorSelectionSurvivesDomNoise(cdpTarget.webSocketDebuggerUrl, page);
await assertCdpProtocolMatrix(cdpTarget.webSocketDebuggerUrl, page);

const session = await api('/devtools/sessions', {
  method: 'POST',
  body: JSON.stringify({ page_id: activeDebugPage.page_id }),
});
const snapshot = await api(`/devtools/sessions/${session.session_id}/snapshot`);
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
if (!snapshot.network.some((entry) => entry.url.includes('/devtools/api/ping'))) {
  throw new Error('AV-CDP-11 failed: network event missing');
}
if (!JSON.stringify(snapshot.storage || {}).includes('bifrost-storage-key')) {
  throw new Error('AV-CDP-11 failed: storage snapshot missing');
}

const evalResponse = await fetch(`${admin}/devtools/sessions/${session.session_id}/commands`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ command: 'runtime.evaluate', params: { expression: 'document.title' } }),
});
if (evalResponse.status < 400) {
  throw new Error('AV-CDP-03 failed: runtime.evaluate unexpectedly succeeded in read mode');
}
const evalError = await evalResponse.text();
if (!evalError.includes('requires_control')) {
  throw new Error(`AV-CDP-03 failed: expected requires_control, got ${evalError}`);
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
await page.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
await page.waitForTimeout(800);
const controlDebugPage = await waitForDevToolsPage(
  (candidate) => candidate.url.includes('case=av-cdp-control') && candidate.mode === 'control' && candidate.state === 'discoverable',
  'AV-CDP-14 failed: control mode page not listed',
);
const controlCdpTarget = (await api('/devtools/cdp/json/list')).find((target) => target.id === controlDebugPage.page_id);
const controlEvalReplies = await roundtripCdp(controlCdpTarget.webSocketDebuggerUrl, [
  { id: 401, method: 'Runtime.evaluate', params: { expression: 'document.querySelector("#debug-fixture").dataset.case' } },
]);
const controlEvalReply = controlEvalReplies.find((reply) => reply.id === 401);
if (controlEvalReply?.result?.result?.value !== 'basic') {
  throw new Error(`AV-CDP-14 failed: Runtime.evaluate did not execute in page bridge control mode ${JSON.stringify(controlEvalReplies)}`);
}
let auditRecords = await api('/devtools/audit/evaluate?limit=5');
const datasetExpression = 'document.querySelector("#debug-fixture").dataset.case';
if (!auditRecords.some((entry) => entry.expression_sha256 === sha256(datasetExpression) && entry.expression_preview === datasetExpression && entry.target_page_id === controlDebugPage.page_id)) {
  throw new Error(`F3 failed: Runtime.evaluate audit record missing ${JSON.stringify(auditRecords)}`);
}
await api('/rules/devtools-page-bridge-api', {
  method: 'PUT',
  body: JSON.stringify({
    content: allowlistRuleContent,
    enabled: true,
  }),
});
await new Promise((resolve) => setTimeout(resolve, 1500));
await page.goto(`http://devtools-fixture.test:${sitePort}/basic.html?case=av-cdp-allowlist`, { waitUntil: 'load' });
await page.waitForFunction(() => window.__BIFROST_DEVTOOLS_BRIDGE__?.state === 'connected', null, { timeout: 8000 });
await page.waitForTimeout(800);
const allowlistDebugPage = await waitForDevToolsPage(
  (candidate) => candidate.url.includes('case=av-cdp-allowlist') && candidate.mode === 'control' && candidate.state === 'discoverable',
  'F3 failed: allowlist control mode page not listed',
);
if (!allowlistDebugPage.evaluate_allowlist?.includes('^document\\.title$')) {
  throw new Error(`F3 failed: evaluate allowlist was not propagated to debug page ${JSON.stringify(allowlistDebugPage)}`);
}
const allowlistCdpTarget = (await api('/devtools/cdp/json/list')).find((target) => target.id === allowlistDebugPage.page_id);
const allowlistReplies = await roundtripCdp(allowlistCdpTarget.webSocketDebuggerUrl, [
  { id: 411, method: 'Runtime.evaluate', params: { expression: 'document.title' } },
  { id: 412, method: 'Runtime.evaluate', params: { expression: 'document.cookie' } },
]);
const allowedEvalReply = allowlistReplies.find((reply) => reply.id === 411);
const rejectedEvalReply = allowlistReplies.find((reply) => reply.id === 412);
if (allowedEvalReply?.result?.result?.value !== 'Bifrost DevTools Basic') {
  throw new Error(`F3 failed: allowlisted Runtime.evaluate should succeed ${JSON.stringify(allowlistReplies)}`);
}
if (rejectedEvalReply?.error?.code !== -32000 || rejectedEvalReply.error.message !== 'evaluate not in allowlist') {
  throw new Error(`F3 failed: non-allowlisted Runtime.evaluate should be rejected ${JSON.stringify(allowlistReplies)}`);
}
auditRecords = await api('/devtools/audit/evaluate?limit=5');
if (!auditRecords.some((entry) => entry.expression_sha256 === sha256('document.cookie') && entry.rejected_by_allowlist === true)) {
  throw new Error(`F3 failed: rejected allowlist audit record missing ${JSON.stringify(auditRecords)}`);
}
for (let i = 0; i < 7; i += 1) {
  const replies = await roundtripCdp(allowlistCdpTarget.webSocketDebuggerUrl, [
    { id: 420 + i, method: 'Runtime.evaluate', params: { expression: 'document.title' } },
  ]);
  const reply = replies.find((candidate) => candidate.id === 420 + i);
  if (reply?.result?.result?.value !== 'Bifrost DevTools Basic') {
    throw new Error(`F3 failed: ring-buffer seed evaluate ${i} failed ${JSON.stringify(replies)}`);
  }
}
auditRecords = await api('/devtools/audit/evaluate?limit=50');
if (auditRecords.length !== 5) {
  throw new Error(`F3 failed: audit ring buffer should be bounded to capacity 5, got ${auditRecords.length}: ${JSON.stringify(auditRecords)}`);
}
const finalDebugPage = allowlistDebugPage;

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
const cdpFrontendLog = [];
const adminPage = await adminContext.newPage();
adminPage.on('console', (msg) => {
  cdpFrontendLog.push({ type: msg.type(), text: msg.text() });
});
adminPage.on('websocket', (ws) => {
  if (!ws.url().includes('/_bifrost/api/devtools/cdp/')) return;
  cdpFrontendLog.push({ direction: 'open', url: ws.url() });
  ws.on('framesent', (event) => {
    try {
      cdpFrontendLog.push({ direction: 'sent', url: ws.url(), data: JSON.parse(event.payload) });
    } catch {
      cdpFrontendLog.push({ direction: 'sent_raw', url: ws.url(), payload: String(event.payload).slice(0, 120) });
    }
  });
  ws.on('framereceived', (event) => {
    try {
      cdpFrontendLog.push({ direction: 'recv', url: ws.url(), data: JSON.parse(event.payload) });
    } catch {
      cdpFrontendLog.push({ direction: 'recv_raw', url: ws.url(), payload: String(event.payload).slice(0, 120) });
    }
  });
});
await adminPage.goto(webui, { waitUntil: 'load' });
await adminPage.getByText('DevTools', { exact: true }).waitFor({ timeout: 10000 });
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
await adminPage.getByText('DevTools', { exact: true }).click();
await adminPage.getByTestId('devtools-page-list').waitFor({ timeout: 8000 });
await adminPage.getByPlaceholder('Search online pages').fill('av-cdp-allowlist');
const primaryCard = adminPage.getByTestId('devtools-page-card').filter({ hasText: 'Bifrost DevTools Basic' });
await primaryCard.waitFor({ timeout: 8000 });
const visiblePrimaryCards = await primaryCard.count();
if (visiblePrimaryCards !== 1) {
  throw new Error(`AV-CDP-13 failed: WebUI should list one target after same-tab reload, got ${visiblePrimaryCards}`);
}
await primaryCard.click();
await adminPage.getByTestId('devtools-detail').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-back').waitFor({ timeout: 8000 });
if (process.env.BIFROST_TEST_INSTALL_EMBEDDED_DEVTOOLS === '1') {
  await adminPage.getByRole('button', { name: 'Install Chrome DevTools' }).click();
  await adminPage.getByRole('progressbar').waitFor({ timeout: 5000 });
  const devtoolsFrame = adminPage.locator('iframe[title="Chrome DevTools Frontend"]');
  await devtoolsFrame.waitFor({ timeout: 60000 });
  const frameSrc = await devtoolsFrame.getAttribute('src');
  if (!frameSrc?.includes('/_bifrost/api/devtools/frontend/inspector.html?ws=')) {
    throw new Error(`AV-CDP-08 failed: WebUI did not embed Chrome DevTools frontend iframe (${frameSrc})`);
  }
  await assertEmbeddedFrontendHasNoScreencastPane(devtoolsFrame);
  async function assertFrontendProtocolForPage(pageId, label) {
    await adminPage.waitForTimeout(8000);
    const pageEntries = cdpFrontendLog.filter((entry) => !entry.url || entry.url.includes(pageId));
    const sentMethods = pageEntries
      .filter((entry) => entry.direction === 'sent' && entry.data?.method)
      .map((entry) => entry.data.method);
    const recvErrors = pageEntries
      .filter((entry) => entry.direction === 'recv' && entry.data?.error)
      .map((entry) => entry.data.error.message);
    const recvIds = new Set(
      pageEntries
        .filter((entry) => entry.direction === 'recv' && entry.data?.id !== undefined)
        .map((entry) => entry.data.id),
    );
    const sentIds = pageEntries
      .filter((entry) => entry.direction === 'sent' && entry.data?.id !== undefined)
      .map((entry) => entry.data.id);
    const missingReplies = sentIds.filter((id) => !recvIds.has(id));
    const mismatchedSessionReplies = pageEntries
      .filter((entry) => entry.direction === 'sent' && entry.data?.id !== undefined && entry.data?.sessionId)
      .filter((sent) => {
        const reply = pageEntries.find((entry) => entry.direction === 'recv' && entry.data?.id === sent.data.id);
        return !reply || reply.data?.sessionId !== sent.data.sessionId;
      });
    const unsupported = recvErrors.filter((message) => String(message).startsWith('unsupported CDP method'));
    const screencastFrames = pageEntries.filter((entry) => entry.direction === 'recv' && entry.data?.method === 'Page.screencastFrame');
    const requiredFrontendMethods = [
      'Network.enable',
      'Page.enable',
      'Page.getResourceTree',
      'Runtime.enable',
      'Debugger.enable',
      'DOM.enable',
      'CSS.enable',
      'Target.setAutoAttach',
      'Target.setDiscoverTargets',
      'DOM.getDocument',
      'CSS.getMatchedStylesForNode',
      'CSS.getComputedStyleForNode',
    ];
    const missingFrontendMethods = requiredFrontendMethods.filter((method) => !sentMethods.includes(method));
    const frontendConsoleErrors = cdpFrontendLog.filter((entry) => entry.type === 'error');
    if (!cdpFrontendLog.some((entry) => entry.direction === 'open' && entry.url.includes(pageId))) {
      throw new Error(`AV-CDP-08 failed: Chrome DevTools frontend did not open Bifrost CDP websocket for ${label}`);
    }
    if (missingReplies.length) {
      throw new Error(`AV-CDP-08 failed: Chrome DevTools frontend CDP requests missed replies for ${label}: ${missingReplies.join(',')}`);
    }
    if (mismatchedSessionReplies.length) {
      throw new Error(`AV-CDP-10 failed: Chrome DevTools frontend CDP session replies were not routed for ${label}: ${JSON.stringify(mismatchedSessionReplies.map((entry) => entry.data))}`);
    }
    if (missingFrontendMethods.length) {
      throw new Error(`AV-CDP-08 failed: Chrome DevTools frontend did not request expected startup methods for ${label}: ${missingFrontendMethods.join(',')}`);
    }
    if (unsupported.length) {
      throw new Error(`AV-CDP-08 failed: Chrome DevTools frontend hit unsupported CDP methods for ${label}: ${unsupported.join(' | ')}`);
    }
    if (screencastFrames.length) {
      throw new Error(`AV-CDP-15 failed: Chrome DevTools frontend received disabled screencast frames for ${label}`);
    }
    if (frontendConsoleErrors.length) {
      throw new Error(`AV-CDP-08 failed: Chrome DevTools frontend console errors ${frontendConsoleErrors.map((entry) => entry.text).join(' | ')}`);
    }
  }
  await assertFrontendProtocolForPage(finalDebugPage.page_id, 'primary page');
  await assertEmbeddedFrontendInstalled(finalDebugPage.page_id);
  await adminPage.getByTestId('devtools-back').click();
  await adminPage.getByTestId('devtools-page-list').waitFor({ timeout: 8000 });
  await adminPage.getByPlaceholder('Search online pages').fill('secondary');
  const secondaryCard = adminPage.getByTestId('devtools-page-card').filter({ hasText: 'Bifrost DevTools Secondary' });
  await secondaryCard.waitFor({ timeout: 8000 });
  await secondaryCard.click();
  await adminPage.getByTestId('devtools-detail').waitFor({ timeout: 8000 });
  await adminPage.waitForFunction(
    (pageId) => document.querySelector('iframe[title="Chrome DevTools Frontend"]')?.getAttribute('src')?.includes(pageId),
    secondaryDebugPage.page_id,
    { timeout: 15000 },
  );
  await assertFrontendProtocolForPage(secondaryDebugPage.page_id, 'secondary page');
  await assertEmbeddedFrontendHasNoScreencastPane(devtoolsFrame);
} else {
  await adminPage.getByText('Debug URL', { exact: true }).waitFor({ timeout: 8000 });
  const debugUrlText = await adminPage.locator('textarea:not([aria-hidden="true"])').inputValue();
  if (!debugUrlText.startsWith('devtools://devtools/bundled/inspector.html?ws=')) {
    throw new Error(`AV-CDP-07 failed: WebUI did not expose system Chrome DevTools URL (${debugUrlText})`);
  }
  await adminContext.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: `http://127.0.0.1:${proxyPort}` });
  await adminPage.getByRole('button', { name: 'Copy Debug URL' }).click();
  const copiedDebugUrl = await adminPage.evaluate(() => navigator.clipboard.readText());
  if (copiedDebugUrl !== debugUrlText) {
    throw new Error(`AV-CDP-07 failed: Copy Debug URL copied wrong value ${copiedDebugUrl}`);
  }
  await adminPage.getByRole('button', { name: 'Open in Chrome DevTools' }).click();
  const chromeTarget = await waitForChromeTarget(
    (target) => target.url?.startsWith('devtools://devtools/bundled/inspector.html?ws=') && target.url.includes(finalDebugPage.page_id),
  );
  if (!chromeTarget.webSocketDebuggerUrl) {
    throw new Error(`AV-CDP-07 failed: opened Chrome DevTools target missing debugger URL ${JSON.stringify(chromeTarget)}`);
  }
  const chromeDevtoolsUrl = await chromeTargetEvaluate(chromeTarget.webSocketDebuggerUrl, 'location.href');
  if (!String(chromeDevtoolsUrl).includes(`/_bifrost/api/devtools/cdp/${finalDebugPage.page_id}`)) {
    throw new Error(`AV-CDP-07 failed: system Chrome DevTools opened the wrong target ${chromeDevtoolsUrl}`);
  }
  await waitForChromeTargetExpression(
    chromeTarget.webSocketDebuggerUrl,
    'document.readyState === "complete" && location.href.includes("/_bifrost/api/devtools/cdp/")',
  );
  await chromeTargetScreenshot(chromeTarget.webSocketDebuggerUrl);
  await adminPage.getByRole('button', { name: 'Install Chrome DevTools' }).waitFor({ timeout: 8000 });
}

await api('/auth/passwd', {
  method: 'POST',
  body: JSON.stringify({ username: 'admin', password: 'Str0ngPass123!' }),
});
await api('/auth/remote', {
  method: 'POST',
  body: JSON.stringify({ enabled: true }),
});
const login = await api('/auth/login', {
  method: 'POST',
  body: JSON.stringify({ username: 'admin', password: 'Str0ngPass123!' }),
});
const localOrigin = `http://127.0.0.1:${proxyPort}`;
const noTokenHandshake = await rawWsHandshake(wsPathFromUrl(allowlistCdpTarget.webSocketDebuggerUrl), localOrigin);
if (noTokenHandshake.status !== 401 || !noTokenHandshake.response.includes('missing_token')) {
  throw new Error(`F23 failed: auth-enabled CDP websocket without token should be rejected ${JSON.stringify(noTokenHandshake)}`);
}
const tokenHandshake = await rawWsHandshake(wsPathFromUrl(allowlistCdpTarget.webSocketDebuggerUrl), localOrigin, login.token);
if (tokenHandshake.status !== 101) {
  throw new Error(`F23 failed: auth-enabled CDP websocket with token should upgrade ${JSON.stringify(tokenHandshake)}`);
}

await browser.close();
if (process.env.BIFROST_TEST_INSTALL_EMBEDDED_DEVTOOLS === '1') {
  console.log('AV-CDP-01/02/03/04/05/06/08/09/10/11/12/13/14/15/16/17/18/19/20 plus embedded Chrome DevTools install, iframe, protocol matrix, card navigation, page switching, no screencast pane, and stable Elements selection passed');
} else {
  console.log('AV-CDP-01/02/03/04/05/06/07/09/10/11/12/13/14/15/16/17/19/20 plus WebUI card navigation, protocol matrix, and system Chrome open passed');
}
NODE
