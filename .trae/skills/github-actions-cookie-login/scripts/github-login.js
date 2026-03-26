#!/usr/bin/env node

const path = require("path");
const { spawn } = require("child_process");

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) {
      continue;
    }
    const name = key.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      args[name] = true;
      continue;
    }
    args[name] = next;
    i += 1;
  }
  return args;
}

function main() {
  const args = parseArgs(process.argv);
  const repoRoot = path.resolve(__dirname, "../../../..");
  const configRelative = args.config || ".env/github-actions-login.json";
  const configPath = path.resolve(repoRoot, configRelative);
  const runnerPath = path.resolve(
    repoRoot,
    ".trae/skills/site-cookie-login/scripts/site-login.js",
  );

  const child = spawn(process.execPath, [runnerPath, "--config", configPath], {
    cwd: repoRoot,
    stdio: "inherit",
  });

  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });
}

main();
