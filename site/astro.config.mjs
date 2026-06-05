import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

const site = process.env.SITE_URL ?? "https://bifrost-proxy.github.io/bifrost";
const siteUrl = new URL(site);
const base =
  process.env.BASE_PATH ??
  (siteUrl.hostname.endsWith("github.io") ? siteUrl.pathname || "/" : "/");
const withBase = (route) => `${base === "/" ? "" : base}${route}`;
export default defineConfig({
  site,
  base,
  redirects: {
    "/reference/getting-started/overview/": withBase("/getting-started/overview/"),
    "/reference/getting-started/installation/": withBase("/getting-started/installation/"),
    "/reference/getting-started/cli-quick-start/":
      withBase("/getting-started/cli-quick-start/"),
    "/reference/getting-started/desktop/": withBase("/getting-started/desktop/"),
  },
  integrations: [
    starlight({
      title: "Bifrost",
      description: "Bifrost documentation site for Chinese and English readers",
      favicon: "/favicon.png",
      disable404Route: true,
      locales: {
        root: {
          label: "中文",
          lang: "zh-CN",
        },
        en: {
          label: "English",
          lang: "en-US",
        },
      },
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/bifrost-proxy/bifrost",
        },
      ],
      editLink: {
        baseUrl:
          "https://github.com/bifrost-proxy/bifrost/edit/main/site/src/content/docs/",
      },
      customCss: ["./src/styles/starlight.css"],
      sidebar: [
        {
          label: "开始使用 / Getting Started",
          autogenerate: { directory: "getting-started" },
        },
        {
          label: "参考文档 / Reference",
          autogenerate: { directory: "reference" },
        },
      ],
      head: [
        {
          tag: "link",
          attrs: { rel: "alternate", hreflang: "zh-CN", href: withBase("/docs/") },
        },
        {
          tag: "link",
          attrs: { rel: "alternate", hreflang: "en-US", href: withBase("/en/reference/") },
        },
      ],
    }),
  ],
  vite: {
    server: {
      fs: {
        allow: [".."],
      },
    },
  },
});
