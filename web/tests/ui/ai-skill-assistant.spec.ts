import { test, expect } from "@playwright/test";
import { openPage, waitForToast } from "./helpers/admin-helpers";

test("AI skill assistant supports hover content, copy, drag, link, and click hide", async ({
  page,
}) => {
  await openPage(page, "traffic");

  const assistant = page.getByTestId("ai-skill-assistant");
  const launcher = page.getByTestId("ai-skill-assistant-launcher");
  await expect(assistant).toBeVisible();

  await launcher.hover();
  const panel = page.getByTestId("ai-skill-assistant-panel");
  await expect(panel).toBeVisible();
  await expect(panel).toContainText("bifrost install-skill -y");
  await expect(panel).toContainText("通过 AI 操作规则增删改查");
  await expect(panel).toContainText("流量搜索和问题排查");
  await expect(panel).toContainText("多端口独立规则");

  const link = page.getByTestId("ai-skill-assistant-skill-link");
  await expect(link).toHaveAttribute(
    "href",
    "https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md",
  );

  const initialLauncherBox = await launcher.boundingBox();
  const initialPanelBox = await panel.boundingBox();
  if (!initialLauncherBox || !initialPanelBox) {
    throw new Error("AI skill assistant launcher and panel should be visible");
  }
  await page.mouse.move(
    initialLauncherBox.x + initialLauncherBox.width / 2,
    initialLauncherBox.y - 5,
  );
  await page.waitForTimeout(220);
  await expect(panel).toBeVisible();
  await page.mouse.move(
    initialPanelBox.x + initialPanelBox.width / 2,
    initialPanelBox.y + initialPanelBox.height / 2,
  );
  await expect(panel).toBeVisible();

  await page.getByTestId("ai-skill-assistant-copy").click();
  await waitForToast(page, "Skill install command copied");

  const before = await launcher.boundingBox();
  if (!before) {
    throw new Error("AI skill assistant launcher is not visible before drag");
  }
  await page.mouse.move(before.x + before.width / 2, before.y + before.height / 2);
  await page.mouse.down();
  await page.mouse.move(before.x - 118, before.y - 82, { steps: 6 });
  await page.mouse.up();

  const after = await launcher.boundingBox();
  if (!after) {
    throw new Error("AI skill assistant launcher is not visible after drag");
  }
  expect(Math.abs(after.x - before.x)).toBeGreaterThan(40);
  expect(Math.abs(after.y - before.y)).toBeGreaterThan(40);

  await launcher.click();
  await expect(assistant).toBeHidden();
});

test("AI skill assistant remains readable in dark theme", async ({ page }) => {
  await openPage(page, "rules");
  await page.getByTestId("theme-toggle").click();

  const launcher = page.getByTestId("ai-skill-assistant-launcher");
  await launcher.hover();
  const panel = page.getByTestId("ai-skill-assistant-panel");
  await expect(panel).toBeVisible();
  await expect(panel).toContainText("AI Skill 加速 Bifrost 操作");
  await expect(panel).toContainText("SKILL.md");
});
