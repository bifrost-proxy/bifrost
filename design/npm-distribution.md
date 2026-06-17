# npm 渠道分发

## 功能模块详细描述

通过 npm 渠道分发 bifrost CLI 二进制文件，用户可以通过 `npm install -g @bifrost-proxy/bifrost` 安装 bifrost。

采用 **平台包 + 主包** 的架构模式（与 esbuild、turbo、SWC 等项目一致）：

- **平台包**：每个 target 对应一个独立的 npm 包，包含该平台的预编译二进制文件
- **主包**（`@bifrost-proxy/bifrost`）：作为入口，根据当前平台自动选择并安装对应的平台包

### 包结构

```
@bifrost-proxy/bifrost              # 主包 - 入口 + 平台分发逻辑
@bifrost-proxy/bifrost-linux-x64    # Linux x86_64
@bifrost-proxy/bifrost-linux-arm64  # Linux aarch64
@bifrost-proxy/bifrost-linux-x64-musl  # Linux x86_64 (musl)
@bifrost-proxy/bifrost-linux-arm64-musl # Linux aarch64 (musl)
@bifrost-proxy/bifrost-linux-arm    # Linux armv7
@bifrost-proxy/bifrost-darwin-x64   # macOS Intel
@bifrost-proxy/bifrost-darwin-arm64 # macOS Apple Silicon
@bifrost-proxy/bifrost-win32-x64    # Windows x64
@bifrost-proxy/bifrost-win32-arm64  # Windows ARM64
```

### 用户安装方式

```bash
# 全局安装
npm install -g @bifrost-proxy/bifrost

# npx 直接运行
npx @bifrost-proxy/bifrost start

# 项目开发依赖
npm install --save-dev @bifrost-proxy/bifrost
```

## 实现逻辑

### 按平台安装机制

安装时**只下载当前操作系统对应的二进制文件**，而非全部平台二进制。实现方式：

1. **`optionalDependencies` + `os`/`cpu` 过滤**（主路径）：npm/yarn/pnpm 根据平台包 `package.json` 中的 `os` 和 `cpu` 字段自动过滤，只安装匹配当前平台的包
2. **`postinstall` 兜底脚本**（降级路径）：当 `--no-optional` 等场景导致平台包未安装时，通过三级容错自动获取二进制

### 平台包

每个平台包包含：
- `package.json`：声明 `os` 和 `cpu` 字段限制安装平台
- 预编译的 bifrost 二进制文件

npm 的 `optionalDependencies` + `os`/`cpu` 机制确保只有匹配当前平台的包会被安装。

### 主包

主包通过 `optionalDependencies` 声明所有平台包，并提供：
- `bin/bifrost`：Node.js 入口脚本，调用 `lib/index.js` 的 `getBinaryPath()`，再用 `execFileSync` 透传 `process.argv.slice(2)` 与 `process.env` 启动二进制；二进制以非零状态退出时透传 `error.status`，被信号中断时用 `process.kill(process.pid, error.signal)` 透传信号
- `lib/index.js`：导出 `getBinaryPath()`，先 `require.resolve` 平台包并校验 `bin/<binary>` 存在，否则回退到 `downloaded-*` 路径，全部缺失时抛出可读错误
- `lib/platform.js`：平台/libc 检测，导出 `PLATFORM_MAP`、`getPlatformKey`、`getPackageName`、`detectLinuxLibc`、`parseGlibcVersion`、`versionLt`、`MIN_GLIBC_VERSION`（当前为 `2.39`）
- `install.js`：`postinstall` 兜底脚本

### postinstall 兜底机制（install.js）

当 `optionalDependencies` 安装的平台包不可用时（如使用 `--no-optional`、npm 版本过旧不支持 `os`/`cpu` 过滤等），`install.js` 提供三级容错：

