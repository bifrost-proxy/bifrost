import { execFile, spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createWriteStream } from "node:fs";
import fs from "node:fs/promises";
import { promisify } from "node:util";
import { allocateUiTestEnv } from "./helpers/test-env";

const env = await allocateUiTestEnv();
const backendPort = env.backendPort;
const BASE_PROXY_URL = process.env.PROXY_URL || `http://127.0.0.1:${backendPort}`;
const BACKEND_URL =
  process.env.ADMIN_STATUS_URL ||
  `${BASE_PROXY_URL.replace(/\/$/, "")}/_bifrost/api/proxy/address`;
const ACCESS_STATUS_URL = `${BASE_PROXY_URL.replace(/\/$/, "")}/_bifrost/api/whitelist`;
const execFileAsync = promisify(execFile);

const getRepoRoot = () => {
  const current = fileURLToPath(import.meta.url);
  return path.resolve(path.dirname(current), "../../..");
};

const isBackendReady = async () => {
  try {
    const res = await fetch(BACKEND_URL);
    return res.ok;
  } catch {
    return false;
  }
};

const hasAccessControlConfigured = async () => {
  try {
    const res = await fetch(ACCESS_STATUS_URL);
    if (!res.ok) {
      return false;
    }
    const body = (await res.json()) as { mode?: string };
    return typeof body.mode === "string" && body.mode.length > 0;
  } catch {
    return false;
  }
};

const waitForBackend = async () => {
  for (let i = 0; i < 600; i += 1) {
    if (await isBackendReady()) return true;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  return false;
};

const ensureBackendBinaryBuilt = async (repoRoot: string, targetDir: string) => {
  await execFileAsync(
    "cargo",
    ["build", "--bin", "bifrost"],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        CARGO_TARGET_DIR: targetDir,
      },
      timeout: 15 * 60 * 1000,
      maxBuffer: 20 * 1024 * 1024,
    },
  );
};

const isProcessAlive = (pid: number) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

const stopTrackedProcess = async (pidFile: string) => {
  try {
    const pidText = await fs.readFile(pidFile, "utf-8");
    const pid = Number(pidText);
    if (Number.isNaN(pid) || !isProcessAlive(pid)) {
      return;
    }
    try {
      process.kill(-pid);
    } catch {
      process.kill(pid);
    }
    await fs.rm(pidFile, { force: true });
  } catch {
    void 0;
  }
};

const startTrafficGenerator = async (repoRoot: string) => {
  const trafficPidFile = process.env.BIFROST_UI_TEST_TRAFFIC_PID_FILE || path.join(repoRoot, ".ui-traffic.pid");
  try {
    const pidText = await fs.readFile(trafficPidFile, "utf-8");
    const pid = Number(pidText);
    if (!Number.isNaN(pid) && isProcessAlive(pid)) {
      return;
    }
  } catch {
    void 0;
  }

  const generatorPath = path.join(repoRoot, "web", "tests", "ui", "traffic-generator.cjs");
  const child = spawn("node", [generatorPath], {
    cwd: path.join(repoRoot, "web"),
      env: {
        ...process.env,
        PROXY_URL: BASE_PROXY_URL,
      },
      stdio: "ignore",
      detached: true,
  });
  const pid = child.pid;
  if (!pid) {
    throw new Error("Failed to start traffic generator");
  }
  await fs.writeFile(trafficPidFile, String(pid));
};

export default async () => {
  const repoRoot = getRepoRoot();
  const pidFile = process.env.BIFROST_UI_TEST_PID_FILE || path.join(repoRoot, ".ui-backend.pid");
  const ready = await isBackendReady();
  const accessConfigured = ready ? await hasAccessControlConfigured() : false;

  if (!ready || !accessConfigured) {
    await stopTrackedProcess(pidFile);
    const dataDir = process.env.BIFROST_DATA_DIR || path.join(repoRoot, ".bifrost-ui-test");
    const targetDir = process.env.BIFROST_UI_TEST_TARGET_DIR || path.join(repoRoot, ".bifrost-ui-target");
    const binPath = path.join(targetDir, "debug", "bifrost");
    const logPath = process.env.BIFROST_UI_TEST_LOG_FILE || path.join(repoRoot, ".ui-backend.log");
    const logStream = createWriteStream(logPath, { flags: "a" });

    await ensureBackendBinaryBuilt(repoRoot, targetDir);

    const child = spawn(binPath, [
      "start",
      "--host",
      "127.0.0.1",
      "-p",
      String(backendPort),
      "--unsafe-ssl",
      "--no-system-proxy",
      "--access-mode",
      "allow_all",
    ], {
      cwd: repoRoot,
      env: {
        ...process.env,
        PROXY_URL: BASE_PROXY_URL,
        ADMIN_STATUS_URL: BACKEND_URL,
        BIFROST_UI_TEST_PORT: String(backendPort),
        BIFROST_DATA_DIR: dataDir,
        CARGO_TARGET_DIR: targetDir,
      },
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
    });
    child.stdout?.pipe(logStream);
    child.stderr?.pipe(logStream);
    const pid = child.pid;
    if (!pid) {
      throw new Error("Failed to start Bifrost backend process");
    }
    await fs.writeFile(pidFile, String(pid));

    const ok = await waitForBackend();
    if (!ok) {
      try {
        process.kill(-pid);
      } catch {
        try {
          process.kill(pid);
        } catch {
          return;
        }
      }
      throw new Error("Bifrost backend failed to start for UI tests");
    }
  }

  await startTrafficGenerator(repoRoot);
};
