# npm 渠道分发

## 背景

Bifrost CLI 通过多种渠道分发（`install.sh`、Homebrew、GitHub Release 二进制、macOS DMG、Windows MSI），npm 渠道让 Node/前端开发者可以直接通过 `npm install -g @bifrost-proxy/bifrost` 或 `npx @bifrost-proxy/bifrost start` 使用，无需手动下载二进制或 curl 脚本。

方案参照 esbuild / turbo / SWC 的“平台包 + 主包”架构：

- 每个 target 对应一个独立平台包，只装当前 OS/CPU 的一份预编译二进制。
- 主包 `@bifrost-proxy/bifrost` 通过 `optionalDependencies` 声明所有平台包，运行时按 platform 选中对应平台包并透传参数、退出码与信号。
- Linux glibc 主机额外挂载同架构 musl/static fallback 包，覆盖旧 glibc 环境。

## 用户目标验证清单

### 必须实现

- `npm install -g @bifrost-proxy/bifrost` 完成后 `bifrost --version` 可用。
- `npx @bifrost-proxy/bifrost start` 可以直接启动主端口。
- 平台过滤有效：Mac/ARM64 上不下载 Windows / Linux 二进制，反之亦然。
- Linux glibc 主机同时安装 gnu 与 musl fallback 包；旧 glibc（< 2.39）自动 fallback 到 musl 静态包。
- 主包 CLI 透传参数与环境变量，二进制退出码原样返回，SIGINT / SIGTERM 由 `process.kill(process.pid, error.signal)` 转发。
- `--no-optional` 场景下 `install.js` 三级降级（require.resolve → 单独 `npm install` → registry tgz 直下）保证仍能落地二进制。

### 必须不破坏

- `install.sh` / Homebrew / GitHub Release DMG / MSI 渠道继续独立可用，npm 渠道不改动其它渠道产物。
- CI 的 `release.yml` 生成的 9 个 CLI target artifact（每个非 Windows 至少 `.tar.gz` + `.tar.xz`，Windows 需 `.zip`）保持齐全。
- CLI 二进制 outbound HTTP 不读取系统代理或 `HTTP_PROXY` / `HTTPS_PROXY`，npm 渠道下载不改变这一不变量。
- `bifrost upgrade` 与既有升级路径不受 npm 包发布顺序影响。

### 必须真实验证

- `publish-npm` job 在正式 release 触发时全绿。
- 至少一次 `npm install -g @bifrost-proxy/bifrost@<version>` + `bifrost --version` 真机验证。
- `npx @bifrost-proxy/bifrost start --port 18888 --no-system-proxy` 在 macOS 与 Linux 真机各验证一次。

## 产品语义

### 包结构

```
@bifrost-proxy/bifrost              # 主包 - 入口 + 平台分发逻辑
@bifrost-proxy/bifrost-linux-x64
@bifrost-proxy/bifrost-linux-arm64
@bifrost-proxy/bifrost-linux-x64-musl
@bifrost-proxy/bifrost-linux-arm64-musl
@bifrost-proxy/bifrost-linux-arm
@bifrost-proxy/bifrost-darwin-x64
@bifrost-proxy/bifrost-darwin-arm64
@bifrost-proxy/bifrost-win32-x64
@bifrost-proxy/bifrost-win32-arm64
```

### 平台映射

| 平台键 | Rust Target | npm 包名 | os | cpu | npm libc |
| --- | --- | --- | --- | --- | --- |
| linux-x64-glibc | x86_64-unknown-linux-gnu | @bifrost-proxy/bifrost-linux-x64 | linux | x64 | glibc |
| linux-arm64-glibc | aarch64-unknown-linux-gnu | @bifrost-proxy/bifrost-linux-arm64 | linux | arm64 | glibc |
| linux-x64-musl | x86_64-unknown-linux-musl | @bifrost-proxy/bifrost-linux-x64-musl | linux | x64 | — |
| linux-arm64-musl | aarch64-unknown-linux-musl | @bifrost-proxy/bifrost-linux-arm64-musl | linux | arm64 | — |
| linux-arm-glibc | armv7-unknown-linux-gnueabihf | @bifrost-proxy/bifrost-linux-arm | linux | arm | — |
| darwin-x64 | x86_64-apple-darwin | @bifrost-proxy/bifrost-darwin-x64 | darwin | x64 | — |
| darwin-arm64 | aarch64-apple-darwin | @bifrost-proxy/bifrost-darwin-arm64 | darwin | arm64 | — |
| win32-x64 | x86_64-pc-windows-msvc | @bifrost-proxy/bifrost-win32-x64 | win32 | x64 | — |
| win32-arm64 | aarch64-pc-windows-msvc | @bifrost-proxy/bifrost-win32-arm64 | win32 | arm64 | — |

