#!/usr/bin/env node

const puppeteer = require("puppeteer");
const http = require("http");
const net = require("net");
const { randomUUID } = require("crypto");
const { DEFAULT_PORT } = require("./lib/config.js");

function parseArgs(args) {
  const options = {
    port: DEFAULT_PORT,
    target: null,
    page: "/_bifrost/traffic",
    duration: 5000,
    headless: true,
    expectTrafficSubscription: true,
    apiCheck: true,
    seed: null,
    verbose: false,
    help: false,
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case "-h":
      case "--help":
        options.help = true;
        break;
      case "-p":
      case "--port":
        options.port = parseInt(args[++i], 10) || DEFAULT_PORT;
        break;
      case "-t":
      case "--target":
        options.target = args[++i];
        break;
      case "--page":
        options.page = args[++i] || options.page;
        break;
      case "--duration":
        options.duration = parseInt(args[++i], 10) || options.duration;
        break;
      case "--headful":
        options.headless = false;
        break;
      case "--no-api-check":
        options.apiCheck = false;
        break;
      case "--seed":
        options.seed = args[++i] || null;
        break;
      case "--no-expect-traffic-subscription":
        options.expectTrafficSubscription = false;
        break;
      case "-v":
      case "--verbose":
        options.verbose = true;
        break;
    }
  }

  return options;
}

function showHelp() {
  console.log(`
push-debug - 最小化 push / websocket 排查工具

Usage:
  node push-debug.js [options]

Options:
  -p, --port <port>                     管理端端口 (默认: ${DEFAULT_PORT})
  -t, --target <url>                    目标基地址，例如 http://127.0.0.1:9910
  --page <path>                         要打开的页面路径 (默认: /_bifrost/traffic)
  --duration <ms>                       打开页面后的抓取时长 (默认: 5000)
  --headful                             使用可见浏览器
  --seed <http|connect>                 启动最小本地服务并通过代理造一条流量
  --no-api-check                        跳过启动后的 API ready 检查
  --no-expect-traffic-subscription      不强制检查 need_traffic=true
  -v, --verbose                         输出更多调试信息
  -h, --help                            显示帮助

Examples:
  node push-debug.js -p 9910
  node push-debug.js -t http://127.0.0.1:9910 --duration 8000
  node push-debug.js -p 9910 --seed http
  node push-debug.js -p 9910 --seed connect --duration 8000
  node push-debug.js -p 9910 --page /_bifrost/rules --no-expect-traffic-subscription
`);
}

function isInterestingApi(url) {
  return (
    url.includes("/_bifrost/api/traffic") ||
    url.includes("/_bifrost/api/proxy/address") ||
    url.includes("/_bifrost/api/system/overview")
  );
}

function isInterestingPushFrame(payload) {
  return (
    payload.includes("traffic_") ||
    payload.includes("connected") ||
    payload.includes("need_traffic") ||
    payload.includes("overview_update") ||
    payload.includes("metrics_update")
  );
}

async function ensureApiReady(baseUrl) {
  const url = `${baseUrl}/_bifrost/api/proxy/address`;
  let response;
  try {
    response = await fetch(url, { signal: AbortSignal.timeout(5000) });
  } catch (error) {
    throw new Error(
      `无法连接管理端 API: ${url}，请先确认代理已启动并且端口可访问 (${error.message})`,
    );
  }
  if (!response.ok) {
    throw new Error(
      `管理端 API 未 ready: ${response.status} ${response.statusText} (${url})`,
    );
  }
  return response.json();
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function fetchTrafficList(baseUrl) {
  const response = await fetch(`${baseUrl}/_bifrost/api/traffic?limit=10`, {
    signal: AbortSignal.timeout(5000),
  });
  if (!response.ok) {
    throw new Error(`traffic list failed: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

async function waitForTrafficMatch(baseUrl, predicate, timeoutMs = 8000) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const payload = await fetchTrafficList(baseUrl);
    const record = (payload.records || []).find(predicate);
    if (record) {
      return record;
    }
    await sleep(250);
  }
  return null;
}

async function withHttpSeed(proxyPort) {
  const path = `/push-debug-${randomUUID()}`;
  const server = http.createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ ok: true, path: req.url }));
  });

  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const targetPort = server.address().port;

  const cleanup = async () => {
    await new Promise((resolve) => server.close(() => resolve()));
  };

  const trigger = async () => {
    await new Promise((resolve, reject) => {
      const req = http.request(
        {
          host: "127.0.0.1",
          port: proxyPort,
          method: "GET",
          path: `http://127.0.0.1:${targetPort}${path}`,
          headers: {
            Host: `127.0.0.1:${targetPort}`,
            Connection: "close",
          },
        },
        (res) => {
          res.resume();
          res.on("end", resolve);
        },
      );
      req.on("error", reject);
      req.end();
    });
  };

  return {
    description: `HTTP GET 127.0.0.1:${targetPort}${path}`,
    cleanup,
    trigger,
    matchRecord: (record) =>
      record.m === "GET" && record.h === "127.0.0.1" && record.p === path,
  };
}

