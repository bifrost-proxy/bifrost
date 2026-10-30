import { defineConfig } from "@playwright/test";
// Build the current CLI (including frontend assets) before running this suite.
export default defineConfig({
  testDir: "./tests/ui",
  testMatch: "search-session-live.spec.ts",
  workers: 1,
  timeout: 300_000,
  use: {
    actionTimeout: 15_000,
    navigationTimeout: 20_000,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    baseURL: "http://127.0.0.1:8848",
    launchOptions: {
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
    },
    storageState: {
      cookies: [],
      origins: [
        {
          origin: "http://127.0.0.1:8848",
          localStorage: [
            {
              name: "bifrost-locale",
              value: JSON.stringify({ state: { locale: "en" }, version: 0 }),
            },
          ],
        },
      ],
    },
  },
  webServer: [
    {
      command:
        "BIFROST_DATA_DIR=../.bifrost-e2e-search-live BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 ../.bifrost-ui-target/debug/bifrost start --host 127.0.0.1 -p 8848 --unsafe-ssl --no-system-proxy --skip-cert-check --access-mode allow_all",
      url: "http://127.0.0.1:8848/_bifrost/api/proxy/address",
      reuseExistingServer: false,
      timeout: 30_000,
    },
  ],
});
