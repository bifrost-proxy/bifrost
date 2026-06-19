import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "jsdom",
    environmentOptions: {
      jsdom: {
        url: "http://127.0.0.1/",
      },
    },
    setupFiles: ["./vitest.setup.ts"],
    exclude: ["tests/ui/**", "node_modules/**"],
  },
});
