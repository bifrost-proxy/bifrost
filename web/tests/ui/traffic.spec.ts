import { test, expect } from "@playwright/test";
import type { APIRequestContext } from "@playwright/test";
import { createServer, request as httpRequest } from "node:http";
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

const execFileAsync = promisify(execFile);
const proxyUrl =
  process.env.PROXY_URL ||
  `http://127.0.0.1:${process.env.BIFROST_UI_TEST_PORT ?? process.env.BACKEND_PORT ?? 9910}`;
const proxyHost = new URL(proxyUrl);
const proxyPort = proxyHost.port || "80";
const apiBase =
  process.env.ADMIN_API_BASE || `${proxyUrl.replace(/\/$/, "")}/_bifrost/api`;
const currentFilePath = fileURLToPath(import.meta.url);

const startMockServer = async () => {
  const server = createServer((req, res) => {
    res.statusCode = 200;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ path: req.url || "/" }));
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

const startSseServer = async () => {
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

const startOpenAiLikeSseServer = async () => {
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
              delta: { role: i === 1 ? "assistant" : undefined, content: `token-${i}` },
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

const startWsServer = async () => {
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

const sendProxyRequest = async (url: string, targetProxyUrl = proxyUrl) => {
  await execFileAsync(
    "curl",
    ["-sS", "--fail", "-x", targetProxyUrl, url],
    { timeout: 10000 },
  );
};

const clearTraffic = async (request: APIRequestContext) => {
  await request.delete(`${apiBase}/traffic`);
};

const waitForRecordedTraffic = async (request: APIRequestContext, path: string) => {
  await expect
    .poll(
      async () => {
        const response = await request.get(`${apiBase}/traffic/updates?limit=500`);
        return response.ok() ? await response.text() : "";
      },
      { timeout: 10000 },
    )
    .toContain(path);
};

const findFreePort = async () => {
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

const waitForBackendReady = async (baseApi: string) => {
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

const startIsolatedBackend = async () => {
  const repoRoot = path.resolve(path.dirname(currentFilePath), "../../..");
  const port = await findFreePort();
  const runtimeDir = await fs.mkdtemp(path.join(os.tmpdir(), "bifrost-ui-traffic-"));
  const dataDir = path.join(runtimeDir, "data");
  const logPath = path.join(runtimeDir, "backend.log");
  const binPath = path.join(repoRoot, ".bifrost-ui-target", "debug", "bifrost");
  await fs.mkdir(dataDir, { recursive: true });
  const logStream = createWriteStream(logPath, { flags: "a" });

  const child = spawn(
    binPath,
    ["start", "--host", "127.0.0.1", "-p", String(port), "--unsafe-ssl", "--skip-cert-check", "--no-system-proxy", "--access-mode", "allow_all"],
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
      logStream.end();
      if (child.exitCode === null) {
        child.kill("SIGTERM");
        await new Promise<void>((resolve) => {
          child.once("exit", () => resolve());
          setTimeout(() => {
            if (child.exitCode === null) {
              child.kill("SIGKILL");
            }
          }, 5000);
        });
      }
      await fs.rm(runtimeDir, { recursive: true, force: true });
    },
  };
};

const getTrafficRecordsByApi = async (baseApiUrl: string) => {
  const response = await fetch(`${baseApiUrl}/traffic?limit=100`);
  expect(response.ok).toBeTruthy();
  return (await response.json()) as {
    records?: Array<{ id: string; p?: string; lp?: number; capp?: string | null }>;
  };
};

const waitForTrafficRecordByApi = async (baseApiUrl: string, targetPath: string) => {
  let found: { id: string; p?: string; lp?: number; capp?: string | null } | undefined;
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

const updateTrafficClientApp = async (dataDir: string, recordId: string, clientApp: string) => {
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

const updateTrafficListenerPort = async (
  dataDir: string,
  recordId: string,
  listenerPort: number,
) => {
  const dbPath = path.join(dataDir, "traffic", "traffic.db");
  const script = `
import sqlite3
import sys

db_path, record_id, listener_port = sys.argv[1:4]
conn = sqlite3.connect(db_path)
try:
    conn.execute("UPDATE traffic_records SET listener_port = ? WHERE id = ?", (int(listener_port), record_id))
    conn.commit()
finally:
    conn.close()
`;

  await execFileAsync("python3", ["-c", script, dbPath, recordId, String(listenerPort)], {
    timeout: 10000,
  });
};

const waitForClientApp = async (
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
        const payload = (await response.json()) as { client_app?: string | null };
        return payload.client_app ?? "";
      },
      { timeout: 15000 },
    )
    .toBe(expectedClientApp);
};

const searchTraffic = async (
  baseApiUrl: string,
  conditions: Array<{ field: string; operator: string; value: string }>,
) => {
  const response = await fetch(`${baseApiUrl}/search`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
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
  expect(response.ok).toBeTruthy();
  return (await response.json()) as {
    results: Array<{ record: { p?: string } }>;
    total_matched: number;
  };
};

const queryTraffic = async (baseApiUrl: string, params: Record<string, string>) => {
  const searchParams = new URLSearchParams(params);
  const response = await fetch(`${baseApiUrl}/traffic?${searchParams.toString()}`);
  expect(response.ok).toBeTruthy();
  return (await response.json()) as {
    records: Array<{ p?: string; lp?: number; capp?: string | null }>;
    total: number;
  };
};

const streamSseViaProxy = async (url: string) => {
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

const sendWsViaProxy = async (url: string) => {
  await new Promise<void>((resolve, reject) => {
    const agent = new HttpProxyAgent(proxyUrl);
    const ws = new WebSocket(url, { agent });
    ws.on("open", () => {
      ws.send("hello");
      ws.send(Buffer.from([1, 2, 3, 4, 5, 6]));
      ws.close();
    });
    ws.on("close", () => resolve());
    ws.on("error", (err: Error) => reject(err));
  });
};

test("加载流量列表并显示详情", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/hello-${token}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
  await page.reload();
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
  await expect(row).toBeVisible();
  await expect(row).toContainText(proxyPort);
  const firstRow = row;
  await expect(firstRow).toHaveAttribute(
    "data-response-size",
    expect.stringMatching(/^[1-9]\d*$/),
  );
  await firstRow.click();

  const header = page.getByTestId("traffic-detail-header");
  await expect(header).toContainText("GET");
  await expect(header).toHaveAttribute(
    "data-url",
    expect.stringContaining("/hello"),
  );
  await expect(page.getByTestId("traffic-detail")).toContainText("Proxy Port");
  await expect(page.getByTestId("traffic-detail")).toContainText(proxyPort);

  await server.close();
});

test("独立详情路由加载单条请求", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/detail-route-${token}`;

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
    await page.reload();

    const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
    await expect(row).toBeVisible();
    const recordId = await row.getAttribute("data-record-id");
    expect(recordId).toBeTruthy();

    await page.goto(`/_bifrost/traffic/detail?id=${recordId}`);
    await expect(page).toHaveURL(new RegExp(`/traffic/detail\\?id=${recordId}$`));
    await expect(page.getByText(`Request ID: ${recordId}`)).toBeVisible();
    await expect(page.getByTestId("traffic-detail")).toBeVisible();
    await expect(page.getByTestId("traffic-detail-header")).toContainText(path);
  } finally {
    await server.close();
  }
});

test("详情页 Copy as cURL 会包含 POST body", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/copy-curl-body-${token}`;
  const body = JSON.stringify({ hello: token });

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);

    await execFileAsync(
      "curl",
      [
        "-sS",
        "--fail",
        "-x",
        proxyUrl,
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
        "--data",
        body,
        `http://127.0.0.1:${server.port}${path}`,
      ],
      { timeout: 10000 },
    );

    await page.reload();

    const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
    await expect(row).toBeVisible();
    await row.click();

    await page.getByTestId("traffic-detail-copy-trigger").click();
    await page.getByText("Copy as cURL").click();

    const readCopiedText = () =>
      page.evaluate(async () => navigator.clipboard.readText());

    await expect.poll(readCopiedText).toContain("--data");
    await expect.poll(readCopiedText).toContain(body);
  } finally {
    await server.close();
  }
});


test("左侧 Filters 展示基础请求数量", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await sendProxyRequest(`http://127.0.0.1:${server.port}/filter-count-a-${token}`);
    await sendProxyRequest(`http://127.0.0.1:${server.port}/filter-count-b-${token}`);

    await expect(page.getByLabel("Local (127.0.0.1) count")).toHaveText("2");
    await expect(page.getByLabel("127.0.0.1 count").first()).toHaveText("2");
  } finally {
    await server.close();
  }
});

