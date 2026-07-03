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
    <meta name="robots" content="index, follow, max-image-preview:large" />
    <link rel="canonical" href="%SITE_URL%" />
    <meta property="og:title" content="Bifrost - AI Proxy" />
    <meta property="og:type" content="website" />
    <meta property="og:url" content="%SITE_URL%" />
    <meta property="og:image" content="%OG_IMAGE%" />
    <meta property="og:description" content="Traffic capture, rewrite, replay, and Coding Agent workflows." />
    <meta name="twitter:card" content="summary_large_image" />
    <meta name="twitter:title" content="Bifrost - AI Proxy" />
    <meta name="twitter:description" content="Traffic capture, rewrite, replay, and Coding Agent workflows." />
    <meta name="twitter:image" content="%OG_IMAGE%" />
    <link rel="stylesheet" href="%HOME_CSS%" />
    <script src="%HOME_JS%" defer></script>
    <script type="application/ld+json">
      {
        "@context": "https://schema.org",
        "@graph": [
          { "@type": "Organization", "name": "Bifrost", "url": "%SITE_URL%" },
          { "@type": "WebSite", "name": "Bifrost", "url": "%SITE_URL%" },
          { "@type": "SoftwareApplication", "name": "Bifrost" },
          { "@type": "WebPage", "name": "Bifrost" }
        ]
      }
    </script>
  </head>
  <body>
    <a href="%BASE_PATH%docs/">Docs</a>
    <a href="https://github.com/bifrost-proxy/bifrost">GitHub</a>
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
    assert.match(output, /rel="canonical" href="https:\/\/bifrost-proxy\.github\.io\/bifrost\/"/);
    assert.match(output, /content="https:\/\/bifrost-proxy\.github\.io\/bifrost\/og-image\.png"/);
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

test("collectHomeErrors rejects the removed top text navigation", async () => {
  await withFixture(async ({ distRoot }) => {
    await fsp.writeFile(
      path.join(distRoot, "index.html"),
      `<html><body>
        <nav class="nav-links"><a href="/docs/">Docs</a></nav>
        <link rel="canonical" href="https://bifrost-proxy.github.io/bifrost/" />
        <meta name="robots" content="index, follow, max-image-preview:large" />
        <meta property="og:title" content="Bifrost" />
        <meta property="og:type" content="website" />
        <meta property="og:url" content="https://bifrost-proxy.github.io/bifrost/" />
        <meta property="og:image" content="https://bifrost-proxy.github.io/bifrost/og-image.png" />
        <meta property="og:description" content="Bifrost" />
        <meta name="twitter:card" content="summary_large_image" />
        <meta name="twitter:title" content="Bifrost" />
        <meta name="twitter:description" content="Bifrost" />
        <meta name="twitter:image" content="https://bifrost-proxy.github.io/bifrost/og-image.png" />
        <script type="application/ld+json">
          {
            "@context": "https://schema.org",
            "@graph": [
              { "@type": "Organization", "name": "Bifrost" },
              { "@type": "WebSite", "name": "Bifrost" },
              { "@type": "SoftwareApplication", "name": "Bifrost" },
              { "@type": "WebPage", "name": "Bifrost" }
            ]
          }
        </script>
        <a href="https://github.com/bifrost-proxy/bifrost">GitHub</a>
        <img src="/favicon.png" alt="" width="32" height="32" />
        <button data-lang="en" aria-pressed="true">EN</button>
        <button data-lang="zh" aria-pressed="false">中文</button>
        <div role="tablist"><button role="tab" aria-selected="true">CLI</button></div>
        <code>bifrost start -d</code>
      </body></html>`,
      "utf8",
    );

    const errors = await collectHomeErrors({ distRoot, basePath: "/" });

    assert.ok(errors.some((error) => error.includes("top text navigation")));
  });
});
