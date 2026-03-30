#!/usr/bin/env node

const path = require("node:path");

const wsModulePath = path.join(process.cwd(), "web", "node_modules", "ws");
const { WebSocket } = require(wsModulePath);

const url = process.argv[2];
const count = Number(process.argv[3] || "4");
const waitMs = Number(process.argv[4] || "3000");

if (!url) {
  console.error("usage: ws_channel_limit_probe.js <ws-url> [count] [wait-ms]");
  process.exit(2);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function openClient(name) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${url}?x_client_id=${encodeURIComponent(name)}`);
    const messages = [];
    const errors = [];

    ws.on("open", () => {
      ws.send(JSON.stringify({ need_overview: true }));
      resolve({ name, ws, messages, errors });
    });

    ws.on("message", (buf) => {
      messages.push(buf.toString());
    });

    ws.on("error", (err) => {
      errors.push(err.message);
    });

    ws.on("unexpected-response", (_req, res) => {
      errors.push(`unexpected-response:${res.statusCode}`);
    });

    ws.on("close", (code, reason) => {
      if (reason && reason.length > 0) {
        messages.push(
          JSON.stringify({
            type: "close",
            data: { code, reason: reason.toString() },
          }),
        );
      }
    });
  });
}

(async () => {
  const clients = [];
  for (let i = 1; i <= count; i += 1) {
    clients.push(await openClient(`e2e_chan_${i}_${process.pid}_${Date.now()}`));
    await sleep(800);
  }

  await sleep(waitMs);

  const oldest = clients[0];
  const result = {
    oldest_disconnect: oldest.messages.some((msg) => msg.includes('"type":"disconnect"')),
    oldest_messages: oldest.messages,
    oldest_errors: oldest.errors,
    total_clients: clients.length,
  };

  for (const client of clients) {
    client.ws.close();
  }

  console.log(JSON.stringify(result));
})().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