test("切换页面后保留已加载流量并持续接收 push", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const firstPath = `/persist-first-${token}`;
  const secondPath = `/persist-second-${token}`;

  try {
    await page.goto("/_bifrost/settings");
    await expect(page).toHaveURL(/\/_bifrost\/settings$/);

    await sendProxyRequest(`http://127.0.0.1:${server.port}${firstPath}`);
    await waitForRecordedTraffic(request, firstPath);

    await page.getByText("Network", { exact: true }).click();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page.reload();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: firstPath }).first(),
    ).toBeVisible();

    await page.getByText("Settings", { exact: true }).click();
    await expect(page).toHaveURL(/\/_bifrost\/settings$/);

    await sendProxyRequest(`http://127.0.0.1:${server.port}${secondPath}`);
    await waitForRecordedTraffic(request, secondPath);

    await page.getByText("Network", { exact: true }).click();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: firstPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: secondPath }).first(),
    ).toBeVisible();
  } finally {
    await server.close();
  }
});

test("Header 仅在存在差异时显示切换", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/headers-plain-${token}`;
  const ruleName = `header-diff-${token}`;
  const createRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: `127.0.0.1 resHeaders://X-UI-Header-Diff=${token}`,
      enabled: true,
    },
  });
  expect(createRuleRes.ok()).toBeTruthy();

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
    await page.reload();
    const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
    await expect(row).toBeVisible();
    await row.click();
    await page.getByTestId("request-tab-header").click();

    await expect(page.getByTestId("request-header-view-mode-tabs")).toHaveCount(0);
    await expect(page.getByTestId("request-header-view-tab-current")).toHaveCount(0);
    await expect(page.getByTestId("request-header-view-tab-original")).toHaveCount(0);

    await page.getByTestId("response-tab-header").click();
    await expect(page.getByTestId("response-header-view-mode-tabs")).toBeVisible();
    await expect(page.getByTestId("response-header-view-tab-current")).toHaveCount(1);
    await expect(page.getByTestId("response-header-view-tab-original")).toHaveCount(1);
  } finally {
    await request.delete(`${apiBase}/rules/${encodeURIComponent(ruleName)}`);
    await server.close();
  }
});

