import http from "node:http";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..");

const proxyPort = Number(process.env.BIFROST_PROXY_PORT || "9900");
const targetUrl = process.env.LOADTEST_TARGET_URL;
const concurrency = Number(process.env.LOADTEST_CONCURRENCY || "16");
const durationMs = Number(process.env.LOADTEST_DURATION_MS || "15000");
const detailSampleLimit = Number(process.env.LOADTEST_DETAIL_SAMPLE_LIMIT || "20");
const shouldStartProxy = process.env.LOADTEST_START_PROXY === "1";
const bifrostBin = process.env.BIFROST_BIN || "./target/debug/bifrost";
const apiBase = `http://127.0.0.1:${proxyPort}/_bifrost/api`;
const reportDir = path.join(repoRoot, ".artifacts", "loadtest");
const runId = new Date().toISOString().replaceAll(":", "-");
const dataDir = path.join(repoRoot, ".bifrost-loadtest-502", runId);
const outputPath = path.join(reportDir, `upstream-502-${runId}.json`);

if (!targetUrl) {
  console.error("LOADTEST_TARGET_URL is required");
  process.exit(1);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function ensureDir(dir) {
  await fs.mkdir(dir, { recursive: true });
}

function assertTcpPortFree(port, host = "127.0.0.1") {
  return new Promise((resolve, reject) => {
    const lsof = spawnSync("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN"], { encoding: "utf8" });
    if (lsof.status === 0 && lsof.stdout.trim()) {
      reject(new Error(`TCP port ${host}:${port} is not available:\n${lsof.stdout.trim()}`));
      return;
    }
    const server = http.createServer();
    server.once("error", (error) => {
      reject(new Error(`TCP port ${host}:${port} is not available: ${error.message}`));
    });
    server.listen(port, host, () => {
      server.close(resolve);
    });
  });
}

function listeningPids(port) {
  const result = spawnSync("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-Fp"], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    return [];
  }
  return result.stdout
    .split("\n")
    .filter((line) => line.startsWith("p"))
    .map((line) => Number(line.slice(1)))
    .filter(Number.isFinite);
}

function assertProxyOwnedByChild(port, bifrost) {
  if (!bifrost) {
    return;
  }
  const pids = listeningPids(port);
  if (!pids.includes(bifrost.pid)) {
    throw new Error(
      `Proxy readiness check reached port ${port}, but listener pids ${JSON.stringify(
        pids,
      )} do not include started Bifrost pid ${bifrost.pid}`,
    );
  }
}

