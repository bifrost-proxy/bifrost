// Legacy traffic suite: extract focused fixture modules as lifecycle ownership
// is isolated, preserving the registered end-to-end cases and assertions.
import { expect } from "@playwright/test";
import type { APIRequestContext, Page } from "@playwright/test";
import {
  Agent as HttpAgent,
  createServer,
  request as httpRequest,
} from "node:http";
import type { AddressInfo } from "node:net";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { execFile, spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import fs from "node:fs/promises";
import WebSocket, { WebSocketServer } from "ws";
import { HttpProxyAgent } from "http-proxy-agent";
import { HttpsProxyAgent } from "https-proxy-agent";
import { createServer as createHttpsServer } from "node:https";

export const execFileAsync = promisify(execFile);

export const proxyUrl =
  process.env.PROXY_URL ||
  `http://127.0.0.1:${process.env.BIFROST_UI_TEST_PORT ?? process.env.BACKEND_PORT ?? 9910}`;

export const proxyHost = new URL(proxyUrl);

export const proxyPort = proxyHost.port || "80";

export const apiBase =
  process.env.ADMIN_API_BASE || `${proxyUrl.replace(/\/$/, "")}/_bifrost/api`;

export const currentFilePath = fileURLToPath(
  new URL("../traffic.spec.ts", import.meta.url),
);

export const decodeQueryJson = <T>(value: string): T => {
  const base64 = value.replace(/-/g, "+").replace(/_/g, "/");
  const padded = base64 + "=".repeat((4 - (base64.length % 4)) % 4);
  return JSON.parse(Buffer.from(padded, "base64").toString("utf8")) as T;
};

export const startMockServer = async () => {
  const server = createServer((req, res) => {
    res.statusCode = 200;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ path: req.url || "/" }));
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;

  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeIdleConnections?.();
        server.closeAllConnections?.();
        server.close((err?: Error) => (err ? reject(err) : resolve()));
      }),
  };
};

export const startSseServer = async () => {
  const server = createServer((req, res) => {
    const url = req.url || "";
    if (!url.startsWith("/sse-test")) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    for (let i = 1; i <= 40; i += 1) {
      if (i === 20) {
        res.write(
          [
            `id: ${i}`,
            "data: target-long line 1",
            "data: target-long line 2",
            "data: target-long line 3",
            "data: target-long line 4",
            "data: target-long line 5",
            "data: target-long line 6",
            "data: target-long line 7",
            "data: target-long line 8",
            "data: target-long line 9",
            "data: target-long line 10",
            "data: target-long line 11",
            "data: target-long line 12",
            "data: target-long line 13",
            "data: target-long line 14",
            "data: target-long line 15",
            "data: target-long line 16",
            "data: target-long line 17",
            "data: target-long line 18",
            "data: target-long line 19",
            "data: target-long line 20",
            "data: target-long line 21",
            "data: target-long line 22",
            "data: target-long line 23",
            "data: target-long line 24",
            "data: target-long line 25",
            "",
            "",
          ].join("\n"),
        );
        continue;
      }
      if (i === 1) {
        res.write(`id: ${i}\ndata: alpha\n\n`);
        continue;
      }
      if (i === 2) {
        res.write(`id: ${i}\ndata: beta\n\n`);
        continue;
      }
      res.write(`id: ${i}\ndata: event-${i}\n\n`);
    }
    res.end();
  });

  await new Promise<void>((resolve) => server.listen(0, resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeIdleConnections?.();
        server.closeAllConnections?.();
        server.close((err?: Error) => (err ? reject(err) : resolve()));
      }),
  };
};

