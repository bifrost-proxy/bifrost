#!/usr/bin/env node

import fsp from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const siteRoot = path.resolve(scriptDir, "..");
const distRoot = path.join(siteRoot, "dist");

function normalizeBasePath(value) {
  if (!value || value === "/") {
    return "/";
  }
  const withLeadingSlash = value.startsWith("/") ? value : `/${value}`;
  return `${withLeadingSlash.replace(/^\/+/, "/").replace(/\/+$/, "")}/`;
}

function defaultBasePath() {
  if (process.env.BASE_PATH) {
    return normalizeBasePath(process.env.BASE_PATH);
  }
  const site = new URL(process.env.SITE_URL ?? "https://bifrost-proxy.github.io/bifrost");
  return normalizeBasePath(site.hostname.endsWith("github.io") ? site.pathname : "/");
}

const redirects = [
  ["reference/getting-started/overview/index.html", "getting-started/overview"],
  ["reference/getting-started/installation/index.html", "getting-started/installation"],
  ["reference/getting-started/cli-quick-start/index.html", "getting-started/cli-quick-start"],
  ["reference/getting-started/desktop/index.html", "getting-started/desktop"],
];

function redirectHtml(target) {
  const safeTarget = target.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
  return [
    "<!doctype html>",
    '<html lang="zh-CN">',
    "<head>",
    '<meta charset="utf-8">',
    '<meta name="robots" content="noindex">',
    `<meta http-equiv="refresh" content="0; url=${safeTarget}">`,
    `<link rel="canonical" href="${safeTarget}">`,
    "<title>Redirecting...</title>",
    "</head>",
    "<body>",
    `<a href="${safeTarget}">Redirecting to ${safeTarget}</a>`,
    "</body>",
    "</html>",
    "",
  ].join("\n");
}

export async function writeRedirects({ basePath = defaultBasePath() } = {}) {
  const normalizedBasePath = normalizeBasePath(basePath);
  await Promise.all(
    redirects.map(async ([from, to]) => {
      const target = `${normalizedBasePath}${to}`;
      const outPath = path.join(distRoot, from);
      await fsp.mkdir(path.dirname(outPath), { recursive: true });
      await fsp.writeFile(outPath, redirectHtml(target), "utf8");
    }),
  );
  return redirects.length;
}

const count = await writeRedirects();
console.log(`Static redirects written: ${count}`);
