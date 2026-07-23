import { readFileSync, writeFileSync, mkdirSync, copyFileSync, chmodSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { execSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { platform, arch } from "node:process";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, "..");
const NPM_DIR = join(ROOT, "npm");

const FLAGS_WITH_VALUE = new Set(["--token", "--otp"]);
const positionalArgs = [];
const rawArgs = process.argv.slice(2);
for (let i = 0; i < rawArgs.length; i++) {
  const arg = rawArgs[i];
  if (FLAGS_WITH_VALUE.has(arg)) {
    i++;
  } else if (!arg.startsWith("--")) {
    positionalArgs.push(arg);
  }
}
const VERSION = process.env.BIFROST_VERSION || positionalArgs[0];
if (!VERSION) {
  console.error("Missing version. Set BIFROST_VERSION env or pass as argument.");
  console.error("Usage: node npm-publish.mjs <version> [--dry-run] [--local] [--print-tag] [--token <NPM_TOKEN>]");
  process.exit(1);
}

const ARTIFACTS_DIR = process.env.ARTIFACTS_DIR || join(ROOT, "artifacts");
const DRY_RUN = process.argv.includes("--dry-run");
const LOCAL_MODE = process.argv.includes("--local");
const NPM_TAG = VERSION.includes("-") ? "next" : "latest";
const PRINT_TAG = process.argv.includes("--print-tag");

if (PRINT_TAG) {
  console.log(NPM_TAG);
  process.exit(0);
}

const tokenIdx = process.argv.indexOf("--token");
const NPM_TOKEN_ARG = tokenIdx !== -1 ? process.argv[tokenIdx + 1] : null;

const otpIdx = process.argv.indexOf("--otp");
const OTP_ARG = otpIdx !== -1 ? process.argv[otpIdx + 1] : null;

const PLATFORMS = [
  {
    npmPkg: "bifrost-linux-x64",
    rustTarget: "x86_64-unknown-linux-gnu",
    binary: "bifrost",
  },
  {
    npmPkg: "bifrost-linux-arm64",
    rustTarget: "aarch64-unknown-linux-gnu",
    binary: "bifrost",
  },
  {
    npmPkg: "bifrost-linux-arm",
    rustTarget: "armv7-unknown-linux-gnueabihf",
    binary: "bifrost",
  },
  {
    npmPkg: "bifrost-linux-x64-musl",
    rustTarget: "x86_64-unknown-linux-musl",
    binary: "bifrost",
  },
  {
    npmPkg: "bifrost-linux-arm64-musl",
    rustTarget: "aarch64-unknown-linux-musl",
    binary: "bifrost",
  },
  {
    npmPkg: "bifrost-darwin-x64",
    rustTarget: "x86_64-apple-darwin",
    binary: "bifrost",
  },
  {
    npmPkg: "bifrost-darwin-arm64",
    rustTarget: "aarch64-apple-darwin",
    binary: "bifrost",
  },
  {
    npmPkg: "bifrost-win32-x64",
    rustTarget: "x86_64-pc-windows-msvc",
    binary: "bifrost.exe",
  },
  {
    npmPkg: "bifrost-win32-arm64",
    rustTarget: "aarch64-pc-windows-msvc",
    binary: "bifrost.exe",
  },
];

function updateVersion(pkgJsonPath, version) {
  const pkg = JSON.parse(readFileSync(pkgJsonPath, "utf8"));
  pkg.version = version;

  if (pkg.optionalDependencies) {
    for (const dep of Object.keys(pkg.optionalDependencies)) {
      if (dep.startsWith("@bifrost-proxy/")) {
        pkg.optionalDependencies[dep] = version;
      }
    }
  }

  writeFileSync(pkgJsonPath, JSON.stringify(pkg, null, 2) + "\n");
  console.log(`  Updated ${pkgJsonPath} to v${version}`);
}

function findBinaryInArtifacts(rustTarget, binaryName) {
  const artifactDir = join(ARTIFACTS_DIR, `cli-${rustTarget}`);

  const tarGz = join(artifactDir, `bifrost-v${VERSION}-${rustTarget}.tar.gz`);
  if (existsSync(tarGz)) {
    return { type: "tar.gz", path: tarGz };
  }

  const tarXz = join(artifactDir, `bifrost-v${VERSION}-${rustTarget}.tar.xz`);
  if (existsSync(tarXz)) {
    return { type: "tar.xz", path: tarXz };
  }

  const zip = join(artifactDir, `bifrost-v${VERSION}-${rustTarget}.zip`);
  if (existsSync(zip)) {
    return { type: "zip", path: zip };
  }

  const directBinary = join(artifactDir, binaryName);
  if (existsSync(directBinary)) {
    return { type: "binary", path: directBinary };
  }

  return null;
}

function extractBinary(archive, destDir, binaryName) {
  mkdirSync(destDir, { recursive: true });

  if (archive.type === "binary") {
    copyFileSync(archive.path, join(destDir, binaryName));
  } else if (archive.type === "tar.gz" || archive.type === "tar.xz") {
    const tarFlag = archive.type === "tar.xz" ? "-xJf" : "-xzf";
    execSync(`tar ${tarFlag} "${archive.path}" -C "${destDir}" --strip-components=1`, {
      stdio: "inherit",
    });
    if (!existsSync(join(destDir, binaryName))) {
      execSync(`tar ${tarFlag} "${archive.path}" -C "${destDir}"`, { stdio: "inherit" });
      const extracted = execSync(`find "${destDir}" -name "${binaryName}" -type f | head -1`)
        .toString()
        .trim();
      if (extracted && extracted !== join(destDir, binaryName)) {
        copyFileSync(extracted, join(destDir, binaryName));
      }
    }
  } else if (archive.type === "zip") {
    execSync(`unzip -o "${archive.path}" -d "${destDir}"`, { stdio: "inherit" });
    const extracted = execSync(`find "${destDir}" -name "${binaryName}" -type f | head -1`)
      .toString()
      .trim();
    if (extracted && extracted !== join(destDir, binaryName)) {
      copyFileSync(extracted, join(destDir, binaryName));
    }
  }

  if (binaryName !== "bifrost.exe") {
    chmodSync(join(destDir, binaryName), 0o755);
  }
}

const PUBLISH_RETRY_COUNT = 3;
const PUBLISH_RETRY_DELAY_MS = 15_000;
const PUBLISH_INTERVAL_MS = 5_000;

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function npmPublishOnce(pkgDir) {
  const args = ["publish", "--access", "public", "--tag", NPM_TAG, "--registry", "https://registry.npmjs.org/"];
  if (DRY_RUN) args.push("--dry-run");
  if (OTP_ARG) args.push("--otp", OTP_ARG);

  const env = { ...process.env };
  if (NPM_TOKEN_ARG) {
    env.npm_token = NPM_TOKEN_ARG;
    env.NODE_AUTH_TOKEN = NPM_TOKEN_ARG;
    const npmrcPath = join(pkgDir, ".npmrc");
    writeFileSync(npmrcPath, "//registry.npmjs.org/:_authToken=${npm_token}\n");
  }

  const logArgs = OTP_ARG ? args.filter((a) => a !== OTP_ARG) : args;
  console.log(`  npm ${logArgs.join(" ")} (in ${pkgDir})`);
  execSync(`npm ${args.join(" ")}`, { cwd: pkgDir, stdio: "inherit", env });
}

async function npmPublish(pkgDir) {
  for (let attempt = 1; attempt <= PUBLISH_RETRY_COUNT; attempt++) {
    try {
      npmPublishOnce(pkgDir);
      return;
    } catch (err) {
      const isConflict = err.message && err.message.includes("E409");
      if (isConflict && attempt < PUBLISH_RETRY_COUNT) {
        console.log(`  ⏳ E409 Conflict, retrying in ${PUBLISH_RETRY_DELAY_MS / 1000}s... (attempt ${attempt}/${PUBLISH_RETRY_COUNT})`);
        await sleep(PUBLISH_RETRY_DELAY_MS);
      } else {
        throw err;
      }
    }
  }
}

function getLocalPlatformKey() {
  const platformMap = { darwin: "darwin", linux: "linux", win32: "win32" };
  const archMap = { x64: "x64", arm64: "arm64", arm: "arm" };
  const p = platformMap[platform];
  const a = archMap[arch];
  if (!p || !a) {
    console.error(`Unsupported local platform: ${platform}-${arch}`);
    process.exit(1);
  }
  return `${p}-${a}`;
}

function findLocalBinary() {
  const binaryName = platform === "win32" ? "bifrost.exe" : "bifrost";
  const candidates = [
    join(ROOT, "target", "release", binaryName),
    join(ROOT, "target", "debug", binaryName),
  ];
  for (const p of candidates) {
    if (existsSync(p)) return p;
  }
  return null;
}

async function main() {
  console.log(`\n📦 Bifrost npm publish`);
  console.log(`  Version: ${VERSION}`);
  console.log(`  Tag: ${NPM_TAG}`);
  console.log(`  Mode: ${LOCAL_MODE ? "local" : "ci"}`);
  console.log(`  Dry run: ${DRY_RUN}\n`);

  if (LOCAL_MODE) {
    await publishLocal();
  } else {
    await publishCI();
  }
}

async function publishLocal() {
  const key = getLocalPlatformKey();
  const localPlatform = PLATFORMS.find((p) => p.npmPkg === `bifrost-${key}`);
  if (!localPlatform) {
    console.error(`No npm package mapping for platform: ${key}`);
    process.exit(1);
  }

  const binaryPath = findLocalBinary();
  if (!binaryPath) {
    console.error("No compiled binary found. Run `cargo build -p bifrost-cli --release` first.");
    process.exit(1);
  }
  console.log(`  Binary: ${binaryPath}`);

  console.log("\n1️⃣  Updating versions...\n");
  for (const p of PLATFORMS) {
    updateVersion(join(NPM_DIR, p.npmPkg, "package.json"), VERSION);
  }
  updateVersion(join(NPM_DIR, "bifrost", "package.json"), VERSION);

  console.log("\n2️⃣  Injecting local binary...\n");
  const binDir = join(NPM_DIR, localPlatform.npmPkg, "bin");
  mkdirSync(binDir, { recursive: true });
  copyFileSync(binaryPath, join(binDir, localPlatform.binary));
  if (localPlatform.binary !== "bifrost.exe") {
    chmodSync(join(binDir, localPlatform.binary), 0o755);
  }
  console.log(`  ✅ ${localPlatform.npmPkg}: ${localPlatform.binary} ready`);

  for (const p of PLATFORMS) {
    copyFileSync(join(ROOT, "README.md"), join(NPM_DIR, p.npmPkg, "README.md"));
  }
  console.log("  ✅ Copied root README.md into all platform packages");

  console.log("\n3️⃣  Publishing current platform package...\n");
  console.log(`  📦 Publishing @bifrost-proxy/${localPlatform.npmPkg}...`);
  await npmPublish(join(NPM_DIR, localPlatform.npmPkg));

  console.log("\n4️⃣  Publishing remaining platform packages (version bump only)...\n");
  for (const p of PLATFORMS) {
    if (p.npmPkg === localPlatform.npmPkg) continue;
    const pkgBinDir = join(NPM_DIR, p.npmPkg, "bin");
    mkdirSync(pkgBinDir, { recursive: true });
    const stubName = join(pkgBinDir, p.binary);
    if (!existsSync(stubName)) {
      writeFileSync(stubName, "");
      if (p.binary !== "bifrost.exe") chmodSync(stubName, 0o755);
    }
    if (!DRY_RUN) {
      console.log(`  ⏳ Waiting ${PUBLISH_INTERVAL_MS / 1000}s before next publish...`);
      await sleep(PUBLISH_INTERVAL_MS);
    }
    console.log(`  📦 Publishing @bifrost-proxy/${p.npmPkg} (stub)...`);
    await npmPublish(join(NPM_DIR, p.npmPkg));
  }

  console.log("\n5️⃣  Publishing main package...\n");
  copyFileSync(join(ROOT, "README.md"), join(NPM_DIR, "bifrost", "README.md"));
  console.log("  ✅ Copied root README.md into main package");
  if (!DRY_RUN) {
    console.log(`  ⏳ Waiting ${PUBLISH_INTERVAL_MS / 1000}s for platform packages to propagate...`);
    await sleep(PUBLISH_INTERVAL_MS);
  }
  console.log(`  📦 Publishing @bifrost-proxy/bifrost...`);
  await npmPublish(join(NPM_DIR, "bifrost"));

  console.log(`\n✅ All packages published successfully! (v${VERSION})\n`);
  console.log(`Test with: npx @bifrost-proxy/bifrost@${VERSION} --version\n`);
}

async function publishCI() {
  console.log(`  Artifacts: ${ARTIFACTS_DIR}`);

  console.log("\n  📂 Artifact directory structure:");
  try {
    const output = execSync(
      `find "${ARTIFACTS_DIR}" -type f \\( -name "*.tar.gz" -o -name "*.tar.xz" -o -name "*.zip" \\) | sort`,
      { encoding: "utf8" }
    );
    console.log(output || "  (empty)");
  } catch {
    console.log("  (could not list artifacts)");
  }

  console.log("1️⃣  Updating versions...\n");
  for (const p of PLATFORMS) {
    updateVersion(join(NPM_DIR, p.npmPkg, "package.json"), VERSION);
  }
  updateVersion(join(NPM_DIR, "bifrost", "package.json"), VERSION);

  console.log("\n2️⃣  Injecting binaries into platform packages...\n");
  let allFound = true;
  for (const p of PLATFORMS) {
    const archive = findBinaryInArtifacts(p.rustTarget, p.binary);
    if (!archive) {
      console.error(`  ❌ Binary not found for ${p.rustTarget}`);
      allFound = false;
      continue;
    }

    const binDir = join(NPM_DIR, p.npmPkg, "bin");
    console.log(`  📂 ${p.npmPkg}: extracting from ${archive.path}`);
    extractBinary(archive, binDir, p.binary);

    const binaryPath = join(binDir, p.binary);
    if (!existsSync(binaryPath)) {
      console.error(`  ❌ Binary not found after extraction: ${binaryPath}`);
      allFound = false;
    } else {
      const stat = execSync(`ls -la "${binaryPath}"`, { encoding: "utf8" }).trim();
      console.log(`  ✅ ${p.npmPkg}: ${p.binary} ready (${stat})`);
    }
  }

  if (!allFound) {
    console.error("\n❌ Some platform binaries are missing. Aborting publish.");
    process.exit(1);
  }

  for (const p of PLATFORMS) {
    copyFileSync(join(ROOT, "README.md"), join(NPM_DIR, p.npmPkg, "README.md"));
  }
  console.log("  ✅ Copied root README.md into all platform packages");

  console.log("\n3️⃣  Publishing platform packages...\n");
  const failedPlatforms = [];
  for (let i = 0; i < PLATFORMS.length; i++) {
    const p = PLATFORMS[i];
    const pkgDir = join(NPM_DIR, p.npmPkg);
    console.log(`\n  📦 Publishing @bifrost-proxy/${p.npmPkg}...`);
    try {
      await npmPublish(pkgDir);
      if (i < PLATFORMS.length - 1 && !DRY_RUN) {
        console.log(`  ⏳ Waiting ${PUBLISH_INTERVAL_MS / 1000}s before next publish...`);
        await sleep(PUBLISH_INTERVAL_MS);
      }
    } catch (err) {
      console.error(`  ❌ Failed to publish @bifrost-proxy/${p.npmPkg}: ${err.message}`);
      failedPlatforms.push(p.npmPkg);
    }
  }

  if (failedPlatforms.length > 0) {
    console.error(`\n❌ Failed to publish platform packages: ${failedPlatforms.join(", ")}`);
    console.error("Aborting main package publish to prevent broken installation.");
    process.exit(1);
  }

  console.log("\n4️⃣  Publishing main package...\n");
  copyFileSync(join(ROOT, "README.md"), join(NPM_DIR, "bifrost", "README.md"));
  console.log("  ✅ Copied root README.md into main package");
  if (!DRY_RUN) {
    console.log(`  ⏳ Waiting ${PUBLISH_INTERVAL_MS / 1000}s for platform packages to propagate...`);
    await sleep(PUBLISH_INTERVAL_MS);
  }
  const mainPkgDir = join(NPM_DIR, "bifrost");
  console.log(`  📦 Publishing @bifrost-proxy/bifrost...`);
  await npmPublish(mainPkgDir);

  console.log(`\n✅ All packages published successfully! (v${VERSION})\n`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
