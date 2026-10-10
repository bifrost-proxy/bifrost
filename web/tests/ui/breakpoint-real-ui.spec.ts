import { test, expect } from "@playwright/test";
import { gzipSync, gunzipSync } from "node:zlib";
import {
  apiBase,
  clearRules,
  clearTraffic,
  waitForTrafficRow,
  waitForToast,
  setSelectValue,
  openPage,
} from "./helpers/admin-helpers";
import {
  configureBreakpoint,
  pending,
  wireRequest,
  loopbackUpstream,
  assertBlocked,
  addHeader,
  editHeader,
  setBreakpointEditor as setMonacoEditor,
  restrictBrowserToLoopback,
} from "./helpers/breakpoint-real";

// One worker isolates the proxy; each test resets rules, gate and traffic independently.
test.describe.configure({ mode: "default" });
test.use({ actionTimeout: 20000 });
test.beforeEach(async ({ page, request }) => {
  await restrictBrowserToLoopback(page);
  await request.post(`${apiBase}/breakpoint/settings`, {
    data: { enabled: false, max_body_bytes: 1048576 },
  });
  await request.put(`${apiBase}/config/performance`, {
    data: {
      breakpoint_timeout_ms: 30000,
      binary_traffic_performance_mode: true,
    },
  });
  await clearRules(request);
  await clearTraffic(request);
});
test.afterEach(async ({ request }) => {
  await request.post(`${apiBase}/breakpoint/settings`, {
    data: { enabled: false, max_body_bytes: 1048576 },
  });
});

for (const theme of ["light", "dark"] as const) {
  test(`Breakpoint real UI Paused filter gate lifecycle in ${theme} theme`, async ({
    page,
  }, testInfo) => {
    const upstream = await loopbackUpstream();
    try {
      await openPage(page, "traffic");
      const paused = page.getByRole("checkbox", {
        name: "Paused",
        exact: true,
      });
      await expect(paused).toHaveCount(0);
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port}/pause-only breakpoint://request`,
      );
      if ((await page.locator("html").getAttribute("data-theme")) !== theme) {
        await page.getByTestId("theme-toggle").click();
      }
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      await expect(
        page
          .getByTestId("toolbar-quick-filters")
          .getByRole("checkbox", { name: "Paused", exact: true }),
      ).toBeVisible();
      await expect(page.getByTestId("toolbar-quick-filters")).toContainText(
        "Imported",
      );
      await expect(paused).not.toBeChecked();
      await wireRequest(upstream.url("/ordinary-restores")).result;
      const ordinary = await waitForTrafficRow(page, "/ordinary-restores");
      await expect(ordinary).toBeVisible();
      await paused.focus();
      await page.keyboard.press("Space");
      await expect(paused).toBeChecked();
      await expect(ordinary).toHaveCount(0);
      await page.screenshot({
        path: testInfo.outputPath(`paused-${theme}.png`),
        fullPage: true,
      });
      const gate = page.getByTestId("toolbar-breakpoint-toggle");
      await gate.click();
      await expect(gate).toHaveAttribute("aria-checked", "false");
      await expect(paused).toHaveCount(0);
      await expect(ordinary).toBeVisible();
      await gate.click();
      await expect(paused).toBeVisible();
      await expect(paused).not.toBeChecked();
      await expect(ordinary).toBeVisible();
    } finally {
      await upstream.close();
    }
  });
}

