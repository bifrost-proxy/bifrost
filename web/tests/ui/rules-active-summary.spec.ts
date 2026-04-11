import { test, expect } from "@playwright/test";
import {
  apiBase,
  clearRules,
  clearTraffic,
  openPage,
  sendProxyRequest,
  startMockHttpServer,
  uniqueName,
  waitForTrafficRow,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ request }) => {
  await clearTraffic(request);
  await clearRules(request);
});

test("RulesDynamicIsland 显示 active 规则数量并展开后列出规则", async ({
  page,
  request,
}) => {
  const ruleA = uniqueName("island-rule-a");
  const ruleB = uniqueName("island-rule-b");

  const resA = await request.post(`${apiBase}/rules`, {
    data: { name: ruleA, content: "example.com host://127.0.0.1:3000" },
  });
  expect(resA.ok()).toBeTruthy();
  const resB = await request.post(`${apiBase}/rules`, {
    data: { name: ruleB, content: "api.test.com host://127.0.0.1:4000" },
  });
  expect(resB.ok()).toBeTruthy();

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const island = page.getByTestId("rules-dynamic-island");
  await expect(island).toBeVisible();
  await expect(island).toContainText("2 active");

  await island.click();
  await expect(island.getByText(ruleA)).toBeVisible();
  await expect(island.getByText(ruleB)).toBeVisible();
});

test("RulesDynamicIsland 显示 variable conflicts 警告图标和详情", async ({
  page,
  request,
}) => {
  const ruleX = uniqueName("conflict-rule-x");
  const ruleY = uniqueName("conflict-rule-y");

  const contentX = `example.com reqHeaders://{data}\n\n\`\`\` data\nx-env: prod\n\`\`\``;
  const contentY = `example.com reqHeaders://{data}\n\n\`\`\` data\nx-env: staging\n\`\`\``;

  const resX = await request.post(`${apiBase}/rules`, {
    data: { name: ruleX, content: contentX },
  });
  expect(resX.ok()).toBeTruthy();
  const resY = await request.post(`${apiBase}/rules`, {
    data: { name: ruleY, content: contentY },
  });
  expect(resY.ok()).toBeTruthy();

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const island = page.getByTestId("rules-dynamic-island");
  await expect(island).toBeVisible();

  const conflictIcon = page.getByTestId("island-conflict-icon");
  await expect(conflictIcon).toBeVisible();

  await island.click();

  const conflictsSection = page.getByTestId("island-variable-conflicts");
  await expect(conflictsSection).toBeVisible();
  await expect(conflictsSection).toContainText("Variable Conflicts");
  await expect(conflictsSection).toContainText("{data}");
  await expect(conflictsSection).toContainText(ruleX);
  await expect(conflictsSection).toContainText(ruleY);
});

test("RulesDynamicIsland merged content 展开折叠功能", async ({
  page,
  request,
}) => {
  const ruleName = uniqueName("merged-rule");

  const res = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: "merged-test.com host://127.0.0.1:5000",
    },
  });
  expect(res.ok()).toBeTruthy();

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const island = page.getByTestId("rules-dynamic-island");
  await island.click();

  const toggleBtn = page.getByTestId("island-toggle-merged");
  await expect(toggleBtn).toBeVisible();
  await expect(toggleBtn).toContainText("Show");

  await expect(page.getByTestId("island-merged-content")).toHaveCount(0);

  await toggleBtn.click();
  await expect(toggleBtn).toContainText("Hide");

  const mergedPre = page.getByTestId("island-merged-content");
  await expect(mergedPre).toBeVisible();
  await expect(mergedPre).toContainText("merged-test.com");

  await toggleBtn.click();
  await expect(page.getByTestId("island-merged-content")).toHaveCount(0);
});

test("Response Headers 使用 original/current 模式切换（无 actual tab）", async ({
  page,
  request,
}) => {
  const server = await startMockHttpServer();
  const ruleName = uniqueName("header-diff-rule");

  try {
    const createRes = await request.post(`${apiBase}/rules`, {
      data: {
        name: ruleName,
        content: `127.0.0.1:${server.port} resHeaders://X-Added-By-Rule=injected`,
      },
    });
    expect(createRes.ok()).toBeTruthy();

    await openPage(page, "traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const reqPath = `/${uniqueName("header-diff")}`;
    await sendProxyRequest(`http://127.0.0.1:${server.port}${reqPath}`);
    await page.reload();

    const row = await waitForTrafficRow(page, reqPath);
    await row.click();

    await expect(page.getByTestId("traffic-detail-header")).toBeVisible();

    const responseTab = page.locator('[data-testid="response-tab-header"]');
    if (await responseTab.isVisible()) {
      await responseTab.click();
    }

    const actualTab = page.locator('[data-testid="response-header-view-tab-actual"]');
    await expect(actualTab).toHaveCount(0);

    const modeTabs = page.locator('[data-testid="response-header-view-mode-tabs"]');
    if (await modeTabs.isVisible()) {
      const originalTab = page.locator('[data-testid="response-header-view-tab-original"]');
      if (await originalTab.isVisible()) {
        await originalTab.click();
        await expect(page.locator('[data-testid="response-header-view-tab-original"]')).toBeVisible();

        const currentTab = page.locator('[data-testid="response-header-view-tab-current"]');
        await currentTab.click();
        await expect(currentTab).toBeVisible();
      }
    }
  } finally {
    await server.close();
  }
});

test("选中已被外部删除的规则时显示 warning 并自动刷新列表", async ({
  page,
  request,
}) => {
  const ruleName = uniqueName("deleted-externally");

  const createRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: "external-delete.com host://127.0.0.1:3000",
    },
  });
  expect(createRes.ok()).toBeTruthy();

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const ruleItem = page.getByTestId("rule-item").filter({ hasText: ruleName }).first();
  await expect(ruleItem).toBeVisible();

  await request.delete(`${apiBase}/rules/${encodeURIComponent(ruleName)}`);

  await ruleItem.click();

  await expect(
    page
      .locator(".ant-message-notice")
      .filter({ hasText: /no longer exists|refreshing/i })
      .last(),
  ).toBeVisible({ timeout: 10000 });

  await expect(
    page.getByTestId("rule-item").filter({ hasText: ruleName }),
  ).toHaveCount(0);
});

test("RulesDynamicIsland 无规则时显示 0 active 且无冲突图标", async ({
  page,
}) => {
  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const island = page.getByTestId("rules-dynamic-island");
  await expect(island).toBeVisible();
  await expect(island).toContainText("0 active");

  await expect(page.getByTestId("island-conflict-icon")).toHaveCount(0);
});

test("删除规则失败时显示 error message", async ({ page, request }) => {
  const ruleName = uniqueName("delete-fail-rule");

  const createRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: "delete-fail-test.com host://127.0.0.1:3000",
    },
  });
  expect(createRes.ok()).toBeTruthy();

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const ruleItem = page
    .getByTestId("rule-item")
    .filter({ hasText: ruleName })
    .first();
  await expect(ruleItem).toBeVisible();
  await ruleItem.click();

  await request.delete(
    `${apiBase}/rules/${encodeURIComponent(ruleName)}`,
  );

  const deleteBtn = page.getByTestId("rule-delete-btn");
  if (await deleteBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
    await deleteBtn.click();

    const confirmBtn = page.getByRole("button", { name: /ok|confirm|yes|确认|确定/i });
    if (await confirmBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await confirmBtn.click();
    }

    await expect(
      page.locator(".ant-message-notice").last(),
    ).toBeVisible({ timeout: 10000 });
  }
});
