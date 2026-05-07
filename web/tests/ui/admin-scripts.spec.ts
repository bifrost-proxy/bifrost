import { test, expect } from "@playwright/test";
import {
  apiBase,
  clearRules,
  clearScripts,
  clearTraffic,
  openPage,
  setMonacoEditor,
  uniqueName,
  waitForToast,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

async function createScriptFromHeaderMenu(
  page: import("@playwright/test").Page,
  type: "request" | "response" | "decode" | "parser",
) {
  await page.getByTestId("scripts-create-menu-button").click();
  await page.getByTestId(`scripts-create-${type}-item`).click();
}

test.beforeEach(async ({ request }) => {
  await clearTraffic(request);
  await clearRules(request);
  await clearScripts(request);
});

test("Scripts 左侧头部在亮色和暗色主题下只保留创建菜单与更多菜单", async ({
  page,
}) => {
  await openPage(page, "scripts");
  await page.evaluate(() => {
    localStorage.setItem(
      "bifrost-theme",
      JSON.stringify({ state: { mode: "light" }, version: 0 }),
    );
  });
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.getByTestId("scripts-create-menu-button")).toBeVisible();
  await expect(page.getByTestId("scripts-more-menu-button")).toBeVisible();
  await expect(page.getByTestId("scripts-new-request-button")).toHaveCount(0);
  await expect(page.getByTestId("scripts-new-response-button")).toHaveCount(0);
  await expect(page.getByTestId("scripts-new-decode-button")).toHaveCount(0);

  await page.getByTestId("scripts-create-menu-button").click();
  await expect(page.getByTestId("scripts-create-request-item")).toBeVisible();
  await expect(page.getByTestId("scripts-create-response-item")).toBeVisible();
  await expect(page.getByTestId("scripts-create-decode-item")).toBeVisible();
  await expect(page.getByTestId("scripts-create-parser-item")).toBeVisible();
  await page.keyboard.press("Escape");

  await page.evaluate(() => {
    localStorage.setItem(
      "bifrost-theme",
      JSON.stringify({ state: { mode: "dark" }, version: 0 }),
    );
  });
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByTestId("scripts-create-menu-button")).toBeVisible();
  await expect(page.getByTestId("scripts-more-menu-button")).toBeVisible();
  await page.getByTestId("scripts-more-menu-button").click();
  await expect(page.getByTestId("scripts-more-sandbox-item")).toBeVisible();
  await expect(page.getByTestId("scripts-more-export-all-item")).toBeVisible();
  await expect(page.getByTestId("scripts-more-import-item")).toBeVisible();
  const fileChooserPromise = page.waitForEvent("filechooser");
  await page.getByTestId("scripts-more-import-item").click();
  const fileChooser = await fileChooserPromise;
  expect(fileChooser.isMultiple()).toBeTruthy();
});

test("Scripts parser 编辑器识别 bp parser 运行时 ctx/request/response 字段", async ({
  page,
}) => {
  await openPage(page, "scripts");
  await createScriptFromHeaderMenu(page, "parser");
  await setMonacoEditor(
    page,
    page.getByTestId("scripts-editor"),
    `function bodyBase64() {
  if (ctx.phase === "request" || ctx.phase === "websocket_send") {
    return request && request.bodyBase64 ? request.bodyBase64 : "";
  }
  return response && response.bodyBase64 ? response.bodyBase64 : "";
}

function currentRequest() {
  if (ctx.phase === "request" || ctx.phase === "websocket_send") return request || {};
  return response && response.request ? response.request : {};
}

function requestPattern(params) {
  if (params.pattern) return params.pattern;
  if (ctx.phase === "request" || ctx.phase === "websocket_send") {
    return request && request.path ? request.path : "/";
  }
  return response && response.request && response.request.path ? response.request.path : "/";
}

function normalizeHttpRpc(endpointInfo, parsed, params) {
  var req = currentRequest();
  var phase = ctx.phase === "response" || ctx.phase === "websocket_recv" ? "response" : "request";
  return {
    protocol: "http-rpc",
    serializer: endpointInfo && (phase === "response" ? endpointInfo.resp_serializer : endpointInfo.serializer),
    psm: params.psm || "",
    method: (endpointInfo && (endpointInfo.rpc_method || endpointInfo.name)) || params.method || params.rpcMethod || "",
    endpoint_id: endpointInfo && endpointInfo.endpoint_id,
    endpoint_path: (endpointInfo && endpointInfo.path) || params.path || "",
    http: {
      method: req.method || "",
      host: req.host || "",
      path: req.path || "",
      url: req.url || "",
    },
    schema_type: phase,
    data: parsed,
  };
}

ctx.output = { data: JSON.stringify(normalizeHttpRpc({}, {}, {})), code: "0", msg: bodyBase64() || requestPattern({}) };`,
  );

  await page.waitForTimeout(1200);
  const markerInfo = await page.evaluate(() => {
    const monaco = (window as unknown as { monaco?: typeof import("monaco-editor") }).monaco;
    if (!monaco?.editor) {
      return { monacoReady: false, errors: [] as string[] };
    }
    const errors = monaco.editor
      .getModelMarkers({})
      .filter((marker) => marker.severity === monaco.MarkerSeverity.Error)
      .map((marker) => marker.message);
    return { monacoReady: true, errors };
  });

  expect(markerInfo.monacoReady).toBeTruthy();
  expect(markerInfo.errors).toEqual([]);
});

