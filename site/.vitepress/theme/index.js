import DefaultTheme from "vitepress/theme";
import desktopDownloads from "../../desktop-downloads.json";
import "./style.css";

function setDownloadMessage(message) {
  for (const node of document.querySelectorAll("[data-vp-download-message]")) {
    node.textContent = message;
  }
}

function assetUrl(target) {
  const asset = desktopDownloads.assets?.[target];
  if (!asset) {
    return null;
  }
  return new URL(asset, desktopDownloads.baseUrl).href;
}

function bindDesktopDownloads() {
  const cards = Array.from(document.querySelectorAll("[data-vp-download-target]"));
  if (cards.length === 0) {
    return;
  }

  for (const card of cards) {
    if (card.dataset.vpDownloadBound === "true") {
      continue;
    }
    card.dataset.vpDownloadBound = "true";
    card.addEventListener("click", (event) => {
      if (card.getAttribute("aria-disabled") === "true") {
        event.preventDefault();
      }
    });
  }

  let readyCount = 0;
  for (const card of cards) {
    const href = assetUrl(card.dataset.vpDownloadTarget);
    if (!href) {
      card.href = "#";
      card.setAttribute("aria-disabled", "true");
      card.removeAttribute("download");
      continue;
    }
    card.href = href;
    card.setAttribute("download", "");
    card.removeAttribute("aria-disabled");
    readyCount += 1;
  }
  for (const grid of document.querySelectorAll("[data-vp-download-status]")) {
    grid.dataset.vpDownloadStatus = readyCount > 0 ? "ready" : "error";
  }
  setDownloadMessage(
    readyCount > 0
      ? `已就绪，点击应用卡片即可下载 ${desktopDownloads.tag} 安装包。 / Ready. Click a card to download ${desktopDownloads.tag}.`
      : "当前构建未包含下载链接。 / Download links are not available in this build.",
  );
}

export default {
  ...DefaultTheme,
  enhanceApp(ctx) {
    DefaultTheme.enhanceApp?.(ctx);
    if (typeof window === "undefined") {
      return;
    }
    window.requestAnimationFrame(bindDesktopDownloads);
    const previousAfterRouteChanged = ctx.router.onAfterRouteChanged;
    ctx.router.onAfterRouteChanged = async (to) => {
      await previousAfterRouteChanged?.(to);
      window.requestAnimationFrame(bindDesktopDownloads);
    };
  },
};