for (const phase of ["request", "response", "both", "all"] as const) {
  test(`Breakpoint real UI Rules → gate → ${phase} pauses and applies actual wire edits`, async ({
    page,
    request,
  }, testInfo) => {
    const upstream = await loopbackUpstream();
    try {
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port}/phase* breakpoint://${phase}`,
      );
      const client = wireRequest(upstream.url("/phase?old=1"), {
        method: "POST",
        body: "original-request",
        headers: { "Content-Type": "text/plain" },
      });
      const firstPhase = phase === "response" ? "response" : "request";
      // No reload or row selection: this proves the first live pause automatically opens details.
      await expect(
        page.getByTestId("breakpoint-editor-banner"),
      ).toHaveAttribute("data-phase", firstPhase);
      await expect(
        page.getByTestId(
          firstPhase === "request"
            ? "breakpoint-status-input"
            : "breakpoint-method-input",
        ),
      ).toHaveCount(0);
      await assertBlocked(
        page,
        client.completed,
        client.responseStarted,
        client.dataStarted,
      );
      const firstRow = await waitForTrafficRow(page, "/phase");
      const requestId = await firstRow.getAttribute("data-record-id");
      expect(requestId).toBeTruthy();
      const lightBackground = await firstRow.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      );
      expect(lightBackground).not.toBe("rgba(0, 0, 0, 0)");
      await page.screenshot({
        path: testInfo.outputPath(`${phase}-first-light.png`),
        fullPage: true,
      });
      await page.getByTestId("theme-toggle").click();
      await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
      const darkBackground = await firstRow.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      );
      expect(darkBackground).not.toBe(lightBackground);
      expect(darkBackground).not.toBe("rgba(0, 0, 0, 0)");
      await page.screenshot({
        path: testInfo.outputPath(`${phase}-first-dark.png`),
        fullPage: true,
      });
      await page.getByTestId("theme-toggle").click();
      if (firstPhase === "request") {
        expect(upstream.records).toHaveLength(0);
        expect(upstream.startedRequests()).toBe(0);
        await page.getByTestId("breakpoint-method-input").fill("PUT");
        await page
          .getByTestId("breakpoint-url-input")
          .fill(upstream.url("/phase-edited?dup=one&dup=two"));
        await setMonacoEditor(
          page,
          page.getByTestId("traffic-detail"),
          "edited-request-body",
        );
        await addHeader(page, "request", "X-Duplicate", "one");
        await addHeader(page, "request", "X-Duplicate", "two");
        await editHeader(page, "request", "Host", "edited.virtual.test:1234");
        await editHeader(page, "request", "User-Agent", "breakpoint-ui-agent");
        await editHeader(page, "request", "Cookie", "edited=ui");
        await editHeader(
          page,
          "request",
          "Referer",
          "https://edited-referrer.test/ui",
        );
        await page.getByTestId("breakpoint-apply-resume").click();
        await waitForToast(page, "Breakpoint edits applied");
        await expect.poll(() => upstream.records.length).toBe(1);
        const received = upstream.records[0];
        expect(received.method).toBe("PUT");
        expect(received.url).toBe("/phase-edited?dup=one&dup=two");
        expect(received.bytes.toString()).toBe("edited-request-body");
        expect(received.headers.host).toBe("edited.virtual.test:1234");
        expect(received.headers["user-agent"]).toBe("breakpoint-ui-agent");
        expect(received.headers.cookie).toBe("edited=ui");
        expect(received.headers.referer).toBe(
          "https://edited-referrer.test/ui",
        );
        expect(
          received.rawHeaders.filter(
            (value) => value.toLowerCase() === "x-duplicate",
          ),
        ).toHaveLength(2);
        expect(Number(received.headers["content-length"])).toBe(
          received.bytes.length,
        );
      } else {
        expect(upstream.records).toHaveLength(1);
        expect(upstream.records[0].method).toBe("POST");
        expect(upstream.records[0].url).toBe("/phase?old=1");
        expect(upstream.records[0].bytes.toString()).toBe("original-request");
      }
      if (phase !== "request") {
        // The second phase must preserve user selection. Same row, explicit selection if necessary.
        const row = page.locator(
          `[data-testid="traffic-row"][data-record-id="${requestId}"]`,
        );
        await expect(row).toHaveAttribute("data-breakpoint-phase", "response");
        if (firstPhase === "request") await row.click();
        await expect(
          page.getByTestId("breakpoint-editor-banner"),
        ).toHaveAttribute("data-phase", "response");
        await assertBlocked(
          page,
          client.completed,
          client.responseStarted,
          client.dataStarted,
        );
        await page.getByTestId("breakpoint-status-input").fill("418");
        await setMonacoEditor(
          page,
          page.getByTestId("traffic-detail"),
          "edited-response-body",
        );
        await addHeader(page, "response", "Set-Cookie", "a=one");
        await addHeader(page, "response", "Set-Cookie", "b=two");
        await page.screenshot({
          path: testInfo.outputPath(`${phase}-light.png`),
          fullPage: true,
        });
        await page.getByTestId("theme-toggle").click();
        await expect(page.locator("html")).toHaveAttribute(
          "data-theme",
          "dark",
        );
        await page.screenshot({
          path: testInfo.outputPath(`${phase}-dark.png`),
          fullPage: true,
        });
        await page.getByTestId("breakpoint-apply-resume").click();
      }
      const result = await client.result;
      expect(result.status).toBe(phase === "request" ? 200 : 418);
      expect(result.bytes.toString()).toBe(
        phase === "request" ? "original-response" : "edited-response-body",
      );
      if (phase !== "request") {
        expect(result.headers["set-cookie"]).toEqual(["a=one", "b=two"]);
        expect(Number(result.headers["content-length"])).toBe(
          result.bytes.length,
        );
      }
      await expect.poll(async () => (await pending(request)).length).toBe(0);
    } finally {
      await upstream.close();
    }
  });
}

test("Breakpoint real UI gate alone and disabled gate never pause matching traffic", async ({
  page,
  request,
}) => {
  const upstream = await loopbackUpstream();
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/matching breakpoint://request`,
    );
    expect((await wireRequest(upstream.url("/unmatched")).result).status).toBe(
      200,
    );
    expect(await pending(request)).toHaveLength(0);
    await page.getByTestId("toolbar-breakpoint-toggle").click();
    expect((await wireRequest(upstream.url("/matching")).result).status).toBe(
      200,
    );
    expect(await pending(request)).toHaveLength(0);
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveCount(0);
  } finally {
    await upstream.close();
  }
});

