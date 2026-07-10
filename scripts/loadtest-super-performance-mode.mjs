#!/usr/bin/env node
import http from "node:http";
import fs from "node:fs/promises";
import path from "node:path";
import { spawn } from "node:child_process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const reportDir = path.join(repoRoot, ".artifacts", "loadtest");
const runId = new Date().toISOString().replace(/[:.]/g, "-");

const totalRequests = Number(process.env.SUPER_PERF_LOADTEST_REQUESTS ?? 2000);
const concurrency = Number(process.env.SUPER_PERF_LOADTEST_CONCURRENCY ?? 64);
const host = "127.0.0.1";
const bifrostBin =
  process.env.BIFROST_BIN ||
  (await fileExists(path.join(repoRoot, "target", "release", "bifrost"))
    ? path.join(repoRoot, "target", "release", "bifrost")
    : path.join(repoRoot, "target", "debug", "bifrost"));

function percentile(values, p) {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
  return Number(sorted[index].toFixed(2));
}

async function fileExists(file) {
  try {
    await fs.access(file);
    return true;
  } catch {
    return false;
  }
}

async function allocatePort() {
  return await new Promise((resolve, reject) => {
    const server = http.createServer();
    server.listen(0, host, () => {
      const address = server.address();
      const port = address?.port;
      server.close((err) => (err ? reject(err) : resolve(port)));
    });
    server.on("error", reject);
  });
}

