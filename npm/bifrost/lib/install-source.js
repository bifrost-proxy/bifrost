const path = require("path");

function detectPackageManager(entryPath) {
  const normalized = path.resolve(entryPath).replaceAll("\\", "/").toLowerCase();
  if (
    normalized.includes("/.pnpm/") ||
    normalized.includes("/pnpm/global/") ||
    normalized.includes("/library/pnpm/") ||
    normalized.includes("/appdata/local/pnpm/")
  ) {
    return "pnpm";
  }
  return "npm";
}

function buildChildEnvironment(entryPath, baseEnvironment = process.env) {
  return {
    ...baseEnvironment,
    BIFROST_CLI_INSTALL_SOURCE: detectPackageManager(entryPath),
  };
}

exports.detectPackageManager = detectPackageManager;
exports.buildChildEnvironment = buildChildEnvironment;
