const fs = require("fs");
const os = require("os");
const path = require("path");
const zlib = require("zlib");
const https = require("https");
const child_process = require("child_process");
const { getPlatformKey } = require("./lib/platform.js");

const packageJSON = require(path.join(__dirname, "package.json"));

const PLATFORM_MAP = {
  "linux-x64-glibc": { pkg: "@bifrost-proxy/bifrost-linux-x64", binary: "bifrost" },
  "linux-x64-musl": { pkg: "@bifrost-proxy/bifrost-linux-x64-musl", binary: "bifrost" },
  "linux-arm64-glibc": { pkg: "@bifrost-proxy/bifrost-linux-arm64", binary: "bifrost" },
  "linux-arm64-musl": { pkg: "@bifrost-proxy/bifrost-linux-arm64-musl", binary: "bifrost" },
  "linux-arm-glibc": { pkg: "@bifrost-proxy/bifrost-linux-arm", binary: "bifrost" },
  "darwin-x64": { pkg: "@bifrost-proxy/bifrost-darwin-x64", binary: "bifrost" },
  "darwin-arm64": { pkg: "@bifrost-proxy/bifrost-darwin-arm64", binary: "bifrost" },
  "win32-x64": { pkg: "@bifrost-proxy/bifrost-win32-x64", binary: "bifrost.exe" },
  "win32-arm64": { pkg: "@bifrost-proxy/bifrost-win32-arm64", binary: "bifrost.exe" },
};

function getPlatformInfo() {
  const key = getPlatformKey({ arch: os.arch() });
  const info = PLATFORM_MAP[key];
  if (!info) {
    throw new Error(
      `Unsupported platform: ${key}. Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`
    );
  }
  return { ...info, key, subpath: `bin/${info.binary}` };
}

function downloadedBinPath(pkg, binary) {
  return path.join(__dirname, `downloaded-${pkg.replace("/", "-").replace("@", "")}-${binary}`);
}

function fetch(url) {
  return new Promise((resolve, reject) => {
    https.get(url, (res) => {
      if ((res.statusCode === 301 || res.statusCode === 302) && res.headers.location)
        return fetch(res.headers.location).then(resolve, reject);
      if (res.statusCode !== 200)
        return reject(new Error(`Server responded with ${res.statusCode}`));
      let chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => resolve(Buffer.concat(chunks)));
    }).on("error", reject);
  });
}

function extractFileFromTarGzip(buffer, subpath) {
  try {
    buffer = zlib.unzipSync(buffer);
  } catch (err) {
    throw new Error(`Invalid gzip data in archive: ${err && err.message || err}`);
  }
  let str = (i, n) => String.fromCharCode(...buffer.subarray(i, i + n)).replace(/\0.*$/, "");
  let offset = 0;
  subpath = `package/${subpath}`;
  while (offset < buffer.length) {
    let name = str(offset, 100);
    let size = parseInt(str(offset + 124, 12), 8);
    offset += 512;
    if (!isNaN(size)) {
      if (name === subpath) return buffer.subarray(offset, offset + size);
      offset += (size + 511) & ~511;
    }
  }
  throw new Error(`Could not find ${JSON.stringify(subpath)} in archive`);
}

function installUsingNPM(pkg, subpath, binPath) {
  const env = { ...process.env, npm_config_global: void 0 };
  const installDir = path.join(__dirname, "npm-install");
  fs.mkdirSync(installDir, { recursive: true });
  try {
    fs.writeFileSync(path.join(installDir, "package.json"), "{}");
    child_process.execSync(
      `npm install --loglevel=error --prefer-offline --no-audit --progress=false ${pkg}@${packageJSON.version}`,
      { cwd: installDir, stdio: "pipe", env }
    );
    const installedBinPath = path.join(installDir, "node_modules", pkg, subpath);
    fs.copyFileSync(installedBinPath, binPath);
    fs.chmodSync(binPath, 0o755);
  } finally {
    try {
      removeRecursive(installDir);
    } catch {}
  }
}

function removeRecursive(dir) {
  for (const entry of fs.readdirSync(dir)) {
    const entryPath = path.join(dir, entry);
    let stats;
    try {
      stats = fs.lstatSync(entryPath);
    } catch {
      continue;
    }
    if (stats.isDirectory()) removeRecursive(entryPath);
    else fs.unlinkSync(entryPath);
  }
  fs.rmdirSync(dir);
}

async function downloadDirectlyFromNPM(pkg, subpath, binPath) {
  const tarballName = pkg.replace("@bifrost-proxy/", "");
  const url = `https://registry.npmjs.org/${pkg}/-/${tarballName}-${packageJSON.version}.tgz`;
  console.error(`[bifrost] Trying to download ${JSON.stringify(url)}`);
  try {
    const bytes = extractFileFromTarGzip(await fetch(url), subpath);
    fs.writeFileSync(binPath, bytes);
    fs.chmodSync(binPath, 0o755);
  } catch (e) {
    console.error(`[bifrost] Failed to download ${JSON.stringify(url)}: ${e && e.message || e}`);
    throw e;
  }
}

function validateBinary(binPath) {
  try {
    const stdout = child_process
      .execFileSync(binPath, ["--version"], { stdio: "pipe" })
      .toString()
      .trim();
    const version = stdout.replace(/^bifrost\s+/i, "").trim();
    if (version !== packageJSON.version) {
      console.warn(
        `[bifrost] Warning: expected version ${packageJSON.version} but got ${version}`
      );
    }
  } catch {
    // validation is best-effort
  }
}

async function checkAndPreparePackage() {
  const { pkg, binary, subpath } = getPlatformInfo();

  try {
    const packageDir = path.dirname(require.resolve(`${pkg}/package.json`));
    const binPath = path.join(packageDir, "bin", binary);
    if (fs.existsSync(binPath)) {
      validateBinary(binPath);
      return;
    }
  } catch {}

  console.error(
    `[bifrost] Failed to find package "${pkg}" on the file system\n\n` +
      `This can happen if you use the "--no-optional" flag. The "optionalDependencies"\n` +
      `feature is used by bifrost to install the correct binary for your current platform.\n` +
      `This install script will now attempt to work around this.\n`
  );

  const binPath = downloadedBinPath(pkg, binary);

  try {
    console.error(`[bifrost] Trying to install package "${pkg}" using npm`);
    installUsingNPM(pkg, subpath, binPath);
    validateBinary(binPath);
    return;
  } catch (e) {
    console.error(
      `[bifrost] Failed to install package "${pkg}" using npm: ${e && e.message || e}`
    );
  }

  try {
    await downloadDirectlyFromNPM(pkg, subpath, binPath);
    validateBinary(binPath);
    return;
  } catch {
    throw new Error(`Failed to install package "${pkg}"`);
  }
}

checkAndPreparePackage().catch((e) => {
  console.error(`[bifrost] ${e && e.message || e}`);
  process.exit(1);
});
