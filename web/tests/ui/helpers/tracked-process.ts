import fs from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";

function isProcessAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") return false;
    throw error;
  }
}

/** Stop only the process group recorded by this isolated test run. */
export async function stopTrackedProcess(pidFile: string, timeoutMs = 15000) {
  let text: string;
  try {
    text = await fs.readFile(pidFile, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
    throw error;
  }
  const pid = Number(text);
  if (!Number.isSafeInteger(pid) || pid <= 1 || pid === process.pid) {
    throw new Error(`Invalid tracked process PID in ${pidFile}`);
  }

  if (isProcessAlive(pid)) {
    try {
      process.kill(-pid, "SIGTERM");
    } catch {
      try {
        process.kill(pid, "SIGTERM");
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
      }
    }
    const deadline = Date.now() + timeoutMs;
    while (isProcessAlive(pid)) {
      if (Date.now() >= deadline) {
        throw new Error(
          `Tracked process ${pid} did not exit after ${timeoutMs}ms`,
        );
      }
      await delay(50);
    }
  }
  // Do not remove ownership evidence before graceful shutdown has completed.
  await fs.rm(pidFile, { force: true });
}