1. **检测平台包是否已安装**：通过 `require.resolve("${pkg}/package.json")` 定位平台包目录，检查 `bin/<binary>` 是否存在，存在则直接使用并 `validateBinary`
2. **通过 npm install 单独安装**：在 `__dirname/npm-install` 临时目录写入空 `package.json` 后执行 `npm install --loglevel=error --prefer-offline --no-audit --progress=false <平台包>@<版本号>`（清空 `npm_config_global` 避免污染全局），把 `node_modules/<pkg>/bin/<binary>` 复制到 `downloaded-<pkg>-<binary>` 文件并 `chmod 0o755`，最后递归清理临时目录
3. **直接从 npm registry 下载 tgz**：通过 HTTPS 从 `https://registry.npmjs.org/<pkg>/-/<tarballName>-<version>.tgz` 下载（`tarballName` 即去掉 `@bifrost-proxy/` 前缀的包名），跟随 301/302 重定向；内置 mini tar+gzip 解析器从 `package/bin/<binary>` 解出二进制写入 `downloaded-*` 路径

安装完成后通过 `execFileSync(binPath, ["--version"])` 解析输出（去掉 `bifrost ` 前缀）并与 `package.json.version` 比对，不一致时仅打印 warning；校验失败异常被吞掉（best-effort），不会阻塞安装。

### 二进制查找优先级（lib/index.js）

1. 用 `lib/platform.js` 的 `getPlatformKey({ platform, arch })` 解析当前平台键 → 在 `PLATFORM_MAP` 中查表得到 npm 包名；不支持时直接抛错并列出所有 supported keys
2. 通过 `require.resolve("${packageName}/package.json")` 查找 `optionalDependencies` 安装的平台包，校验 `bin/<binaryName>`（Windows 为 `bifrost.exe`，其他为 `bifrost`）是否存在
3. 回退到主包目录下的 `downloaded-<pkg>-<binaryName>` 二进制（由 `install.js` 兜底下载产生）
4. 两条路径都没找到时抛出 `The platform-specific package ... is not installed. Please reinstall @bifrost-proxy/bifrost to fix this.`

### 平台映射表

| 平台键 (platform key)        | Rust Target                       | npm 包名                                   | os      | cpu   |
|------------------------------|-----------------------------------|--------------------------------------------|---------|-------|
| linux-x64-glibc              | x86_64-unknown-linux-gnu          | @bifrost-proxy/bifrost-linux-x64           | linux   | x64   |
| linux-arm64-glibc            | aarch64-unknown-linux-gnu         | @bifrost-proxy/bifrost-linux-arm64         | linux   | arm64 |
| linux-x64-musl               | x86_64-unknown-linux-musl         | @bifrost-proxy/bifrost-linux-x64-musl      | linux   | x64   |
| linux-arm64-musl             | aarch64-unknown-linux-musl        | @bifrost-proxy/bifrost-linux-arm64-musl    | linux   | arm64 |
| linux-arm-glibc              | armv7-unknown-linux-gnueabihf     | @bifrost-proxy/bifrost-linux-arm           | linux   | arm   |
| darwin-x64                   | x86_64-apple-darwin               | @bifrost-proxy/bifrost-darwin-x64          | darwin  | x64   |
| darwin-arm64                 | aarch64-apple-darwin              | @bifrost-proxy/bifrost-darwin-arm64        | darwin  | arm64 |
| win32-x64                    | x86_64-pc-windows-msvc            | @bifrost-proxy/bifrost-win32-x64           | win32   | x64   |
| win32-arm64                  | aarch64-pc-windows-msvc           | @bifrost-proxy/bifrost-win32-arm64         | win32   | arm64 |

平台包 `package.json` 中 `os`/`cpu` 字段对应上表（如 `darwin`+`arm64`），不区分 glibc/musl；libc 区分仅在主包侧通过 `getPlatformKey` 在运行时检测后挑选不同的 npm 包名。

### Linux libc 检测（lib/platform.js）

