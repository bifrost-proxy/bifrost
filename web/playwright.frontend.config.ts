import { defineConfig } from "@playwright/test";
import { fileURLToPath } from "node:url";
import net from "node:net";

async function findFreePort(): Promise<number> {
  return await new Promise<number>((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      if (!addr || typeof addr === "string") {
        server.close(() => reject(new Error("Failed to allocate a port")));
        return;
      }
      server.close(() => resolve(addr.port));
    });
  });
}

const webPort = Number(process.env.WEB_PORT || 0) || (await findFreePort());
const webRoot = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  testDir: "./tests/ui",
  timeout: 60000,
  workers: 1,
  expect: {
    timeout: 10000,
  },
  use: {
    baseURL: `http://127.0.0.1:${webPort}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
      : undefined,
  },
  webServer: {
    command: `WEB_PORT=${webPort} pnpm exec vite --host 127.0.0.1 --port ${webPort}`,
    url: `http://127.0.0.1:${webPort}/_bifrost/`,
    reuseExistingServer: false,
    cwd: webRoot,
    timeout: 90000,
  },
});