async function withConnectSeed(proxyPort) {
  const echoServer = net.createServer((socket) => {
    socket.on("data", (chunk) => socket.write(chunk));
  });

  await new Promise((resolve) => echoServer.listen(0, "127.0.0.1", resolve));
  const targetPort = echoServer.address().port;

  const cleanup = async () => {
    await new Promise((resolve) => echoServer.close(() => resolve()));
  };

  const trigger = async () => {
    const socket = new net.Socket();
    await new Promise((resolve, reject) => {
      socket.connect(proxyPort, "127.0.0.1", resolve);
      socket.once("error", reject);
    });

    socket.write(
      `CONNECT 127.0.0.1:${targetPort} HTTP/1.1\r\nHost: 127.0.0.1:${targetPort}\r\nProxy-Connection: Keep-Alive\r\n\r\n`,
    );

    let buffer = "";
    await new Promise((resolve, reject) => {
      const onData = (chunk) => {
        buffer += chunk.toString("utf8");
        if (!buffer.includes("\r\n\r\n")) return;
        cleanupListeners();
        if (!buffer.startsWith("HTTP/1.1 200") && !buffer.startsWith("HTTP/1.0 200")) {
          reject(new Error(`unexpected CONNECT response: ${buffer}`));
          return;
        }
        resolve();
      };
      const onError = (error) => {
        cleanupListeners();
        reject(error);
      };
      const onClose = () => {
        cleanupListeners();
        reject(new Error("CONNECT tunnel closed before establishment"));
      };
      const cleanupListeners = () => {
        socket.off("data", onData);
        socket.off("error", onError);
        socket.off("close", onClose);
      };
      socket.on("data", onData);
      socket.on("error", onError);
      socket.on("close", onClose);
    });

    socket.write("push-debug-connect");
    await new Promise((resolve, reject) => {
      const onData = (chunk) => {
        if (chunk.toString("utf8").includes("push-debug-connect")) {
          cleanupListeners();
          resolve();
        }
      };
      const onError = (error) => {
        cleanupListeners();
        reject(error);
      };
      const cleanupListeners = () => {
        socket.off("data", onData);
        socket.off("error", onError);
      };
      socket.on("data", onData);
      socket.on("error", onError);
    });

    socket.destroy();
  };

  return {
    description: `CONNECT 127.0.0.1:${targetPort}`,
    cleanup,
    trigger,
    matchRecord: (record) =>
      record.m === "CONNECT" && record.h === "127.0.0.1" && record.proto === "tunnel",
  };
}

