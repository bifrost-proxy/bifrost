import { fileURLToPath } from "node:url";
import path from "node:path";
import { defineConfig } from "vitepress";

import { buildPagesSync, normalizePath } from "../scripts/docs-sync-lib.mjs";

const siteRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(siteRoot, "..");
const site = process.env.SITE_URL ?? "https://bifrost-proxy.github.io/bifrost";

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
  const siteUrl = new URL(site);
  return normalizeBasePath(siteUrl.hostname.endsWith("github.io") ? siteUrl.pathname : "/");
}

const pages = buildPagesSync(repoRoot).sort((left, right) => left.order - right.order);
const basePath = defaultBasePath();

function pageLink(target) {
  const normalized = normalizePath(target).replace(/\.mdx?$/, "");
  if (normalized.endsWith("/index")) {
    return `/${normalized.slice(0, -"/index".length)}/`;
  }
  return `/${normalized}`;
}

function sectionItems({ locale = "zh", prefix, excludePrefix }) {
  return pages
    .filter((page) => {
      const target = normalizePath(page.target);
      const matchesPrefix = target.startsWith(prefix);
      const notExcluded = !excludePrefix || !target.startsWith(excludePrefix);
      const matchesLocale = locale === "en" ? target.startsWith("en/") : !target.startsWith("en/");
      return matchesPrefix && notExcluded && matchesLocale;
    })
    .map((page) => ({
      text: page.title,
      link: pageLink(page.target),
    }));
}

const zhSidebar = [
  {
    text: "开始使用 / Getting Started",
    collapsed: false,
    items: sectionItems({ prefix: "getting-started/" }),
  },
  {
    text: "参考文档 / Reference",
    collapsed: false,
    items: sectionItems({ prefix: "reference/", excludePrefix: "reference/rules/" }),
  },
  {
    text: "Rules",
    collapsed: true,
    items: sectionItems({ prefix: "reference/rules/" }),
  },
];

const enSidebar = [
  {
    text: "Getting Started",
    collapsed: false,
    items: sectionItems({ locale: "en", prefix: "en/getting-started/" }),
  },
  {
    text: "Reference",
    collapsed: false,
    items: sectionItems({
      locale: "en",
      prefix: "en/reference/",
      excludePrefix: "en/reference/rules/",
    }),
  },
  {
    text: "Rules",
    collapsed: true,
    items: sectionItems({ locale: "en", prefix: "en/reference/rules/" }),
  },
];

export default defineConfig({
  lang: "zh-CN",
  title: "Bifrost",
  description: "Bifrost documentation site for Chinese and English readers",
  base: basePath,
  srcDir: "src/content/docs",
  outDir: "dist",
  cacheDir: ".vitepress/cache",
  cleanUrls: true,
  lastUpdated: false,
  appearance: true,
  vite: {
    build: {
      target: "esnext",
    },
  },
  ignoreDeadLinks: [
    /^https?:\/\//,
    /^mailto:/,
    /^tel:/,
    /^\/bifrost\/$/,
    /^\/bifrost\/docs\/$/,
  ],
  head: [
    ["link", { rel: "icon", href: `${basePath}favicon.png` }],
    ["link", { rel: "alternate", hreflang: "zh-CN", href: `${basePath}docs/` }],
    ["link", { rel: "alternate", hreflang: "en-US", href: `${basePath}en/reference/` }],
  ],
  locales: {
    root: {
      label: "中文",
      lang: "zh-CN",
    },
    en: {
      label: "English",
      lang: "en-US",
      link: "/en/getting-started/overview",
    },
  },
  themeConfig: {
    i18nRouting: false,
    logo: "/favicon.png",
    logoLink: {
      link: basePath,
      target: "_self",
    },
    search: {
      provider: "local",
      options: {
        locales: {
          root: {
            translations: {
              button: { buttonText: "搜索", buttonAriaLabel: "搜索" },
              modal: { displayDetails: "显示详情", resetButtonTitle: "重置搜索" },
            },
          },
        },
      },
    },
    nav: [
      { text: "文档", link: "/docs/" },
      { text: "安装", link: "/getting-started/installation" },
      { text: "English", link: "/en/getting-started/overview" },
      { text: "GitHub", link: "https://github.com/bifrost-proxy/bifrost" },
    ],
    sidebar: {
      "/docs/": zhSidebar,
      "/getting-started/": zhSidebar,
      "/reference/": zhSidebar,
      "/en/getting-started/": enSidebar,
      "/en/reference/": enSidebar,
    },
    outline: {
      label: "本页内容",
      level: [2, 3],
    },
    editLink: {
      pattern: "https://github.com/bifrost-proxy/bifrost/edit/main/:path",
      text: "编辑此页",
    },
    docFooter: {
      prev: "上一页",
      next: "下一页",
    },
    darkModeSwitchLabel: "外观",
    sidebarMenuLabel: "菜单",
    returnToTopLabel: "回到顶部",
    socialLinks: [
      {
        icon: "github",
        link: "https://github.com/bifrost-proxy/bifrost",
        ariaLabel: "GitHub",
      },
    ],
  },
});
