// @vitest-environment node
import { afterEach, describe, expect, it } from "vitest";
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { stopTrackedProcess } from "../ui/helpers/tracked-process";

const children: ChildProcess[] = [];
const directories: string[] = [];

afterEach(async () => {
  for (const child of children.splice(0)) {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGKILL");
      await once(child, "exit");
    }
  }
  await Promise.all(
    directories
      .splice(0)
      .map((dir) => fs.rm(dir, { recursive: true, force: true })),
  );
});

async function pidFile() {
  const dir = await fs.mkdtemp(
    path.join(os.tmpdir(), "bifrost-tracked-process-"),
  );
  directories.push(dir);
  return path.join(dir, "backend.pid");
}

async function startFixture(exitDelay: number | null) {
  const child = spawn(
    process.execPath,
    [
      "-e",
      `
    process.on('SIGTERM', () => { ${exitDelay === null ? "" : `setTimeout(() => process.exit(0), ${exitDelay});`} });
    console.log('ready');
    setInterval(() => {}, 1000);
  `,
    ],
    { detached: true, stdio: ["ignore", "pipe", "pipe"] },
  );
  children.push(child);
  await once(child.stdout!, "data");
  const file = await pidFile();
  await fs.writeFile(file, String(child.pid));
  return { child, file };
}

describe("isolated backend shutdown", () => {
  // Windows terminates processes without running POSIX SIGTERM handlers.
  it.skipIf(process.platform === "win32")(
    "waits for graceful exit before removing the PID file and permitting restart",
    async () => {
      const { child, file } = await startFixture(200);
      const stopped = stopTrackedProcess(file, 3000);
      await expect(fs.readFile(file, "utf8")).resolves.toBe(String(child.pid));
      await stopped;
      expect(child.exitCode).toBe(0);
      await expect(fs.access(file)).rejects.toMatchObject({ code: "ENOENT" });
    },
  );

  it.skipIf(process.platform === "win32")(
    "reports a shutdown timeout and retains ownership evidence",
    async () => {
      const { child, file } = await startFixture(null);
      await expect(stopTrackedProcess(file, 100)).rejects.toThrow(
        "did not exit",
      );
      await expect(fs.readFile(file, "utf8")).resolves.toBe(String(child.pid));
    },
  );

  it("rejects unsafe process IDs without removing their file", async () => {
    const file = await pidFile();
    for (const invalid of [
      "0",
      "1",
      "-1",
      "not-a-pid",
      "1.5",
      String(process.pid),
    ]) {
      await fs.writeFile(file, invalid);
      await expect(stopTrackedProcess(file)).rejects.toThrow(
        "Invalid tracked process PID",
      );
      await expect(fs.readFile(file, "utf8")).resolves.toBe(invalid);
    }
  });

  it("accepts an already-removed PID file", async () => {
    await expect(stopTrackedProcess(await pidFile())).resolves.toBeUndefined();
  });
});
