import assert from "node:assert/strict";
import fsp from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { buildHome, collectHomeErrors } from "./home-static-lib.mjs";

async function withFixture(callback) {
  const root = await fsp.mkdtemp(path.join(os.tmpdir(), "bifrost-home-static-"));
  try {
    const homeRoot = path.join(root, "home");
    const distRoot = path.join(root, "dist");
    await fsp.mkdir(homeRoot, { recursive: true });
    await fsp.mkdir(distRoot, { recursive: true });
    await fsp.writeFile(path.join(distRoot, "favicon.png"), "fake-png", "utf8");
    await callback({ homeRoot, distRoot });
  } finally {
    await fsp.rm(root, { recursive: true, force: true });
  }
}

const html = `<!doctype html>
<html>
  <head>
    <link rel="stylesheet" href="%HOME_CSS%" />
    <script src="%HOME_JS%" defer></script>
  </head>
  <body>
    <a href="%BASE_PATH%docs/">Docs</a>
    <img src="%BASE_PATH%favicon.png" alt="" width="32" height="32" />
    <button data-lang="en" aria-pressed="true">EN</button>
    <button data-lang="zh" aria-pressed="false">中文</button>
    <div role="tablist"><button role="tab" aria-selected="true">CLI</button></div>
    <code>bifrost start -d</code>
  </body>
</html>`;

test("buildHome writes hashed assets and replaces the GitHub Pages base path", async () => {
  await withFixture(async ({ homeRoot, distRoot }) => {
    await fsp.writeFile(path.join(homeRoot, "index.html"), html, "utf8");
    await fsp.writeFile(path.join(homeRoot, "styles.css"), "body{color:#111}", "utf8");
    await fsp.writeFile(path.join(homeRoot, "home.js"), "document.body.dataset.ready='1';", "utf8");

    const result = await buildHome({ homeRoot, distRoot, basePath: "/bifrost" });
    const output = await fsp.readFile(result.htmlPath, "utf8");

    assert.match(output, /href="\/bifrost\/docs\/"/);
    assert.match(output, /href="\/bifrost\/assets\/styles\.[a-f0-9]{10}\.css"/);
    assert.match(output, /src="\/bifrost\/assets\/home\.[a-f0-9]{10}\.js"/);
    assert.deepEqual(await collectHomeErrors({ distRoot, basePath: "/bifrost" }), []);
  });
});

test("collectHomeErrors rejects framework markers and missing image dimensions", async () => {
  await withFixture(async ({ distRoot }) => {
    await fsp.writeFile(
      path.join(distRoot, "index.html"),
      '<html><body><a href="/docs/">Docs</a><img src="/favicon.png"><astro-island></astro-island></body></html>',
      "utf8",
    );

    const errors = await collectHomeErrors({ distRoot, basePath: "/" });

    assert.ok(errors.some((error) => error.includes("astro-island")));
    assert.ok(errors.some((error) => error.includes("missing width/height")));
  });
});
