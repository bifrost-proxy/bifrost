import { test, expect, type APIRequestContext, type Page } from "@playwright/test";
import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import fs from "node:fs/promises";
import { createServer as createTcpServer, Socket as NetSocket } from "node:net";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  apiBase,
  backendPort,
  clearTraffic,
  sendProxyRequest,
  startMockHttpServer,
  uniqueName,
} from "./helpers/admin-helpers";
import { TLS_RECONNECT_NOTICE } from "../../src/utils/tlsInterceptionNotice";

const BASE_PROXY_URL = process.env.PROXY_URL || `http://127.0.0.1:${backendPort}`;
const PUSH_RECORDER_KEY = "__bifrostPushRecorder";

type PushRecorderSnapshot = {
  urls: string[];
  messages: string[];
};

type RecordedTrafficDelta = {
  inserts: Array<Record<string, unknown>>;
  updates: Array<Record<string, unknown>>;
  has_more?: boolean;
  server_total?: number;
  server_sequence?: number;
};

const getRepoRoot = () => {
  const current = fileURLToPath(import.meta.url);
  return path.resolve(path.dirname(current), "../../..");
};

const isProcessAlive = (pid: number) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

const isBackendReady = async () => {
  try {
    const res = await fetch(`${apiBase}/proxy/address`);
    return res.ok;
  } catch {
    return false;
  }
};

