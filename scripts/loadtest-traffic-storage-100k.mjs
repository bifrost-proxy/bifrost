import http from "node:http";
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..");

const proxyPort = Number(process.env.BIFROST_PROXY_PORT || "19904");
const upstreamPort = Number(process.env.BIFROST_UPSTREAM_PORT || "28084");
const bifrostBin = process.env.BIFROST_BIN || "./target/release/bifrost";
const maxRecords = Number(process.env.LOADTEST_STORAGE_MAX_RECORDS || "100000");
const recordTarget = Number(process.env.LOADTEST_STORAGE_RECORDS || "100000");
const overflowRecords = Number(process.env.LOADTEST_STORAGE_OVERFLOW_RECORDS || "0");
const totalWriteTarget = recordTarget + overflowRecords;
const writeConcurrency = Number(process.env.LOADTEST_STORAGE_WRITE_CONCURRENCY || "256");
const readIterations = Number(process.env.LOADTEST_STORAGE_READ_ITERATIONS || "80");
const batchIterations = Number(process.env.LOADTEST_STORAGE_BATCH_ITERATIONS || "30");
const searchIterations = Number(process.env.LOADTEST_STORAGE_SEARCH_ITERATIONS || "12");
const waitForRecordsTimeoutMs = Number(process.env.LOADTEST_STORAGE_WAIT_TIMEOUT_MS || "120000");
const reportDir = path.join(repoRoot, ".artifacts", "loadtest");
const runId = new Date().toISOString().replaceAll(":", "-");
const outputPath = path.join(reportDir, `traffic-storage-100k-${runId}.json`);
const logPath = path.join(reportDir, `traffic-storage-100k-${runId}.log`);
const dataDir = path.join(repoRoot, ".bifrost-storage-100k", runId);
const proxyUrl = `http://127.0.0.1:${proxyPort}`;
const apiBase = `${proxyUrl}/_bifrost/api`;

const proxyAgent = new http.Agent({
  keepAlive: true,
  maxSockets: Number(process.env.LOADTEST_PROXY_MAX_SOCKETS || "1024"),
  maxFreeSockets: Number(process.env.LOADTEST_PROXY_MAX_FREE_SOCKETS || "256"),
});

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
    const server = net.createServer();
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
  const pids = listeningPids(port);
  if (!pids.includes(bifrost.pid)) {
    throw new Error(
      `Proxy readiness check reached port ${port}, but listener pids ${JSON.stringify(
        pids,
      )} do not include started Bifrost pid ${bifrost.pid}`,
    );
  }
}

async function requestJson(url, options = {}, timeoutMs = 30000) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      ...options,
      signal: controller.signal,
    });
    if (!response.ok) {
      const text = await response.text().catch(() => "");
      throw new Error(`Request failed: ${response.status} ${response.statusText} ${text}`);
    }
    return response.json();
  } finally {
    clearTimeout(timeout);
  }
}

function timed(name, fn) {
  const started = performance.now();
  return Promise.resolve()
    .then(fn)
    .then((value) => ({
      name,
      ok: true,
      ms: performance.now() - started,
      value,
    }))
    .catch((error) => ({
      name,
      ok: false,
      ms: performance.now() - started,
      error: error.message,
    }));
}

function percentile(values, p) {
  const sorted = [...values].sort((a, b) => a - b);
  if (!sorted.length) {
    return 0;
  }
  const index = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
  return Number(sorted[index].toFixed(2));
}

function summarizeTimed(results) {
  const ok = results.filter((result) => result.ok);
  const failed = results.filter((result) => !result.ok);
  const latencies = ok.map((result) => result.ms);
  return {
    attempts: results.length,
    ok: ok.length,
    failed: failed.length,
    p50Ms: percentile(latencies, 50),
    p95Ms: percentile(latencies, 95),
    p99Ms: percentile(latencies, 99),
    maxMs: Number(Math.max(0, ...latencies).toFixed(2)),
    sampleErrors: failed.slice(0, 5).map((result) => result.error),
  };
}

