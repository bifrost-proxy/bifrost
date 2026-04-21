#!/usr/bin/env node

const { WebSocket } = require("../../web/node_modules/ws");
const { HttpProxyAgent } = require("../../web/node_modules/http-proxy-agent");

const [, , proxyUrl, wsUrl, expectedType = "connected", secondaryType = ""] = process.argv;

if (!proxyUrl || !wsUrl) {
  console.error("Usage: ws_via_http_proxy.js <proxyUrl> <wsUrl> [expectedType] [secondaryType]");
  process.exit(2);
}

const agent = new HttpProxyAgent(proxyUrl);
const ws = new WebSocket(wsUrl, { agent });
const seen = new Set();

const timer = setTimeout(() => {
  console.error(`Timed out waiting for ${expectedType}${secondaryType ? ` and ${secondaryType}` : ""}`);
  ws.terminate();
  process.exit(1);
}, 8000);

function maybeFinish() {
  if (!seen.has(expectedType)) {
    return;
  }
  if (secondaryType && !seen.has(secondaryType)) {
    return;
  }
  clearTimeout(timer);
  ws.close();
}

ws.on("message", (data) => {
  const text = String(data);
  console.log(text);
  try {
    const parsed = JSON.parse(text);
    if (parsed?.type) {
      seen.add(parsed.type);
      maybeFinish();
    }
  } catch (error) {
    console.error(`Failed to parse WebSocket message: ${error.message}`);
  }
});

ws.on("error", (error) => {
  clearTimeout(timer);
  console.error(`WebSocket error: ${error.message}`);
  process.exit(1);
});

ws.on("close", () => {
  if (!seen.has(expectedType) || (secondaryType && !seen.has(secondaryType))) {
    process.exit(1);
  }
  process.exit(0);
});