async function runPushDebug(options) {
  const baseUrl = options.target || `http://127.0.0.1:${options.port}`;
  const pageUrl = `${baseUrl}${options.page.startsWith("/") ? options.page : `/${options.page}`}`;

  console.log("\n🔎 Push 最小化排查");
  console.log("=".repeat(50));
  console.log(`目标服务: ${baseUrl}`);
  console.log(`页面地址: ${pageUrl}`);

  if (options.apiCheck) {
    const proxyInfo = await ensureApiReady(baseUrl);
    console.log("API ready: yes");
    if (options.verbose) {
      console.log(`代理地址信息: ${JSON.stringify(proxyInfo)}`);
    }
  }

  const browser = await puppeteer.launch({
    headless: options.headless,
    defaultViewport: { width: 1440, height: 900 },
  });

  const page = await browser.newPage();
  const client = await page.target().createCDPSession();
  await client.send("Network.enable");

  const summary = {
    pushSocketUrl: null,
    sentNeedTraffic: false,
    receivedTrafficDelta: false,
    receivedTrafficUpdates: false,
    apiRequests: [],
    sentFrames: [],
    receivedFrames: [],
    seededTraffic: null,
    matchedTrafficRecord: null,
  };

  const wsRequestMap = new Map();

  page.on("requestfinished", async (request) => {
    const url = request.url();
    if (!isInterestingApi(url)) return;
    summary.apiRequests.push(`${request.method()} ${url}`);
    console.log(`API ${request.method()} ${url}`);
  });

  client.on("Network.webSocketCreated", (event) => {
    wsRequestMap.set(event.requestId, event.url);
    if (event.url.includes("/api/push")) {
      summary.pushSocketUrl = event.url;
      console.log(`WS OPEN ${event.url}`);
    } else if (options.verbose) {
      console.log(`WS OPEN ${event.url}`);
    }
  });

  client.on("Network.webSocketFrameSent", (event) => {
    const url = wsRequestMap.get(event.requestId) || "";
    const payload = event.response?.payloadData || "";
    if (!url.includes("/api/push") || !isInterestingPushFrame(payload)) return;
    summary.sentFrames.push(payload);
    if (payload.includes('"need_traffic":true')) {
      summary.sentNeedTraffic = true;
    }
    console.log(`WS SENT ${payload.slice(0, 500)}`);
  });

  client.on("Network.webSocketFrameReceived", (event) => {
    const url = wsRequestMap.get(event.requestId) || "";
    const payload = event.response?.payloadData || "";
    if (!url.includes("/api/push") || !isInterestingPushFrame(payload)) return;
    summary.receivedFrames.push(payload);
    if (payload.includes('"type":"traffic_delta"')) {
      summary.receivedTrafficDelta = true;
    }
    if (payload.includes('"type":"traffic_updates"')) {
      summary.receivedTrafficUpdates = true;
    }
    console.log(`WS RECV ${payload.slice(0, 500)}`);
  });

  let seedSession = null;
  try {
    await page.goto(pageUrl, { waitUntil: "networkidle2" });
    await page.waitForSelector('[data-testid="traffic-table"], body', { timeout: 10000 });

    if (options.seed) {
      if (options.seed === "http") {
        seedSession = await withHttpSeed(options.port);
      } else if (options.seed === "connect") {
        seedSession = await withConnectSeed(options.port);
      } else {
        throw new Error(`unsupported seed type: ${options.seed}`);
      }

      summary.seededTraffic = seedSession.description;
      console.log(`SEED ${seedSession.description}`);
      await seedSession.trigger();
      summary.matchedTrafficRecord = await waitForTrafficMatch(baseUrl, seedSession.matchRecord);
      if (summary.matchedTrafficRecord) {
        console.log(`API MATCH ${JSON.stringify(summary.matchedTrafficRecord)}`);
      } else {
        console.log("API MATCH none");
      }
    }

    await new Promise((resolve) => setTimeout(resolve, options.duration));
  } finally {
    if (seedSession) {
      await seedSession.cleanup();
    }
    await browser.close();
  }

  console.log("\n📌 摘要");
  console.log(`push socket: ${summary.pushSocketUrl || "none"}`);
  console.log(`sent need_traffic=true: ${summary.sentNeedTraffic ? "yes" : "no"}`);
  console.log(`received traffic_delta: ${summary.receivedTrafficDelta ? "yes" : "no"}`);
  console.log(`received traffic_updates: ${summary.receivedTrafficUpdates ? "yes" : "no"}`);
  console.log(`seeded traffic: ${summary.seededTraffic || "no"}`);
  console.log(`matched API record: ${summary.matchedTrafficRecord ? "yes" : "no"}`);

  const hints = [];
  if (!summary.pushSocketUrl) {
    hints.push("页面没有建立 /api/push websocket，先检查页面初始化和服务端可用性。");
  }
  if (options.expectTrafficSubscription && !summary.sentNeedTraffic) {
    hints.push("页面没有向服务端发送 need_traffic=true，优先检查 push 订阅合并与 CONNECTING->OPEN 竞态。");
  }
  if (summary.sentNeedTraffic && !summary.receivedTrafficDelta && !summary.receivedTrafficUpdates) {
    hints.push("页面已订阅 traffic，但当前抓取窗口未看到 traffic push；继续检查服务端 push 分发或是否真的产生了新流量。");
  }
  if (summary.seededTraffic && !summary.matchedTrafficRecord) {
    hints.push("脚本已经造流量，但管理端 API 里没有找到对应记录，优先检查代理链路本身，而不是 push。");
  }
  if (summary.receivedTrafficDelta || summary.receivedTrafficUpdates) {
    hints.push("浏览器已收到 traffic push；若页面仍未更新，优先检查前端 store 的消费与 summary 替换条件。");
  }

  if (hints.length > 0) {
    console.log("\n💡 诊断建议");
    for (const hint of hints) {
      console.log(`- ${hint}`);
    }
  }

  return {
    success: !options.expectTrafficSubscription || summary.sentNeedTraffic,
    summary,
  };
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    showHelp();
    process.exit(0);
  }

  try {
    const result = await runPushDebug(options);
    process.exit(result.success ? 0 : 2);
  } catch (error) {
    console.error(`\n❌ push 排查失败: ${error.message}`);
    process.exit(1);
  }
}

const isMainModule = process.argv[1]?.endsWith("push-debug.js");
if (isMainModule) {
  main();
}

module.exports = {
  parseArgs,
  runPushDebug,
};
