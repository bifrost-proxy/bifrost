#!/usr/bin/env node

const url = process.argv[2];
const durationMs = Number(process.argv[3] || 3200);

if (!url) {
  console.error("usage: metrics_push_probe.js <ws-url> [duration-ms]");
  process.exit(2);
}

const updates = [];
const socket = new WebSocket(url);
let settled = false;

function finish(code, error) {
  if (settled) return;
  settled = true;
  if (error) {
    console.error(error instanceof Error ? error.message : String(error));
  } else {
    process.stdout.write(`${JSON.stringify({ count: updates.length, updates })}\n`);
  }
  process.exit(code);
}

socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ need_metrics: true, metrics_interval_ms: 500 }));
  setTimeout(() => {
    socket.close(1000);
    finish(updates.length > 0 ? 0 : 1, updates.length > 0 ? undefined : "no metrics_update received");
  }, durationMs);
});

socket.addEventListener("message", (event) => {
  try {
    const message = JSON.parse(String(event.data));
    if (message.type === "metrics_update") {
      updates.push(message.data);
    }
  } catch {
    // Ignore non-JSON frames; the probe only validates metrics_update data.
  }
});

socket.addEventListener("error", (event) => finish(1, event.error || "websocket error"));
setTimeout(() => finish(1, "metrics push probe timed out"), durationMs + 5000);