Linux musl/static 包**故意不声明** `libc`，从而在 glibc 主机上也能被 npm 拉下来作为 fallback；运行时由主包 `getPlatformKey` 检测 glibc 版本再决定用哪个 npm 包名。

## 技术细节

### 主包结构

```
npm/bifrost/
├── package.json        # 主包 metadata，optionalDependencies 声明所有平台包
├── bin/bifrost         # Node.js 入口脚本
├── lib/
│   ├── index.js        # 导出 getBinaryPath()
│   └── platform.js     # PLATFORM_MAP、getPlatformKey、detectLinuxLibc、parseGlibcVersion
├── install.js          # postinstall 兜底
```

### `bin/bifrost`

- `execFileSync(getBinaryPath(), process.argv.slice(2), { stdio: 'inherit', env: process.env })`。
- 子进程非零退出 → `process.exit(error.status)`。
- 被信号中断 → `process.kill(process.pid, error.signal)`。

### `lib/index.js` 二进制查找优先级

1. `getPlatformKey({ platform, arch })` → `PLATFORM_MAP` → npm 包名；未支持 platform 抛出错误并列出全部 supported keys。
2. `require.resolve("${packageName}/package.json")` 定位平台包 → 检查 `bin/<binaryName>`（Windows: `bifrost.exe`，其他 `bifrost`）。
3. Fallback 到主包目录 `downloaded-<pkg>-<binaryName>`（由 `install.js` 兜底下载）。
4. 全部失败 → 抛 `The platform-specific package ... is not installed. Please reinstall @bifrost-proxy/bifrost to fix this.`。

### `lib/platform.js` Linux libc 检测

- `linux-arm`（armv7）直接固定为 `linux-arm-glibc`。
- 其余 Linux 平台走 `detectLinuxLibc`：
  1. `execSync("ldd --version 2>&1 || true")` 输出含 `musl` → `musl`。
  2. 否则解析 `GLIBC` / `GNU libc` `X.Y` 版本；`< MIN_GLIBC_VERSION`（当前 `2.39`）→ `musl`。
  3. 没有可解析 glibc 版本 → 检查 `/lib/ld-musl-*.so.1` 是否存在。
  4. 仍无法判定 → 默认 `musl`（musl 静态包在 glibc 系统也能跑，避免低版本 glibc 主机加载失败）。
- `detectLinuxLibc` 支持注入 `execSync`/`existsSync`/`lddOutput`，便于单测。

### `install.js` 三级兜底

1. `require.resolve("${pkg}/package.json")` 定位平台包目录，`existsSync(bin/<binary>)` 命中即 `validateBinary`。
2. 在 `__dirname/npm-install` 临时目录写空 `package.json`，`npm install --loglevel=error --prefer-offline --no-audit --progress=false <pkg>@<version>`（清空 `npm_config_global` 避免污染全局）。把 `node_modules/<pkg>/bin/<binary>` 复制到 `downloaded-<pkg>-<binary>` 并 `chmod 0o755`，最后递归清理临时目录。
3. HTTPS 从 `https://registry.npmjs.org/<pkg>/-/<tarballName>-<version>.tgz` 下载（`tarballName` 即去掉 `@bifrost-proxy/` 前缀的包名）；跟随 301/302 重定向；内置 mini tar+gzip 解析器从 `package/bin/<binary>` 解出二进制。
- `validateBinary`：`execFileSync(binPath, ["--version"])` 输出与 `package.json.version` 比对，不一致仅 warning，失败异常吞掉不阻塞。

