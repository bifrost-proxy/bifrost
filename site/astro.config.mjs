import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

const site = process.env.SITE_URL ?? "https://bifrost-proxy.github.io/bifrost";
const siteUrl = new URL(site);
const base =
  process.env.BASE_PATH ??
  (siteUrl.hostname.endsWith("github.io") ? siteUrl.pathname || "/" : "/");

export default defineConfig({
  site,
  base,
  integrations: [
    starlight({
      title: "Bifrost",
      description: "高性能 HTTP/HTTPS/SOCKS5 代理服务器的官网与文档站",
      disable404Route: true,
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
          label: "开始使用",
          autogenerate: { directory: "getting-started" },
        },
        {
          label: "参考文档",
          autogenerate: { directory: "reference" },
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