const waitForBackend = async () => {
  for (let i = 0; i < 240; i += 1) {
    if (await isBackendReady()) {
      return true;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  return false;
};

const stopTrackedBackend = async () => {
  const pidFile =
    process.env.BIFROST_UI_TEST_PID_FILE || path.join(getRepoRoot(), ".ui-backend.pid");
  try {
    const pidText = await fs.readFile(pidFile, "utf-8");
    const pid = Number(pidText);
    if (Number.isNaN(pid) || !isProcessAlive(pid)) {
      await fs.rm(pidFile, { force: true });
      return;
    }
    try {
      process.kill(-pid);
    } catch {
      process.kill(pid);
    }
    await fs.rm(pidFile, { force: true });
  } catch {
    void 0;
  }
};

const startTrackedBackend = async () => {
  const repoRoot = getRepoRoot();
  const pidFile =
    process.env.BIFROST_UI_TEST_PID_FILE || path.join(repoRoot, ".ui-backend.pid");
  const dataDir = process.env.BIFROST_DATA_DIR || path.join(repoRoot, ".bifrost-ui-test");
  const targetDir =
    process.env.BIFROST_UI_TEST_TARGET_DIR || path.join(repoRoot, ".bifrost-ui-target");
  const binPath = path.join(targetDir, "debug", "bifrost");
  const logPath =
    process.env.BIFROST_UI_TEST_LOG_FILE || path.join(repoRoot, ".ui-backend.log");
  const logStream = createWriteStream(logPath, { flags: "a" });
  const { cmd, args } = await fs
    .access(binPath)
    .then(() => ({
      cmd: binPath,
      args: [
        "start",
        "--host",
        "127.0.0.1",
        "-p",
        String(backendPort),
        "--unsafe-ssl",
        "--no-system-proxy",
        "--access-mode",
        "allow_all",
      ],
    }))
    .catch(() => ({
      cmd: "cargo",
      args: [
        "run",
        "--bin",
        "bifrost",
        "--",
        "start",
        "--host",
        "127.0.0.1",
        "-p",
        String(backendPort),
        "--unsafe-ssl",
        "--no-system-proxy",
        "--access-mode",
        "allow_all",
      ],
    }));

  const child = spawn(cmd, args, {
    cwd: repoRoot,
    env: {
      ...process.env,
      PROXY_URL: BASE_PROXY_URL,
      BIFROST_UI_TEST_PORT: String(backendPort),
      BIFROST_DATA_DIR: dataDir,
      BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT: "1",
      CARGO_TARGET_DIR: targetDir,
    },
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
  });
  child.stdout?.pipe(logStream);
  child.stderr?.pipe(logStream);
  const pid = child.pid;
  if (!pid) {
    throw new Error("Failed to start tracked backend");
  }
  await fs.writeFile(pidFile, String(pid));
  const ok = await waitForBackend();
  if (!ok) {
    throw new Error("Tracked backend failed to start");
  }
};

async function fetchTrafficList(request: APIRequestContext) {
  const response = await request.get(`${apiBase}/traffic?limit=100`);
  return (await response.json()) as { records?: Array<Record<string, unknown>> };
}

async function fetchTrafficDetail(request: APIRequestContext, id: string) {
  const response = await request.get(`${apiBase}/traffic/${id}`);
  return (await response.json()) as Record<string, unknown> & {
    id: string;
    response_size: number;
    client_app?: string | null;
    client_ip?: string | null;
    socket_status?: { is_open?: boolean };
  };
}

async function waitForTrafficRecordId(
  request: APIRequestContext,
  predicate: (record: Record<string, unknown>) => boolean,
) {
  let foundId: string | null = null;
  await expect
    .poll(
      async () => {
        const payload = await fetchTrafficList(request);
        const record = payload.records?.find(predicate);
        foundId = typeof record?.id === "string" ? record.id : null;
        return foundId;
      },
      { timeout: 15000 },
    )
    .toMatch(/^REQ-/);
  return foundId as string;
}

async function startTunnelEchoServer() {
  const sockets = new Set<NetSocket>();
  const server = createTcpServer((socket) => {
    sockets.add(socket);
    socket.on("data", (chunk) => {
      socket.write(chunk);
    });
    socket.on("close", () => sockets.delete(socket));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    close: async () => {
      for (const socket of sockets) {
        socket.destroy();
      }
      await new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      );
    },
  };
}

async function waitForConnectEstablished(socket: NetSocket) {
  return new Promise<void>((resolve, reject) => {
    let buffer = "";
    const cleanup = () => {
      socket.off("data", onData);
      socket.off("error", onError);
      socket.off("close", onClose);
    };
    const onError = (error: Error) => {
      cleanup();
      reject(error);
    };
    const onClose = () => {
      cleanup();
      reject(new Error("CONNECT tunnel closed before establishment"));
    };
    const onData = (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      if (!buffer.includes("\r\n\r\n")) {
        return;
      }
      cleanup();
      if (!buffer.startsWith("HTTP/1.1 200") && !buffer.startsWith("HTTP/1.0 200")) {
        reject(new Error(`Unexpected CONNECT response: ${buffer}`));
        return;
      }
      resolve();
    };
    socket.on("data", onData);
    socket.on("error", onError);
    socket.on("close", onClose);
  });
}

async function openConnectTunnel(port: number) {
  const socket = new NetSocket();
  await new Promise<void>((resolve, reject) => {
    socket.connect(backendPort, "127.0.0.1", () => resolve());
    socket.once("error", reject);
  });
  socket.write(
    `CONNECT 127.0.0.1:${port} HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nProxy-Connection: Keep-Alive\r\n\r\n`,
  );
  await waitForConnectEstablished(socket);

  socket.write("hello-through-tunnel");
  await new Promise<void>((resolve, reject) => {
    const onData = (chunk: Buffer) => {
      if (chunk.toString("utf8").includes("hello-through-tunnel")) {
        cleanup();
        resolve();
      }
    };
    const onError = (error: Error) => {
      cleanup();
      reject(error);
    };
    const cleanup = () => {
      socket.off("data", onData);
      socket.off("error", onError);
    };
    socket.on("data", onData);
    socket.on("error", onError);
  });

  return {
    socket,
    close: async () => {
      if (socket.destroyed) {
        return;
      }
      await new Promise<void>((resolve) => {
        socket.once("close", () => resolve());
        socket.destroy();
      });
    },
  };
}

async function openTrafficPageAndWaitForPush(page: Page) {
  const pushSocketPromise = page.waitForEvent("websocket", (ws) =>
    ws.url().includes("/api/push"),
  );
  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();
  await pushSocketPromise;
}

async function installPushRecorder(page: Page) {
  await page.addInitScript((recorderKey) => {
    const nativeWebSocket = window.WebSocket;
    const recorder = {
      urls: [] as string[],
      messages: [] as string[],
    };

    class InstrumentedWebSocket extends nativeWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        const urlString = String(url);
        if (!urlString.includes("/api/push")) {
          return;
        }
        recorder.urls.push(urlString);
        this.addEventListener("message", (event) => {
          if (typeof event.data === "string") {
            recorder.messages.push(event.data);
          }
        });
      }
    }

    Object.setPrototypeOf(InstrumentedWebSocket, nativeWebSocket);
    Object.defineProperty(window, recorderKey, {
      configurable: true,
      value: recorder,
      writable: false,
    });
    window.WebSocket = InstrumentedWebSocket as typeof WebSocket;
  }, PUSH_RECORDER_KEY);
}

