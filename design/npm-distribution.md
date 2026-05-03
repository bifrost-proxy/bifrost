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
- `bin/bifrost`：Node.js 入口脚本，查找并执行已安装的平台包中的二进制文件
- `lib/index.js`：平台解析逻辑，支持两种二进制路径查找（optionalDeps 安装的平台包 → postinstall 下载的二进制）
- `install.js`：`postinstall` 兜底脚本

### postinstall 兜底机制（install.js）

当 `optionalDependencies` 安装的平台包不可用时（如使用 `--no-optional`、npm 版本过旧不支持 `os`/`cpu` 过滤等），`install.js` 提供三级容错：

1. **检测平台包是否已安装**：通过 `require.resolve()` 查找对应平台包
2. **通过 npm install 单独安装**：在临时目录中执行 `npm install <平台包>@<版本号>`
3. **直接从 npm registry 下载 tgz**：通过 HTTPS 从 `registry.npmjs.org` 下载平台包 tgz 并提取二进制文件

安装完成后执行 `--version` 验证二进制版本。

### 二进制查找优先级（lib/index.js）

1. 通过 `require.resolve()` 查找 `optionalDependencies` 安装的平台包中的二进制
2. 查找 `postinstall` 下载的 `downloaded-*` 二进制文件

### 平台映射表

| Rust Target                       | npm 包名                             | os      | cpu   |
|-----------------------------------|--------------------------------------|---------|-------|
| x86_64-unknown-linux-gnu          | @bifrost-proxy/bifrost-linux-x64     | linux   | x64   |
| aarch64-unknown-linux-gnu         | @bifrost-proxy/bifrost-linux-arm64   | linux   | arm64 |
| x86_64-unknown-linux-musl         | @bifrost-proxy/bifrost-linux-x64-musl | linux  | x64   |
| aarch64-unknown-linux-musl        | @bifrost-proxy/bifrost-linux-arm64-musl | linux | arm64 |
| armv7-unknown-linux-gnueabihf     | @bifrost-proxy/bifrost-linux-arm     | linux   | arm   |
| x86_64-apple-darwin               | @bifrost-proxy/bifrost-darwin-x64    | darwin  | x64   |
| aarch64-apple-darwin              | @bifrost-proxy/bifrost-darwin-arm64  | darwin  | arm64 |
| x86_64-pc-windows-msvc            | @bifrost-proxy/bifrost-win32-x64    | win32   | x64   |
| aarch64-pc-windows-msvc           | @bifrost-proxy/bifrost-win32-arm64  | win32   | arm64 |

## 依赖项

- GitHub Actions Release workflow（已有）
- npm registry（npmjs.com）
- NPM_TOKEN secret（需配置到 GitHub repo secrets）

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

在 release.yml 的 `release` job 之后新增 `publish-npm` job：

1. 下载所有 CLI 构建产物
2. 解压二进制文件到对应平台包目录
3. 同步版本号到所有 package.json
4. 逐个执行 `npm publish` 发布平台包
5. 复制根目录 README.md 到主包目录
6. 发布主包

## 测试方案

- 本地执行 `node npm/bifrost/bin/bifrost --version` 验证二进制查找逻辑
- CI 中发布到 npm 后可通过 `npx @bifrost-proxy/bifrost --version` 验证
- 验证各包管理器只安装当前平台包：`npm install --dry-run` / 实际安装后检查 `node_modules/@bifrost-proxy/` 目录

## 校验要求

- npm publish 前验证二进制文件存在且可执行
- 版本号与 release tag 一致
- postinstall 脚本语法检查：`node -c install.js`

## 文档更新要求

- 更新 release.yml 中的 Installation 说明，新增 npm 安装方式
