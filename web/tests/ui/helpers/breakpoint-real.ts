import {
  expect,
  type Page,
  type APIRequestContext,
  type Locator,
} from "@playwright/test";
import {
  createServer,
  request as httpRequest,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";
import type { AddressInfo } from "node:net";
import {
  apiBase,
  backendPort,
  openPage,
  uniqueName,
  waitForToast,
} from "./admin-helpers";

export async function configureBreakpoint(page: Page, content: string) {
  await openPage(page, "rules");
  await page.getByTestId("rule-new-button").click();
  const ruleName = uniqueName("real-breakpoint");
  const nameInput = page.getByPlaceholder("Rule name");
  await nameInput.fill("");
  await nameInput.pressSequentially(ruleName, { delay: 5 });
  await nameInput.press("Tab");
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeHidden();
  await expect(page.getByTestId("rule-editor-title")).toHaveText(ruleName);
  await setBreakpointEditor(
    page,
    page.getByTestId("rule-editor-container"),
    content,
  );
  await expect(page.getByTestId("rule-save-button")).toBeEnabled();
  await page.getByTestId("rule-save-button").click();
  await waitForToast(page, "Saved");
  await openPage(page, "traffic");
  const gate = page.getByTestId("toolbar-breakpoint-toggle");
  await expect(gate).toHaveAttribute("aria-checked", "false");
  await gate.click();
  await expect(gate).toHaveAttribute("aria-checked", "true");
}

export async function pending(request: APIRequestContext) {
  const response = await request.get(`${apiBase}/breakpoint/pending`);
  expect(response.ok()).toBe(true);
  return (await response.json()) as Array<{
    request_id: string;
    phase: string;
    body_omitted: boolean;
  }>;
}

export interface WireResult {
  status: number;
  headers: IncomingMessage["headers"];
  rawHeaders: string[];
  bytes: Buffer;
}
export function wireRequest(
  url: string,
  options: {
    method?: string;
    body?: Buffer | string;
    headers?: Record<string, string | string[]>;
    leaveOpen?: boolean;
  } = {},
) {
  let completed = false;
  let responseStarted = false;
  let dataStarted = false;
  const req = httpRequest({
    host: "127.0.0.1",
    port: backendPort,
    path: url,
    method: options.method || "GET",
    headers: {
      Host: new URL(url).host,
      Connection: "close",
      ...(options.body !== undefined && !options.leaveOpen
        ? { "Content-Length": String(Buffer.byteLength(options.body)) }
        : {}),
      ...options.headers,
    },
  });
  const result = new Promise<WireResult>((resolve, reject) => {
    req.on("response", (res) => {
      responseStarted = true;
      const chunks: Buffer[] = [];
      res.on("data", (chunk) => {
        dataStarted = true;
        chunks.push(Buffer.from(chunk));
      });
      res.on("end", () => {
        completed = true;
        resolve({
          status: res.statusCode || 0,
          headers: res.headers,
          rawHeaders: res.rawHeaders,
          bytes: Buffer.concat(chunks),
        });
      });
      res.on("error", reject);
    });
    req.on("error", reject);
    req.setTimeout(45000, () =>
      req.destroy(new Error("loopback proxy request timed out")),
    );
  });
  // Attach rejection handler immediately: aborted clients are exercised deliberately.
  void result.catch(() => {});
  if (options.leaveOpen) req.write(options.body ?? "");
  else req.end(options.body);
  return {
    result,
    completed: () => completed,
    responseStarted: () => responseStarted,
    dataStarted: () => dataStarted,
    abort: () => req.destroy(),
    finish: (tail?: string | Buffer) => req.end(tail),
  };
}

export async function assertBlocked(
  page: Page,
  completed: () => boolean,
  responseStarted?: () => boolean,
  dataStarted?: () => boolean,
) {
  await page.waitForTimeout(300);
  expect(
    completed(),
    "client must remain blocked while breakpoint is pending",
  ).toBe(false);
  if (responseStarted)
    expect(
      responseStarted(),
      "response headers must not leak while paused",
    ).toBe(false);
  if (dataStarted)
    expect(dataStarted(), "response bytes must not leak while paused").toBe(
      false,
    );
}

export interface RecordedRequest {
  method: string;
  url: string;
  headers: IncomingMessage["headers"];
  rawHeaders: string[];
  bytes: Buffer;
}
export async function loopbackUpstream(
  responder?: (req: IncomingMessage, res: ServerResponse, body: Buffer) => void,
) {
  const records: RecordedRequest[] = [];
  let startedRequests = 0;
  const server = createServer(async (req, res) => {
    startedRequests++;
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    const bytes = Buffer.concat(chunks);
    records.push({
      method: req.method || "GET",
      url: req.url || "/",
      headers: req.headers,
      rawHeaders: req.rawHeaders,
      bytes,
    });
    if (responder) responder(req, res, bytes);
    else {
      const body = Buffer.from("original-response");
      res.writeHead(200, {
        "Content-Type": "text/plain",
        "Content-Length": body.length,
      });
      res.end(body);
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    records,
    startedRequests: () => startedRequests,
    url: (path: string) => `http://127.0.0.1:${port}${path}`,
    close: async () => {
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      );
    },
  };
}

export async function addHeader(
  page: Page,
  phase: "request" | "response",
  name: string,
  value: string,
) {
  await page.getByTestId(`${phase}-tab-header`).click();
  await page.getByTestId(`${phase}-header-view-add`).click();
  await page
    .locator(`[data-testid^="${phase}-header-view-name-"]`)
    .last()
    .fill(name);
  await page
    .locator(`[data-testid^="${phase}-header-view-value-"]`)
    .last()
    .fill(value);
}

export async function loopbackTlsUpstream(
  mode: "text" | "gzip" | "stream" | "finite-sse" | "bodyless" = "text",
  options: { status?: number } = {},
) {
  const { execFile } = await import("node:child_process");
  const { promisify } = await import("node:util");
  const fs = await import("node:fs/promises");
  const os = await import("node:os");
  const path = await import("node:path");
  const https = await import("node:https");
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "bifrost-breakpoint-tls-"),
  );
  const key = path.join(directory, "key.pem");
  const certificate = path.join(directory, "cert.pem");
  await promisify(execFile)("openssl", [
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
    certificate,
  ]);
  let hits = 0;
  const records: RecordedRequest[] = [];
  const server = https.createServer(
    { key: await fs.readFile(key), cert: await fs.readFile(certificate) },
    async (req, res) => {
      hits++;
      const chunks: Buffer[] = [];
      for await (const chunk of req) chunks.push(Buffer.from(chunk));
      records.push({
        method: req.method || "GET",
        url: req.url || "/",
        headers: req.headers,
        rawHeaders: req.rawHeaders,
        bytes: Buffer.concat(chunks),
      });
      if (mode === "bodyless") {
        res.writeHead(options.status ?? 200, {
          "Content-Type": "text/event-stream",
          Connection: "close",
        });
        res.flushHeaders();
        res.end();
      } else if (mode === "stream") {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write("data: tls-open-stream\n\n");
      } else {
        const { gzipSync } = await import("node:zlib");
        const body =
          mode === "gzip"
            ? gzipSync(Buffer.from("tls-original"))
            : Buffer.from("tls-original");
        res.writeHead(200, {
          "Content-Type":
            mode === "finite-sse" ? "text/event-stream" : "text/plain",
          "Content-Length": body.length,
          ...(mode === "gzip" ? { "Content-Encoding": "gzip" } : {}),
        });
        res.end(body);
      }
    },
  );
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    records,
    hits: () => hits,
    close: async () => {
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      );
      await fs.rm(directory, { recursive: true, force: true });
    },
  };
}

