const fs = require("fs");
const fsp = require("fs/promises");
const net = require("net");
const path = require("path");
const { spawn } = require("child_process");

const { REPO_ROOT } = require("./config");

function randomSuffix() {
  return Math.random().toString(16).slice(2, 8);
}

function makeRunId(prefix = "scenario") {
  return `${prefix}-${Date.now()}-${process.pid}-${randomSuffix()}`;
}

async function findFreePort(start = 39000, end = 39999) {
  const locksDir = path.join(REPO_ROOT, ".bifrost-e2e-runs", "port-locks");
  await fsp.mkdir(locksDir, { recursive: true });
  for (let port = start; port <= end; port += 1) {
    const lockPath = path.join(locksDir, `${port}.lock`);
    let lockHandle = null;
    try {
      lockHandle = await fsp.open(lockPath, "wx");
    } catch {
      continue;
    }

    const available = await new Promise((resolve) => {
      const server = net.createServer();
      server.once("error", () => resolve(false));
      server.listen(port, "127.0.0.1", () => {
        server.close(() => resolve(true));
      });
    });
    if (available) {
      await lockHandle.close();
      return { port, lockPath };
    }
    await lockHandle.close();
    await fsp.rm(lockPath, { force: true });
  }
  throw new Error(`No free port available in range ${start}-${end}`);
}

async function waitForBackendReady(baseUrl, timeoutMs = 60000) {
  const statusUrl = `${baseUrl.replace(/\/$/, "")}/api/proxy/address`;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    try {
      const response = await fetch(statusUrl);
      if (response.ok) {
        return;
      }
    } catch {
      // keep polling
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`Timed out waiting for isolated proxy at ${statusUrl}`);
}

function stopProcessTree(child) {
  if (!child?.pid) {
    return;
  }
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {
    try {
      child.kill("SIGTERM");
    } catch {
      // ignore
    }
  }
}

async function startIsolatedProxy(prefix = "scenario") {
  const runId = makeRunId(prefix);
  const { port, lockPath } = await findFreePort();
  const runtimeDir = path.join(REPO_ROOT, ".bifrost-e2e-runs", runId);
  const dataDir = path.join(runtimeDir, "data");
  const targetDir = path.join(REPO_ROOT, ".bifrost-e2e-target");
  const binPath = path.join(REPO_ROOT, "target", "debug", "bifrost");
  const logPath = path.join(runtimeDir, "backend.log");
  const baseUrl = `http://127.0.0.1:${port}/_bifrost`;

  await fsp.mkdir(runtimeDir, { recursive: true });
  await fsp.mkdir(dataDir, { recursive: true });

  const logStream = fs.createWriteStream(logPath, { flags: "a" });
  const hasDebugBinary = await fsp.access(binPath).then(() => true).catch(() => false);
  const command = hasDebugBinary ? binPath : "cargo";
  const args = hasDebugBinary
    ? ["start", "--host", "127.0.0.1", "--port", String(port), "--skip-cert-check", "--access-mode", "allow_all", "--no-system-proxy"]
    : [
        "run",
        "--bin",
        "bifrost",
        "--",
        "start",
        "--host",
        "127.0.0.1",
        "--port",
        String(port),
        "--skip-cert-check",
        "--access-mode",
        "allow_all",
        "--no-system-proxy",
      ];

  const child = spawn(command, args, {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      BIFROST_DATA_DIR: dataDir,
      CARGO_TARGET_DIR: targetDir,
    },
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });

  child.stdout?.pipe(logStream);
  child.stderr?.pipe(logStream);

  try {
    await waitForBackendReady(baseUrl);
  } catch (error) {
    stopProcessTree(child);
    throw error;
  }

  return {
    runId,
    port,
    baseUrl,
    dataDir,
    logPath,
    async stop() {
      stopProcessTree(child);
      await new Promise((resolve) => setTimeout(resolve, 300));
      logStream.end();
      await fsp.rm(lockPath, { force: true });
    },
  };
}

function rewriteScenarioStrings(input, port) {
  if (typeof input === "string") {
    return input
      .replaceAll("127.0.0.1:9900", `127.0.0.1:${port}`)
      .replaceAll("localhost:9900", `localhost:${port}`);
  }
  if (Array.isArray(input)) {
    return input.map((item) => rewriteScenarioStrings(item, port));
  }
  if (input && typeof input === "object") {
    return Object.fromEntries(
      Object.entries(input).map(([key, value]) => [key, rewriteScenarioStrings(value, port)]),
    );
  }
  return input;
}

module.exports = {
  startIsolatedProxy,
  rewriteScenarioStrings,
};