async function readPushRecorder(page: Page): Promise<PushRecorderSnapshot> {
  return page.evaluate((recorderKey) => {
    const recorder = (window as typeof window & {
      [key: string]: PushRecorderSnapshot | undefined;
    })[recorderKey];
    return {
      urls: [...(recorder?.urls || [])],
      messages: [...(recorder?.messages || [])],
    };
  }, PUSH_RECORDER_KEY);
}

async function setDocumentVisibility(page: Page, state: "hidden" | "visible") {
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
    window.dispatchEvent(new Event(nextState === "hidden" ? "pagehide" : "pageshow"));
  }, state);
}

function extractTrafficDelta(
  messages: string[],
  predicate: (delta: RecordedTrafficDelta) => boolean = () => true,
): RecordedTrafficDelta | null {
  for (const raw of messages) {
    try {
      const parsed = JSON.parse(raw) as {
        type?: string;
        data?: RecordedTrafficDelta;
      };
      if (parsed.type === "traffic_delta" && parsed.data && predicate(parsed.data)) {
        return parsed.data;
      }
    } catch {
      void 0;
    }
  }
  return null;
}

function countMatchingPaths(
  delta: RecordedTrafficDelta,
  paths: Set<string>,
): number {
  return delta.inserts.reduce((count, record) => {
    const path = typeof record.p === "string" ? record.p : null;
    return path && paths.has(path) ? count + 1 : count;
  }, 0);
}

