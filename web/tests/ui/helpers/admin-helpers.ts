import { expect, type APIRequestContext, type Locator, type Page } from "@playwright/test";
import { createServer, type IncomingMessage, type ServerResponse, request as httpRequest } from "node:http";
import type { AddressInfo } from "node:net";
import { promisify } from "node:util";
import { execFile } from "node:child_process";
import WebSocket, { WebSocketServer } from "ws";
import { HttpProxyAgent } from "http-proxy-agent";

const execFileAsync = promisify(execFile);

export const backendPort = Number(
  process.env.BIFROST_UI_TEST_PORT ?? process.env.BACKEND_PORT ?? 9910,
);
export const proxyUrl = process.env.PROXY_URL || `http://127.0.0.1:${backendPort}`;
export const apiBase =
  process.env.ADMIN_API_BASE || `${proxyUrl.replace(/\/$/, "")}/_bifrost/api`;

export function uniqueName(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
}

async function readJson(response: Awaited<ReturnType<APIRequestContext["get"]>>) {
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

export async function clearTraffic(request: APIRequestContext): Promise<void> {
  await request.delete(`${apiBase}/traffic`);
}

export async function clearRules(request: APIRequestContext): Promise<void> {
  const response = await request.get(`${apiBase}/rules`);
  const rules = (await readJson(response)) as Array<{ name: string }>;
  for (const rule of rules || []) {
    await request.delete(`${apiBase}/rules/${encodeURIComponent(rule.name)}`);
  }
}

export async function clearValues(request: APIRequestContext): Promise<void> {
  const response = await request.get(`${apiBase}/values`);
  const payload = (await readJson(response)) as { values?: Array<{ name: string }> };
  for (const value of payload?.values || []) {
    await request.delete(`${apiBase}/values/${encodeURIComponent(value.name)}`);
  }
}

export async function clearScripts(request: APIRequestContext): Promise<void> {
  const response = await request.get(`${apiBase}/scripts`);
  const payload = (await readJson(response)) as Record<string, Array<{ name: string }> | undefined>;
  for (const type of ["request", "response", "decode", "parser"] as const) {
    for (const script of payload?.[type] || []) {
      await request.delete(`${apiBase}/scripts/${type}/${encodeURIComponent(script.name)}`);
    }
  }
}

export async function clearReplay(request: APIRequestContext): Promise<void> {
  const requestsRes = await request.get(`${apiBase}/replay/requests?saved=true&limit=500`);
  const requestsPayload = (await readJson(requestsRes)) as { requests?: Array<{ id: string }> };
  for (const item of requestsPayload?.requests || []) {
    await request.delete(`${apiBase}/replay/requests/${encodeURIComponent(item.id)}`);
  }

  const groupsRes = await request.get(`${apiBase}/replay/groups`);
  const groupsPayload = (await readJson(groupsRes)) as { groups?: Array<{ id: string }> } | Array<{ id: string }>;
  const groups = Array.isArray(groupsPayload) ? groupsPayload : groupsPayload?.groups || [];
  for (const group of groups) {
    await request.delete(`${apiBase}/replay/groups/${encodeURIComponent(group.id)}`);
  }

  await request.delete(`${apiBase}/replay/history`);
}

export async function resetAccessControl(request: APIRequestContext): Promise<void> {
  await request.put(`${apiBase}/whitelist/mode`, { data: { mode: "allow_all" } });
  await request.put(`${apiBase}/whitelist/allow-lan`, { data: { allow_lan: false } });
  const statusRes = await request.get(`${apiBase}/whitelist`);
  const status = (await readJson(statusRes)) as {
    whitelist?: string[];
    temporary_whitelist?: string[];
  };
  for (const ip of status?.whitelist || []) {
    await request.delete(`${apiBase}/whitelist`, { data: { ip_or_cidr: ip } });
  }
  for (const ip of status?.temporary_whitelist || []) {
    await request.delete(`${apiBase}/whitelist/temporary`, { data: { ip } });
  }
  await request.delete(`${apiBase}/whitelist/pending`);
}

export interface SeedNotificationRecord {
  notificationType: string;
  title: string;
  message: string;
  metadata?: string | null;
  status: string;
  actionTaken?: string | null;
  createdAt?: number;
  updatedAt?: number;
}

export async function seedNotifications(records: SeedNotificationRecord[]): Promise<void> {
  const dataDir = process.env.BIFROST_DATA_DIR;
  if (!dataDir) {
    throw new Error("BIFROST_DATA_DIR is required to seed notifications");
  }

  const payload = JSON.stringify(records);
  const script = `
import json
import pathlib
import sqlite3
import sys
import time

data_dir = pathlib.Path(sys.argv[1])
db_path = data_dir / "notifications.db"
db_path.parent.mkdir(parents=True, exist_ok=True)
conn = sqlite3.connect(db_path)
conn.executescript("""
CREATE TABLE IF NOT EXISTS notifications (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  notification_type TEXT NOT NULL,
  title TEXT NOT NULL,
  message TEXT NOT NULL,
  metadata TEXT,
  status TEXT NOT NULL,
  action_taken TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notifications_type ON notifications(notification_type);
CREATE INDEX IF NOT EXISTS idx_notifications_status ON notifications(status);
CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at);
DELETE FROM notifications;
""")
now = int(time.time())
records = json.loads(sys.argv[2])
for record in records:
  conn.execute(
    """
    INSERT INTO notifications(
      notification_type, title, message, metadata, status, action_taken, created_at, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    """,
    (
      record["notificationType"],
      record["title"],
      record["message"],
      record.get("metadata"),
      record["status"],
      record.get("actionTaken"),
      record.get("createdAt", now),
      record.get("updatedAt", now),
    ),
  )
conn.commit()
conn.close()
`;

  await execFileAsync("python3", ["-c", script, dataDir, payload], { timeout: 15000 });
}

export async function sendProxyRequest(
  url: string,
  options: {
    method?: string;
    headers?: Record<string, string>;
    body?: string;
  } = {},
): Promise<void> {
  const args = [
    "-sS",
    "--fail",
    "--noproxy",
    "",
    "-x",
    proxyUrl,
    "-X",
    options.method || "GET",
  ];
  for (const [key, value] of Object.entries(options.headers || {})) {
    args.push("-H", `${key}: ${value}`);
  }
  if (options.body !== undefined) {
    args.push("--data", options.body);
  }
  args.push(url);
  await execFileAsync("curl", args, { timeout: 15000 });
}

export async function sendSseViaProxy(url: string): Promise<void> {
  const target = new URL(url);
  await new Promise<void>((resolve, reject) => {
    const req = httpRequest(
      {
        host: "127.0.0.1",
        port: backendPort,
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
}

export async function sendWsViaProxy(
  url: string,
  messages: Array<string | Buffer> = ["hello", Buffer.from([1, 2, 3, 4])],
): Promise<void> {
  const agent = new HttpProxyAgent(proxyUrl);
  await new Promise<void>((resolve, reject) => {
    const ws = new WebSocket(url, { agent });
    ws.on("open", () => {
      for (const message of messages) {
        ws.send(message);
      }
      ws.close();
    });
    ws.on("close", () => resolve());
    ws.on("error", reject);
  });
}

export async function waitForTrafficRow(page: Page, text: string): Promise<Locator> {
  const row = page.getByTestId("traffic-row").filter({ hasText: text }).first();
  await expect(row).toBeVisible();
  return row;
}

export async function openPage(page: Page, path: string): Promise<void> {
  await page.goto(`/_bifrost/${path.replace(/^\//, "")}`);
}

export async function setMonacoEditor(page: Page, container: Locator, value: string): Promise<void> {
  const input = container.locator("textarea").last();
  await input.click({ force: true });
  await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
  await page.keyboard.press("Backspace");
  if (value.length > 0) {
    await input.type(value, { delay: 0 });
  }
}

export async function waitForToast(page: Page, text: string): Promise<void> {
  await expect(page.locator(".ant-message-notice").filter({ hasText: text }).last()).toBeVisible();
}

export async function setSelectValue(page: Page, trigger: Locator, optionText: string): Promise<void> {
  await trigger.click();
  await page
    .locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)")
    .last()
    .getByText(optionText, { exact: true })
    .click();
}

export async function setSliderValue(page: Page, testId: string, targetDelta: number): Promise<void> {
  const slider = page.getByTestId(testId);
  await expect(slider).toBeVisible();
  const handle = slider.locator(".ant-slider-handle").first();
  await handle.focus();
  const key = targetDelta >= 0 ? "ArrowRight" : "ArrowLeft";
  for (let i = 0; i < Math.abs(targetDelta); i += 1) {
    await page.keyboard.press(key);
  }
}

export interface MockHttpRequestRecord {
  method: string;
  url: string;
  headers: Record<string, string | string[] | undefined>;
  body: string;
}

export interface MockHttpServer {
  port: number;
  requests: MockHttpRequestRecord[];
  close: () => Promise<void>;
}

export interface MockSyncUser {
  user_id: string;
  nickname: string;
  avatar?: string;
  email?: string;
}

export interface MockSyncServerOptions {
  responseDelayMs?: number;
}

export interface MockSyncEnv {
  id: string;
  user_id: string;
  name: string;
  rule: string;
  create_time: string;
  update_time: string;
}

export interface MockSyncServer {
  port: number;
  baseUrl: string;
  user: MockSyncUser;
  token: string;
  listEnvs: () => MockSyncEnv[];
  upsertEnv: (env: MockSyncEnv) => void;
  close: () => Promise<void>;
}

async function readBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString("utf8");
}

export async function startMockHttpServer(
  responder?: (req: IncomingMessage, res: ServerResponse, body: string) => void,
): Promise<MockHttpServer> {
  const requests: MockHttpRequestRecord[] = [];
  const server = createServer(async (req, res) => {
    const body = await readBody(req);
    requests.push({
      method: req.method || "GET",
      url: req.url || "/",
      headers: req.headers,
      body,
    });
    if (responder) {
      responder(req, res, body);
      return;
    }
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ path: req.url || "/", echoedHeaders: req.headers, body }));
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    requests,
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      ),
  };
}