export async function tlsWireRequest(
  port: number,
  onResponseData?: () => void,
  options: { method?: "HEAD" } = {},
) {
  const { spawn } = await import("node:child_process");
  return await new Promise<{ stdout: string; responseHeaders: string }>(
    (resolve, reject) => {
      const child = spawn("curl", [
        "--silent",
        "--show-error",
        "--insecure",
        "--noproxy",
        "",
        "--proxy",
        `http://127.0.0.1:${backendPort}`,
        "--max-time",
        "45",
        "--no-buffer",
        "--include",
        "--suppress-connect-headers",
        "--compressed",
        ...(options.method === "HEAD"
          ? ["--head"]
          : ["--request", "POST", "--data", "tls-request-original"]),
        "--write-out",
        "\n%{http_code}",
        `https://127.0.0.1:${port}/tls-breakpoint`,
      ]);
      const chunks: Buffer[] = [];
      let stderr = "";
      child.stdout.on("data", (chunk) => {
        onResponseData?.();
        chunks.push(Buffer.from(chunk));
      });
      child.stderr.on("data", (chunk) => {
        stderr += chunk.toString();
      });
      child.on("error", reject);
      child.on("close", (code) => {
        if (code !== 0) {
          reject(new Error(`TLS client exited ${code}: ${stderr}`));
          return;
        }
        const output = Buffer.concat(chunks).toString();
        const boundary = output.indexOf("\r\n\r\n");
        resolve({
          stdout: boundary < 0 ? output : output.slice(boundary + 4),
          responseHeaders: boundary < 0 ? "" : output.slice(0, boundary),
        });
      });
    },
  );
}

export async function editHeader(
  page: Page,
  phase: "request" | "response",
  name: string,
  value: string,
) {
  await page.getByTestId(`${phase}-tab-header`).click();
  const names = page.locator(`[data-testid^="${phase}-header-view-name-"]`);
  for (let index = 0; index < (await names.count()); index++) {
    if (
      (await names.nth(index).inputValue()).toLowerCase() === name.toLowerCase()
    ) {
      await page.getByTestId(`${phase}-header-view-value-${index}`).fill(value);
      return;
    }
  }
  await addHeader(page, phase, name, value);
}

export async function setBreakpointEditor(
  page: Page,
  container: Locator,
  content: string,
) {
  const input = container
    .locator(".monaco-editor")
    .last()
    .getByRole("textbox", { name: "Editor content" });
  await expect(input).toBeVisible();
  await container
    .locator(".monaco-editor")
    .last()
    .locator(".view-lines")
    .click({ position: { x: 20, y: 8 } });
  await page.keyboard.press(
    process.platform === "darwin" ? "Meta+A" : "Control+A",
  );
  await page.keyboard.press("Backspace");
  // Exercise Monaco's real paste path, preserving Unicode and newlines.
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.evaluate((value) => navigator.clipboard.writeText(value), content);
  await page.keyboard.press(
    process.platform === "darwin" ? "Meta+V" : "Control+V",
  );
}

export async function restrictBrowserToLoopback(page: Page) {
  // Enforce isolation without substituting mocked API or proxy responses.
  await page.context().route(/^https?:\/\//, (route) => {
    const host = new URL(route.request().url()).hostname;
    return ["127.0.0.1", "localhost", "[::1]"].includes(host)
      ? route.continue()
      : route.abort("blockedbyclient");
  });
}