async function startUpstream() {
  const port = await allocatePort();
  const server = http.createServer((req, res) => {
    const body = JSON.stringify({ ok: true, path: req.url });
    res.writeHead(200, {
      "content-type": "application/json",
      "content-length": Buffer.byteLength(body),
    });
    res.end(body);
  });
  await new Promise((resolve) => server.listen(port, host, resolve));
  return {
    port,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

async function waitForReady(port, child, timeoutMs = 45_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`Bifrost exited before readiness, code=${child.exitCode}`);
    }
    try {
      const res = await fetch(`http://${host}:${port}/_bifrost/api/auth/status`, {
        signal: AbortSignal.timeout(1000),
      });
      if (res.ok) return;
    } catch {
      // retry
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for Bifrost on ${port}`);
}

async function stopProcess(child) {
  if (!child || child.exitCode !== null) return;
  child.kill("SIGINT");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5000)),
  ]);
  if (child.exitCode === null) {
    child.kill("SIGKILL");
    await new Promise((resolve) => child.once("exit", resolve));
  }
}

async function startBifrost({ mode, port, upstreamPort, dataDir }) {
  const args = [
    "-H",
    host,
    "-p",
    String(port),
    "start",
    "-y",
    "--access-mode",
    "allow_all",
    "--skip-cert-check",
    "--unsafe-ssl",
    "--no-system-proxy",
    "--rules",
    `127.0.0.1 resHeaders://X-Bifrost-Loadtest=${mode}`,
  ];
  if (mode === "super") {
    args.splice(args.indexOf("--rules"), 0, "--super-performance-mode");
  }
  const child = spawn(bifrostBin, args, {
    cwd: repoRoot,
    env: {
      ...process.env,
      BIFROST_DATA_DIR: dataDir,
      BIFROST_DISABLE_TRAY: "1",
      BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT: "1",
      HOME: path.join(dataDir, "home"),
      XDG_CONFIG_HOME: path.join(dataDir, "xdg-config"),
      XDG_DATA_HOME: path.join(dataDir, "xdg-data"),
      SKIP_FRONTEND_BUILD: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const log = [];
  child.stdout.on("data", (chunk) => log.push(chunk.toString()));
  child.stderr.on("data", (chunk) => log.push(chunk.toString()));
  await waitForReady(port, child);
  return { child, log, upstreamPort };
}

async function proxyRequest({ proxyPort, upstreamPort, index }) {
  const targetUrl = `http://${host}:${upstreamPort}/loadtest/${index}`;
  const started = performance.now();
  return await new Promise((resolve) => {
    const req = http.request(
      {
        host,
        port: proxyPort,
        method: "GET",
        path: targetUrl,
        headers: {
          host: `${host}:${upstreamPort}`,
          "x-bifrost-loadtest-index": String(index),
          connection: "close",
        },
        timeout: 10_000,
      },
      (res) => {
        res.resume();
        res.on("end", () => {
          resolve({
            ok: res.statusCode === 200,
            ms: performance.now() - started,
            ruleHeader: res.headers["x-bifrost-loadtest"],
          });
        });
      },
    );
    req.on("timeout", () => {
      req.destroy(new Error("timeout"));
    });
    req.on("error", (error) => {
      resolve({ ok: false, ms: performance.now() - started, error: error.message });
    });
    req.end();
  });
}

async function runLoad({ mode, proxyPort, upstreamPort }) {
  const latencies = [];
  let ok = 0;
  let errors = 0;
  let ruleHeaderOk = 0;
  let next = 0;
  const started = performance.now();

  async function worker() {
    while (next < totalRequests) {
      const index = next;
      next += 1;
      const result = await proxyRequest({ proxyPort, upstreamPort, index });
      latencies.push(result.ms);
      if (result.ok) ok += 1;
      else errors += 1;
      if (result.ruleHeader === mode) ruleHeaderOk += 1;
    }
  }

  await Promise.all(Array.from({ length: concurrency }, () => worker()));
  const elapsedMs = performance.now() - started;
  const traffic = await fetch(`http://${host}:${proxyPort}/_bifrost/api/traffic?limit=1`).then((r) =>
    r.json(),
  );
  return {
    mode,
    requests: totalRequests,
    concurrency,
    ok,
    errors,
    ruleHeaderOk,
    elapsedMs: Number(elapsedMs.toFixed(2)),
    rps: Number((ok / (elapsedMs / 1000)).toFixed(2)),
    p50Ms: percentile(latencies, 50),
    p95Ms: percentile(latencies, 95),
    p99Ms: percentile(latencies, 99),
    trafficTotal: traffic.total ?? traffic.records?.length ?? null,
  };
}

async function runMode(mode, upstreamPort) {
  const proxyPort = await allocatePort();
  const dataDir = path.join(repoRoot, `.bifrost-super-perf-${mode}-${runId}`);
  await fs.rm(dataDir, { recursive: true, force: true });
  await fs.mkdir(dataDir, { recursive: true });
  await fs.mkdir(path.join(dataDir, "home"), { recursive: true });
  await fs.mkdir(path.join(dataDir, "xdg-config"), { recursive: true });
  await fs.mkdir(path.join(dataDir, "xdg-data"), { recursive: true });
  const proxy = await startBifrost({ mode, port: proxyPort, upstreamPort, dataDir });
  try {
    return await runLoad({ mode, proxyPort, upstreamPort });
  } finally {
    await stopProcess(proxy.child);
    await fs.rm(dataDir, { recursive: true, force: true });
  }
}

async function main() {
  if (!(await fileExists(bifrostBin))) {
    throw new Error(`Bifrost binary not found: ${bifrostBin}`);
  }
  await fs.mkdir(reportDir, { recursive: true });
  const upstream = await startUpstream();
  try {
    const normal = await runMode("normal", upstream.port);
    const superMode = await runMode("super", upstream.port);
    const report = {
      schema: "bifrost-super-performance-loadtest/v1",
      runId,
      bifrostBin,
      upstreamPort: upstream.port,
      normal,
      super: superMode,
      superModeZeroRecords: superMode.trafficTotal === 0,
      generatedAt: new Date().toISOString(),
    };
    const reportPath = path.join(reportDir, `super-performance-${runId}.json`);
    await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
    console.log(JSON.stringify(report, null, 2));
    console.log(`Report: ${reportPath}`);
    if (superMode.trafficTotal !== 0) {
      throw new Error(`super mode retained traffic records: ${superMode.trafficTotal}`);
    }
  } finally {
    await upstream.close();
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
