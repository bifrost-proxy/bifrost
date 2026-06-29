#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
CHROME_BIN="${CHROME_BIN:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "BIFROST_BIN is not executable: $BIFROST_BIN" >&2
  echo "Build it first with: cargo build --bin bifrost" >&2
  exit 1
fi

if [[ ! -x "$CHROME_BIN" ]]; then
  echo "Chrome is not available, skipping browser rule share confirmation E2E"
  exit 0
fi

free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

header_location() {
  python3 - "$1" <<'PY'
import sys
for line in open(sys.argv[1], "rb"):
    text = line.decode("latin1").strip()
    if text.lower().startswith("location:"):
        print(text.split(":", 1)[1].strip())
        break
PY
}

DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-rule-share-browser.XXXXXX")"
SITE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-rule-share-browser-site.XXXXXX")"
CHROME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-rule-share-chrome.XXXXXX")"
PROXY_PORT="$(free_port)"
TARGET_PORT="$(free_port)"
DEBUG_PORT="$(free_port)"
PROXY_PID=""
SITE_PID=""
CHROME_PID=""

terminate_pid() {
  local pid="$1"
  [[ -n "$pid" ]] || return 0
  if ! kill -0 "$pid" 2>/dev/null; then
    return 0
  fi

  kill "$pid" 2>/dev/null || true
  for _ in {1..20}; do
    if ! kill -0 "$pid" 2>/dev/null; then
      wait "$pid" 2>/dev/null || true
      return 0
    fi
    sleep 0.1
  done

  kill -9 "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

cat >"$DATA_DIR/config.toml" <<'EOF'
[sync]
enabled = false
auto_sync = false
remote_base_url = "http://127.0.0.1:9"
probe_interval_secs = 3600
connect_timeout_ms = 100
EOF

cleanup() {
  terminate_pid "$CHROME_PID"
  if [[ -n "$PROXY_PID" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
    kill "$PROXY_PID" 2>/dev/null || true
    wait "$PROXY_PID" 2>/dev/null || true
  fi
  if [[ -n "$SITE_PID" ]] && kill -0 "$SITE_PID" 2>/dev/null; then
    kill "$SITE_PID" 2>/dev/null || true
    wait "$SITE_PID" 2>/dev/null || true
  fi
  rm -rf "$DATA_DIR" "$SITE_DIR" "$CHROME_DIR"
}
trap cleanup EXIT

echo '<html><body>browser rule share target</body></html>' >"$SITE_DIR/browser-target"

python3 -m http.server "$TARGET_PORT" --bind 127.0.0.1 --directory "$SITE_DIR" >/tmp/bifrost-rule-share-browser-site.log 2>&1 &
SITE_PID=$!

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
  -p "$PROXY_PORT" \
  --host 127.0.0.1 \
  --access-mode allow_all \
  --skip-cert-check \
  --no-system-proxy \
  --no-intercept \
  -y >/tmp/bifrost-rule-share-browser-proxy.log 2>&1 &
PROXY_PID=$!

for _ in {1..80}; do
  if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done
curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" >/dev/null

TARGET_URL="http://127.0.0.1:${TARGET_PORT}/browser-target"
SHARE_URL="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share rsq-browser "$TARGET_URL" \
    --content "browser-share.test bp://127.0.0.1:3000"
)"

curl -sS -o /tmp/bifrost-rule-share-browser-body.out \
  -D /tmp/bifrost-rule-share-browser.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL" >/dev/null
grep -Eiq '^HTTP/.* 302' /tmp/bifrost-rule-share-browser.headers
CONFIRM_URL="$(header_location /tmp/bifrost-rule-share-browser.headers)"
[[ "$CONFIRM_URL" == "http://127.0.0.1:${PROXY_PORT}/_bifrost/share/rule?"* ]]

"$CHROME_BIN" \
  --headless=new \
  --disable-gpu \
  --no-first-run \
  --no-default-browser-check \
  --disable-background-networking \
  --user-data-dir="$CHROME_DIR" \
  --remote-debugging-address=127.0.0.1 \
  --remote-debugging-port="$DEBUG_PORT" \
  about:blank >/tmp/bifrost-rule-share-browser-chrome.log 2>&1 &
CHROME_PID=$!

for _ in {1..80}; do
  if curl -fsS "http://127.0.0.1:${DEBUG_PORT}/json/version" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done
curl -fsS "http://127.0.0.1:${DEBUG_PORT}/json/version" >/dev/null

node - "$DEBUG_PORT" "$CONFIRM_URL" "$TARGET_URL" <<'NODE'
const debugPort = process.argv[2];
const confirmUrl = process.argv[3];
const targetUrl = process.argv[4];
const errors = [];
const observedUrls = [];

async function openTab(url) {
  const endpoint = `http://127.0.0.1:${debugPort}/json/new?${encodeURIComponent(url)}`;
  let response = await fetch(endpoint, { method: "PUT" });
  if (!response.ok) {
    response = await fetch(endpoint);
  }
  if (!response.ok) {
    throw new Error(`failed to open Chrome tab: ${response.status}`);
  }
  return response.json();
}

async function connect(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  let seq = 0;
  const pending = new Map();
  ws.addEventListener("message", event => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(JSON.stringify(msg.error)));
      else resolve(msg.result || {});
      return;
    }
    if (msg.method === "Runtime.exceptionThrown") {
      errors.push(JSON.stringify(msg.params.exceptionDetails));
    }
    if (msg.method === "Page.frameNavigated") {
      const url = msg.params.frame?.url || "";
      if (url) observedUrls.push(url);
    }
    if (msg.method === "Network.requestWillBeSent") {
      const url = msg.params.request?.url || "";
      if (url) observedUrls.push(url);
    }
    if (msg.method === "Log.entryAdded") {
      const text = msg.params.entry?.text || "";
      if (/Failed to fetch|Refused to connect|Content Security Policy/i.test(text)) {
        errors.push(text);
      }
    }
  });
  return {
    send(method, params = {}) {
      const id = ++seq;
      ws.send(JSON.stringify({ id, method, params }));
      return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
    },
    close() {
      ws.close();
    },
  };
}