export const startOpenAiLikeSseServer = async () => {
  const server = createServer((req, res) => {
    const url = req.url || "";
    if (!url.startsWith("/openai-sse-test")) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });

    for (let i = 1; i <= 3; i += 1) {
      res.write(
        `data: ${JSON.stringify({
          id: "chatcmpl-ui-test",
          object: "chat.completion.chunk",
          created: 1710000000,
          model: "gpt-4o-mini",
          choices: [
            {
              index: 0,
              delta: {
                role: i === 1 ? "assistant" : undefined,
                content: `token-${i}`,
              },
              finish_reason: null,
            },
          ],
        })}\n\n`,
      );
    }
    res.write(
      `data: ${JSON.stringify({
        id: "chatcmpl-ui-test",
        object: "chat.completion.chunk",
        created: 1710000000,
        model: "gpt-4o-mini",
        choices: [
          {
            index: 0,
            delta: {},
            finish_reason: "stop",
          },
        ],
      })}\n\n`,
    );
    res.write("data: [DONE]\n\n");
    res.end();
  });

  await new Promise<void>((resolve) => server.listen(0, resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeIdleConnections?.();
        server.closeAllConnections?.();
        server.close((err?: Error) => (err ? reject(err) : resolve()));
      }),
  };
};

export const startWsServer = async () => {
  const wss = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  wss.on("connection", (socket: WebSocket) => {
    socket.on("message", (data: WebSocket.RawData) => {
      socket.send(data);
    });
  });
  await new Promise<void>((resolve) => wss.on("listening", resolve));
  const port = (wss.address() as AddressInfo).port;
  return {
    port,
    waitForClient: async () => {
      await expect
        .poll(() => wss.clients.size, { timeout: 15000 })
        .toBeGreaterThan(0);
    },
    close: () =>
      new Promise<void>((resolve, reject) =>
        wss.close((err?: Error) => (err ? reject(err) : resolve())),
      ),
  };
};

export const startTlsWsServer = async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "bifrost-ui-wss-"));
  const key = path.join(directory, "key.pem");
  const cert = path.join(directory, "cert.pem");
  await execFileAsync("openssl", [
    "req",
    "-x509",
    "-newkey",
    "rsa:2048",
    "-nodes",
    "-days",
    "1",
    "-subj",
    "/CN=127.0.0.1",
    "-addext",
    "subjectAltName=IP:127.0.0.1",
    "-keyout",
    key,
    "-out",
    cert,
  ]);
  const server = createHttpsServer({
    key: await fs.readFile(key),
    cert: await fs.readFile(cert),
  });
  const wss = new WebSocketServer({ server });
  wss.on("connection", (socket) =>
    socket.on("message", (data, isBinary) => {
      socket.send(data, { binary: isBinary });
    }),
  );
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    port: (server.address() as AddressInfo).port,
    close: async () => {
      for (const socket of wss.clients) socket.terminate();
      await new Promise<void>((resolve) => wss.close(() => resolve()));
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
      await fs.rm(directory, { recursive: true, force: true });
    },
  };
};

export const sendProxyRequest = async (
  url: string,
  targetProxyUrl = proxyUrl,
) => {
  await execFileAsync(
    "curl",
    ["-sS", "--fail", "--noproxy", "", "-x", targetProxyUrl, url],
    { timeout: 10000 },
  );
};

export const clearTraffic = async (request: APIRequestContext) => {
  await request.delete(`${apiBase}/traffic`);
};

export const waitForRecordedTraffic = async (
  request: APIRequestContext,
  path: string,
) => {
  await expect
    .poll(
      async () => {
        const response = await request.get(
          `${apiBase}/traffic/updates?limit=500`,
        );
        return response.ok() ? await response.text() : "";
      },
      { timeout: 10000 },
    )
    .toContain(path);
};

export const findFreePort = async () => {
  return await new Promise<number>((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        server.close(() => reject(new Error("Failed to allocate a free port")));
        return;
      }
      const { port } = address;
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }
        resolve(port);
      });
    });
  });
};

export const waitForBackendReady = async (baseApi: string) => {
  await expect
    .poll(
      async () => {
        try {
          const response = await fetch(`${baseApi}/proxy/address`);
          return response.ok;
        } catch {
          return false;
        }
      },
      { timeout: 30000 },
    )
    .toBe(true);
};