test("清空流量时前端立即清理", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/clear-${token}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
  const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
  await expect(row).toBeVisible();

  let deleteSeen = false;
  await page.route("**/_bifrost/api/traffic", async (route, req) => {
    if (req.method() === "DELETE") {
      deleteSeen = true;
      await new Promise((r) => setTimeout(r, 2000));
    }
    await route.continue();
  });

  await page.getByTestId("toolbar-clear-all").click();

  await expect(row).toHaveCount(0, { timeout: 500 });
  expect(deleteSeen).toBeTruthy();

  await server.close();
});

test("Client App 空值与模糊匹配会同步作用于列表查询和 Fuzzy Search", async () => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const emptyClientPath = `/client-empty-${token}`;
  const fuzzyClientPath = `/client-fuzzy-${token}`;
  const fuzzyClientApp = `Client Search ${token}`;
  const fuzzyKeyword = token.slice(0, 6);

  try {
    await sendProxyRequest(`http://127.0.0.1:${server.port}${emptyClientPath}`, backend.proxyUrl);
    await sendProxyRequest(`http://127.0.0.1:${server.port}${fuzzyClientPath}`, backend.proxyUrl);

    const emptyRecord = await waitForTrafficRecordByApi(backend.baseApi, emptyClientPath);
    const fuzzyRecord = await waitForTrafficRecordByApi(backend.baseApi, fuzzyClientPath);

    await updateTrafficClientApp(backend.dataDir, emptyRecord.id, "");
    await updateTrafficClientApp(backend.dataDir, fuzzyRecord.id, fuzzyClientApp);
    await waitForClientApp(backend.baseApi, emptyRecord.id, "");
    await waitForClientApp(backend.baseApi, fuzzyRecord.id, fuzzyClientApp);

    const emptyTraffic = await queryTraffic(backend.baseApi, {
      client_app_empty: "true",
      limit: "50",
    });
    expect(emptyTraffic.records.map((item) => item.p)).toContain(emptyClientPath);
    expect(emptyTraffic.records.map((item) => item.p)).not.toContain(fuzzyClientPath);

    const emptySearch = await searchTraffic(backend.baseApi, [
      { field: "client_app", operator: "is_empty", value: "" },
    ]);
    expect(emptySearch.results.map((item) => item.record.p)).toContain(emptyClientPath);
    expect(emptySearch.results.map((item) => item.record.p)).not.toContain(fuzzyClientPath);

    const fuzzyTraffic = await queryTraffic(backend.baseApi, {
      client_app: fuzzyKeyword,
      client_app_match: "contains",
      limit: "50",
    });
    expect(fuzzyTraffic.records.map((item) => item.p)).toContain(fuzzyClientPath);
    expect(fuzzyTraffic.records.map((item) => item.p)).not.toContain(emptyClientPath);

    const fuzzySearch = await searchTraffic(backend.baseApi, [
      { field: "client_app", operator: "contains", value: fuzzyKeyword },
    ]);
    expect(fuzzySearch.results.map((item) => item.record.p)).toContain(fuzzyClientPath);
    expect(fuzzySearch.results.map((item) => item.record.p)).not.toContain(emptyClientPath);
  } finally {
    await backend.close();
    await server.close();
  }
});

