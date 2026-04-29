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
if [ "$PROXY_PORT" = "9900" ] || [ "$SITE_PORT" = "9900" ]; then
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
  sleep 0.5
  rm -rf "$TEST_ROOT" 2>/dev/null || {
    sleep 1
    rm -rf "$TEST_ROOT" 2>/dev/null || true
  }
}
trap cleanup EXIT

printf '%s\n' '<!doctype html><html><head><title>Bifrost DevTools Basic</title><script>document.cookie="bifrost-cookie-key=cookie-ready; path=/"; localStorage.setItem("bifrost-storage-key","storage-ready"); sessionStorage.setItem("bifrost-session-key","session-ready"); console.log("bifrost-devtools-basic-ready"); console.warn("bifrost-devtools-warning-ready");</script></head><body><div id="debug-fixture" data-case="basic" style="color: rgb(11, 22, 33); display: block;">ready</div><script>fetch("/devtools/api/ping?case=basic").catch(function(){})</script></body></html>' > "$SITE_DIR/basic.html"
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
PROXY_PORT="$PROXY_PORT" SITE_PORT="$SITE_PORT" RULE_CONTENT="$RULE_CONTENT" CONTROL_RULE_CONTENT="$CONTROL_RULE_CONTENT" node --input-type=module <<'NODE'
import { chromium } from './web/node_modules/playwright/index.mjs';
import NodeWebSocket from './web/node_modules/ws/index.js';
import net from 'node:net';
import { createHash, randomBytes } from 'node:crypto';