test("Breakpoint real UI concurrent hits preserve focus and separate drafts after first auto-selection", async ({
  page,
  request,
}) => {
  const upstream = await loopbackUpstream();
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/draft* breakpoint://request`,
    );
    const first = wireRequest(upstream.url("/draft-first"), {
      method: "POST",
      body: "first-original",
    });
    await expect(page.getByTestId("breakpoint-url-input")).toHaveValue(
      upstream.url("/draft-first"),
    );
    await setMonacoEditor(
      page,
      page.getByTestId("traffic-detail"),
      "first-draft",
    );
    const second = wireRequest(upstream.url("/draft-second"), {
      method: "POST",
      body: "second-original",
    });
    const third = wireRequest(upstream.url("/draft-third"), {
      method: "POST",
      body: "third-original",
    });
    await expect.poll(async () => (await pending(request)).length).toBe(3);
    await expect(page.getByTestId("breakpoint-url-input")).toHaveValue(
      upstream.url("/draft-first"),
    );
    await assertBlocked(
      page,
      first.completed,
      first.responseStarted,
      first.dataStarted,
    );
    expect(upstream.records).toHaveLength(0);
    expect(upstream.startedRequests()).toBe(0);
    await (await waitForTrafficRow(page, "/draft-second")).click();
    await setMonacoEditor(
      page,
      page.getByTestId("traffic-detail"),
      "second-draft",
    );
    await (await waitForTrafficRow(page, "/draft-first")).click();
    await page.getByTestId("breakpoint-apply-resume").click();
    await first.result;
    expect(
      upstream.records
        .find((record) => record.url === "/draft-first")
        ?.bytes.toString(),
    ).toBe("first-draft");
    await expect(page.getByTestId("traffic-detail-header")).toContainText(
      "/draft-first",
    );
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveCount(0);
    await (await waitForTrafficRow(page, "/draft-second")).click();
    await page.getByTestId("breakpoint-apply-resume").click();
    await second.result;
    expect(
      upstream.records
        .find((record) => record.url === "/draft-second")
        ?.bytes.toString(),
    ).toBe("second-draft");
    await (await waitForTrafficRow(page, "/draft-third")).click();
    await page.getByTestId("breakpoint-resume-unchanged").click();
    await third.result;
    expect(
      upstream.records
        .find((record) => record.url === "/draft-third")
        ?.bytes.toString(),
    ).toBe("third-original");
  } finally {
    await upstream.close();
  }
});

for (const resolution of ["resume", "timeout", "disable"] as const) {
  test(`Breakpoint real UI pending-only normal and Fuzzy filters remove ${resolution} hits`, async ({
    page,
    request,
  }) => {
    const upstream = await loopbackUpstream();
    try {
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port}/pending breakpoint://response`,
      );
      if (resolution === "timeout")
        await request.put(`${apiBase}/config/performance`, {
          data: { breakpoint_timeout_ms: 5000 },
        });
      await wireRequest(upstream.url("/completed")).result;
      const client = wireRequest(upstream.url("/pending"));
      await expect(
        page.getByTestId("breakpoint-editor-banner"),
      ).toHaveAttribute("data-phase", "response");
      await page.getByTestId("toolbar-breakpoint-pending-only").check();
      await expect(
        page.getByTestId("traffic-row").filter({ hasText: "/completed" }),
      ).toHaveCount(0);
      const row = await waitForTrafficRow(page, "/pending");
      await expect(row).toHaveAttribute("data-breakpoint-phase", "response");
      if (resolution !== "timeout") {
        await page.getByRole("button", { name: /Add Filter/ }).click();
        await page.getByPlaceholder("Enter value...").fill("does-not-match");
        await expect(
          page.getByTestId("traffic-row").filter({ hasText: "/pending" }),
        ).toHaveCount(0);
        await page.getByPlaceholder("Enter value...").fill("pending");
        await expect(row).toBeVisible();
      }
      const light = await row.evaluate(
        (el) => getComputedStyle(el).backgroundColor,
      );
      expect(light).not.toBe("rgba(0, 0, 0, 0)");
      if (resolution !== "timeout") {
        await page.getByRole("button", { name: /Fuzzy Search/ }).click();
        await page
          .getByPlaceholder("Enter keyword to search all content...")
          .fill("pending");
        await page.getByTestId("search-mode-submit").click();
        await expect(
          page.getByTestId("search-result-row").filter({ hasText: "/pending" }),
        ).toHaveAttribute("data-breakpoint-phase", "response");
      }
      if (resolution === "resume") {
        await page.getByRole("button", { name: /Exit/ }).click();
        await page.getByTestId("breakpoint-resume-unchanged").click();
      } else if (resolution === "disable")
        await page.getByTestId("toolbar-breakpoint-toggle").click();
      await client.result;
      await expect.poll(async () => (await pending(request)).length).toBe(0);
      if (resolution === "disable") {
        await expect(
          page.getByRole("checkbox", { name: "Paused", exact: true }),
        ).toHaveCount(0);
        const result = page
          .getByTestId("search-result-row")
          .filter({ hasText: "/pending" });
        await expect(result).toBeVisible();
        await expect(result).not.toHaveAttribute(
          "data-breakpoint-phase",
          /request|response/,
        );
        await page.getByTestId("toolbar-breakpoint-toggle").click();
        await expect(
          page.getByRole("checkbox", { name: "Paused", exact: true }),
        ).not.toBeChecked();
        await expect(result).toBeVisible();
      } else {
        await expect(
          page.getByTestId("traffic-row").filter({ hasText: "/pending" }),
        ).toHaveCount(0);
      }
    } finally {
      await upstream.close();
    }
  });
}

