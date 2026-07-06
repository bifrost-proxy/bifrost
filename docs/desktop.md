# 桌面版安装与构建

桌面版基于 Tauri 构建，安装包内已包含 Web 资源，并会在应用内部启动打包进去的 `bifrost` CLI 后端。

## 安装方式

### 先选择正确安装包

打开 [Releases](https://github.com/bifrost-proxy/bifrost/releases) 后，优先选择最新版本的 `Assets`。桌面端安装包文件名包含平台和架构：

| 系统 | 该选哪个文件 | 适用设备 |
| --- | --- | --- |
| macOS Apple Silicon | `bifrost-desktop-vX.X.X-aarch64-apple-darwin.dmg` | M1/M2/M3/M4 等 Apple Silicon Mac |
| macOS Intel | `bifrost-desktop-vX.X.X-x86_64-apple-darwin.dmg` | Intel Mac |
| Windows x64 | `bifrost-desktop-vX.X.X-x86_64-pc-windows-msvc.msi` | 大多数 Windows 笔记本和台式机 |
| Windows ARM64 | `bifrost-desktop-vX.X.X-aarch64-pc-windows-msvc.msi` | Windows on ARM 设备 |

如果只是想使用命令行，不需要安装桌面端，直接使用 [`getting-started.md`](./getting-started.md) 中的 CLI 安装方式。

### Homebrew Cask（macOS）

由于当前尚未签名，如需通过 Homebrew 安装：

```bash
brew tap bifrost-proxy/bifrost
brew install --cask bifrost-desktop
```

如果不希望使用 Cask，推荐直接从源码执行 `./install.sh`，或使用 CLI 版本。

### 手动下载安装包

可从 [Releases](https://github.com/bifrost-proxy/bifrost/releases) 下载桌面安装包。

#### macOS `.dmg`

1. 下载与你芯片匹配的 `.dmg`。
2. 双击打开 `.dmg`。
3. 将 `Bifrost.app` 拖入 `Applications`。
4. 从 `Applications`、Launchpad 或 Spotlight 启动 Bifrost。
5. 如果 macOS 提示应用来自互联网下载，确认打开；如果未签名版本被 Gatekeeper 拦截，到 `System Settings -> Privacy & Security` 中允许打开。

#### Windows `.msi`

1. 下载与你设备架构匹配的 `.msi`。
2. 双击 `.msi`，按安装向导完成安装。
3. 从 Start Menu 启动 `Bifrost`。
4. 如果 Windows SmartScreen 提示未知发布者，确认安装包来自官方 GitHub Releases 后再继续。

## 首次启动

安装完成后：

- macOS 启动 `Bifrost.app`。
- Windows 从 Start Menu 启动 `Bifrost`。
- 桌面端会在应用内部启动打包的 `bifrost` 后端，不需要先手动执行 `bifrost start`。
- 默认数据目录为 `~/.bifrost`，桌面端和 CLI 默认共享这份配置、证书、日志和运行时状态。
- 桌面端首次启动会异步检查并安装 CA 证书；需要 HTTPS 抓包时，先确认系统已信任 Bifrost CA。
- 桌面端会在启动时检查更新，并最多每 6 小时自动检查一次；发现新版本后会显示右下角通知并打开更新窗口。

如需改写 `config / certs / logs / runtime` 目录，可在启动前设置 `BIFROST_DATA_DIR`。普通用户日常使用不建议改写数据目录，除非你要隔离测试环境或调试多实例。

## 桌面端与 CLI 的关系

桌面端内置一个 `bifrost` 后端，用于支撑当前 App 的代理、规则、流量和设置页面。你仍然可以单独安装 CLI，用于终端、脚本、CI 或 AI coding tools。

如果你先安装的是桌面端，可以在桌面 Settings 的 `Desktop Proxy Core` 卡片里点击 `Install CLI & Skills`。该按钮会：

- 把桌面端内置的 `bifrost` CLI 安装到用户命令行目录。
- 安装 Bifrost AI skills，方便 Codex、Claude Code、Trae、Cursor 等 AI 工具调用 `bifrost`。
- 保持桌面端更新与 CLI 更新的边界清晰：桌面端使用 `bifrost app upgrade`，CLI 使用 `bifrost upgrade`。

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

## 更新与卸载

桌面端会自动检查更新。也可以使用 CLI 手动管理桌面端：

```bash
bifrost app upgrade
bifrost app uninstall
```

源码仓库中也提供统一卸载脚本：

```bash
# 卸载 CLI 和桌面应用
./uninstall.sh

# 连同数据一起清理
./uninstall.sh --purge
```

`--purge` 会删除本地数据目录，包含规则、证书、日志和运行时状态。执行前请确认不需要保留现有配置。

## 常见问题

### macOS 提示无法打开或开发者无法验证

当前未签名构建可能触发 Gatekeeper。确认安装包来自官方 GitHub Releases 后，到 `System Settings -> Privacy & Security` 中允许打开。不要从未知镜像站下载安装包。

### Windows SmartScreen 提示未知发布者

确认文件来自官方 GitHub Releases，且文件名与目标架构匹配。企业环境中如果策略禁止未签名安装包，需要请管理员放行。

### 启动后看不到流量

确认桌面端后端已经启动，再检查系统代理、目标 App 是否走系统代理，以及是否需要开启 TLS 拦截。浏览器或 CLI 工具单独配置代理时，代理地址通常是 `127.0.0.1:9900`。

### HTTPS 抓包失败

先确认 Bifrost CA 已安装并被系统信任。部分 App 使用证书固定或自定义 TLS 栈，无法解密时应使用应用排除、域名排除或规则级 `tlsPassthrough://`，不要强行全局拦截。

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