test("主筛选器支持按代理端口过滤 Traffic", async ({ page }) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const mainPortPath = `/port-filter-main-${token}`;
  const otherPortPath = `/port-filter-other-${token}`;
  const otherPort = backend.port + 1;

  try {
    await sendProxyRequest(`http://127.0.0.1:${server.port}${mainPortPath}`, backend.proxyUrl);
    await sendProxyRequest(`http://127.0.0.1:${server.port}${otherPortPath}`, backend.proxyUrl);

    const mainRecord = await waitForTrafficRecordByApi(backend.baseApi, mainPortPath);
    const otherRecord = await waitForTrafficRecordByApi(backend.baseApi, otherPortPath);
    await updateTrafficListenerPort(backend.dataDir, otherRecord.id, otherPort);

    const portTraffic = await queryTraffic(backend.baseApi, {
      listener_port: String(backend.port),
      limit: "50",
    });
    expect(portTraffic.records.map((item) => item.p)).toContain(mainPortPath);
    expect(portTraffic.records.map((item) => item.p)).not.toContain(otherPortPath);
    expect(mainRecord.lp).toBe(backend.port);

    const portSearch = await searchTraffic(backend.baseApi, [
      { field: "listener_port", operator: "equals", value: String(backend.port) },
    ]);
    expect(portSearch.results.map((item) => item.record.p)).toContain(mainPortPath);
    expect(portSearch.results.map((item) => item.record.p)).not.toContain(otherPortPath);

    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: mainPortPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPortPath }).first(),
    ).toBeVisible();

    if ((await page.getByPlaceholder("Enter value...").count()) === 0) {
      await page.getByRole("button", { name: "Add Filter" }).click();
    }
    await page.getByRole("combobox").first().click();
    await page
      .locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden) .ant-select-item-option-content")
      .filter({ hasText: "Port" })
      .click();
    await expect(page.getByText("Equals").first()).toBeVisible();
    await page.getByPlaceholder("Enter proxy port...").first().fill(String(backend.port));

    await expect(
      page.getByTestId("traffic-row").filter({ hasText: mainPortPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPortPath }),
    ).toHaveCount(0);
  } finally {
    await backend.close();
    await server.close();
  }
});

test("主筛选器支持临时停用单条条件", async ({ page }) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const targetPath = `/filter-enabled-target-${token}`;
  const otherPath = `/filter-enabled-other-${token}`;

  try {
    await sendProxyRequest(`http://127.0.0.1:${server.port}${targetPath}`, backend.proxyUrl);
    await sendProxyRequest(`http://127.0.0.1:${server.port}${otherPath}`, backend.proxyUrl);
    await waitForTrafficRecordByApi(backend.baseApi, targetPath);
    await waitForTrafficRecordByApi(backend.baseApi, otherPath);

    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }).first(),
    ).toBeVisible();

    await page.getByRole("button", { name: "Add Filter" }).click();
    const enabledCheckbox = page.getByRole("checkbox", { name: "Enable filter" }).first();
    await expect(enabledCheckbox).toBeChecked();

    await page.getByRole("combobox").first().click();
    await page
      .locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden) .ant-select-item-option-content")
      .filter({ hasText: "Path" })
      .click();
    await page.getByPlaceholder("Enter value...").first().fill(targetPath);

    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }),
    ).toHaveCount(0);

    await enabledCheckbox.uncheck();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }).first(),
    ).toBeVisible();

    await enabledCheckbox.check();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }),
    ).toHaveCount(0);
  } finally {
    await backend.close();
    await server.close();
  }
});

async function sendHttpViaProxy(url: string) {
  await new Promise<void>((resolve, reject) => {
    const req = httpRequest(
      {
        host: proxyHost.hostname,
        port: proxyHost.port || 80,
        method: "GET",
        path: url,
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

async function seedTrafficBatch(paths: string[], serverPort: number) {
  const batchSize = 40;
  for (let i = 0; i < paths.length; i += batchSize) {
    const batch = paths.slice(i, i + batchSize);
    await Promise.all(
      batch.map((path) =>
        sendHttpViaProxy(`http://127.0.0.1:${serverPort}${path}`),
      ),
    );
  }
}

test("实时更新与页面刷新保持数据", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const firstPath = `/first-${token}`;
  const secondPath = `/second-${token}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${firstPath}`);
  await expect(
    page.getByTestId("traffic-row").filter({ hasText: firstPath }).first(),
  ).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${secondPath}`);
  await expect(
    page.getByTestId("traffic-row").filter({ hasText: secondPath }).first(),
  ).toBeVisible();

  await page.reload();
  await expect(
    page.getByTestId("traffic-row").filter({ hasText: secondPath }).first(),
  ).toBeVisible();

  await server.close();
});

test("刷新时首屏仍能保留最新窗口中的筛选结果", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const targetPrefix = `/latest-window-${token}`;
  const filterParam = Buffer.from(
    JSON.stringify([
      {
        id: `filter-${token}`,
        field: "path",
        operator: "contains",
        value: targetPrefix,
      },
    ]),
  ).toString("base64url");

  try {
    const paths = Array.from({ length: 1000 }, (_, index) => `/noise-${token}-${index}`);
    paths.push(`${targetPrefix}-a`, `${targetPrefix}-b`);
    await seedTrafficBatch(paths, server.port);

    await page.goto(`/_bifrost/traffic?filter=${filterParam}`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: `${targetPrefix}-a` }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: `${targetPrefix}-b` }).first(),
    ).toBeVisible();

    await page.reload();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: `${targetPrefix}-a` }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: `${targetPrefix}-b` }).first(),
    ).toBeVisible();
  } finally {
    await server.close();
  }
});

test("后台历史回填会自动补齐首屏之外的旧记录", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const targetPrefix = `/historic-backfill-${token}`;
  const filterParam = Buffer.from(
    JSON.stringify([
      {
        id: `filter-historic-${token}`,
        field: "path",
        operator: "contains",
        value: targetPrefix,
      },
    ]),
  ).toString("base64url");

  try {
    const paths = [`${targetPrefix}-a`, `${targetPrefix}-b`];
    paths.push(...Array.from({ length: 1000 }, (_, index) => `/historic-noise-${token}-${index}`));
    await seedTrafficBatch(paths, server.port);

    await page.goto(`/_bifrost/traffic?filter=${filterParam}`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await expect(
      page.getByTestId("traffic-row").filter({ hasText: `${targetPrefix}-a` }).first(),
    ).toBeVisible({ timeout: 15000 });
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: `${targetPrefix}-b` }).first(),
    ).toBeVisible({ timeout: 15000 });
  } finally {
    await server.close();
  }
});

test("订阅更新提示与滚动状态一致", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  for (let i = 0; i < 30; i += 1) {
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}/batch-${token}-${i}`,
    );
  }

  await expect(page.getByTestId("traffic-row").first()).toBeVisible();

  const scrollContainer = page.getByTestId("traffic-table-scroll");
  await scrollContainer.evaluate((el) => {
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));
  });

  await sendProxyRequest(`http://127.0.0.1:${server.port}/latest-${token}`);
  const newTrafficIndicator = page.getByTestId("traffic-new-indicator");
  await expect(newTrafficIndicator).toBeVisible();
  await newTrafficIndicator.click({ force: true });
  await expect(
    page.getByTestId("traffic-row").filter({ hasText: `/latest-${token}` }).first(),
  ).toBeVisible();

  await server.close();
});

