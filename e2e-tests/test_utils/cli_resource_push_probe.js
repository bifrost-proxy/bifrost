#!/usr/bin/env node

const fs = require("fs");

const [url, resource, expectedName, mode, readyFile, continueFile, outputFile] =
  process.argv.slice(2);

if (
  !url ||
  !["values", "scripts"].includes(resource) ||
  !expectedName ||
  !["active", "resubscribe"].includes(mode) ||
  !readyFile ||
  !continueFile ||
  !outputFile
) {
  throw new Error(
    "usage: cli_resource_push_probe.js <url> <values|scripts> <expected> <active|resubscribe> <ready> <continue> <output>",
  );
}

const subscriptionKey = resource === "values" ? "need_values" : "need_scripts";
const messageType = resource === "values" ? "values_update" : "scripts_update";
let phase = "initial";
let settled = false;

function snapshotHasExpected(message) {
  if (resource === "values") {
    return (message.data?.values ?? []).some((item) => item.name === expectedName);
  }
  return ["request", "response", "decode", "parser"].some((type) =>
    (message.data?.[type] ?? []).some((item) => item.name === expectedName),
  );
}

function finish(code, detail) {
  if (settled) return;
  settled = true;
  clearTimeout(timeout);
  clearInterval(continuePoll);
  fs.writeFileSync(outputFile, JSON.stringify(detail) + "\n");
  try {
    socket.close();
  } catch {
    // The process exits immediately after preserving the diagnostic payload.
  }
  process.exit(code);
}

const socket = new WebSocket(url);
const timeout = setTimeout(
  () => finish(1, { error: "timeout", resource, expectedName, mode, phase }),
  20_000,
);
let continuePoll;

socket.addEventListener("error", () => {
  finish(1, { error: "websocket error", resource, expectedName, mode, phase });
});

socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ [subscriptionKey]: true }));
});

socket.addEventListener("message", (event) => {
  const message = JSON.parse(String(event.data));
  if (message.type !== messageType) return;

  if (phase === "initial") {
    if (snapshotHasExpected(message)) {
      finish(1, {
        error: "expected resource already existed in initial snapshot",
        resource,
        expectedName,
        mode,
      });
      return;
    }

    fs.writeFileSync(readyFile, "ready\n");
    if (mode === "active") {
      phase = "waiting-active-push";
      return;
    }

    socket.send(JSON.stringify({ [subscriptionKey]: false }));
    phase = "paused";
    continuePoll = setInterval(() => {
      if (fs.existsSync(continueFile)) {
        clearInterval(continuePoll);
        socket.send(JSON.stringify({ [subscriptionKey]: true }));
        phase = "waiting-resubscribe-snapshot";
      }
    }, 50);
    return;
  }

  if (snapshotHasExpected(message)) {
    finish(0, {
      ok: true,
      resource,
      expectedName,
      mode,
      messageType: message.type,
    });
  }
});