const proxyPort = process.env.PROXY_PORT;
const sitePort = process.env.SITE_PORT;
const ruleContent = process.env.RULE_CONTENT;
const controlRuleContent = process.env.CONTROL_RULE_CONTENT;
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
const snapshot = await collectSessionSnapshot(
  session.session_id,
  'full',
  (candidate) =>
    candidate.dom_snapshot?.includes('id="debug-fixture"') &&
    JSON.stringify(candidate.dom_tree || {}).includes('debug-fixture') &&
    candidate.console?.some((entry) => entry.text.includes('bifrost-devtools-basic-ready')) &&
    candidate.network?.some((entry) => entry.url.includes('/devtools/api/ping')) &&
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
if (!snapshot.network.some((entry) => entry.url.includes('/devtools/api/ping'))) {
  throw new Error('AV-CDP-11 failed: network event missing');
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
await adminPage.getByPlaceholder('Search online pages').fill('av-cdp-control');
const primaryCard = adminPage.getByTestId('devtools-page-card').filter({ hasText: 'Bifrost DevTools Basic' });
await primaryCard.waitFor({ timeout: 8000 });
const visiblePrimaryCards = await primaryCard.count();
if (visiblePrimaryCards !== 1) {
  throw new Error(`AV-CDP-13 failed: WebUI should list one target after same-tab reload, got ${visiblePrimaryCards}`);
}
await primaryCard.click();
await adminPage.getByTestId('devtools-detail').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-back').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-custom-workspace').waitFor({ timeout: 8000 });
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
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-elements-panel').getByText(/debug-fixture-manual-refresh/).waitFor({ timeout: 8000 });
await panelSearch.fill('manual-refresh');
await adminPage.locator('[data-testid="devtools-dom-node"][data-selected="true"]').filter({ hasText: 'debug-fixture-manual-refresh' }).waitFor({ timeout: 8000 });
await panelSearch.fill('');
await adminPage.getByRole('tab', { name: /Network/ }).click();
await adminPage.getByTestId('devtools-network-panel').getByText(/devtools\/api\/ping/).waitFor({ timeout: 8000 });
await page.evaluate(() => fetch('/devtools/api/extra?case=webui-network-complete').catch(() => {}));
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-network-panel').getByText(/webui-network-complete/).waitFor({ timeout: 8000 });
await panelSearch.fill('webui-network-complete');
await adminPage.getByTestId('devtools-network-panel').getByText(/webui-network-complete/).waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-network-panel').getByText(/devtools\/api\/ping/).waitFor({ state: 'detached', timeout: 8000 });
await panelSearch.fill('');
await adminPage.getByRole('tab', { name: /LocalStorage/ }).click();
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-key').waitFor({ timeout: 8000 });
await adminPage.getByRole('tab', { name: /Cookies/ }).click();
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-key').waitFor({ timeout: 8000 });
await adminPage.getByRole('tab', { name: /SessionStorage/ }).click();
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-key').waitFor({ timeout: 8000 });
await adminPage.getByRole('tab', { name: /LocalStorage/ }).click();
await page.evaluate(() => {
  document.cookie = 'bifrost-cookie-live=cookie-live; path=/';
  localStorage.setItem('bifrost-storage-live', 'storage-live');
  sessionStorage.setItem('bifrost-session-live', 'session-live');
});
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-live').waitFor({ timeout: 8000 });
await panelSearch.fill('bifrost-storage-live');
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-key').waitFor({ state: 'detached', timeout: 8000 });
await panelSearch.fill('');
await adminPage.getByRole('tab', { name: /Cookies/ }).click();
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-live').waitFor({ timeout: 8000 });
await adminPage.getByRole('tab', { name: /SessionStorage/ }).click();
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-live').waitFor({ timeout: 8000 });
await adminPage.getByRole('tab', { name: /LocalStorage/ }).click();
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
await adminPage.getByRole('tab', { name: /Cookies/ }).click();
await adminPage.getByTestId('devtools-storage-add').click();
await adminPage.getByTestId('devtools-storage-key').fill('bifrost-cookie-edit');
await adminPage.getByTestId('devtools-storage-value').fill('cookie-edited');
await adminPage.getByTestId('devtools-storage-save').click();
await page.waitForFunction(() => document.cookie.includes('bifrost-cookie-edit=cookie-edited'), null, { timeout: 8000 });
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-edit').waitFor({ timeout: 8000 });
await adminPage.getByLabel('Delete bifrost-cookie-edit').click();
await page.waitForFunction(() => !document.cookie.includes('bifrost-cookie-edit='), null, { timeout: 8000 });
await adminPage.getByTestId('devtools-cookies-panel').getByText('bifrost-cookie-edit').waitFor({ state: 'detached', timeout: 8000 });
await adminPage.getByRole('tab', { name: /LocalStorage/ }).click();
await adminPage.getByTestId('devtools-storage-add').click();
await adminPage.getByTestId('devtools-storage-key').fill('bifrost-storage-edit');
await adminPage.getByTestId('devtools-storage-value').fill('storage-edited');
await adminPage.getByTestId('devtools-storage-save').click();
await page.waitForFunction(() => localStorage.getItem('bifrost-storage-edit') === 'storage-edited', null, { timeout: 8000 });
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-edit').waitFor({ timeout: 8000 });
await adminPage.getByLabel('Delete bifrost-storage-edit').click();
await page.waitForFunction(() => localStorage.getItem('bifrost-storage-edit') === null, null, { timeout: 8000 });
await adminPage.getByTestId('devtools-local-storage-panel').getByText('bifrost-storage-edit').waitFor({ state: 'detached', timeout: 8000 });
await adminPage.getByRole('tab', { name: /SessionStorage/ }).click();
await adminPage.getByTestId('devtools-storage-add').click();
await adminPage.getByTestId('devtools-storage-key').fill('bifrost-session-edit');
await adminPage.getByTestId('devtools-storage-value').fill('session-edited');
await adminPage.getByTestId('devtools-storage-save').click();
await page.waitForFunction(() => sessionStorage.getItem('bifrost-session-edit') === 'session-edited', null, { timeout: 8000 });
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-edit').waitFor({ timeout: 8000 });
await adminPage.getByLabel('Delete bifrost-session-edit').click();
await page.waitForFunction(() => sessionStorage.getItem('bifrost-session-edit') === null, null, { timeout: 8000 });
await adminPage.getByTestId('devtools-session-storage-panel').getByText('bifrost-session-edit').waitFor({ state: 'detached', timeout: 8000 });
await adminPage.getByRole('tab', { name: /Console/ }).click();
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-devtools-basic-ready').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-devtools-warning-ready').waitFor({ timeout: 8000 });
await page.evaluate(() => {
  console.info('bifrost-console-info-live');
  console.debug('bifrost-console-debug-live');
  console.error('bifrost-console-error-live');
});
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-info-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-debug-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-panel').getByText('bifrost-console-error-live').waitFor({ timeout: 8000 });
await adminPage.getByTestId('devtools-console-row-log').getByText('bifrost-devtools-basic-ready').waitFor({ timeout: 8000 });
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
await adminPage.getByRole('tab', { name: /Elements/ }).click();
await adminPage.getByTestId('devtools-refresh').click();
await adminPage.getByTestId('devtools-elements-tree').getByText(/debug-fixture/).first().waitFor({ timeout: 8000 });
await adminPage.getByRole('tab', { name: /Console/ }).click();
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
console.log('DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed');
NODE
