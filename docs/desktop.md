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
- 桌面端会在启动时检查更新，并最多每 6 小时自动检查一次；发现新版本后会显示右下角通知并打开更新窗口
- 默认数据目录为 `~/.bifrost`
- 如需改写 `config / certs / logs / runtime` 目录，可在启动前设置 `BIFROST_DATA_DIR`

### CLI 管理桌面端

`bifrost app` 管理桌面端安装包，和 `bifrost upgrade` 的 CLI 更新语义分开：

```bash
bifrost app install
bifrost app upgrade
bifrost app uninstall
```

`bifrost app upgrade` 默认安装最新桌面包；如果检测到独立安装的 CLI，也会同步把 CLI 更新到最新版本。桌面端 UI 触发更新时会使用同一套 Web UI 进度窗口，显示下载、安装和重启进度，完成后自动重启桌面应用。

如果你先安装的是桌面端，也可以在桌面 Settings 的 `Desktop Proxy Core` 卡片里点击 `Install CLI & Skills`。该按钮会把桌面端内置的 `bifrost` CLI 安装到用户命令行目录，并安装 Bifrost AI skills，方便 Codex、Claude Code、Trae、Cursor 等 AI 工具直接调用 `bifrost`。

浏览器打开的 CLI Web UI 不显示这个桌面专属按钮；只有 Tauri 桌面端会显示桌面安装、桌面更新和桌面重启相关操作。

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
