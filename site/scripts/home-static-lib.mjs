import crypto from "node:crypto";
import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
export const siteRoot = path.resolve(scriptDir, "..");
export const repoRoot = path.resolve(siteRoot, "..");
export const defaultHomeRoot = path.join(siteRoot, "home");
export const defaultDistRoot = path.join(siteRoot, "dist");

const budgets = {
  htmlGzip: 28 * 1024,
  cssGzip: 12 * 1024,
  jsGzip: 4 * 1024,
  firstPartyGzip: 150 * 1024,
};

function normalizeBasePath(basePath) {
  if (!basePath || basePath === "/") {
    return "/";
  }
  const withLeadingSlash = basePath.startsWith("/") ? basePath : `/${basePath}`;
  return `${withLeadingSlash.replace(/^\/+/, "/").replace(/\/+$/, "")}/`;
}

export function defaultBasePath() {
  if (process.env.BASE_PATH) {
    return normalizeBasePath(process.env.BASE_PATH);
  }
  const site = new URL(process.env.SITE_URL ?? "https://bifrost-proxy.github.io/bifrost");
  return normalizeBasePath(site.hostname.endsWith("github.io") ? site.pathname : "/");
}

function assetName(name, content) {
  const ext = path.extname(name);
  const base = path.basename(name, ext);
  const hash = crypto.createHash("sha256").update(content).digest("hex").slice(0, 10);
  return `${base}.${hash}${ext}`;
}

async function writeHashedAsset({ sourcePath, distRoot, basePath }) {
  const content = await fsp.readFile(sourcePath);
  const name = assetName(path.basename(sourcePath), content);
  const targetDir = path.join(distRoot, "assets");
  await fsp.mkdir(targetDir, { recursive: true });
  await fsp.writeFile(path.join(targetDir, name), content);
  return `${basePath}assets/${name}`;
}

export async function buildHome({
  homeRoot = defaultHomeRoot,
  distRoot = defaultDistRoot,
  basePath = defaultBasePath(),
} = {}) {
  const normalizedBasePath = normalizeBasePath(basePath);
  const htmlPath = path.join(homeRoot, "index.html");
  const cssPath = path.join(homeRoot, "styles.css");
  const jsPath = path.join(homeRoot, "home.js");

  const [cssUrl, jsUrl, htmlSource] = await Promise.all([
    writeHashedAsset({ sourcePath: cssPath, distRoot, basePath: normalizedBasePath }),
    writeHashedAsset({ sourcePath: jsPath, distRoot, basePath: normalizedBasePath }),
    fsp.readFile(htmlPath, "utf8"),
  ]);

  const html = htmlSource
    .replaceAll("%BASE_PATH%", normalizedBasePath)
    .replaceAll("%HOME_CSS%", cssUrl)
    .replaceAll("%HOME_JS%", jsUrl);

  await fsp.mkdir(distRoot, { recursive: true });
  await fsp.writeFile(path.join(distRoot, "index.html"), html, "utf8");

  return {
    basePath: normalizedBasePath,
    htmlPath: path.join(distRoot, "index.html"),
    cssUrl,
    jsUrl,
  };
}

function gzipSize(content) {
  return gzipSync(content).byteLength;
}

function localAssetPaths(html) {
  const paths = [];
  const pattern = /<(?:link|script|img)\b[^>]*(?:href|src)=["']([^"']+)["'][^>]*>/gi;
  let match;
  while ((match = pattern.exec(html)) !== null) {
    const value = match[1];
    if (/^(?:https?:|mailto:|tel:|data:|blob:|#)/i.test(value)) {
      continue;
    }
    paths.push(value);
  }
  return paths;
}

function stripBase(value, basePath) {
  if (basePath !== "/" && value.startsWith(basePath)) {
    return value.slice(basePath.length);
  }
  return value.replace(/^\/+/, "");
}

function assertImageDimensions(html) {
  const pattern = /<img\b[^>]*>/gi;
  const errors = [];
  let match;
  while ((match = pattern.exec(html)) !== null) {
    const tag = match[0];
    if (!/\bwidth=["'][0-9]+["']/i.test(tag) || !/\bheight=["'][0-9]+["']/i.test(tag)) {
      errors.push(`Image is missing width/height: ${tag}`);
    }
  }
  return errors;
}

export async function collectHomeErrors({
  distRoot = defaultDistRoot,
  basePath = defaultBasePath(),
} = {}) {
  const normalizedBasePath = normalizeBasePath(basePath);
  const htmlPath = path.join(distRoot, "index.html");
  if (!fs.existsSync(htmlPath)) {
    return [`Missing home page: ${htmlPath}`];
  }

  const html = await fsp.readFile(htmlPath, "utf8");
  const errors = [];
  const forbidden = ["%BASE_PATH%", "%HOME_CSS%", "%HOME_JS%", "astro-island", "/_astro/", "@vite/client"];
  for (const marker of forbidden) {
    if (html.includes(marker)) {
      errors.push(`Home page contains forbidden marker: ${marker}`);
    }
  }

  if (!html.includes(`href="${normalizedBasePath}docs/"`)) {
    errors.push("Home page is missing the docs link.");
  }
  if (!html.includes('role="tablist"') || !html.includes('aria-selected="true"')) {
    errors.push("Home page preview tabs are missing accessible tab markup.");
  }
  if (!html.includes('data-lang="zh"') || !html.includes('data-lang="en"')) {
    errors.push("Home page is missing the English/Chinese language switch.");
  }
  if (!html.includes("bifrost start -d")) {
    errors.push("Home page must guide first-run users to start the daemon with `bifrost start -d`.");
  }
  if (html.includes("bifrost start --no-system-proxy")) {
    errors.push("Home page must not default first-run users to `bifrost start --no-system-proxy`.");
  }
  errors.push(...assertImageDimensions(html));

  const htmlGzip = gzipSize(html);
  if (htmlGzip > budgets.htmlGzip) {
    errors.push(`Home HTML gzip size ${htmlGzip} exceeds ${budgets.htmlGzip}.`);
  }

  let firstPartyGzip = htmlGzip;
  for (const asset of localAssetPaths(html)) {
    const relative = stripBase(asset, normalizedBasePath).replace(/[?#].*$/, "");
    const assetPath = path.join(distRoot, relative);
    if (!fs.existsSync(assetPath)) {
      errors.push(`Home page references missing local asset: ${asset}`);
      continue;
    }
    const content = await fsp.readFile(assetPath);
    const size = gzipSize(content);
    firstPartyGzip += size;
    if (relative.endsWith(".css") && size > budgets.cssGzip) {
      errors.push(`Home CSS gzip size ${size} exceeds ${budgets.cssGzip}: ${asset}`);
    }
    if (relative.endsWith(".js") && size > budgets.jsGzip) {
      errors.push(`Home JS gzip size ${size} exceeds ${budgets.jsGzip}: ${asset}`);
    }
  }

  if (firstPartyGzip > budgets.firstPartyGzip) {
    errors.push(`Home first-party gzip size ${firstPartyGzip} exceeds ${budgets.firstPartyGzip}.`);
  }

  return errors;
}

export async function verifyHome(options = {}) {
  const errors = await collectHomeErrors(options);
  if (errors.length > 0) {
    throw new Error(errors.join("\n"));
  }
}
