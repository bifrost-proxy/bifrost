const tabs = Array.from(document.querySelectorAll('[role="tab"]'));
const panels = new Map(
  Array.from(document.querySelectorAll('[role="tabpanel"]')).map((panel) => [panel.id, panel]),
);
const languageButtons = Array.from(document.querySelectorAll("[data-lang]"));
const translations = {
  en: {
    navDocs: "Docs",
    navInstall: "Install",
    navEnglish: "English",
    heroEyebrow: "HTTP / HTTPS / SOCKS5 / WebSocket / Replay",
    heroText:
      "A proxy workbench for capturing traffic, rewriting requests, replaying failures, and debugging real applications from the terminal, desktop, or browser.",
    primaryCta: "Install",
    secondaryCta: "Read Docs",
    terminalReady: "Admin UI ready at http://127.0.0.1:8800/_bifrost/",
    terminalOk: "24 matching requests · latest 200 OK · replay available",
    workflowTitle: "One proxy, several honest debugging paths.",
    workflowOneTitle: "Capture the real request",
    workflowOneText: "Inspect headers, bodies, cookies, streaming messages, and the listener port that saw it.",
    workflowTwoTitle: "Rewrite without rebuilding",
    workflowTwoText: "Patch headers, bodies, URLs, status codes, latency, and routing with local rules.",
    workflowThreeTitle: "Replay the failure",
    workflowThreeText: "Turn a captured request into a repeatable curl, HAR, or replay run while you iterate.",
    workflowFourTitle: "Automate the edge cases",
    workflowFourText: "Use scripts and values when static rules are not enough for the flow you are chasing.",
    startTitle: "From zero to inspected traffic in four steps.",
    stepOne: "Install the CLI",
    stepTwo: "Start without touching system proxy",
    stepThree: "Add a rule for one target",
    stepFour: "Replay and compare",
  },
  zh: {
    navDocs: "文档",
    navInstall: "安装",
    navEnglish: "English",
    heroEyebrow: "HTTP / HTTPS / SOCKS5 / WebSocket / 回放",
    heroText: "Bifrost 是一个代理工作台，用来抓取流量、改写请求、回放故障，并在终端、桌面端和浏览器里调试真实应用。",
    primaryCta: "开始安装",
    secondaryCta: "阅读文档",
    terminalReady: "管理端已就绪：http://127.0.0.1:8800/_bifrost/",
    terminalOk: "24 条匹配请求 · 最新 200 OK · 可直接回放",
    workflowTitle: "一个代理，多条扎实的调试路径。",
    workflowOneTitle: "抓到真实请求",
    workflowOneText: "查看 headers、body、cookie、流式消息，以及捕获请求的监听端口。",
    workflowTwoTitle: "不用重建也能改写",
    workflowTwoText: "用本地规则调整 headers、body、URL、状态码、延迟和路由。",
    workflowThreeTitle: "把故障变成可回放样本",
    workflowThreeText: "把一次抓到的请求变成可重复执行的 curl、HAR 或 replay run。",
    workflowFourTitle: "自动化边界场景",
    workflowFourText: "当静态规则不够时，用 scripts 和 values 编排更复杂的调试流程。",
    startTitle: "四步从零开始看到真实流量。",
    stepOne: "安装 CLI",
    stepTwo: "不接管系统代理启动",
    stepThree: "为目标添加一条规则",
    stepFour: "回放并对比结果",
  },
};

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
  const dictionary = translations[language] ?? translations.en;
  document.documentElement.lang = language === "zh" ? "zh-CN" : "en";
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const value = dictionary[node.dataset.i18n];
    if (value) {
      node.textContent = value;
    }
  }
  for (const button of languageButtons) {
    button.setAttribute("aria-pressed", String(button.dataset.lang === language));
  }
}

for (const button of languageButtons) {
  button.addEventListener("click", () => setLanguage(button.dataset.lang));
}

if (navigator.language?.toLowerCase().startsWith("zh")) {
  setLanguage("zh");
}