export const startIsolatedBackend = async () => {
  const repoRoot = path.resolve(path.dirname(currentFilePath), "../../..");
  const port = await findFreePort();
  const runtimeDir = await fs.mkdtemp(
    path.join(os.tmpdir(), "bifrost-ui-traffic-"),
  );
  const dataDir = path.join(runtimeDir, "data");
  const logPath = path.join(runtimeDir, "backend.log");
  const binPath = process.env.BIFROST_UI_TEST_PREBUILT_BINARY
    ? path.resolve(repoRoot, process.env.BIFROST_UI_TEST_PREBUILT_BINARY)
    : path.join(
        process.env.BIFROST_UI_TEST_TARGET_DIR ||
          path.join(repoRoot, ".bifrost-ui-target"),
        "debug",
        "bifrost",
      );
  await fs.mkdir(dataDir, { recursive: true });
  const logStream = createWriteStream(logPath, { flags: "a" });

  const child = spawn(
    binPath,
    [
      "start",
      "--host",
      "127.0.0.1",
      "-p",
      String(port),
      "--unsafe-ssl",
      "--skip-cert-check",
      "--no-system-proxy",
      "--access-mode",
      "allow_all",
    ],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        BIFROST_DATA_DIR: dataDir,
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT: "1",
        BIFROST_DISABLE_TRAY: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  child.stdout?.pipe(logStream);
  child.stderr?.pipe(logStream);

  const baseUrl = `http://127.0.0.1:${port}`;
  const baseApi = `${baseUrl}/_bifrost/api`;
  await waitForBackendReady(baseApi);

  return {
    port,
    proxyUrl: baseUrl,
    baseUrl,
    baseApi,
    dataDir,
    close: async () => {
      if (child.exitCode === null) {
        await new Promise<void>((resolve) => {
          const timer = setTimeout(() => {
            if (child.exitCode === null) child.kill("SIGKILL");
          }, 5000);
          // close follows exit and draining stdout/stderr. Ending the log
          // first makes shutdown output fail with ERR_STREAM_WRITE_AFTER_END.
          child.once("close", () => {
            clearTimeout(timer);
            resolve();
          });
          child.kill("SIGTERM");
        });
      }
      child.stdout?.unpipe(logStream);
      child.stderr?.unpipe(logStream);
      await new Promise<void>((resolve) => logStream.end(resolve));
      await fs.rm(runtimeDir, { recursive: true, force: true });
    },
  };
};

export const getTrafficRecordsByApi = async (baseApiUrl: string) => {
  const response = await fetch(`${baseApiUrl}/traffic?limit=100`);
  expect(response.ok).toBeTruthy();
  return (await response.json()) as {
    records?: Array<{
      id: string;
      p?: string;
      lp?: number;
      capp?: string | null;
    }>;
  };
};

export const waitForTrafficRecordByApi = async (
  baseApiUrl: string,
  targetPath: string,
) => {
  let found:
    | { id: string; p?: string; lp?: number; capp?: string | null }
    | undefined;
  await expect
    .poll(
      async () => {
        const payload = await getTrafficRecordsByApi(baseApiUrl);
        found = payload.records?.find((record) => record.p === targetPath);
        return found?.id ?? null;
      },
      { timeout: 15000 },
    )
    .toMatch(/^REQ-/);
  return found as { id: string; p?: string; lp?: number; capp?: string | null };
};

export const updateTrafficClientApp = async (
  dataDir: string,
  recordId: string,
  clientApp: string,
) => {
  const dbPath = path.join(dataDir, "traffic", "traffic.db");
  const script = `
import sqlite3
import sys

db_path, record_id, client_app = sys.argv[1:4]
conn = sqlite3.connect(db_path)
try:
    conn.execute("UPDATE traffic_records SET client_app = ? WHERE id = ?", (client_app, record_id))
    conn.commit()
finally:
    conn.close()
`;

  await execFileAsync("python3", ["-c", script, dbPath, recordId, clientApp], {
    timeout: 10000,
  });
};

export const startTemporaryProxy = async (baseApiUrl: string) => {
  const csrfResponse = await fetch(`${baseApiUrl}/security/csrf`);
  expect(csrfResponse.ok).toBeTruthy();
  const csrf = (await csrfResponse.json()) as {
    csrf_token: string;
    header_name?: string;
  };
  const response = await fetch(`${baseApiUrl}/ports`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      [csrf.header_name || "X-Bifrost-CSRF"]: csrf.csrf_token,
    },
    body: JSON.stringify({
      port: 0,
      host: "127.0.0.1",
      // A temporary listener requires a rule binding. Local upstream requests
      // still pass through unchanged because this fixture host never matches.
      rule_refs: [
        {
          type: "inline_rule",
          content: "unused-port-fixture.test status://204",
        },
      ],
    }),
  });
  const body = await response.text();
  expect(
    response.ok,
    `temporary proxy failed (${response.status}): ${body}`,
  ).toBeTruthy();
  const binding = JSON.parse(body) as { port: number; status: string };
  expect(binding.status).toBe("running");
  expect(binding.port).toBeGreaterThan(0);
  return binding.port;
};

