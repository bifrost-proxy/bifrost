import { test, expect } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

test("Settings Agent MCP Servers 自动检查并展示可用性差异", async ({
  page,
}) => {
  let statusRequests = 0;
  const agentConfig = {
    enabled: true,
    work_dir: "/tmp/agent-ui",
    model_providers: {},
    mcp_servers: {
      filesystem: {
        enabled: true,
        command: "mock-mcp",
      },
      broken: {
        enabled: true,
        command: "missing-mcp",
      },
      disabled: {
        enabled: false,
        command: "disabled-mcp",
      },
    },
  };

  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(agentConfig),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/providers", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/mcp-status", async (route) => {
    statusRequests += 1;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        servers: [
          {
            name: "filesystem",
            enabled: true,
            status: "available",
            tool_count: 3,
          },
          {
            name: "broken",
            enabled: true,
            status: "unavailable",
            tool_count: 0,
            error: "spawn MCP 'broken' (missing-mcp): command not found",
          },
          {
            name: "disabled",
            enabled: false,
            status: "disabled",
            tool_count: 0,
          },
        ],
      }),
    });
  });

  await openPage(page, "ai?aiSection=agent-mcp-servers&agentSection=mcp-servers");

  await expect(page.getByTestId("agent-settings-section-mcp-servers")).toBeVisible();
  await expect
    .poll(() => statusRequests)
    .toBeGreaterThanOrEqual(1);
  await expect(page.getByTestId("settings-agent-mcp-status-filesystem")).toContainText(
    "Available",
  );
  await expect(page.getByTestId("settings-agent-mcp-server-filesystem")).toContainText(
    "Tools: 3",
  );
  await expect(page.getByTestId("settings-agent-mcp-status-broken")).toContainText(
    "Unavailable",
  );
  await expect(page.getByTestId("settings-agent-mcp-server-broken")).toContainText(
    "command not found",
  );
  await expect(page.getByTestId("settings-agent-mcp-status-disabled")).toContainText(
    "Disabled",
  );

  await page.getByTestId("settings-agent-mcp-refresh").click();
  await expect
    .poll(() => statusRequests)
    .toBeGreaterThanOrEqual(2);

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByTestId("settings-agent-mcp-status-filesystem")).toContainText(
    "Available",
  );
  await expect(page.getByTestId("settings-agent-mcp-status-broken")).toContainText(
    "Unavailable",
  );
});
