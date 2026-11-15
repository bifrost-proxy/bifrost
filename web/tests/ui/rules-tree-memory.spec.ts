import { expect, test } from "@playwright/test";
import { apiBase, clearRules, openPage } from "./helpers/admin-helpers";

for (const colorScheme of ["light", "dark"] as const) {
  test(`Rules folder memory survives refresh and navigation (${colorScheme})`, async ({
    page,
    request,
  }, testInfo) => {
    await clearRules(request);
    for (const name of ["team/service/child", "team/sibling", "other/child"]) {
      const response = await request.post(`${apiBase}/rules`, {
        data: { name, content: "# folder memory regression" },
      });
      expect(response.ok(), await response.text()).toBeTruthy();
    }
    await page.emulateMedia({ colorScheme });
    await openPage(page, "rules?rule=Default");
    await expect(page.locator("html")).toHaveAttribute(
      "data-theme",
      colorScheme,
    );

    const team = page.locator('[data-folder-path="team"]');
    const service = page.locator('[data-folder-path="team/service"]');
    const other = page.locator('[data-folder-path="other"]');
    const child = page.locator('[data-rule-name="team/service/child"]');
    await expect(child).toBeVisible();
    await service.click();
    await expect(child).toHaveCount(0);
    await team.click();
    await other.click();
    await expect(team).toHaveAttribute("data-folder-expanded", "false");
    await expect(other).toHaveAttribute("data-folder-expanded", "false");

    await page.getByTestId("rule-refresh-button").click();
    await expect(team).toHaveAttribute("data-folder-expanded", "false");
    await page.reload();
    await expect(team).toHaveAttribute("data-folder-expanded", "false");
    await expect(other).toHaveAttribute("data-folder-expanded", "false");
    await page
      .getByTestId("app-sidebar-nav-item")
      .filter({ hasText: "Values" })
      .click();
    await expect(page.getByTestId("rules-list")).toHaveCount(0);
    await page
      .getByTestId("app-sidebar-nav-item")
      .filter({ hasText: "Rules" })
      .click();
    await expect(team).toHaveAttribute("data-folder-expanded", "false");

    // Reopening the parent must preserve its child's independent preference
    await team.click();
    await expect(service).toHaveAttribute("data-folder-expanded", "false");
    await page.reload();
    await expect(team).toHaveAttribute("data-folder-expanded", "true");
    await expect(service).toHaveAttribute("data-folder-expanded", "false");

    const search = page.getByTestId("rule-search-input");
    await search.fill("team/service/child");
    await expect(child).toBeVisible();
    await service.click();
    await search.clear();
    await expect(service).toHaveAttribute("data-folder-expanded", "false");
    await expect(other).toHaveAttribute("data-folder-expanded", "false");

    const response = await request.post(`${apiBase}/rules`, {
      data: { name: "new-folder/child", content: "# new folder" },
    });
    expect(response.ok(), await response.text()).toBeTruthy();
    await page.getByTestId("rule-refresh-button").click();
    await expect(
      page.locator('[data-rule-name="new-folder/child"]'),
    ).toBeVisible();
    await expect(service).toHaveAttribute("data-folder-expanded", "false");

    // Deep links reveal the selected rule without overwriting the saved state
    await openPage(
      page,
      `rules?rule=${encodeURIComponent("team/service/child")}`,
    );
    await expect(child).toBeVisible();
    await expect(page.getByTestId("rule-editor-title")).toHaveText(
      "team/service/child",
    );
    await service.click();
    await page.locator('[data-rule-name="Default"]').click();
    await expect(service).toHaveAttribute("data-folder-expanded", "false");
    await service.click();
    await page.reload();
    await expect(child).toBeVisible();
    await expect(other).toHaveAttribute("data-folder-expanded", "false");
    await page.screenshot({
      path: testInfo.outputPath(`folders-${colorScheme}.png`),
    });
  });
}
