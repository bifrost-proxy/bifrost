import { defineConfig } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { allocateUiTestEnv } from "./tests/ui/helpers/test-env";

const env = await allocateUiTestEnv();
const webPort = env.webPort;
const webRoot = fileURLToPath(new URL(".", import.meta.url));
const backendPort = env.backendPort;
const serveBuiltFrontend =
  process.env.BIFROST_UI_TEST_SERVE_BUILT_FRONTEND === "1";

export default defineConfig({
  testDir: "./tests/ui",
  timeout: 120000,
  globalTimeout: Number(process.env.BIFROST_UI_TEST_GLOBAL_TIMEOUT_MS || 0),
  reporter: process.env.CI
    ? [
        ["line"],
        ["json", { outputFile: "test-results/results.json" }],
        ["html", { open: "never" }],
      ]
    : "list",
  workers: 1,
  expect: {
    timeout: 15000,
  },
  use: {
    baseURL: `http://127.0.0.1:${serveBuiltFrontend ? backendPort : webPort}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
      : undefined,
  },
  webServer: serveBuiltFrontend
    ? undefined
    : {
        command: `BACKEND_PORT=${backendPort} WEB_PORT=${webPort} node_modules/.bin/vite --host 127.0.0.1 --port ${webPort}`,
        url: `http://127.0.0.1:${webPort}/_bifrost/`,
        reuseExistingServer: true,
        cwd: webRoot,
        timeout: 120000,
      },
  globalSetup: "./tests/ui/global-setup.ts",
  globalTeardown: "./tests/ui/global-teardown.ts",
});