export async function startMockSyncServer(
  seedEnvs: MockSyncEnv[] = [],
  seedUser: MockSyncUser = {
    user_id: "ui-sync-user",
    nickname: "UI Sync User",
    email: "ui-sync@example.com",
  },
  options: MockSyncServerOptions = {},
): Promise<MockSyncServer> {
  const envs = new Map<string, MockSyncEnv>();
  for (const env of seedEnvs) {
    envs.set(env.id, { ...env });
  }

  const token = "mock-sync-token";
  const responseDelayMs = options.responseDelayMs ?? 0;

  const sendJson = (res: ServerResponse, statusCode: number, body: unknown) => {
    res.writeHead(statusCode, { "Content-Type": "application/json" });
    res.end(JSON.stringify(body));
  };

  const unauthorized = (res: ServerResponse) => {
    sendJson(res, 401, { code: -10001, message: "unauthorized" });
  };

  const requireAuth = (req: IncomingMessage, res: ServerResponse) => {
    const provided = req.headers["x-bifrost-token"];
    if (provided !== token) {
      unauthorized(res);
      return false;
    }
    return true;
  };

  const server = createServer(async (req, res) => {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    const body = await readBody(req);
    if (responseDelayMs > 0 && url.pathname.startsWith("/v4/env")) {
      await new Promise((resolve) => setTimeout(resolve, responseDelayMs));
    }

    if (url.pathname === "/v4/sso/check") {
      if (req.headers["x-bifrost-token"] === token) {
        sendJson(res, 200, {
          code: 0,
          message: "ok",
          data: { user_id: seedUser.user_id, token },
        });
      } else {
        unauthorized(res);
      }
      return;
    }

    if (url.pathname === "/v4/sso/info") {
      if (!requireAuth(req, res)) return;
      sendJson(res, 200, {
        code: 0,
        message: "ok",
        data: seedUser,
      });
      return;
    }

    if (url.pathname === "/v4/sso/logout") {
      sendJson(res, 200, { code: 0, message: "ok", data: 1 });
      return;
    }

    if (url.pathname === "/v4/sso/login") {
      const next = url.searchParams.get("next");
      res.writeHead(302, {
        Location: `${next}${next?.includes("?") ? "&" : "?"}token=${encodeURIComponent(token)}`,
      });
      res.end();
      return;
    }

    if (url.pathname === "/v4/env" && req.method === "GET") {
      if (!requireAuth(req, res)) return;
      const userIds = url.searchParams.getAll("user_id");
      const list = [...envs.values()].filter((env) =>
        userIds.length === 0 ? true : userIds.includes(env.user_id),
      );
      sendJson(res, 200, {
        code: 0,
        message: "ok",
        data: { list },
      });
      return;
    }

    if (url.pathname === "/v4/env" && req.method === "POST") {
      if (!requireAuth(req, res)) return;
      const payload = JSON.parse(body || "{}") as {
        user_id: string;
        name: string;
        rule: string;
      };
      const now = new Date().toISOString();
      const env: MockSyncEnv = {
        id: uniqueName("remote-env"),
        user_id: payload.user_id,
        name: payload.name,
        rule: payload.rule,
        create_time: now,
        update_time: now,
      };
      envs.set(env.id, env);
      sendJson(res, 200, { code: 0, message: "ok", data: env });
      return;
    }

    if (url.pathname.startsWith("/v4/env/") && req.method === "PATCH") {
      if (!requireAuth(req, res)) return;
      const envId = url.pathname.split("/").pop() || "";
      const current = envs.get(envId);
      if (!current) {
        sendJson(res, 404, { code: -1, message: "not found" });
        return;
      }
      const payload = JSON.parse(body || "{}") as {
        name?: string;
        rule?: string;
      };
      const updated: MockSyncEnv = {
        ...current,
        name: payload.name ?? current.name,
        rule: payload.rule ?? current.rule,
        update_time: new Date().toISOString(),
      };
      envs.set(envId, updated);
      sendJson(res, 200, { code: 0, message: "ok", data: updated });
      return;
    }

    if (url.pathname.startsWith("/v4/env/") && req.method === "DELETE") {
      if (!requireAuth(req, res)) return;
      const envId = url.pathname.split("/").pop() || "";
      envs.delete(envId);
      sendJson(res, 200, { code: 0, message: "ok", data: 1 });
      return;
    }

    sendJson(res, 404, { code: -1, message: "not found" });
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    baseUrl: `http://127.0.0.1:${port}`,
    user: seedUser,
    token,
    listEnvs: () => [...envs.values()].map((env) => ({ ...env })),
    upsertEnv: (env) => {
      envs.set(env.id, { ...env });
    },
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      ),
  };
}

export async function startSseServer(): Promise<{ port: number; close: () => Promise<void> }> {
  const server = createServer((req, res) => {
    if (!(req.url || "").startsWith("/sse")) {
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
    res.write("id: 2\ndata: beta\n\n");
    res.write("id: 3\ndata: gamma\n\n");
    res.end();
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      ),
  };
}

export async function startWsServer(): Promise<{ port: number; close: () => Promise<void> }> {
  const wss = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  wss.on("connection", (socket) => {
    socket.on("message", (data) => {
      socket.send(data);
    });
  });
  await new Promise<void>((resolve) => wss.on("listening", resolve));
  const port = (wss.address() as AddressInfo).port;
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) =>
        wss.close((err) => (err ? reject(err) : resolve())),
      ),
  };
}
