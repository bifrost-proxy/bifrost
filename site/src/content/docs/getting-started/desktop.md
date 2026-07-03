---
title: "桌面版安装与构建"
description: "桌面版安装方式、本地构建步骤与常见注意事项。"
editLink: false
---

> 此页面由 `docs/desktop.md` 自动同步生成。
# 桌面版安装与构建

桌面版基于 Tauri 构建，安装包内已包含 Web 资源，并会在应用内部启动打包进去的 `bifrost` CLI 后端。

## 安装方式

### Homebrew Cask（macOS）

由于当前尚未签名，如需通过 Homebrew 安装：

```bash
brew tap bifrost-proxy/bifrost
brew install --cask bifrost-desktop
```

如果不希望使用 Cask，推荐直接从源码执行 `./install.sh`，或使用 CLI 版本。

### 手动下载安装包

可从 [Releases](https://github.com/bifrost-proxy/bifrost/releases) 下载桌面安装包。

当前发布计划：

| 平台 | 架构 | 文件 |
| --- | --- | --- |
| macOS | Intel | `bifrost-desktop-vX.X.X-x86_64-apple-darwin.dmg` |
| macOS | Apple Silicon | `bifrost-desktop-vX.X.X-aarch64-apple-darwin.dmg` |
| Windows | x64 | `bifrost-desktop-vX.X.X-x86_64-pc-windows-msvc.msi` |
| Windows | ARM64 | `bifrost-desktop-vX.X.X-aarch64-pc-windows-msvc.msi` |

安装完成后：

- macOS 直接启动 `Bifrost.app`
- Windows 通过 `.msi` 安装后从开始菜单启动 `Bifrost`
- 桌面端首次启动会异步检查并安装 CA 证书
- 默认数据目录为 `~/.bifrost`
- 如需改写 `config / certs / logs / runtime` 目录，可在启动前设置 `BIFROST_DATA_DIR`

## 从源码构建

### 使用安装脚本

在 macOS 上执行：

```bash
./install.sh
```

默认行为：

- 安装 `bifrost` CLI 到 `~/.local/bin`
- 构建并安装 `Bifrost.app` 到 `/Applications/Bifrost.app`

可选参数：

```bash
./install.sh --cli-only
./install.sh --desktop-only
./install.sh --app-dir ~/Applications
```

### 手动构建

```bash
git clone https://github.com/bifrost-proxy/bifrost.git
cd bifrost

pnpm install
cd web && pnpm install && cd ..
pnpm run desktop:build

# 仅构建 macOS .app
pnpm run desktop:build:app
```

产物位置：

- macOS `.dmg`：`desktop/src-tauri/target/release/bundle/dmg/`
- macOS `.app`：`desktop/src-tauri/target/release/bundle/macos/`
- Windows `.msi`：`desktop/src-tauri/target/release/bundle/msi/`