export const waitForClientApp = async (
  baseApiUrl: string,
  recordId: string,
  expectedClientApp: string,
) => {
  await expect
    .poll(
      async () => {
        const response = await fetch(`${baseApiUrl}/traffic/${recordId}`);
        if (!response.ok) {
          return null;
        }
        const payload = (await response.json()) as {
          client_app?: string | null;
        };
        return payload.client_app ?? "";
      },
      { timeout: 15000 },
    )
    .toBe(expectedClientApp);
};

export const searchTraffic = async (
  baseApiUrl: string,
  conditions: Array<{ field: string; operator: string; value: string }>,
) => {
  const csrfResponse = await fetch(`${baseApiUrl}/security/csrf`);
  expect(csrfResponse.ok).toBeTruthy();
  const csrf = (await csrfResponse.json()) as {
    csrf_token: string;
    header_name?: string;
  };
  const response = await fetch(`${baseApiUrl}/search`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      [csrf.header_name || "X-Bifrost-CSRF"]: csrf.csrf_token,
    },
    body: JSON.stringify({
      keyword: "",
      scope: {
        request_body: false,
        response_body: false,
        request_headers: false,
        response_headers: false,
        url: false,
        websocket_messages: false,
        sse_events: false,
        all: true,
      },
      filters: {
        protocols: [],
        status_ranges: [],
        content_types: [],
        conditions,
        client_ips: [],
        client_apps: [],
        domains: [],
      },
      limit: 50,
    }),
  });
  const body = await response.text();
  expect(
    response.ok,
    `search failed (${response.status}): ${body}`,
  ).toBeTruthy();
  return JSON.parse(body) as {
    results: Array<{ record: { p?: string } }>;
    total_matched: number;
  };
};

export const queryTraffic = async (
  baseApiUrl: string,
  params: Record<string, string>,
) => {
  const searchParams = new URLSearchParams(params);
  const response = await fetch(
    `${baseApiUrl}/traffic?${searchParams.toString()}`,
  );
  expect(response.ok).toBeTruthy();
  return (await response.json()) as {
    records: Array<{
      id: string;
      p?: string;
      lp?: number;
      capp?: string | null;
      seq?: number;
    }>;
    total: number;
    server_sequence: number;
  };
};

export const getTrafficStatistics = async (baseApiUrl: string) => {
  const response = await fetch(`${baseApiUrl}/traffic/statistics`);
  if (!response.ok) {
    throw new Error(`traffic statistics failed: ${response.status}`);
  }
  return response.json() as Promise<{
    total_requests: number;
    client_ips: Record<string, number>;
    proxy_ports: Record<string, number>;
    applications: Record<string, number>;
    account_names: Record<string, number>;
    domains: Record<string, number>;
  }>;
};

export const updateTrafficMaxRecords = async (
  baseApiUrl: string,
  maxRecords: number,
) => {
  const csrfResponse = await fetch(`${baseApiUrl}/security/csrf`);
  expect(csrfResponse.ok).toBeTruthy();
  const csrf = (await csrfResponse.json()) as {
    csrf_token: string;
    header_name?: string;
  };
  const response = await fetch(`${baseApiUrl}/config/performance`, {
    method: "PUT",
    headers: {
      "content-type": "application/json",
      [csrf.header_name || "X-Bifrost-CSRF"]: csrf.csrf_token,
    },
    body: JSON.stringify({ max_records: maxRecords }),
  });
  const body = await response.text();
  expect(
    response.ok,
    `performance config update failed (${response.status}): ${body}`,
  ).toBeTruthy();
};

