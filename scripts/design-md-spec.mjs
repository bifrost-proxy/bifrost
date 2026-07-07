#!/usr/bin/env node

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const rootDir = dirname(dirname(fileURLToPath(import.meta.url)));
const binPath = join(rootDir, "node_modules", ".bin", process.platform === "win32" ? "designmd.cmd" : "designmd");

if (existsSync(binPath)) {
  const result = spawnSync(binPath, ["spec", "--rules"], {
    cwd: rootDir,
    encoding: "utf8",
  });
  if (result.status === 0) {
    process.stdout.write(result.stdout);
    process.exit(0);
  }
}

const packageEntry = fileURLToPath(import.meta.resolve("@google/design.md"));
const packageDir = dirname(dirname(packageEntry));
const specPath = join(packageDir, "dist", "linter", "spec.md");
const rulesPath = join(packageDir, "dist", "linter", "spec-config.yaml");

if (!existsSync(specPath)) {
  console.error(`Unable to find DESIGN.md spec at ${specPath}`);
  process.exit(1);
}

process.stdout.write(readFileSync(specPath, "utf8"));

if (existsSync(rulesPath)) {
  process.stdout.write("\n\n# Linting Rules\n\n");
  process.stdout.write("```yaml\n");
  process.stdout.write(readFileSync(rulesPath, "utf8"));
  process.stdout.write("\n```\n");
}
