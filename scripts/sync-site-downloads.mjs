#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const positionalArgs = process.argv.slice(2).filter((arg) => !arg.startsWith("--"));
const version = positionalArgs[0] || process.env.BIFROST_VERSION || process.env.VERSION || readWorkspaceVersion();

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`Unsupported version format: ${version}`);
}

const tag = `v${version}`;
const baseUrl = `https://github.com/bifrost-proxy/bifrost/releases/download/${tag}/`;
const assets = {
  "mac-arm": `bifrost-desktop-${tag}-aarch64-apple-darwin.dmg`,
  "mac-intel": `bifrost-desktop-${tag}-x86_64-apple-darwin.dmg`,
  "win-x64": `bifrost-desktop-${tag}-x86_64-pc-windows-msvc.msi`,
  "win-arm": `bifrost-desktop-${tag}-aarch64-pc-windows-msvc.msi`,
};

const manifest = {
  version,
  tag,
  baseUrl,
  assets,
};

const manifestPath = path.join(process.cwd(), "site", "desktop-downloads.json");
fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Synced site desktop downloads to ${tag}`);

function readWorkspaceVersion() {
  const cargo = fs.readFileSync(path.join(process.cwd(), "Cargo.toml"), "utf8");
  const match = cargo.match(/^\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m);
  if (!match) {
    throw new Error("Could not read workspace package version from Cargo.toml");
  }
  return match[1];
}