for (const mode of ["gzip", "binary", "unknown"] as const) {
  test(`Breakpoint real UI ${mode} response editing preserves bytes and encoding`, async ({
    page,
    request,
  }) => {
    const original = Buffer.from([0, 255, 128, 65]);
    const edited = Buffer.from([255, 0, 194, 169, 66]);
    const upstream = await loopbackUpstream((_req, res) => {
      const body =
        mode === "gzip" ? gzipSync(Buffer.from("gzip-original")) : original;
      res.writeHead(200, {
        "Content-Type":
          mode === "gzip" ? "text/plain" : "application/octet-stream",
        "Content-Length": body.length,
        ...(mode === "binary"
          ? {}
          : { "Content-Encoding": mode === "gzip" ? "gzip" : "x-unknown" }),
      });
      res.end(body);
    });
    try {
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port}/encoded breakpoint://response`,
      );
      const client = wireRequest(upstream.url("/encoded"));
      await expect(
        page.getByTestId("breakpoint-editor-banner"),
      ).toHaveAttribute("data-phase", "response");
      await assertBlocked(
        page,
        client.completed,
        client.responseStarted,
        client.dataStarted,
      );
      if (mode === "gzip") {
        await setMonacoEditor(
          page,
          page.getByTestId("traffic-detail"),
          "gzip-edited-✓",
        );
        await setSelectValue(
          page,
          page.getByTestId("breakpoint-body-encoding"),
          "Base64",
        );
        await setSelectValue(
          page,
          page.getByTestId("breakpoint-body-encoding"),
          "UTF-8",
        );
      } else {
        await expect(
          page.getByTestId("breakpoint-body-encoding"),
        ).toContainText("Base64");
        if (mode === "binary") {
          await setSelectValue(
            page,
            page.getByTestId("breakpoint-body-encoding"),
            "UTF-8",
          );
          await expect(page.locator(".ant-message-notice")).toContainText(
            "not valid UTF-8",
          );
          await expect(
            page.getByTestId("breakpoint-body-encoding"),
          ).toContainText("Base64");
        }
        await setMonacoEditor(
          page,
          page.getByTestId("traffic-detail"),
          "%%%invalid%%%",
        );
        await page.getByTestId("breakpoint-apply-resume").click();
        await expect(
          page
            .locator(".ant-message-notice")
            .filter({ hasText: "valid padded Base64" }),
        ).toBeVisible();
        expect(await pending(request)).toHaveLength(1);
        await setMonacoEditor(
          page,
          page.getByTestId("traffic-detail"),
          edited.toString("base64"),
        );
      }
      await page.getByTestId("breakpoint-apply-resume").click();
      const result = await client.result;
      expect(Number(result.headers["content-length"])).toBe(
        result.bytes.length,
      );
      if (mode === "gzip") {
        expect(result.headers["content-encoding"]).toBe("gzip");
        expect(gunzipSync(result.bytes).toString()).toBe("gzip-edited-✓");
      } else {
        expect(result.bytes).toEqual(edited);
        if (mode === "unknown")
          expect(result.headers["content-encoding"]).toBe("x-unknown");
      }
    } finally {
      await upstream.close();
    }
  });
}

test("Breakpoint real UI finite SSE with known length remains editable on actual wire", async ({
  page,
}) => {
  const original = "data: original\n\n";
  const edited = "data: edited-✓\n\n";
  const upstream = await loopbackUpstream((_req, res) => {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Content-Length": Buffer.byteLength(original),
    });
    res.end(original);
  });
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/finite-sse breakpoint://response`,
    );
    const client = wireRequest(upstream.url("/finite-sse"));
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "response",
    );
    await expect(page.getByTestId("breakpoint-body-omitted")).toHaveCount(0);
    await assertBlocked(
      page,
      client.completed,
      client.responseStarted,
      client.dataStarted,
    );
    await setMonacoEditor(page, page.getByTestId("traffic-detail"), edited);
    await page.getByTestId("breakpoint-apply-resume").click();
    const result = await client.result;
    expect(result.bytes.toString()).toBe(edited);
    expect(result.headers["content-type"]).toBe("text/event-stream");
    expect(Number(result.headers["content-length"])).toBe(
      Buffer.byteLength(edited),
    );
  } finally {
    await upstream.close();
  }
});