test("WebSocket 帧与 size 更新展示正确", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startWsServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const wsPath = `/ws-${token}`;

  await sendWsViaProxy(`ws://127.0.0.1:${server.port}${wsPath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  const wsRow = page
    .getByTestId("traffic-row")
    .filter({ hasText: wsPath })
    .last();
  const frameCountAttr = await wsRow.getAttribute("data-frame-count");
  const frameCount = Number(frameCountAttr ?? "0");
  expect(frameCount).toBeGreaterThanOrEqual(2);
  await wsRow.click();
  await page.getByTestId("response-tab-messages").click();

  await expect(page.getByTestId("ws-frames-pane")).toBeVisible();
  await expect(page.getByTestId("ws-frames-summary")).toContainText("frames");
  const frameRows = page.getByTestId("ws-frame-row");
  await expect(frameRows.first()).toBeVisible();
  expect(await frameRows.count()).toBeGreaterThanOrEqual(2);
  await expect(page.getByTestId("ws-frames-table")).toContainText("6 B");

  await server.close();
});

test("WebSocket 外部站点回显与 size 增长", async ({ page, request }) => {
  await clearTraffic(request);
  await sendWsViaProxy("wss://echo.websocket.org/");

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  const wsRow = page
    .getByTestId("traffic-row")
    .filter({ hasText: "echo.websocket.org" })
    .first();
  await expect(wsRow).toBeVisible();

  await expect
    .poll(async () => {
      const frameCountAttr = await wsRow.getAttribute("data-frame-count");
      return Number(frameCountAttr ?? "0");
    })
    .toBeGreaterThanOrEqual(2);

  await wsRow.click();
  await page.getByTestId("response-tab-messages").click();

  await expect(page.getByTestId("ws-frames-pane")).toBeVisible();
  await expect(page.getByTestId("ws-frames-table")).toContainText("hello");
  await expect(page.getByTestId("ws-frames-table")).toContainText("6 B");
});

test("WebSocket 详情 Messages 面板打开后可实时收到新消息", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startWsServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const wsPath = `/ws-live-${token}`;
  const wsUrl = `ws://127.0.0.1:${server.port}${wsPath}`;
  const agent = new HttpProxyAgent(proxyUrl);
  const ws = new WebSocket(wsUrl, { agent });

  try {
    await new Promise<void>((resolve, reject) => {
      ws.once("open", resolve);
      ws.once("error", reject);
    });
    await server.waitForClient();

    ws.send(`seed-${token}`);

    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const wsRow = page.getByTestId("traffic-row").filter({ hasText: wsPath }).last();
    await expect(wsRow).toBeVisible();
    await wsRow.click();
    await page.getByTestId("response-tab-messages").click();

    await expect(page.getByTestId("ws-frames-pane")).toBeVisible();
    await expect(page.getByTestId("ws-frames-table")).toContainText(`seed-${token}`);
    await page.waitForTimeout(500);

    const livePayload = `after-open-${token}`;
    for (let i = 0; i < 5; i += 1) {
      ws.send(`${livePayload}-${i}`);
      await page.waitForTimeout(200);
    }

    await expect(page.getByTestId("ws-frames-table")).toContainText(livePayload);
  } finally {
    if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
      await new Promise<void>((resolve) => {
        ws.once("close", resolve);
        ws.close();
      });
    }
    await server.close();
  }
});