- `linux-arm`（armv7）直接固定为 `linux-arm-glibc`，不做检测。
- 其余 Linux 平台调用 `detectLinuxLibc`：
  1. `execSync("ldd --version 2>&1 || true")`；输出含 `musl` → 判为 `musl`。
  2. 否则解析 `GLIBC`/`GNU libc` 行中的 `X.Y` 版本号；版本 < `MIN_GLIBC_VERSION`（当前 `2.39`，常量在 `lib/platform.js`）→ 视为 `musl`（回退到 musl 静态包），否则 `glibc`。
  3. 没有可解析的 glibc 版本时，检查 `/lib/ld-musl-*.so.1` 是否存在；命中视为 `musl`。
  4. 仍无法判定时 **默认返回 `musl`**（musl 静态包在 glibc 系统上也能跑，避免低版本 glibc 主机加载失败）。
- `detectLinuxLibc` 支持注入 `execSync`/`existsSync`/`lddOutput` 以便单测。

## 依赖项

- GitHub Actions Release workflow（`.github/workflows/release.yml` 中 `publish-npm` job，依赖 `prepare` / `build-cli` / `release` 三个 job）
- npm registry（`https://registry.npmjs.org/`）
- `secrets.NPM_TOKEN`（在 `Publish to npm` 步骤注入为 `NODE_AUTH_TOKEN`）
- Node.js 22（由 `actions/setup-node@v4` 安装）
- 9 个 CLI build job 上传的 `cli-<rustTarget>` artifact，且非 Windows 目标必须同时包含 `.tar.gz` 与 `.tar.xz`，Windows 目标必须包含 `.zip`

## 文件结构

```
npm/
├── bifrost/                    # 主包 @bifrost-proxy/bifrost
│   ├── package.json
│   ├── bin/
│   │   └── bifrost             # Node.js 入口脚本
│   ├── lib/
│   │   ├── index.js            # 平台解析逻辑（供 programmatic API 使用）
│   │   └── platform.js         # 平台映射与检测逻辑
│   └── install.js              # postinstall 兜底脚本
├── bifrost-linux-x64/
│   └── package.json
├── bifrost-linux-arm64/
│   └── package.json
├── bifrost-linux-x64-musl/
│   └── package.json
├── bifrost-linux-arm64-musl/
│   └── package.json
├── bifrost-linux-arm/
│   └── package.json
├── bifrost-darwin-x64/
│   └── package.json
├── bifrost-darwin-arm64/
│   └── package.json
├── bifrost-win32-x64/
│   └── package.json
└── bifrost-win32-arm64/
    └── package.json
scripts/
└── npm-publish.mjs             # 发布脚本：注入版本、复制二进制、执行 npm publish
```

## CI 流程

`release.yml` 中 `publish-npm` job 依赖 `[prepare, build-cli, release]`，跑在 `ubuntu-latest`：

1. `actions/checkout@v4` 切到 `prepare.outputs.tag`
2. `actions/setup-node@v4` 安装 Node 22 并配置 `registry-url: https://registry.npmjs.org`
3. `actions/download-artifact@v4` 以 `pattern: cli-*` 拉取全部 CLI 构建产物到 `artifacts/`
4. **Verify artifacts** 步骤遍历 9 个 `EXPECTED_TARGETS`：检查 `cli-<target>/` 目录存在；非 Windows 目标要求 `bifrost-<tag>-<target>.tar.gz` **和** `.tar.xz` 同时存在（前者给 Homebrew/npm/legacy installer，后者给 install script 与 bifrost upgrade），Windows 目标要求 `.zip` 存在；任一缺失立即 `exit 1`
5. **Publish to npm** 步骤执行 `node scripts/npm-publish.mjs`，注入 `BIFROST_VERSION=prepare.outputs.version`、`ARTIFACTS_DIR=$GITHUB_WORKSPACE/artifacts`、`NODE_AUTH_TOKEN=secrets.NPM_TOKEN`

### scripts/npm-publish.mjs 行为

