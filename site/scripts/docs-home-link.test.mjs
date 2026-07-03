import test from "node:test";
import assert from "node:assert/strict";

import config from "../.vitepress/config.mjs";

test("docs logo uses a native same-tab navigation to the static home page", () => {
  assert.deepEqual(config.themeConfig.logoLink, {
    link: config.base,
    target: "_self",
  });
});
