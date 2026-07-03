import fs from "node:fs/promises";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { test, expect } from "@playwright/test";
import {
  apiBase,
  clearRules,
  clearScripts,
  clearTraffic,
  clearValues,
  openPage,
  sendProxyRequest,
  setMonacoEditor,
  startMockHttpServer,
  uniqueName,
  waitForToast,
  waitForTrafficRow,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

async function changeSort(page: import("@playwright/test").Page, testId: string, label: string) {
  await page.getByTestId(testId).click();
  await page.locator(".ant-select-dropdown").getByText(label, { exact: true }).click();
}

async function writeGroupRuleFile(groupName: string, ruleName: string, content: string) {
  const dataDir = process.env.BIFROST_DATA_DIR;
  if (!dataDir) {
    throw new Error("BIFROST_DATA_DIR is required to seed group rule references");
  }
  const now = new Date().toISOString();
  const rulesDir = path.join(dataDir, "rules");
  const groupDir = path.join(rulesDir, groupName);
  await fs.mkdir(groupDir, { recursive: true });
  await fs.writeFile(
    path.join(groupDir, `${ruleName}.bifrost`),
    [
      "01 rules",
      "",
      "[meta]",
      `name = "${ruleName}"`,
      "enabled = false",
      "sort_order = 0",
      'version = "1.0.0"',
      `created_at = "${now}"`,
      `updated_at = "${now}"`,
      `group = "${groupName}"`,
      "",
      "[meta.sync]",
      `rule_id = "${randomUUID()}"`,
      'status = "local_only"',
      "",
      "[options]",
      "rule_count = 1",
      "",
      "---",
      content,
    ].join("\n"),
    "utf8",
  );
  await fs.writeFile(
    path.join(rulesDir, ".group_cache.json"),
    JSON.stringify({ [`gid-${groupName}`]: groupName }),
    "utf8",
  );
}

test.beforeEach(async ({ request }) => {
  await clearTraffic(request);
  await clearRules(request);
  await clearScripts(request);
  await clearValues(request);
});

test("Rules 页面置顶并保护全局 Default 规则，同时允许编辑内容", async ({
  page,
  request,
}) => {
  const secondaryRuleName = uniqueName("default-switch-target");
  const createSecondaryRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: secondaryRuleName,
      content: "switch-target.test status://219",
      enabled: true,
    },
  });
  if (!createSecondaryRes.ok()) {
    throw new Error(await createSecondaryRes.text());
  }

  await openPage(page, "rules?rule=Default");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const defaultItem = page.getByTestId("rule-item").first();
  await expect(defaultItem).toHaveAttribute("data-rule-name", "Default");
  await expect(defaultItem.locator(".ant-switch")).toHaveClass(/ant-switch-disabled/);
  await defaultItem.getByText("Default", { exact: true }).hover();
  await expect(page.getByText("Default applies globally with the highest priority")).toBeVisible();
  await expect(page.getByText("clear its content if you do not need global rules")).toBeVisible();

  await defaultItem.click();
  await expect(page.getByTestId("rule-editor-title")).toHaveText("Default");
  await expect(page.getByTestId("rule-editor")).toBeVisible();
  await expect(page.getByTestId("rule-editor-meta")).toContainText("Global default");
  await expect(page.getByTestId("rule-delete-button")).toHaveCount(0);

  await defaultItem.click({ button: "right" });
  await expect(page.getByRole("menuitem")).toHaveCount(0);
  await page.keyboard.press("Escape");

  const content = `# Default UI test\n${uniqueName("default-ui")}.test status://218`;
  const editorInput = page
    .getByTestId("rule-editor-container")
    .getByRole("textbox", { name: "Editor content" });
  await editorInput.click({ force: true });
  await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
  await page.keyboard.press("Backspace");
  await page.keyboard.insertText(content);
  const saveButton = page.getByTestId("rule-save-button");
  await expect(saveButton).toBeEnabled();
  await saveButton.click();
  await waitForToast(page, "Saved");

  const response = await request.get(`${apiBase}/rules/Default`);
  expect(response.ok()).toBeTruthy();
  const detail = (await response.json()) as {
    enabled: boolean;
    is_global_default?: boolean;
    can_delete?: boolean;
    can_disable?: boolean;
    content: string;
  };
  expect(detail.enabled).toBeTruthy();
  expect(detail.is_global_default).toBeTruthy();
  expect(detail.can_delete).toBeFalsy();
  expect(detail.can_disable).toBeFalsy();
  expect(detail.content).toContain("Default UI test");

  const secondaryItem = page.locator(`[data-testid="rule-item"][data-rule-name="${secondaryRuleName}"]`);
  await secondaryItem.click();
  await expect(page.getByTestId("rule-editor-title")).toHaveText(secondaryRuleName);
  await expect(page).toHaveURL(new RegExp(`rule=${secondaryRuleName}`));
  await expect(defaultItem).toHaveAttribute("aria-selected", "false");
  await expect(secondaryItem).toHaveAttribute("aria-selected", "true");
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
  await page.getByPlaceholder("Rule name").fill(uniqueName("syntax-empty-rule"));
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByTestId("rule-editor")).toBeVisible();
});