for (const status of [204, 304]) {
  test(`Breakpoint real UI response edit ${status} enforces bodyless wire semantics`, async ({
    page,
    request,
  }) => {
    const upstream = await loopbackUpstream();
    try {
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port}/bodyless breakpoint://response`,
      );
      const client = wireRequest(upstream.url("/bodyless"));
      await expect(
        page.getByTestId("breakpoint-editor-banner"),
      ).toHaveAttribute("data-phase", "response");
      if (status === 204) {
        await page.getByTestId("breakpoint-status-input").fill("100");
        await page.getByTestId("breakpoint-apply-resume").click();
        await expect(page.locator(".ant-message-notice")).toContainText(
          /200.*599/,
        );
        expect(await pending(request)).toHaveLength(1);
        await assertBlocked(
          page,
          client.completed,
          client.responseStarted,
          client.dataStarted,
        );
      }
      await page.getByTestId("breakpoint-status-input").fill(String(status));
      await page.getByTestId("breakpoint-apply-resume").click();
      const result = await client.result;
      expect(result.status).toBe(status);
      expect(result.bytes.length).toBe(0);
      expect(result.headers["transfer-encoding"]).toBeUndefined();
    } finally {
      await upstream.close();
    }
  });
}

test("Breakpoint real UI HEAD keeps response bodyless when status changes", async ({
  page,
}) => {
  const upstream = await loopbackUpstream();
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/head breakpoint://response`,
    );
    const client = wireRequest(upstream.url("/head"), { method: "HEAD" });
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "response",
    );
    await page.getByTestId("breakpoint-status-input").fill("202");
    await page.getByTestId("breakpoint-apply-resume").click();
    const result = await client.result;
    expect(result.status).toBe(202);
    expect(result.bytes.length).toBe(0);
  } finally {
    await upstream.close();
  }
});

test("Breakpoint real UI browser reconnect preserves pending and drafts; disconnect does not forward requests", async ({
  page,
  request,
}) => {
  const upstream = await loopbackUpstream();
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/reconnect* breakpoint://request`,
    );
    const client = wireRequest(upstream.url("/reconnect"), {
      method: "POST",
      body: "original",
    });
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "request",
    );
    await page.getByTestId("breakpoint-method-input").fill("PATCH");
    await setMonacoEditor(
      page,
      page.getByTestId("traffic-detail"),
      "reconnect-draft",
    );
    await page.context().setOffline(true);
    await page.waitForTimeout(500);
    expect(upstream.records).toHaveLength(0);
    expect(upstream.startedRequests()).toBe(0);
    expect(client.completed()).toBe(false);
    await page.context().setOffline(false);
    await page.waitForTimeout(1500);
    await expect(page.getByTestId("breakpoint-method-input")).toHaveValue(
      "PATCH",
    );
    await page.getByTestId("breakpoint-apply-resume").click();
    await client.result;
    expect(upstream.records[0].bytes.toString()).toBe("reconnect-draft");
    expect(upstream.records[0].method).toBe("PATCH");
    const second = wireRequest(upstream.url("/reconnect-second"));
    await expect.poll(async () => (await pending(request)).length).toBe(1);
    await page.close();
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(upstream.records).toHaveLength(1);
    expect(second.completed()).toBe(false);
    await request.post(`${apiBase}/breakpoint/settings`, {
      data: { enabled: false, max_body_bytes: 1048576 },
    });
    expect((await second.result).status).toBe(200);
  } finally {
    await upstream.close();
  }
});

test("Breakpoint real UI client disconnect cleanup is bounded by safe timeout", async ({
  page,
  request,
}) => {
  const upstream = await loopbackUpstream();
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/abort breakpoint://request`,
    );
    await request.put(`${apiBase}/config/performance`, {
      data: { breakpoint_timeout_ms: 5000 },
    });
    const client = wireRequest(upstream.url("/abort"));
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "request",
    );
    expect(upstream.records).toHaveLength(0);
    expect(upstream.startedRequests()).toBe(0);
    client.abort();
    await expect(client.result).rejects.toThrow();
    await expect
      .poll(async () => (await pending(request)).length, { timeout: 8000 })
      .toBe(0);
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveCount(0);
  } finally {
    await upstream.close();
  }
});