### CI 集成（`release.yml`）

`publish-npm` job 依赖 `[prepare, build-cli, release]`，跑在 `ubuntu-latest`：

1. `actions/checkout@v4` 切到 `prepare.outputs.tag`。
2. `actions/setup-node@v4` 装 Node 22 并配置 `registry-url: https://registry.npmjs.org`。
3. `actions/download-artifact@v4 pattern: cli-*` 拉全部 CLI 产物到 `artifacts/`。
4. **Verify artifacts** 步骤遍历 9 个 `EXPECTED_TARGETS`：非 Windows 目标必须同时存在 `bifrost-<tag>-<target>.tar.gz` 与 `.tar.xz`；Windows 目标必须存在 `.zip`；缺失即 `exit 1`。
5. **Publish to npm** 执行 `node scripts/npm-publish.mjs`，注入 `BIFROST_VERSION`、`ARTIFACTS_DIR`、`NODE_AUTH_TOKEN=secrets.NPM_TOKEN`。

### `scripts/npm-publish.mjs`

- **入参**：`BIFROST_VERSION` 环境变量或第一个 positional 参数；可选 `--dry-run` / `--local` / `--token <NPM_TOKEN>` / `--otp <CODE>`；CI 模式可用 `ARTIFACTS_DIR` 覆盖产物目录。
- **CI 模式（默认）**：
  1. 同步 9 个平台包 + 主包 `package.json` 的 `version`；改写主包 `optionalDependencies` 中所有 `@bifrost-proxy/*` 条目为 `BIFROST_VERSION`。
  2. 在 `artifacts/cli-<rustTarget>/` 按 `tar.gz` → `tar.xz` → `zip` → 裸二进制 顺序找产物，解压到对应平台包 `bin/`；先试 `--strip-components=1`，找不到目标文件名再全量解压 + `find` 兜底；非 Windows 二进制 `chmod 0o755`。
  3. 根 `README.md` 复制到每个平台包与主包目录。
  4. 依次 `npm publish --access public --tag latest --registry https://registry.npmjs.org/`（`--dry-run` 透传），平台包之间间隔 `PUBLISH_INTERVAL_MS`（5 s）；平台包失败收集到 `failedPlatforms`，全部完成后若有失败 `exit 1` 不再发主包；主包发布前再等 5 s 让平台包传播。
  5. 单包最多 3 次重试（`PUBLISH_RETRY_COUNT`），仅在 `E409` 冲突时按 15 s 间隔（`PUBLISH_RETRY_DELAY_MS`）重试；其它错误立即抛出。
  6. `--token` 传入时，每个包目录写临时 `.npmrc`（`//registry.npmjs.org/:_authToken=${npm_token}`），同时设置 `npm_token` 与 `NODE_AUTH_TOKEN`。
- **`--local` 模式**：按本机 `platform`/`arch` 选一个平台包，从 `target/release/<binary>` 或 `target/debug/<binary>` 注入二进制，其余平台包写入空 stub（保证主包 `optionalDependencies` 解析成功），最后发主包；README 复制 / 版本同步 / 重试 / 节流与 CI 模式共用。CI 入口不触发 `--local`。

### CLI + Web + Admin API 边界

- npm 分发本身不新增 CLI 命令；`npx @bifrost-proxy/bifrost <subcommand>` 等价于 `bifrost <subcommand>`。
- 主包 CLI 是 Node shim，不解析业务参数，也不注入代理配置；outbound HTTP 不变量继续维持。
- Admin API 和 Web UI 不受影响，端口、路径、鉴权与常规 CLI 一致。

### Sync 边界

`bifrost sync` 走 Rust 端 HTTPS，不感知 npm 分发；npm 升级只是替换二进制，不改变 sync 协议或 rule 存储格式。

## Phase 1-4

### Phase 1：平台包骨架

- 生成 9 个 `npm/bifrost-<target>/package.json` 骨架，声明正确的 `os`/`cpu`/`bin`；Linux musl 不声明 `libc`。
- 主包 `package.json` 声明所有 `optionalDependencies`，`bin/bifrost` 指向 `lib/index.js`。