async function fetchJson(url, options = {}) {
  const response = await fetch(url, options);
  if (!response.ok) {
    throw new Error(`Request failed: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

function proxiedHttpRequest(url) {
  return new Promise((resolve, reject) => {
    const target = new URL(url);
    const startedAt = Date.now();
    const req = http.request(
      {
        host: "127.0.0.1",
        port: proxyPort,
        method: "GET",
        path: url,
        headers: {
          Host: target.host,
          Connection: "keep-alive",
          Accept: "*/*",
        },
      },
      (res) => {
        let bytes = 0;
        res.on("data", (chunk) => {
          bytes += chunk.length;
        });
        res.on("end", () => {
          resolve({
            statusCode: res.statusCode || 0,
            bytes,
            latencyMs: Date.now() - startedAt,
          });
        });
      },
    );
    req.setTimeout(10000, () => {
      req.destroy(new Error("request timeout"));
    });
    req.on("error", reject);
    req.end();
  });
}

function percentile(values, p) {
  if (!values.length) {
    return 0;
  }
  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
  return sorted[index];
}

function startBifrost() {
  return spawn(
    bifrostBin,
    [
      "start",
      "--host",
      "127.0.0.1",
      "--port",
      String(proxyPort),
      "--skip-cert-check",
      "--no-system-proxy",
    ],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        BIFROST_DATA_DIR: dataDir,
        BIFROST_DISABLE_TRAY: "1",
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
}

async function waitForProxyReady(bifrost, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (bifrost && bifrost.exitCode !== null) {
      throw new Error(`Bifrost exited before readiness check passed (exit=${bifrost.exitCode})`);
    }
    try {
      await fetchJson(`${apiBase}/proxy/address`);
      assertProxyOwnedByChild(proxyPort, bifrost);
      return;
    } catch {
      await sleep(500);
    }
  }
  throw new Error("Timed out waiting for Bifrost admin API");
}

async function runLoad() {
  const counters = {
    started: 0,
    completed: 0,
    ok: 0,
    non2xx: 0,
    errors: 0,
    bytes: 0,
    latenciesMs: [],
  };
  const stopAt = Date.now() + durationMs;

  const workers = Array.from({ length: concurrency }, async () => {
    while (Date.now() < stopAt) {
      counters.started += 1;
      try {
        const result = await proxiedHttpRequest(targetUrl);
        counters.completed += 1;
        counters.bytes += result.bytes;
        counters.latenciesMs.push(result.latencyMs);
        if (result.statusCode >= 200 && result.statusCode < 300) {
          counters.ok += 1;
        } else {
          counters.non2xx += 1;
        }
      } catch {
        counters.completed += 1;
        counters.errors += 1;
      }
    }
  });

  await Promise.all(workers);
  return counters;
}

async function collectRecentTargetTraffic(startTs) {
  const traffic = await fetchJson(`${apiBase}/traffic?limit=1000`);
  const target = new URL(targetUrl);
  return traffic.records.filter((record) => {
    const ts = Number(record.ts || 0);
    return ts >= startTs && record.h === target.host && record.p === target.pathname;
  });
}

async function collect502Details(records) {
  const sample = records.filter((record) => record.s === 502).slice(0, detailSampleLimit);
  const details = [];
  for (const record of sample) {
    const detail = await fetchJson(`${apiBase}/traffic/${record.id}`);
    details.push({
      id: detail.id,
      status: detail.status,
      duration_ms: detail.duration_ms,
      error_message: detail.error_message,
      url: detail.url,
    });
  }
  return details;
}

function countBy(items, keyOf) {
  const counts = new Map();
  for (const item of items) {
    const key = keyOf(item);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  return Object.fromEntries([...counts.entries()].sort((a, b) => b[1] - a[1]));
}

async function main() {
  await ensureDir(reportDir);
  if (shouldStartProxy) {
    await ensureDir(dataDir);
    await assertTcpPortFree(proxyPort);
  }

  let bifrost;
  const logChunks = [];
  if (shouldStartProxy) {
    bifrost = startBifrost();
    bifrost.stdout?.on("data", (chunk) => logChunks.push(chunk));
    bifrost.stderr?.on("data", (chunk) => logChunks.push(chunk));
  }

  try {
    await waitForProxyReady(bifrost);
    const startTs = Date.now();
    const load = await runLoad();
    await sleep(1500);
    const records = await collectRecentTargetTraffic(startTs);
    const detail502 = await collect502Details(records);

    const report = {
      runId,
      proxyPort,
      targetUrl,
      concurrency,
      durationMs,
      load: {
        ...load,
        p50Ms: percentile(load.latenciesMs, 50),
        p95Ms: percentile(load.latenciesMs, 95),
        p99Ms: percentile(load.latenciesMs, 99),
      },
      traffic: {
        matchedRecords: records.length,
        statusBreakdown: countBy(records, (record) => String(record.s)),
        errorBreakdown: countBy(detail502, (detail) => detail.error_message || "UNKNOWN"),
        sample502: detail502,
      },
    };

    await fs.writeFile(outputPath, JSON.stringify(report, null, 2));
    if (shouldStartProxy) {
      await fs.writeFile(`${outputPath}.log`, Buffer.concat(logChunks));
    }

    console.log(JSON.stringify(report, null, 2));
    console.log(`report: ${outputPath}`);
  } finally {
    if (bifrost && !bifrost.killed) {
      bifrost.kill("SIGINT");
      await Promise.race([
        new Promise((resolve) => bifrost.once("exit", resolve)),
        sleep(5000).then(() => {
          if (!bifrost.killed) {
            bifrost.kill("SIGKILL");
          }
        }),
      ]);
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