for (const kind of ["oversize", "stream"] as const) {
  test(`Breakpoint real UI ${kind} response pauses promptly with explicit body edit limit`, async ({
    page,
    request,
  }) => {
    const upstream = await loopbackUpstream((_req, res) => {
      if (kind === "oversize") {
        const body = Buffer.alloc(8192, 65);
        res.writeHead(200, {
          "Content-Type": "text/plain",
          "Content-Length": body.length,
        });
        res.end(body);
      } else {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write("data: indefinitely open\n\n");
      }
    });
    try {
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port}/limited breakpoint://response`,
      );
      await request.post(`${apiBase}/breakpoint/settings`, {
        data: { enabled: true, max_body_bytes: 1024 },
      });
      const client = wireRequest(upstream.url("/limited"));
      await expect(
        page.getByTestId("breakpoint-editor-banner"),
      ).toHaveAttribute("data-phase", "response");
      await expect(page.getByTestId("breakpoint-body-omitted")).toBeVisible();
      await assertBlocked(
        page,
        client.completed,
        client.responseStarted,
        client.dataStarted,
      );
      if (kind === "stream")
        await page.getByTestId("breakpoint-status-input").fill("204");
      else {
        await addHeader(page, "response", "X-Limit-Edited", "yes");
        // Framing follows the preserved payload, including metadata-only edits.
        const names = page.locator(
          '[data-testid^="response-header-view-name-"]',
        );
        for (let index = 0; index < (await names.count()); index++) {
          if (
            (await names.nth(index).inputValue()).toLowerCase() ===
            "content-length"
          )
            await page
              .getByTestId(`response-header-view-value-${index}`)
              .fill("1");
        }
      }
      await page.getByTestId("breakpoint-apply-resume").click();
      const result = await client.result;
      expect(result.status).toBe(kind === "stream" ? 204 : 200);
      expect(result.bytes.length).toBe(kind === "stream" ? 0 : 8192);
      if (kind === "oversize") {
        expect(result.headers["x-limit-edited"]).toBe("yes");
        expect(Number(result.headers["content-length"])).toBe(8192);
      }
    } finally {
      await upstream.close();
    }
  });
}

test("Breakpoint real UI binary request Base64 edit reaches upstream unchanged in bytes", async ({
  page,
}) => {
  const upstream = await loopbackUpstream();
  const edited = Buffer.from([255, 0, 128, 65]);
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/binary-request breakpoint://request`,
    );
    const client = wireRequest(upstream.url("/binary-request"), {
      method: "POST",
      headers: { "Content-Type": "application/octet-stream" },
      body: Buffer.from([0, 255, 128]),
    });
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "request",
    );
    await expect(page.getByTestId("breakpoint-body-encoding")).toContainText(
      "Base64",
    );
    expect(upstream.records).toHaveLength(0);
    expect(upstream.startedRequests()).toBe(0);
    await setMonacoEditor(
      page,
      page.getByTestId("traffic-detail"),
      edited.toString("base64"),
    );
    await page.getByTestId("breakpoint-apply-resume").click();
    await client.result;
    expect(upstream.records[0].bytes).toEqual(edited);
    expect(Number(upstream.records[0].headers["content-length"])).toBe(
      edited.length,
    );
  } finally {
    await upstream.close();
  }
});

for (const tlsCase of [
  { phase: "request", mode: "text" },
  { phase: "response", mode: "text" },
  { phase: "both", mode: "text" },
  { phase: "response", mode: "gzip" },
  { phase: "response", mode: "stream" },
  { phase: "response", mode: "finite-sse" },
] as const) {
  test(`Breakpoint real UI nonstandard TLS ${tlsCase.phase}/${tlsCase.mode} actual wire edits`, async ({
    page,
  }, testInfo) => {
    const { loopbackTlsUpstream, tlsWireRequest } = await import(
      "./helpers/breakpoint-real"
    );
    const upstream = await loopbackTlsUpstream(tlsCase.mode);
    try {
      expect([443, 8443, 9900]).not.toContain(upstream.port);
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port} breakpoint://${tlsCase.phase}`,
      );
      let completed = false;
      let responseData = false;
      const resultPromise = tlsWireRequest(upstream.port, () => {
        responseData = true;
      }).then((result) => {
        completed = true;
        return result;
      });
      void resultPromise.catch(() => {});
      const firstPhase = tlsCase.phase === "response" ? "response" : "request";
      await expect(
        page.getByTestId("breakpoint-editor-banner"),
      ).toHaveAttribute("data-phase", firstPhase);
      const firstTlsRow = await waitForTrafficRow(page, "/tls-breakpoint");
      const requestId = await firstTlsRow.getAttribute("data-record-id");
      expect(requestId).toBeTruthy();
      await assertBlocked(
        page,
        () => completed,
        () => responseData,
      );
      if (firstPhase === "request") {
        expect(upstream.hits()).toBe(0);
        await page.getByTestId("breakpoint-method-input").fill("PUT");
        await page
          .getByTestId("breakpoint-url-input")
          .fill(`https://127.0.0.1:${upstream.port}/tls-edited?query=edited`);
        await setMonacoEditor(
          page,
          page.getByTestId("traffic-detail"),
          "tls-request-edited",
        );
        await editHeader(page, "request", "Host", "edited.virtual.test:1234");
        await editHeader(page, "request", "User-Agent", "breakpoint-ui-agent");
        await editHeader(page, "request", "Cookie", "edited=ui");
        await editHeader(
          page,
          "request",
          "Referer",
          "https://edited-referrer.test/ui",
        );
        await addHeader(page, "request", "X-Duplicate", "one");
        await addHeader(page, "request", "X-Duplicate", "two");
        await page.getByTestId("breakpoint-apply-resume").click();
        await expect.poll(() => upstream.records.length).toBe(1);
        expect(upstream.records[0].method).toBe("PUT");
        expect(upstream.records[0].url).toBe("/tls-edited?query=edited");
        expect(upstream.records[0].bytes.toString()).toBe("tls-request-edited");
        expect(upstream.records[0].headers.host).toBe(
          "edited.virtual.test:1234",
        );
        expect(upstream.records[0].headers["user-agent"]).toBe(
          "breakpoint-ui-agent",
        );
        expect(upstream.records[0].headers.cookie).toBe("edited=ui");
        expect(upstream.records[0].headers.referer).toBe(
          "https://edited-referrer.test/ui",
        );
        expect(
          upstream.records[0].rawHeaders.filter(
            (value) => value.toLowerCase() === "x-duplicate",
          ),
        ).toHaveLength(2);
      }
      if (tlsCase.phase !== "request") {
        if (tlsCase.phase === "both") {
          const row = page.locator(
            `[data-testid="traffic-row"][data-record-id="${requestId}"]`,
          );
          await expect(row).toHaveAttribute(
            "data-breakpoint-phase",
            "response",
          );
          await row.click();
        }
        await expect(
          page.getByTestId("breakpoint-editor-banner"),
        ).toHaveAttribute("data-phase", "response");
        expect(upstream.hits()).toBe(1);
        if (tlsCase.mode === "finite-sse")
          await expect(page.getByTestId("breakpoint-body-omitted")).toHaveCount(
            0,
          );
        await assertBlocked(
          page,
          () => completed,
          () => responseData,
        );
        await page
          .getByTestId("breakpoint-status-input")
          .fill(tlsCase.mode === "stream" ? "204" : "202");
        if (tlsCase.mode === "stream")
          await expect(
            page.getByTestId("breakpoint-body-omitted"),
          ).toBeVisible();
        else
          await setMonacoEditor(
            page,
            page.getByTestId("traffic-detail"),
            "tls-edited-response",
          );
        if (tlsCase.mode === "finite-sse")
          await addHeader(page, "response", "Content-Length", "1");
        await page.screenshot({
          path: testInfo.outputPath("tls-pause.png"),
          fullPage: true,
        });
        await page.getByTestId("breakpoint-apply-resume").click();
      }
      const result = await resultPromise;
      if (tlsCase.mode === "finite-sse") {
        expect(result.responseHeaders).toMatch(
          /content-type: text\/event-stream/i,
        );
        expect(result.responseHeaders).toMatch(
          /content-length: 19(?:\r?\n|$)/i,
        );
      }
      expect(result.stdout).toBe(
        tlsCase.phase === "request"
          ? "tls-original\n200"
          : tlsCase.mode === "stream"
            ? "\n204"
            : "tls-edited-response\n202",
      );
    } finally {
      await upstream.close();
    }
  });
}