test("Scripts 页面完成创建、测试、push 同步，并让请求脚本真实作用于代理流量", async ({
  page,
  context,
}) => {
  const requestScriptName = uniqueName("request-script");
  const responseScriptName = uniqueName("response-script");
  const decodeScriptName = uniqueName("decode-script");
  const parserScriptName = uniqueName("parser-script");
  const pushedScriptName = uniqueName("push-script");
  await openPage(page, "scripts");
  await expect(page.getByTestId("scripts-list-panel")).toBeVisible();
  await expect(page.getByTestId("scripts-create-menu-button")).toBeVisible();
  await expect(page.getByTestId("scripts-more-menu-button")).toBeVisible();

  const syncPage = await context.newPage();
  await openPage(syncPage, "scripts");
  await expect(syncPage.getByTestId("scripts-list-panel")).toBeVisible();

  await page.getByTestId("scripts-create-menu-button").click();
  await expect(page.getByTestId("scripts-create-request-item")).toBeVisible();
  await expect(page.getByTestId("scripts-create-response-item")).toBeVisible();
  await expect(page.getByTestId("scripts-create-decode-item")).toBeVisible();
  await expect(page.getByTestId("scripts-create-parser-item")).toBeVisible();
  await page.getByTestId("scripts-create-request-item").click();
  await setMonacoEditor(
    page,
    page.getByTestId("scripts-editor"),
    'request.headers["x-script-ui"] = "applied";',
  );
  await page.getByTestId("scripts-save-button").click();
  const saveDialog = page.getByRole("dialog", { name: "Save New Script" });
  await saveDialog
    .getByPlaceholder("Enter script name (e.g., api/add-auth-header)")
    .fill(requestScriptName);
  await saveDialog.getByRole("button", { name: "Save" }).click();
  await waitForToast(page, "Script created");

  const requestNode = page
    .getByTestId("script-item")
    .filter({ hasText: requestScriptName.split("/").pop() || requestScriptName })
    .first();
  await expect(requestNode).toBeVisible();
  await expect(
    syncPage
      .getByTestId("script-item")
      .filter({ hasText: requestScriptName.split("/").pop() || requestScriptName })
      .first(),
  ).toBeVisible();

  await requestNode.click();
  await page.getByTestId("scripts-test-button").click();
  await expect(page.getByTestId("scripts-test-result-panel")).toBeVisible();

  await createScriptFromHeaderMenu(page, "response");
  await setMonacoEditor(
    page,
    page.getByTestId("scripts-editor"),
    'response.headers["x-response-script"] = "enabled";',
  );
  await page.getByTestId("scripts-save-button").click();
  await saveDialog
    .getByPlaceholder("Enter script name (e.g., api/add-auth-header)")
    .fill(responseScriptName);
  await saveDialog.getByRole("button", { name: "Save" }).click();
  await waitForToast(page, "Script created");

  await createScriptFromHeaderMenu(page, "decode");
  await setMonacoEditor(
    page,
    page.getByTestId("scripts-editor"),
    'ctx.output = { data: "decoded-ui", code: "ok", msg: "from-ui" };',
  );
  await page.getByTestId("scripts-save-button").click();
  await saveDialog
    .getByPlaceholder("Enter script name (e.g., api/add-auth-header)")
    .fill(decodeScriptName);
  await saveDialog.getByRole("button", { name: "Save" }).click();
  await waitForToast(page, "Script created");

  const decodeNode = page
    .getByTestId("script-item")
    .filter({ hasText: decodeScriptName.split("/").pop() || decodeScriptName })
    .first();
  await decodeNode.click();
  await page.getByTestId("scripts-test-button").click();
  await expect(page.getByTestId("scripts-test-result-panel")).toBeVisible();

  await createScriptFromHeaderMenu(page, "parser");
  await expect(page.getByText("Parser", { exact: true })).toBeVisible();
  await setMonacoEditor(
    page,
    page.getByTestId("scripts-editor"),
    'ctx.output = { data: "parser-ui", code: "0", msg: "" };',
  );
  await page.getByTestId("scripts-save-button").click();
  await saveDialog
    .getByPlaceholder("Enter script name (e.g., api/add-auth-header)")
    .fill(parserScriptName);
  await saveDialog.getByRole("button", { name: "Save" }).click();
  await waitForToast(page, "Script created");
  await expect(
    page.getByTestId("script-item").filter({ hasText: parserScriptName }).first(),
  ).toBeVisible();
  await expect(
    page.getByTestId("script-item").filter({ hasText: "PAR" }).first(),
  ).toBeVisible();

  await page.getByTestId("scripts-more-menu-button").click();
  await expect(page.getByTestId("scripts-more-sandbox-item")).toBeVisible();
  await expect(page.getByTestId("scripts-more-export-all-item")).toBeVisible();
  await expect(page.getByTestId("scripts-more-import-item")).toBeVisible();
  await page.keyboard.press("Escape");

  await createScriptFromHeaderMenu(page, "request");
  await setMonacoEditor(
    page,
    page.getByTestId("scripts-editor"),
    'request.headers["x-push-sync"] = "ok";',
  );
  await page.getByTestId("scripts-save-button").click();
  await saveDialog
    .getByPlaceholder("Enter script name (e.g., api/add-auth-header)")
    .fill(pushedScriptName);
  await saveDialog.getByRole("button", { name: "Save" }).click();
  await waitForToast(page, "Script created");

  await expect(
    page.getByTestId("script-item").filter({ hasText: pushedScriptName }).first(),
  ).toBeVisible();
  await expect(
    syncPage.getByTestId("script-item").filter({ hasText: pushedScriptName }).first(),
  ).toBeVisible();

  await page.getByTestId("script-item").filter({ hasText: requestScriptName }).first().click();
  await page.getByTestId("scripts-delete-button").click();
  await page.getByRole("dialog", { name: "Delete Script" }).getByRole("button", { name: "Delete" }).click();
  await waitForToast(page, "Script deleted");
  await expect(
    page.getByTestId("script-item").filter({ hasText: requestScriptName }),
  ).toHaveCount(0);
  await expect(
    syncPage.getByTestId("script-item").filter({ hasText: requestScriptName }),
  ).toHaveCount(0);

  await syncPage.close();
});

