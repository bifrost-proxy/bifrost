const tabs = Array.from(document.querySelectorAll('[role="tab"]'));
const panels = new Map(
  Array.from(document.querySelectorAll('[role="tabpanel"]')).map((panel) => [panel.id, panel]),
);
const languageButtons = Array.from(document.querySelectorAll("[data-lang]"));
const downloadCards = Array.from(document.querySelectorAll("[data-download-target]"));
const downloadStatus = document.querySelector("[data-download-status]");
const downloadMessage = document.querySelector("[data-download-message]");
const desktopDownloads = readDesktopDownloads();
const translations = {
  en: {
    heroEyebrow: "AI-era proxy / Coding Agent ready",
    heroText:
      "A one-stop proxy solution for the AI era. Capture real traffic, rewrite and replay requests, then hand the evidence to your Coding Agent so it can build useful skills.",
    primaryCta: "Install",
    desktopCta: "Desktop App",
    secondaryCta: "Read Docs",
    terminalReady: "Admin UI ready at http://127.0.0.1:8800/_bifrost/",
    terminalOk: "24 matching requests · latest 200 OK · replay available",
    aiStepOneTitle: "Install the skill",
    aiStepTwoTitle: "Capture real traffic",
    aiCaseLabel: "Agent case",
    aiCaseText:
      "“Help me capture the login API, understand the request and response shape, then implement a reusable Coding Agent skill for refreshing sessions.”",
    aiResult: "Traffic evidence becomes rules, replay scripts, docs, and a working skill.",
    appsTitle: "Download desktop apps",
    appsSubtitle: "Current release, direct download.",
    appsLoading: "Preparing current release links...",
    appsReady: "Ready. Click an app to download the current package.",
    appsError: "Download links are not available in this build.",
    macArmTitle: "macOS Apple Silicon",
    macArmText: "M1 / M2 / M3 / M4",
    macIntelTitle: "macOS Intel",
    macIntelText: "Intel Mac",
    windowsTitle: "Windows",
    windowsText: "x64 installer",
    windowsArmTitle: "Windows ARM64",
    windowsArmText: "ARM installer",
    workflowTitle: "One proxy layer between real traffic and useful AI work.",
    workflowOneTitle: "Give agents real context",
    workflowOneText: "Capture headers, bodies, cookies, streams, and replay material instead of describing APIs by memory.",
    workflowTwoTitle: "Turn traffic into skills",
    workflowTwoText: "Let a Coding Agent read traffic evidence, infer flows, and package the result as a reusable skill.",
    workflowThreeTitle: "Replay before changing code",
    workflowThreeText: "Convert captured requests into reproducible curl, HAR, and replay runs before the agent edits anything.",
    workflowFourTitle: "Debug the edge cases",
    workflowFourText: "Combine rules, scripts, values, and TLS controls when production-like behavior needs precise simulation.",
    installEyebrow: "Install",
    installTitle: "Choose CLI precision or desktop comfort.",
    cliInstallLabel: "Terminal workflow",
    cliInstallTitle: "Install the CLI",
    cliInstallText: "Best for scripts, CI, terminals, and Coding Agents that need reproducible traffic evidence.",
    cliInstallLink: "CLI install guide",
    desktopInstallLabel: "Visual workflow",
    desktopInstallTitle: "Install the desktop app",
    desktopInstallText: "Download the macOS DMG or Windows MSI, launch Bifrost, then inspect traffic and update from the app.",
    desktopInstallLink: "Desktop install guide",
    startTitle: "From zero to inspected traffic in four steps.",
    stepOne: "Install CLI or Desktop",
    stepTwo: "Start the daemon",
    stepThree: "Add a rule for one target",
    stepFour: "Replay and compare",
  },
  zh: {
    heroEyebrow: "AI 时代代理方案 / 适配 Coding Agent",
    heroText: "Bifrost 是 AI 时代的一站式代理解决方案。抓取真实流量、改写和回放请求，再把证据交给 Coding Agent，让它沉淀可复用的技能。",
    primaryCta: "开始安装",
    desktopCta: "桌面端 App",
    secondaryCta: "阅读文档",
    terminalReady: "管理端已就绪：http://127.0.0.1:8800/_bifrost/",
    terminalOk: "24 条匹配请求 · 最新 200 OK · 可直接回放",
    aiStepOneTitle: "安装技能",
    aiStepTwoTitle: "抓取真实流量",
    aiCaseLabel: "Agent 示例",
    aiCaseText: "“帮我抓取登录接口，理解请求和响应结构，然后实现一个可复用的 Coding Agent 会话刷新技能。”",
    aiResult: "真实流量会变成规则、回放脚本、文档和可运行的技能。",
    appsTitle: "下载桌面端 App",
    appsSubtitle: "按当前发布版本生成直链，点击直接下载。",
    appsLoading: "正在准备当前版本下载链接...",
    appsReady: "已就绪，点击应用卡片即可下载当前安装包。",
    appsError: "当前构建未包含下载链接。",
    macArmTitle: "macOS Apple Silicon",
    macArmText: "M1 / M2 / M3 / M4",
    macIntelTitle: "macOS Intel",
    macIntelText: "Intel Mac",
    windowsTitle: "Windows",
    windowsText: "x64 安装包",
    windowsArmTitle: "Windows ARM64",
    windowsArmText: "ARM 安装包",
    workflowTitle: "把真实流量和有用的 AI 工作接到一起。",
    workflowOneTitle: "给 Agent 真实上下文",
    workflowOneText: "抓取 headers、body、cookie、流式消息和可回放材料，不再靠记忆描述接口。",
    workflowTwoTitle: "把流量沉淀为技能",
    workflowTwoText: "让 Coding Agent 读取真实证据、理解业务流程，并包装成可复用的技能。",
    workflowThreeTitle: "改代码前先回放",
    workflowThreeText: "把抓到的请求转成可重复执行的 curl、HAR 和 replay run，再让 Agent 迭代。",
    workflowFourTitle: "调试复杂边界",
    workflowFourText: "用规则、脚本、Values 和 TLS 控制组合出更接近真实生产行为的场景。",
    installEyebrow: "安装",
    installTitle: "选择 CLI 的精确控制，或桌面端的顺手体验。",
    cliInstallLabel: "终端工作流",
    cliInstallTitle: "安装 CLI",
    cliInstallText: "适合脚本、CI、终端和需要真实流量证据的 Coding Agent。",
    cliInstallLink: "CLI 安装教程",
    desktopInstallLabel: "可视化工作流",
    desktopInstallTitle: "安装桌面端 App",
    desktopInstallText: "下载 macOS DMG 或 Windows MSI，启动后直接查看流量、编辑规则并完成更新。",
    desktopInstallLink: "桌面端安装教程",
    startTitle: "四步从零开始看到真实流量。",
    stepOne: "安装 CLI 或桌面端",
    stepTwo: "后台启动服务",
    stepThree: "为目标添加一条规则",
    stepFour: "回放并对比结果",
  },
};

