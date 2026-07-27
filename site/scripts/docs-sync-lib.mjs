import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export const repoRoot = path.resolve(__dirname, "../..");
export const docsRoot = path.join(repoRoot, "docs");
export const docsEnRoot = path.join(repoRoot, "docs-en");
export const outputRoot = path.join(repoRoot, "site", "src", "content", "docs");

const knownPages = new Map(
  [
    [
      "docs/README.md",
      {
        target: "reference/index.md",
        title: "文档总览",
        description: "Bifrost 文档目录与推荐阅读路径。",
        order: 1,
      },
    ],
    [
      "docs/overview.md",
      {
        target: "getting-started/overview.md",
        title: "项目概览",
        description: "用几分钟理解 Bifrost 的定位、能力边界和适合的使用场景。",
        order: 10,
      },
    ],
    [
      "docs/getting-started.md",
      {
        target: "getting-started/installation.md",
        title: "安装与启动",
        description: "按照 docs/getting-started.md 的最新方式安装和启动 Bifrost。",
        order: 20,
      },
    ],
    [
      "docs/mobile-proxy.md",
      {
        target: "getting-started/mobile-proxy.md",
        title: "移动端连接 Bifrost",
        description: "让 iPhone、iPad 或 Android 设备连接到 PC、Mac 或 Linux 上运行的 Bifrost 代理。",
        order: 25,
      },
    ],
    [
      "docs/cli-quick-start.md",
      {
        target: "getting-started/cli-quick-start.md",
        title: "CLI 快速开始",
        description: "按场景快速掌握 Bifrost CLI 的常用工作流。",
        order: 30,
      },
    ],
    [
      "docs/desktop.md",
      {
        target: "getting-started/desktop.md",
        title: "桌面版安装与构建",
        description: "桌面版安装方式、本地构建步骤与常见注意事项。",
        order: 40,
      },
    ],
    [
      "docs/cli.md",
      {
        target: "reference/cli.md",
        title: "CLI 详细命令",
        description: "Bifrost CLI 命令、参数与环境变量的完整参考。",
        order: 110,
      },
    ],
    [
      "docs/rule.md",
      {
        target: "reference/rule-engine.md",
        title: "规则引擎",
        description: "Bifrost 规则系统的能力全景与配置入口。",
        order: 120,
      },
    ],
    [
      "docs/operation.md",
      {
        target: "reference/operations.md",
        title: "运行操作",
        description: "启动、使用与运行 Bifrost 的常见操作说明。",
        order: 130,
      },
    ],
    [
      "docs/pattern.md",
      {
        target: "reference/patterns.md",
        title: "匹配模式",
        description: "域名、路径、通配符与正则等匹配模式说明。",
        order: 140,
      },
    ],
    [
      "docs/scripts.md",
      {
        target: "reference/scripting.md",
        title: "脚本能力",
        description: "QuickJS 脚本能力、限制与使用方式。",
        order: 150,
      },
    ],
    [
      "docs/values.md",
      {
        target: "reference/values.md",
        title: "Values 使用说明",
        description: "Values 存储、规则引用、CLI 管理与脚本访问说明。",
        order: 160,
      },
    ],
    [
      "docs/replay.md",
      {
        target: "reference/replay.md",
        title: "请求重放说明",
        description: "请求重放能力、支持类型和使用建议。",
        order: 170,
      },
    ],
    [
      "docs/architecture.md",
      {
        target: "reference/architecture.md",
        title: "项目结构与模块说明",
        description: "Bifrost 代码仓库结构与核心模块说明。",
        order: 180,
      },
    ],
    [
      "docs/agent-skill.md",
      {
        target: "reference/agent-skill.md",
        title: "Agent Skill 安装说明",
        description: "Bifrost Agent Skill 的安装方式与使用入口。",
        order: 190,
      },
    ],
    [
      "docs/rules/README.md",
      {
        target: "reference/rules/index.md",
        title: "Rules 概览",
        description: "Bifrost Rules 语法与能力目录。",
        order: 200,
      },
    ],
    [
      "docs/rules/routing.md",
      {
        title: "路由控制",
        description: "规则路由、转发与代理控制说明。",
        order: 210,
      },
    ],
    [
      "docs/rules/request-modification.md",
      {
        title: "请求改写",
        description: "请求头、请求体与相关字段的改写说明。",
        order: 220,
      },
    ],
    [
      "docs/rules/response-modification.md",
      {
        title: "响应改写",
        description: "响应头、响应体与相关字段的改写说明。",
        order: 230,
      },
    ],
    [
      "docs/rules/url-manipulation.md",
      {
        title: "URL 改写",
        description: "URL、路径与查询参数的改写能力。",
        order: 240,
      },
    ],
    [
      "docs/rules/body-manipulation.md",
      {
        title: "Body 改写",
        description: "请求体与响应体改写能力说明。",
        order: 250,
      },
    ],
    [
      "docs/rules/timing-throttle.md",
      {
        title: "流量控制",
        description: "延迟、限速与相关节流能力说明。",
        order: 260,
      },
    ],
    [
      "docs/rules/status-redirect.md",
      {
        title: "状态跳转",
        description: "状态码控制、重定向和响应分支能力。",
        order: 270,
      },
    ],
    [
      "docs/rules/filters.md",
      {
        title: "过滤器",
        description: "请求与响应过滤条件的配置方式。",
        order: 280,
      },
    ],
    [
      "docs/rules/websocket.md",
      {
        title: "WebSocket",
        description: "WebSocket 相关规则与使用说明。",
        order: 290,
      },
    ],
    [
      "docs/rules/scripts.md",
      {
        title: "脚本规则",
        description: "在 Rules 中接入脚本能力的方式。",
        order: 300,
      },
    ],
    [
      "docs/rules/patterns.md",
      {
        title: "规则模式",
        description: "Rules 内的匹配模式与优先级细节。",
        order: 310,
      },
    ],
    [
      "docs/rules/rule-priority.md",
      {
        title: "规则优先级",
        description: "规则冲突与优先级处理方式。",
        order: 320,
      },
    ],
  ].map(([source, config]) => [normalizePath(source), config]),
);