for (const bodyless of ["HEAD", "204", "304"] as const) {
  test(`Breakpoint real UI initially bodyless TLS SSE ${bodyless} pauses without Content-Length`, async ({
    page,
  }) => {
    const { loopbackTlsUpstream, tlsWireRequest } = await import(
      "./helpers/breakpoint-real"
    );
    const status = bodyless === "HEAD" ? 200 : Number(bodyless);
    const upstream = await loopbackTlsUpstream("bodyless", { status });
    let completed = false;
    let responseStarted = false;
    try {
      await configureBreakpoint(
        page,
        `127.0.0.1:${upstream.port}/tls-breakpoint breakpoint://response`,
      );
      const client = tlsWireRequest(
        upstream.port,
        () => {
          responseStarted = true;
        },
        bodyless === "HEAD" ? { method: "HEAD" } : {},
      ).then((result) => {
        completed = true;
        return result;
      });
      await expect(
        page.getByTestId("breakpoint-editor-banner"),
      ).toHaveAttribute("data-phase", "response");
      await assertBlocked(
        page,
        () => completed,
        () => responseStarted,
      );
      const editedStatus = bodyless === "HEAD" ? 202 : status;
      await page
        .getByTestId("breakpoint-status-input")
        .fill(String(editedStatus));
      await addHeader(page, "response", "X-Bodyless-Edit", bodyless);
      await page.getByTestId("breakpoint-apply-resume").click();
      const result = await client;
      expect(result.stdout).toBe(`\n${editedStatus}`);
      expect(result.responseHeaders).toMatch(
        new RegExp(`x-bodyless-edit: ${bodyless}`, "i"),
      );
      expect(upstream.records[0].method).toBe(
        bodyless === "HEAD" ? "HEAD" : "POST",
      );
    } finally {
      await upstream.close();
    }
  });
}