test("WebSocket 列表未到底部时不应强制滚动到底部", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startWsServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const wsPath = `/ws-scroll-${token}`;
  const wsUrl = `ws://127.0.0.1:${server.port}${wsPath}`;

  const agent = new HttpProxyAgent(proxyUrl);
  const ws = new WebSocket(wsUrl, { agent });

  await new Promise<void>((resolve, reject) => {
    ws.once("open", resolve);
    ws.once("error", reject);
  });

  for (let i = 0; i < 120; i += 1) {
    ws.send(`seed-${token}-${i}`);
  }

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  const wsRow = page
    .getByTestId("traffic-row")
    .filter({ hasText: wsPath })
    .last();
  await expect(wsRow).toBeVisible();
  await wsRow.click();
  await page.getByTestId("response-tab-messages").click();

  await expect(page.getByTestId("ws-frames-pane")).toBeVisible();

  const summary = page.getByTestId("ws-frames-summary");
  const summaryHandle = await summary.elementHandle();
  if (summaryHandle) {
    await page.waitForFunction(
      (el: SVGElement | HTMLElement) => {
        const text = el.textContent || "";
        const match = text.match(/(\d+)\s+of\s+(\d+)\s+frames/);
        if (!match) return false;
        return Number(match[1]) >= 50;
      },
      summaryHandle,
    );
  }

  const scrollContainer = page.getByTestId("ws-frames-table");
  await scrollContainer.evaluate((el) => {
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));
  });
  const scrollTopBefore = await scrollContainer.evaluate((el) => el.scrollTop);

  for (let i = 120; i < 140; i += 1) {
    ws.send(`append-${token}-${i}`);
  }

  if (summaryHandle) {
    await page.waitForFunction(
      (el: SVGElement | HTMLElement) => {
        const text = el.textContent || "";
        const match = text.match(/(\d+)\s+of\s+(\d+)\s+frames/);
        if (!match) return false;
        return Number(match[1]) >= 70;
      },
      summaryHandle,
    );
  }

  const scrollTopAfter = await scrollContainer.evaluate((el) => el.scrollTop);
  expect(scrollTopAfter).toBe(scrollTopBefore);

  await new Promise<void>((resolve) => {
    ws.once("close", resolve);
    ws.close();
  });
  await server.close();
});

test("SSE 事件订阅与列表展示正确", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startSseServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-test?token=${token}`;

  await streamSseViaProxy(`http://127.0.0.1:${server.port}${ssePath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  let recordId: string | null = null;
  await expect
    .poll(async () => {
      const row = page
        .getByTestId("traffic-row")
        .filter({ hasText: "/sse-test" })
        .last();
      recordId = await row.getAttribute("data-record-id");
      return recordId;
    })
    .toMatch(/^REQ-/);

  const sseRow = page.locator(
    `[data-testid="traffic-row"][data-record-id="${recordId}"]`,
  );
  await expect(sseRow).toHaveAttribute(
    "data-response-size",
    expect.stringMatching(/^[1-9]\d*$/),
  );
  await sseRow.click();
  await page.getByTestId("response-tab-messages").click();

  await expect(page.getByTestId("sse-message-container")).toBeVisible();
  await expect(page.getByTestId("sse-message-count")).toContainText("events");
  await expect(page.getByTestId("sse-message-list")).toContainText("alpha");
  await expect(page.getByTestId("sse-message-list")).toContainText("beta");

  await server.close();
});

test("SSE 详情通过弹窗展开且不改变列表项高度", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startSseServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-test?token=${token}`;

  await streamSseViaProxy(`http://127.0.0.1:${server.port}${ssePath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  let recordId: string | null = null;
  await expect
    .poll(async () => {
      const row = page
        .getByTestId("traffic-row")
        .filter({ hasText: "/sse-test" })
        .last();
      recordId = await row.getAttribute("data-record-id");
      return recordId;
    })
    .toMatch(/^REQ-/);

  const sseRow = page.locator(
    `[data-testid="traffic-row"][data-record-id="${recordId}"]`,
  );
  await sseRow.click();
  await page.getByTestId("response-tab-messages").click();

  const container = page.getByTestId("sse-message-container");
  await expect(container).toBeVisible();

  const search = container.getByPlaceholder("Search events...");
  await search.fill("target-long");

  const detailCard = container
    .getByTestId("sse-event-card")
    .filter({ hasText: "target-long" })
    .first();

  await expect(detailCard).toBeVisible();
  const beforeBox = await detailCard.boundingBox();
  expect(beforeBox).not.toBeNull();

  await detailCard.getByTestId("sse-event-toggle").click();

  await expect
    .poll(async () => {
      const currentHeight = (await detailCard.boundingBox())?.height || 0;
      return Math.abs(currentHeight - (beforeBox?.height || 0));
    })
    .toBeLessThan(2);
  await expect(page.getByTestId("sse-event-detail-content")).toBeVisible();
  await expect(page.getByTestId("sse-event-detail-content")).toContainText("target-long");
  await page
    .locator(".ant-modal")
    .filter({ has: page.getByTestId("sse-event-detail-content") })
    .locator(".ant-modal-footer")
    .getByRole("button", { name: "Close" })
    .click();
  await expect(page.getByTestId("sse-event-detail-content")).not.toBeVisible();

  await server.close();
});