const knownEnglishPages = new Map(
  [
    [
      "docs-en/README.md",
      {
        target: "en/reference/index.md",
        title: "Documentation Overview",
        description: "Bifrost documentation directory and recommended reading paths.",
        order: 1,
      },
    ],
    [
      "docs-en/overview.md",
      {
        target: "en/getting-started/overview.md",
        title: "Project Overview",
        description: "Understand Bifrost positioning, scope, and common use cases.",
        order: 10,
      },
    ],
    [
      "docs-en/getting-started.md",
      {
        target: "en/getting-started/installation.md",
        title: "Installation and Startup",
        description: "Install and start Bifrost using the English documentation path.",
        order: 20,
      },
    ],
    [
      "docs-en/mobile-proxy.md",
      {
        target: "en/getting-started/mobile-proxy.md",
        title: "Connect a Mobile Device to Bifrost",
        description: "Connect iPhone, iPad, and Android devices to Bifrost on a PC, Mac, or Linux host.",
        order: 25,
      },
    ],
    [
      "docs-en/cli-quick-start.md",
      {
        target: "en/getting-started/cli-quick-start.md",
        title: "CLI Quick Start",
        description: "Task-oriented Bifrost CLI workflows.",
        order: 30,
      },
    ],
    [
      "docs-en/desktop.md",
      {
        target: "en/getting-started/desktop.md",
        title: "Desktop Installation and Build",
        description: "Desktop app installation, local build steps, and notes.",
        order: 40,
      },
    ],
    [
      "docs-en/cli.md",
      {
        target: "en/reference/cli.md",
        title: "CLI Command Reference",
        description: "Bifrost CLI commands, flags, environment variables, and workflows.",
        order: 110,
      },
    ],
    [
      "docs-en/rule.md",
      {
        target: "en/reference/rule-engine.md",
        title: "Rule Engine",
        description: "Bifrost rule system overview and configuration entry points.",
        order: 120,
      },
    ],
    [
      "docs-en/operation.md",
      {
        target: "en/reference/operations.md",
        title: "Operation Reference",
        description: "Rule operation value formats and common usage.",
        order: 130,
      },
    ],
    [
      "docs-en/pattern.md",
      {
        target: "en/reference/patterns.md",
        title: "Matching Patterns",
        description: "Domain, path, wildcard, regex, and negated pattern behavior.",
        order: 140,
      },
    ],
    [
      "docs-en/scripts.md",
      {
        target: "en/reference/scripting.md",
        title: "Scripting",
        description: "QuickJS scripting capabilities, restrictions, and usage.",
        order: 150,
      },
    ],
    [
      "docs-en/values.md",
      {
        target: "en/reference/values.md",
        title: "Values Guide",
        description: "Values storage, rule references, CLI management, and script access.",
        order: 160,
      },
    ],
    [
      "docs-en/replay.md",
      {
        target: "en/reference/replay.md",
        title: "Request Replay",
        description: "Request replay capabilities, supported cases, and usage guidance.",
        order: 170,
      },
    ],
    [
      "docs-en/architecture.md",
      {
        target: "en/reference/architecture.md",
        title: "Project Structure",
        description: "Repository layout and core module descriptions.",
        order: 180,
      },
    ],
    [
      "docs-en/agent-skill.md",
      {
        target: "en/reference/agent-skill.md",
        title: "Agent Skill Installation",
        description: "Install and use the Bifrost Agent Skill.",
        order: 190,
      },
    ],
    [
      "docs-en/breakpoint.md",
      {
        target: "en/reference/breakpoint.md",
        title: "Breakpoint",
        description: "Pause, inspect, edit, and resume matched HTTP traffic.",
        order: 195,
      },
    ],
    [
      "docs-en/rules/README.md",
      {
        target: "en/reference/rules/index.md",
        title: "Rules Overview",
        description: "Bifrost Rules syntax and capability catalog.",
        order: 200,
      },
    ],
    [
      "docs-en/rules/routing.md",
      {
        title: "Routing",
        description: "Rule routing, forwarding, and proxy control.",
        order: 210,
      },
    ],
    [
      "docs-en/rules/request-modification.md",
      {
        title: "Request Modification",
        description: "Request headers, body, and related rewrite operations.",
        order: 220,
      },
    ],
    [
      "docs-en/rules/response-modification.md",
      {
        title: "Response Modification",
        description: "Response headers, body, and related rewrite operations.",
        order: 230,
      },
    ],
    [
      "docs-en/rules/url-manipulation.md",
      {
        title: "URL Manipulation",
        description: "URL, path, and query-parameter rewrite capabilities.",
        order: 240,
      },
    ],
    [
      "docs-en/rules/body-manipulation.md",
      {
        title: "Body Manipulation",
        description: "Request and response body manipulation capabilities.",
        order: 250,
      },
    ],
    [
      "docs-en/rules/timing-throttle.md",
      {
        title: "Delay and Throttling",
        description: "Delay, speed limiting, and throttling capabilities.",
        order: 260,
      },
    ],
    [
      "docs-en/rules/status-redirect.md",
      {
        title: "Status and Redirect",
        description: "Status-code control, redirects, and response branching.",
        order: 270,
      },
    ],
    [
      "docs-en/rules/filters.md",
      {
        title: "Filters",
        description: "Request and response filter configuration.",
        order: 280,
      },
    ],
    [
      "docs-en/rules/websocket.md",
      {
        title: "WebSocket",
        description: "WebSocket routing rules and usage.",
        order: 290,
      },
    ],
    [
      "docs-en/rules/scripts.md",
      {
        title: "Script Rules",
        description: "Attach scripts to Rules.",
        order: 300,
      },
    ],
    [
      "docs-en/rules/patterns.md",
      {
        title: "Rule Patterns",
        description: "Rule matching patterns and priority details.",
        order: 310,
      },
    ],
    [
      "docs-en/rules/rule-priority.md",
      {
        title: "Rule Priority",
        description: "Rule conflict and priority handling.",
        order: 320,
      },
    ],
  ].map(([source, config]) => [normalizePath(source), config]),
);