test("Rules 编辑器 bp 补全使用 parser scripts，decode bp 校验不报缺失脚本", async ({
  page,
  request,
}) => {
  const parserScriptName = uniqueName("bp-parser-hint");
  const createParserRes = await request.put(
    `${apiBase}/scripts/parser/${encodeURIComponent(parserScriptName)}`,
    {
      data: { content: 'ctx.output = { code: "0", data: "parser-hint", msg: "" };' },
    },
  );
  if (!createParserRes.ok()) {
    throw new Error(await createParserRes.text());
  }

  const validateRes = await request.post(`${apiBase}/rules/validate`, {
    data: {
      content: [
        `bp-local.test bp://${parserScriptName} decode://bp`,
        "bp-remote.test bp://https://example.com/parser.js?sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef decode://bp",
        "bp-file.test bp://file:///tmp/parser.js decode://bp",
        "bp-abs.test bp:///tmp/parser.js decode://bp",
      ].join("\n"),
    },
  });
  expect(validateRes.ok()).toBeTruthy();
  const validatePayload = (await validateRes.json()) as {
    warnings?: Array<{ message: string }>;
  };
  const warningText = (validatePayload.warnings || [])
    .map((warning) => warning.message)
    .join("\n");
  expect(warningText).not.toContain("Script 'bp' not found");
  expect(warningText).not.toContain("parser.js");

  const ruleName = uniqueName("bp-hint-rule");
  const createRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: "bp-hint.test bp://https://example.com/parser.js?sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef decode://bp",
      enabled: true,
    },
  });
  if (!createRuleRes.ok()) {
    throw new Error(await createRuleRes.text());
  }

  await openPage(page, "rules");
  await page.getByTestId("rule-item").filter({ hasText: ruleName }).first().click();
  await expect(page.getByTestId("rule-editor")).toBeVisible();

  await page.evaluate((value) => {
    const monaco = (window as unknown as { monaco?: typeof import("monaco-editor") }).monaco;
    const models = monaco?.editor.getModels() ?? [];
    const model =
      models.find((candidate) => candidate.getValue().includes("bp-hint.test")) ?? models[0];
    model?.setValue(value);
  }, "bp-hint.test bp://");
  const editorInput = page
    .getByTestId("rule-editor-container")
    .getByRole("textbox", { name: "Editor content" });
  await editorInput.click({ force: true });
  await page.keyboard.press(process.platform === "darwin" ? "Meta+ArrowRight" : "End");
  await page.keyboard.press("Control+Space");
  await expect(
    page.locator(".suggest-widget .monaco-list-row").filter({
      hasText: `${parserScriptName} decode://bp`,
    }).first(),
  ).toBeVisible();

  await request.put(`${apiBase}/scripts/parser/${encodeURIComponent("build_in_bp")}`, {
    data: { content: 'ctx.output = { code: "0", data: "built-in-nav", msg: "" };' },
  });
  const navRuleName = uniqueName("bp-nav-rule");
  const createNavRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: navRuleName,
      content: "bp-nav.test bp://build_in_bp?protocol=thrift decode://bp",
      enabled: true,
    },
  });
  if (!createNavRuleRes.ok()) {
    throw new Error(await createNavRuleRes.text());
  }
  await page.goto("/_bifrost/rules");
  await page.getByTestId("rule-item").filter({ hasText: navRuleName }).first().click();
  await expect(page.getByTestId("rule-editor")).toBeVisible();
  const editorBox = page.getByTestId("rule-editor-container").locator(".monaco-editor").first();
  await expect(editorBox).toBeVisible();
  await expect(page.locator(".view-line").filter({ hasText: "bp://build_in_bp" }).first()).toBeVisible();
  const box = await editorBox.boundingBox();
  if (!box) {
    throw new Error("Rule editor is not visible");
  }
  await editorBox.click({
    position: { x: 220, y: 18 },
    modifiers: [process.platform === "darwin" ? "Meta" : "Control"],
  });
  await expect(page).toHaveURL(/\/_bifrost\/scripts/);
  await expect(page.getByTestId("scripts-editor-panel")).toContainText("build_in_bp");
});