let currentLanguage = "en";

function activateTab(tab) {
  for (const current of tabs) {
    const selected = current === tab;
    current.setAttribute("aria-selected", String(selected));
    const panel = panels.get(current.getAttribute("aria-controls"));
    if (panel) {
      panel.hidden = !selected;
      panel.classList.toggle("is-active", selected);
    }
  }
}

for (const tab of tabs) {
  tab.addEventListener("click", () => activateTab(tab));
  tab.addEventListener("keydown", (event) => {
    const index = tabs.indexOf(tab);
    const nextIndex =
      event.key === "ArrowRight"
        ? (index + 1) % tabs.length
        : event.key === "ArrowLeft"
          ? (index - 1 + tabs.length) % tabs.length
          : -1;

    if (nextIndex >= 0) {
      event.preventDefault();
      tabs[nextIndex].focus();
      activateTab(tabs[nextIndex]);
    }
  });
}

function setLanguage(language) {
  currentLanguage = translations[language] ? language : "en";
  const dictionary = translations[currentLanguage] ?? translations.en;
  document.documentElement.lang = currentLanguage === "zh" ? "zh-CN" : "en";
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const value = dictionary[node.dataset.i18n];
    if (value) {
      node.textContent = value;
    }
  }
  for (const button of languageButtons) {
    button.setAttribute("aria-pressed", String(button.dataset.lang === currentLanguage));
  }
}

function setDownloadMessage(key) {
  if (!downloadMessage) {
    return;
  }
  const dictionary = translations[currentLanguage] ?? translations.en;
  downloadMessage.textContent = dictionary[key] ?? translations.en[key] ?? "";
}

function readDesktopDownloads() {
  const script = document.getElementById("desktop-downloads-data");
  if (!script?.textContent) {
    return null;
  }
  try {
    const data = JSON.parse(script.textContent);
    if (!data?.baseUrl || !data?.assets) {
      return null;
    }
    return data;
  } catch {
    return null;
  }
}

function assetUrl(target) {
  const asset = desktopDownloads?.assets?.[target];
  if (!asset) {
    return null;
  }
  return new URL(asset, desktopDownloads.baseUrl).href;
}

function applyDownloads() {
  let readyCount = 0;
  for (const card of downloadCards) {
    const href = assetUrl(card.dataset.downloadTarget);
    if (!href) {
      card.href = "#";
      card.setAttribute("aria-disabled", "true");
      continue;
    }
    card.href = href;
    card.setAttribute("download", "");
    card.removeAttribute("aria-disabled");
    readyCount += 1;
  }

  if (downloadStatus) {
    downloadStatus.dataset.downloadStatus = readyCount > 0 ? "ready" : "error";
  }
  setDownloadMessage(readyCount > 0 ? "appsReady" : "appsError");
}

function resolveDownloads() {
  if (downloadCards.length === 0) {
    return;
  }
  applyDownloads();
}

for (const card of downloadCards) {
  card.addEventListener("click", (event) => {
    if (card.getAttribute("aria-disabled") === "true") {
      event.preventDefault();
    }
  });
}

for (const button of languageButtons) {
  button.addEventListener("click", () => setLanguage(button.dataset.lang));
}

if (navigator.language?.toLowerCase().startsWith("zh")) {
  setLanguage("zh");
}

resolveDownloads();