function startUpstreamServer() {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url || "/", `http://127.0.0.1:${upstreamPort}`);
    const match = url.pathname.match(/^\/record\/(\d+)$/);
    if (!match) {
      res.writeHead(404);
      res.end("not found");
      return;
    }

    const index = Number(match[1]);
    const bucket = Number(url.searchParams.get("bucket") || "0");
    const needle = url.searchParams.get("needle") || `needle-${index % 1000}`;
    const body = JSON.stringify({
      ok: true,
      index,
      bucket,
      needle,
      phase: url.searchParams.get("phase") || "bulk",
      payload: `storage-body-${needle}-${"x".repeat(160)}`,
    });
    res.writeHead(200, {
      "Content-Type": "application/json",
      "Content-Length": Buffer.byteLength(body),
      "x-bifrost-loadtest-index": String(index),
      "x-bifrost-loadtest-needle": needle,
    });
    res.end(body);
  });

  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(upstreamPort, "127.0.0.1", () => resolve(server));
  });
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

function childExited(child) {
  return child.exitCode !== null || child.signalCode !== null;
}

async function waitForProxyReady(bifrost, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (bifrost.exitCode !== null) {
      throw new Error(`Bifrost exited before readiness check passed (exit=${bifrost.exitCode})`);
    }
    try {
      await requestJson(`${apiBase}/system/overview`, {}, 2000);
      assertProxyOwnedByChild(proxyPort, bifrost);
      return;
    } catch {
      await sleep(500);
    }
  }
  throw new Error("Timed out waiting for Bifrost admin API");
}

function proxiedRecordRequest(index) {
  return new Promise((resolve, reject) => {
    const bucket = index % 100;
    const needle = `needle-${index % 1000}`;
    const targetUrl = `http://127.0.0.1:${upstreamPort}/record/${index}?bucket=${bucket}&needle=${needle}&phase=bulk`;
    const target = new URL(targetUrl);
    const started = performance.now();
    const req = http.request(
      {
        host: "127.0.0.1",
        port: proxyPort,
        method: "GET",
        path: targetUrl,
        agent: proxyAgent,
        headers: {
          Host: target.host,
          "x-bifrost-loadtest-index": String(index),
          "x-bifrost-loadtest-bucket": String(bucket),
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
            ms: performance.now() - started,
          });
        });
      },
    );
    req.setTimeout(10000, () => req.destroy(new Error("record request timeout")));
    req.on("error", reject);
    req.end();
  });
}

