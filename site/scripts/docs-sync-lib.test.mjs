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
import { collectSiteLinkErrors } from "./verify-site-links.mjs";

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
      "docs-en/README.md": "# Documentation Overview\n\n[CLI](./cli.md)",
      "docs-en/cli.md": "# CLI Command Reference",
      "docs/future/new-topic.md": "# Future Topic",
      "docs/future/README.md": "# Future Index",
      "docs/.draft.md": "# Hidden",
    },
    async (root) => {
      const pages = buildPagesSync(root);
      const sources = pages.map((page) => page.source).sort();

      assert.deepEqual(sources, [
        "docs-en/README.md",
        "docs-en/cli.md",
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
      assert.equal(
        pages.find((page) => page.source === "docs-en/README.md")?.target,
        "en/reference/index.mdx",
      );
      assert.equal(
        pages.find((page) => page.source === "docs-en/cli.md")?.target,
        "en/reference/cli.md",
      );
    },
  );
});

test("rewriteMarkdownLinks keeps English docs within English site routes", async () => {
  await withFixture(
    {
      "docs/README.md": "# Docs\n\n[CLI](./cli.md)",
      "docs/cli.md": "# CLI",
      "docs-en/README.md": "# Documentation Overview\n\n[CLI](./cli.md)",
      "docs-en/cli.md": "# CLI Command Reference",
      "docs-en/rules/README.md": "# Rules\n\n[Routing](./routing.md)",
      "docs-en/rules/routing.md": "# Routing Rules",
    },
    async (root) => {
      const pages = buildPagesSync(root);
      const map = sourceToTargetMap(pages);
      const englishIndex = pages.find((candidate) => candidate.source === "docs-en/README.md");
      const englishIndexMarkdown = fs.readFileSync(
        path.join(root, "docs-en/README.md"),
        "utf8",
      );
      const rewrittenIndex = rewriteMarkdownLinks(englishIndexMarkdown, englishIndex, map);

      assert.match(rewrittenIndex, /\[CLI\]\(\.\/cli\/\)/);

      const englishRules = pages.find(
        (candidate) => candidate.source === "docs-en/rules/README.md",
      );
      const englishRulesMarkdown = fs.readFileSync(
        path.join(root, "docs-en/rules/README.md"),
        "utf8",
      );
      const rewrittenRules = rewriteMarkdownLinks(englishRulesMarkdown, englishRules, map);

      assert.match(rewrittenRules, /\[Routing\]\(\.\/routing\/\)/);
    },
  );
});

test("rewriteMarkdownLinks rewrites links through discovered site routes", async () => {
  await withFixture(
    {
      "docs/README.md": "# Docs\n\n[CLI](./cli.md)\n[Future](./future/new-topic.md#usage)",
      "docs/cli-quick-start.md": "# Quick Start\n\n[CLI](./cli.md)\n[Install](./getting-started.md)",
      "docs/cli.md": "# CLI",
      "docs/getting-started.md": "# Install",
      "docs/rule.md": "# Rules\n\n[Patterns](./pattern.md)",
      "docs/pattern.md": "# Patterns",
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

      const quickStart = pages.find((candidate) => candidate.source === "docs/cli-quick-start.md");
      const quickStartMarkdown = fs.readFileSync(
        path.join(root, "docs/cli-quick-start.md"),
        "utf8",
      );
      const rewrittenQuickStart = rewriteMarkdownLinks(quickStartMarkdown, quickStart, map);

      assert.match(rewrittenQuickStart, /\[CLI\]\(\.\.\/\.\.\/reference\/cli\/\)/);
      assert.match(rewrittenQuickStart, /\[Install\]\(\.\.\/installation\/\)/);

      const rule = pages.find((candidate) => candidate.source === "docs/rule.md");
      const ruleMarkdown = fs.readFileSync(path.join(root, "docs/rule.md"), "utf8");
      const rewrittenRule = rewriteMarkdownLinks(ruleMarkdown, rule, map);

      assert.match(rewrittenRule, /\[Patterns\]\(\.\.\/patterns\/\)/);
    },
  );
});

test("syncDocs removes stale generated pages and writes source metadata", async () => {
  await withFixture(
    {
      "docs/README.md": "# Docs",
      "docs/new-topic.md": "# New Topic",
      "docs-en/README.md": "# Documentation Overview",
      "docs-en/new-topic.md": "# New Topic",
      "site/src/content/docs/reference/stale.md": [
        "---",
        "title: Stale",
        "---",
        "",
        "> 此页面由 `docs/old.md` 自动同步生成。",
        "",
      ].join("\n"),
      "site/src/content/docs/en/reference/stale.md": [
        "---",
        "title: Stale",
        "---",
        "",
        "> This page is automatically synced from `docs-en/old.md`.",
        "",
      ].join("\n"),
    },
    async (root) => {
      const output = path.join(root, "site", "src", "content", "docs");
      const pages = await syncDocs({
        root,
        docsOutputRoot: output,
      });

      assert.equal(pages.length, 4);
      assert.equal(fs.existsSync(path.join(output, "reference/stale.md")), false);
      assert.equal(fs.existsSync(path.join(output, "en/reference/stale.md")), false);
      const generated = fs.readFileSync(path.join(output, "reference/new-topic.md"), "utf8");
      assert.match(generated, /> 此页面由 `docs\/new-topic\.md` 自动同步生成。/);
      assert.match(generated, /sidebar:\n  label: "New Topic"\n  order:/);
      const englishGenerated = fs.readFileSync(
        path.join(output, "en/reference/new-topic.md"),
        "utf8",
      );
      assert.match(
        englishGenerated,
        /> This page is automatically synced from `docs-en\/new-topic\.md`\./,
      );
      assert.match(englishGenerated, /sidebar:\n  label: "New Topic"\n  order:/);
    },
  );
});

test("collectSiteLinkErrors verifies local links under the configured base path", async () => {
  await withFixture(
    {
      "site/dist/index.html": '<a href="/bifrost/reference/getting-started/cli-quick-start/">CLI</a>',
      "site/dist/reference/getting-started/cli-quick-start/index.html":
        '<a href="/bifrost/getting-started/cli-quick-start/">Canonical</a>',
      "site/dist/getting-started/cli-quick-start/index.html": '<a href="../installation/">Install</a>',
      "site/dist/getting-started/installation/index.html": "<main>Install</main>",
    },
    async (root) => {
      const errors = await collectSiteLinkErrors({
        distRoot: path.join(root, "site", "dist"),
        basePath: "/bifrost",
      });

      assert.deepEqual(errors, []);
    },
  );
});

test("collectSiteLinkErrors reports missing local site targets", async () => {
  await withFixture(
    {
      "site/dist/index.html": [
        '<a href="/bifrost/reference/missing/">Missing</a>',
        '<a href="/getting-started/cli-quick-start/">Missing base</a>',
      ].join("\n"),
    },
    async (root) => {
      const errors = await collectSiteLinkErrors({
        distRoot: path.join(root, "site", "dist"),
        basePath: "/bifrost",
      });

      assert.equal(errors.length, 2);
      assert.match(errors[0], /reference\/missing/);
      assert.match(errors[1], /without configured base path/);
    },
  );
});