export async function setDocumentVisibility(
  page: Page,
  state: "hidden" | "visible",
) {
  await page.evaluate((nextState) => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => nextState,
    });
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => nextState === "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(
      new Event(nextState === "hidden" ? "pagehide" : "pageshow"),
    );
  }, state);
}

export type TrafficBurstStats = {
  deltaCount: number;
  maxBatch: number;
  totalInserts: number;
  latestOldestSequence: number;
  latestServerTotal: number;
};

export async function installTrafficBurstRecorder(page: Page) {
  await page.addInitScript(() => {
    const nativeWebSocket = window.WebSocket;
    const stats: TrafficBurstStats = {
      deltaCount: 0,
      maxBatch: 0,
      totalInserts: 0,
      latestOldestSequence: 0,
      latestServerTotal: 0,
    };

    class InstrumentedWebSocket extends nativeWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        if (!String(url).includes("/api/push")) return;
        this.addEventListener("message", (event) => {
          if (typeof event.data !== "string") return;
          try {
            const message = JSON.parse(event.data) as {
              type?: string;
              data?: {
                inserts?: unknown[];
                updates?: unknown[];
                oldest_sequence?: number;
                server_total?: number;
              };
            };
            if (message.type !== "traffic_delta" || !message.data) return;
            const batchSize =
              (message.data.inserts?.length ?? 0) +
              (message.data.updates?.length ?? 0);
            stats.deltaCount += 1;
            stats.maxBatch = Math.max(stats.maxBatch, batchSize);
            stats.totalInserts += message.data.inserts?.length ?? 0;
            stats.latestOldestSequence = Math.max(
              stats.latestOldestSequence,
              message.data.oldest_sequence ?? 0,
            );
            stats.latestServerTotal =
              message.data.server_total ?? stats.latestServerTotal;
          } catch {
            // Ignore non-JSON and unrelated push frames without retaining them.
          }
        });
      }
    }

    Object.setPrototypeOf(InstrumentedWebSocket, nativeWebSocket);
    Object.defineProperty(window, "__trafficBurstStats", {
      configurable: true,
      value: stats,
      writable: false,
    });
    window.WebSocket = InstrumentedWebSocket as typeof nativeWebSocket;
  });
}

export async function readTrafficBurstStats(
  page: Page,
): Promise<TrafficBurstStats> {
  return page.evaluate(() => ({
    ...(window as typeof window & { __trafficBurstStats: TrafficBurstStats })
      .__trafficBurstStats,
  }));
}

export type TrafficStatisticsFrame = {
  receivedAt: number;
  totalRequests: number;
  connectionId: number;
};

export type TrafficStatisticsSocketEvent = {
  kind: "open" | "close" | "subscription";
  connectionId: number;
  at: number;
  subscription?: unknown;
};

export async function installTrafficStatisticsRecorder(page: Page) {
  await page.addInitScript(() => {
    const nativeWebSocket = window.WebSocket;
    const frames: TrafficStatisticsFrame[] = [];
    const events: TrafficStatisticsSocketEvent[] = [];
    let nextConnection = 0;
    class StatisticsInstrumentedWebSocket extends nativeWebSocket {
      private diagnosticConnection = 0;
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        if (!String(url).includes("/api/push")) return;
        this.diagnosticConnection = ++nextConnection;
        const connectionId = this.diagnosticConnection;
        this.addEventListener("open", () =>
          events.push({ kind: "open", connectionId, at: performance.now() }),
        );
        this.addEventListener("close", () =>
          events.push({ kind: "close", connectionId, at: performance.now() }),
        );
        this.addEventListener("message", (event) => {
          if (typeof event.data !== "string") return;
          try {
            const message = JSON.parse(event.data) as {
              type?: string;
              data?: { total_requests?: number };
            };
            if (
              message.type === "traffic_statistics" &&
              typeof message.data?.total_requests === "number"
            )
              frames.push({
                receivedAt: performance.now(),
                totalRequests: message.data.total_requests,
                connectionId,
              });
          } catch {
            /* Ignore unrelated frames. */
          }
        });
      }
      override send(
        data: Parameters<InstanceType<typeof nativeWebSocket>["send"]>[0],
      ) {
        if (this.diagnosticConnection && typeof data === "string") {
          try {
            const subscription = JSON.parse(data) as { need_traffic?: boolean };
            if (typeof subscription.need_traffic === "boolean")
              events.push({
                kind: "subscription",
                connectionId: this.diagnosticConnection,
                at: performance.now(),
                subscription,
              });
          } catch {
            /* Ignore non-subscription traffic. */
          }
        }
        super.send(data);
      }
    }
    Object.setPrototypeOf(StatisticsInstrumentedWebSocket, nativeWebSocket);
    Object.defineProperty(window, "__trafficStatisticsFrames", {
      configurable: true,
      value: frames,
    });
    Object.defineProperty(window, "__trafficStatisticsSocketEvents", {
      configurable: true,
      value: events,
    });
    window.WebSocket =
      StatisticsInstrumentedWebSocket as typeof nativeWebSocket;
  });
}

