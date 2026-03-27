import { defineConfig } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { allocateUiTestEnv } from "./tests/ui/helpers/test-env";

const env = await allocateUiTestEnv();
const webPort = env.webPort;
const webRoot = fileURLToPath(new URL(".", import.meta.url));
const backendPort = env.backendPort;

export default defineConfig({
  testDir: "./tests/ui",
  timeout: 120000,
  workers: 1,
  expect: {
    timeout: 15000,
  },
  use: {
    baseURL: `http://127.0.0.1:${webPort}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  webServer: {
    command: `BACKEND_PORT=${backendPort} WEB_PORT=${webPort} pnpm run dev -- --host 127.0.0.1 --port ${webPort}`,
    url: `http://127.0.0.1:${webPort}/_bifrost/`,
    reuseExistingServer: false,
    cwd: webRoot,
    timeout: 120000,
  },
  globalSetup: "./tests/ui/global-setup.ts",
  globalTeardown: "./tests/ui/global-teardown.ts",
});