test("Rules 支持 @规则引用解析，并可在编辑器中点击展开详情", async ({
  page,
  request,
}) => {
  const sharedRuleName = uniqueName("at-shared");
  const commentedRuleName = uniqueName("commented-shared");
  const entryRuleName = uniqueName("at-entry");
  const missingRuleName = uniqueName("at-missing");
  const missingReferenceName = uniqueName("at-unknown");
  const groupName = uniqueName("at-team");
  const groupRuleName = uniqueName("at-group-shared");
  const groupRuleReference = `${groupName}/${groupRuleName}`;
  const sharedContent = "at-shared.test reqHeaders://X-At-Rule=ok";
  const commentedContent = "commented-ui.test statusCode://209";
  const groupSharedContent = "at-shared.test reqHeaders://X-Group-At-Rule=ok";
  const commentReferenceName = uniqueName("at-comment-only");
  const entryContent = [
    `@${sharedRuleName}\t# tab comment should still resolve`,
    `@${commentedRuleName}`,
    `@${groupRuleReference}`,
    `# @${commentReferenceName} should stay a comment`,
    "at-entry.test statusCode://204",
  ].join("\n");
  const missingRuleContent = `@${missingReferenceName}\nmissing-ui.test statusCode://204`;

  await writeGroupRuleFile(groupName, groupRuleName, groupSharedContent);

  const createSharedRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: sharedRuleName,
      content: sharedContent,
      enabled: false,
    },
  });
  if (!createSharedRes.ok()) {
    throw new Error(await createSharedRes.text());
  }

  const createCommentedRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: commentedRuleName,
      content: commentedContent,
      enabled: false,
    },
  });
  if (!createCommentedRes.ok()) {
    throw new Error(await createCommentedRes.text());
  }

  const createEntryRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: entryRuleName,
      content: entryContent,
      enabled: true,
    },
  });
  if (!createEntryRes.ok()) {
    throw new Error(await createEntryRes.text());
  }

  const validRes = await request.post(`${apiBase}/rules/validate`, {
    data: {
      current_rule_name: entryRuleName,
      content: entryContent,
    },
  });
  expect(validRes.ok()).toBeTruthy();
  const validPayload = (await validRes.json()) as {
    valid: boolean;
    rule_count: number;
    errors: Array<{ message: string; code?: string }>;
  };
  expect(validPayload.valid).toBeTruthy();
  expect(validPayload.rule_count).toBe(4);
  expect(validPayload.errors).toHaveLength(0);

  const candidatesRes = await request.get(`${apiBase}/rules/reference-candidates`);
  expect(candidatesRes.ok()).toBeTruthy();
  const candidatesPayload = (await candidatesRes.json()) as Array<{
    name: string;
    rule_name: string;
    group_name?: string | null;
  }>;
  expect(candidatesPayload.some((candidate) => candidate.name === sharedRuleName)).toBeTruthy();
  expect(candidatesPayload.some((candidate) => candidate.name === commentedRuleName)).toBeTruthy();
  expect(
    candidatesPayload.some(
      (candidate) =>
        candidate.name === groupRuleReference &&
        candidate.rule_name === groupRuleName &&
        candidate.group_name === groupName,
    ),
  ).toBeTruthy();

  const missingRes = await request.post(`${apiBase}/rules/validate`, {
    data: {
      current_rule_name: entryRuleName,
      content: "@missing-rule",
    },
  });
  expect(missingRes.ok()).toBeTruthy();
  const missingPayload = (await missingRes.json()) as {
    valid: boolean;
    errors: Array<{ message: string; code?: string }>;
  };
  expect(missingPayload.valid).toBeFalsy();
  expect(missingPayload.errors[0]?.code).toBe("E020");
  expect(missingPayload.errors[0]?.message).toContain("missing-rule");

  const createMissingRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: missingRuleName,
      content: missingRuleContent,
      enabled: false,
    },
  });
  expect(createMissingRes.ok()).toBeFalsy();
  const createMissingPayload = (await createMissingRes.json()) as {
    saved?: boolean;
    syntax?: { errors?: Array<{ code?: string; message?: string }> };
  };
  expect(createMissingPayload.saved).toBe(false);
  expect(createMissingPayload.syntax?.errors?.[0]?.code).toBe("E020");

  await openPage(page, "rules");
  await page.getByTestId("rule-item").filter({ hasText: entryRuleName }).first().click();
  await expect(page.getByTestId("rule-editor")).toBeVisible();
  const editorBox = page.getByTestId("rule-editor-container").locator(".monaco-editor").first();
  await expect(editorBox).toBeVisible();
  const referenceLine = page.locator(".view-line").filter({ hasText: `@${sharedRuleName}` }).first();
  await expect(referenceLine).toBeVisible();
  const commentReferenceLine = page
    .locator(".view-line")
    .filter({ hasText: `# @${commentReferenceName}` })
    .first();
  await expect(commentReferenceLine).toBeVisible();
  await expect
    .poll(async () =>
      commentReferenceLine.evaluate((element) => {
        const nodes = [element, ...Array.from(element.querySelectorAll("*"))];
        return nodes.some((node) =>
          node.className.toString().includes("ruleReferenceDecoration"),
        );
      }),
    )
    .toBeFalsy();
  const commentBox = await commentReferenceLine.boundingBox();
  if (!commentBox) {
    throw new Error("Comment reference line is not visible");
  }
  await page.mouse.click(commentBox.x + 28, commentBox.y + commentBox.height / 2);
  await expect(page.getByTestId("rule-reference-zone")).toBeHidden();

  const box = await referenceLine.boundingBox();
  if (!box) {
    throw new Error("Rule reference line is not visible");
  }
  await page.mouse.click(box.x + 24, box.y + box.height / 2);

  const zone = page.getByTestId("rule-reference-zone");
  await expect(zone).toBeVisible();
  await expect(zone).toHaveAttribute("data-rule-reference-name", sharedRuleName);
  await expect(zone).toContainText(sharedContent);

  await page.mouse.click(box.x + 24, box.y + box.height / 2);
  await expect(zone).toBeHidden();

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.mouse.click(box.x + 24, box.y + box.height / 2);
  await expect(zone).toBeVisible();
  await expect(zone).toContainText(sharedContent);
  const darkZoneColors = await zone.evaluate((element) => {
    const style = window.getComputedStyle(element);
    return {
      background: style.backgroundColor,
      color: style.color,
      border: style.borderColor,
    };
  });
  expect(darkZoneColors.background).not.toBe(darkZoneColors.color);
  expect(darkZoneColors.border).not.toBe("rgba(0, 0, 0, 0)");

  const groupReferenceLine = page.locator(".view-line").filter({ hasText: `@${groupRuleReference}` }).first();
  await expect(groupReferenceLine).toBeVisible();
  await groupReferenceLine.click({ position: { x: 24, y: 10 }, force: true });
  await expect(zone).toBeVisible();
  await expect(zone).toHaveAttribute("data-rule-reference-name", groupRuleReference);
  await expect(zone).toContainText(groupSharedContent);

  await page.getByTestId("rule-item").filter({ hasText: entryRuleName }).first().click();
  await expect(page.locator(".view-line").filter({ hasText: `@${sharedRuleName}` }).first()).toBeVisible();

  const editorInput = page
    .getByTestId("rule-editor-container")
    .getByRole("textbox", { name: "Editor content" });
  await editorInput.click({ force: true });
  await page.keyboard.press(process.platform === "darwin" ? "Meta+ArrowRight" : "End");
  await page.keyboard.press("Enter");
  await page.keyboard.type(`@${groupName.slice(0, 2)}${groupRuleName.slice(-2)}`);
  await page.keyboard.press("Control+Space");
  await expect(
    page.locator(".suggest-widget .monaco-list-row").filter({
      hasText: `@${groupRuleReference}`,
    }).first(),
  ).toBeVisible();
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

  await page.locator('input[type="file"]').last().setInputFiles({
    name: `${valueName}.bifrost`,
    mimeType: "text/plain",
    buffer: Buffer.from(exportedContent, "utf8"),
  });
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
    const resetUiConfigRes = await request.put(`${apiBase}/config/ui`, {
      data: { rulesSortMode: "manual" },
    });
    if (!resetUiConfigRes.ok()) {
      throw new Error(await resetUiConfigRes.text());
    }

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

    await changeSort(page, "rule-sort-select", "Updated");
    await expect(page.getByTestId("rule-item").first()).toHaveAttribute(
      "data-rule-name",
      ruleName,
    );
    await page.reload();
    await expect(page.getByTestId("rules-list")).toBeVisible();
    await expect(page.getByTestId("rule-sort-select")).toContainText("Updated");
    await expect(page.getByTestId("rule-item").first()).toHaveAttribute(
      "data-rule-name",
      ruleName,
    );
    await changeSort(page, "rule-sort-select", "Manual");

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
    await page.reload();
    await expect(page.getByTestId("rules-list")).toBeVisible();
    await expect(page.getByTestId("rule-sort-select")).toContainText("Name");
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