- **入参**：`BIFROST_VERSION` 环境变量或第一个 positional 参数；可选 `--dry-run`、`--local`、`--token <NPM_TOKEN>`、`--otp <CODE>`；CI 模式还可用 `ARTIFACTS_DIR` 覆盖产物目录。
- **CI 模式（默认）**：
  1. 同步版本号到 9 个平台包 + 主包 `package.json`，并把主包 `optionalDependencies` 中所有 `@bifrost-proxy/*` 条目的版本号一起改写为 `BIFROST_VERSION`
  2. 在 `artifacts/cli-<rustTarget>/` 中按 `tar.gz` → `tar.xz` → `zip` → 裸二进制顺序查找产物，解压到对应平台包的 `bin/`；先尝试 `--strip-components=1`，若提取后还找不到目标文件名则全量解压后 `find` 兜底；非 Windows 二进制 `chmod 0o755`
  3. 把根目录 `README.md` 复制到每个平台包以及主包目录
  4. 依次 `npm publish --access public --tag latest --registry https://registry.npmjs.org/`（带 `--dry-run` 时透传），平台包之间间隔 5s（`PUBLISH_INTERVAL_MS`），平台包发布失败收集到 `failedPlatforms`，全部完成后若有失败则 `exit 1` 不再发主包；主包发布前再等 5s 让平台包传播
  5. 单个包发布支持最多 3 次重试（`PUBLISH_RETRY_COUNT`），仅在错误信息含 `E409`（冲突）时按 15s 间隔 (`PUBLISH_RETRY_DELAY_MS`) 重试，其他错误立即上抛
  6. 通过 `--token` 传入时，会在每个包目录写入临时 `.npmrc`（`//registry.npmjs.org/:_authToken=${npm_token}`），同时设置 `npm_token` 与 `NODE_AUTH_TOKEN`
- **`--local` 模式**：根据本机 `platform`/`arch` 选一个平台包，从 `target/release/<binary>` 或 `target/debug/<binary>` 注入二进制，其余平台包先写入空 stub 文件再发布（保证主包 `optionalDependencies` 解析成功），最后发主包；与 CI 模式共用 README 复制、版本同步、重试与节流逻辑。

（CI 入口仅调用 CI 模式；`--local` 路径供本地手工发布调试，未在 release.yml 中触发。）

## 测试方案

- 本地执行 `node npm/bifrost/bin/bifrost --version` 验证二进制查找逻辑
- CI 中发布到 npm 后可通过 `npx @bifrost-proxy/bifrost --version` 验证
- 验证各包管理器只安装当前平台包：`npm install --dry-run` / 实际安装后检查 `node_modules/@bifrost-proxy/` 目录
- `lib/platform.js` 的 `detectLinuxLibc`/`getPlatformKey` 通过 `options.execSync`/`existsSync`/`lddOutput` 参数注入便于单测（自动化单测 planned, not yet shipped as of 2026-06-16）

## 校验要求

- npm publish 前的产物校验由 `publish-npm` job 的 *Verify artifacts* 步骤完成：9 个 `EXPECTED_TARGETS` 中任一 `.tar.gz`/`.tar.xz`（非 Windows）或 `.zip`（Windows）缺失即失败
- `scripts/npm-publish.mjs` 解包后再次 `existsSync(binDir/binary)`，缺失则中止整次发布
- 主包 + 9 个平台包版本号统一由 `updateVersion()` 改写为 `BIFROST_VERSION`，同时改写主包 `optionalDependencies` 中 `@bifrost-proxy/*` 条目的版本号
- `install.js` 的 `validateBinary` 是 best-effort：版本不匹配只打 warning，不阻塞安装；自动化 `node -c install.js` 语法检查 (planned, not yet shipped as of 2026-06-16)

## 文档更新要求

- `release.yml` 生成的 GitHub Release Notes 中 Installation 段落已包含 `npm install -g @bifrost-proxy/bifrost` 与 `npx @bifrost-proxy/bifrost start` 两条说明（见 release.yml 第 612-620 行附近）
- README / 站点文档同步 npm 安装方式 (planned, not yet shipped as of 2026-06-16)
