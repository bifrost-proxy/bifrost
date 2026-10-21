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
  await expect(panel).toContainText("Create, read, update, and delete rules with AI");
  await expect(panel).toContainText("Traffic search and troubleshooting");
  await expect(panel).toContainText("Independent rules per port");
  await expect(panel).not.toContainText("drag the bubble");

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
  await expect(panel).toContainText("Speed up Bifrost with an AI skill");
  await expect(panel).toContainText("SKILL.md");
});
