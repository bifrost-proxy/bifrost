import { defineConfig } from "@playwright/test";
// Isolated frontend regression with deterministic API streams, no proxy/backend build.
export default defineConfig({
  testDir: "./tests/search",
  testMatch: "search-session.spec.ts",
  workers: 1,
  use: {
    baseURL: "http://127.0.0.1:3047",
    launchOptions: {
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
    },
  },
  webServer: {
    command:
      "BACKEND_PORT=8847 WEB_PORT=3047 node_modules/.bin/vite --host 127.0.0.1 --port 3047",
    url: "http://127.0.0.1:3047/_bifrost/",
    reuseExistingServer: false,
  },
});
