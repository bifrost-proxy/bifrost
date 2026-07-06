import assert from "node:assert/strict";
import fsp from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { writeRedirects } from "./write-redirects.mjs";

async function withDist(callback) {
  const distRoot = await fsp.mkdtemp(path.join(os.tmpdir(), "bifrost-site-redirects-"));
  try {
    await callback(distRoot);
  } finally {
    await fsp.rm(distRoot, { recursive: true, force: true });
  }
}

test("writeRedirects covers the legacy /bifrost/ entry only for root deployments", async () => {
  await withDist(async (distRoot) => {
    const count = await writeRedirects({ basePath: "/", distRoot });
    const legacyRedirect = await fsp.readFile(path.join(distRoot, "bifrost/index.html"), "utf8");

    assert.equal(count, 5);
    assert.match(legacyRedirect, /<meta name="robots" content="noindex">/);
    assert.match(legacyRedirect, /<meta http-equiv="refresh" content="0; url=\/">/);
    assert.match(legacyRedirect, /<link rel="canonical" href="\/">/);
  });
});

test("writeRedirects keeps /bifrost/ as the real home for subpath deployments", async () => {
  await withDist(async (distRoot) => {
    const count = await writeRedirects({ basePath: "/bifrost/", distRoot });

    await assert.rejects(fsp.access(path.join(distRoot, "bifrost/index.html")));
    assert.equal(count, 4);
  });
});