test("Rules 页面真实变更后可保存且保存后禁用按钮", async ({
  page,
  request,
}) => {
  const ruleName = uniqueName("undo-save-rule");
  const originalContent = "example.com host://127.0.0.1:3000";

  const createRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: originalContent,
    },
  });
  if (!createRuleRes.ok()) {
    throw new Error(await createRuleRes.text());
  }

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const ruleItem = page.getByTestId("rule-item").filter({ hasText: ruleName }).first();
  await ruleItem.click();
  await expect(page.getByTestId("rule-editor-title")).toHaveText(ruleName);

  const saveButton = page.getByTestId("rule-save-button");
  await setMonacoEditor(
    page,
    page.getByTestId("rule-editor-container"),
    `${originalContent}\n# saved change`,
  );
  await expect(saveButton).toBeEnabled();

  await saveButton.click();
  await waitForToast(page, "Saved");
  await expect(saveButton).toBeDisabled();
});

test("Rules Dynamic Island 展开的 Merged Rules 支持一键复制", async ({
  page,
  context,
  request,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const ruleName = uniqueName("merged-copy-rule");
  const ruleContent = [
    "# merged copy check",
    "example.com reqHeaders://{copy_headers}",
    "```copy_headers",
    "x-merged-copy: ok",
    "```",
  ].join("\n");

  const createRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: ruleContent,
      enabled: true,
    },
  });
  if (!createRuleRes.ok()) {
    throw new Error(await createRuleRes.text());
  }

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  await page.getByTestId("rules-dynamic-island-trigger").click();
  await page.getByTestId("rules-dynamic-island-merged-toggle").click();

  const mergedContent = page.getByTestId("rules-dynamic-island-merged-content");
  await expect(mergedContent).toContainText("x-merged-copy: ok");

  const shownMergedRules = (await mergedContent.textContent())?.trim();
  await page.getByTestId("rules-dynamic-island-copy-merged").click();
  await waitForToast(page, "Merged rules copied");

  const clipboardText = await page.evaluate(async () => navigator.clipboard.readText());
  expect(clipboardText).toBe(shownMergedRules);
});

