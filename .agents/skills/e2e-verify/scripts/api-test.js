#!/usr/bin/env node

const { DEFAULT_PORT } = require("./lib/config.js");

const { replaceAppId, getAppId } = require("./lib/utils.js");

function parseApiArgs(args) {
  const options = {
    api: null,
    method: "GET",
    body: null,
    target: null,
    port: DEFAULT_PORT,
    timeout: 30000,
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
      case "--api":
        options.api = args[++i];
        break;
      case "-m":
      case "--method":
        options.method = args[++i];
        break;
      case "-d":
      case "--data":
        options.body = args[++i];
        break;
      case "-t":
      case "--target":
        options.target = args[++i];
        break;
      case "-p":
      case "--port":
        options.port = parseInt(args[++i], 10) || DEFAULT_PORT;
        break;
      case "--timeout":
        options.timeout = parseInt(args[++i], 10) || 30000;
        break;
      case "-v":
      case "--verbose":
        options.verbose = true;
        break;
    }
  }

  return options;
}

function showApiHelp() {
  console.log(`
api-test - API 接口验证工具

Usage:
  node api-test.js --api <path> [options]

Options:
  --api <path>         API 路径 (必需)
  -m, --method <M>     HTTP 方法 (默认: GET)
  -d, --data <json>    请求体 JSON
  -t, --target <url>   目标服务器 URL
  -p, --port <port>    端口号 (默认: ${DEFAULT_PORT})
  --timeout <ms>       超时时间 (默认: 30000)
  -v, --verbose        详细输出
  -h, --help           显示帮助

Examples:
  node api-test.js --api /_bifrost/api/system/overview
  node api-test.js --api /_bifrost/api/rules -v
  node api-test.js --api /_bifrost/api/rules -m POST -d '{"name":"test","content":"example.com http://127.0.0.1:8080"}'
`);
}

async function verifyAPI(options = {}) {
  const {
    api,
    method = "GET",
    body = null,
    headers = {},
    target = null,
    port = DEFAULT_PORT,
    timeout = 30000,
    verbose = false,
  } = options;

  if (!api) {
    return {
      success: false,
      error: "未指定 API 路径",
    };
  }

  const baseUrl = target || `http://localhost:${port}`;

  const appId = getAppId();
  const normalizedApi = api.startsWith("/") ? api : `/${api}`;
  const finalApi = replaceAppId(normalizedApi, appId);
  const url = `${baseUrl}${finalApi}`;

  const requestHeaders = {
    "Content-Type": "application/json",
    ...headers,
  };

  if (verbose) {
    console.log("\n📡 API 请求:");
    console.log(`   URL: ${url}`);
    console.log(`   Method: ${method}`);
    if (body) {
      console.log(`   Body: ${body}`);
    }
  }

  const startTime = Date.now();

  try {
    const fetchOptions = {
      method,
      headers: requestHeaders,
      signal: AbortSignal.timeout(timeout),
    };

    if (body && method !== "GET" && method !== "HEAD") {
      fetchOptions.body =
        typeof body === "string" ? body : JSON.stringify(body);
    }

    const response = await fetch(url, fetchOptions);
    const duration = Date.now() - startTime;

    let responseBody;
    const contentType = response.headers.get("content-type") || "";
    if (contentType.includes("application/json")) {
      responseBody = await response.json();
    } else {
      responseBody = await response.text();
    }

    const result = {
      success: response.ok,
      status: response.status,
      statusText: response.statusText,
      duration,
      url,
      method,
      headers: Object.fromEntries(response.headers.entries()),
      body: responseBody,
    };

    if (verbose) {
      console.log("\n📥 API 响应:");
      console.log(`   状态: ${response.status} ${response.statusText}`);
      console.log(`   耗时: ${duration}ms`);
      if (typeof responseBody === "object") {
        console.log(
          `   响应体: ${JSON.stringify(responseBody, null, 2).substring(0, 500)}`,
        );
      }
    }

    return result;
  } catch (error) {
    const duration = Date.now() - startTime;

    if (verbose) {
      console.log("\n❌ API 请求失败:");
      console.log(`   错误: ${error.message}`);
      console.log(`   耗时: ${duration}ms`);
    }

    return {
      success: false,
      error: error.message,
      duration,
      url,
      method,
    };
  }
}

async function runApiTest(options = {}) {
  console.log("\n🔌 API 接口验证");
  console.log("=".repeat(50));

  const result = await verifyAPI(options);

  if (result.success) {
    console.log(`\n✅ API 验证通过`);
    console.log(`   状态: ${result.status} ${result.statusText}`);
    console.log(`   耗时: ${result.duration}ms`);
  } else {
    console.log(`\n❌ API 验证失败`);
    if (result.error) {
      console.log(`   错误: ${result.error}`);
    } else {
      console.log(`   状态: ${result.status} ${result.statusText}`);
    }
  }

  return result;
}

async function main() {
  const args = process.argv.slice(2);
  const options = parseApiArgs(args);

  if (options.help) {
    showApiHelp();
    process.exit(0);
  }

  if (!options.api) {
    console.error("❌ 错误: 请指定 --api 参数");
    showApiHelp();
    process.exit(1);
  }

  const result = await runApiTest(options);
  process.exit(result.success ? 0 : 1);
}

const isMainModule = process.argv[1]?.endsWith("api-test.js");
if (isMainModule) {
  main().catch((error) => {
    console.error("❌ 执行出错:", error.message);
    process.exit(1);
  });
}

module.exports = {
  verifyAPI,
  runApiTest,
};
