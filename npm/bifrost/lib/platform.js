const fs = require("fs");
const child_process = require("child_process");

const MIN_GLIBC_VERSION = "2.39";

const PLATFORM_MAP = {
  "linux-x64-glibc": "@bifrost-proxy/bifrost-linux-x64",
  "linux-x64-musl": "@bifrost-proxy/bifrost-linux-x64-musl",
  "linux-arm64-glibc": "@bifrost-proxy/bifrost-linux-arm64",
  "linux-arm64-musl": "@bifrost-proxy/bifrost-linux-arm64-musl",
  "linux-arm-glibc": "@bifrost-proxy/bifrost-linux-arm",
  "darwin-x64": "@bifrost-proxy/bifrost-darwin-x64",
  "darwin-arm64": "@bifrost-proxy/bifrost-darwin-arm64",
  "win32-x64": "@bifrost-proxy/bifrost-win32-x64",
  "win32-arm64": "@bifrost-proxy/bifrost-win32-arm64",
};

function parseVersion(version) {
  return String(version)
    .split(".")
    .map((part) => Number.parseInt(part, 10))
    .filter((part) => Number.isFinite(part));
}

function versionLt(left, right) {
  const a = parseVersion(left);
  const b = parseVersion(right);
  const length = Math.max(a.length, b.length);
  for (let i = 0; i < length; i++) {
    const ai = a[i] || 0;
    const bi = b[i] || 0;
    if (ai < bi) return true;
    if (ai > bi) return false;
  }
  return false;
}

function parseGlibcVersion(lddOutput) {
  const text = String(lddOutput || "");
  if (!/GLIBC|GNU libc/i.test(text)) return null;
  const match = text.match(/[0-9]+\.[0-9]+/);
  return match ? match[0] : null;
}

function hasMuslLoader(existsSync = fs.existsSync) {
  try {
    return (
      existsSync("/lib/ld-musl-x86_64.so.1") ||
      existsSync("/lib/ld-musl-aarch64.so.1") ||
      existsSync("/lib/ld-musl-armhf.so.1")
    );
  } catch {
    return false;
  }
}

function detectLinuxLibc(options = {}) {
  const execSync = options.execSync || child_process.execSync;
  const existsSync = options.existsSync || fs.existsSync;

  let lddOutput = "";
  if (Object.prototype.hasOwnProperty.call(options, "lddOutput")) {
    lddOutput = options.lddOutput;
  } else {
    try {
      lddOutput = execSync("ldd --version 2>&1 || true", {
        stdio: ["pipe", "pipe", "pipe"],
        encoding: "utf8",
      });
    } catch {}
  }

  if (/musl/i.test(lddOutput)) return "musl";

  const glibcVersion = parseGlibcVersion(lddOutput);
  if (glibcVersion) {
    return versionLt(glibcVersion, MIN_GLIBC_VERSION) ? "musl" : "glibc";
  }

  if (hasMuslLoader(existsSync)) return "musl";

  return "musl";
}

function getPlatformKey(options = {}) {
  const platform = options.platform || process.platform;
  const arch = options.arch || process.arch;

  let key = `${platform}-${arch}`;
  if (platform === "linux") {
    if (arch === "arm") {
      return `${key}-glibc`;
    }
    key = `${key}-${detectLinuxLibc(options)}`;
  }
  return key;
}

function getPackageName(options = {}) {
  const key = getPlatformKey(options);
  return PLATFORM_MAP[key] || null;
}

module.exports = {
  MIN_GLIBC_VERSION,
  PLATFORM_MAP,
  detectLinuxLibc,
  getPackageName,
  getPlatformKey,
  parseGlibcVersion,
  versionLt,
};
