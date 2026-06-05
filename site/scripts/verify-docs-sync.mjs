import fs from "node:fs";
import path from "node:path";

import { buildPagesSync, outputRoot, repoRoot } from "./docs-sync-lib.mjs";

const pages = buildPagesSync(repoRoot);
const errors = [];

for (const page of pages) {
  const targetPath = path.join(outputRoot, page.target);
  if (!fs.existsSync(targetPath)) {
    errors.push(`missing generated target for ${page.source}: ${page.target}`);
    continue;
  }

  const content = fs.readFileSync(targetPath, "utf8");
  const marker = page.source.startsWith("docs-en/")
    ? `> This page is automatically synced from \`${page.source}\`.`
    : `> 此页面由 \`${page.source}\` 自动同步生成。`;
  if (!content.includes(marker)) {
    errors.push(`generated target does not record source ${page.source}: ${page.target}`);
  }
}

const targets = new Set();
for (const page of pages) {
  if (targets.has(page.target)) {
    errors.push(`duplicate generated target: ${page.target}`);
  }
  targets.add(page.target);
}

if (errors.length > 0) {
  console.error("Docs sync verification failed:");
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exit(1);
}

console.log(`Docs sync verification passed for ${pages.length} docs pages.`);