async function runWriteWorkload() {
  let nextIndex = 0;
  const counters = {
    started: 0,
    completed: 0,
    ok: 0,
    non2xx: 0,
    errors: 0,
    bytes: 0,
    latenciesMs: [],
  };
  const startedAt = Date.now();
  const workers = Array.from({ length: writeConcurrency }, async () => {
    while (true) {
      const index = nextIndex;
      nextIndex += 1;
      if (index >= totalWriteTarget) {
        return;
      }
      counters.started += 1;
      try {
        const result = await proxiedRecordRequest(index);
        counters.completed += 1;
        counters.bytes += result.bytes;
        counters.latenciesMs.push(result.ms);
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
  const durationMs = Date.now() - startedAt;
  return {
    started: counters.started,
    completed: counters.completed,
    ok: counters.ok,
    non2xx: counters.non2xx,
    errors: counters.errors,
    bytes: counters.bytes,
    durationMs,
    rps: Number(((counters.completed / durationMs) * 1000).toFixed(2)),
    p50Ms: percentile(counters.latenciesMs, 50),
    p95Ms: percentile(counters.latenciesMs, 95),
    p99Ms: percentile(counters.latenciesMs, 99),
    maxMs: Number(Math.max(0, ...counters.latenciesMs).toFixed(2)),
    latencySampleMs: counters.latenciesMs.slice(0, 100).map((value) => Number(value.toFixed(2))),
  };
}

async function trafficList(query = "") {
  const suffix = query ? `?${query}` : "";
  return requestJson(`${apiBase}/traffic${suffix}`, {}, 30000);
}

async function waitForRecords(expectedMinimum, expectedServerSequence = null) {
  const deadline = Date.now() + waitForRecordsTimeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await trafficList("limit=1");
    const hasExpectedTotal = (last.total || 0) >= expectedMinimum;
    const hasExpectedSequence =
      expectedServerSequence == null || (last.server_sequence || 0) >= expectedServerSequence;
    if (hasExpectedTotal && hasExpectedSequence) {
      return last;
    }
    await sleep(1000);
  }
  throw new Error(
    `Timed out waiting for total>=${expectedMinimum} and server_sequence>=${expectedServerSequence}, last total=${last?.total}, server_sequence=${last?.server_sequence}`,
  );
}

async function runReadAndSearchWorkload(sampleIds) {
  const latest = [];
  const hostFilter = [];
  const statusFilter = [];
  const batch = [];
  const urlSearch = [];
  const responseBodySearch = [];

  for (let i = 0; i < readIterations; i += 1) {
    latest.push(await timed("latest-list", () => trafficList("limit=100")));
    hostFilter.push(
      await timed("host-filter-list", () =>
        trafficList(`host_contains=127.0.0.1:${upstreamPort}&limit=100`),
      ),
    );
    statusFilter.push(await timed("status-filter-list", () => trafficList("status=200&limit=100")));
  }

  for (let i = 0; i < batchIterations; i += 1) {
    const start = (i * 31) % Math.max(sampleIds.length, 1);
    const ids = sampleIds.slice(start, start + 200);
    if (!ids.length) {
      continue;
    }
    batch.push(
      await timed("batch-detail", async () => {
        const encoded = encodeURIComponent(ids.join(","));
        const response = await fetch(
          `${apiBase}/traffic/batch?ids=${encoded}&include=headers,bodies&max_body=2048`,
        );
        if (!response.ok) {
          throw new Error(`batch failed: ${response.status} ${response.statusText}`);
        }
        const text = await response.text();
        return { lines: text.trim().split("\n").filter(Boolean).length };
      }),
    );
  }

  for (let i = 0; i < searchIterations; i += 1) {
    const needle = `needle-${(i * 83) % 1000}`;
    urlSearch.push(
      await timed("url-search", () =>
        requestJson(
          `${apiBase}/search`,
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              keyword: needle,
              scope: { all: false, url: true },
              filters: {},
              limit: 50,
              max_scan: maxRecords,
              max_results: 50,
            }),
          },
          120000,
        ),
      ),
    );
    responseBodySearch.push(
      await timed("response-body-search", () =>
        requestJson(
          `${apiBase}/search`,
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              keyword: `storage-body-${needle}`,
              scope: { all: false, response_body: true },
              filters: {},
              limit: 20,
              max_scan: maxRecords,
              max_results: 20,
            }),
          },
          120000,
        ),
      ),
    );
  }

  return {
    latest: summarizeTimed(latest),
    hostFilter: summarizeTimed(hostFilter),
    statusFilter: summarizeTimed(statusFilter),
    batch: summarizeTimed(batch),
    urlSearch: summarizeTimed(urlSearch),
    responseBodySearch: summarizeTimed(responseBodySearch),
    searchSamples: {
      url: urlSearch.slice(0, 3).map((result) => ({
        ok: result.ok,
        ms: Number(result.ms.toFixed(2)),
        totalSearched: result.value?.total_searched,
        totalMatched: result.value?.total_matched,
        hasMore: result.value?.has_more,
        error: result.error,
      })),
      responseBody: responseBodySearch.slice(0, 3).map((result) => ({
        ok: result.ok,
        ms: Number(result.ms.toFixed(2)),
        totalSearched: result.value?.total_searched,
        totalMatched: result.value?.total_matched,
        hasMore: result.value?.has_more,
        error: result.error,
      })),
    },
  };
}

async function sampleSystem() {
  const [overview, memory, performanceConfig] = await Promise.all([
    requestJson(`${apiBase}/system/overview`, {}, 10000),
    requestJson(`${apiBase}/system/memory`, {}, 10000),
    requestJson(`${apiBase}/config/performance`, {}, 10000),
  ]);
  return { overview, memory, performanceConfig };
}

