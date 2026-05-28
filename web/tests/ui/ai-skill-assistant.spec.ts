import { test, expect } from "@playwright/test";
import { openPage, waitForToast } from "./helpers/admin-helpers";

test("AI skill assistant opens from the status bar next to version", async ({
  page,
}) => {
  await openPage(page, "traffic");

  await expect(page.getByTestId("ai-skill-assistant-launcher")).toHaveCount(0);
  const trigger = page.getByTestId("ai-skill-assistant-trigger");
  const version = page.getByTestId("statusbar-version-button");
  await expect(trigger).toBeVisible();
  await expect(trigger).toContainText("Skill");
  await expect(version).toBeVisible();

  await expect
    .poll(async () => {
      const triggerBox = await trigger.boundingBox();
      const versionBox = await version.boundingBox();
      if (!triggerBox || !versionBox) return false;
      return (
        Math.abs(triggerBox.y - versionBox.y) < 4 &&
        triggerBox.x > versionBox.x
      );
    })
    .toBe(true);

  await trigger.click();
  const panel = page.getByTestId("ai-skill-assistant-panel");
  await expect(panel).toBeVisible();
  await expect(panel).toContainText("bifrost install-skill -y");
  await expect(panel).toContainText("通过 AI 操作规则增删改查");
  await expect(panel).toContainText("流量搜索和问题排查");
  await expect(panel).toContainText("多端口独立规则");
  await expect(panel).not.toContainText("拖拽气泡");

  await expect
    .poll(async () => {
      const triggerBox = await trigger.boundingBox();
      const panelBox = await panel.boundingBox();
      if (!triggerBox || !panelBox) return false;
      return panelBox.y + panelBox.height <= triggerBox.y + 2;
    })
    .toBe(true);

  const link = page.getByTestId("ai-skill-assistant-skill-link");
  await expect(link).toHaveAttribute(
    "href",
    "https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md",
  );

  await page.getByTestId("ai-skill-assistant-copy").click();
  await waitForToast(page, "Skill install command copied");

  await trigger.click();
  await expect(panel).toBeHidden();
});

test("AI skill assistant status bar popover remains readable in dark theme", async ({
  page,
}) => {
  await openPage(page, "rules");
  await page.getByTestId("theme-toggle").click();

  await page.getByTestId("ai-skill-assistant-trigger").click();
  const panel = page.getByTestId("ai-skill-assistant-panel");
  await expect(panel).toBeVisible();
  await expect(panel).toContainText("AI Skill 加速 Bifrost 操作");
  await expect(panel).toContainText("SKILL.md");
});
