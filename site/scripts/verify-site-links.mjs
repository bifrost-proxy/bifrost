#!/usr/bin/env node

import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..", "..");

function normalizeBasePath(basePath) {
  if (!basePath || basePath === "/") {
    return "/";
  }
  const withLeadingSlash = basePath.startsWith("/") ? basePath : `/${basePath}`;
  return withLeadingSlash.replace(/\/+$/, "");
}

function defaultBasePath() {
  if (process.env.BASE_PATH) {
    return normalizeBasePath(process.env.BASE_PATH);
  }
  const site = new URL(process.env.SITE_URL ?? "https://bifrost-proxy.github.io/bifrost");
  return normalizeBasePath(site.hostname.endsWith("github.io") ? site.pathname : "/");
}

function routeForHtmlFile(distRoot, filePath) {
  const relative = path.relative(distRoot, filePath).split(path.sep).join("/");
  if (relative === "index.html") {
    return "/";
  }
  if (relative.endsWith("/index.html")) {
    return `/${relative.slice(0, -"/index.html".length)}/`;
  }
  return `/${relative}`;
}

async function listHtmlFiles(root) {
  const entries = await fsp.readdir(root, { withFileTypes: true });
  const files = await Promise.all(
    entries.map(async (entry) => {
      const fullPath = path.join(root, entry.name);
      if (entry.isDirectory()) {
        return listHtmlFiles(fullPath);
      }
      return entry.isFile() && entry.name.endsWith(".html") ? [fullPath] : [];
    }),
  );
  return files.flat();
}

function isIgnoredLink(rawLink) {
  if (!rawLink || rawLink.startsWith("#")) {
    return true;
  }
  if (/^(?:https?:|mailto:|tel:|javascript:|data:|blob:)/i.test(rawLink)) {
    return true;
  }
  return rawLink.startsWith("{{") || rawLink.startsWith("<");
}

function stripBasePath(pathname, basePath) {
  if (basePath === "/") {
    return pathname;
  }
  if (pathname === basePath) {
    return "/";
  }
  if (pathname.startsWith(`${basePath}/`)) {
    return pathname.slice(basePath.length) || "/";
  }
  return pathname;
}

function fileExistsForPath(distRoot, pathname) {
  const decodedPath = decodeURIComponent(pathname);
  const safePath = path.normalize(decodedPath).replace(/^(\.\.(?:\/|\\|$))+/, "");
  const withoutLeadingSlash = safePath.replace(/^\/+/, "");
  const absoluteTarget = path.join(distRoot, withoutLeadingSlash);

  if (decodedPath.endsWith("/")) {
    return fs.existsSync(path.join(absoluteTarget, "index.html"));
  }
  if (path.extname(decodedPath)) {
    return fs.existsSync(absoluteTarget);
  }
  return (
    fs.existsSync(absoluteTarget) ||
    fs.existsSync(path.join(absoluteTarget, "index.html")) ||
    fs.existsSync(`${absoluteTarget}.html`)
  );
}

function extractLocalLinks(html) {
  const links = [];
  const pattern = /\s(?:href|src)=["']([^"']+)["']/gi;
  let match;
  while ((match = pattern.exec(html)) !== null) {
    links.push(match[1]);
  }
  return links;
}

export async function collectSiteLinkErrors({
  distRoot = path.join(repoRoot, "site", "dist"),
  basePath = defaultBasePath(),
} = {}) {
  if (!fs.existsSync(distRoot)) {
    return [`Missing site dist directory: ${distRoot}`];
  }

  const normalizedBasePath = normalizeBasePath(basePath);
  const htmlFiles = await listHtmlFiles(distRoot);
  const errors = [];

  for (const htmlFile of htmlFiles) {
    const html = await fsp.readFile(htmlFile, "utf8");
    const route = routeForHtmlFile(distRoot, htmlFile);
    const sourceUrl = `https://local.test${normalizedBasePath === "/" ? "" : normalizedBasePath}${route}`;

    for (const rawLink of extractLocalLinks(html)) {
      if (isIgnoredLink(rawLink)) {
        continue;
      }

      let resolved;
      try {
        resolved = new URL(rawLink, sourceUrl);
      } catch {
        errors.push(`${path.relative(distRoot, htmlFile)} has an invalid link: ${rawLink}`);
        continue;
      }

      if (resolved.origin !== "https://local.test") {
        continue;
      }

      if (
        normalizedBasePath !== "/" &&
        rawLink.startsWith("/") &&
        !rawLink.startsWith("//") &&
        rawLink !== normalizedBasePath &&
        !rawLink.startsWith(`${normalizedBasePath}/`)
      ) {
        errors.push(
          `${path.relative(distRoot, htmlFile)} uses root-absolute link without configured base path: ${rawLink}`,
        );
        continue;
      }

      const localPath = stripBasePath(resolved.pathname, normalizedBasePath);
      if (!fileExistsForPath(distRoot, localPath)) {
        errors.push(
          `${path.relative(distRoot, htmlFile)} links to missing target ${rawLink} (${localPath})`,
        );
      }
    }
  }

  return errors;
}

async function main() {
  const errors = await collectSiteLinkErrors();
  if (errors.length > 0) {
    console.error("Site link verification failed:");
    for (const error of errors) {
      console.error(`- ${error}`);
    }
    process.exit(1);
  }
  console.log("Site link verification passed.");
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