async function evaluate(client, expression) {
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(JSON.stringify(result.exceptionDetails));
  }
  return result.result?.value;
}

const tab = await openTab(confirmUrl);
const client = await connect(tab.webSocketDebuggerUrl);
await client.send("Runtime.enable");
await client.send("Log.enable");
await client.send("Page.enable");
await client.send("Network.enable");

for (let i = 0; i < 80; i += 1) {
  const ready = await evaluate(client, "document.readyState");
  if (ready === "complete") break;
  await new Promise(resolve => setTimeout(resolve, 100));
}

const before = await evaluate(client, `(() => ({
  title: document.querySelector('h1')?.textContent || '',
  hasHashInput: Boolean(document.querySelector('#confirmation')),
  requiresHashText: document.body.innerText.includes('Type the full content hash to apply'),
  applyDisabled: document.querySelector('#apply')?.disabled ?? null,
  status: document.querySelector('#status')?.textContent || ''
}))()`);

if (!before.title.includes("Apply Shared Bifrost Rule")) {
  throw new Error(`unexpected confirmation title: ${before.title}`);
}
if (before.hasHashInput || before.requiresHashText) {
  throw new Error(`confirmation page still asks for hash: ${JSON.stringify(before)}`);
}
if (before.applyDisabled !== false) {
  throw new Error(`apply button should be enabled without hash input: ${JSON.stringify(before)}`);
}

await evaluate(client, "document.querySelector('#apply').click()");

let finalState = null;
for (let i = 0; i < 120; i += 1) {
  finalState = await evaluate(client, `(() => ({
    href: window.location.href,
    status: document.querySelector('#status')?.textContent || '',
    body: document.body?.innerText || ''
  }))()`);
  if (finalState.href === targetUrl || observedUrls.includes(targetUrl)) break;
  if (/Failed to fetch/i.test(finalState.status)) break;
  await new Promise(resolve => setTimeout(resolve, 100));
}

client.close();

if (errors.some(text => /Failed to fetch|Refused to connect|Content Security Policy/i.test(text))) {
  throw new Error(`browser reported fetch/CSP error: ${errors.join("\\n")}`);
}
if (finalState.href !== targetUrl && !observedUrls.includes(targetUrl)) {
  throw new Error(`expected redirect to ${targetUrl}, got ${JSON.stringify(finalState)}; observed=${JSON.stringify(observedUrls)}`);
}
if (/Failed to fetch/i.test(finalState.status)) {
  throw new Error(`Apply Rule showed Failed to fetch: ${JSON.stringify(finalState)}`);
}

console.log("browser apply succeeded without hash");
NODE

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-browser-list.txt
grep -F 'share/rsq-browser [enabled]' /tmp/bifrost-rule-share-browser-list.txt

echo "rule share browser confirmation E2E passed"