async function main() {
  if (maxRecords !== 100000) {
    throw new Error(`This scenario must run with LOADTEST_STORAGE_MAX_RECORDS=100000, got ${maxRecords}`);
  }
  await ensureDir(reportDir);
  await ensureDir(dataDir);
  await assertTcpPortFree(proxyPort);
  await assertTcpPortFree(upstreamPort);

  const upstreamServer = await startUpstreamServer();
  const bifrost = startBifrost();
  const logChunks = [];
  bifrost.stdout?.on("data", (chunk) => logChunks.push(chunk));
  bifrost.stderr?.on("data", (chunk) => logChunks.push(chunk));

  try {
    await waitForProxyReady(bifrost);
    await requestJson(
      `${apiBase}/config/performance`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ max_records: maxRecords }),
      },
      10000,
    );

    const before = await sampleSystem();
    const write = await runWriteWorkload();
    if (write.errors > 0 || write.non2xx > 0) {
      throw new Error(`Write workload had errors=${write.errors}, non2xx=${write.non2xx}`);
    }
    const expectedVisibleAfterWrite =
      overflowRecords > 0 ? Math.floor(maxRecords * 0.8) : Math.min(recordTarget, maxRecords);
    const afterWriteList = await waitForRecords(expectedVisibleAfterWrite, totalWriteTarget + 1);
    const latestForIds = await trafficList("limit=200");
    const sampleIds = (latestForIds.records || []).map((record) => record.id).filter(Boolean);
    const readSearch = await runReadAndSearchWorkload(sampleIds);
    const after = await sampleSystem();
    const finalTraffic = await trafficList("limit=1");

    const dbStats = after.memory.traffic_db || {};
    const report = {
      schema: "bifrost-traffic-storage-loadtest/v1",
      runId,
      gitHead: spawnSync("git", ["rev-parse", "HEAD"], { cwd: repoRoot, encoding: "utf8" }).stdout.trim(),
      bifrostBin,
      proxyPort,
      upstreamPort,
      dataDir,
      outputPath,
      parameters: {
        maxRecords,
        recordTarget,
        overflowRecords,
        totalWriteTarget,
        writeConcurrency,
        readIterations,
        batchIterations,
        searchIterations,
      },
      before: {
        trafficMaxRecords: before.performanceConfig.traffic?.max_records,
        trafficDb: before.memory.traffic_db,
      },
      write,
      afterWrite: {
        total: afterWriteList.total,
        serverSequence: afterWriteList.server_sequence,
        sampleIds: sampleIds.slice(0, 10),
      },
      readSearch,
      after: {
        total: finalTraffic.total,
        serverSequence: finalTraffic.server_sequence,
        trafficMaxRecords: after.performanceConfig.traffic?.max_records,
        trafficDb: dbStats,
        process: after.memory.process,
      },
      analysis: {
        recordCompleteness:
          write.ok > 0 ? Number(((afterWriteList.total || 0) / Math.min(write.ok, maxRecords)).toFixed(6)) : 0,
        retainedRecords: finalTraffic.total || 0,
        configuredMaxRecords: after.performanceConfig.traffic?.max_records,
        writeRps: write.rps,
        writeP95Ms: write.p95Ms,
        latestListP95Ms: readSearch.latest.p95Ms,
        hostFilterP95Ms: readSearch.hostFilter.p95Ms,
        statusFilterP95Ms: readSearch.statusFilter.p95Ms,
        batchDetailP95Ms: readSearch.batch.p95Ms,
        urlSearchP95Ms: readSearch.urlSearch.p95Ms,
        responseBodySearchP95Ms: readSearch.responseBodySearch.p95Ms,
        rssMiB: after.memory.process?.rss_mb || after.memory.process?.rss_mib,
      },
    };

    await fs.writeFile(outputPath, JSON.stringify(report, null, 2));
    await fs.writeFile(logPath, Buffer.concat(logChunks));
    console.log(JSON.stringify(report.analysis, null, 2));
    console.log(`report: ${outputPath}`);
  } finally {
    proxyAgent.destroy();
    upstreamServer.closeAllConnections?.();
    upstreamServer.closeIdleConnections?.();
    upstreamServer.close();
    if (!childExited(bifrost)) {
      bifrost.kill("SIGINT");
      await Promise.race([
        new Promise((resolve) => bifrost.once("exit", resolve)),
        sleep(5000).then(() => {
          if (!childExited(bifrost)) {
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