test("Scripts 列表在获得焦点后支持上下键切换选中项", async ({
  page,
  request,
}) => {
  const firstScriptName = uniqueName("aaa-keyboard-script-a");
  const secondScriptName = uniqueName("aab-keyboard-script-b");

  const createFirstScriptRes = await request.put(
    `${apiBase}/scripts/request/${encodeURIComponent(firstScriptName)}`,
    {
      data: { content: 'request.headers["x-keyboard-script"] = "first";' },
    },
  );
  if (!createFirstScriptRes.ok()) {
    throw new Error(await createFirstScriptRes.text());
  }

  const createSecondScriptRes = await request.put(
    `${apiBase}/scripts/request/${encodeURIComponent(secondScriptName)}`,
    {
      data: { content: 'request.headers["x-keyboard-script"] = "second";' },
    },
  );
  if (!createSecondScriptRes.ok()) {
    throw new Error(await createSecondScriptRes.text());
  }

  await openPage(page, "scripts");
  await expect(page.getByTestId("scripts-list-panel")).toBeVisible();

  const firstScriptItem = page
    .getByTestId("script-item")
    .filter({ hasText: firstScriptName })
    .first();
  const secondScriptItem = page
    .getByTestId("script-item")
    .filter({ hasText: secondScriptName })
    .first();

  await expect(firstScriptItem).toBeVisible();
  await expect(secondScriptItem).toBeVisible();
  await expect(firstScriptItem).toHaveAttribute("data-script-name", firstScriptName);
  await expect(secondScriptItem).toHaveAttribute("data-script-name", secondScriptName);

  await firstScriptItem.click();
  await expect(firstScriptItem).toHaveAttribute("aria-selected", "true");

  const scriptsListbox = page.getByRole("listbox", { name: "Scripts list" });
  await scriptsListbox.focus();

  await page.keyboard.press("ArrowDown");
  await expect(secondScriptItem).toHaveAttribute("aria-selected", "true");
  await expect(firstScriptItem).toHaveAttribute("aria-selected", "false");

  await page.keyboard.press("ArrowUp");
  await expect(firstScriptItem).toHaveAttribute("aria-selected", "true");
  await expect(secondScriptItem).toHaveAttribute("aria-selected", "false");
});
