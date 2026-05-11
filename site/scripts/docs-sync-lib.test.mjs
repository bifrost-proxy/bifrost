import assert from "node:assert/strict";
import fs from "node:fs";
import fsp from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  buildPagesSync,
  rewriteMarkdownLinks,
  sourceToTargetMap,
  syncDocs,
} from "./docs-sync-lib.mjs";

async function withFixture(files, callback) {
  const root = await fsp.mkdtemp(path.join(os.tmpdir(), "bifrost-docs-sync-"));
  try {
    await Promise.all(
      Object.entries(files).map(async ([relativePath, content]) => {
        const filePath = path.join(root, relativePath);
        await fsp.mkdir(path.dirname(filePath), { recursive: true });
        await fsp.writeFile(filePath, content, "utf8");
      }),
    );
    await callback(root);
  } finally {
    await fsp.rm(root, { recursive: true, force: true });
  }
}

test("buildPagesSync discovers every docs markdown file recursively", async () => {
  await withFixture(
    {
      "docs/README.md": "# Docs\n\n[CLI](./cli.md)",
      "docs/cli.md": "# CLI",
      "docs/future/new-topic.md": "# Future Topic",
      "docs/future/README.md": "# Future Index",
      "docs/.draft.md": "# Hidden",
    },
    async (root) => {
      const pages = buildPagesSync(root);
      const sources = pages.map((page) => page.source).sort();

      assert.deepEqual(sources, [
        "docs/README.md",
        "docs/cli.md",
        "docs/future/README.md",
        "docs/future/new-topic.md",
      ]);
      assert.equal(
        pages.find((page) => page.source === "docs/cli.md")?.target,
        "reference/cli.md",
      );
      assert.equal(
        pages.find((page) => page.source === "docs/future/README.md")?.target,
        "reference/future/index.md",
      );
      assert.equal(
        pages.find((page) => page.source === "docs/future/new-topic.md")?.target,
        "reference/future/new-topic.md",
      );
    },
  );
});

test("rewriteMarkdownLinks rewrites links through discovered site routes", async () => {
  await withFixture(
    {
      "docs/README.md": "# Docs\n\n[CLI](./cli.md)\n[Future](./future/new-topic.md#usage)",
      "docs/cli.md": "# CLI",
      "docs/future/new-topic.md": "# Future Topic",
    },
    async (root) => {
      const pages = buildPagesSync(root);
      const map = sourceToTargetMap(pages);
      const page = pages.find((candidate) => candidate.source === "docs/README.md");
      const markdown = fs.readFileSync(path.join(root, "docs/README.md"), "utf8");

      const rewritten = rewriteMarkdownLinks(markdown, page, map);

      assert.match(rewritten, /\[CLI\]\(\.\/cli\/\)/);
      assert.match(rewritten, /\[Future\]\(\.\/future\/new-topic\/#usage\)/);
    },
  );
});

test("syncDocs removes stale generated pages and writes source metadata", async () => {
  await withFixture(
    {
      "docs/README.md": "# Docs",
      "docs/new-topic.md": "# New Topic",
      "site/src/content/docs/reference/stale.md": [
        "---",
        "title: Stale",
        "---",
        "",
        "> 此页面由 `docs/old.md` 自动同步生成。",
        "",
      ].join("\n"),
    },
    async (root) => {
      const output = path.join(root, "site", "src", "content", "docs");
      const pages = await syncDocs({
        root,
        docsOutputRoot: output,
      });

      assert.equal(pages.length, 2);
      assert.equal(fs.existsSync(path.join(output, "reference/stale.md")), false);
      const generated = fs.readFileSync(path.join(output, "reference/new-topic.md"), "utf8");
      assert.match(generated, /> 此页面由 `docs\/new-topic\.md` 自动同步生成。/);
      assert.match(generated, /sidebar:\n  label: "New Topic"\n  order:/);
    },
  );
});