test("Rules 列表支持按 / 分组的树状展开/折叠", async ({
  page,
  request,
}) => {
  const folderName = uniqueName("tree-folder");
  const firstRuleName = `${folderName}/a-child`;
  const secondRuleName = `${folderName}/b-child`;
  const topRuleName = uniqueName("tree-top");

  for (const name of [firstRuleName, secondRuleName, topRuleName]) {
    const createRes = await request.post(`${apiBase}/rules`, {
      data: {
        name,
        content: "127.0.0.1 reqHeaders://X-Tree-Rule=ok",
      },
    });
    if (!createRes.ok()) {
      throw new Error(await createRes.text());
    }
  }

  await openPage(page, "rules");
  await expect(page.getByTestId("rules-list")).toBeVisible();

  const folderRow = page.getByTestId("rule-folder-item").filter({ hasText: folderName }).first();
  await expect(folderRow).toBeVisible();
  if ((await folderRow.getAttribute("data-folder-expanded")) !== "true") {
    await folderRow.click();
  }
  await expect(folderRow).toHaveAttribute("data-folder-expanded", "true");

  const firstLeaf = page.locator(`[data-rule-name="${firstRuleName}"]`);
  const secondLeaf = page.locator(`[data-rule-name="${secondRuleName}"]`);
  await expect(firstLeaf).toBeVisible();
  await expect(secondLeaf).toBeVisible();

  await folderRow.click();
  await expect(firstLeaf).toHaveCount(0);
  await expect(secondLeaf).toHaveCount(0);

  await folderRow.click();
  await expect(firstLeaf).toBeVisible();
  await expect(secondLeaf).toBeVisible();
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
