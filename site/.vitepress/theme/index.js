import DefaultTheme from "vitepress/theme";
import "./style.css";

const releaseApiUrl = "https://api.github.com/repos/bifrost-proxy/bifrost/releases/latest";
const downloadTargets = {
  "mac-arm": /bifrost-desktop-v.+-aarch64-apple-darwin\.dmg$/,
  "mac-intel": /bifrost-desktop-v.+-x86_64-apple-darwin\.dmg$/,
  "win-x64": /bifrost-desktop-v.+-x86_64-pc-windows-msvc\.msi$/,
  "win-arm": /bifrost-desktop-v.+-aarch64-pc-windows-msvc\.msi$/,
};

let releaseAssetsPromise;

function getReleaseAssets() {
  releaseAssetsPromise ??= fetch(releaseApiUrl, {
    headers: { Accept: "application/vnd.github+json" },
  })
    .then((response) => {
      if (!response.ok) {
        throw new Error(`GitHub release API returned ${response.status}`);
      }
      return response.json();
    })
    .then((release) => (Array.isArray(release.assets) ? release.assets : []));
  return releaseAssetsPromise;
}

function setDownloadMessage(message) {
  for (const node of document.querySelectorAll("[data-vp-download-message]")) {
    node.textContent = message;
  }
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

  getReleaseAssets()
    .then((assets) => {
      let readyCount = 0;
      for (const card of cards) {
        const matcher = downloadTargets[card.dataset.vpDownloadTarget];
        const asset = assets.find((candidate) => matcher?.test(candidate.name));
        if (!asset?.browser_download_url) {
          card.href = "#";
          card.setAttribute("aria-disabled", "true");
          continue;
        }
        card.href = asset.browser_download_url;
        card.setAttribute("download", "");
        card.removeAttribute("aria-disabled");
        readyCount += 1;
      }
      for (const grid of document.querySelectorAll("[data-vp-download-status]")) {
        grid.dataset.vpDownloadStatus = readyCount > 0 ? "ready" : "error";
      }
      setDownloadMessage(
        readyCount > 0
          ? "已就绪，点击应用卡片即可下载最新安装包。 / Ready. Click a card to download the latest package."
          : "暂时无法解析最新安装包，请稍后再试。 / Could not resolve the latest packages. Please try again later.",
      );
    })
    .catch(() => {
      for (const card of cards) {
        card.href = "#";
        card.setAttribute("aria-disabled", "true");
        card.removeAttribute("download");
      }
      for (const grid of document.querySelectorAll("[data-vp-download-status]")) {
        grid.dataset.vpDownloadStatus = "error";
      }
      setDownloadMessage(
        "暂时无法解析最新安装包，请稍后再试。 / Could not resolve the latest packages. Please try again later.",
      );
    });
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