export async function readTrafficStatisticsFrames(
  page: Page,
): Promise<TrafficStatisticsFrame[]> {
  return page.evaluate(() => [
    ...(
      window as typeof window & {
        __trafficStatisticsFrames: TrafficStatisticsFrame[];
      }
    ).__trafficStatisticsFrames,
  ]);
}

export async function readTrafficStatisticsDiagnostics(page: Page) {
  return page.evaluate(() => {
    const state = window as typeof window & {
      __trafficStatisticsFrames?: TrafficStatisticsFrame[];
      __trafficStatisticsSocketEvents?: TrafficStatisticsSocketEvent[];
    };
    return {
      timeOrigin: performance.timeOrigin,
      frames: state.__trafficStatisticsFrames ?? [],
      socketEvents: state.__trafficStatisticsSocketEvents ?? [],
    };
  });
}

export const streamSseViaProxy = async (url: string) => {
  const target = new URL(url);
  await new Promise<void>((resolve, reject) => {
    const req = httpRequest(
      {
        host: proxyHost.hostname,
        port: proxyHost.port || 80,
        method: "GET",
        path: url,
        headers: {
          Host: target.host,
        },
      },
      (res) => {
        res.on("data", () => {});
        res.on("end", () => resolve());
      },
    );
    req.on("error", reject);
    req.end();
  });
};

export const sendWsViaProxy = async (url: string) => {
  await new Promise<void>((resolve, reject) => {
    const agent = url.startsWith("wss:")
      ? new HttpsProxyAgent(proxyUrl)
      : new HttpProxyAgent(proxyUrl);
    const ws = new WebSocket(url, { agent, rejectUnauthorized: false });
    ws.on("open", () => {
      ws.send("hello");
      ws.send(Buffer.from([1, 2, 3, 4, 5, 6]));
      ws.close();
    });
    ws.on("close", () => resolve());
    ws.on("error", (err: Error) => reject(err));
  });
};

export async function sendHttpViaProxy(
  url: string,
  targetProxyUrl = proxyUrl,
  agent?: HttpAgent,
) {
  const targetProxy = new URL(targetProxyUrl);
  await new Promise<void>((resolve, reject) => {
    const req = httpRequest(
      {
        host: targetProxy.hostname,
        port: targetProxy.port || 80,
        method: "GET",
        path: url,
        agent,
        headers: {
          Host: new URL(url).host,
        },
      },
      (res) => {
        res.on("data", () => {});
        res.on("end", () => resolve());
      },
    );
    req.on("error", reject);
    req.end();
  });
}

export async function seedTrafficBatch(
  paths: string[],
  serverPort: number,
  targetProxyUrl = proxyUrl,
  batchSize = 40,
) {
  let nextIndex = 0;
  const agent = new HttpAgent({ keepAlive: true, maxSockets: batchSize });
  const worker = async () => {
    while (nextIndex < paths.length) {
      const currentIndex = nextIndex;
      nextIndex += 1;
      await sendHttpViaProxy(
        `http://127.0.0.1:${serverPort}${paths[currentIndex]}`,
        targetProxyUrl,
        agent,
      );
    }
  };
  try {
    await Promise.all(
      Array.from({ length: Math.min(batchSize, paths.length) }, () => worker()),
    );
  } finally {
    agent.destroy();
  }
}