test.describe.serial("traffic push regressions", () => {
  test("服务重启后保留数据并恢复实时推送", async ({ page, context, request }) => {
    test.setTimeout(180000);
    await clearTraffic(request);
    const server = await startMockHttpServer();
    const persistedPath = `/${uniqueName("restart-persisted")}`;
    const livePath = `/${uniqueName("restart-live")}`;

    try {
      await sendProxyRequest(`http://127.0.0.1:${server.port}${persistedPath}`);

      await openTrafficPageAndWaitForPush(page);
      await expect(
        page.getByTestId("traffic-row").filter({ hasText: persistedPath }).first(),
      ).toBeVisible();

      await stopTrackedBackend();
      await startTrackedBackend();

      const freshPage = await context.newPage();
      try {
        await openTrafficPageAndWaitForPush(freshPage);
        await expect(
          freshPage.getByTestId("traffic-row").filter({ hasText: persistedPath }).first(),
        ).toBeVisible();

        await sendProxyRequest(`http://127.0.0.1:${server.port}${livePath}`);
        await expect(
          page.getByTestId("traffic-row").filter({ hasText: livePath }).first(),
        ).toBeVisible();
      } finally {
        await freshPage.close();
      }
    } finally {
      await server.close();
    }
  });

  test("多个页面同时打开时都能完整收到实时流量", async ({ page, context, request }) => {
    await clearTraffic(request);
    const server = await startMockHttpServer();
    const requestPath = `/${uniqueName("multi-page")}`;

    try {
      const secondPage = await context.newPage();
      try {
        await Promise.all([
          openTrafficPageAndWaitForPush(page),
          openTrafficPageAndWaitForPush(secondPage),
        ]);

        await sendProxyRequest(`http://127.0.0.1:${server.port}${requestPath}`);

        await expect(
          page.getByTestId("traffic-row").filter({ hasText: requestPath }).first(),
        ).toBeVisible();
        await expect(
          secondPage.getByTestId("traffic-row").filter({ hasText: requestPath }).first(),
        ).toBeVisible();

        await secondPage.reload();
        await expect(
          secondPage.getByTestId("traffic-row").filter({ hasText: requestPath }).first(),
        ).toBeVisible();
      } finally {
        await secondPage.close();
      }
    } finally {
      await server.close();
    }
  });

  test("CONNECT 长连接状态更新能推送到页面并正确落库", async ({ page, request }) => {
    await clearTraffic(request);
    const server = await startTunnelEchoServer();
    let serverClosed = false;
    let tunnel: Awaited<ReturnType<typeof openConnectTunnel>> | null = null;

    try {
      await openTrafficPageAndWaitForPush(page);

      tunnel = await openConnectTunnel(server.port);

      const connectId = await waitForTrafficRecordId(
        request,
        (item) => item.m === "CONNECT" && item.h === "127.0.0.1",
      );
      const row = page.locator(`[data-testid="traffic-row"][data-record-id="${connectId}"]`);
      await expect(row).toBeVisible();

      await expect
        .poll(async () => {
          const detail = await fetchTrafficDetail(request, connectId);
          return detail.socket_status?.is_open === true;
        })
        .toBe(true);

      await tunnel.close();
      tunnel = null;
      await server.close();
      serverClosed = true;

      await expect
        .poll(async () => {
          const detail = await fetchTrafficDetail(request, connectId);
          return detail.socket_status?.is_open === false;
        })
        .toBe(true);

      await expect
        .poll(async () => {
          const responseSize = await row.getAttribute("data-response-size");
          return Number(responseSize || "0");
        })
        .toBeGreaterThan(0);
    } finally {
      if (tunnel) {
        await tunnel.close();
      }
      if (!serverClosed) {
        await server.close();
      }
    }
  });

  test("CONNECT 详情的 Response 面板可按应用和非本机 Client IP 开启", async ({
    page,
    request,
  }) => {
    await clearTraffic(request);
    const tlsConfigRes = await request.get(`${apiBase}/config/tls`);
    const originalTlsConfig = await tlsConfigRes.json();
    const whitelistRes = await request.get(`${apiBase}/whitelist`);
    const originalWhitelist = (await whitelistRes.json()) as { whitelist?: string[] };
    const syntheticClientIp = "192.168.50.24";
    const server = await startTunnelEchoServer();
    let serverClosed = false;
    let tunnel: Awaited<ReturnType<typeof openConnectTunnel>> | null = null;

    try {
      await openTrafficPageAndWaitForPush(page);

      tunnel = await openConnectTunnel(server.port);

      const connectId = await waitForTrafficRecordId(
        request,
        (item) => item.m === "CONNECT" && item.h === "127.0.0.1",
      );
      const row = page.locator(`[data-testid="traffic-row"][data-record-id="${connectId}"]`);
      await expect(row).toBeVisible();

      let clientApp: string | null = null;
      await expect
        .poll(async () => {
          const detail = await fetchTrafficDetail(request, connectId);
          clientApp =
            typeof detail.client_app === "string" && detail.client_app.length > 0
              ? detail.client_app
              : null;
          return clientApp;
        })
        .not.toBeNull();

      expect(clientApp).toBeTruthy();

      const actualDetail = await fetchTrafficDetail(request, connectId);
      await page.route(`**/_bifrost/api/traffic/${connectId}`, async (route) => {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            ...actualDetail,
            client_ip: syntheticClientIp,
          }),
        });
      });
      await row.click();

      await expect(page.getByTestId("response-tab-header")).toBeVisible();
      await expect(page.getByRole("button", { name: "Intercept this app" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Intercept this client" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Allow this client" })).toBeVisible();
      await expect(page.locator("body")).toContainText(TLS_RECONNECT_NOTICE);
      await page.getByRole("button", { name: "Intercept this app" }).click();
      const confirmDialog = page.getByRole("dialog", {
        name: "Add App to Intercept List",
      });
      await expect(confirmDialog).toBeVisible();
      await confirmDialog.getByRole("button", { name: "Add" }).click();

      await expect(
        page.locator(".ant-message-notice").filter({
          hasText: TLS_RECONNECT_NOTICE,
        }).last(),
      ).toBeVisible();

      await expect
        .poll(async () => {
          const response = await request.get(`${apiBase}/config/tls`);
          const body = (await response.json()) as {
            app_intercept_include?: string[];
          };
          return body.app_intercept_include?.includes(clientApp as string) ?? false;
        })
        .toBe(true);

      await page.getByRole("button", { name: "Intercept this client" }).click();
      const ipDialog = page.getByRole("dialog", {
        name: "Add Client IP to Intercept List",
      });
      await expect(ipDialog).toBeVisible();
      await ipDialog.getByRole("button", { name: "Add" }).click();

      await expect
        .poll(async () => {
          const response = await request.get(`${apiBase}/config/tls`);
          const body = (await response.json()) as {
            ip_intercept_include?: string[];
          };
          return body.ip_intercept_include?.includes(syntheticClientIp) ?? false;
        })
        .toBe(true);

      await page.getByRole("button", { name: "Allow this client" }).click();
      const whitelistDialog = page.getByRole("dialog", {
        name: "Add Client to Whitelist",
      });
      await expect(whitelistDialog).toBeVisible();
      await whitelistDialog.getByRole("button", { name: "Add" }).click();

      await expect
        .poll(async () => {
          const response = await request.get(`${apiBase}/whitelist`);
          const body = (await response.json()) as {
            whitelist?: string[];
          };
          const whitelist = body.whitelist ?? [];
          return (
            whitelist.includes(syntheticClientIp) ||
            whitelist.includes(`${syntheticClientIp}/32`)
          );
        })
        .toBe(true);

      await tunnel.close();
      tunnel = null;
      await server.close();
      serverClosed = true;
    } finally {
      if (tunnel) {
        await tunnel.close();
      }
      if (!serverClosed) {
        await server.close();
      }
      await request.put(`${apiBase}/config/tls`, { data: originalTlsConfig });
      const originalWhitelistEntries = originalWhitelist.whitelist ?? [];
      const hadSyntheticClientIp =
        originalWhitelistEntries.includes(syntheticClientIp) ||
        originalWhitelistEntries.includes(`${syntheticClientIp}/32`);
      if (!hadSyntheticClientIp) {
        await request.delete(`${apiBase}/whitelist`, {
          data: { ip_or_cidr: syntheticClientIp },
        });
      }
    }
  });

  test("窗口隐藏后恢复时会通过批量 delta 补齐 backlog", async ({ page, request }) => {
    await clearTraffic(request);
    await installPushRecorder(page);
    const server = await startMockHttpServer();
    const hiddenPaths = Array.from({ length: 5 }, () => `/${uniqueName("hidden-batch")}`);
    const hiddenPathSet = new Set(hiddenPaths);

    try {
      await openTrafficPageAndWaitForPush(page);
      const initialRecorder = await readPushRecorder(page);

      await setDocumentVisibility(page, "hidden");
      await page.waitForTimeout(800);

      await Promise.all(
        hiddenPaths.map((requestPath) =>
          sendProxyRequest(`http://127.0.0.1:${server.port}${requestPath}`),
        ),
      );

      await setDocumentVisibility(page, "visible");

      let resumedDelta: RecordedTrafficDelta | null = null;
      await expect
        .poll(
          async () => {
            const currentRecorder = await readPushRecorder(page);
            resumedDelta = extractTrafficDelta(
              currentRecorder.messages.slice(initialRecorder.messages.length),
              (delta) => countMatchingPaths(delta, hiddenPathSet) >= 2,
            );
            return resumedDelta ? countMatchingPaths(resumedDelta, hiddenPathSet) : 0;
          },
          { timeout: 15000 },
        )
        .toBeGreaterThanOrEqual(2);

      for (const requestPath of hiddenPaths) {
        await expect(
          page.getByTestId("traffic-row").filter({ hasText: requestPath }).first(),
        ).toBeVisible();
      }
    } finally {
      await server.close();
    }
  });
});