export function normalizePath(value) {
  return value.replaceAll(path.sep, "/");
}

function quoteYaml(value) {
  return JSON.stringify(value);
}

function trimMarkdownTitle(value) {
  return value
    .replace(/^\s*#+\s*/, "")
    .replace(/[`*_]/g, "")
    .trim();
}

export function titleFromMarkdown(markdown, fallback) {
  const firstHeading = markdown.match(/^#\s+(.+)$/m);
  if (!firstHeading) {
    return fallback;
  }
  return trimMarkdownTitle(firstHeading[1]) || fallback;
}

function titleFromPath(relativeSource) {
  const parsed = path.posix.parse(normalizePath(relativeSource));
  if (parsed.base === "README.md") {
    if (parsed.dir === "docs") {
      return "文档总览";
    }
    if (parsed.dir === "docs-en") {
      return "Documentation Overview";
    }
    return path.posix.basename(parsed.dir);
  }
  return parsed.name
    .split("-")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function defaultTargetForSource(source) {
  const normalized = normalizePath(source);
  const isEnglish = normalized.startsWith("docs-en/");
  const relative = normalized.replace(/^(docs|docs-en)\//, "");
  const parsed = path.posix.parse(relative);

  if (relative === "README.md") {
    return isEnglish ? "en/reference/index.md" : "reference/index.md";
  }
  if (parsed.base === "README.md") {
    return `${isEnglish ? "en/" : ""}reference/${parsed.dir}/index.md`;
  }
  return `${isEnglish ? "en/" : ""}reference/${relative}`;
}

function defaultDescription(title, source) {
  if (source.startsWith("docs-en/")) {
    return `Automatically synced English ${title} documentation from ${source}.`;
  }
  return `从 ${source} 自动同步生成的 ${title} 文档。`;
}

function defaultOrder(source, index) {
  const normalized = normalizePath(source);
  if (normalized.startsWith("docs/rules/") || normalized.startsWith("docs-en/rules/")) {
    return 600 + index;
  }
  return 900 + index;
}

function walkMarkdownFiles(rootDir, currentDir = rootDir, results = []) {
  for (const entry of fs.readdirSync(currentDir, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) {
      continue;
    }

    const entryPath = path.join(currentDir, entry.name);
    if (entry.isDirectory()) {
      walkMarkdownFiles(rootDir, entryPath, results);
      continue;
    }

    if (entry.isFile() && entry.name.endsWith(".md")) {
      results.push(normalizePath(path.relative(rootDir, entryPath)));
    }
  }
  return results;
}

export function discoverDocSourcesSync(root = docsRoot) {
  return walkMarkdownFiles(root)
    .map((relative) => `${path.basename(root)}/${relative}`)
    .sort((left, right) => left.localeCompare(right));
}

function createPage({ source, markdown, index }) {
  const normalizedSource = normalizePath(source);
  const override = knownPages.get(normalizedSource) ?? knownEnglishPages.get(normalizedSource) ?? {};
  const target = normalizePath(override.target ?? defaultTargetForSource(normalizedSource));
  const fallbackTitle = override.title ?? titleFromPath(normalizedSource);
  const title = override.title ?? titleFromMarkdown(markdown, fallbackTitle);
  const description = override.description ?? defaultDescription(title, normalizedSource);
  const order = override.order ?? defaultOrder(normalizedSource, index);

  return {
    source: normalizedSource,
    target,
    title,
    description,
    order,
  };
}

export function buildPagesSync(root = repoRoot) {
  const sourceRoots = [path.join(root, "docs"), path.join(root, "docs-en")].filter((sourceRoot) =>
    fs.existsSync(sourceRoot),
  );
  const sources = sourceRoots.flatMap((sourceRoot) => discoverDocSourcesSync(sourceRoot));
  return sources.map((source, index) => {
    const markdown = fs.readFileSync(path.join(root, source), "utf8");
    return createPage({ source, markdown, index });
  });
}

export function sourceToTargetMap(pages) {
  return new Map(pages.map((page) => [page.source, page.target]));
}

function frontmatter(page) {
  return [
    "---",
    `title: ${quoteYaml(page.title)}`,
    `description: ${quoteYaml(page.description)}`,
    "editLink: false",
    "---",
    "",
    page.source.startsWith("docs-en/")
      ? `> This page is automatically synced from \`${page.source}\`.`
      : `> 此页面由 \`${page.source}\` 自动同步生成。`,
    "",
  ].join("\n");
}

export function toRoutePath(target) {
  const normalized = normalizePath(target);
  if (normalized.endsWith("/index.md") || normalized.endsWith("/index.mdx")) {
    const dir = normalized.replace(/\/index\.mdx?$/, "");
    return dir.length === 0 ? "./" : `${dir}/`;
  }
  return normalized.replace(/\.mdx?$/, "");
}

function routeDirForTarget(target) {
  const normalized = normalizePath(target).replace(/\.mdx?$/, "");
  if (normalized.endsWith("/index")) {
    return normalized.slice(0, -"/index".length) || ".";
  }
  return path.posix.dirname(normalized);
}

export function rewriteMarkdownLinks(markdown, page, targetMap) {
  return markdown.replace(/\]\(([^)#?\s]+\.md)(#[^)]+)?\)/g, (match, rawPath, hash = "") => {
    if (
      rawPath.startsWith("http://") ||
      rawPath.startsWith("https://") ||
      rawPath.startsWith("/")
    ) {
      return match;
    }

    const sourceLink = normalizePath(path.posix.join(path.posix.dirname(page.source), rawPath));
    const mappedTarget = targetMap.get(sourceLink);
    if (!mappedTarget) {
      return match;
    }

    let relativeRoute = normalizePath(
      path.posix.relative(routeDirForTarget(page.target), toRoutePath(mappedTarget)),
    );
    if (!relativeRoute) {
      relativeRoute = ".";
    }
    if (toRoutePath(mappedTarget).endsWith("/") && !relativeRoute.endsWith("/")) {
      relativeRoute = `${relativeRoute}/`;
    }
    const routePath = relativeRoute.startsWith(".")
      ? relativeRoute
      : `./${relativeRoute}`;
    return `](${routePath}${hash})`;
  });
}

async function ensureDir(dir) {
  await fsp.mkdir(dir, { recursive: true });
}

async function listContentFiles(root, current = root, results = []) {
  let entries = [];
  try {
    entries = await fsp.readdir(current, { withFileTypes: true });
  } catch (error) {
    if (error.code === "ENOENT") {
      return results;
    }
    throw error;
  }

  for (const entry of entries) {
    const entryPath = path.join(current, entry.name);
    if (entry.isDirectory()) {
      await listContentFiles(root, entryPath, results);
      continue;
    }
    if (entry.isFile() && (entry.name.endsWith(".md") || entry.name.endsWith(".mdx"))) {
      results.push(entryPath);
    }
  }
  return results;
}

async function removeEmptyDirs(root, current = root) {
  let entries = [];
  try {
    entries = await fsp.readdir(current, { withFileTypes: true });
  } catch (error) {
    if (error.code === "ENOENT") {
      return;
    }
    throw error;
  }

  for (const entry of entries) {
    if (entry.isDirectory()) {
      await removeEmptyDirs(root, path.join(current, entry.name));
    }
  }

  if (current !== root) {
    const remaining = await fsp.readdir(current);
    if (remaining.length === 0) {
      await fsp.rmdir(current);
    }
  }
}

async function cleanGeneratedDocs(root) {
  const files = await listContentFiles(root);
  await Promise.all(
    files.map(async (file) => {
      const content = await fsp.readFile(file, "utf8");
      if (
        content.includes("<!-- Generated from docs/") ||
        /^> 此页面由 `docs\/.+` 自动同步生成。$/m.test(content) ||
        /^> This page is automatically synced from `docs-en\/.+`\.$/m.test(content)
      ) {
        await fsp.unlink(file);
      }
    }),
  );
  await removeEmptyDirs(root);
}

export async function syncDocs({
  root = repoRoot,
  docsOutputRoot = outputRoot,
} = {}) {
  const pages = buildPagesSync(root);
  const targetMap = sourceToTargetMap(pages);
  const targets = new Set();

  for (const page of pages) {
    if (targets.has(page.target)) {
      throw new Error(`Duplicate docs site target: ${page.target}`);
    }
    targets.add(page.target);
  }

  await ensureDir(docsOutputRoot);
  await cleanGeneratedDocs(docsOutputRoot);

  await Promise.all(
    pages.map(async (page) => {
      const sourcePath = path.join(root, page.source);
      const targetPath = path.join(docsOutputRoot, page.target);
      const markdown = rewriteMarkdownLinks(
        await fsp.readFile(sourcePath, "utf8"),
        page,
        targetMap,
      );

      await ensureDir(path.dirname(targetPath));
      await fsp.writeFile(
        targetPath,
        `${frontmatter(page)}${markdown.trim()}\n`,
        "utf8",
      );
    }),
  );

  const generatedAt = path.join(docsOutputRoot, ".generated-at");
  await fsp.writeFile(
    generatedAt,
    `Synced ${pages.length} docs from docs and docs-en\n`,
    "utf8",
  );

  return pages;
}

export function formatCoverageSummary(pages) {
  return pages
    .map((page) => `${page.source} -> ${page.target}`)
    .join("\n");
}