test("SSE 外部站点流式更新与 size 增长", async ({ page, request }) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/external-sse-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write("id: 1\ndata: alpha\n\n");
    const timer = setInterval(() => {
      res.write(`id: ${Date.now()}\ndata: update-${Date.now()}\n\n`);
    }, 400);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "6",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const row = page
      .getByTestId("traffic-row")
      .filter({ hasText: ssePath })
      .first();
    await expect(row).toBeVisible();

    const sizeBefore = Number((await row.getAttribute("data-response-size")) || "0");
    await expect
      .poll(async () => {
        const size = await row.getAttribute("data-response-size");
        return Number(size || "0");
      })
      .toBeGreaterThan(sizeBefore);

    await row.click();
    await page.getByTestId("response-tab-messages").click();
    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect
      .poll(async () => {
        const text = await page.getByTestId("sse-message-count").textContent();
        const match = text?.match(/(\d+)\s+events/);
        return match ? Number(match[1]) : 0;
      })
      .toBeGreaterThan(0);
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("SSE 详情 Messages 面板打开后可实时收到新事件", async ({ page, request }) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-live-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(`id: 1\ndata: seed-${token}\n\n`);
    let counter = 0;
    const timer = setInterval(() => {
      counter += 1;
      res.write(`id: ${counter + 1}\ndata: after-open-${token}-${counter}\n\n`);
      if (counter >= 6) {
        clearInterval(timer);
        res.end();
      }
    }, 400);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "8",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const sseRow = page.getByTestId("traffic-row").filter({ hasText: ssePath }).first();
    await expect(sseRow).toBeVisible();
    await sseRow.click();
    await page.getByTestId("response-tab-messages").click();

    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect(page.getByTestId("sse-message-list")).toContainText(`seed-${token}`);
    await expect(page.getByTestId("sse-message-list")).toContainText(`after-open-${token}`);
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("SSE 详情切换 Response tab 后消息列表不应丢失", async ({ page, request }) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-tab-switch-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(`id: 1\ndata: seed-${token}\n\n`);
    let counter = 0;
    const timer = setInterval(() => {
      counter += 1;
      res.write(`id: ${counter + 1}\ndata: after-tab-${token}-${counter}\n\n`);
      if (counter >= 8) {
        clearInterval(timer);
        res.end();
      }
    }, 300);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "8",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const sseRow = page.getByTestId("traffic-row").filter({ hasText: ssePath }).first();
    await expect(sseRow).toBeVisible();
    await sseRow.click();
    await page.getByTestId("response-tab-messages").click();

    const messages = page.getByTestId("sse-message-list");
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-tab-${token}-1`);

    await page.getByTestId("response-tab-header").click();
    await expect(page.getByTestId("response-header-view-mode-tabs")).toBeVisible();
    await page.waitForTimeout(700);

    await page.getByTestId("response-tab-messages").click();
    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-tab-${token}-1`);
    await expect(messages).toContainText(`after-tab-${token}-2`);
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("SSE 详情切到弹窗再附回右侧面板后消息列表不应丢失", async ({ page, request }) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-detach-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(`id: 1\ndata: seed-${token}\n\n`);
    let counter = 0;
    const timer = setInterval(() => {
      counter += 1;
      res.write(`id: ${counter + 1}\ndata: after-detach-${token}-${counter}\n\n`);
      if (counter >= 10) {
        clearInterval(timer);
        res.end();
      }
    }, 300);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "10",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const sseRow = page.getByTestId("traffic-row").filter({ hasText: ssePath }).first();
    await expect(sseRow).toBeVisible();
    await sseRow.click();
    await page.getByTestId("response-tab-messages").click();

    const messages = page.getByTestId("sse-message-list");
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-detach-${token}-1`);

    const popupPromise = page.waitForEvent("popup");
    await page.getByTestId("traffic-detail-open-window").click();
    const popup = await popupPromise;
    await popup.waitForLoadState("domcontentloaded");
    await expect(popup.getByTestId("traffic-detail-attach-back")).toBeVisible();

    await popup.getByTestId("traffic-detail-attach-back").click();
    await expect.poll(() => popup.isClosed()).toBe(true);

    await expect(page.getByTestId("traffic-detail")).toBeVisible();
    await page.getByTestId("response-tab-messages").click();
    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-detach-${token}-1`);
    await expect(messages).toContainText(`after-detach-${token}-2`);
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("OpenAI 风格 SSE 会自动打开聚合后的 Response tab", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startOpenAiLikeSseServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/openai-sse-test?token=${token}`;

  await streamSseViaProxy(`http://127.0.0.1:${server.port}${ssePath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  let recordId: string | null = null;
  await expect
    .poll(async () => {
      const row = page
        .getByTestId("traffic-row")
        .filter({ hasText: "/openai-sse-test" })
        .last();
      recordId = await row.getAttribute("data-record-id");
      return recordId;
    })
    .toMatch(/^REQ-/);

  const sseRow = page.locator(
    `[data-testid="traffic-row"][data-record-id="${recordId}"]`,
  );
  await sseRow.click();

  await expect(page.getByTestId("response-tab-openai")).toBeVisible();
  await expect(page.getByTestId("traffic-detail")).toContainText("Model Info");
  await expect(page.getByTestId("traffic-detail")).toContainText("gpt-4o-mini");
  await expect(page.getByTestId("traffic-detail")).toContainText("token-1token-2token-3");

  await server.close();
});

test("push 重连后已删除记录应从列表移除", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const keepPath = `/reconnect-keep-${token}`;
  const deletePath1 = `/reconnect-del1-${token}`;
  const deletePath2 = `/reconnect-del2-${token}`;

  let blockServerMessages = false;

  await page.routeWebSocket(/\/api\/push/, (ws) => {
    const srv = ws.connectToServer();
    ws.onMessage((message) => {
      srv.send(message);
    });
    srv.onMessage((message) => {
      if (!blockServerMessages) {
        ws.send(message);
      }
    });
  });

  try {
    await sendProxyRequest(`http://127.0.0.1:${server.port}${keepPath}`);
    await sendProxyRequest(`http://127.0.0.1:${server.port}${deletePath1}`);
    await sendProxyRequest(`http://127.0.0.1:${server.port}${deletePath2}`);

    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const keepRow = page.getByTestId("traffic-row").filter({ hasText: keepPath }).first();
    const delRow1 = page.getByTestId("traffic-row").filter({ hasText: deletePath1 }).first();
    const delRow2 = page.getByTestId("traffic-row").filter({ hasText: deletePath2 }).first();
    await expect(keepRow).toBeVisible();
    await expect(delRow1).toBeVisible();
    await expect(delRow2).toBeVisible();

    const delId1 = await delRow1.getAttribute("data-record-id");
    const delId2 = await delRow2.getAttribute("data-record-id");
    expect(delId1).toBeTruthy();
    expect(delId2).toBeTruthy();

    blockServerMessages = true;

    await page.evaluate(() => {
      const bt = (window as unknown as Record<string, Record<string, () => void>>).__bifrost_test;
      if (bt) bt.pauseRealtime();
    });

    await page.waitForTimeout(1000);

    const del1Resp = await request.fetch(`${apiBase}/traffic`, {
      method: "DELETE",
      headers: { "Content-Type": "application/json" },
      data: JSON.stringify({ ids: [delId1] }),
    });
    expect(del1Resp.ok()).toBeTruthy();

    const del2Resp = await request.fetch(`${apiBase}/traffic`, {
      method: "DELETE",
      headers: { "Content-Type": "application/json" },
      data: JSON.stringify({ ids: [delId2] }),
    });
    expect(del2Resp.ok()).toBeTruthy();

    const verifyDeleted = await request.get(
      `${apiBase}/traffic/${encodeURIComponent(delId1!)}`,
    );
    expect(verifyDeleted.status()).toBe(404);

    await page.waitForTimeout(500);

    const rowCountBeforeResume = await page.getByTestId("traffic-row").filter({ hasText: token }).count();

    blockServerMessages = false;

    await page.evaluate(() => {
      const bt = (window as unknown as Record<string, Record<string, () => void>>).__bifrost_test;
      if (bt) bt.resumeRealtime();
    });

    await page.waitForTimeout(8000);

    const rowCountAfterResume = await page.getByTestId("traffic-row").filter({ hasText: token }).count();

    await expect(keepRow).toBeVisible();

    const del1Count = await delRow1.count();
    const del2Count = await delRow2.count();

    expect(
      del1Count + del2Count,
      `BUG: Deleted rows still visible after push reconnect. ` +
        `Before resume: ${rowCountBeforeResume}, after: ${rowCountAfterResume}. ` +
        `Expected deleted rows to be removed.`,
    ).toBe(0);
  } finally {
    blockServerMessages = false;
    await page.evaluate(() => {
      const bt = (window as unknown as Record<string, Record<string, () => void>>).__bifrost_test;
      if (bt) bt.resumeRealtime();
    });
    await server.close();
  }
});