test("Breakpoint real UI visual query editing and invalid metadata do not silently release pending", async ({
  page,
  request,
}) => {
  const upstream = await loopbackUpstream();
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/validation breakpoint://request`,
    );
    const client = wireRequest(upstream.url("/validation?dup=old&dup=kept"), {
      method: "POST",
      body: "original",
    });
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "request",
    );
    await page.getByTestId("breakpoint-method-input").fill("INVALID METHOD");
    await page.getByTestId("breakpoint-apply-resume").click();
    await expect(page.locator(".ant-message-notice")).toContainText(
      "valid HTTP method",
    );
    expect(await pending(request)).toHaveLength(1);
    expect(upstream.records).toHaveLength(0);
    expect(upstream.startedRequests()).toBe(0);
    await page.getByTestId("breakpoint-method-input").fill("PUT");
    await page.getByTestId("breakpoint-url-input").fill("file:///invalid");
    await page.getByTestId("breakpoint-apply-resume").click();
    await expect(
      page
        .locator(".ant-message-notice")
        .filter({ hasText: "absolute HTTP(S) URL" }),
    ).toBeVisible();
    expect(await pending(request)).toHaveLength(1);
    await page
      .getByTestId("breakpoint-url-input")
      .fill(upstream.url("/validation?dup=old&dup=kept"));
    await page.getByTestId("request-tab-query").click();
    await page.getByTestId("breakpoint-query-value-0").fill("edited");
    await page.getByTestId("breakpoint-query-add").click();
    await page
      .locator('[data-testid^="breakpoint-query-name-"]')
      .last()
      .fill("more");
    await page
      .locator('[data-testid^="breakpoint-query-value-"]')
      .last()
      .fill("✓ & spaces");
    await page.getByTestId("breakpoint-apply-resume").click();
    await client.result;
    const url = new URL(upstream.records[0].url, upstream.url("/"));
    expect(url.searchParams.getAll("dup")).toEqual(["edited", "kept"]);
    expect(url.searchParams.get("more")).toBe("✓ & spaces");
  } finally {
    await upstream.close();
  }
});

test("Breakpoint real UI infinite chunked request pauses before EOF with explicit bounded editing", async ({
  page,
  request,
}) => {
  const upstream = await loopbackUpstream();
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/request-stream breakpoint://request`,
    );
    const client = wireRequest(upstream.url("/request-stream"), {
      method: "POST",
      body: "stream-prefix-",
      leaveOpen: true,
      headers: { "Content-Type": "text/plain", "Transfer-Encoding": "chunked" },
    });
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "request",
      { timeout: 3000 },
    );
    await expect(page.getByTestId("breakpoint-body-omitted")).toBeVisible();
    expect(await pending(request)).toHaveLength(1);
    await assertBlocked(
      page,
      client.completed,
      client.responseStarted,
      client.dataStarted,
    );
    expect(upstream.records).toHaveLength(0);
    expect(upstream.startedRequests()).toBe(0);
    await page.getByTestId("breakpoint-resume-unchanged").click();
    client.finish("stream-suffix");
    expect((await client.result).status).toBe(200);
    expect(upstream.records[0].bytes.toString()).toBe(
      "stream-prefix-stream-suffix",
    );
  } finally {
    await upstream.close();
  }
});

test("Breakpoint real UI stalled declared response downgrades capture and replays prefix exactly", async ({
  page,
}) => {
  const prefix = Buffer.from("stalled-prefix-");
  const suffix = Buffer.from("late-suffix");
  let finish: (() => void) | undefined;
  const upstream = await loopbackUpstream((_req, res) => {
    res.writeHead(200, {
      "Content-Type": "text/plain",
      "Content-Length": prefix.length + suffix.length,
    });
    res.write(prefix);
    finish = () => res.end(suffix);
  });
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/stalled breakpoint://response`,
    );
    const client = wireRequest(upstream.url("/stalled"));
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "response",
      { timeout: 6000 },
    );
    await expect(page.getByTestId("breakpoint-body-omitted")).toBeVisible();
    await assertBlocked(
      page,
      client.completed,
      client.responseStarted,
      client.dataStarted,
    );
    await page.getByTestId("breakpoint-resume-unchanged").click();
    finish?.();
    const result = await client.result;
    expect(result.bytes).toEqual(Buffer.concat([prefix, suffix]));
    expect(Number(result.headers["content-length"])).toBe(
      prefix.length + suffix.length,
    );
  } finally {
    await upstream.close();
  }
});

test("Breakpoint real UI stalled declared request downgrades capture and preserves consumed prefix", async ({
  page,
}) => {
  const upstream = await loopbackUpstream();
  const prefix = "request-prefix-";
  const suffix = "late-request-suffix";
  try {
    await configureBreakpoint(
      page,
      `127.0.0.1:${upstream.port}/request-stalled breakpoint://request`,
    );
    const client = wireRequest(upstream.url("/request-stalled"), {
      method: "POST",
      body: prefix,
      leaveOpen: true,
      headers: {
        "Content-Type": "text/plain",
        "Content-Length": String(Buffer.byteLength(prefix + suffix)),
      },
    });
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "request",
      { timeout: 6000 },
    );
    await expect(page.getByTestId("breakpoint-body-omitted")).toBeVisible();
    await assertBlocked(
      page,
      client.completed,
      client.responseStarted,
      client.dataStarted,
    );
    expect(upstream.startedRequests()).toBe(0);
    await page.getByTestId("breakpoint-resume-unchanged").click();
    client.finish(suffix);
    expect((await client.result).status).toBe(200);
    expect(upstream.records[0].bytes.toString()).toBe(prefix + suffix);
  } finally {
    await upstream.close();
  }
});
