#!/usr/bin/env node

import fsp from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const siteRoot = path.resolve(scriptDir, "..");
const outDir = process.env.PROJECT_PAGES_REDIRECT_OUT_DIR ?? "project-pages-redirect-dist";
const distRoot = path.resolve(siteRoot, outDir);
const target = "https://bifrost-proxy.github.io/";

function redirectHtml() {
  return [
    "<!doctype html>",
    '<html lang="zh-CN">',
    "<head>",
    '<meta charset="utf-8">',
    '<meta name="robots" content="noindex">',
    `<meta http-equiv="refresh" content="0; url=${target}">`,
    `<link rel="canonical" href="${target}">`,
    "<title>Redirecting to Bifrost</title>",
    "</head>",
    "<body>",
    `<a href="${target}">Redirecting to Bifrost</a>`,
    "</body>",
    "</html>",
    "",
  ].join("\n");
}

await fsp.rm(distRoot, { recursive: true, force: true });
await fsp.mkdir(distRoot, { recursive: true });
await fsp.writeFile(path.join(distRoot, "index.html"), redirectHtml(), "utf8");
await fsp.writeFile(path.join(distRoot, ".nojekyll"), "", "utf8");
console.log(`Project Pages redirect written to ${distRoot}`);
