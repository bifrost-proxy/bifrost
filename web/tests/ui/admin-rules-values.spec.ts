import fs from "node:fs/promises";
import { test, expect } from "@playwright/test";
import {
  apiBase,
  clearRules,
  clearTraffic,
  clearValues,
  openPage,
  sendProxyRequest,
  startMockHttpServer,
  uniqueName,
  waitForTrafficRow,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

async function changeSort(page: import("@playwright/test").Page, testId: string, label: string) {
  await page.getByTestId(testId).click();
  await page.locator(".ant-select-dropdown").getByText(label, { exact: true }).click();
}

test.beforeEach(async ({ request }) => {
  await clearTraffic(request);
  await clearRules(request);
  await clearValues(request);
});

test("Rules 页面会主动拉取 syntax 信息，并包含动态脚本与协议别名", async ({
  page,
  request,
}) => {
  const requestScriptName = uniqueName("syntax-request-script");
  const createScriptRes = await request.put(
    `${apiBase}/scripts/request/${encodeURIComponent(requestScriptName)}`,
    {
      data: { content: 'request.headers["x-syntax-check"] = "ok";' },
    },
  );
  if (!createScriptRes.ok()) {
    throw new Error(await createScriptRes.text());
  }

  const syntaxResponsePromise = page.waitForResponse((response) =>
    response.url().includes("/_bifrost/api/syntax"),
  );

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const syntaxResponse = await syntaxResponsePromise;
  expect(syntaxResponse.ok()).toBeTruthy();
  const syntaxPayload = (await syntaxResponse.json()) as {
    protocol_aliases: Record<string, string>;
    protocols: Array<{ name: string }>;
    scripts: { request_scripts: Array<{ name: string }> };
  };

  expect(syntaxPayload.protocol_aliases.pathReplace).toBe("urlReplace");
  expect(syntaxPayload.protocols.some((protocol) => protocol.name === "reqHeaders")).toBeTruthy();
  expect(
    syntaxPayload.scripts.request_scripts.some((script) => script.name === requestScriptName),
  ).toBeTruthy();

  await page.getByTestId("rule-new-button").click();
  await expect(page.getByTestId("rule-editor")).toBeVisible();
});

test("Values 页面完成 CRUD、支持多种排序，并通过 push 自动同步外部写入", async ({
  page,
  context,
  request,
}) => {
  const valueName = uniqueName("a-ui-value");
  const renamedValueName = `${valueName}-renamed`;
  const pushedValueName = uniqueName("z-push-value");

  await openPage(page, "values");
  await expect(page.getByTestId("values-list")).toBeVisible();

  const syncPage = await context.newPage();
  await openPage(syncPage, "values");
  await expect(syncPage.getByTestId("values-list")).toBeVisible();

  await page.getByTestId("value-new-button").click();
  await page
    .getByRole("dialog")
    .getByPlaceholder("Value name (e.g., api_key, auth_token)")
    .fill(valueName);
  await page.getByRole("button", { name: "Create" }).click();

  const valueItem = page.getByTestId("value-item").filter({ hasText: valueName }).first();
  await expect(valueItem).toBeVisible();
  await valueItem.click();

  await valueItem.getByTestId("value-item-menu").click();
  await page.getByRole("menuitem", { name: "Rename" }).click();
  await page.getByRole("dialog").getByPlaceholder("New name").fill(renamedValueName);
  await page.getByRole("button", { name: "Rename" }).click();
  const renamedValueItem = page
    .getByTestId("value-item")
    .filter({ hasText: renamedValueName })
    .first();
  await expect(renamedValueItem).toBeVisible();

  await page.getByTestId("value-new-button").click();
  await page
    .getByRole("dialog")
    .getByPlaceholder("Value name (e.g., api_key, auth_token)")
    .fill(pushedValueName);
  await page.getByRole("button", { name: "Create" }).click();

  await expect(
    page.getByTestId("value-item").filter({ hasText: pushedValueName }).first(),
  ).toBeVisible();
  await expect(page.getByTestId("value-item").first()).toHaveAttribute(
    "data-value-name",
    pushedValueName,
  );
  await expect(
    syncPage.getByTestId("value-item").filter({ hasText: pushedValueName }).first(),
  ).toBeVisible();
  await expect(syncPage.getByTestId("value-item").first()).toHaveAttribute(
    "data-value-name",
    pushedValueName,
  );

  await changeSort(page, "value-sort-select", "Name");
  await expect(page.getByTestId("value-item").first()).toHaveAttribute(
    "data-value-name",
    renamedValueName,
  );

  await page.waitForTimeout(1100);
  const updateValueRes = await request.put(
    `${apiBase}/values/${encodeURIComponent(renamedValueName)}`,
    { data: { value: "updated-by-api" } },
  );
  if (!updateValueRes.ok()) {
    throw new Error(await updateValueRes.text());
  }
  await page.getByTestId("value-refresh-button").click();

  await changeSort(page, "value-sort-select", "Updated");
  await expect(page.getByTestId("value-item").first()).toHaveAttribute(
    "data-value-name",
    renamedValueName,
  );

  await renamedValueItem.getByTestId("value-item-menu").click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  await page.getByRole("dialog", { name: "Delete Value" }).getByRole("button", { name: "Delete" }).click();
  await expect(
    page.getByTestId("value-item").filter({ hasText: renamedValueName }),
  ).toHaveCount(0);
  await expect(
    syncPage.getByTestId("value-item").filter({ hasText: renamedValueName }),
  ).toHaveCount(0);

  await syncPage.close();
});

test("Values 页面支持 bifrost-file 导出后再导入恢复数据", async ({
  page,
}) => {
  const valueName = uniqueName("bifrost-file-value");

  await openPage(page, "values");
  await expect(page.getByTestId("values-list")).toBeVisible();

  await page.getByTestId("value-new-button").click();
  await page
    .getByRole("dialog")
    .getByPlaceholder("Value name (e.g., api_key, auth_token)")
    .fill(valueName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByTestId("value-item").filter({ hasText: valueName }).first()).toBeVisible();

  const downloadPromise = page.waitForEvent("download");
  await page.getByTestId("value-export-all-button").click();
  const download = await downloadPromise;
  const downloadPath = await download.path();
  if (!downloadPath) {
    throw new Error("Expected exported bifrost file to be written to disk");
  }
  const exportedContent = await fs.readFile(downloadPath, "utf8");
  expect(exportedContent).toContain("01 values");
  expect(exportedContent).toContain(valueName);

  const valueItem = page.getByTestId("value-item").filter({ hasText: valueName }).first();
  await valueItem.getByTestId("value-item-menu").click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  await page.getByRole("dialog", { name: "Delete Value" }).getByRole("button", { name: "Delete" }).click();
  await expect(page.getByTestId("value-item").filter({ hasText: valueName })).toHaveCount(0);

  await page
    .getByTestId("value-import-button")
    .locator('input[type="file"]')
    .setInputFiles({
      name: `${valueName}.bifrost`,
      mimeType: "text/plain",
      buffer: Buffer.from(exportedContent, "utf8"),
    });
  await waitForToast(page, "导入成功");
  await expect(page.getByTestId("value-item").filter({ hasText: valueName }).first()).toBeVisible();
});

test("Rules 页面支持持久化排序，且解析顺序符合列表顺序", async ({
  page,
  request,
}) => {
  const ruleName = uniqueName("alpha-rule");
  const latestRuleName = uniqueName("beta-rule");
  const server = await startMockHttpServer();

  try {
    const createRuleRes = await request.post(`${apiBase}/rules`, {
      data: {
        name: ruleName,
        content: "127.0.0.1 reqHeaders://X-UI-Rule=alpha",
      },
    });
    if (!createRuleRes.ok()) {
      throw new Error(await createRuleRes.text());
    }
    const createLatestRuleRes = await request.post(`${apiBase}/rules`, {
      data: {
        name: latestRuleName,
        content: "127.0.0.1 reqHeaders://X-UI-Rule=beta",
      },
    });
    if (!createLatestRuleRes.ok()) {
      throw new Error(await createLatestRuleRes.text());
    }

    await openPage(page, "rules");
    await expect(page.getByTestId("rules-list")).toBeVisible();
    await page.evaluate(() => {
      document.querySelectorAll(".ant-modal-mask, .ant-modal-wrap").forEach((element) => {
        const node = element as HTMLElement;
        node.style.display = "none";
        node.style.pointerEvents = "none";
      });
    });

    const ruleItem = page.getByTestId("rule-item").filter({ hasText: ruleName }).first();
    const latestRuleItem = page
      .getByTestId("rule-item")
      .filter({ hasText: latestRuleName })
      .first();
    await expect(ruleItem).toBeVisible();
    await expect(latestRuleItem).toBeVisible();

    const updateRuleRes = await request.put(`${apiBase}/rules/${encodeURIComponent(ruleName)}`, {
      data: { content: "127.0.0.1 reqHeaders://X-UI-Rule=alpha" },
    });
    if (!updateRuleRes.ok()) {
      throw new Error(await updateRuleRes.text());
    }
    await page.getByTestId("rule-refresh-button").click();

    await expect(page.getByTestId("rule-item").nth(0)).toHaveAttribute(
      "data-rule-name",
      latestRuleName,
    );
    await expect(page.getByTestId("rule-item").nth(1)).toHaveAttribute(
      "data-rule-name",
      ruleName,
    );

    await sendProxyRequest(`http://127.0.0.1:${server.port}/rules-check`);
    await expect.poll(() => server.requests.length).toBeGreaterThan(0);
    expect(server.requests.at(-1)?.headers["x-ui-rule"]).toBe("beta");

    await ruleItem.dragTo(latestRuleItem, {
      targetPosition: { x: 20, y: 4 },
    });

    await expect(page.getByTestId("rule-item").nth(0)).toHaveAttribute(
      "data-rule-name",
      ruleName,
    );
    await expect(page.getByTestId("rule-item").nth(1)).toHaveAttribute(
      "data-rule-name",
      latestRuleName,
    );

    const requestsAfterReorder = server.requests.length;
    await sendProxyRequest(`http://127.0.0.1:${server.port}/rules-reordered`);
    await expect.poll(() => server.requests.length).toBeGreaterThan(requestsAfterReorder);
    expect(server.requests.at(-1)?.headers["x-ui-rule"]).toBe("alpha");

    await openPage(page, "traffic");
    const row = await waitForTrafficRow(page, "/rules-reordered");
    await row.click();
    await expect(page.getByTestId("traffic-detail-header")).toContainText("/rules-reordered");

    await openPage(page, "rules");
    await expect(ruleItem).toBeVisible();
    await ruleItem.locator(".ant-switch").click();

    await page.getByTestId("rule-sort-select").click();
    await page.locator(".ant-select-dropdown").getByText("Name", { exact: true }).click();
    await expect(page.getByTestId("rule-item").first()).toHaveAttribute(
      "data-rule-name",
      ruleName,
    );

    await page.getByTestId("rule-sort-select").click();
    await page.locator(".ant-select-dropdown").getByText("Manual", { exact: true }).click();
    await expect(page.getByTestId("rule-item").first()).toHaveAttribute(
      "data-rule-name",
      ruleName,
    );

    const requestsAfterManualRestore = server.requests.length;
    await sendProxyRequest(`http://127.0.0.1:${server.port}/rules-manual-restored`);
    await expect.poll(() => server.requests.length).toBeGreaterThan(requestsAfterManualRestore);
    expect(server.requests.at(-1)?.headers["x-ui-rule"]).toBe("beta");

    await ruleItem.click({ button: "right" });
    await page.getByRole("menuitem", { name: "Delete" }).click();
    await page.getByRole("dialog", { name: "Delete Rule" }).getByRole("button", { name: "Delete" }).click();
    await expect(
      page.getByTestId("rule-item").filter({ hasText: ruleName }),
    ).toHaveCount(0);
  } finally {
    await server.close();
  }
});

test("Rules 列表在获得焦点后支持上下键切换选中项", async ({
  page,
  request,
}) => {
  const firstRuleName = uniqueName("keyboard-rule-a");
  const secondRuleName = uniqueName("keyboard-rule-b");

  const createFirstRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: firstRuleName,
      content: "127.0.0.1 reqHeaders://X-Keyboard-Rule=first",
    },
  });
  if (!createFirstRuleRes.ok()) {
    throw new Error(await createFirstRuleRes.text());
  }

  const createSecondRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: secondRuleName,
      content: "127.0.0.1 reqHeaders://X-Keyboard-Rule=second",
    },
  });
  if (!createSecondRuleRes.ok()) {
    throw new Error(await createSecondRuleRes.text());
  }

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const firstRuleItem = page.getByTestId("rule-item").nth(0);
  const secondRuleItem = page.getByTestId("rule-item").nth(1);

  await expect(firstRuleItem).toBeVisible();
  await expect(secondRuleItem).toBeVisible();
  await expect(firstRuleItem).toHaveAttribute("data-rule-name", /keyboard-rule-[ab]-/);
  await expect(secondRuleItem).toHaveAttribute("data-rule-name", /keyboard-rule-[ab]-/);

  await firstRuleItem.click();
  await expect(firstRuleItem).toHaveAttribute("aria-selected", "true");

  const rulesListbox = page.getByRole("listbox", { name: "Rules list" });
  await rulesListbox.focus();

  await page.keyboard.press("ArrowDown");
  await expect(secondRuleItem).toHaveAttribute("aria-selected", "true");
  await expect(firstRuleItem).toHaveAttribute("aria-selected", "false");

  await page.keyboard.press("ArrowUp");
  await expect(firstRuleItem).toHaveAttribute("aria-selected", "true");
  await expect(secondRuleItem).toHaveAttribute("aria-selected", "false");
});