### Phase 2：主包运行时

- 实现 `lib/platform.js`（`getPlatformKey`、`detectLinuxLibc`）。
- 实现 `lib/index.js`（`getBinaryPath`）。
- 实现 `bin/bifrost`（透传参数、退出码、信号）。
- 实现 `install.js`（三级降级）。

### Phase 3：发布脚本

- `scripts/npm-publish.mjs` CI 模式、`--local` 模式、`--dry-run`、重试节流。
- `release.yml` `publish-npm` job 装配。
- Verify artifacts 步骤覆盖 9 个 target。

### Phase 4：真机验证 + 文档

- `npm install -g` / `npx` 真机跑通。
- README / 站点文档补 npm 安装段落（planned, not yet shipped as of 2026-06-16）。
- Release Notes Installation 段已包含 `npm install -g @bifrost-proxy/bifrost` 与 `npx @bifrost-proxy/bifrost start`（`release.yml` 第 612-620 行附近）。

## 测试方案

### 单元测试

- `lib/platform.js` 的 `detectLinuxLibc` / `getPlatformKey` 通过 `options.execSync` / `existsSync` / `lddOutput` 注入进行单测（planned, not yet shipped as of 2026-06-16，待补 `npm/bifrost/lib/platform.test.js`）。
- `node -c install.js` 语法检查（planned）。

### E2E / CI 校验

- `publish-npm` job 的 Verify artifacts 步骤：任一 target 缺 `.tar.gz` / `.tar.xz` / `.zip` 即 `exit 1`。
- 解包后 `existsSync(binDir/binary)` 缺失则中止发布。
- `npx @bifrost-proxy/bifrost --version` 与 `--local` 模式 dry-run。

### 真实场景测试

- 用例位于 `human_tests/npm-distribution.md`：
  - TC-NPM-01：macOS arm64 `npm install -g @bifrost-proxy/bifrost@<v>` → `bifrost --version` 与 `bifrost start --port 18888 --no-system-proxy`。
  - TC-NPM-02：Linux x64 glibc 主机验证同时安装 gnu + musl fallback 包。
  - TC-NPM-03：Linux x64 旧 glibc（< 2.39）验证运行时选中 musl 包。
  - TC-NPM-04：`npm install --no-optional @bifrost-proxy/bifrost` 触发 `install.js` 三级兜底并成功。
  - TC-NPM-05：`npm-publish.mjs --local --dry-run` 本机跑通。

### Coverage 门禁

- npm 侧目前没有自动化覆盖率；Node shim 逻辑窄，靠 `install.js` 三级路径 + 真机 TC-NPM-01/02/03/04 兜底。
- Rust 主线仍跑 `make coverage`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 9 个平台包 `os` / `cpu` / `libc` 声明与 Linux musl 特殊处理。
- 复核 `install.js` 三级路径的错误处理与临时目录清理。
- 跑 `--local --dry-run` 与 Verify artifacts。

### 第 2 轮

- 复核 `npm-publish.mjs` 重试节流、`.npmrc` 写入、README 复制。
- 真机 `npm install -g` + `npx` 双路径验证。
- 若 npm registry 409 / 502 高频，追加 retry 配置调整。

## 风险与决策

- **决策**：使用 `optionalDependencies` + `os`/`cpu` 过滤作为主路径；Linux musl 不声明 `libc` 让 glibc 主机也能拿到 fallback。
- **决策**：`install.js` 采用 best-effort validate，`--version` 校验失败仅 warning，避免误伤 airgap / mirror 场景。
- **风险**：npm registry 传播延迟可能导致主包发出去时平台包还没到；已用 5 s 间隔 + 409 重试兜底。
- **风险**：`--local` 模式会往非本机平台包塞 stub 文件，若失误跑到 CI 会污染发布；因此仅本地手动使用，CI 不触发。
- **风险**：Linux 旧 glibc 主机检测依赖 `ldd --version` 文本解析；默认策略选择 `musl` 静态包，最坏情况是二进制更大但可用。