test("Values 列表在获得焦点后支持上下键切换选中项", async ({
  page,
  request,
}) => {
  const firstValueName = uniqueName("aaa-keyboard-value-a");
  const secondValueName = uniqueName("aab-keyboard-value-b");

  const createFirstValueRes = await request.post(`${apiBase}/values`, {
    data: {
      name: firstValueName,
      value: "first",
    },
  });
  if (!createFirstValueRes.ok()) {
    throw new Error(await createFirstValueRes.text());
  }

  const createSecondValueRes = await request.post(`${apiBase}/values`, {
    data: {
      name: secondValueName,
      value: "second",
    },
  });
  if (!createSecondValueRes.ok()) {
    throw new Error(await createSecondValueRes.text());
  }

  await openPage(page, "values");
  await expect(page.getByTestId("values-list")).toBeVisible();
  await changeSort(page, "value-sort-select", "Name");

  const firstValueItem = page
    .getByTestId("value-item")
    .filter({ hasText: firstValueName })
    .first();
  const secondValueItem = page
    .getByTestId("value-item")
    .filter({ hasText: secondValueName })
    .first();

  await expect(firstValueItem).toBeVisible();
  await expect(secondValueItem).toBeVisible();
  await expect(firstValueItem).toHaveAttribute("data-value-name", firstValueName);
  await expect(secondValueItem).toHaveAttribute("data-value-name", secondValueName);

  await firstValueItem.click();
  await expect(firstValueItem).toHaveAttribute("aria-selected", "true");

  const valuesListbox = page.getByRole("listbox", { name: "Values list" });
  await valuesListbox.focus();

  await page.keyboard.press("ArrowDown");
  await expect(secondValueItem).toHaveAttribute("aria-selected", "true");
  await expect(firstValueItem).toHaveAttribute("aria-selected", "false");

  await page.keyboard.press("ArrowUp");
  await expect(firstValueItem).toHaveAttribute("aria-selected", "true");
  await expect(secondValueItem).toHaveAttribute("aria-selected", "false");
});
